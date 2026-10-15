//! AppPort knowledge: the agent's native, deterministic understanding of AppPort.
//!
//! Derived from the published `@appport/*` packages, not hand-maintained from
//! memory: `tools/appport-inspect` reads the packages' type definitions and
//! runtime values into `knowledge/appport-surface.json` and generates fixtures,
//! and this crate's tests fail when the Rust model disagrees with either. See
//! `docs/appport-knowledge.md` for the type-by-type mapping.
//!
//! The module has no runtime dependency on Node or npm, and no model is
//! involved anywhere in it. The model is a consumer: it receives the small,
//! serialized slices this module produces ([`CapabilityView`],
//! [`ReplacementBoundary`], [`Graph`], [`Issue`]) and is never asked to supply
//! structural truth.
//!
//! The entry point is [`Contract`].

mod capability;
pub mod constants;
mod contract;
mod graph;
mod inspect;
mod integration;
mod literal;
mod replacement;
pub mod rules;
mod schema;
mod validate;
mod verification;

use std::fmt;

use serde::Serialize;
use serde_json::Value;

pub use capability::*;
pub use contract::*;
pub use graph::{Edge, EdgeKind, Graph, Node, NodeKind, Unresolved};
pub use inspect::{fields, inspect_capability, CapabilityView, EvidenceRequirements, FieldView, MethodView};
pub use integration::*;
pub use literal::*;
pub use replacement::*;
pub use schema::*;
pub use validate::{validate_manifest, Issue};
pub use verification::*;

/// Why a document could not be read as an AppPort contract.
#[derive(Debug)]
pub enum KnowledgeError {
    /// Not JSON, or not shaped like an `AppPortManifest`.
    Manifest(serde_json::Error),
}

impl fmt::Display for KnowledgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KnowledgeError::Manifest(e) => write!(f, "not an AppPort manifest: {e}"),
        }
    }
}

impl std::error::Error for KnowledgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KnowledgeError::Manifest(e) => Some(e),
        }
    }
}

/// An application contract the agent has observed: a parsed manifest plus its
/// integration descriptor, with the original document kept for exact digests.
#[derive(Debug, Clone)]
pub struct Contract {
    raw: Value,
    manifest: Manifest,
    integration: Option<IntegrationDescriptor>,
    integration_error: Option<String>,
}

impl Contract {
    /// Parses a manifest document.
    pub fn from_json(text: &str) -> Result<Self, KnowledgeError> {
        Self::from_value(serde_json::from_str(text).map_err(KnowledgeError::Manifest)?)
    }

    /// Parses a manifest already read as JSON.
    pub fn from_value(raw: Value) -> Result<Self, KnowledgeError> {
        let manifest: Manifest = serde_json::from_value(raw.clone()).map_err(KnowledgeError::Manifest)?;
        let (integration, integration_error) = match manifest.integration() {
            Ok(descriptor) => (descriptor, None),
            Err(error) => (None, Some(error.to_string())),
        };
        Ok(Self { raw, manifest, integration, integration_error })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The integration descriptor, when published and well-formed.
    pub fn integration(&self) -> Option<&IntegrationDescriptor> {
        self.integration.as_ref()
    }

    /// The original document.
    pub fn raw(&self) -> &Value {
        &self.raw
    }

    /// `contractDigest`: identity of what the application promises.
    pub fn digest(&self) -> String {
        rules::contract_digest_of_value(&self.raw)
    }

    /// Inspects one capability version.
    pub fn inspect(&self, name: &str, version: u32) -> Option<CapabilityView> {
        inspect_capability(&self.manifest, self.integration(), name, version)
    }

    /// Inspects every capability version, in manifest order.
    pub fn inspect_all(&self) -> Vec<CapabilityView> {
        self.manifest
            .capability_versions()
            .filter_map(|(entry, definition)| self.inspect(&entry.name, definition.version))
            .collect()
    }

    /// The declared integration graph.
    pub fn graph(&self) -> Graph {
        Graph::derive(&self.manifest, self.integration())
    }

    /// What a replacement of `name@version` must respect.
    pub fn replacement_boundary(&self, name: &str, version: u32) -> Option<ReplacementBoundary> {
        ReplacementBoundary::derive(&self.manifest, self.integration(), &self.graph(), name, version)
    }

    /// Structural validation, plus a check that the typed model captured the whole
    /// document: any member the model does not know is reported as `model.unmodeled`,
    /// which is how a contract from a newer AppPort shows up at runtime.
    pub fn validate(&self) -> Vec<Issue> {
        let mut issues = validate_manifest(&self.manifest);
        if let Some(error) = &self.integration_error {
            if !issues.iter().any(|i| i.rule == "integration.parse") {
                issues.push(Issue {
                    level: IssueLevel::Error,
                    rule: "integration.parse".into(),
                    path: format!("attributes[\"{}\"]", constants::INTEGRATION_EXTENSION),
                    message: error.clone(),
                });
            }
        }
        for pointer in self.unmodeled() {
            issues.push(Issue {
                level: IssueLevel::Warning,
                rule: "model.unmodeled".into(),
                path: pointer,
                message: "present in the document but not in the AppPort knowledge model".into(),
            });
        }
        issues
    }

    /// JSON Pointers of members present in the document that the typed model dropped.
    pub fn unmodeled(&self) -> Vec<String> {
        let Ok(typed) = serde_json::to_value(&self.manifest) else {
            return vec![String::new()];
        };
        let mut out = Vec::new();
        missing(&self.raw, &typed, "", &mut out);
        // The integration descriptor lives in free-form attributes; check its typed form too.
        if let (Some(descriptor), Some(raw)) = (
            &self.integration,
            self.raw.pointer(&format!("/attributes/{}", constants::INTEGRATION_EXTENSION.replace('/', "~1"))),
        ) {
            if let Ok(typed) = serde_json::to_value(descriptor) {
                let base = format!("/attributes/{}", constants::INTEGRATION_EXTENSION.replace('/', "~1"));
                missing(raw, &typed, &base, &mut out);
            }
        }
        out
    }
}

fn missing(raw: &Value, typed: &Value, pointer: &str, out: &mut Vec<String>) {
    match (raw, typed) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                let child = format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"));
                match b.get(key) {
                    Some(other) => missing(value, other, &child, out),
                    None => out.push(child),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                missing(x, y, &format!("{pointer}/{index}"), out);
            }
        }
        _ => {}
    }
}

/// Compact JSON for handing a knowledge slice to a model: stable member order,
/// no insignificant whitespace, absent fields omitted.
pub fn model_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("knowledge types always serialize")
}
