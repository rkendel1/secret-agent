//! The coding loop for ordinary repositories, and its evaluation.
//!
//! No AppPort knowledge is involved: the agent observes the repository, runs the
//! project's own test command, plans with [`agent_core::generic`], asks the model
//! for the change, and accepts it only when the test command exits 0. Test files
//! are read-only. The evaluation then runs acceptance tests the agent never saw,
//! so an accepted change that only satisfies the visible tests is counted as a
//! failure, not a success.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use agent_core::generic::{coding_task, is_test_file, plan_coding, tail, Baseline, CodingContext, CodingTask};
use agent_core::observe::{normalize, observe};
use agent_model::{connect, Model, ModelConfig, Provider, Request, Usage};
use serde::Serialize;
use serde_json::json;

use crate::edit;
use crate::eval::copy_project;
use crate::report::{now_rfc3339, Log};

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

/// Runs `command` in `dir`, capturing stdout and stderr together.
pub fn run_command(dir: &Path, command: &[String], timeout: Duration) -> CommandRun {
    let began = Instant::now();
    let failed = |output: String| CommandRun {
        command: command.to_vec(),
        exit_code: None,
        timed_out: false,
        duration_ms: began.elapsed().as_millis() as u64,
        output,
    };
    let Some((program, args)) = command.split_first() else { return failed("empty command".into()) };
    let mut child = match Command::new(program)
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

/// How to run one coding task.
#[derive(Debug, Clone)]
pub struct CodingOptions {
    /// The repository to change (a copy; it is modified).
    pub repo: PathBuf,
    pub task: CodingTask,
    pub model: ModelConfig,
    pub context: CodingContext,
    pub max_attempts: u32,
    pub test_timeout: Duration,
    pub log: Option<PathBuf>,
    pub quiet: bool,
}

/// One model attempt.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAttempt {
    pub attempt: u32,
    pub model: String,
    pub usage: Usage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    pub prompt_bytes: usize,
    pub files_changed: Vec<String>,
    /// Why nothing could be applied (unparseable response, or a boundary violation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unusable: Option<String>,
    pub outside_boundary: bool,
    pub edited_tests: bool,
    /// The test run on the proposal, when it was applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_exit: Option<i32>,
    pub tests_passed: bool,
    pub accepted: bool,
    pub rolled_back: bool,
    pub duration_ms: u64,
}

