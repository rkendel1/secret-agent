//! The declared integration graph of one application.
//!
//! Built deterministically from a single manifest's integration descriptor:
//! dependencies (`requires`), data flows (`access`, `emits`), and the probes its
//! verification methods and preconditions call. It is the graph the *contract
//! declares*, not one discovered from a running system — live, multi-application
//! graph construction is deliberately out of scope here.
//!
//! References to another application cannot be resolved from one manifest; they
//! become nodes marked `resolved: false` and are listed in [`Graph::unresolved`],
//! matching how `lintDependencyModel` treats them.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::contract::Manifest;
use super::integration::{
    AccessMode, Dependency, DependencyRelationship, DependencyTarget, IntegrationDescriptor, ResourceOperation,
    ResourceOwner,
};
use super::rules::{parse_dependency_ref, satisfies_version_constraint};
use super::verification::VerificationMethod;

/// What a node stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Application,
    Capability,
    Event,
    Resource,
}

/// A node. `id` is stable and readable:
/// `application:<app>`, `capability:<app>/<name>@<version>` (or `<name>` when unpinned),
/// `event:<app>/<name>`, `resource:<iri>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    /// The application the node belongs to, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    /// False when the node is only referenced, not declared, by this manifest.
    pub resolved: bool,
}

/// How two nodes relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Invokes,
    Observes,
    Delegates,
    ConsumesEvent,
    /// Capability → event it may emit.
    Emits,
    /// Capability → resource it reads.
    Reads,
    /// Capability → resource it changes.
    Changes,
    /// Capability → observation capability one of its verification methods probes.
    VerifiedBy,
    /// Capability → observation capability one of its preconditions probes.
    CheckedBy,
    /// Application → resource it is the system of record for.
    Owns,
    /// Application → capability it declares.
    Declares,
}

impl From<DependencyRelationship> for EdgeKind {
    fn from(relationship: DependencyRelationship) -> Self {
        match relationship {
            DependencyRelationship::Invokes => EdgeKind::Invokes,
            DependencyRelationship::Observes => EdgeKind::Observes,
            DependencyRelationship::Delegates => EdgeKind::Delegates,
            DependencyRelationship::ConsumesEvent => EdgeKind::ConsumesEvent,
        }
    }
}

/// A directed edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    /// For dependencies: whether the dependent can do without the target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// The declaring dependency, verification method or precondition id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// For `changes`: the declared operations.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub operations: Vec<ResourceOperation>,
}

/// A reference the graph could not resolve from this manifest alone.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct Unresolved {
    pub from: String,
    pub target: String,
    pub reason: String,
}

/// The declared graph. Nodes and edges are sorted, so equal contracts give equal graphs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Graph {
    pub application: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub unresolved: Vec<Unresolved>,
}

/// Node id helpers.
pub mod id {
    pub fn application(app: &str) -> String {
        format!("application:{app}")
    }
    pub fn capability(app: &str, name: &str, version: u32) -> String {
        format!("capability:{app}/{name}@{version}")
    }
    pub fn capability_ref(app: &str, reference: &str) -> String {
        format!("capability:{app}/{reference}")
    }
    pub fn event(app: &str, name: &str) -> String {
        format!("event:{app}/{name}")
    }
    pub fn resource(iri: &str) -> String {
        format!("resource:{iri}")
    }
}

