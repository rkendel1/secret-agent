//! Three-arm evaluation: bare (A), context-assisted (B) and AppPort-native (C).
//!
//! Every arm gets the same task text, the same fixture copied fresh for each
//! trial, the same model configuration and the same proposal budget. Arm order
//! rotates per trial and task. After each trial an independent oracle scores
//! the final state of the project:
//!
//! - **positive tasks** (`expect: verified-change`): a fresh baseline of the
//!   pristine fixture and a fresh run of the final project, judged by the
//!   deterministic judge for the task's `oracleTarget`, with the changed files
//!   computed by comparing the project to the fixture — never from the arm's
//!   own account;
//! - **negative tasks** (`expect: limitation`): the project must be
//!   byte-identical to the fixture, and the arm must not claim success.
//!
//! Each trial is recorded with a distinct category; nothing is dropped.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use agent_core::appport::knowledge::Contract;
use agent_core::observe::observe;
use agent_core::plan::{plan, ContextMode, Intent};
use agent_core::verify::{judge, CheckStatus, Verdict};
use agent_model::gateway::{
    preflight, preflight_text, redact, secrets_of, GatewayPolicy, PreflightReport, FREE_GATEWAY_MODELS,
};
use agent_model::ModelConfig;
use serde::{Deserialize, Serialize};

use crate::collect::{collect, discover, Counters};
use crate::eval::copy_project;
use crate::host::HostConfig;
use crate::native::{run_native, NativeOptions};
use crate::report::now_rfc3339;
use crate::run::{run, RunOptions};

/// A task of the three-arm suite (`fixtures/appport-tasks/<id>/task.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArmTask {
    pub id: String,
    /// `verified-change` or `limitation`.
    pub expect: String,
    pub category: String,
    /// The project, relative to the repository root.
    pub project: String,
    /// The text every arm receives.
    pub goal: String,
    /// `name@version`, given to every arm when present.
    pub target: Option<String>,
    /// What the oracle verifies, for positive tasks. Never shown to an arm.
    pub oracle_target: Option<String>,
    /// An observable check of the goal itself, for positive tasks. Never shown to an arm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_check: Option<GoalCheck>,
}

/// A check that the goal was achieved, not only that behaviour was preserved.
///
/// Each step invokes a capability on the running application (a fresh process,
/// granted exactly the permissions the steps' capabilities declare in the
/// fixture's own contract) and must answer `ok`. Inputs and expected values may
/// refer to an earlier step's input or output: `{"$step": 0, "at": "/customerId"}`
/// (output) or `{"$step": 0, "input": "/email"}`. The check must fail on the
/// unchanged fixture; if it passes there, it measures nothing and the oracle
/// reports an error instead of a verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalCheck {
    pub description: String,
    pub steps: Vec<GoalStep>,
}

/// One goal-check step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalStep {
    /// `name@version`.
    pub capability: String,
    pub input: serde_json::Value,
    #[serde(default)]
    pub expect: Vec<GoalExpect>,
}

/// An expected value in a step's output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalExpect {
    /// JSON pointer into the step's output.
    pub at: String,
    pub equals: serde_json::Value,
}

impl ArmTask {
    pub fn positive(&self) -> bool {
        self.expect == "verified-change"
    }
}

/// An arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Arm {
    /// A: the existing loop, bare context.
    #[serde(rename = "A-bare")]
    Bare,
    /// B: the existing loop, AppPort knowledge as context.
    #[serde(rename = "B-context")]
    Context,
    /// C: the AppPort-native workflow.
    #[serde(rename = "C-native")]
    Native,
}

impl std::str::FromStr for Arm {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().chars().next() {
            Some('A') => Ok(Arm::Bare),
            Some('B') => Ok(Arm::Context),
            Some('C') => Ok(Arm::Native),
            _ => Err(format!("unknown arm {s:?}: use A, B or C")),
        }
    }
}

impl std::fmt::Display for Arm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Arm::Bare => "A-bare",
            Arm::Context => "B-context",
            Arm::Native => "C-native",
        })
    }
}

/// How to run the evaluation.
#[derive(Debug, Clone)]
pub struct ArmEvalOptions {
    /// The repository root that task `project` paths are relative to.
    pub root: PathBuf,
    pub tasks: Vec<PathBuf>,
    pub model: ModelConfig,
    pub arms: Vec<Arm>,
    pub trials: u32,
    /// Proposal budget per trial, for every arm.
    pub max_attempts: u32,
    /// Model-call budget per trial for the native arm.
    pub max_turns: u32,
    pub host: HostConfig,
    pub out: PathBuf,
    pub quiet: bool,
    /// Stop starting trials once the measured spend (above zero) reaches this.
    pub budget_usd: Option<f64>,
    /// Never start a trial that could take the run past this many model calls
    /// (A and B: up to `max_attempts` per trial; C: up to `max_turns`).
    pub max_model_calls: Option<usize>,
    /// Stop starting trials after this many minutes.
    pub max_minutes: Option<f64>,
    /// The free-gateway policy. When set, the configuration must pass
    /// [`preflight`] before any trial; if it does not, no trial runs.
    pub free_gateway: Option<GatewayPolicy>,
}

/// The tasks a smoke run uses: one positive, one negative.
pub const SMOKE_TASKS: &[&str] = &["replace-lookup", "contract-change"];

/// The most model calls one trial of `arm` can make.
pub fn max_calls(arm: Arm, max_attempts: u32, max_turns: u32) -> usize {
    match arm {
        Arm::Bare | Arm::Context => max_attempts as usize,
        Arm::Native => max_turns as usize,
    }
}

/// What the independent oracle found.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Oracle {
    /// Files that differ from the fixture.
    pub changed_files: Vec<String>,
    /// For positive tasks: whether every check passed (preservation, declared
    /// verification and, when the task has one, the goal check).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    /// For positive tasks: whether behaviour and the contract were preserved and
    /// declared verification passed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserved: Option<bool>,
    /// For tasks with a goal check: whether the goal was achieved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_achieved: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_detail: Option<String>,
    pub failed_checks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One trial.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArmTrial {
    pub task: String,
    pub expect: String,
    pub arm: String,
    pub trial: u32,
    /// The provider and model this trial was configured with.
    pub provider: String,
    pub model: String,
    /// 1-based position of this arm in this trial's rotated order.
    pub order: usize,
    /// What the arm claimed: `accepted`, `limitation`, `rejected`, `incomplete`,
    /// `provider-refusal`, `provider-error`, `harness-error`, `not-run`.
    pub claim: String,
    /// The scored outcome (see the module docs).
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oracle: Option<Oracle>,
    pub model_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tokens_estimated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Native-arm tool calls (0 for A and B).
    pub tool_calls: usize,
    /// AppPort knowledge (B) or discovery and inspection results (C) sent to the model, in bytes.
    pub appport_bytes: usize,
    pub proposals: usize,
    pub accepted_on_first_proposal: bool,
    pub rejected_proposals: usize,
    pub rollbacks: usize,
    pub boundary_violations: usize,
    pub contract_change_attempts: usize,
    pub unusable_responses: usize,
    pub application_runs: usize,
    pub wall_ms: u64,
}

