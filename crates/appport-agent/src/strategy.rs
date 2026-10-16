//! Agent strategies: the experimental variables of the coding loop.
//!
//! The loop's authorities do not change here:
//! - the deterministic planner draws the edit boundary ([`agent_core::generic::plan_coding`]);
//! - the execution boundary decides what may be written ([`crate::edit`]);
//! - the project's own tests decide acceptance.
//!
//! A strategy only chooses what the model is shown ([`ContextStrategy`]) and
//! what to do after a rejected attempt ([`RecoveryPolicy`]). Neither can widen
//! the editable files, grant a permission, or mark an attempt as accepted.
//!
//! Each strategy has a stable id and a version. Reports record both, so a result
//! can be tied to the exact behaviour that produced it.

use std::collections::{BTreeMap, BTreeSet};

use agent_core::appport::knowledge::{model_json, Contract};
use agent_core::generic::{is_test_file, render_coding_task, Baseline, CodingPlan, TaskView};
use agent_core::observe::Project;
use agent_core::plan::ModelTask;
use serde::{Deserialize, Serialize};

use crate::host::HostConfig;

/// The default bound on read-only context, in bytes (the planner's own budget).
pub const DEFAULT_CONTEXT_BUDGET: usize = 48 * 1024;

/// A context strategy, by id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextStrategyId {
    /// The intent and the editable files only.
    Minimal,
    /// The planner's context: editable files, failing tests, their imports, the test output (the baseline).
    #[default]
    Observed,
    /// Observed, plus each editable file's exports and where importers use them.
    DependencyAware,
    /// Dependency-aware, plus the AppPort contract of capabilities implemented in the editable files.
    AppportAware,
}

impl ContextStrategyId {
    pub const ALL: [ContextStrategyId; 4] = [Self::Minimal, Self::Observed, Self::DependencyAware, Self::AppportAware];
}

impl std::str::FromStr for ContextStrategyId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|_| {
            format!("unknown context strategy {s:?}: use minimal, observed, dependency-aware or appport-aware")
        })
    }
}

impl std::fmt::Display for ContextStrategyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Minimal => "minimal",
            Self::Observed => "observed",
            Self::DependencyAware => "dependency-aware",
            Self::AppportAware => "appport-aware",
        })
    }
}

/// A recovery policy, by id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryPolicyId {
    /// Retry with the verifier's feedback until attempts run out (the baseline).
    #[default]
    Feedback,
    /// Retry with feedback, and add read-only context the failure points at.
    ExpandContext,
    /// Retry with feedback, but stop when the same failure repeats.
    StopOnRepeat,
}

impl std::str::FromStr for RecoveryPolicyId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_string()))
            .map_err(|_| format!("unknown recovery policy {s:?}: use feedback, expand-context or stop-on-repeat"))
    }
}

impl std::fmt::Display for RecoveryPolicyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Feedback => "feedback",
            Self::ExpandContext => "expand-context",
            Self::StopOnRepeat => "stop-on-repeat",
        })
    }
}

// ---------------------------------------------------------------------------
// Context selection
// ---------------------------------------------------------------------------

/// What a context strategy works from. Everything here was established
/// deterministically before any model call.
pub struct ContextInput<'a> {
    pub project: &'a Project,
    pub plan: &'a CodingPlan,
    pub baseline: &'a Baseline,
    /// Bound on read-only context, in bytes. Editable files are always included.
    pub budget: usize,
    pub host: &'a HostConfig,
    /// Read-only files a recovery policy asked for, with its reason.
    pub requested: &'a [(String, String)],
}

/// One thing a strategy considered, and whether it was included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Selected {
    /// `file`, `fact` or `test-output`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub editable: bool,
    pub reason: String,
    pub bytes: usize,
    pub included: bool,
}

/// What a strategy chose to show the model, and why. Serialized into every
/// attempt's record, without file contents.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSelection {
    pub strategy: ContextStrategyId,
    pub version: u32,
    pub budget_bytes: usize,
    /// Bytes of everything included (files, facts, test output).
    pub total_bytes: usize,
    pub items: Vec<Selected>,
    /// What the strategy could not establish. Never filled in by guessing.
    pub unknowns: Vec<String>,
    #[serde(skip)]
    view: ViewParts,
}

#[derive(Debug, Clone, Default)]
struct ViewParts {
    show_test: bool,
    show_output: bool,
    files: Vec<(String, String, String)>,
    facts: Vec<String>,
}