impl Graph {
    /// Derives the declared graph from a manifest and its (already parsed) integration descriptor.
    pub fn derive(manifest: &Manifest, integration: Option<&IntegrationDescriptor>) -> Graph {
        let mut builder = Builder::new(&manifest.application.id);
        let app = manifest.application.id.as_str();
        let app_node = id::application(app);
        builder.node(&app_node, NodeKind::Application, Some(app), true);

        for (entry, definition) in manifest.capability_versions() {
            let node = id::capability(app, &entry.name, definition.version);
            builder.node(&node, NodeKind::Capability, Some(app), true);
            builder.edge(&app_node, &node, EdgeKind::Declares, None, None, Vec::new());
            for event in definition.emitted_events() {
                let target = id::event(app, event);
                let declared = manifest.event(event).is_some();
                builder.node(&target, NodeKind::Event, Some(app), declared);
                if !declared {
                    builder.unresolved(&node, &target, "emits an event this manifest does not declare");
                }
                builder.edge(&node, &target, EdgeKind::Emits, None, None, Vec::new());
            }
        }
        for event in &manifest.events {
            builder.node(&id::event(app, &event.name), NodeKind::Event, Some(app), true);
        }

        let Some(integration) = integration else {
            return builder.finish();
        };

        for resource in integration.resources.as_deref().unwrap_or_default() {
            let node = id::resource(&resource.id);
            let owner = match &resource.owner {
                Some(ResourceOwner::This(_)) => Some(app.to_string()),
                Some(ResourceOwner::Application(a)) => Some(a.application.clone()),
                None => None,
            };
            builder.node(&node, NodeKind::Resource, owner.as_deref(), true);
            if let Some(owner) = owner {
                let owner_node = id::application(&owner);
                builder.node(&owner_node, NodeKind::Application, Some(&owner), owner == app);
                builder.edge(&owner_node, &node, EdgeKind::Owns, None, None, Vec::new());
            }
        }

        for dependency in integration.requires.as_deref().unwrap_or_default() {
            builder.dependency(manifest, &app_node, dependency);
        }

        for (key, entry) in &integration.capabilities {
            let Some(reference) = parse_dependency_ref(key) else {
                builder.unresolved(&app_node, key, "integration key is not name@version");
                continue;
            };
            let from = match reference.version {
                Some(version) if manifest.capability_version(&reference.name, version).is_some() => {
                    id::capability(app, &reference.name, version)
                }
                _ => {
                    builder.unresolved(
                        &app_node,
                        key,
                        "integration entry for a capability version this manifest does not declare",
                    );
                    continue;
                }
            };
            for dependency in entry.requires.as_deref().unwrap_or_default() {
                builder.dependency(manifest, &from, dependency);
            }
            for access in entry.access.as_deref().unwrap_or_default() {
                let target = id::resource(&access.resource);
                if integration.resource(&access.resource).is_none() {
                    builder.node(&target, NodeKind::Resource, None, false);
                    builder.unresolved(&from, &target, "accesses a resource the descriptor does not declare");
                }
                let kind = match access.mode {
                    AccessMode::Reads => EdgeKind::Reads,
                    AccessMode::Changes => EdgeKind::Changes,
                };
                builder.edge(&from, &target, kind, None, None, access.operations.clone().unwrap_or_default());
            }
            for method in entry.verify.as_deref().unwrap_or_default() {
                if let VerificationMethod::State(state) = method {
                    builder.probe(
                        manifest,
                        &from,
                        &state.via.capability,
                        state.via.version,
                        EdgeKind::VerifiedBy,
                        &state.id,
                    );
                }
            }
            for precondition in entry.preconditions.as_deref().unwrap_or_default() {
                let via = &precondition.via;
                builder.probe(manifest, &from, &via.capability, via.version, EdgeKind::CheckedBy, &precondition.id);
            }
        }
        builder.finish()
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Edges pointing at `id`.
    pub fn incoming<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Edge> + 'a {
        self.edges.iter().filter(move |e| e.to == id)
    }

    /// Edges leaving `id`.
    pub fn outgoing<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Edge> + 'a {
        self.edges.iter().filter(move |e| e.from == id)
    }
}

struct Builder {
    application: String,
    nodes: BTreeMap<String, Node>,
    edges: BTreeSet<Edge>,
    unresolved: BTreeSet<Unresolved>,
}

impl Builder {
    fn new(application: &str) -> Self {
        Self {
            application: application.to_string(),
            nodes: BTreeMap::new(),
            edges: BTreeSet::new(),
            unresolved: BTreeSet::new(),
        }
    }

