//! The coding loop for ordinary repositories, and its evaluation.
//!
//! No AppPort knowledge is involved: the agent observes the repository, runs the
//! project's own test command, plans with [`agent_core::generic`], asks the model
//! for the change, and accepts it only when the test command exits 0. Test files
//! are read-only. The evaluation then runs acceptance tests the agent never saw,
//! so an accepted change that only satisfies the visible tests is counted as a
//! failure, not a success.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use agent_core::generic::{is_test_file, plan_coding, tail, Baseline, CodingContext, CodingTask};
use agent_core::observe::{normalize, observe};
use agent_model::gateway::GatewayPolicy;
use agent_model::{connect, Model, ModelConfig, Provider, Request, Usage};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::edit;
use crate::eval::copy_project;
use crate::host::HostConfig;
use crate::report::{now_rfc3339, Log};
use crate::strategy::{
    context_strategy, recovery_policy, ContextInput, ContextSelection, ContextStrategyId, Failure, RecoveryDecision,
    RecoveryInput, RecoveryPolicyId, DEFAULT_CONTEXT_BUDGET,
};

/// Test output handed back to the model after a rejection.
const FEEDBACK_TAIL: usize = 4 * 1024;

/// The result of running a command.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRun {
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub output: String,
}

impl CommandRun {
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

/// Runs `command` in `dir`, capturing stdout and stderr together, without any
/// provider credential in its environment ([`agent_model::gateway::CREDENTIAL_ENV`]
/// and `hidden_env`). On timeout its whole process group is killed.
pub fn run_command(dir: &Path, command: &[String], timeout: Duration, hidden_env: &[String]) -> CommandRun {
    let began = Instant::now();
    let failed = |output: String| CommandRun {
        command: command.to_vec(),
        exit_code: None,
        timed_out: false,
        duration_ms: began.elapsed().as_millis() as u64,
        output,
    };
    let Some((program, args)) = command.split_first() else { return failed("empty command".into()) };
    let mut builder = Command::new(program);
    // The command runs code a model wrote: it never sees a provider credential.
    for name in agent_model::gateway::CREDENTIAL_ENV.iter().copied().chain(hidden_env.iter().map(String::as_str)) {
        builder.env_remove(name);
    }
    // Its own process group, so a timeout stops everything it started.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut builder, 0);
    let mut child = match builder
        .args(args)
        .current_dir(dir)
        .env("CI", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return failed(format!("cannot run {program}: {e}")),
    };
    let mut readers = Vec::new();
    for pipe in [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
        child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        readers.push(std::thread::spawn(move || {
            let mut pipe = pipe;
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        }));
    }
    let deadline = began + timeout;
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if Instant::now() > deadline => {
                #[cfg(unix)]
                let _ = Command::new("kill").args(["-KILL", &format!("-{}", child.id())]).status();
                let _ = child.kill();
                break (child.wait().ok(), true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break (None, false),
        }
    };
    let output: String = readers
        .into_iter()
        .filter_map(|r| r.join().ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    CommandRun {
        command: command.to_vec(),
        exit_code: status.and_then(|s| s.code()),
        timed_out,
        duration_ms: began.elapsed().as_millis() as u64,
        output,
    }
}

/// Which strategies a coding run uses. The default is the baseline: the
/// observed context and feedback retries, exactly as before strategies existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Strategies {
    #[serde(default)]
    pub context: ContextStrategyId,
    #[serde(default)]
    pub recovery: RecoveryPolicyId,
    /// Bound on read-only context, in bytes.
    #[serde(default = "default_budget")]
    pub context_budget: usize,
}

fn default_budget() -> usize {
    DEFAULT_CONTEXT_BUDGET
}

impl Default for Strategies {
    fn default() -> Self {
        Strategies {
            context: ContextStrategyId::default(),
            recovery: RecoveryPolicyId::default(),
            context_budget: DEFAULT_CONTEXT_BUDGET,
        }
    }
}

impl From<CodingContext> for Strategies {
    fn from(context: CodingContext) -> Self {
        Strategies {
            context: match context {
                CodingContext::Observed => ContextStrategyId::Observed,
                CodingContext::Minimal => ContextStrategyId::Minimal,
            },
            ..Strategies::default()
        }
    }
}

/// How to run one coding task.
#[derive(Debug, Clone)]
pub struct CodingOptions {
    /// The repository to change (a copy; it is modified).
    pub repo: PathBuf,
    pub task: CodingTask,
    pub model: ModelConfig,
    pub strategies: Strategies,
    pub max_attempts: u32,
    pub test_timeout: Duration,
    /// For strategies that read an AppPort application's contract.
    pub host: HostConfig,
    pub log: Option<PathBuf>,
    pub quiet: bool,
}

/// One model attempt.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAttempt {
    pub attempt: u32,
    /// The model that answered, as the provider reported it.
    pub model: String,
    pub usage: Usage,
    /// Requests the provider layer retried before this answer.
    pub provider_retries: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    pub prompt_bytes: usize,
    /// Files shown to the model, and bytes of context, for this attempt.
    pub context_files: Vec<String>,
    pub context_bytes: usize,
    pub files_changed: Vec<String>,
    /// Why nothing could be applied (unparseable response, or a boundary violation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unusable: Option<String>,
    pub outside_boundary: bool,
    pub edited_tests: bool,
    /// The test run on the proposal, when it was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_exit: Option<i32>,
    pub test_timed_out: bool,
    pub tests_passed: bool,
    pub accepted: bool,
    pub rolled_back: bool,
    /// The recovery policy's decision after this attempt was rejected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryDecision>,
    pub duration_ms: u64,
}

/// The strategies a run used, with their versions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyRecord {
    pub context: String,
    pub context_version: u32,
    pub recovery: String,
    pub recovery_version: u32,
    pub context_budget: usize,
}

/// The report of one coding run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingReport {
    pub task: String,
    /// The context strategy (kept for compatibility; see `strategies`).
    pub context: String,
    pub strategies: StrategyRecord,
    pub model: String,
    /// - `accepted`: the project's own test command passed on an applied change;
    /// - `rejected`: no attempt passed (see `stoppedBy` when recovery stopped early);
    /// - `refused`: the planner could not bound the change;
    /// - `provider-error`: a model call failed; nothing is left applied;
    /// - `infrastructure-error`: the test command could not run;
    /// - `rollback-failed`: a rejected change could not be undone and checked.
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// Why the recovery policy stopped before the attempt budget ran out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_by: Option<String>,
    pub baseline_passed: bool,
    pub failing_tests: Vec<String>,
    pub editable: Vec<String>,
    /// What the context strategy selected for the first attempt, item by item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_selection: Option<ContextSelection>,
    pub attempts: Vec<CodingAttempt>,
    /// Times the agent ran the project's test command (baseline included).
    pub test_runs: usize,
    pub model_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tokens_estimated: bool,
    pub rollbacks: usize,
    /// Files the accepted change touched.
    pub accepted_files: Vec<String>,
    pub wall_ms: u64,
    pub started_at: String,
}

