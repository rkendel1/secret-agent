//! The deterministic action planner.
//!
//! Given an intent ("replace `customers.lookup@1` while preserving its contract"),
//! the observed contract and the observed project, the planner decides — without
//! a model — what may change, what must not, how the result will be judged, and
//! the minimal context a model needs for the one creative step: writing the new
//! implementation.
//!
//! Every structural decision is recorded as a [`Decision`] with the fact it rests
//! on, so the run log shows where each conclusion came from.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;
use serde_json::Value;

use crate::appport::knowledge::{
    fields, model_json, CapabilityEffect, CapabilityKind, CapabilityView, Contract, EdgeKind, NodeKind,
    ReplacementBoundary, VerificationMethod,
};
use crate::appport::knowledge::{CapabilityId, Graph};
use crate::observe::Project;

/// Capabilities every AppPort runtime registers itself; never targets or probes.
pub const BUILTIN_PREFIX: &str = "appport.";

/// What the agent was asked to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Intent {
    pub capability: String,
    pub version: u32,
    /// Free text: what the new implementation should achieve.
    pub goal: String,
}

impl Intent {
    /// Parses `replace <name>@<version>[: <goal>]`.
    pub fn parse(text: &str) -> Result<Intent, String> {
        let text = text.trim();
        let rest = text
            .strip_prefix("replace ")
            .ok_or_else(|| format!("unsupported intent {text:?}: expected `replace <name>@<version>[: <goal>]`"))?;
        let (reference, goal) = rest.split_once(':').map_or((rest, ""), |(r, g)| (r, g));
        let (capability, version) = reference
            .trim()
            .rsplit_once('@')
            .and_then(|(n, v)| Some((n.to_string(), v.parse::<u32>().ok()?)))
            .ok_or_else(|| format!("{:?} is not name@version", reference.trim()))?;
        let goal = match goal.trim() {
            "" => "provide a new implementation that preserves the capability's contract".to_string(),
            goal => goal.to_string(),
        };
        Ok(Intent { capability, version, goal })
    }
}

/// One deterministic conclusion and the fact it rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub step: &'static str,
    pub fact: String,
}

/// A project file handed to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextFile {
    pub path: String,
    pub editable: bool,
    pub reason: String,
}

/// A capability whose declared verification methods will be executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Flow {
    pub capability: CapabilityId,
    pub methods: Vec<String>,
    pub reason: String,
}

/// What the agent will run against the application, before and after the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ProbePlan {
    /// Observation capabilities with no required input whose outputs carry
    /// concept-annotated values: they discover existing state to probe with.
    pub enumerators: Vec<CapabilityId>,
    /// Observation capabilities whose answers on existing state must not change.
    pub regression: Vec<CapabilityId>,
    /// Capabilities whose declared verification methods must confirm afterwards.
    pub flows: Vec<Flow>,
}

/// An acceptance criterion. All must pass for a change to be accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckSpec {
    pub code: String,
    pub rule: String,
    /// Where the criterion comes from.
    pub source: String,
    /// The capability a per-capability check is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<CapabilityId>,
}

/// The plan for one intent.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub intent: Intent,
    pub application: String,
    pub target: CapabilityId,
    /// `contractDigest` of the observed contract: an implementation replacement keeps it.
    pub contract_digest: String,
    pub view: CapabilityView,
    pub boundary: ReplacementBoundary,
    pub context: Vec<ContextFile>,
    /// Permissions the verification session needs, from the contract. Nothing more is granted.
    pub permissions: Vec<String>,
    pub probes: ProbePlan,
    pub checks: Vec<CheckSpec>,
    pub decisions: Vec<Decision>,
}

impl Plan {
    /// Files the change may touch.
    pub fn editable(&self) -> Vec<&str> {
        self.context.iter().filter(|f| f.editable).map(|f| f.path.as_str()).collect()
    }
}

/// Why no plan could be made. The agent stops; it does not ask the model to fill gaps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Refusal {}

fn refuse(code: &str, message: impl Into<String>) -> Refusal {
    Refusal { code: code.into(), message: message.into() }
}

/// Budget for read-only context handed to the model.
const CONTEXT_BUDGET_BYTES: usize = 48 * 1024;