impl ContextSelection {
    /// The model task for this selection and the feedback so far.
    pub fn task(&self, plan: &CodingPlan, baseline: &Baseline, feedback: &[String]) -> ModelTask {
        let view = TaskView {
            intent: &plan.intent,
            test: self.view.show_test.then_some(plan.test.as_slice()),
            test_output: (self.view.show_output && !baseline.passed).then_some(baseline.output.as_str()),
            editable: plan.editable(),
            files: self.view.files.iter().map(|(p, l, c)| (p.as_str(), l.clone(), c.as_str())).collect(),
            facts: self.view.facts.clone(),
            feedback,
        };
        render_coding_task(&view)
    }

    /// Paths of the files shown to the model.
    pub fn files(&self) -> Vec<&str> {
        self.view.files.iter().map(|(p, _, _)| p.as_str()).collect()
    }
}

/// Chooses what the model is shown. Implementations must report every file and
/// fact they consider, with a reason, and must mark what they cannot establish
/// as unknown instead of inventing it.
pub trait ContextStrategy {
    fn id(&self) -> ContextStrategyId;
    /// Bumped whenever the strategy's selection changes.
    fn version(&self) -> u32;
    fn select(&self, input: &ContextInput) -> Result<ContextSelection, String>;
}

/// The strategy for an id.
pub fn context_strategy(id: ContextStrategyId) -> Box<dyn ContextStrategy> {
    match id {
        ContextStrategyId::Minimal => Box::new(Minimal),
        ContextStrategyId::Observed => Box::new(Observed),
        ContextStrategyId::DependencyAware => Box::new(DependencyAware),
        ContextStrategyId::AppportAware => Box::new(AppportAware),
    }
}

/// Builds a selection, enforcing the budget on read-only material.
struct Builder<'a> {
    input: &'a ContextInput<'a>,
    selection: ContextSelection,
    spent: usize,
}

impl<'a> Builder<'a> {
    fn new(strategy: &dyn ContextStrategy, input: &'a ContextInput<'a>) -> Self {
        Builder {
            input,
            selection: ContextSelection {
                strategy: strategy.id(),
                version: strategy.version(),
                budget_bytes: input.budget,
                total_bytes: 0,
                items: Vec::new(),
                unknowns: Vec::new(),
                view: ViewParts::default(),
            },
            spent: 0,
        }
    }

    fn file(&mut self, path: &str, editable: bool, reason: &str, label: String) -> Result<(), String> {
        if self.selection.view.files.iter().any(|(p, _, _)| p == path) {
            return Ok(());
        }
        let contents = self.input.project.read(path).map_err(|e| format!("{path}: {e}"))?;
        let bytes = contents.len();
        let included = editable || self.spent + bytes <= self.input.budget;
        if !editable && included {
            self.spent += bytes;
        }
        self.selection.items.push(Selected {
            kind: "file".into(),
            path: Some(path.to_string()),
            editable,
            reason: if included { reason.to_string() } else { format!("{reason}; left out: over the context budget") },
            bytes,
            included,
        });
        if included {
            self.selection.total_bytes += bytes;
            self.selection.view.files.push((path.to_string(), label, contents));
        }
        Ok(())
    }

    fn fact(&mut self, text: String, reason: &str) {
        let bytes = text.len();
        let included = self.spent + bytes <= self.input.budget;
        if included {
            self.spent += bytes;
            self.selection.total_bytes += bytes;
            self.selection.view.facts.push(text.clone());
        }
        self.selection.items.push(Selected {
            kind: "fact".into(),
            path: None,
            editable: false,
            reason: if included {
                format!("{reason}: {text}")
            } else {
                format!("{reason}; left out: over the context budget")
            },
            bytes,
            included,
        });
    }

    fn test_output(&mut self) {
        self.selection.view.show_test = true;
        if self.input.baseline.passed {
            return;
        }
        self.selection.view.show_output = true;
        let bytes = agent_core::generic::tail(&self.input.baseline.output, 6 * 1024).len();
        self.selection.total_bytes += bytes;
        self.selection.items.push(Selected {
            kind: "test-output".into(),
            path: None,
            editable: false,
            reason: "the test command's output before the change".into(),
            bytes,
            included: true,
        });
    }