/// Runs the loop on `options.repo`.
///
/// The strategies choose what the model sees and what follows a rejection.
/// What may be written (the planner's editable files) and what counts as done
/// (the project's own test command) are the same for every strategy.
pub fn run_coding(options: &CodingOptions) -> Result<CodingReport, String> {
    let mut log = Log::new(options.log.as_deref(), options.quiet)
        .map_err(|e| e.to_string())?
        .redacting(agent_model::gateway::secrets_of(&options.model));
    let secrets = agent_model::gateway::secrets_of(&options.model);
    let hidden = agent_model::gateway::credential_env(&options.model);
    let host = options.host.hiding(&options.model);
    let started_at = now_rfc3339();
    let task = &options.task;
    let strategy = context_strategy(options.strategies.context);
    let policy = recovery_policy(options.strategies.recovery);
    let mut model: Box<dyn Model> = connect(&options.model).map_err(|e| e.to_string())?;
    let mut report = CodingReport {
        task: task.id.clone(),
        context: options.strategies.context.to_string(),
        strategies: StrategyRecord {
            context: strategy.id().to_string(),
            context_version: strategy.version(),
            recovery: policy.id().to_string(),
            recovery_version: policy.version(),
            context_budget: options.strategies.context_budget,
        },
        model: model.describe(),
        outcome: "rejected".into(),
        refusal: None,
        stopped_by: None,
        baseline_passed: false,
        failing_tests: Vec::new(),
        editable: Vec::new(),
        context_selection: None,
        attempts: Vec::new(),
        test_runs: 0,
        model_calls: 0,
        input_tokens: 0,
        output_tokens: 0,
        tokens_estimated: false,
        rollbacks: 0,
        accepted_files: Vec::new(),
        wall_ms: 0,
        started_at,
    };
    let finish = |mut report: CodingReport, log: &mut Log| -> Result<CodingReport, String> {
        report.model_calls = report.attempts.len();
        report.input_tokens = report.attempts.iter().map(|a| a.usage.input_tokens).sum();
        report.output_tokens = report.attempts.iter().map(|a| a.usage.output_tokens).sum();
        report.tokens_estimated = report.attempts.iter().any(|a| a.usage.estimated);
        report.rollbacks = report.attempts.iter().filter(|a| a.rolled_back).count();
        report.wall_ms = log.elapsed_ms() as u64;
        log.event(
            "done",
            json!({ "report": &report }),
            format!(
                "{}: {} model call(s), {} test run(s)",
                report.outcome.to_uppercase(),
                report.model_calls,
                report.test_runs
            ),
        );
        Ok(report)
    };

    let project = observe(&options.repo).map_err(|e| format!("{}: {e}", options.repo.display()))?;
    let root = project.root.clone();
    log.event(
        "observe",
        json!({ "sources": project.sources.len() }),
        format!("{} source file(s)", project.sources.len()),
    );

    let first = run_command(&root, &task.test, options.test_timeout, &hidden);
    report.test_runs += 1;
    log.event(
        "baseline",
        json!({ "run": &first }),
        format!("`{}` → exit {:?} in {} ms", task.test.join(" "), first.exit_code, first.duration_ms),
    );
    // A command that did not run says nothing about the code.
    if first.exit_code.is_none() && !first.timed_out {
        report.outcome = "infrastructure-error".into();
        report.refusal =
            Some(format!("the test command could not run: {}", first.output.chars().take(300).collect::<String>()));
        return finish(report, &mut log);
    }
    let baseline = Baseline { passed: first.passed(), output: first.output.clone() };
    report.baseline_passed = baseline.passed;

    let plan = match plan_coding(&project, &task.intent, &task.test, &baseline) {
        Ok(plan) => plan,
        Err(refusal) => {
            log.event("plan", json!({ "refusal": &refusal }), format!("refused: {refusal}"));
            report.outcome = "refused".into();
            report.refusal = Some(refusal.to_string());
            return finish(report, &mut log);
        }
    };
    for decision in &plan.decisions {
        log.event("plan", json!({ "decision": decision }), format!("{}: {}", decision.step, decision.fact));
    }
    report.failing_tests = plan.failing_tests.clone();
    let editable: Vec<String> = plan.editable().iter().map(|s| s.to_string()).collect();
    report.editable = editable.clone();

    let mut requested: Vec<(String, String)> = Vec::new();
    let select = |requested: &[(String, String)]| {
        strategy.select(&ContextInput {
            project: &project,
            plan: &plan,
            baseline: &baseline,
            budget: options.strategies.context_budget,
            host: &host,
            requested,
        })
    };
    let mut selection = select(&requested)?;
    log.event(
        "context",
        json!({ "selection": &selection }),
        format!(
            "{} v{}: {} file(s), {} fact(s), {} bytes; {} unknown(s)",
            selection.strategy,
            selection.version,
            selection.files().len(),
            selection.items.iter().filter(|i| i.kind == "fact" && i.included).count(),
            selection.total_bytes,
            selection.unknowns.len()
        ),
    );
    report.context_selection = Some(selection.clone());

    let mut feedback: Vec<String> = Vec::new();
    let mut failures: Vec<Failure> = Vec::new();
    for attempt in 1..=options.max_attempts.max(1) {
        let began = Instant::now();
        let task_text = selection.task(&plan, &baseline, &feedback);
        log.event(
            "model",
            json!({ "attempt": attempt, "task": &task_text }),
            format!("attempt {attempt}: asking for the change"),
        );
        let request = Request::new(task_text.system.clone(), task_text.prompt.clone(), options.model.max_tokens);
        let response = match model.complete(&request) {
            Ok(response) => response,
            Err(e) => {
                // Nothing is applied between attempts: the repository is as the last rollback left it.
                let message = agent_model::gateway::redact(&e.to_string(), &secrets);
                log.event(
                    "model",
                    json!({ "attempt": attempt, "error": &message }),
                    format!("attempt {attempt}: {message}"),
                );
                report.outcome = "provider-error".into();
                report.refusal = Some(message);
                return finish(report, &mut log);
            }
        };
        log.event(
            "model",
            json!({ "attempt": attempt, "response": &response }),
            format!(
                "attempt {attempt}: {} in / {} out tokens",
                response.usage.input_tokens, response.usage.output_tokens
            ),
        );
        let mut record = CodingAttempt {
            attempt,
            model: response.model.clone(),
            usage: response.usage,
            provider_retries: response.retries,
            stop_reason: response.stop_reason.clone(),
            prompt_bytes: task_text.system.len() + task_text.prompt.len(),
            context_files: selection.files().iter().map(|s| s.to_string()).collect(),
            context_bytes: selection.total_bytes,
            files_changed: Vec::new(),
            unusable: None,
            outside_boundary: false,
            edited_tests: false,
            test_exit: None,
            test_timed_out: false,
            tests_passed: false,
            accepted: false,
            rolled_back: false,
            recovery: None,
            duration_ms: 0,
        };

        let parsed = edit::parse(&response.text)
            .or_else(|problem| edit::single_fenced(&response.text, &editable).map(|e| vec![e]).ok_or(problem));
        let edits = parsed.and_then(|e| edit::targets(&e).map(|t| (e, t)));
        let failure = 'attempt: {
            let (edits, targets) = match edits {
                Ok(parsed) => parsed,
                Err(problem) => {
                    log.event(
                        "apply",
                        json!({ "attempt": attempt, "unusable": &problem }),
                        format!("attempt {attempt}: unusable: {problem}"),
                    );
                    feedback = vec![format!("Your reply could not be applied: {problem}")];
                    record.unusable = Some(problem.clone());
                    break 'attempt Failure::Unusable { reason: problem };
                }
            };
            record.edited_tests = targets.iter().any(|t| is_test_file(t));
            let outside: Vec<&String> = targets.iter().filter(|t| !editable.contains(t)).collect();
            record.outside_boundary = !outside.is_empty();
            if record.outside_boundary {
                let problem = format!(
                    "it changes {}, outside the editable files ({}){}",
                    outside.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                    editable.join(", "),
                    if record.edited_tests { "; test files are read-only" } else { "" }
                );
                log.event(
                    "apply",
                    json!({ "attempt": attempt, "outside": &outside }),
                    format!("attempt {attempt}: not applied: {problem}"),
                );
                feedback = vec![format!("The previous reply was not applied: {problem}.")];
                record.unusable = Some(problem.clone());
                break 'attempt Failure::Unusable { reason: problem };
            }
            let applied = match edit::apply(&root, &edits) {
                Ok(applied) => applied,
                Err(e) if e.rollback.as_ref().is_some_and(|r| !r.verified()) => {
                    report.outcome = "rollback-failed".into();
                    report.refusal = Some(e.to_string());
                    report.attempts.push(record);
                    return finish(report, &mut log);
                }
                Err(e) => {
                    feedback = vec![format!("The previous reply was not applied: {}.", e.message)];
                    record.unusable = Some(e.message.clone());
                    break 'attempt Failure::Unusable { reason: e.message };
                }
            };
            record.files_changed = applied.changed.clone();
            if applied.changed.is_empty() {
                feedback = vec!["The previous reply changed nothing.".into()];
                record.unusable = Some("identical to the current files".into());
                break 'attempt Failure::Unusable { reason: "identical to the current files".into() };
            }
            let run = run_command(&root, &task.test, options.test_timeout, &hidden);
            report.test_runs += 1;
            record.test_exit = run.exit_code;
            record.test_timed_out = run.timed_out;
            record.tests_passed = run.passed();
            log.event(
                "verify",
                json!({ "attempt": attempt, "run": &run }),
                format!(
                    "attempt {attempt}: `{}` → exit {:?}{}: {}",
                    task.test.join(" "),
                    run.exit_code,
                    if run.timed_out { " (timed out)" } else { "" },
                    if run.passed() { "ACCEPTED" } else { "REJECTED" }
                ),
            );
            if run.passed() {
                record.accepted = true;
                record.duration_ms = began.elapsed().as_millis() as u64;
                report.accepted_files = record.files_changed.clone();
                report.attempts.push(record);
                report.outcome = "accepted".into();
                return finish(report, &mut log);
            }
            let undone = edit::rollback(&root, applied);
            record.rolled_back = undone.verified();
            if !undone.verified() {
                log.event(
                    "apply",
                    json!({ "attempt": attempt, "rollback": &undone }),
                    format!("attempt {attempt}: could not be undone cleanly: {}", undone.describe()),
                );
                report.outcome = "rollback-failed".into();
                report.refusal = Some(undone.describe());
                report.attempts.push(record);
                return finish(report, &mut log);
            }
            if run.exit_code.is_none() && !run.timed_out {
                report.outcome = "infrastructure-error".into();
                report.refusal = Some(format!(
                    "the test command could not run: {}",
                    run.output.chars().take(300).collect::<String>()
                ));
                report.attempts.push(record);
                return finish(report, &mut log);
            }
            feedback = vec![format!(
                "`{}` failed (exit {:?}{}). Its output ends with:\n```\n{}\n```",
                task.test.join(" "),
                run.exit_code,
                if run.timed_out { ", timed out" } else { "" },
                tail(&run.output, FEEDBACK_TAIL).trim_end()
            )];
            Failure::Tests { output: run.output, timed_out: run.timed_out }
        };
        failures.push(failure);
        record.duration_ms = began.elapsed().as_millis() as u64;
        if attempt < options.max_attempts.max(1) {
            let shown = selection.files();
            let decision =
                policy.decide(&RecoveryInput { project: &project, plan: &plan, failures: &failures, shown: &shown });
            log.event(
                "recover",
                json!({ "attempt": attempt, "decision": &decision }),
                format!("attempt {attempt}: {decision:?}"),
            );
            record.recovery = Some(decision.clone());
            report.attempts.push(record);
            match decision {
                RecoveryDecision::RetryWithFeedback => {}
                RecoveryDecision::RetryWithContext { add } => {
                    // Read-only context only: the editable files never change.
                    requested.extend(add.into_iter().filter(|(p, _)| !editable.contains(p)));
                    selection = select(&requested)?;
                }
                RecoveryDecision::Stop { reason } => {
                    report.stopped_by = Some(reason);
                    break;
                }
            }
        } else {
            report.attempts.push(record);
        }
    }
    finish(report, &mut log)
}

