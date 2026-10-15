//! The closed-loop experiment: the same intent, many times, with and without
//! the AppPort knowledge in the model's context.
//!
//! Every trial runs the whole loop on a fresh copy of the project, so trials
//! never see each other's changes. Both arms are judged by the same
//! deterministic verifier; only what the model is told differs
//! ([`ContextMode`]). The aggregate answers, with numbers from the running
//! application rather than from the model:
//!
//! - does the loop reach a verified, accepted change without a human?
//! - how often is the first proposal already acceptable, with and without the knowledge?
//! - what does each arm cost (model calls, tokens, rejected attempts, rollbacks, time)?
//! - which checks reject the proposals that fail?

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use agent_core::plan::ContextMode;
use agent_model::ModelConfig;
use serde::Serialize;

use crate::host::HostConfig;
use crate::report::{now_rfc3339, RunReport};
use crate::run::{run, RunOptions};

/// Directories never copied into a trial (`node_modules` is linked instead).
const SKIP: [&str; 5] = ["node_modules", ".git", "target", "dist", "coverage"];

/// How to run the experiment.
#[derive(Debug, Clone)]
pub struct EvalOptions {
    /// The project every trial starts from. Never modified.
    pub project: PathBuf,
    pub intent: String,
    pub model: ModelConfig,
    pub arms: Vec<ContextMode>,
    pub trials: u32,
    pub max_attempts: u32,
    pub host: HostConfig,
    /// Where trials, their reports and logs, `eval.json` and `eval.md` go.
    pub out: PathBuf,
    pub quiet: bool,
}

/// One trial's result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trial {
    pub arm: String,
    pub trial: u32,
    /// `accepted`, `rejected`, or `error` when the loop could not run.
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub attempts: usize,
    pub model_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tokens_estimated: bool,
    pub rejected_attempts: usize,
    pub rollbacks: usize,
    pub unusable_responses: usize,
    /// Proposals that changed the published contract.
    pub contract_edits: usize,
    /// Proposals that wrote outside the edit boundary.
    pub boundary_violations: usize,
    /// Check codes that rejected an attempt, one entry per rejecting check.
    pub rejected_by: Vec<String>,
    pub wall_ms: u64,
    /// The trial's own report, relative to the eval directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

/// One arm, aggregated.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Arm {
    pub arm: String,
    pub trials: usize,
    pub accepted: usize,
    pub accepted_first_attempt: usize,
    pub accepted_after_retry: usize,
    /// Every attempt was rejected.
    pub rejected: usize,
    pub errors: usize,
    pub acceptance_rate: f64,
    pub first_attempt_rate: f64,
    /// Mean attempts over accepted trials.
    pub mean_attempts_to_accept: Option<f64>,
    pub model_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tokens_estimated: bool,
    pub rejected_attempts: usize,
    pub rollbacks: usize,
    pub unusable_responses: usize,
    pub contract_edits: usize,
    pub boundary_violations: usize,
    /// Model calls and tokens per accepted trial (all trials' spend / accepted trials).
    pub model_calls_per_accepted: Option<f64>,
    pub tokens_per_accepted: Option<f64>,
    /// How often each check rejected an attempt.
    pub rejected_by: BTreeMap<String, usize>,
    pub mean_wall_ms: f64,
}

/// The experiment's result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalReport {
    pub intent: String,
    pub project: String,
    pub model: String,
    pub max_attempts: u32,
    pub started_at: String,
    pub finished_at: String,
    pub arms: Vec<Arm>,
    pub trials: Vec<Trial>,
}

impl EvalReport {
    /// The aggregate for `arm`.
    pub fn arm(&self, arm: ContextMode) -> Option<&Arm> {
        self.arms.iter().find(|a| a.arm == arm.to_string())
    }
}

/// Copies a project for one trial: everything but build output and VCS data,
/// with `node_modules` linked to the original's.
pub fn copy_project(from: &Path, to: &Path) -> std::io::Result<()> {
    fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let name = entry.file_name();
            if SKIP.contains(&name.to_string_lossy().as_ref()) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                copy(&entry.path(), &to.join(&name))?;
            } else if kind.is_file() {
                fs::copy(entry.path(), to.join(&name))?;
            }
        }
        Ok(())
    }
    copy(from, to)?;
    let modules = from.join("node_modules");
    if modules.exists() {
        link(&modules.canonicalize()?, &to.join("node_modules"))?;
    }
    Ok(())
}

#[cfg(unix)]
fn link(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(from, to)
}

#[cfg(not(unix))]
fn link(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(from, to)
}