    fn unknown(&mut self, text: String) {
        if !self.selection.unknowns.contains(&text) {
            self.selection.unknowns.push(text);
        }
    }

    /// The planner's context, as the observed strategy shows it.
    fn observed(&mut self) -> Result<(), String> {
        self.test_output();
        for file in &self.input.plan.context {
            let label = format!("{}; {}", if file.editable { "EDITABLE" } else { "read-only" }, file.reason);
            self.file(&file.path, file.editable, &file.reason, label)?;
        }
        self.requested()
    }

    /// Files a recovery policy asked for.
    fn requested(&mut self) -> Result<(), String> {
        for (path, reason) in self.input.requested {
            if self.input.project.source(path).is_some() {
                self.file(path, false, reason, format!("read-only; {reason}"))?;
            }
        }
        Ok(())
    }

    fn finish(self) -> ContextSelection {
        self.selection
    }
}

/// The intent and the editable files, nothing observed about them.
pub struct Minimal;

impl ContextStrategy for Minimal {
    fn id(&self) -> ContextStrategyId {
        ContextStrategyId::Minimal
    }
    fn version(&self) -> u32 {
        1
    }
    fn select(&self, input: &ContextInput) -> Result<ContextSelection, String> {
        let mut b = Builder::new(self, input);
        for file in input.plan.context.iter().filter(|f| f.editable) {
            // The reason would name the failing test: the minimal arm does not see it.
            b.file(&file.path, true, "editable", "EDITABLE".into())?;
        }
        b.requested()?;
        Ok(b.finish())
    }
}

/// The planner's context (the baseline).
pub struct Observed;

impl ContextStrategy for Observed {
    fn id(&self) -> ContextStrategyId {
        ContextStrategyId::Observed
    }
    fn version(&self) -> u32 {
        1
    }
    fn select(&self, input: &ContextInput) -> Result<ContextSelection, String> {
        let mut b = Builder::new(self, input);
        b.observed()?;
        Ok(b.finish())
    }
}

/// Names a module exports, found lexically. `None` entries are forms the scan
/// does not resolve (`export *`, `export default` of an expression).
pub fn exports(text: &str) -> (Vec<String>, Vec<String>) {
    let mut names = Vec::new();
    let mut unresolved = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("export ") else { continue };
        let rest = rest.trim_start();
        let rest = rest.strip_prefix("async ").unwrap_or(rest);
        if let Some(r) =
            ["function* ", "function ", "class ", "const ", "let ", "var "].iter().find_map(|k| rest.strip_prefix(k))
        {
            let name: String =
                r.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect();
            if !name.is_empty() {
                names.push(name);
            }
        } else if let Some(r) = rest.strip_prefix('{') {
            let inner = r.split('}').next().unwrap_or_default();
            for part in inner.split(',') {
                let exported = part.split(" as ").last().unwrap_or(part).trim();
                if !exported.is_empty() {
                    names.push(exported.to_string());
                }
            }
            if r.contains(" from ") {
                unresolved.push(format!("re-export `{}`", line.chars().take(80).collect::<String>()));
            }
        } else if rest.starts_with('*') {
            unresolved.push(format!("`{}`", line.chars().take(80).collect::<String>()));
        } else if rest.starts_with("default") {
            unresolved.push("a default export".into());
        }
    }
    names.dedup();
    (names, unresolved)
}

/// Lines of `text` that use `name` as a whole word, with 1-based line numbers.
fn uses(text: &str, name: &str) -> Vec<(usize, String)> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let t = line.trim_start();
            if t.starts_with("import ") || t.starts_with("//") || t.starts_with('*') {
                return false;
            }
            line.match_indices(name).any(|(i, _)| {
                let before = line[..i].chars().next_back();
                let after = line[i + name.len()..].chars().next();
                !before.is_some_and(is_word) && !after.is_some_and(is_word)
            })
        })
        .map(|(i, line)| (i + 1, line.trim().chars().take(160).collect()))
        .collect()
}

/// Most call sites listed per exported name.
const MAX_USES: usize = 8;

/// Observed, plus each editable file's exports and where its importers use them.
pub struct DependencyAware;