/// The report of one coding run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingReport {
    pub task: String,
    pub context: String,
    pub model: String,
    /// `accepted`, `rejected`, or `refused` when no plan could be made.
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    pub baseline_passed: bool,
    pub failing_tests: Vec<String>,
    pub editable: Vec<String>,
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
pub fn run_coding(options: &CodingOptions) -> Result<CodingReport, String> {
    let mut log = Log::new(options.log.as_deref(), options.quiet).map_err(|e| e.to_string())?;
    let started_at = now_rfc3339();
    let task = &options.task;
    let mut model: Box<dyn Model> = connect(&options.model).map_err(|e| e.to_string())?;
    let mut report = CodingReport {
        task: task.id.clone(),
        context: options.context.to_string(),
        model: model.describe(),
        outcome: "rejected".into(),
        refusal: None,
        baseline_passed: false,
        failing_tests: Vec::new(),
        editable: Vec::new(),
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

    let project = observe(&options.repo).map_err(|e| format!("{}: {e}", options.repo.display()))?;
    let root = project.root.clone();
    log.event(
        "observe",
        json!({ "sources": project.sources.len() }),
        format!("{} source file(s)", project.sources.len()),
    );

    let first = run_command(&root, &task.test, options.test_timeout);
    report.test_runs += 1;
    let baseline = Baseline { passed: first.passed(), output: first.output.clone() };
    report.baseline_passed = baseline.passed;
    log.event(
        "baseline",
        json!({ "run": &first }),
        format!("`{}` → exit {:?} in {} ms", task.test.join(" "), first.exit_code, first.duration_ms),
    );

    let plan = match plan_coding(&project, &task.intent, &task.test, &baseline) {
        Ok(plan) => plan,
        Err(refusal) => {
            log.event("plan", json!({ "refusal": &refusal }), format!("refused: {refusal}"));
            report.outcome = "refused".into();
            report.refusal = Some(refusal.to_string());
            report.wall_ms = log.elapsed_ms() as u64;
            return Ok(report);
        }
    };
    for decision in &plan.decisions {
        log.event("plan", json!({ "decision": decision }), format!("{}: {}", decision.step, decision.fact));
    }
    report.failing_tests = plan.failing_tests.clone();
    let editable: Vec<String> = plan.editable().iter().map(|s| s.to_string()).collect();
    report.editable = editable.clone();
    let mut files = BTreeMap::new();
    for file in &plan.context {
        files.insert(file.path.clone(), project.read(&file.path).map_err(|e| format!("{}: {e}", file.path))?);
    }

    let mut feedback: Vec<String> = Vec::new();
    for attempt in 1..=options.max_attempts.max(1) {
        let began = Instant::now();
        let task_text = coding_task(options.context, &plan, &files, &baseline, &feedback);
        log.event(
            "model",
            json!({ "attempt": attempt, "task": &task_text }),
            format!("attempt {attempt}: asking for the change"),
        );
        let request = Request {
            system: task_text.system.clone(),
            prompt: task_text.prompt.clone(),
            max_tokens: options.model.max_tokens,
        };
        let response = model.complete(&request).map_err(|e| e.to_string())?;
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
            stop_reason: response.stop_reason.clone(),
            prompt_bytes: task_text.system.len() + task_text.prompt.len(),
            files_changed: Vec::new(),
            unusable: None,
            outside_boundary: false,
            edited_tests: false,
            test_exit: None,
            tests_passed: false,
            accepted: false,
            rolled_back: false,
            duration_ms: 0,
        };

        let parsed = edit::parse(&response.text)
            .or_else(|problem| edit::single_fenced(&response.text, &editable).map(|e| vec![e]).ok_or(problem));
        let edits = parsed.and_then(|e| edit::targets(&e).map(|t| (e, t)));
        let (edits, targets) = match edits {
            Ok(parsed) => parsed,
            Err(problem) => {
                log.event(
                    "apply",
                    json!({ "attempt": attempt, "unusable": &problem }),
                    format!("attempt {attempt}: unusable: {problem}"),
                );
                feedback = vec![format!("Your reply could not be applied: {problem}")];
                record.unusable = Some(problem);
                record.duration_ms = began.elapsed().as_millis() as u64;
                report.attempts.push(record);
                continue;
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
            record.unusable = Some(problem);
            record.duration_ms = began.elapsed().as_millis() as u64;
            report.attempts.push(record);
            continue;
        }
        let applied = edit::apply(&root, &edits)?;
        record.files_changed = applied.changed.clone();
        if applied.changed.is_empty() {
            feedback = vec!["The previous reply changed nothing.".into()];
            record.unusable = Some("identical to the current files".into());
            record.duration_ms = began.elapsed().as_millis() as u64;
            report.attempts.push(record);
            continue;
        }
        let run = run_command(&root, &task.test, options.test_timeout);
        report.test_runs += 1;
        record.test_exit = run.exit_code;
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
        record.duration_ms = began.elapsed().as_millis() as u64;
        if run.passed() {
            record.accepted = true;
            report.accepted_files = record.files_changed.clone();
            report.attempts.push(record);
            report.outcome = "accepted".into();
            break;
        }
        edit::rollback(&root, applied);
        record.rolled_back = true;
        feedback = vec![format!(
            "`{}` failed (exit {:?}{}). Its output ends with:\n```\n{}\n```",
            task.test.join(" "),
            run.exit_code,
            if run.timed_out { ", timed out" } else { "" },
            tail(&run.output, FEEDBACK_TAIL).trim_end()
        )];
        report.attempts.push(record);
    }

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

/// How to run the coding evaluation.
#[derive(Debug, Clone)]
pub struct CodingEvalOptions {
    /// Task directories: each holds `task.json`, `repo/`, optional `hidden/` and `scripted/`.
    pub tasks: Vec<PathBuf>,
    pub model: ModelConfig,
    pub arms: Vec<CodingContext>,
    pub trials: u32,
    pub max_attempts: u32,
    pub test_timeout: Duration,
    pub out: PathBuf,
    pub quiet: bool,
}

/// One trial.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingTrial {
    pub task: String,
    pub arm: String,
    pub trial: u32,
    /// `accepted`, `rejected`, `refused` or `error`.
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The hidden acceptance tests passed on the final state of the repository.
    pub hidden_passed: Option<bool>,
    /// Accepted and the hidden acceptance tests pass.
    pub success: bool,
    pub attempts: usize,
    pub model_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tokens_estimated: bool,
    pub test_runs: usize,
    pub rollbacks: usize,
    pub unusable_responses: usize,
    pub boundary_violations: usize,
    pub test_edit_attempts: usize,
    /// The accepted change touched only the files a person would.
    pub stayed_in_intended_files: Option<bool>,
    pub wall_ms: u64,
}

/// One task × arm, aggregated.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingCell {
    pub task: String,
    pub arm: String,
    pub trials: usize,
    pub successes: usize,
    pub accepted: usize,
    pub accepted_first_attempt: usize,
    /// Accepted by the visible tests but failing the hidden ones.
    pub false_accepts: usize,
    pub rejected: usize,
    pub refused: usize,
    pub errors: usize,
    pub success_rate: f64,
    pub model_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub test_runs: usize,
    pub rollbacks: usize,
    pub unusable_responses: usize,
    pub boundary_violations: usize,
    pub test_edit_attempts: usize,
    pub stayed_in_intended_files: usize,
    pub mean_wall_ms: f64,
}