/// Plans `intent` against the observed contract and project.
pub fn plan(intent: &Intent, contract: &Contract, project: &Project) -> Result<Plan, Refusal> {
    let mut decisions = Vec::new();
    let mut decide = |step: &'static str, fact: String| decisions.push(Decision { step, fact });
    let manifest = contract.manifest();
    let application = manifest.application.id.clone();
    let key = format!("{}@{}", intent.capability, intent.version);

    let errors: Vec<String> =
        contract.validate().into_iter().filter(|i| i.is_error()).map(|i| format!("{} at {}", i.rule, i.path)).collect();
    if !errors.is_empty() {
        return Err(refuse("contract.invalid", format!("the observed contract has errors: {}", errors.join("; "))));
    }
    decide("contract", format!("observed contract of {application} is structurally valid"));

    if intent.capability.starts_with(BUILTIN_PREFIX) {
        return Err(refuse("target.builtin", format!("{key} is provided by the AppPort runtime itself")));
    }
    let view = contract
        .inspect(&intent.capability, intent.version)
        .ok_or_else(|| refuse("target.undeclared", format!("the contract does not declare {key}")))?;
    let boundary = contract
        .replacement_boundary(&intent.capability, intent.version)
        .ok_or_else(|| refuse("target.undeclared", format!("the contract does not declare {key}")))?;
    if view.kind != CapabilityKind::Request {
        return Err(refuse(
            "target.stream",
            format!("{key} is a stream capability; only request capabilities are supported"),
        ));
    }
    decide(
        "target",
        format!(
            "{key}: effect {}, {} input field(s), {} output field(s), {} declared failure(s), {} replacement constraint(s)",
            view.effect.map_or("unknown".to_string(), |e| format!("{e:?}").to_lowercase()),
            view.inputs.len(),
            view.outputs.len(),
            view.failures.len(),
            boundary.constraints.len()
        ),
    );

    // Where the implementation lives.
    let declaration = project.capability(&intent.capability, intent.version).ok_or_else(|| {
        refuse("source.undeclared", format!("no defineCapability call for {key} was found in the project"))
    })?;
    let handler = declaration.handler.as_ref().ok_or_else(|| {
        refuse(
            "source.handler",
            format!("the handler of {key} (declared in {}) could not be located", declaration.declared_in),
        )
    })?;
    decide(
        "source",
        format!(
            "{key} is declared in {}:{} and implemented in {}{}",
            declaration.declared_in,
            declaration.line,
            handler.module,
            handler.symbol.as_ref().map(|s| format!("#{s}")).unwrap_or_default()
        ),
    );
    if handler.module == declaration.declared_in {
        decide(
            "source",
            format!(
                "the handler is inline in the contract module {}; the contract identity check guards it",
                handler.module
            ),
        );
    }

    let graph = contract.graph();
    let id = |name: &str, version: u32| CapabilityId { name: name.to_string(), version };
    let target = id(&intent.capability, intent.version);
    let local = |node: &str| parse_capability_node(&application, node);

    // Who is affected: through the contract, and through the code.
    let contract_dependents: BTreeSet<CapabilityId> =
        boundary.dependents.iter().filter_map(|d| local(&d.node)).filter(|c| *c != target).collect();
    for dependent in &boundary.dependents {
        decide("graph", format!("{} --{}--> {key}", dependent.node, kebab(dependent.relationship)));
    }
    let code_reach: BTreeSet<CapabilityId> = project
        .capabilities_reaching(&handler.module)
        .into_iter()
        .map(|c| id(&c.name, c.version))
        .filter(|c| *c != target)
        .collect();
    for reached in &code_reach {
        decide(
            "code",
            format!("{}@{} imports {} (directly or transitively)", reached.name, reached.version, handler.module),
        );
    }

    let definition = |c: &CapabilityId| manifest.capability_version(&c.name, c.version);
    let is_observation =
        |c: &CapabilityId| definition(c).is_some_and(|d| d.effect == Some(CapabilityEffect::Observation));

    // Regression: observations whose answers on existing state must not change.
    let mut regression = Vec::new();
    for c in std::iter::once(&target).chain(contract_dependents.iter()).chain(code_reach.iter()) {
        if regression.contains(c) {
            continue;
        }
        if is_observation(c) {
            regression.push(c.clone());
        } else {
            decide(
                "probes",
                format!("{}@{} is not an observation: not probed for regression on existing state", c.name, c.version),
            );
        }
    }

    // Flows: declared verification methods that rely on the target or may change with it.
    let integration = contract.integration();
    let mut flows = Vec::new();
    for (entry, def) in manifest.capability_versions() {
        let c = id(&entry.name, def.version);
        let Some(methods) = integration.and_then(|i| i.capability(&c.name, c.version)).and_then(|e| e.verify.as_ref())
        else {
            continue;
        };
        let probes_target = methods.iter().any(|m| match m {
            VerificationMethod::State(s) => s.via.capability == target.name && s.via.version == target.version,
            VerificationMethod::Event(_) => false,
        });
        let reason = if c == target {
            "the target's own declared verification"
        } else if probes_target {
            "its declared state verification re-reads through the target"
        } else if code_reach.contains(&c) {
            "its implementation imports the target's module"
        } else {
            continue;
        };
        decide("probes", format!("verify {}@{} ({reason})", c.name, c.version));
        flows.push(Flow {
            capability: c,
            methods: methods.iter().map(|m| m.id().to_string()).collect(),
            reason: reason.into(),
        });
    }
    for edge in graph.incoming(&node_id(&application, &target)).filter(|e| e.kind == EdgeKind::CheckedBy) {
        decide(
            "probes",
            format!(
                "{} checks a precondition through the target ({}); preconditions are not executed",
                edge.from,
                edge.via.clone().unwrap_or_default()
            ),
        );
    }

    // Enumerators: discover existing state, keyed by semantic concept.
    let needed: BTreeSet<String> = regression
        .iter()
        .filter_map(&definition)
        .flat_map(|d| fields(&d.input_schema))
        .filter_map(|f| f.concept)
        .collect();
    let mut enumerators = Vec::new();
    for (entry, def) in manifest.capability_versions() {
        if entry.name.starts_with(BUILTIN_PREFIX)
            || def.effect != Some(CapabilityEffect::Observation)
            || def.kind != CapabilityKind::Request
            || def.is_deprecated()
            || fields(&def.input_schema).iter().any(|f| f.required)
        {
            continue;
        }
        let provides: BTreeSet<String> = fields(&def.output_schema).into_iter().filter_map(|f| f.concept).collect();
        let shared: Vec<&String> = provides.intersection(&needed).collect();
        if !shared.is_empty() {
            decide(
                "probes",
                format!(
                    "{}@{} needs no input and returns {}: it enumerates existing state",
                    entry.name,
                    def.version,
                    shared.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ),
            );
            enumerators.push(id(&entry.name, def.version));
        }
    }
    if enumerators.is_empty() && !regression.is_empty() {
        decide(
            "probes",
            "no enumerator matches the probed inputs' concepts: regression probes use synthesized inputs only".into(),
        );
    }

    // Least privilege: exactly the permissions the probes need, from the contract.
    let mut permissions = BTreeSet::new();
    let mut called: Vec<CapabilityId> = enumerators.iter().chain(regression.iter()).cloned().collect();
    for flow in &flows {
        called.push(flow.capability.clone());
        for method in integration
            .and_then(|i| i.capability(&flow.capability.name, flow.capability.version))
            .and_then(|e| e.verify.as_ref())
            .into_iter()
            .flatten()
        {
            match method {
                VerificationMethod::State(s) => called.push(id(&s.via.capability, s.via.version)),
                VerificationMethod::Event(e) => {
                    if let Some(event) = manifest.event(&e.event) {
                        permissions.extend(event.authorization.iter().cloned());
                    }
                }
            }
        }
    }
    for c in &called {
        if let Some(d) = definition(c) {
            permissions.extend(d.authorization.iter().cloned());
        }
    }
    let permissions: Vec<String> = permissions.into_iter().collect();
    decide("permissions", format!("verification session is granted only [{}]", permissions.join(", ")));

    let context = select_context(project, &graph, &application, &target, &handler.module, &mut decide);
    let checks = checks(&key, &view, &regression, &flows, contract);

    Ok(Plan {
        intent: intent.clone(),
        application,
        target,
        contract_digest: contract.digest(),
        view,
        boundary,
        context,
        permissions,
        probes: ProbePlan { enumerators, regression, flows },
        checks,
        decisions,
    })
}