impl DependencyAware {
    fn add_dependencies(b: &mut Builder) {
        let project = b.input.project;
        for path in b.input.plan.editable() {
            let Ok(text) = project.read(path) else { continue };
            let (names, unresolved) = exports(&text);
            for u in unresolved {
                b.unknown(format!("{path}: {u} is not resolved by the lexical scan"));
            }
            let importers: Vec<&str> = project.importers(path);
            if importers.is_empty() {
                b.unknown(format!(
                    "no module imports {path} statically; it may be loaded dynamically, by a runner, or not at all"
                ));
                continue;
            }
            b.fact(format!("{path} is imported by {}", importers.join(", ")), "import graph (observed)");
            for name in &names {
                let mut sites = Vec::new();
                for importer in &importers {
                    let Ok(source) = project.read(importer) else { continue };
                    for (line, code) in uses(&source, name) {
                        sites.push(format!(
                            "{importer}:{line} `{code}`{}",
                            if is_test_file(importer) { " (test)" } else { "" }
                        ));
                    }
                }
                match sites.len() {
                    0 => b.unknown(format!("`{name}` (exported by {path}) has no lexical use in its importers")),
                    n => {
                        let shown: Vec<String> = sites.into_iter().take(MAX_USES).collect();
                        b.fact(
                            format!(
                                "`{name}` (exported by {path}) is used at {}{}",
                                shown.join("; "),
                                if n > MAX_USES { format!(" and {} more", n - MAX_USES) } else { String::new() }
                            ),
                            "call sites (lexical match in importers)",
                        );
                    }
                }
            }
        }
        b.unknown(
            "call sites are whole-word matches of exported names in importing modules; aliased imports, dynamic calls and transitive users are not resolved".into(),
        );
    }
}

impl ContextStrategy for DependencyAware {
    fn id(&self) -> ContextStrategyId {
        ContextStrategyId::DependencyAware
    }
    fn version(&self) -> u32 {
        1
    }
    fn select(&self, input: &ContextInput) -> Result<ContextSelection, String> {
        let mut b = Builder::new(self, input);
        b.observed()?;
        DependencyAware::add_dependencies(&mut b);
        Ok(b.finish())
    }
}

/// Dependency-aware, plus the AppPort contract of every capability whose
/// handler is an editable file, read from the application's own runtime.
pub struct AppportAware;

impl ContextStrategy for AppportAware {
    fn id(&self) -> ContextStrategyId {
        ContextStrategyId::AppportAware
    }
    fn version(&self) -> u32 {
        1
    }
    fn select(&self, input: &ContextInput) -> Result<ContextSelection, String> {
        let mut b = Builder::new(self, input);
        b.observed()?;
        DependencyAware::add_dependencies(&mut b);
        let project = input.project;
        let Some(entry) = project.application.clone() else {
            b.unknown("the repository is not an AppPort application (no createApplication export was found); no contract knowledge applies".into());
            return Ok(b.finish());
        };
        let editable: BTreeSet<&str> = input.plan.editable().into_iter().collect();
        let implemented: Vec<_> = project
            .capabilities
            .iter()
            .filter(|c| c.handler.as_ref().is_some_and(|h| editable.contains(h.module.as_str())))
            .collect();
        if implemented.is_empty() {
            b.unknown("no editable file implements a declared AppPort capability".into());
            return Ok(b.finish());
        }
        let contract = match crate::collect::discover(input.host, &project.root, &entry)
            .and_then(|(manifest, _)| Contract::from_value(manifest).map_err(|e| e.to_string()))
        {
            Ok(contract) => contract,
            Err(e) => {
                b.unknown(format!("the AppPort application could not be read ({e}); its contract is unknown"));
                return Ok(b.finish());
            }
        };
        for declared in implemented {
            let id = format!("{}@{}", declared.name, declared.version);
            let handler = declared.handler.as_ref().map(|h| h.module.as_str()).unwrap_or_default();
            match contract.inspect(&declared.name, declared.version) {
                Some(view) => b.fact(
                    format!("{handler} implements {id}, whose published contract is {}", model_json(&view)),
                    "AppPort contract (published by the runtime)",
                ),
                None => {
                    b.unknown(format!("{id} is declared in source but not published by the runtime"));
                    continue;
                }
            }
            if let Some(boundary) = contract.replacement_boundary(&declared.name, declared.version) {
                let dependents: Vec<String> =
                    boundary.dependents.iter().map(|d| serde_json::to_string(d).unwrap_or_default()).collect();
                if !dependents.is_empty() {
                    b.fact(
                        format!("{id} has dependents that must keep working: {}", dependents.join("; ")),
                        "AppPort replacement boundary",
                    );
                }
                if !boundary.verification.is_empty() {
                    b.fact(
                        format!("{id} is covered by declared verification: {}", model_json(&boundary.verification)),
                        "AppPort replacement boundary",
                    );
                }
            }
        }
        Ok(b.finish())
    }
}