/// The evaluation result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingEvalReport {
    pub model: String,
    pub max_attempts: u32,
    pub started_at: String,
    pub finished_at: String,
    pub cells: Vec<CodingCell>,
    /// Per arm, over every task.
    pub arms: Vec<CodingCell>,
    pub trials: Vec<CodingTrial>,
}

impl CodingEvalReport {
    pub fn arm(&self, arm: CodingContext) -> Option<&CodingCell> {
        self.arms.iter().find(|a| a.arm == arm.to_string())
    }
}

/// Runs every task × arm × trial, then writes `coding-eval.json` and `coding-eval.md` to `out`.
pub fn coding_eval(options: &CodingEvalOptions) -> Result<CodingEvalReport, String> {
    let started_at = now_rfc3339();
    fs::create_dir_all(&options.out).map_err(|e| format!("{}: {e}", options.out.display()))?;
    let out = options.out.canonicalize().map_err(|e| e.to_string())?;
    let mut trials = Vec::new();
    let mut model_name = String::new();
    for task_dir in &options.tasks {
        let task = read_task(task_dir)?;
        let mut model = options.model.clone();
        if model.provider == Provider::Scripted {
            model.responses = Some(task_dir.join("scripted"));
        }
        for arm in &options.arms {
            for trial in 1..=options.trials {
                let dir = out.join(format!("{}/{arm}-{trial:02}", task.id));
                let _ = fs::remove_dir_all(&dir);
                let repo = dir.join("repo");
                copy_project(&task_dir.join("repo"), &repo).map_err(|e| format!("copying {}: {e}", task.id))?;
                if !options.quiet {
                    eprintln!("== {} {arm} trial {trial}/{}", task.id, options.trials);
                }
                let result = run_coding(&CodingOptions {
                    repo: repo.clone(),
                    task: task.clone(),
                    model: model.clone(),
                    context: *arm,
                    max_attempts: options.max_attempts,
                    test_timeout: options.test_timeout,
                    log: Some(dir.join("run.jsonl")),
                    quiet: options.quiet,
                });
                let mut summary = match result {
                    Ok(report) => {
                        model_name = report.model.clone();
                        let _ = fs::write(
                            dir.join("report.json"),
                            serde_json::to_string_pretty(&report).expect("serializes") + "\n",
                        );
                        if report.outcome == "accepted" {
                            save_diff(
                                &task_dir.join("repo"),
                                &repo,
                                &report.accepted_files,
                                &dir.join("accepted.diff"),
                            );
                        }
                        summarize(&task, *arm, trial, &report)
                    }
                    Err(error) => CodingTrial {
                        task: task.id.clone(),
                        arm: arm.to_string(),
                        trial,
                        outcome: "error".into(),
                        error: Some(error),
                        hidden_passed: None,
                        success: false,
                        attempts: 0,
                        model_calls: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        tokens_estimated: false,
                        test_runs: 0,
                        rollbacks: 0,
                        unusable_responses: 0,
                        boundary_violations: 0,
                        test_edit_attempts: 0,
                        stayed_in_intended_files: None,
                        wall_ms: 0,
                    },
                };
                // Score with the hidden acceptance tests, after the agent is done.
                if let (Some(hidden), true) = (&task.hidden, task_dir.join("hidden").is_dir()) {
                    if summary.outcome != "error" {
                        let target = repo.join("hidden");
                        copy_project(&task_dir.join("hidden"), &target).map_err(|e| e.to_string())?;
                        let run = run_command(&repo, hidden, options.test_timeout);
                        let _ = fs::write(dir.join("hidden.txt"), &run.output);
                        let _ = fs::remove_dir_all(&target);
                        summary.hidden_passed = Some(run.passed());
                    }
                }
                summary.success = summary.outcome == "accepted" && summary.hidden_passed.unwrap_or(true);
                trials.push(summary);
            }
        }
    }
    let mut cells = Vec::new();
    let task_ids: Vec<String> = trials.iter().map(|t| t.task.clone()).fold(Vec::new(), |mut acc, t| {
        if !acc.contains(&t) {
            acc.push(t);
        }
        acc
    });
    for task in &task_ids {
        for arm in &options.arms {
            let mine: Vec<&CodingTrial> =
                trials.iter().filter(|t| &t.task == task && t.arm == arm.to_string()).collect();
            cells.push(aggregate(task, &arm.to_string(), &mine));
        }
    }
    let arms = options
        .arms
        .iter()
        .map(|arm| {
            let mine: Vec<&CodingTrial> = trials.iter().filter(|t| t.arm == arm.to_string()).collect();
            aggregate("all", &arm.to_string(), &mine)
        })
        .collect();
    let report = CodingEvalReport {
        model: model_name,
        max_attempts: options.max_attempts,
        started_at,
        finished_at: now_rfc3339(),
        cells,
        arms,
        trials,
    };
    fs::write(out.join("coding-eval.json"), serde_json::to_string_pretty(&report).expect("serializes") + "\n")
        .map_err(|e| e.to_string())?;
    fs::write(out.join("coding-eval.md"), coding_markdown(&report)).map_err(|e| e.to_string())?;
    Ok(report)
}