/// Prices per million tokens (input, output), where known. Allowlisted free
/// gateway models are priced at zero (preflight checks the live catalog too).
pub fn price(model: &str) -> Option<(f64, f64)> {
    let m = model.to_lowercase();
    if FREE_GATEWAY_MODELS.contains(&model) {
        Some((0.0, 0.0))
    } else if m.contains("opus-5-5") || m.contains("opus-5.5") {
        Some((4.0, 20.0))
    } else if m.contains("sonnet-5-5") || m.contains("sonnet-5.5") {
        Some((2.0, 10.0))
    } else if m.contains("haiku-5-5") || m.contains("haiku-5.5") {
        Some((0.1, 0.5))
    } else {
        None
    }
}

fn cost(model: &str, input: u64, output: u64, estimated: bool) -> Option<f64> {
    let (i, o) = price(model)?;
    (!estimated).then(|| input as f64 * i / 1e6 + output as f64 * o / 1e6)
}

/// Reads a task file.
pub fn read_arm_task(dir: &Path) -> Result<ArmTask, String> {
    let path = dir.join("task.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Files under `dir` (relative, `/`-separated), excluding dependencies and VCS data.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(name.as_str(), "node_modules" | ".git" | "target") {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => walk(base, &path, out),
                Ok(t) if t.is_file() => {
                    let rel = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                    out.insert(rel, fs::read(&path).unwrap_or_default());
                }
                _ => {}
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn changed_files(template: &Path, project: &Path) -> Vec<String> {
    let before = files(template);
    let after = files(project);
    let mut changed: Vec<String> =
        after.iter().filter(|(p, b)| before.get(*p) != Some(*b)).map(|(p, _)| p.clone()).collect();
    changed.extend(before.keys().filter(|p| !after.contains_key(*p)).cloned());
    changed.sort();
    changed
}

/// Scores the final state of `project` against the pristine `template`.
pub fn oracle(task: &ArmTask, template: &Path, project: &Path, scratch: &Path, host: &HostConfig) -> Oracle {
    let changed = changed_files(template, project);
    let mut result = Oracle {
        changed_files: changed.clone(),
        verified: None,
        preserved: None,
        goal_achieved: None,
        goal_detail: None,
        failed_checks: Vec::new(),
        verdict: None,
        error: None,
    };
    let Some(target) = task.oracle_target.as_deref().filter(|_| task.positive()) else { return result };
    let scored = (|| -> Result<(Verdict, Option<Result<(), String>>), String> {
        let (name, version) = target.rsplit_once('@').ok_or("oracleTarget is not name@version")?;
        let version: u32 = version.parse().map_err(|_| "oracleTarget version")?;
        let _ = fs::remove_dir_all(scratch);
        copy_project(template, scratch).map_err(|e| e.to_string())?;
        let pristine = observe(scratch).map_err(|e| e.to_string())?;
        let entry = pristine.application.clone().ok_or("the fixture has no application entry")?;
        let (manifest, _) = discover(host, &pristine.root, &entry)?;
        let contract = Contract::from_value(manifest).map_err(|e| e.to_string())?;
        let intent = Intent { capability: name.into(), version, goal: task.goal.clone() };
        let plan = plan(&intent, &contract, &pristine).map_err(|r| format!("oracle cannot plan: {r}"))?;
        let mut counters = Counters::default();
        let baseline = collect(host, &pristine.root, &entry, &plan, &contract, None, &mut counters);
        if let Some(failure) = &baseline.failure {
            return Err(format!("oracle baseline failed: {failure}"));
        }
        let final_project = observe(project).map_err(|e| e.to_string())?;
        let final_entry = final_project.application.clone().unwrap_or_else(|| entry.clone());
        let candidate =
            collect(host, &final_project.root, &final_entry, &plan, &contract, Some(&baseline), &mut counters);
        let verdict = judge(&plan, &baseline, &candidate, &changed);
        let goal = match &task.goal_check {
            None => None,
            Some(check) => {
                // A check the unchanged fixture passes cannot tell a change from no change.
                if run_goal_check(check, &contract, host, &pristine.root, &entry).is_ok() {
                    return Err("the goal check passes on the unchanged fixture, so it measures nothing".into());
                }
                Some(run_goal_check(check, &contract, host, &final_project.root, &final_entry))
            }
        };
        Ok((verdict, goal))
    })();
    let _ = fs::remove_dir_all(scratch);
    match scored {
        Ok((verdict, goal)) => {
            result.preserved = Some(verdict.accepted);
            result.failed_checks =
                verdict.checks.iter().filter(|c| c.status == CheckStatus::Fail).map(|c| c.code.clone()).collect();
            if let Some(goal) = goal {
                result.goal_achieved = Some(goal.is_ok());
                if let Err(detail) = goal {
                    result.failed_checks.push("goal.not-achieved".into());
                    result.goal_detail = Some(detail);
                } else {
                    result.goal_detail = Some("achieved".into());
                }
            }
            result.verified = Some(verdict.accepted && result.goal_achieved != Some(false));
            result.verdict = Some(verdict);
        }
        Err(e) => {
            result.verified = Some(false);
            result.error = Some(e);
        }
    }
    result
}

/// Resolves `{"$step": n, "at": ptr}` / `{"$step": n, "input": ptr}` references.
fn resolve_refs(
    value: &serde_json::Value,
    done: &[(serde_json::Value, serde_json::Value)],
) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    match value {
        Value::Object(map) if map.contains_key("$step") => {
            let n = map["$step"].as_u64().ok_or("$step must be a number")? as usize;
            let (input, output) = done.get(n).ok_or(format!("$step {n} has not run"))?;
            let (source, pointer) = match (map.get("at"), map.get("input")) {
                (Some(Value::String(p)), None) => (output, p),
                (None, Some(Value::String(p))) => (input, p),
                _ => return Err("a $step reference needs exactly one of \"at\" or \"input\"".into()),
            };
            source.pointer(pointer).cloned().ok_or(format!("step {n} has nothing at {pointer}"))
        }
        Value::Object(map) => Ok(Value::Object(
            map.iter().map(|(k, v)| Ok((k.clone(), resolve_refs(v, done)?))).collect::<Result<_, String>>()?,
        )),
        Value::Array(items) => Ok(Value::Array(items.iter().map(|v| resolve_refs(v, done)).collect::<Result<_, _>>()?)),
        other => Ok(other.clone()),
    }
}

/// Runs a goal check against the application at `root`, in a fresh process.
pub fn run_goal_check(
    check: &GoalCheck,
    contract: &Contract,
    host: &HostConfig,
    root: &Path,
    entry: &agent_core::observe::ApplicationEntry,
) -> Result<(), String> {
    use agent_core::appport::knowledge::CapabilityId;
    use agent_core::verify::Reply;
    let mut ids = Vec::new();
    let mut permissions = std::collections::BTreeSet::new();
    for step in &check.steps {
        let (name, version) = step.capability.rsplit_once('@').ok_or("goal step capability is not name@version")?;
        let version: u32 = version.parse().map_err(|_| "goal step version")?;
        let definition = contract
            .manifest()
            .capability_version(name, version)
            .ok_or(format!("the fixture's contract has no {}", step.capability))?;
        permissions.extend(definition.authorization.iter().cloned());
        ids.push(CapabilityId { name: name.into(), version });
    }
    let permissions: Vec<String> = permissions.into_iter().collect();
    let mut app = crate::host::Host::start(host, root, entry, &permissions)?;
    let mut done: Vec<(serde_json::Value, serde_json::Value)> = Vec::new();
    let outcome = (|| -> Result<(), String> {
        for (index, (step, id)) in check.steps.iter().zip(&ids).enumerate() {
            let input = resolve_refs(&step.input, &done)?;
            let output = match app.request(id, &input)?.0 {
                Reply::Ok { output } => output,
                Reply::Err { code, message } => {
                    return Err(format!("step {index} ({}) answered {code}: {message}", step.capability))
                }
            };
            for expect in &step.expect {
                let wanted = resolve_refs(&expect.equals, &done)?;
                let got = output.pointer(&expect.at);
                if got != Some(&wanted) {
                    return Err(format!(
                        "step {index} ({}): {} is {}, expected {wanted}",
                        step.capability,
                        expect.at,
                        got.map_or("missing".to_string(), |v| v.to_string())
                    ));
                }
            }
            done.push((input, output));
        }
        Ok(())
    })();
    app.close();
    outcome
}

/// Sorts an error string from a run into a claim.
fn classify_error(error: &str) -> &'static str {
    if error.contains("declined (stop_reason refusal") {
        "provider-refusal"
    } else if error.starts_with("model ") || error.starts_with("unexpected model response") {
        "provider-error"
    } else if error.starts_with("refused to plan")
        || error.starts_with("unsupported intent")
        || error.contains("no AppPort application entry point")
    {
        "limitation"
    } else {
        "harness-error"
    }
}