// ---------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------

/// Why an attempt was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Failure {
    /// The reply could not be applied (no file block, or outside the boundary).
    Unusable { reason: String },
    /// Applied, and the test command failed.
    Tests { output: String, timed_out: bool },
}

impl Failure {
    /// A comparable signature: the failing-test lines, or the reason.
    pub fn signature(&self) -> String {
        match self {
            Failure::Unusable { reason } => format!("unusable: {reason}"),
            Failure::Tests { output, timed_out } => {
                let failing: BTreeSet<&str> = output
                    .lines()
                    .map(str::trim)
                    .filter(|l| {
                        l.starts_with("not ok") || l.starts_with('✖') || l.starts_with("FAIL") || l.contains("Error:")
                    })
                    .collect();
                format!(
                    "tests{}: {}",
                    if *timed_out { " (timed out)" } else { "" },
                    failing.into_iter().collect::<Vec<_>>().join(" | ")
                )
            }
        }
    }
}

/// What to do after a rejected attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "action")]
pub enum RecoveryDecision {
    /// Ask again with the verifier's feedback.
    RetryWithFeedback,
    /// Ask again with the feedback and these extra read-only files.
    RetryWithContext { add: Vec<(String, String)> },
    /// Stop: more attempts would not have more to go on.
    Stop { reason: String },
}

/// What a recovery policy sees. Read-only.
pub struct RecoveryInput<'a> {
    pub project: &'a Project,
    pub plan: &'a CodingPlan,
    /// Every failure so far, oldest first; the last one is the attempt just rejected.
    pub failures: &'a [Failure],
    /// Files already shown to the model.
    pub shown: &'a [&'a str],
}

/// Decides what follows a rejected attempt. A policy cannot change the editable
/// files, the permissions or the acceptance command: it can only retry, add
/// read-only context, or stop. The attempt budget is enforced by the loop.
pub trait RecoveryPolicy {
    fn id(&self) -> RecoveryPolicyId;
    fn version(&self) -> u32;
    fn decide(&self, input: &RecoveryInput) -> RecoveryDecision;
}

pub fn recovery_policy(id: RecoveryPolicyId) -> Box<dyn RecoveryPolicy> {
    match id {
        RecoveryPolicyId::Feedback => Box::new(FeedbackRetry),
        RecoveryPolicyId::ExpandContext => Box::new(ExpandContext),
        RecoveryPolicyId::StopOnRepeat => Box::new(StopOnRepeat),
    }
}

/// Always retry with feedback (the baseline behaviour).
pub struct FeedbackRetry;

impl RecoveryPolicy for FeedbackRetry {
    fn id(&self) -> RecoveryPolicyId {
        RecoveryPolicyId::Feedback
    }
    fn version(&self) -> u32 {
        1
    }
    fn decide(&self, _: &RecoveryInput) -> RecoveryDecision {
        RecoveryDecision::RetryWithFeedback
    }
}

/// Source files a failure's output names that are not yet shown (read-only).
fn named_in_output(input: &RecoveryInput, output: &str) -> Vec<(String, String)> {
    let shown: BTreeSet<&str> = input.shown.iter().copied().collect();
    let mut add: BTreeMap<String, String> = BTreeMap::new();
    for source in &input.project.sources {
        if !shown.contains(source.path.as_str()) && output.contains(source.path.as_str()) {
            add.insert(source.path.clone(), "named in the failing test output".into());
        }
    }
    add.into_iter().collect()
}

/// Retry with feedback, adding read-only files the failure points at: files the
/// test output names, then modules that import an editable file.
pub struct ExpandContext;

impl RecoveryPolicy for ExpandContext {
    fn id(&self) -> RecoveryPolicyId {
        RecoveryPolicyId::ExpandContext
    }
    fn version(&self) -> u32 {
        1
    }
    fn decide(&self, input: &RecoveryInput) -> RecoveryDecision {
        let Some(Failure::Tests { output, .. }) = input.failures.last() else {
            return RecoveryDecision::RetryWithFeedback;
        };
        let mut add = named_in_output(input, output);
        if add.is_empty() {
            let shown: BTreeSet<&str> = input.shown.iter().copied().collect();
            for path in input.plan.editable() {
                for importer in input.project.importers(path) {
                    if !shown.contains(importer) && !add.iter().any(|(p, _)| p == importer) {
                        add.push((importer.to_string(), format!("imports the editable {path}")));
                    }
                }
            }
        }
        if add.is_empty() {
            RecoveryDecision::RetryWithFeedback
        } else {
            RecoveryDecision::RetryWithContext { add }
        }
    }
}