/// Reads `<dir>/task.json`.
pub fn read_task(dir: &Path) -> Result<CodingTask, String> {
    let path = dir.join("task.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// One arm of an evaluation: a named set of strategies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingArm {
    pub id: String,
    #[serde(flatten)]
    pub strategies: Strategies,
}

impl CodingArm {
    /// The arm `code-eval --arms observed,minimal` has always meant.
    pub fn of(context: CodingContext) -> CodingArm {
        CodingArm { id: context.to_string(), strategies: context.into() }
    }

    /// The components this arm sets differently from `other`.
    pub fn differs_from(&self, other: &CodingArm) -> Vec<&'static str> {
        let (a, b) = (&self.strategies, &other.strategies);
        let mut out = Vec::new();
        if a.context != b.context {
            out.push("context");
        }
        if a.recovery != b.recovery {
            out.push("recovery");
        }
        if a.context_budget != b.context_budget {
            out.push("contextBudget");
        }
        out
    }
}

/// Limits on a whole evaluation. A trial that could cross one is not started
/// and is recorded as `not-run`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    /// Never start a trial that could take the run past this many model calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_model_calls: Option<usize>,
    /// Stop starting trials after this many minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_minutes: Option<f64>,
    /// Stop starting trials once measured spend (above zero) reaches this. Needs known pricing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
}