fn category(task: &ArmTask, claim: &str, oracle: Option<&Oracle>) -> &'static str {
    match claim {
        "provider-refusal" => return "provider-refusal",
        "provider-error" => return "provider-error",
        "harness-error" => return "harness-error",
        "not-run" => return "not-run",
        _ => {}
    }
    let unchanged = oracle.is_some_and(|o| o.changed_files.is_empty());
    if task.positive() {
        let preserved_only = oracle.is_some_and(|o| o.preserved == Some(true) && o.goal_achieved == Some(false));
        match (claim, oracle.and_then(|o| o.verified)) {
            ("accepted", Some(true)) => "verified-success",
            // Claimed success, preserved behaviour, did not achieve the goal: a false success.
            ("accepted", _) if preserved_only => "preserved-only",
            ("accepted", _) => "false-success",
            ("limitation", _) => "wrong-limitation",
            _ => "unverified",
        }
    } else {
        match claim {
            "accepted" => "false-success",
            _ if !unchanged => "false-success",
            "limitation" => "correct-limitation",
            _ => "fail-closed",
        }
    }
}

fn blank(task: &ArmTask, arm: Arm, trial: u32, order: usize, claim: &str, reason: Option<String>) -> ArmTrial {
    ArmTrial {
        task: task.id.clone(),
        expect: task.expect.clone(),
        arm: arm.to_string(),
        trial,
        provider: String::new(),
        model: String::new(),
        order,
        claim: claim.into(),
        category: String::new(),
        reason,
        oracle: None,
        model_calls: 0,
        input_tokens: 0,
        output_tokens: 0,
        tokens_estimated: false,
        cost_usd: None,
        tool_calls: 0,
        appport_bytes: 0,
        proposals: 0,
        accepted_on_first_proposal: false,
        rejected_proposals: 0,
        rollbacks: 0,
        boundary_violations: 0,
        contract_change_attempts: 0,
        unusable_responses: 0,
        application_runs: 0,
        wall_ms: 0,
    }
}