/// Runs every trial of every arm, then writes `eval.json` and `eval.md` to `out`.
pub fn eval(options: &EvalOptions) -> Result<EvalReport, String> {
    let started_at = now_rfc3339();
    let source = options.project.canonicalize().map_err(|e| format!("{}: {e}", options.project.display()))?;
    fs::create_dir_all(&options.out).map_err(|e| format!("{}: {e}", options.out.display()))?;
    let out = options.out.canonicalize().map_err(|e| e.to_string())?;
    let mut trials = Vec::new();
    let mut model = String::new();
    for arm in &options.arms {
        for trial in 1..=options.trials {
            let dir = out.join(format!("{arm}-{trial:02}"));
            let _ = fs::remove_dir_all(&dir);
            let project = dir.join("project");
            copy_project(&source, &project).map_err(|e| format!("copying the project: {e}"))?;
            if !options.quiet {
                eprintln!("== {arm} trial {trial}/{}", options.trials);
            }
            let result = run(&RunOptions {
                project: project.clone(),
                intent: options.intent.clone(),
                model: options.model.clone(),
                max_attempts: options.max_attempts,
                host: options.host.clone(),
                report: Some(dir.join("report.json")),
                log: Some(dir.join("run.jsonl")),
                quiet: options.quiet,
                context: *arm,
            });
            if let Ok(report) = &result {
                if report.outcome == "accepted" {
                    save_accepted_diff(&source, &project, report, &dir.join("accepted.diff"));
                }
            }
            let relative = format!("{arm}-{trial:02}/report.json");
            trials.push(match result {
                Ok(report) => {
                    model = report.model.clone();
                    summarize(arm, trial, &report, relative)
                }
                Err(error) => Trial {
                    arm: arm.to_string(),
                    trial,
                    outcome: "error".into(),
                    error: Some(error),
                    attempts: 0,
                    model_calls: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    tokens_estimated: false,
                    rejected_attempts: 0,
                    rollbacks: 0,
                    unusable_responses: 0,
                    contract_edits: 0,
                    boundary_violations: 0,
                    rejected_by: Vec::new(),
                    wall_ms: 0,
                    report: None,
                },
            });
        }
    }
    let arms = options.arms.iter().map(|arm| aggregate(arm, &trials)).collect();
    let report = EvalReport {
        intent: options.intent.clone(),
        project: options.project.to_string_lossy().to_string(),
        model,
        max_attempts: options.max_attempts,
        started_at,
        finished_at: now_rfc3339(),
        arms,
        trials,
    };
    fs::write(out.join("eval.json"), serde_json::to_string_pretty(&report).expect("serializes") + "\n")
        .map_err(|e| e.to_string())?;
    fs::write(out.join("eval.md"), markdown(&report)).map_err(|e| e.to_string())?;
    Ok(report)
}

/// Writes `diff -u` of every file the accepted attempt changed, against the template.
fn save_accepted_diff(template: &Path, project: &Path, report: &RunReport, out: &Path) {
    let Some(last) = report.attempts.last() else { return };
    let mut text = String::new();
    for path in &last.files_changed {
        let output = std::process::Command::new("diff")
            .arg("-u")
            .arg("--label")
            .arg(format!("a/{path}"))
            .arg("--label")
            .arg(format!("b/{path}"))
            .arg(template.join(path))
            .arg(project.join(path))
            .output();
        if let Ok(output) = output {
            text.push_str(&String::from_utf8_lossy(&output.stdout));
        }
    }
    let _ = fs::write(out, text);
}

fn summarize(arm: &ContextMode, trial: u32, report: &RunReport, path: String) -> Trial {
    let rejected_by = report
        .attempts
        .iter()
        .filter_map(|a| a.verdict.as_ref())
        .filter(|v| !v.accepted)
        .flat_map(|v| v.failed().into_iter().map(str::to_string))
        .chain(report.attempts.iter().filter(|a| a.unusable.is_some()).map(|_| "response.format".to_string()))
        .collect();
    Trial {
        arm: arm.to_string(),
        trial,
        outcome: report.outcome.clone(),
        error: None,
        attempts: report.attempts.len(),
        model_calls: report.totals.model_calls,
        input_tokens: report.totals.input_tokens,
        output_tokens: report.totals.output_tokens,
        tokens_estimated: report.totals.tokens_estimated,
        rejected_attempts: report.totals.rejected_attempts,
        rollbacks: report.totals.rollbacks,
        unusable_responses: report.attempts.iter().filter(|a| a.unusable.is_some()).count(),
        contract_edits: report.attempts.iter().filter(|a| a.contract_changed).count(),
        boundary_violations: report.attempts.iter().filter(|a| a.outside_boundary).count(),
        rejected_by,
        wall_ms: report.totals.wall_ms,
        report: Some(path),
    }
}