/// How to run the coding evaluation.
#[derive(Debug, Clone)]
pub struct CodingEvalOptions {
    /// Names the run in its report.
    pub experiment: String,
    /// The repository root, for provenance (the commit).
    pub root: PathBuf,
    /// Task directories: each holds `task.json`, `repo/`, optional `hidden/` and `scripted/`.
    pub tasks: Vec<PathBuf>,
    pub model: ModelConfig,
    pub arms: Vec<CodingArm>,
    /// The arm others are compared with (default: the first).
    pub baseline: Option<String>,
    pub trials: u32,
    pub max_attempts: u32,
    pub test_timeout: Duration,
    pub host: HostConfig,
    pub limits: Limits,
    /// Rotate the arm order per trial and task, so no arm always runs first.
    pub balanced_order: bool,
    /// Require the free-gateway preflight before any trial. Always applied when
    /// the model is configured at the Vercel AI Gateway.
    pub free_gateway: Option<GatewayPolicy>,
    /// The resolved experiment specification, recorded as given.
    pub spec: Option<serde_json::Value>,
    pub out: PathBuf,
    pub quiet: bool,
}

impl CodingEvalOptions {
    /// The defaults `code-eval` has always used.
    pub fn new(tasks: Vec<PathBuf>, model: ModelConfig, arms: Vec<CodingArm>, out: PathBuf) -> Self {
        CodingEvalOptions {
            experiment: "code-eval".into(),
            root: PathBuf::from("."),
            tasks,
            model,
            arms,
            baseline: None,
            trials: 5,
            max_attempts: 3,
            test_timeout: Duration::from_secs(120),
            host: HostConfig::default(),
            limits: Limits::default(),
            balanced_order: true,
            free_gateway: None,
            spec: None,
            out,
            quiet: false,
        }
    }
}

/// One trial.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingTrial {
    pub task: String,
    pub arm: String,
    pub strategies: Strategies,
    pub trial: u32,
    /// 1-based position of this arm in this trial's order.
    pub order: usize,
    /// What the loop reported: `accepted`, `rejected`, `refused`, `provider-error`,
    /// `infrastructure-error`, `rollback-failed`, `error` (the harness) or `not-run`.
    pub outcome: String,
    /// The scored result; see [`classify`].
    pub category: String,
    /// Why, in words, for every category but `verified-success`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether a change was ever applied (a reply parsed and inside the boundary).
    pub patch_applied: bool,
    /// The project's own test command passed on the final change.
    pub tests_passed: bool,
    /// The hidden acceptance tests passed on the final state of the repository.
    pub hidden_passed: Option<bool>,
    /// Verified: accepted by the project's tests **and** by the hidden acceptance tests when the task has them.
    pub success: bool,
    pub attempts: usize,
    pub model_calls: usize,
    /// Distinct model ids the provider reported.
    pub models_reported: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tokens_estimated: bool,
    pub provider_retries: u32,
    pub recovery_decisions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_by: Option<String>,
    pub context_bytes: usize,
    pub test_runs: usize,
    pub rollbacks: usize,
    pub unusable_responses: usize,
    pub boundary_violations: usize,
    pub test_edit_attempts: usize,
    /// The accepted change touched only the files a person would.
    pub stayed_in_intended_files: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub started_at: String,
    pub wall_ms: u64,
}

/// Scores a trial from what the loop did and what the hidden tests found. A
/// trial is a `verified-success` only with execution evidence: the project's
/// tests passed on an applied change, and so did the hidden acceptance tests
/// when the task declares them. Provider and infrastructure failures are never
/// counted as task outcomes.
pub fn classify(trial: &CodingTrial, has_hidden: bool) -> (&'static str, Option<String>) {
    match trial.outcome.as_str() {
        "not-run" => ("not-run", trial.reason.clone()),
        "provider-error" => ("provider-error", trial.error.clone()),
        "infrastructure-error" | "error" | "rollback-failed" => ("infrastructure-error", trial.error.clone()),
        "refused" => ("limitation", trial.error.clone()),
        "accepted" => match (trial.hidden_passed, has_hidden) {
            (Some(true), _) | (None, false) if trial.tests_passed => ("verified-success", None),
            (Some(false), _) => (
                "accepted-unverified",
                Some("the project's tests passed, but the hidden acceptance tests failed".into()),
            ),
            _ => (
                "infrastructure-error",
                Some("required evidence is missing: the hidden acceptance tests did not run".into()),
            ),
        },
        _ if !trial.patch_applied => {
            ("patch-failed", Some(format!("no reply could be applied in {} attempt(s)", trial.attempts)))
        }
        _ if trial.stopped_by.is_some() => {
            ("rejected", trial.stopped_by.clone().map(|s| format!("stopped by recovery: {s}")))
        }
        _ => (
            "rejected",
            Some(format!("the project's tests failed on every applied change ({} attempt(s))", trial.attempts)),
        ),
    }
}

/// The categories, in report order.
pub const CATEGORIES: [&str; 8] = [
    "verified-success",
    "accepted-unverified",
    "rejected",
    "timed-out",
    "patch-failed",
    "limitation",
    "provider-error",
    "infrastructure-error",
];

/// One task × arm (or one arm over every task), aggregated.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingCell {
    pub task: String,
    pub arm: String,
    pub trials: usize,
    /// Trials that ran (not `not-run`).
    pub ran: usize,
    pub categories: BTreeMap<String, usize>,
    pub successes: usize,
    pub accepted: usize,
    pub accepted_first_attempt: usize,
    /// Accepted by the visible tests but failing the hidden ones.
    pub false_accepts: usize,
    pub rejected: usize,
    pub refused: usize,
    pub errors: usize,
    /// Successes over trials that ran, excluding provider and infrastructure failures.
    pub success_rate: f64,
    /// Trials the rate is computed over.
    pub scored: usize,
    pub model_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// True when any trial's tokens were estimated, not reported by the provider.
    pub tokens_estimated: bool,
    pub provider_retries: u32,
    pub recoveries: usize,
    pub test_runs: usize,
    pub rollbacks: usize,
    pub unusable_responses: usize,
    pub boundary_violations: usize,
    pub test_edit_attempts: usize,
    pub stayed_in_intended_files: usize,
    pub mean_context_bytes: f64,
    pub mean_wall_ms: f64,
    pub median_wall_ms: f64,
    /// `None` when any trial's cost is unknown.
    pub cost_usd: Option<f64>,
}