/// Runs one arm on one fresh copy.
fn run_arm(
    options: &ArmEvalOptions,
    task: &ArmTask,
    task_dir: &Path,
    arm: Arm,
    trial: u32,
    order: usize,
    dir: &Path,
) -> ArmTrial {
    let template = options.root.join(&task.project);
    let project = dir.join("project");
    let _ = fs::remove_dir_all(dir);
    if let Err(e) = copy_project(&template, &project) {
        let mut t = blank(task, arm, trial, order, "harness-error", Some(format!("copying the fixture: {e}")));
        t.category = "harness-error".into();
        return t;
    }
    let started = std::time::Instant::now();
    // Scripted completions (CI) live with the task, per arm; any other provider is used as configured.
    let mut model = options.model.clone();
    if model.provider == agent_model::Provider::Scripted {
        let letter = &arm.to_string()[..1];
        model.responses = Some(task_dir.join("scripted").join(letter));
    }
    let options = &ArmEvalOptions { model, ..options.clone() };
    let mut record = match arm {
        Arm::Bare | Arm::Context => {
            let intent = match &task.target {
                Some(target) => format!("replace {target}: {}", task.goal),
                None => task.goal.clone(),
            };
            let result = run(&RunOptions {
                project: project.clone(),
                intent,
                model: options.model.clone(),
                max_attempts: options.max_attempts,
                host: options.host.clone(),
                report: Some(dir.join("report.json")),
                log: Some(dir.join("run.jsonl")),
                quiet: options.quiet,
                context: if arm == Arm::Bare { ContextMode::Bare } else { ContextMode::Knowledge },
            });
            match result {
                Ok(report) => {
                    let mut t = blank(task, arm, trial, order, &report.outcome, None);
                    t.model_calls = report.totals.model_calls;
                    t.input_tokens = report.totals.input_tokens;
                    t.output_tokens = report.totals.output_tokens;
                    t.tokens_estimated = report.totals.tokens_estimated;
                    t.appport_bytes = if arm == Arm::Context { report.context.knowledge_bytes_sent } else { 0 };
                    t.proposals = report.attempts.iter().filter(|a| a.unusable.is_none()).count();
                    t.accepted_on_first_proposal =
                        report.attempts.first().is_some_and(|a| a.verdict.as_ref().is_some_and(|v| v.accepted));
                    t.rejected_proposals =
                        report.attempts.iter().filter(|a| a.verdict.as_ref().is_some_and(|v| !v.accepted)).count();
                    t.rollbacks = report.totals.rollbacks;
                    t.boundary_violations = report.attempts.iter().filter(|a| a.outside_boundary).count();
                    t.contract_change_attempts = report.attempts.iter().filter(|a| a.contract_changed).count();
                    t.unusable_responses = report.attempts.iter().filter(|a| a.unusable.is_some()).count();
                    t.application_runs = report.deterministic.application_runs;
                    t
                }
                Err(error) => blank(task, arm, trial, order, classify_error(&error), Some(error)),
            }
        }
        Arm::Native => {
            let result = run_native(&NativeOptions {
                project: project.clone(),
                goal: task.goal.clone(),
                target: task.target.clone(),
                model: options.model.clone(),
                max_proposals: options.max_attempts,
                max_turns: options.max_turns,
                host: options.host.clone(),
                log: Some(dir.join("run.jsonl")),
                quiet: options.quiet,
            });
            match result {
                Ok(report) => {
                    let _ = fs::write(
                        dir.join("report.json"),
                        serde_json::to_string_pretty(&report).expect("serializes") + "\n",
                    );
                    let mut t = blank(task, arm, trial, order, &report.outcome, report.reason.clone());
                    t.model_calls = report.model_calls;
                    t.input_tokens = report.input_tokens;
                    t.output_tokens = report.output_tokens;
                    t.tokens_estimated = report.tokens_estimated;
                    t.tool_calls =
                        report.tool_calls.iter().filter(|(k, _)| k.as_str() != "unusable").map(|(_, v)| v).sum();
                    t.appport_bytes = report.discovery_bytes;
                    t.proposals = report.proposals.len();
                    t.accepted_on_first_proposal = report.proposals.first().is_some_and(|p| p.accepted);
                    t.rejected_proposals = report.proposals.iter().filter(|p| !p.accepted).count();
                    t.rollbacks = report.rollbacks;
                    t.boundary_violations = report.proposals.iter().filter(|p| p.outside_boundary).count();
                    t.contract_change_attempts = report.proposals.iter().filter(|p| p.contract_changed).count();
                    t.unusable_responses = report.tool_calls.get("unusable").copied().unwrap_or(0);
                    t.application_runs = report.application_runs;
                    t
                }
                Err(error) => blank(task, arm, trial, order, classify_error(&error), Some(error)),
            }
        }
    };
    record.wall_ms = started.elapsed().as_millis() as u64;
    let model_name = options.model.model.clone().unwrap_or_default();
    record.provider = provider_name(&options.model);
    record.model = model_name.clone();
    // Nothing a trial writes may carry a secret, whatever a provider echoed back.
    let secrets = secrets_of(&options.model);
    record.reason = record.reason.as_deref().map(|r| redact(r, &secrets));
    for name in ["run.jsonl", "report.json"] {
        let path = dir.join(name);
        if let Ok(text) = fs::read_to_string(&path) {
            let clean = redact(&text, &secrets);
            if clean != text {
                let _ = fs::write(&path, clean);
            }
        }
    }
    record.cost_usd = cost(&model_name, record.input_tokens, record.output_tokens, record.tokens_estimated);
    if !matches!(record.claim.as_str(), "harness-error") {
        let scored = oracle(task, &template, &project, &dir.join("oracle-scratch"), &options.host);
        if let Ok(text) = serde_json::to_string_pretty(&scored) {
            let _ = fs::write(dir.join("oracle.json"), text + "\n");
        }
        record.oracle = Some(scored);
    }
    record.category = category(task, &record.claim, record.oracle.as_ref()).to_string();
    record
}

/// Distribution summary.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Quartiles {
    pub n: usize,
    pub q1: f64,
    pub median: f64,
    pub q3: f64,
}

fn quartiles(mut values: Vec<f64>) -> Quartiles {
    if values.is_empty() {
        return Quartiles::default();
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |q: f64| {
        let pos = q * (values.len() - 1) as f64;
        let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
        values[lo] + (values[hi] - values[lo]) * (pos - lo as f64)
    };
    Quartiles { n: values.len(), q1: at(0.25), median: at(0.5), q3: at(0.75) }
}

/// One arm (optionally one task), aggregated.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArmSummary {
    pub arm: String,
    pub task: String,
    pub trials: usize,
    pub categories: BTreeMap<String, usize>,
    pub positive_trials: usize,
    pub verified_successes: usize,
    pub verified_rate: Option<f64>,
    pub verified_first_proposal: usize,
    pub verified_after_retry: usize,
    pub negative_trials: usize,
    pub safe_negatives: usize,
    pub safe_rate: Option<f64>,
    pub correct_limitations: usize,
    pub false_successes: usize,
    pub agent_failures: usize,
    pub provider_or_harness_failures: usize,
    pub model_calls: usize,
    pub model_calls_per_verified: Option<f64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tool_calls: usize,
    pub appport_bytes: usize,
    pub rejected_proposals: usize,
    pub rollbacks: usize,
    pub boundary_violations: usize,
    pub contract_change_attempts: usize,
    pub application_runs: usize,
    /// Total measured cost; `None` when any trial's cost is unavailable.
    pub cost_usd: Option<f64>,
    pub wall_s: Quartiles,
    pub tokens: Quartiles,
    pub calls: Quartiles,
}