fn kebab(kind: EdgeKind) -> String {
    serde_json::to_value(kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn node_id(application: &str, c: &CapabilityId) -> String {
    format!("capability:{application}/{}@{}", c.name, c.version)
}

/// `capability:<app>/<name>@<version>` of this application → its id.
fn parse_capability_node(application: &str, node: &str) -> Option<CapabilityId> {
    let reference = node.strip_prefix(&format!("capability:{application}/"))?;
    let (name, version) = reference.rsplit_once('@')?;
    Some(CapabilityId { name: name.to_string(), version: version.parse().ok()? })
}

fn select_context(
    project: &Project,
    graph: &Graph,
    application: &str,
    target: &CapabilityId,
    module: &str,
    decide: &mut impl FnMut(&'static str, String),
) -> Vec<ContextFile> {
    let mut files = vec![ContextFile {
        path: module.to_string(),
        editable: true,
        reason: format!("implements {}@{}", target.name, target.version),
    }];
    let mut seen: BTreeSet<String> = BTreeSet::from([module.to_string()]);
    let contract_modules: BTreeSet<&str> = project.capabilities.iter().map(|c| c.declared_in.as_str()).collect();
    let mut budget = CONTEXT_BUDGET_BYTES;
    let mut add =
        |path: &str, reason: String, files: &mut Vec<ContextFile>, decide: &mut dyn FnMut(&'static str, String)| {
            if seen.contains(path) || contract_modules.contains(path) {
                return;
            }
            let Some(source) = project.source(path) else { return };
            if source.bytes > budget {
                decide("context", format!("{path} omitted: over the context budget"));
                return;
            }
            budget -= source.bytes;
            seen.insert(path.to_string());
            files.push(ContextFile { path: path.to_string(), editable: false, reason });
        };

    // What the current implementation uses.
    for import in project.source(module).map(|s| s.imports.clone()).unwrap_or_default() {
        add(&import, format!("imported by the current implementation ({module})"), &mut files, decide);
    }

    // Implementations that touch the same declared state resources.
    let node = node_id(application, target);
    let resources: BTreeSet<&str> = graph
        .outgoing(&node)
        .filter(|e| matches!(e.kind, EdgeKind::Reads | EdgeKind::Changes))
        .map(|e| e.to.as_str())
        .collect();
    let mut peers: BTreeMap<String, String> = BTreeMap::new();
    for edge in graph.edges.iter().filter(|e| matches!(e.kind, EdgeKind::Reads | EdgeKind::Changes)) {
        if edge.from == node || !resources.contains(edge.to.as_str()) {
            continue;
        }
        let Some(peer) = parse_capability_node(application, &edge.from) else { continue };
        let Some(handler) = project.capability(&peer.name, peer.version).and_then(|c| c.handler.as_ref()) else {
            continue;
        };
        let verb = if edge.kind == EdgeKind::Changes { "changes" } else { "reads" };
        let resource =
            graph.node(&edge.to).filter(|n| n.kind == NodeKind::Resource).map_or(edge.to.as_str(), |n| n.id.as_str());
        peers
            .entry(handler.module.clone())
            .or_insert(format!("{}@{} {verb} {resource}, which the target reads", peer.name, peer.version));
    }
    // Writers first: they show how the state the target reads is produced.
    let mut ordered: Vec<(String, String)> = peers.into_iter().collect();
    ordered.sort_by_key(|(_, reason)| !reason.contains(" changes "));
    for (path, reason) in ordered {
        add(&path, reason.clone(), &mut files, decide);
        for import in project.source(&path).map(|s| s.imports.clone()).unwrap_or_default() {
            add(&import, format!("imported by {path}"), &mut files, decide);
        }
    }
    for file in &files {
        decide("context", format!("{} {}: {}", if file.editable { "edit" } else { "read" }, file.path, file.reason));
    }
    files
}

fn checks(
    key: &str,
    view: &CapabilityView,
    regression: &[CapabilityId],
    flows: &[Flow],
    contract: &Contract,
) -> Vec<CheckSpec> {
    let c = |code: &str, rule: String, source: &str| CheckSpec {
        code: code.into(),
        rule,
        source: source.into(),
        subject: None,
    };
    let mut checks = vec![
        c("edit.boundary", "only the target's implementation module changes".into(), "plan"),
        c("runtime.load", "the application loads and answers appport.manifest@1".into(), "AppPort runtime"),
        c(
            "contract.identity",
            format!("the published contract is unchanged (contractDigest {})", short(&contract.digest())),
            "contractDigest; ReplacementBoundary.contract_pin",
        ),
        c("contract.valid", "the published contract has no validation errors".into(), "Contract::validate"),
        c("output.conformance", "every output observed conforms to its declared output schema".into(), "outputSchema"),
    ];
    let declared: Vec<&str> = view.failures.iter().map(|f| f.code.as_str()).collect();
    checks.push(c(
        "failures.declared",
        if declared.is_empty() {
            format!("{key} fails only with the codes it failed with before")
        } else {
            format!("{key} fails only with its declared failure codes ({})", declared.join(", "))
        },
        "integration.failures",
    ));
    if !regression.is_empty() {
        checks.push(c(
            "behavior.regression",
            format!(
                "answers on existing state are unchanged for {}",
                regression.iter().map(|c| format!("{}@{}", c.name, c.version)).collect::<Vec<_>>().join(", ")
            ),
            "baseline observation",
        ));
    }
    for flow in flows {
        checks.push(CheckSpec {
            subject: Some(flow.capability.clone()),
            ..c(
                "verification",
                format!(
                    "{}@{}: declared verification ({}) is not contradicted and every state method confirms",
                    flow.capability.name,
                    flow.capability.version,
                    flow.methods.join(", ")
                ),
                "integration.verify",
            )
        });
    }
    checks
}

fn short(digest: &str) -> String {
    digest.chars().take(19).collect()
}

/// What the model receives: instructions and the minimal context. Nothing else
/// about the project is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelTask {
    pub system: String,
    pub prompt: String,
}

/// The fence the model must use for each file it returns.
pub const FILE_OPEN: &str = "=== FILE: ";
pub const FILE_CLOSE: &str = "=== END FILE ===";

/// Builds the model task for an attempt. `files` holds the current contents of
/// `plan.context`; `feedback` lists what the previous attempt violated.
pub fn model_task(plan: &Plan, files: &BTreeMap<String, String>, feedback: &[String]) -> ModelTask {
    let system = "You write implementation code for one capability of an AppPort application. \
The agent that called you owns the contract, decides what may change, and verifies your result by running the \
application; it does not take your word for anything. Change only the editable file. Keep its exported symbol and \
signature. Reply with the complete new contents of each file you change, and nothing else, in this exact form:\n\
=== FILE: <path> ===\n<entire file contents>\n=== END FILE ==="
        .to_string();

    let editable = plan.editable();
    let symbol_note = plan
        .decisions
        .iter()
        .find(|d| d.step == "source" && d.fact.contains(" implemented in "))
        .map(|d| d.fact.clone())
        .unwrap_or_default();

    let mut prompt = String::new();
    let mut line = |text: &str| {
        prompt.push_str(text);
        prompt.push('\n');
    };
    line(&format!(
        "# Task\nReplace the implementation of {}@{} in application {}.",
        plan.target.name, plan.target.version, plan.application
    ));
    line(&format!("Goal: {}", plan.intent.goal));
    line(&format!("{symbol_note}."));
    line(&format!("Editable file: {}", editable.join(", ")));
    line("\n# Contract of the capability (authoritative, JSON)");
    line(&model_json(&plan.view));
    line("\n# Constraints the replacement must satisfy");
    for constraint in &plan.boundary.constraints {
        line(&format!("- [{}] {}", constraint.code, constraint.rule));
    }
    if !plan.view.failures.is_empty() {
        line(&format!(
            "- [failures] signal failures only with the declared codes, as AppPort errors (`errors` from @appport/protocol): {}",
            plan.view.failures.iter().map(|f| f.code.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    line("\n# How the result will be judged (by running the application)");
    for check in &plan.checks {
        line(&format!("- [{}] {}", check.code, check.rule));
    }
    if !feedback.is_empty() {
        line("\n# The previous attempt was rejected. It violated:");
        for item in feedback {
            line(&format!("- {item}"));
        }
        line("Fix every item above.");
    }
    line("\n# Files");
    for file in &plan.context {
        let Some(contents) = files.get(&file.path) else { continue };
        line(&format!(
            "## {} ({}; {})\n```\n{}\n```",
            file.path,
            if file.editable { "EDITABLE" } else { "read-only" },
            file.reason,
            contents.trim_end()
        ));
    }
    line(&format!("\nReply with the new contents of {} only.", editable.join(", ")));
    ModelTask { system, prompt }
}

/// Every value of `concept`-annotated fields in `output`, by concept IRI.
pub fn harvest_concepts(schema: &crate::appport::knowledge::Schema, output: &Value) -> BTreeMap<String, Vec<Value>> {
    let mut found: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for field in fields(schema) {
        let Some(concept) = field.concept else { continue };
        for value in crate::conformance::values_at(output, &field.pointer) {
            let bucket = found.entry(concept.clone()).or_default();
            if !bucket.contains(value) {
                bucket.push(value.clone());
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_intents() {
        let intent = Intent::parse("replace customers.lookup@1: read from the store").unwrap();
        assert_eq!(
            (intent.capability.as_str(), intent.version, intent.goal.as_str()),
            ("customers.lookup", 1, "read from the store")
        );
        assert_eq!(Intent::parse("replace a.b@2").unwrap().version, 2);
        assert!(Intent::parse("delete a.b@1").is_err());
        assert!(Intent::parse("replace a.b").is_err());
    }

    #[test]
    fn parses_capability_nodes() {
        assert_eq!(
            parse_capability_node("com.x", "capability:com.x/a.b@3"),
            Some(CapabilityId { name: "a.b".into(), version: 3 })
        );
        assert_eq!(parse_capability_node("com.x", "capability:com.y/a.b@3"), None);
        assert_eq!(parse_capability_node("com.x", "event:com.x/a.b"), None);
    }
}