/// An arm against the baseline. Descriptive only: no significance test is computed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub arm: String,
    pub baseline: String,
    /// The components this arm changes relative to the baseline (an ablation varies one).
    pub differs_in: Vec<String>,
    pub success_rate_delta_pp: f64,
    pub scored: (usize, usize),
    pub mean_model_calls_delta: f64,
    pub mean_tokens_delta: f64,
    pub median_wall_ms_delta: f64,
    pub mean_context_bytes_delta: f64,
}

/// The evaluation result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingEvalReport {
    pub schema: String,
    pub experiment: String,
    pub model: String,
    pub provider: String,
    /// `complete`, `preflight-failed`, or `partial` while running.
    pub status: String,
    pub max_attempts: u32,
    pub trials_per_arm: u32,
    pub arm_configs: Vec<CodingArm>,
    pub baseline: String,
    pub limits: Limits,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preflight: Option<agent_model::gateway::PreflightReport>,
    pub provenance: crate::arms::Provenance,
    /// Every fixture held the same bytes after the run as before it.
    pub fixtures_unchanged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<serde_json::Value>,
    pub started_at: String,
    pub finished_at: String,
    pub cells: Vec<CodingCell>,
    /// Per arm, over every task.
    pub arms: Vec<CodingCell>,
    pub comparisons: Vec<Comparison>,
    pub trials: Vec<CodingTrial>,
}

/// The report format version.
pub const CODING_REPORT_SCHEMA: &str = "secret-agent.coding-eval/2";

impl CodingEvalReport {
    pub fn arm(&self, arm: CodingContext) -> Option<&CodingCell> {
        self.arm_named(&arm.to_string())
    }

    pub fn arm_named(&self, id: &str) -> Option<&CodingCell> {
        self.arms.iter().find(|a| a.arm == id)
    }
}

fn blank_trial(task: &CodingTask, arm: &CodingArm, trial: u32, order: usize, outcome: &str) -> CodingTrial {
    CodingTrial {
        task: task.id.clone(),
        arm: arm.id.clone(),
        strategies: arm.strategies,
        trial,
        order,
        outcome: outcome.into(),
        category: String::new(),
        reason: None,
        error: None,
        patch_applied: false,
        tests_passed: false,
        hidden_passed: None,
        success: false,
        attempts: 0,
        model_calls: 0,
        models_reported: Vec::new(),
        input_tokens: 0,
        output_tokens: 0,
        tokens_estimated: false,
        provider_retries: 0,
        recovery_decisions: Vec::new(),
        stopped_by: None,
        context_bytes: 0,
        test_runs: 0,
        rollbacks: 0,
        unusable_responses: 0,
        boundary_violations: 0,
        test_edit_attempts: 0,
        stayed_in_intended_files: None,
        cost_usd: None,
        started_at: now_rfc3339(),
        wall_ms: 0,
    }
}