fn save_diff(template: &Path, repo: &Path, files: &[String], out: &Path) {
    let mut text = String::new();
    for path in files {
        let Some(path) = normalize(path) else { continue };
        if let Ok(output) = Command::new("diff")
            .args(["-u", "--label", &format!("a/{path}"), "--label", &format!("b/{path}")])
            .arg(template.join(&path))
            .arg(repo.join(&path))
            .output()
        {
            text.push_str(&String::from_utf8_lossy(&output.stdout));
        }
    }
    let _ = fs::write(out, text);
}

fn summarize(task: &CodingTask, arm: CodingContext, trial: u32, report: &CodingReport) -> CodingTrial {
    let intended = &task.intended_files;
    CodingTrial {
        task: task.id.clone(),
        arm: arm.to_string(),
        trial,
        outcome: report.outcome.clone(),
        error: report.refusal.clone(),
        hidden_passed: None,
        success: false,
        attempts: report.attempts.len(),
        model_calls: report.model_calls,
        input_tokens: report.input_tokens,
        output_tokens: report.output_tokens,
        tokens_estimated: report.tokens_estimated,
        test_runs: report.test_runs,
        rollbacks: report.rollbacks,
        unusable_responses: report.attempts.iter().filter(|a| a.unusable.is_some()).count(),
        boundary_violations: report.attempts.iter().filter(|a| a.outside_boundary).count(),
        test_edit_attempts: report.attempts.iter().filter(|a| a.edited_tests).count(),
        stayed_in_intended_files: (report.outcome == "accepted" && !intended.is_empty())
            .then(|| report.accepted_files.iter().all(|f| intended.contains(f))),
        wall_ms: report.wall_ms,
    }
}