    fn node(&mut self, id: &str, kind: NodeKind, application: Option<&str>, resolved: bool) {
        let node = self.nodes.entry(id.to_string()).or_insert_with(|| Node {
            id: id.to_string(),
            kind,
            application: application.map(str::to_string),
            resolved,
        });
        // A node declared anywhere is resolved.
        node.resolved |= resolved;
        if node.application.is_none() {
            node.application = application.map(str::to_string);
        }
    }

    fn edge(
        &mut self,
        from: &str,
        to: &str,
        kind: EdgeKind,
        required: Option<bool>,
        via: Option<&str>,
        operations: Vec<ResourceOperation>,
    ) {
        self.edges.insert(Edge {
            from: from.into(),
            to: to.into(),
            kind,
            required,
            via: via.map(str::to_string),
            operations,
        });
    }

    fn unresolved(&mut self, from: &str, target: &str, reason: &str) {
        self.unresolved.insert(Unresolved { from: from.into(), target: target.into(), reason: reason.into() });
    }

    fn dependency(&mut self, manifest: &Manifest, from: &str, dependency: &Dependency) {
        let kind = EdgeKind::from(dependency.relationship);
        let required = Some(dependency.required);
        let via = Some(dependency.id.as_str());
        let local = dependency.target.application().map_or(true, |a| a == self.application);
        let owner = dependency.target.application().unwrap_or(&self.application).to_string();
        match &dependency.target {
            DependencyTarget::Event(target) => {
                let node = id::event(&owner, &target.event);
                let declared = local && manifest.event(&target.event).is_some();
                self.node(&node, NodeKind::Event, Some(&owner), declared);
                if !declared {
                    let reason = if local {
                        "event is not declared by this application"
                    } else {
                        "event of another application"
                    };
                    self.unresolved(from, &node, reason);
                }
                self.edge(from, &node, kind, required, via, Vec::new());
            }
            DependencyTarget::Capability(target) => {
                let Some(reference) = parse_dependency_ref(&target.capability) else {
                    self.unresolved(from, &target.capability, "dependency target is not name or name@version");
                    return;
                };
                if !local {
                    let node = id::capability_ref(&owner, &target.capability);
                    self.node(&node, NodeKind::Capability, Some(&owner), false);
                    self.unresolved(from, &node, "capability of another application");
                    self.edge(from, &node, kind, required, via, Vec::new());
                    return;
                }
                let candidates: Vec<u32> = manifest
                    .capability(&reference.name)
                    .map(|entry| {
                        entry
                            .definitions
                            .iter()
                            .map(|d| d.version)
                            .filter(|v| match (reference.version, &dependency.version_constraint) {
                                (Some(pinned), _) => *v == pinned,
                                (None, Some(constraint)) => satisfies_version_constraint(*v, constraint),
                                (None, None) => false,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if candidates.is_empty() {
                    let node = id::capability_ref(&owner, &target.capability);
                    self.node(&node, NodeKind::Capability, Some(&owner), false);
                    self.unresolved(from, &node, "no declared capability version matches");
                    self.edge(from, &node, kind, required, via, Vec::new());
                }
                for version in candidates {
                    let node = id::capability(&owner, &reference.name, version);
                    self.edge(from, &node, kind, required, via, Vec::new());
                }
            }
        }
    }

    fn probe(&mut self, manifest: &Manifest, from: &str, capability: &str, version: u32, kind: EdgeKind, via: &str) {
        let app = self.application.clone();
        let node = id::capability(&app, capability, version);
        if manifest.capability_version(capability, version).is_none() {
            self.node(&node, NodeKind::Capability, Some(&app), false);
            self.unresolved(from, &node, "probe capability is not declared by this application");
        }
        self.edge(from, &node, kind, None, Some(via), Vec::new());
    }

    fn finish(self) -> Graph {
        Graph {
            application: self.application,
            nodes: self.nodes.into_values().collect(),
            edges: self.edges.into_iter().collect(),
            unresolved: self.unresolved.into_iter().collect(),
        }
    }
}