/// Runs every task × arm × trial, then writes `coding-eval.json` and
/// `coding-eval.md` to `out`. Each trial runs on a fresh copy of the task's
/// repository; `coding-eval.partial.json` is rewritten after every trial.
pub fn coding_eval(options: &CodingEvalOptions) -> Result<CodingEvalReport, String> {
    if options.arms.is_empty() {
        return Err("no arms to run".into());
    }
    let mut seen = BTreeSet::new();
    for arm in &options.arms {
        if !seen.insert(arm.id.as_str()) {
            return Err(format!("arm id {:?} is used twice", arm.id));
        }
    }
    let baseline = options.baseline.clone().unwrap_or_else(|| options.arms[0].id.clone());
    if !options.arms.iter().any(|a| a.id == baseline) {
        return Err(format!("the baseline arm {baseline:?} is not among the arms"));
    }
    let model_name = options.model.model.clone().unwrap_or_default();
    if options.limits.budget_usd.is_some() && crate::arms::price(&model_name).is_none() {
        return Err(format!("a spending limit needs known pricing, and {model_name:?} has none"));
    }
    let tasks: Vec<(PathBuf, CodingTask)> =
        options.tasks.iter().map(|d| read_task(d).map(|t| (d.clone(), t))).collect::<Result<_, _>>()?;
    let started_at = now_rfc3339();
    let began = Instant::now();
    fs::create_dir_all(&options.out).map_err(|e| format!("{}: {e}", options.out.display()))?;
    let out = options.out.canonicalize().map_err(|e| e.to_string())?;
    let digests: BTreeMap<String, String> =
        tasks.iter().map(|(dir, t)| (t.id.clone(), crate::arms::digest_files(&[dir.as_path()]))).collect();
    let mut report = CodingEvalReport {
        schema: CODING_REPORT_SCHEMA.into(),
        experiment: options.experiment.clone(),
        model: model_name.clone(),
        provider: crate::arms::provider_name(&options.model),
        status: "partial".into(),
        max_attempts: options.max_attempts,
        trials_per_arm: options.trials,
        arm_configs: options.arms.clone(),
        baseline: baseline.clone(),
        limits: options.limits.clone(),
        preflight: None,
        provenance: crate::arms::provenance_of(&options.root, &options.model, digests.clone()),
        fixtures_unchanged: true,
        spec: options.spec.clone(),
        started_at,
        finished_at: String::new(),
        cells: Vec::new(),
        arms: Vec::new(),
        comparisons: Vec::new(),
        trials: Vec::new(),
    };

    // Live inference at the gateway runs only on a model shown to be free.
    let at_gateway = options
        .model
        .base_url
        .as_deref()
        .is_some_and(|u| u.trim_end_matches('/') == agent_model::gateway::GATEWAY_BASE_URL);
    let policy = options.free_gateway.clone().or_else(|| at_gateway.then(GatewayPolicy::default));
    if let Some(policy) = policy {
        let checked = agent_model::gateway::preflight(&options.model, &policy);
        let text = agent_model::gateway::preflight_text(&checked);
        if !options.quiet {
            eprint!("{text}");
        }
        let _ =
            fs::write(out.join("preflight.json"), serde_json::to_string_pretty(&checked).expect("serializes") + "\n");
        let _ = fs::write(out.join("preflight.txt"), &text);
        let proceed = checked.proceed;
        report.preflight = Some(checked);
        if !proceed {
            report.status = "preflight-failed".into();
            return finish_eval(report, options, &tasks, &out);
        }
    }

    let write_partial = |report: &CodingEvalReport| {
        let _ = fs::write(
            out.join("coding-eval.partial.json"),
            serde_json::to_string_pretty(report).expect("serializes") + "\n",
        );
    };
    let hidden_env = agent_model::gateway::credential_env(&options.model);
    for trial in 1..=options.trials {
        for (index, (task_dir, task)) in tasks.iter().enumerate() {
            let mut order: Vec<&CodingArm> = options.arms.iter().collect();
            if options.balanced_order {
                order.rotate_left((trial as usize - 1 + index) % options.arms.len());
            }
            for (position, arm) in order.into_iter().enumerate() {
                let calls: usize = report.trials.iter().map(|t| t.model_calls).sum();
                let spent: f64 = report.trials.iter().filter_map(|t| t.cost_usd).sum();
                let stop = if options.limits.max_model_calls.is_some_and(|m| calls + options.max_attempts as usize > m)
                {
                    Some(format!(
                        "this trial could exceed the cap of {} model calls",
                        options.limits.max_model_calls.unwrap_or_default()
                    ))
                } else if options.limits.max_minutes.is_some_and(|m| began.elapsed().as_secs_f64() >= m * 60.0) {
                    Some(format!("time limit of {} minutes reached", options.limits.max_minutes.unwrap_or_default()))
                } else if options.limits.budget_usd.is_some_and(|b| spent > 0.0 && spent >= b) {
                    Some(format!("budget of ${:.2} reached", options.limits.budget_usd.unwrap_or_default()))
                } else {
                    None
                };
                if let Some(reason) = stop {
                    let mut t = blank_trial(task, arm, trial, position + 1, "not-run");
                    t.reason = Some(reason);
                    let (category, reason) = classify(&t, false);
                    t.category = category.into();
                    t.reason = reason;
                    report.trials.push(t);
                    continue;
                }
                let mut model = options.model.clone();
                if model.provider == Provider::Scripted {
                    model.responses = Some(task_dir.join("scripted"));
                }
                let dir = out.join(format!("{}/{}-{trial:02}", task.id, arm.id));
                let _ = fs::remove_dir_all(&dir);
                let repo = dir.join("repo");
                if !options.quiet {
                    eprintln!("== {} {} trial {trial}/{} (position {})", task.id, arm.id, options.trials, position + 1);
                }
                let mut summary = blank_trial(task, arm, trial, position + 1, "error");
                let started = Instant::now();
                let result = copy_project(&task_dir.join("repo"), &repo)
                    .map_err(|e| format!("copying {}: {e}", task.id))
                    .and_then(|_| {
                        run_coding(&CodingOptions {
                            repo: repo.clone(),
                            task: task.clone(),
                            model: model.clone(),
                            strategies: arm.strategies,
                            max_attempts: options.max_attempts,
                            test_timeout: options.test_timeout,
                            host: options.host.clone(),
                            log: Some(dir.join("run.jsonl")),
                            quiet: options.quiet,
                        })
                    });
                match result {
                    Ok(run) => {
                        let _ = fs::write(
                            dir.join("report.json"),
                            serde_json::to_string_pretty(&run).expect("serializes") + "\n",
                        );
                        if run.outcome == "accepted" {
                            save_diff(&task_dir.join("repo"), &repo, &run.accepted_files, &dir.join("accepted.diff"));
                        }
                        fill(&mut summary, task, &run);
                    }
                    Err(error) => {
                        summary.error = Some(agent_model::gateway::redact(
                            &error,
                            &agent_model::gateway::secrets_of(&options.model),
                        ))
                    }
                }
                // Whatever the outcome, the final state against the fixture.
                if repo.is_dir() {
                    let changed = crate::arms::changed_files(&task_dir.join("repo"), &repo);
                    save_diff(&task_dir.join("repo"), &repo, &changed, &dir.join("final.diff"));
                }
                // Score with the hidden acceptance tests, after the agent is done.
                let has_hidden = task.hidden.is_some();
                if let Some(hidden) = &task.hidden {
                    if summary.outcome == "accepted" && task_dir.join("hidden").is_dir() {
                        let target = repo.join("hidden");
                        let run = copy_project(&task_dir.join("hidden"), &target)
                            .map(|_| run_command(&repo, hidden, options.test_timeout, &hidden_env));
                        let _ = fs::remove_dir_all(&target);
                        match run {
                            Ok(run) => {
                                let _ = fs::write(dir.join("hidden.txt"), &run.output);
                                if run.exit_code.is_some() || run.timed_out {
                                    summary.hidden_passed = Some(run.passed());
                                }
                            }
                            Err(e) => summary.error = Some(format!("hidden tests: {e}")),
                        }
                    }
                }
                let (category, reason) = classify(&summary, has_hidden);
                summary.category = category.into();
                summary.reason = summary.reason.take().or(reason);
                summary.success = category == "verified-success";
                summary.wall_ms = started.elapsed().as_millis() as u64;
                let model_id = summary.models_reported.first().cloned().unwrap_or_else(|| model_name.clone());
                summary.cost_usd = crate::arms::price(&model_id)
                    .or_else(|| crate::arms::price(&model_name))
                    .filter(|_| !summary.tokens_estimated)
                    .map(|(i, o)| summary.input_tokens as f64 * i / 1e6 + summary.output_tokens as f64 * o / 1e6);
                let _ = fs::write(
                    dir.join("trial.json"),
                    serde_json::to_string_pretty(&summary).expect("serializes") + "\n",
                );
                report.trials.push(summary);
                write_partial(&report);
            }
        }
    }
    report.fixtures_unchanged =
        tasks.iter().all(|(dir, t)| digests.get(&t.id) == Some(&crate::arms::digest_files(&[dir.as_path()])));
    report.status = "complete".into();
    finish_eval(report, options, &tasks, &out)
}