fn aggregate(arm: &ContextMode, trials: &[Trial]) -> Arm {
    let mine: Vec<&Trial> = trials.iter().filter(|t| t.arm == arm.to_string()).collect();
    let accepted: Vec<&&Trial> = mine.iter().filter(|t| t.outcome == "accepted").collect();
    let n = mine.len().max(1) as f64;
    let mut rejected_by = BTreeMap::new();
    for code in mine.iter().flat_map(|t| &t.rejected_by) {
        *rejected_by.entry(code.clone()).or_insert(0) += 1;
    }
    Arm {
        arm: arm.to_string(),
        trials: mine.len(),
        accepted: accepted.len(),
        accepted_first_attempt: accepted.iter().filter(|t| t.attempts == 1).count(),
        accepted_after_retry: accepted.iter().filter(|t| t.attempts > 1).count(),
        rejected: mine.iter().filter(|t| t.outcome == "rejected").count(),
        errors: mine.iter().filter(|t| t.outcome == "error").count(),
        acceptance_rate: accepted.len() as f64 / n,
        first_attempt_rate: accepted.iter().filter(|t| t.attempts == 1).count() as f64 / n,
        mean_attempts_to_accept: (!accepted.is_empty())
            .then(|| accepted.iter().map(|t| t.attempts as f64).sum::<f64>() / accepted.len() as f64),
        model_calls: mine.iter().map(|t| t.model_calls).sum(),
        input_tokens: mine.iter().map(|t| t.input_tokens).sum(),
        output_tokens: mine.iter().map(|t| t.output_tokens).sum(),
        tokens_estimated: mine.iter().any(|t| t.tokens_estimated),
        rejected_attempts: mine.iter().map(|t| t.rejected_attempts).sum(),
        rollbacks: mine.iter().map(|t| t.rollbacks).sum(),
        unusable_responses: mine.iter().map(|t| t.unusable_responses).sum(),
        contract_edits: mine.iter().map(|t| t.contract_edits).sum(),
        boundary_violations: mine.iter().map(|t| t.boundary_violations).sum(),
        model_calls_per_accepted: (!accepted.is_empty())
            .then(|| mine.iter().map(|t| t.model_calls).sum::<usize>() as f64 / accepted.len() as f64),
        tokens_per_accepted: (!accepted.is_empty())
            .then(|| mine.iter().map(|t| t.input_tokens + t.output_tokens).sum::<u64>() as f64 / accepted.len() as f64),
        rejected_by,
        mean_wall_ms: mine.iter().map(|t| t.wall_ms as f64).sum::<f64>() / n,
    }
}

/// A human-readable summary of the experiment.
pub fn markdown(report: &EvalReport) -> String {
    let mut text = format!(
        "# Closed-loop evaluation\n\n- intent: `{}`\n- project: `{}`\n- model: `{}`\n- attempts per trial: up to {}\n- run: {} → {}\n\n",
        report.intent, report.project, report.model, report.max_attempts, report.started_at, report.finished_at
    );
    text.push_str("## Outcomes\n\n| arm | trials | accepted on 1st attempt | accepted after retry | rejected (all attempts) | errors | acceptance rate | mean attempts to accept |\n|---|---|---|---|---|---|---|---|\n");
    let pct = |n: usize, of: usize| if of == 0 { 0.0 } else { n as f64 * 100.0 / of as f64 };
    for arm in &report.arms {
        text.push_str(&format!(
            "| {} | {} | {} ({:.0}%) | {} | {} | {} | {:.0}% | {} |\n",
            arm.arm,
            arm.trials,
            arm.accepted_first_attempt,
            pct(arm.accepted_first_attempt, arm.trials),
            arm.accepted_after_retry,
            arm.rejected,
            arm.errors,
            arm.acceptance_rate * 100.0,
            arm.mean_attempts_to_accept.map_or("–".to_string(), |m| format!("{m:.2}")),
        ));
    }
    text.push_str("\n## Cost and behaviour\n\n| arm | model calls | tokens in / out | model calls per accepted | tokens per accepted | rejected attempts | rollbacks | contract edits | boundary violations | unusable responses | mean wall time |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
    for arm in &report.arms {
        text.push_str(&format!(
            "| {} | {} | {} / {}{} | {} | {} | {} | {} | {} | {} | {} | {:.1} s |\n",
            arm.arm,
            arm.model_calls,
            arm.input_tokens,
            arm.output_tokens,
            if arm.tokens_estimated { " (est.)" } else { "" },
            arm.model_calls_per_accepted.map_or("–".to_string(), |m| format!("{m:.2}")),
            arm.tokens_per_accepted.map_or("–".to_string(), |t| format!("{t:.0}")),
            arm.rejected_attempts,
            arm.rollbacks,
            arm.contract_edits,
            arm.boundary_violations,
            arm.unusable_responses,
            arm.mean_wall_ms / 1000.0
        ));
    }
    text.push_str("\n## Why proposals were rejected\n\n| arm | check | rejections |\n|---|---|---|\n");
    for arm in &report.arms {
        for (code, count) in &arm.rejected_by {
            text.push_str(&format!("| {} | `{code}` | {count} |\n", arm.arm));
        }
    }
    text.push_str("\n## Trials\n\n| arm | trial | outcome | attempts | rejected by | tokens in / out | wall time |\n|---|---|---|---|---|---|---|\n");
    for t in &report.trials {
        text.push_str(&format!(
            "| {} | {} | {}{} | {} | {} | {} / {} | {:.1} s |\n",
            t.arm,
            t.trial,
            t.outcome,
            t.error.as_ref().map(|e| format!(": {e}")).unwrap_or_default(),
            t.attempts,
            if t.rejected_by.is_empty() { "–".to_string() } else { t.rejected_by.join(", ") },
            t.input_tokens,
            t.output_tokens,
            t.wall_ms as f64 / 1000.0
        ));
    }
    text
}