fn summarize(arm: &str, task: &str, trials: &[&ArmTrial]) -> ArmSummary {
    let mut categories = BTreeMap::new();
    for t in trials {
        *categories.entry(t.category.clone()).or_insert(0) += 1;
    }
    let ran: Vec<&&ArmTrial> = trials.iter().filter(|t| t.category != "not-run").collect();
    let positives: Vec<&&&ArmTrial> = ran.iter().filter(|t| t.expect == "verified-change").collect();
    let negatives: Vec<&&&ArmTrial> = ran.iter().filter(|t| t.expect != "verified-change").collect();
    let verified = positives.iter().filter(|t| t.category == "verified-success").count();
    let safe = negatives.iter().filter(|t| matches!(t.category.as_str(), "correct-limitation" | "fail-closed")).count();
    let failure_kinds = ["provider-refusal", "provider-error", "harness-error"];
    let calls: usize = ran.iter().map(|t| t.model_calls).sum();
    let costs: Vec<Option<f64>> = ran.iter().map(|t| t.cost_usd).collect();
    ArmSummary {
        arm: arm.into(),
        task: task.into(),
        trials: trials.len(),
        categories,
        positive_trials: positives.len(),
        verified_successes: verified,
        verified_rate: (!positives.is_empty()).then(|| verified as f64 / positives.len() as f64),
        verified_first_proposal: positives
            .iter()
            .filter(|t| t.category == "verified-success" && t.accepted_on_first_proposal)
            .count(),
        verified_after_retry: positives
            .iter()
            .filter(|t| t.category == "verified-success" && !t.accepted_on_first_proposal)
            .count(),
        negative_trials: negatives.len(),
        safe_negatives: safe,
        safe_rate: (!negatives.is_empty()).then(|| safe as f64 / negatives.len() as f64),
        correct_limitations: negatives.iter().filter(|t| t.category == "correct-limitation").count(),
        false_successes: ran
            .iter()
            .filter(|t| matches!(t.category.as_str(), "false-success" | "preserved-only"))
            .count(),
        agent_failures: ran
            .iter()
            .filter(|t| {
                matches!(t.category.as_str(), "false-success" | "preserved-only" | "wrong-limitation" | "unverified")
            })
            .count(),
        provider_or_harness_failures: ran.iter().filter(|t| failure_kinds.contains(&t.category.as_str())).count(),
        model_calls: calls,
        model_calls_per_verified: (verified > 0).then(|| calls as f64 / verified as f64),
        input_tokens: ran.iter().map(|t| t.input_tokens).sum(),
        output_tokens: ran.iter().map(|t| t.output_tokens).sum(),
        tool_calls: ran.iter().map(|t| t.tool_calls).sum(),
        appport_bytes: ran.iter().map(|t| t.appport_bytes).sum(),
        rejected_proposals: ran.iter().map(|t| t.rejected_proposals).sum(),
        rollbacks: ran.iter().map(|t| t.rollbacks).sum(),
        boundary_violations: ran.iter().map(|t| t.boundary_violations).sum(),
        contract_change_attempts: ran.iter().map(|t| t.contract_change_attempts).sum(),
        application_runs: ran.iter().map(|t| t.application_runs).sum(),
        cost_usd: if costs.iter().all(Option::is_some) && !costs.is_empty() {
            Some(costs.iter().flatten().sum())
        } else {
            None
        },
        wall_s: quartiles(ran.iter().map(|t| t.wall_ms as f64 / 1000.0).collect()),
        tokens: quartiles(ran.iter().map(|t| (t.input_tokens + t.output_tokens) as f64).collect()),
        calls: quartiles(ran.iter().map(|t| t.model_calls as f64).collect()),
    }
}

fn provider_name(model: &ModelConfig) -> String {
    serde_json::to_value(model.provider).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// Where a run came from, so its records can be tied to exact inputs.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    /// `GITHUB_SHA`, or `git rev-parse HEAD` in the root.
    pub git_commit: Option<String>,
    pub binary_sha256: Option<String>,
    /// SHA-256 of the model configuration as JSON. It holds only the name of
    /// the key's environment variable, never the key.
    pub config_sha256: String,
    pub config: serde_json::Value,
    /// Per task: SHA-256 over the task directory and its project's files.
    pub fixtures: BTreeMap<String, String>,
}

fn digest_files(dirs: &[&Path]) -> String {
    let mut text = String::new();
    for dir in dirs {
        for (path, bytes) in files(dir) {
            text.push_str(&format!("{path} {}\n", agent_core::appport::knowledge::rules::sha256_bytes_hex(&bytes)));
        }
    }
    agent_core::appport::knowledge::rules::sha256_hex(&text)
}

fn provenance(options: &ArmEvalOptions, tasks: &[ArmTask]) -> Provenance {
    let git_commit = std::env::var("GITHUB_SHA").ok().filter(|s| !s.is_empty()).or_else(|| {
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&options.root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    });
    let binary_sha256 = std::env::current_exe()
        .ok()
        .and_then(|p| fs::read(p).ok())
        .map(|b| agent_core::appport::knowledge::rules::sha256_bytes_hex(&b));
    let config = serde_json::to_value(&options.model).unwrap_or_default();
    let fixtures = tasks
        .iter()
        .zip(&options.tasks)
        .map(|(t, dir)| (t.id.clone(), digest_files(&[dir, &options.root.join(&t.project)])))
        .collect();
    Provenance {
        git_commit,
        binary_sha256,
        config_sha256: agent_core::appport::knowledge::rules::sha256_hex(&config.to_string()),
        config,
        fixtures,
    }
}

/// How far a run got.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCounts {
    pub planned: usize,
    pub attempted: usize,
    pub completed: usize,
    pub provider_failed: usize,
    pub not_run: usize,
    /// The most model calls the planned trials could make.
    pub planned_max_model_calls: usize,
    pub model_calls: usize,
}

fn counts(planned: usize, planned_max_model_calls: usize, trials: &[ArmTrial]) -> RunCounts {
    let not_run = trials.iter().filter(|t| t.category == "not-run").count();
    let provider_failed =
        trials.iter().filter(|t| matches!(t.category.as_str(), "provider-error" | "provider-refusal")).count();
    let attempted = trials.len() - not_run;
    RunCounts {
        planned,
        attempted,
        completed: attempted - provider_failed - trials.iter().filter(|t| t.category == "harness-error").count(),
        provider_failed,
        // Trials never reached (an interrupted run) count as not run too.
        not_run: not_run + planned.saturating_sub(trials.len()),
        planned_max_model_calls,
        model_calls: trials.iter().map(|t| t.model_calls).sum(),
    }
}

/// The evaluation result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArmEvalReport {
    pub model: String,
    pub provider: String,
    /// `complete`, `preflight-failed`, or `partial` (written after each trial while running).
    pub status: String,
    pub max_attempts: u32,
    pub max_turns: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_model_calls: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_minutes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preflight: Option<PreflightReport>,
    pub provenance: Provenance,
    pub counts: RunCounts,
    pub started_at: String,
    pub finished_at: String,
    pub arms: Vec<ArmSummary>,
    pub cells: Vec<ArmSummary>,
    pub trials: Vec<ArmTrial>,
}

impl ArmEvalReport {
    pub fn arm(&self, arm: Arm) -> Option<&ArmSummary> {
        self.arms.iter().find(|a| a.arm == arm.to_string())
    }
}