/// Copies what the loop reported into the trial record.
fn fill(summary: &mut CodingTrial, task: &CodingTask, run: &CodingReport) {
    let intended = &task.intended_files;
    summary.outcome = run.outcome.clone();
    summary.error = run.refusal.clone();
    summary.patch_applied = run.attempts.iter().any(|a| !a.files_changed.is_empty() && a.unusable.is_none());
    summary.tests_passed = run.attempts.last().is_some_and(|a| a.tests_passed);
    summary.attempts = run.attempts.len();
    summary.model_calls = run.model_calls;
    summary.models_reported = run.attempts.iter().map(|a| a.model.clone()).fold(Vec::new(), |mut acc, m| {
        if !acc.contains(&m) {
            acc.push(m);
        }
        acc
    });
    summary.input_tokens = run.input_tokens;
    summary.output_tokens = run.output_tokens;
    summary.tokens_estimated = run.tokens_estimated;
    summary.provider_retries = run.attempts.iter().map(|a| a.provider_retries).sum();
    summary.recovery_decisions = run
        .attempts
        .iter()
        .filter_map(|a| a.recovery.as_ref())
        .map(|d| {
            serde_json::to_value(d)
                .ok()
                .and_then(|v| v.get("action").and_then(|a| a.as_str()).map(str::to_string))
                .unwrap_or_default()
        })
        .collect();
    summary.stopped_by = run.stopped_by.clone();
    summary.context_bytes = run.context_selection.as_ref().map_or(0, |s| s.total_bytes);
    summary.test_runs = run.test_runs;
    summary.rollbacks = run.rollbacks;
    summary.unusable_responses = run.attempts.iter().filter(|a| a.unusable.is_some()).count();
    summary.boundary_violations = run.attempts.iter().filter(|a| a.outside_boundary).count();
    summary.test_edit_attempts = run.attempts.iter().filter(|a| a.edited_tests).count();
    summary.stayed_in_intended_files = (run.outcome == "accepted" && !intended.is_empty())
        .then(|| run.accepted_files.iter().all(|f| intended.contains(f)));
    if run.outcome == "rejected" && run.attempts.last().is_some_and(|a| a.test_timed_out) {
        summary.outcome = "rejected".into();
        summary.stopped_by = summary.stopped_by.take();
        summary.error = Some("the test command timed out on the last applied change".into());
        summary.category = "timed-out".into();
    }
}

/// Aggregates, compares and writes the report.
fn finish_eval(
    mut report: CodingEvalReport,
    options: &CodingEvalOptions,
    tasks: &[(PathBuf, CodingTask)],
    out: &Path,
) -> Result<CodingEvalReport, String> {
    // A test timeout on the last applied change is its own category.
    for t in &mut report.trials {
        if t.outcome == "rejected"
            && t.error.as_deref() == Some("the test command timed out on the last applied change")
        {
            t.category = "timed-out".into();
            t.reason = t.error.clone();
        }
    }
    let mut cells = Vec::new();
    for (_, task) in tasks {
        for arm in &options.arms {
            let mine: Vec<&CodingTrial> =
                report.trials.iter().filter(|t| t.task == task.id && t.arm == arm.id).collect();
            cells.push(aggregate(&task.id, &arm.id, &mine));
        }
    }
    report.cells = cells;
    report.arms = options
        .arms
        .iter()
        .map(|arm| {
            let mine: Vec<&CodingTrial> = report.trials.iter().filter(|t| t.arm == arm.id).collect();
            aggregate("all", &arm.id, &mine)
        })
        .collect();
    let base_config = options.arms.iter().find(|a| a.id == report.baseline).expect("checked");
    let base = report.arm_named(&report.baseline).cloned();
    report.comparisons = match base {
        Some(base) => options
            .arms
            .iter()
            .filter(|a| a.id != report.baseline)
            .filter_map(|arm| {
                let cell = report.arm_named(&arm.id)?;
                let per = |total: f64, n: usize| if n == 0 { 0.0 } else { total / n as f64 };
                Some(Comparison {
                    arm: arm.id.clone(),
                    baseline: base.arm.clone(),
                    differs_in: arm.differs_from(base_config).into_iter().map(str::to_string).collect(),
                    success_rate_delta_pp: (cell.success_rate - base.success_rate) * 100.0,
                    scored: (cell.scored, base.scored),
                    mean_model_calls_delta: per(cell.model_calls as f64, cell.ran)
                        - per(base.model_calls as f64, base.ran),
                    mean_tokens_delta: per((cell.input_tokens + cell.output_tokens) as f64, cell.ran)
                        - per((base.input_tokens + base.output_tokens) as f64, base.ran),
                    median_wall_ms_delta: cell.median_wall_ms - base.median_wall_ms,
                    mean_context_bytes_delta: cell.mean_context_bytes - base.mean_context_bytes,
                })
            })
            .collect(),
        None => Vec::new(),
    };
    report.finished_at = now_rfc3339();
    fs::write(out.join("coding-eval.json"), serde_json::to_string_pretty(&report).expect("serializes") + "\n")
        .map_err(|e| e.to_string())?;
    fs::write(out.join("coding-eval.md"), coding_markdown(&report)).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(out.join("coding-eval.partial.json"));
    Ok(report)
}