fn aggregate(task: &str, arm: &str, trials: &[&CodingTrial]) -> CodingCell {
    let n = trials.len();
    let count = |f: &dyn Fn(&CodingTrial) -> bool| trials.iter().filter(|t| f(t)).count();
    CodingCell {
        task: task.to_string(),
        arm: arm.to_string(),
        trials: n,
        successes: count(&|t| t.success),
        accepted: count(&|t| t.outcome == "accepted"),
        accepted_first_attempt: count(&|t| t.outcome == "accepted" && t.attempts == 1),
        false_accepts: count(&|t| t.outcome == "accepted" && t.hidden_passed == Some(false)),
        rejected: count(&|t| t.outcome == "rejected"),
        refused: count(&|t| t.outcome == "refused"),
        errors: count(&|t| t.outcome == "error"),
        success_rate: if n == 0 { 0.0 } else { count(&|t| t.success) as f64 / n as f64 },
        model_calls: trials.iter().map(|t| t.model_calls).sum(),
        input_tokens: trials.iter().map(|t| t.input_tokens).sum(),
        output_tokens: trials.iter().map(|t| t.output_tokens).sum(),
        test_runs: trials.iter().map(|t| t.test_runs).sum(),
        rollbacks: trials.iter().map(|t| t.rollbacks).sum(),
        unusable_responses: trials.iter().map(|t| t.unusable_responses).sum(),
        boundary_violations: trials.iter().map(|t| t.boundary_violations).sum(),
        test_edit_attempts: trials.iter().map(|t| t.test_edit_attempts).sum(),
        stayed_in_intended_files: count(&|t| t.stayed_in_intended_files == Some(true)),
        mean_wall_ms: if n == 0 { 0.0 } else { trials.iter().map(|t| t.wall_ms as f64).sum::<f64>() / n as f64 },
    }
}

/// A human-readable summary.
pub fn coding_markdown(report: &CodingEvalReport) -> String {
    let mut text = format!(
        "# Coding-agent evaluation (no AppPort)\n\n- model: `{}`\n- attempts per trial: up to {}\n- run: {} → {}\n\nSuccess = accepted by the project's own tests **and** passing hidden acceptance tests the agent never saw.\n\n",
        report.model, report.max_attempts, report.started_at, report.finished_at
    );
    text.push_str("| task | arm | trials | success | accepted on 1st attempt | false accepts | rejected | refused | errors | model calls | tokens in / out | test runs | rollbacks | boundary violations | test edits attempted | stayed in intended files | mean wall time |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for c in report.cells.iter().chain(report.arms.iter()) {
        text.push_str(&format!(
            "| {} | {} | {} | {} ({:.0}%) | {} | {} | {} | {} | {} | {} | {} / {} | {} | {} | {} | {} | {}/{} | {:.1} s |\n",
            if c.task == "all" { "**all**".to_string() } else { format!("`{}`", c.task) },
            c.arm,
            c.trials,
            c.successes,
            c.success_rate * 100.0,
            c.accepted_first_attempt,
            c.false_accepts,
            c.rejected,
            c.refused,
            c.errors,
            c.model_calls,
            c.input_tokens,
            c.output_tokens,
            c.test_runs,
            c.rollbacks,
            c.boundary_violations,
            c.test_edit_attempts,
            c.stayed_in_intended_files,
            c.accepted,
            c.mean_wall_ms / 1000.0
        ));
    }
    text.push_str("\n## Trials\n\n| task | arm | trial | outcome | hidden tests | attempts | tokens in / out | test runs | wall time |\n|---|---|---|---|---|---|---|---|---|\n");
    for t in &report.trials {
        text.push_str(&format!(
            "| `{}` | {} | {} | {}{} | {} | {} | {} / {} | {} | {:.1} s |\n",
            t.task,
            t.arm,
            t.trial,
            t.outcome,
            t.error.as_ref().map(|e| format!(": {e}")).unwrap_or_default(),
            match t.hidden_passed {
                Some(true) => "pass",
                Some(false) => "FAIL",
                None => "–",
            },
            t.attempts,
            t.input_tokens,
            t.output_tokens,
            t.test_runs,
            t.wall_ms as f64 / 1000.0
        ));
    }
    text
}