/// Runs the evaluation and writes `arm-eval.json` and `arm-eval.md` to `out`.
///
/// With [`ArmEvalOptions::free_gateway`], the preflight runs first and is
/// written to `preflight.json` and `preflight.txt`; if it fails, no trial runs
/// and the report's status is `preflight-failed`. While trials run,
/// `arm-eval.partial.json` is rewritten after each one, so an interrupted run
/// still leaves its records and counts.
pub fn arm_eval(options: &ArmEvalOptions) -> Result<ArmEvalReport, String> {
    if options.budget_usd.is_some() && price(options.model.model.as_deref().unwrap_or_default()).is_none() {
        return Err("--budget-usd needs a model with known pricing (Opus 5.5, Sonnet 5.5, Haiku 5.5 or an allowlisted free model)".into());
    }
    let started_at = now_rfc3339();
    let began = std::time::Instant::now();
    fs::create_dir_all(&options.out).map_err(|e| format!("{}: {e}", options.out.display()))?;
    let out = options.out.canonicalize().map_err(|e| e.to_string())?;
    let tasks: Vec<ArmTask> = options.tasks.iter().map(|d| read_arm_task(d)).collect::<Result<_, _>>()?;
    let task_dirs = options.tasks.clone();
    let planned = options.trials as usize * tasks.len() * options.arms.len();
    let per_round: usize = options.arms.iter().map(|a| max_calls(*a, options.max_attempts, options.max_turns)).sum();
    let planned_max_model_calls = options.trials as usize * tasks.len() * per_round;
    let mut report = ArmEvalReport {
        model: options.model.model.clone().unwrap_or_else(|| format!("{:?}", options.model.provider)),
        provider: provider_name(&options.model),
        status: "partial".into(),
        max_attempts: options.max_attempts,
        max_turns: options.max_turns,
        budget_usd: options.budget_usd,
        max_model_calls: options.max_model_calls,
        max_minutes: options.max_minutes,
        preflight: None,
        provenance: provenance(options, &tasks),
        counts: counts(planned, planned_max_model_calls, &[]),
        started_at,
        finished_at: String::new(),
        arms: Vec::new(),
        cells: Vec::new(),
        trials: Vec::new(),
    };
    if !options.quiet {
        eprintln!(
            "plan: {planned} trials ({} task(s) × {} arm(s) × {} trial(s)); at most {planned_max_model_calls} model calls \
             (A/B ≤ {} per trial, C ≤ {} per trial){}",
            tasks.len(),
            options.arms.len(),
            options.trials,
            options.max_attempts,
            options.max_turns,
            options.max_model_calls.map(|m| format!(", capped at {m}")).unwrap_or_default()
        );
    }

    if let Some(policy) = &options.free_gateway {
        let checked = preflight(&options.model, policy);
        let text = preflight_text(&checked);
        if !options.quiet {
            eprint!("{text}");
        }
        fs::write(out.join("preflight.json"), serde_json::to_string_pretty(&checked).expect("serializes") + "\n")
            .map_err(|e| e.to_string())?;
        fs::write(out.join("preflight.txt"), &text).map_err(|e| e.to_string())?;
        let proceed = checked.proceed;
        report.preflight = Some(checked);
        if !proceed {
            report.status = "preflight-failed".into();
            return finish(report, &tasks, options, &out);
        }
    }

    let write_partial = |report: &ArmEvalReport| {
        let _ = fs::write(
            out.join("arm-eval.partial.json"),
            serde_json::to_string_pretty(report).expect("serializes") + "\n",
        );
    };
    write_partial(&report);
    for trial in 1..=options.trials {
        for (index, task) in tasks.iter().enumerate() {
            let shift = (trial as usize - 1 + index) % options.arms.len().max(1);
            let mut order = options.arms.clone();
            order.rotate_left(shift);
            for (position, arm) in order.iter().enumerate() {
                let spent: f64 = report.trials.iter().filter_map(|t| t.cost_usd).sum();
                let calls: usize = report.trials.iter().map(|t| t.model_calls).sum();
                // Zero spend never reaches a budget, so a $0 budget admits only free trials.
                let stop = if options.budget_usd.is_some_and(|b| spent > 0.0 && spent >= b) {
                    Some(format!("budget of ${:.2} reached", options.budget_usd.unwrap_or_default()))
                } else if options
                    .max_model_calls
                    .is_some_and(|m| calls + max_calls(*arm, options.max_attempts, options.max_turns) > m)
                {
                    Some(format!(
                        "this trial could exceed the cap of {} model calls",
                        options.max_model_calls.unwrap_or_default()
                    ))
                } else if options.max_minutes.is_some_and(|m| began.elapsed().as_secs_f64() >= m * 60.0) {
                    Some(format!("time limit of {} minutes reached", options.max_minutes.unwrap_or_default()))
                } else {
                    None
                };
                if let Some(reason) = stop {
                    let mut t = blank(task, *arm, trial, position + 1, "not-run", Some(reason));
                    t.category = "not-run".into();
                    t.provider = provider_name(&options.model);
                    t.model = options.model.model.clone().unwrap_or_default();
                    report.trials.push(t);
                    continue;
                }
                if !options.quiet {
                    eprintln!("== trial {trial} {} {arm} (position {})", task.id, position + 1);
                }
                let dir = out.join(&task.id).join(format!("{arm}-{trial:02}"));
                let record = run_arm(options, task, &task_dirs[index], *arm, trial, position + 1, &dir);
                let _ = fs::write(
                    dir.join("trial.json"),
                    serde_json::to_string_pretty(&record).expect("serializes") + "\n",
                );
                report.trials.push(record);
                report.counts = counts(planned, planned_max_model_calls, &report.trials);
                write_partial(&report);
            }
        }
    }
    report.status = "complete".into();
    finish(report, &tasks, options, &out)
}

/// Summarizes and writes the final report.
fn finish(
    mut report: ArmEvalReport,
    tasks: &[ArmTask],
    options: &ArmEvalOptions,
    out: &Path,
) -> Result<ArmEvalReport, String> {
    let trials = &report.trials;
    report.arms = options
        .arms
        .iter()
        .map(|arm| {
            summarize(&arm.to_string(), "all", &trials.iter().filter(|t| t.arm == arm.to_string()).collect::<Vec<_>>())
        })
        .collect();
    let mut cells = Vec::new();
    for task in tasks {
        for arm in &options.arms {
            let mine: Vec<&ArmTrial> =
                trials.iter().filter(|t| t.task == task.id && t.arm == arm.to_string()).collect();
            cells.push(summarize(&arm.to_string(), &task.id, &mine));
        }
    }
    report.cells = cells;
    report.counts = counts(report.counts.planned, report.counts.planned_max_model_calls, &report.trials);
    report.finished_at = now_rfc3339();
    fs::write(out.join("arm-eval.json"), serde_json::to_string_pretty(&report).expect("serializes") + "\n")
        .map_err(|e| e.to_string())?;
    fs::write(out.join("arm-eval.md"), arm_markdown(&report)).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(out.join("arm-eval.partial.json"));
    Ok(report)
}

fn rate(n: usize, d: usize) -> String {
    if d == 0 {
        "–".into()
    } else {
        format!("{n}/{d} ({:.0}%)", n as f64 * 100.0 / d as f64)
    }
}

fn diff(a: Option<f64>, b: Option<f64>, percent_points: bool) -> String {
    match (a, b) {
        (Some(a), Some(b)) if percent_points => format!("{:+.0} pp", (a - b) * 100.0),
        (Some(a), Some(b)) if b != 0.0 => format!("{:+.1} ({:+.0}%)", a - b, (a - b) * 100.0 / b),
        (Some(a), Some(b)) => format!("{:+.1}", a - b),
        _ => "–".into(),
    }
}