fn save_diff(template: &Path, repo: &Path, files: &[String], out: &Path) {
    let mut text = String::new();
    for path in files {
        let Some(path) = normalize(path) else { continue };
        let (old, new) = (template.join(&path), repo.join(&path));
        let mut command = Command::new("diff");
        command.args(["-u", "--label", &format!("a/{path}"), "--label", &format!("b/{path}")]);
        command.arg(if old.exists() { old } else { PathBuf::from("/dev/null") });
        command.arg(if new.exists() { new } else { PathBuf::from("/dev/null") });
        if let Ok(output) = command.output() {
            text.push_str(&String::from_utf8_lossy(&output.stdout));
        }
    }
    let _ = fs::write(out, text);
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

fn aggregate(task: &str, arm: &str, trials: &[&CodingTrial]) -> CodingCell {
    let ran: Vec<&&CodingTrial> = trials.iter().filter(|t| t.category != "not-run").collect();
    let count = |f: &dyn Fn(&CodingTrial) -> bool| ran.iter().filter(|t| f(t)).count();
    let mut categories = BTreeMap::new();
    for t in trials {
        *categories.entry(t.category.clone()).or_insert(0) += 1;
    }
    // Provider and infrastructure failures say nothing about the strategy.
    let scored = count(&|t| !matches!(t.category.as_str(), "provider-error" | "infrastructure-error"));
    let successes = count(&|t| t.success);
    let n = ran.len();
    let costs: Vec<Option<f64>> = ran.iter().map(|t| t.cost_usd).collect();
    CodingCell {
        task: task.to_string(),
        arm: arm.to_string(),
        trials: trials.len(),
        ran: n,
        categories,
        successes,
        accepted: count(&|t| t.outcome == "accepted"),
        accepted_first_attempt: count(&|t| t.outcome == "accepted" && t.attempts == 1),
        false_accepts: count(&|t| t.category == "accepted-unverified"),
        rejected: count(&|t| matches!(t.category.as_str(), "rejected" | "timed-out" | "patch-failed")),
        refused: count(&|t| t.category == "limitation"),
        errors: count(&|t| matches!(t.category.as_str(), "provider-error" | "infrastructure-error")),
        success_rate: if scored == 0 { 0.0 } else { successes as f64 / scored as f64 },
        scored,
        model_calls: ran.iter().map(|t| t.model_calls).sum(),
        input_tokens: ran.iter().map(|t| t.input_tokens).sum(),
        output_tokens: ran.iter().map(|t| t.output_tokens).sum(),
        tokens_estimated: ran.iter().any(|t| t.tokens_estimated),
        provider_retries: ran.iter().map(|t| t.provider_retries).sum(),
        recoveries: ran.iter().map(|t| t.recovery_decisions.len()).sum(),
        test_runs: ran.iter().map(|t| t.test_runs).sum(),
        rollbacks: ran.iter().map(|t| t.rollbacks).sum(),
        unusable_responses: ran.iter().map(|t| t.unusable_responses).sum(),
        boundary_violations: ran.iter().map(|t| t.boundary_violations).sum(),
        test_edit_attempts: ran.iter().map(|t| t.test_edit_attempts).sum(),
        stayed_in_intended_files: count(&|t| t.stayed_in_intended_files == Some(true)),
        mean_context_bytes: if n == 0 {
            0.0
        } else {
            ran.iter().map(|t| t.context_bytes as f64).sum::<f64>() / n as f64
        },
        mean_wall_ms: if n == 0 { 0.0 } else { ran.iter().map(|t| t.wall_ms as f64).sum::<f64>() / n as f64 },
        median_wall_ms: median(ran.iter().map(|t| t.wall_ms as f64).collect()),
        cost_usd: if !costs.is_empty() && costs.iter().all(Option::is_some) {
            Some(costs.iter().flatten().sum())
        } else {
            None
        },
    }
}

/// A human-readable summary.
pub fn coding_markdown(report: &CodingEvalReport) -> String {
    let mut text = format!(
        "# Coding-agent evaluation: `{}`\n\n- model: `{}` via `{}`\n- status: **{}**\n- attempts per trial: up to {}; trials per arm and task: {}\n- baseline arm: `{}`\n- commit: {}\n- fixtures unchanged by the run: {}\n- run: {} → {}\n\n**Verified success** = an applied change passed the project's own tests **and** the hidden acceptance tests the agent never saw (when the task has them). Provider and infrastructure failures are listed but not scored. Token counts marked *estimated* were not reported by the provider.\n\n",
        report.experiment,
        report.model,
        report.provider,
        report.status,
        report.max_attempts,
        report.trials_per_arm,
        report.baseline,
        report.provenance.git_commit.as_deref().unwrap_or("unknown"),
        if report.fixtures_unchanged { "yes" } else { "**NO**" },
        report.started_at,
        report.finished_at
    );
    if let Some(p) = report.preflight.as_ref().filter(|p| !p.proceed) {
        text.push_str(&format!(
            "> **Preflight failed: no trial ran.**\n\n```\n{}```\n\n",
            agent_model::gateway::preflight_text(p)
        ));
    }
    text.push_str("## Arms\n\n| arm | context | recovery | context budget |\n|---|---|---|---|\n");
    for arm in &report.arm_configs {
        text.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            arm.id, arm.strategies.context, arm.strategies.recovery, arm.strategies.context_budget
        ));
    }
    text.push_str("\n## Outcomes\n\n| task | arm | trials | verified | rate (scored) | ");
    text.push_str(&CATEGORIES[1..].join(" | "));
    text.push_str(" | not-run | model calls | tokens in / out | provider retries | recoveries | mean context bytes | median wall | cost |\n|---|---|---|---|---|");
    text.push_str(&"---|".repeat(CATEGORIES.len() - 1));
    text.push_str("---|---|---|---|---|---|---|---|\n");
    for c in report.cells.iter().chain(report.arms.iter()) {
        let cat = |k: &str| c.categories.get(k).copied().unwrap_or(0).to_string();
        text.push_str(&format!(
            "| {} | `{}` | {} | {} | {:.0}% of {} | {} | {} | {} | {} / {}{} | {} | {} | {:.0} | {:.1} s | {} |\n",
            if c.task == "all" { "**all**".to_string() } else { format!("`{}`", c.task) },
            c.arm,
            c.trials,
            c.successes,
            c.success_rate * 100.0,
            c.scored,
            CATEGORIES[1..].iter().map(|k| cat(k)).collect::<Vec<_>>().join(" | "),
            cat("not-run"),
            c.model_calls,
            c.input_tokens,
            c.output_tokens,
            if c.tokens_estimated { " (estimated)" } else { "" },
            c.provider_retries,
            c.recoveries,
            c.mean_context_bytes,
            c.median_wall_ms / 1000.0,
            c.cost_usd.map_or("unknown".into(), |v| format!("${v:.4}"))
        ));
    }
    if !report.comparisons.is_empty() {
        text.push_str(&format!(
            "\n## Against the baseline (`{}`)\n\nDescriptive only: no significance test is computed, and small trial counts cannot support one.\n\n| arm | differs in | Δ verified rate | scored (arm / baseline) | Δ model calls per trial | Δ tokens per trial | Δ median wall | Δ context bytes |\n|---|---|---|---|---|---|---|---|\n",
            report.baseline
        ));
        for c in &report.comparisons {
            text.push_str(&format!(
                "| `{}` | {} | {:+.0} pp | {} / {} | {:+.2} | {:+.0} | {:+.1} s | {:+.0} |\n",
                c.arm,
                if c.differs_in.is_empty() { "nothing".into() } else { c.differs_in.join(", ") },
                c.success_rate_delta_pp,
                c.scored.0,
                c.scored.1,
                c.mean_model_calls_delta,
                c.mean_tokens_delta,
                c.median_wall_ms_delta / 1000.0,
                c.mean_context_bytes_delta
            ));
        }
    }
    text.push_str("\n## Trials\n\n| task | arm | trial | order | category | reason | patch applied | tests | hidden | attempts | recovery | tokens in / out | wall |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for t in &report.trials {
        text.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} / {} | {:.1} s |\n",
            t.task,
            t.arm,
            t.trial,
            t.order,
            t.category,
            t.reason.as_deref().unwrap_or("").replace('|', "/").chars().take(120).collect::<String>(),
            if t.patch_applied { "yes" } else { "no" },
            if t.tests_passed { "pass" } else { "–" },
            match t.hidden_passed {
                Some(true) => "pass",
                Some(false) => "FAIL",
                None => "–",
            },
            t.attempts,
            if t.recovery_decisions.is_empty() { "–".into() } else { t.recovery_decisions.join(", ") },
            t.input_tokens,
            t.output_tokens,
            t.wall_ms as f64 / 1000.0
        ));
    }
    text
}