/// Retry with feedback; stop when an attempt fails exactly as the previous one did.
pub struct StopOnRepeat;

impl RecoveryPolicy for StopOnRepeat {
    fn id(&self) -> RecoveryPolicyId {
        RecoveryPolicyId::StopOnRepeat
    }
    fn version(&self) -> u32 {
        1
    }
    fn decide(&self, input: &RecoveryInput) -> RecoveryDecision {
        match input.failures {
            [.., previous, last] if previous.signature() == last.signature() => RecoveryDecision::Stop {
                reason: format!(
                    "the same failure twice in a row ({}); another attempt has nothing new to go on",
                    last.signature().chars().take(160).collect::<String>()
                ),
            },
            _ => RecoveryDecision::RetryWithFeedback,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_unknown_ids_are_rejected() {
        for id in ContextStrategyId::ALL {
            assert_eq!(id.to_string().parse::<ContextStrategyId>().unwrap(), id);
            assert_eq!(context_strategy(id).id(), id);
        }
        for id in [RecoveryPolicyId::Feedback, RecoveryPolicyId::ExpandContext, RecoveryPolicyId::StopOnRepeat] {
            assert_eq!(id.to_string().parse::<RecoveryPolicyId>().unwrap(), id);
            assert_eq!(recovery_policy(id).id(), id);
        }
        assert!("repo-map".parse::<ContextStrategyId>().unwrap_err().contains("unknown context strategy"));
        assert!("reflect".parse::<RecoveryPolicyId>().is_err());
        assert_eq!(ContextStrategyId::default(), ContextStrategyId::Observed, "the baseline is the default");
        assert_eq!(RecoveryPolicyId::default(), RecoveryPolicyId::Feedback);
    }

    #[test]
    fn exports_are_found_lexically_and_the_rest_is_unknown() {
        let text = "export function total(x) {}\nexport async function load() {}\nexport const RATE = 2;\nexport { a, b as c };\nexport * from './x.js';\nexport default 3;\nconst hidden = 1;";
        let (names, unresolved) = exports(text);
        assert_eq!(names, ["total", "load", "RATE", "a", "c"]);
        assert_eq!(unresolved.len(), 2);
    }

    #[test]
    fn uses_are_whole_words_outside_imports_and_comments() {
        let text = "import { total } from './a.js';\n// total here\nconst t = total(1);\nconst subtotal = 2;\n";
        assert_eq!(uses(text, "total"), [(3, "const t = total(1);".to_string())]);
    }

    #[test]
    fn stop_on_repeat_stops_only_on_an_identical_failure() {
        let fail = |o: &str| Failure::Tests { output: format!("ok 1\nnot ok 2 - {o}\n"), timed_out: false };
        let project = Project {
            root: std::path::PathBuf::new(),
            package: None,
            appport_packages: Default::default(),
            sources: Vec::new(),
            application: None,
            capabilities: Vec::new(),
            findings: Vec::new(),
        };
        let plan = CodingPlan {
            intent: String::new(),
            test: Vec::new(),
            failing_tests: Vec::new(),
            context: Vec::new(),
            decisions: Vec::new(),
        };
        let decide = |failures: &[Failure]| {
            StopOnRepeat.decide(&RecoveryInput { project: &project, plan: &plan, failures, shown: &[] })
        };
        assert_eq!(decide(&[fail("a")]), RecoveryDecision::RetryWithFeedback);
        assert_eq!(decide(&[fail("a"), fail("b")]), RecoveryDecision::RetryWithFeedback);
        assert!(matches!(decide(&[fail("b"), fail("b")]), RecoveryDecision::Stop { .. }));
        assert_eq!(
            FeedbackRetry.decide(&RecoveryInput {
                project: &project,
                plan: &plan,
                failures: &[fail("b"), fail("b")],
                shown: &[]
            }),
            RecoveryDecision::RetryWithFeedback
        );
    }
}