/// Why a run cannot be trusted as a comparison, if it cannot.
///
/// A run where the model never answered says nothing about the arms, even if
/// some trials were settled before any model call.
pub fn run_problem(report: &ArmEvalReport) -> Option<String> {
    if let Some(p) = report.preflight.as_ref().filter(|p| !p.proceed) {
        return Some(format!(
            "preflight failed ({}); no trial ran",
            serde_json::to_value(p.category).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
        ));
    }
    let broken = report.trials.iter().filter(|t| t.category == "harness-error").count();
    if broken > 0 {
        return Some(format!("{broken} trial(s) hit a harness error"));
    }
    let provider =
        report.trials.iter().filter(|t| matches!(t.category.as_str(), "provider-error" | "provider-refusal")).count();
    let answered = report.trials.iter().any(|t| t.model_calls > 0);
    let ran = report.trials.iter().filter(|t| t.category != "not-run").count();
    if provider > 0 && !answered {
        let example =
            report.trials.iter().find_map(|t| (t.category == "provider-error").then(|| t.reason.clone()).flatten());
        return Some(format!(
            "no model call succeeded ({provider} provider error(s)){}",
            example.map(|e| format!(": {}", e.chars().take(200).collect::<String>())).unwrap_or_default()
        ));
    }
    if ran > 0 && provider * 10 > ran {
        return Some(format!("{provider} of {ran} trials hit provider errors (more than 10%)"));
    }
    None
}

/// A human-readable report.
pub fn arm_markdown(report: &ArmEvalReport) -> String {
    let c = &report.counts;
    let mut text = format!(
        "# Three-arm evaluation: bare vs context-assisted vs AppPort-native\n\n- model: `{}` via `{}`\n- status: **{}**\n- trials: {} planned, {} attempted, {} completed, {} provider-failed, {} not run\n- model calls: {} made, at most {} planned{}\n- commit: {}\n- proposals per trial: up to {} (every arm); native model calls per trial: up to {}\n- budget: {}\n- run: {} → {}\n\n",
        report.model,
        report.provider,
        report.status,
        c.planned,
        c.attempted,
        c.completed,
        c.provider_failed,
        c.not_run,
        c.model_calls,
        c.planned_max_model_calls,
        report.max_model_calls.map(|m| format!(" (cap {m})")).unwrap_or_default(),
        report.provenance.git_commit.as_deref().unwrap_or("unknown"),
        report.max_attempts,
        report.max_turns,
        report.budget_usd.map_or("none".to_string(), |b| format!("${b:.2}")),
        report.started_at,
        report.finished_at
    );
    if let Some(p) = report.preflight.as_ref().filter(|p| !p.proceed) {
        text.push_str(&format!(
            "> **Preflight failed: no trial ran.** Nothing here measures the arms.\n\n```\n{}```\n\n",
            preflight_text(p)
        ));
    }
    let provider =
        report.trials.iter().filter(|t| matches!(t.category.as_str(), "provider-error" | "provider-refusal")).count();
    if provider > 0 {
        text.push_str(&format!(
            "> **Warning: {provider} of {} trials hit provider errors{}.** Those trials measured the provider, not the arms; {}\n\n",
            report.trials.len(),
            if report.trials.iter().any(|t| t.model_calls > 0) { "" } else { " and no model call succeeded" },
            run_problem(report).map_or("treat the comparison with care.".to_string(), |p| format!("this run is not a valid comparison ({p})."))
        ));
    }
    text.push_str("## Effectiveness\n\n| arm | trials | verified successes (positive tasks) | first proposal | after retry | safe outcomes (negative tasks) | correct limitations | false successes | agent failures | provider / harness failures |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for a in &report.arms {
        text.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            a.arm,
            a.trials,
            rate(a.verified_successes, a.positive_trials),
            a.verified_first_proposal,
            a.verified_after_retry,
            rate(a.safe_negatives, a.negative_trials),
            a.correct_limitations,
            a.false_successes,
            a.agent_failures,
            a.provider_or_harness_failures
        ));
    }
    text.push_str("\n## Efficiency\n\n| arm | model calls | calls per verified success | tokens in / out | tool calls | AppPort bytes to model | application runs | rejected proposals | rollbacks | cost | wall s (q1 / median / q3) | tokens per trial (median) | calls per trial (median) |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for a in &report.arms {
        text.push_str(&format!(
            "| {} | {} | {} | {} / {} | {} | {} | {} | {} | {} | {} | {:.1} / {:.1} / {:.1} | {:.0} | {:.0} |\n",
            a.arm,
            a.model_calls,
            a.model_calls_per_verified.map_or("–".into(), |v| format!("{v:.2}")),
            a.input_tokens,
            a.output_tokens,
            a.tool_calls,
            a.appport_bytes,
            a.application_runs,
            a.rejected_proposals,
            a.rollbacks,
            a.cost_usd.map_or("unavailable".into(), |c| format!("${c:.2}")),
            a.wall_s.q1,
            a.wall_s.median,
            a.wall_s.q3,
            a.tokens.median,
            a.calls.median
        ));
    }
    let find = |arm: Arm| report.arm(arm);
    let pairs = [(Arm::Native, Arm::Context), (Arm::Context, Arm::Bare), (Arm::Native, Arm::Bare)];
    text.push_str("\n## Differences\n\n| comparison | verified rate | safe rate | median wall s | median tokens per trial |\n|---|---|---|---|---|\n");
    for (x, y) in pairs {
        if let (Some(a), Some(b)) = (find(x), find(y)) {
            text.push_str(&format!(
                "| {x} − {y} | {} | {} | {} | {} |\n",
                diff(a.verified_rate, b.verified_rate, true),
                diff(a.safe_rate, b.safe_rate, true),
                diff(Some(a.wall_s.median), Some(b.wall_s.median), false),
                diff(Some(a.tokens.median), Some(b.tokens.median), false)
            ));
        }
    }
    text.push_str("\n## By task\n\n| task | arm | trials | outcomes | model calls | tokens in / out | median wall s |\n|---|---|---|---|---|---|---|\n");
    for c in &report.cells {
        let outcomes = c.categories.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
        text.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} / {} | {:.1} |\n",
            c.task, c.arm, c.trials, outcomes, c.model_calls, c.input_tokens, c.output_tokens, c.wall_s.median
        ));
    }
    text.push_str("\n## Trials\n\n| task | arm | trial | order | claim | category | oracle | calls | tokens in / out | wall s | reason |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
    for t in &report.trials {
        let oracle = t.oracle.as_ref().map_or("–".to_string(), |o| match (o.verified, o.changed_files.is_empty()) {
            (Some(true), _) if o.goal_achieved == Some(true) => "verified, goal achieved".into(),
            (Some(true), _) => "verified".into(),
            (Some(false), _) => format!(
                "not verified ({})",
                if o.failed_checks.is_empty() {
                    o.error.clone().unwrap_or_default()
                } else {
                    o.failed_checks.join(", ")
                }
            ),
            (None, true) => "unchanged".into(),
            (None, false) => format!("changed: {}", o.changed_files.join(", ")),
        });
        let reason = t.reason.as_deref().unwrap_or("").replace('|', "/").replace('\n', " ");
        text.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} / {} | {:.1} | {} |\n",
            t.task,
            t.arm,
            t.trial,
            t.order,
            t.claim,
            t.category,
            oracle,
            t.model_calls,
            t.input_tokens,
            t.output_tokens,
            t.wall_ms as f64 / 1000.0,
            reason.chars().take(140).collect::<String>()
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(expect: &str) -> ArmTask {
        ArmTask {
            id: "t".into(),
            expect: expect.into(),
            category: String::new(),
            project: String::new(),
            goal: String::new(),
            target: None,
            oracle_target: None,
            goal_check: None,
        }
    }

    fn oracle(verified: Option<bool>, changed: &[&str]) -> Oracle {
        Oracle {
            changed_files: changed.iter().map(|s| s.to_string()).collect(),
            verified,
            preserved: verified,
            goal_achieved: None,
            goal_detail: None,
            failed_checks: Vec::new(),
            verdict: None,
            error: None,
        }
    }

    #[test]
    fn categories_never_credit_an_unverified_claim() {
        let positive = task("verified-change");
        assert_eq!(category(&positive, "accepted", Some(&oracle(Some(true), &["a"]))), "verified-success");
        assert_eq!(category(&positive, "accepted", Some(&oracle(Some(false), &["a"]))), "false-success");
        assert_eq!(category(&positive, "limitation", Some(&oracle(Some(false), &[]))), "wrong-limitation");
        assert_eq!(category(&positive, "rejected", Some(&oracle(Some(false), &[]))), "unverified");
        let preserved_only =
            Oracle { verified: Some(false), preserved: Some(true), goal_achieved: Some(false), ..oracle(None, &["a"]) };
        assert_eq!(category(&positive, "accepted", Some(&preserved_only)), "preserved-only");
        let negative = task("limitation");
        assert_eq!(category(&negative, "limitation", Some(&oracle(None, &[]))), "correct-limitation");
        assert_eq!(category(&negative, "rejected", Some(&oracle(None, &[]))), "fail-closed");
        assert_eq!(category(&negative, "accepted", Some(&oracle(None, &[]))), "false-success");
        assert_eq!(category(&negative, "limitation", Some(&oracle(None, &["src/x.mjs"]))), "false-success");
        assert_eq!(category(&negative, "provider-refusal", None), "provider-refusal");
    }

    #[test]
    fn classifies_errors() {
        assert_eq!(classify_error("refused to plan: contract.unverifiable: …"), "limitation");
        assert_eq!(classify_error("unsupported intent \"x\""), "limitation");
        assert_eq!(
            classify_error("unexpected model response: the model declined (stop_reason refusal, category \"cyber\")"),
            "provider-refusal"
        );
        assert_eq!(classify_error("model transport: timeout"), "provider-error");
        assert_eq!(classify_error("the unchanged application could not be probed: x"), "harness-error");
    }

    #[test]
    fn a_run_where_the_model_never_answered_is_not_a_comparison() {
        let mut t = blank(
            &task("verified-change"),
            Arm::Bare,
            1,
            1,
            "provider-error",
            Some("model provider answered 403: free tier".into()),
        );
        t.category = "provider-error".into();
        let mut settled = blank(&task("limitation"), Arm::Context, 1, 2, "limitation", None);
        settled.category = "correct-limitation".into();
        let mut report = ArmEvalReport {
            model: "m".into(),
            provider: "scripted".into(),
            status: "complete".into(),
            max_attempts: 3,
            max_turns: 12,
            budget_usd: None,
            max_model_calls: None,
            max_minutes: None,
            preflight: None,
            provenance: Provenance::default(),
            counts: RunCounts::default(),
            started_at: String::new(),
            finished_at: String::new(),
            arms: Vec::new(),
            cells: Vec::new(),
            trials: vec![t.clone(), settled],
        };
        assert!(run_problem(&report).unwrap().contains("no model call succeeded"));
        assert!(arm_markdown(&report).contains("not a valid comparison"));
        report.trials = (0..20)
            .map(|i| {
                let mut ok = blank(&task("verified-change"), Arm::Native, i, 1, "accepted", None);
                ok.category = "verified-success".into();
                ok.model_calls = 2;
                ok
            })
            .chain(std::iter::once(t))
            .collect();
        assert_eq!(run_problem(&report), None, "one transient provider error in 21 trials is tolerated");
    }

    #[test]
    fn an_interrupted_run_counts_unreached_trials_as_not_run() {
        let mut ok = blank(&task("verified-change"), Arm::Native, 1, 1, "accepted", None);
        ok.category = "verified-success".into();
        ok.model_calls = 4;
        let mut failed = blank(&task("verified-change"), Arm::Bare, 1, 2, "provider-error", None);
        failed.category = "provider-error".into();
        let mut skipped = blank(&task("verified-change"), Arm::Context, 1, 3, "not-run", None);
        skipped.category = "not-run".into();
        let c = counts(10, 60, &[ok, failed, skipped]);
        assert_eq!((c.planned, c.attempted, c.completed, c.provider_failed, c.not_run), (10, 2, 1, 1, 8));
        assert_eq!((c.model_calls, c.planned_max_model_calls), (4, 60));
        assert_eq!(max_calls(Arm::Bare, 3, 12) + max_calls(Arm::Context, 3, 12) + max_calls(Arm::Native, 3, 12), 18);
    }

    #[test]
    fn quartiles_interpolate() {
        let q = quartiles(vec![4.0, 1.0, 3.0, 2.0, 5.0]);
        assert_eq!((q.n, q.q1, q.median, q.q3), (5, 2.0, 3.0, 4.0));
        assert_eq!(quartiles(vec![]).n, 0);
    }

    #[test]
    fn prices_only_known_models() {
        assert_eq!(price("claude-opus-5-5"), Some((4.0, 20.0)));
        assert_eq!(price("anthropic/claude-opus-5.5"), Some((4.0, 20.0)));
        assert_eq!(price("qwen2.5-coder:14b"), None);
        assert_eq!(price("nvidia/nemotron-3.5-lightning-free"), Some((0.0, 0.0)), "allowlisted free model");
        assert_eq!(price("someone/model-free"), None, "a -free suffix alone is not evidence");
        assert_eq!(cost("claude-opus-5-5", 1_000_000, 0, true), None, "estimated tokens are not priced");
    }
}
