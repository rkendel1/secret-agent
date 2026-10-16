//! The loop.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use agent_core::appport::knowledge::{model_json, Contract};
use agent_core::observe::{observe, Project};
use agent_core::plan::{plan, task_for, ContextMode, Intent, Plan};
use agent_core::verify::{judge, CheckStatus, Session, Verdict};
use agent_model::{connect, Model, ModelConfig, Request};
use serde_json::json;

use crate::collect::{collect, discover, Counters};
use crate::edit;
use crate::goal::{run_goal_check, GoalCheck};
use crate::host::HostConfig;
use crate::report::{
    now_rfc3339, Attempt, Context, Deterministic, GoalCheckRun, GoalEvidence, InFlight, InFlightFile, Log, RunReport,
    Totals,
};

/// How to run the loop.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub project: PathBuf,
    pub intent: String,
    pub model: ModelConfig,
    pub max_attempts: u32,
    pub host: HostConfig,
    pub report: Option<PathBuf>,
    pub log: Option<PathBuf>,
    pub quiet: bool,
    /// What the model is told (the verifier is the same either way).
    pub context: ContextMode,
    /// A check of the goal itself; a change is accepted only if it fails before
    /// the change and passes after it.
    pub goal_check: Option<GoalCheck>,
    /// Accept only with evidence of the goal: a goal check, or declared
    /// verification that fails before the change and passes after it. The
    /// `run` command requires it unless `--accept-preserving`; the evaluation
    /// harnesses score the goal with their own oracle and set it to false.
    pub require_goal_evidence: bool,
    /// Rewrite files with uncommitted changes (refused by default).
    pub allow_dirty: bool,
}

/// What observation and planning established, before any model call.
pub struct Prepared {
    pub project: Project,
    pub contract: Contract,
    pub plan: Plan,
    pub deterministic: Deterministic,
}

/// Observes the project, reads the contract its runtime publishes, and plans.
/// No model is involved.
pub fn prepare(options: &RunOptions, log: &mut Log) -> Result<Prepared, String> {
    let intent = Intent::parse(&options.intent)?;
    let project = observe(&options.project).map_err(|e| format!("{}: {e}", options.project.display()))?;
    log.event(
        "observe",
        json!({ "project": &project }),
        format!(
            "{} source file(s), @appport packages [{}], application {}",
            project.sources.len(),
            project.appport_packages.iter().map(|(k, v)| format!("{k}@{v}")).collect::<Vec<_>>().join(", "),
            project.application.as_ref().map_or("not found".to_string(), |a| format!("{}#{}", a.module, a.export))
        ),
    );
    for c in &project.capabilities {
        log.event(
            "observe",
            json!({}),
            format!(
                "declares {}@{} at {}:{} → {}",
                c.name,
                c.version,
                c.declared_in,
                c.line,
                c.handler.as_ref().map_or("handler not resolved".to_string(), |h| format!(
                    "{}{}",
                    h.module,
                    h.symbol.as_ref().map(|s| format!("#{s}")).unwrap_or_default()
                ))
            ),
        );
    }
    for finding in &project.findings {
        log.event(
            "observe",
            json!({ "finding": finding }),
            format!("finding {}: {} {}", finding.code, finding.path, finding.message),
        );
    }
    let entry = project.application.clone().ok_or_else(|| {
        format!(
            "no AppPort application entry point was found; nothing to run (findings: {})",
            project.findings.iter().map(|f| f.code.as_str()).collect::<Vec<_>>().join(", ")
        )
    })?;

    let (manifest, envelopes) = discover(&options.host, &project.root, &entry)?;
    let contract = Contract::from_value(manifest).map_err(|e| e.to_string())?;
    let graph = contract.graph();
    log.event(
        "contract",
        json!({ "digest": contract.digest(), "graph": &graph }),
        format!(
            "{} {}: {} capability version(s), {} event(s); graph {} nodes / {} edges; contractDigest {}",
            contract.manifest().application.id,
            contract.manifest().application.version,
            contract.manifest().capability_versions().count(),
            contract.manifest().events.len(),
            graph.nodes.len(),
            graph.edges.len(),
            contract.digest()
        ),
    );

    let plan = plan(&intent, &contract, &project).map_err(|r| format!("refused to plan: {r}"))?;
    for decision in &plan.decisions {
        log.event("plan", json!({ "decision": decision }), format!("{}: {}", decision.step, decision.fact));
    }
    for check in &plan.checks {
        log.event("plan", json!({ "check": check }), format!("accept only if [{}] {}", check.code, check.rule));
    }

    let deterministic = Deterministic {
        source_files_observed: project.sources.len(),
        capabilities_in_contract: contract.manifest().capability_versions().count(),
        capabilities_located_in_source: project.capabilities.iter().filter(|c| c.handler.is_some()).count(),
        graph_nodes: graph.nodes.len(),
        graph_edges: graph.edges.len(),
        dependents_of_target: plan.boundary.dependents.len(),
        replacement_constraints: plan.boundary.constraints.len(),
        acceptance_checks: plan.checks.len(),
        decisions: plan.decisions.len(),
        permissions_granted: plan.permissions.clone(),
        application_runs: 1,
        envelopes_exchanged: envelopes,
        ..Deterministic::default()
    };
    Ok(Prepared { project, contract, plan, deterministic })
}

/// Errors from [`prepare`] that mean "the agent declines", not "the agent broke".
pub fn is_limitation(error: &str) -> bool {
    error.starts_with("refused to plan")
        || error.starts_with("unsupported intent")
        || error.contains("no AppPort application entry point")
}

/// Holds the project for one run: two runs changing the same project at once
/// would undo each other's work. The lock lives in the temporary directory,
/// keyed by the project's canonical path, so nothing is written to the project.
pub struct ProjectLock {
    path: PathBuf,
}

impl ProjectLock {
    pub fn acquire(project: &Path) -> Result<ProjectLock, String> {
        use std::io::Write;
        let root = project.canonicalize().map_err(|e| format!("{}: {e}", project.display()))?;
        let digest = agent_core::appport::knowledge::rules::sha256_hex(&root.to_string_lossy());
        let path = std::env::temp_dir().join(format!("appport-agent-{}.lock", &digest[..16]));
        for _ in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}\n{}", std::process::id(), root.display());
                    return Ok(ProjectLock { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder = std::fs::read_to_string(&path).unwrap_or_default();
                    let pid = holder.lines().next().unwrap_or_default().trim().to_string();
                    if !pid.is_empty() && !alive(&pid) {
                        // The run that held it is gone; its own report says what it left.
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    return Err(format!(
                        "another appport-agent run (pid {pid}) is changing {}; wait for it to finish, or remove {} if no run is active",
                        root.display(),
                        path.display()
                    ));
                }
                Err(e) => return Err(format!("cannot lock {}: {e}", root.display())),
            }
        }
        Err(format!("cannot lock {}", root.display()))
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Whether a process id is running (assumed so where it cannot be checked).
fn alive(pid: &str) -> bool {
    if cfg!(unix) {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(true)
    } else {
        true
    }
}

/// Files among `files` with uncommitted changes (or untracked), when `root` is in
/// a git work tree. Empty when it is not, or git is unavailable.
pub fn uncommitted(root: &Path, files: &[String]) -> Vec<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--untracked-files=all", "--"])
        .args(files)
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.len() > 3).map(|l| l.trim().to_string()).collect()
        }
        _ => Vec::new(),
    }
}

/// Writes the report, if asked to, with every secret redacted.
fn write_report(path: Option<&Path>, report: &RunReport, secrets: &[String]) -> Result<(), String> {
    let Some(path) = path else { return Ok(()) };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(report).expect("report serializes") + "\n";
    let text = agent_model::gateway::redact(&text, secrets);
    // Through a rename, so a report is never half-written.
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)
        .and_then(|_| std::fs::rename(&temp, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Runs the whole loop. `Ok` carries the report for every outcome the run can
/// account for (see [`RunReport::outcome`]); `Err` is a usage or harness error
/// before anything was changed.
pub fn run(options: &RunOptions) -> Result<RunReport, String> {
    let secrets = agent_model::gateway::secrets_of(&options.model);
    let mut log = Log::new(options.log.as_deref(), options.quiet)
        .map_err(|e| format!("run log: {e}"))?
        .redacting(secrets.clone());
    // The application runs code a model wrote: it never sees a provider credential.
    let options = &RunOptions { host: options.host.hiding(&options.model), ..options.clone() };
    let _lock = ProjectLock::acquire(&options.project)?;
    let started_at = now_rfc3339();
    let mut report = RunReport {
        intent: options.intent.clone(),
        context_mode: options.context.to_string(),
        project: display(&options.project),
        application: String::new(),
        model: options.model.model.clone().unwrap_or_else(|| format!("{:?}", options.model.provider)),
        outcome: "limitation".into(),
        reason: None,
        project_state: "unchanged".into(),
        goal: GoalEvidence { required: options.require_goal_evidence, method: "none".into(), detail: String::new() },
        in_flight: None,
        baseline_failures: Vec::new(),
        attempts: Vec::new(),
        totals: Totals::default(),
        deterministic: Deterministic::default(),
        context: Context::default(),
        started_at,
    };
    let finish = |mut report: RunReport, log: &mut Log| -> Result<RunReport, String> {
        let attempts = &report.attempts;
        report.totals = Totals {
            model_calls: attempts.len(),
            input_tokens: attempts.iter().map(|a| a.usage.input_tokens).sum(),
            output_tokens: attempts.iter().map(|a| a.usage.output_tokens).sum(),
            tokens_estimated: attempts.iter().any(|a| a.usage.estimated),
            rejected_attempts: attempts.iter().filter(|a| !a.verdict.as_ref().is_some_and(|v| v.accepted)).count(),
            rollbacks: attempts.iter().filter(|a| a.rolled_back).count(),
            wall_ms: log.elapsed_ms() as u64,
        };
        log.event(
            "done",
            json!({ "report": &report }),
            format!(
                "{}{}: {} model call(s), {} rejected; {} application run(s); project {}",
                report.outcome.to_uppercase(),
                report.reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default(),
                report.totals.model_calls,
                report.totals.rejected_attempts,
                report.deterministic.application_runs,
                report.project_state
            ),
        );
        write_report(options.report.as_deref(), &report, &secrets)?;
        Ok(report)
    };

    let prepared = match prepare(options, &mut log) {
        Ok(prepared) => prepared,
        Err(e) if is_limitation(&e) => {
            report.reason = Some(e);
            return finish(report, &mut log);
        }
        Err(e) => return Err(e),
    };
    let Prepared { project, contract, plan, deterministic } = prepared;
    // After planning, which needs no model, so a declined run never depends on one.
    let mut model: Box<dyn Model> = connect(&options.model).map_err(|e| e.to_string())?;
    report.model = model.describe();
    report.application = plan.application.clone();
    report.deterministic = deterministic;
    let entry = project.application.clone().expect("prepare checked the entry point");
    let root = project.root.clone();
    log.event("model", json!({ "model": model.describe() }), format!("model: {}", model.describe()));
    let editable: Vec<String> = plan.editable().iter().map(|s| s.to_string()).collect();

    // Never take over someone's uncommitted work: a whole-file rewrite of a file
    // with uncommitted changes could lose them for good.
    let dirty = uncommitted(&root, &editable);
    if !dirty.is_empty() && !options.allow_dirty {
        report.reason = Some(format!(
            "uncommitted changes in the files this change would rewrite ({}); commit or stash them, or pass --allow-dirty",
            dirty.join(", ")
        ));
        return finish(report, &mut log);
    }

    // Baseline: the application as it is.
    let mut counters = Counters::default();
    let baseline = collect(&options.host, &root, &entry, &plan, &contract, None, &mut counters);
    report.deterministic.application_runs += 1;
    if let Some(failure) = &baseline.failure {
        return Err(format!("the unchanged application could not be probed: {failure}"));
    }
    log_session(&mut log, "baseline", &baseline);
    let baseline_verdict = judge(&plan, &baseline, &baseline, &editable);
    let baseline_failures: Vec<String> = baseline_verdict
        .checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail && c.code != "edit.boundary")
        .flat_map(|c| c.violations.iter().map(move |d| format!("[{}] {d}", c.code)))
        .collect();
    for failure in &baseline_failures {
        log.event("baseline", json!({ "failure": failure }), format!("already failing: {failure}"));
    }
    report.baseline_failures = baseline_failures.clone();

    // What will show the goal was achieved. Decided before any model call: if
    // nothing can show it, no change can be accepted, so none is asked for.
    if let Some(check) = &options.goal_check {
        match run_goal_check(check, &contract, &options.host, &root, &entry) {
            Ok(()) => {
                report.reason = Some(
                    "the goal check already passes on the unchanged project, so it cannot show that a change achieved the goal"
                        .into(),
                );
                return finish(report, &mut log);
            }
            Err(detail) => {
                log.event(
                    "goal",
                    json!({ "baseline": &detail }),
                    format!("goal check fails before any change: {detail}"),
                );
            }
        }
        report.goal.method = "goal-check".into();
        report.goal.detail = check.description.clone();
    } else if baseline_failures.is_empty() {
        if options.require_goal_evidence {
            report.reason = Some(
                "nothing observable can show this goal was achieved: the contract's declared verification already passes, so a change that preserves behaviour would be indistinguishable from no change; supply --goal-check <file> (see docs/agent-loop.md), or pass --accept-preserving to accept a verified, contract-preserving change without goal evidence"
                    .into(),
            );
            return finish(report, &mut log);
        }
    } else {
        report.goal.method = "declared-verification".into();
        report.goal.detail =
            format!("verification failing before the change must pass after it: {}", baseline_failures.join("; "));
    }
    if !options.require_goal_evidence && options.goal_check.is_none() {
        report.goal.method = "none".into();
        report.goal.detail = "not required: a contract-preserving change is accepted without goal evidence".into();
    }

    // Context for the model: the plan's files, read once.
    let mut files = BTreeMap::new();
    for file in &plan.context {
        files.insert(file.path.clone(), project.read(&file.path).map_err(|e| format!("{}: {e}", file.path))?);
    }
    let sent: Vec<&String> = match options.context {
        ContextMode::Knowledge => files.values().collect(),
        ContextMode::Bare => editable.iter().filter_map(|p| files.get(p)).collect(),
    };
    report.context = Context {
        files_sent: sent.len(),
        project_source_files: project.sources.len(),
        source_bytes_sent: sent.iter().map(|f| f.len()).sum(),
        project_source_bytes: project.source_bytes(),
        knowledge_bytes_sent: match options.context {
            ContextMode::Knowledge => model_json(&plan.view).len(),
            ContextMode::Bare => 0,
        },
    };

    let mut feedback: Vec<String> = Vec::new();
    report.outcome = "rejected".into();
    for attempt in 1..=options.max_attempts.max(1) {
        let began = Instant::now();
        let task = task_for(options.context, &plan, &files, &feedback);
        log.event(
            "model",
            json!({ "attempt": attempt, "task": &task }),
            format!(
                "attempt {attempt}: asking for the implementation ({} bytes of context{})",
                task.prompt.len(),
                if feedback.is_empty() { String::new() } else { format!(", {} violation(s) to fix", feedback.len()) }
            ),
        );
        let request = Request::new(task.system.clone(), task.prompt.clone(), options.model.max_tokens);
        let response = match model.complete(&request) {
            Ok(response) => response,
            Err(e) => {
                // Nothing is applied between attempts, so the project is as it was.
                let message = agent_model::gateway::redact(&e.to_string(), &secrets);
                log.event(
                    "model",
                    json!({ "attempt": attempt, "error": &message }),
                    format!("attempt {attempt}: {message}"),
                );
                report.outcome = "provider-error".into();
                report.reason = Some(message);
                return finish(report, &mut log);
            }
        };
        log.event(
            "model",
            json!({ "attempt": attempt, "response": &response }),
            format!(
                "attempt {attempt}: {} answered ({} in / {} out tokens{}{})",
                response.model,
                response.usage.input_tokens,
                response.usage.output_tokens,
                if response.usage.estimated { ", estimated" } else { "" },
                if response.retries > 0 {
                    format!(", after {} retried request(s)", response.retries)
                } else {
                    String::new()
                }
            ),
        );
        let mut record = Attempt {
            attempt,
            model: response.model.clone(),
            usage: response.usage,
            prompt_bytes: task.system.len() + task.prompt.len(),
            response_bytes: response.text.len(),
            files_changed: Vec::new(),
            unusable: None,
            verdict: None,
            evidence: Vec::new(),
            rolled_back: false,
            rollback: None,
            goal_check: None,
            stop_reason: response.stop_reason.clone(),
            contract_changed: false,
            outside_boundary: false,
            duration_ms: 0,
        };

        let parsed =
            edit::parse(&response.text).or_else(|problem| match edit::single_fenced(&response.text, &editable) {
                Some(single) => {
                    log.event(
                        "apply",
                        json!({ "attempt": attempt, "lenient": &single.path }),
                        format!(
                            "attempt {attempt}: no file block; the response's only code block is taken as {}",
                            single.path
                        ),
                    );
                    Ok(vec![single])
                }
                None if response.stop_reason.as_deref() == Some("max_tokens") => {
                    Err(format!("{problem} (the response was cut off at max_tokens)"))
                }
                None => Err(problem),
            });
        let edits = match parsed.and_then(|e| edit::targets(&e).map(|t| (e, t))) {
            Ok(parsed) => parsed,
            Err(problem) => {
                log.event(
                    "apply",
                    json!({ "attempt": attempt, "unusable": &problem }),
                    format!("attempt {attempt}: unusable response: {problem}"),
                );
                feedback = vec![format!("[response.format] {problem}")];
                record.unusable = Some(problem);
                record.duration_ms = began.elapsed().as_millis() as u64;
                report.attempts.push(record);
                continue;
            }
        };
        let (edits, targets) = edits;
        let outside: Vec<&String> = targets.iter().filter(|t| !editable.contains(t)).collect();
        let (candidate, applied, changed) = if outside.is_empty() {
            let applied = match edit::apply(&root, &edits) {
                Ok(applied) => applied,
                Err(e) => match e.rollback.clone() {
                    // Something was written and could not be undone and checked.
                    Some(r) if !r.verified() => {
                        report.outcome = "rollback-failed".into();
                        report.project_state = "uncertain".into();
                        report.reason = Some(format!("writing attempt {attempt} failed: {e}"));
                        record.rollback = Some(r);
                        report.attempts.push(record);
                        return finish(report, &mut log);
                    }
                    Some(_) => {
                        report.outcome = "harness-error".into();
                        report.reason = Some(format!("writing attempt {attempt} failed and was undone: {e}"));
                        report.attempts.push(record);
                        return finish(report, &mut log);
                    }
                    // Refused before anything was written: the proposal's problem.
                    None => {
                        log.event(
                            "apply",
                            json!({ "attempt": attempt, "unusable": &e.message }),
                            format!("attempt {attempt}: {e}"),
                        );
                        feedback = vec![format!("[edit.boundary] {}", e.message)];
                        record.unusable = Some(e.message);
                        record.duration_ms = began.elapsed().as_millis() as u64;
                        report.attempts.push(record);
                        continue;
                    }
                },
            };
            let changed = applied.changed.clone();
            // The recovery record goes out before the changed application runs.
            let originals: BTreeMap<String, Option<String>> = applied.originals().into_iter().collect();
            report.in_flight = Some(InFlight {
                attempt,
                files: applied
                    .digests()
                    .into_iter()
                    .map(|(path, existed, before, written)| InFlightFile {
                        original: originals.get(&path).cloned().flatten(),
                        path,
                        existed_before: existed,
                        sha256_before: before,
                        sha256_written: written,
                    })
                    .collect(),
            });
            let outcome = std::mem::replace(&mut report.outcome, "interrupted".into());
            write_report(options.report.as_deref(), &report, &secrets)?;
            report.outcome = outcome;
            log.event(
                "apply",
                json!({ "attempt": attempt, "changed": &changed, "inFlight": &report.in_flight }),
                format!(
                    "attempt {attempt}: wrote {}",
                    if changed.is_empty() { "nothing (identical)".to_string() } else { changed.join(", ") }
                ),
            );
            let candidate = collect(&options.host, &root, &entry, &plan, &contract, Some(&baseline), &mut counters);
            report.deterministic.application_runs += 1;
            (candidate, Some(applied), changed)
        } else {
            log.event(
                "apply",
                json!({ "attempt": attempt, "outside": &outside }),
                format!(
                    "attempt {attempt}: not applied, outside the boundary: {}",
                    outside.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ),
            );
            let session = Session {
                failure: Some("not run: the change writes outside its boundary".into()),
                ..Session::default()
            };
            (session, None, targets.clone())
        };
        log_session(&mut log, "candidate", &candidate);
        let verdict = judge(&plan, &baseline, &candidate, &changed);
        log_verdict(&mut log, attempt, &verdict);
        report.deterministic.verification_methods_evaluated +=
            candidate.evidence.iter().map(|e| e.methods.len()).sum::<usize>();
        record.files_changed = changed;
        let failed = |code: &str| verdict.checks.iter().any(|c| c.code == code && c.status == CheckStatus::Fail);
        record.outside_boundary = failed("edit.boundary");
        // Only meaningful when the application ran and published a manifest.
        record.contract_changed = candidate.manifest.is_some() && failed("contract.identity");
        record.evidence = candidate
            .evidence
            .iter()
            .map(|e| {
                let outcome = serde_json::to_value(e.outcome).ok().and_then(|v| v.as_str().map(str::to_string));
                format!("{}@{}: {}", e.capability.name, e.capability.version, outcome.unwrap_or_default())
            })
            .collect();
        feedback = verdict.feedback();
        let mut accepted = verdict.accepted;
        // Preservation is not the goal: with a goal check, the change must also achieve it.
        if accepted {
            if let Some(check) = &options.goal_check {
                let result = run_goal_check(check, &contract, &options.host, &root, &entry);
                report.deterministic.application_runs += 1;
                log.event(
                    "goal",
                    json!({ "attempt": attempt, "goal": result.as_ref().err() }),
                    match &result {
                        Ok(()) => format!("attempt {attempt}: goal check passed"),
                        Err(detail) => format!("attempt {attempt}: goal check FAILED: {detail}"),
                    },
                );
                if let Err(detail) = &result {
                    accepted = false;
                    feedback = vec![format!(
                        "[goal.not-achieved] the change preserves behaviour but does not achieve the goal: {detail}"
                    )];
                }
                record.goal_check = Some(GoalCheckRun {
                    passed: result.is_ok(),
                    detail: result.err().unwrap_or_else(|| "achieved".into()),
                });
            }
        }
        record.duration_ms = began.elapsed().as_millis() as u64;
        record.verdict = Some(verdict);
        if accepted {
            report.attempts.push(record);
            report.in_flight = None;
            report.outcome = "accepted".into();
            report.project_state = "changed".into();
            break;
        }
        if let Some(applied) = applied {
            let undone = edit::rollback(&root, applied);
            record.rolled_back = undone.verified();
            let verified = undone.verified();
            log.event(
                "apply",
                json!({ "attempt": attempt, "rollback": &undone }),
                format!(
                    "attempt {attempt}: rejected; {}",
                    if verified { "the project is restored and checked".to_string() } else { undone.describe() }
                ),
            );
            record.rollback = Some(undone);
            if !verified {
                report.reason = Some(format!(
                    "attempt {attempt} was rejected and could not be undone cleanly: {}",
                    record.rollback.as_ref().map(edit::Rollback::describe).unwrap_or_default()
                ));
                report.attempts.push(record);
                report.outcome = "rollback-failed".into();
                report.project_state = "uncertain".into();
                return finish(report, &mut log);
            }
            report.in_flight = None;
        }
        report.attempts.push(record);
    }
    report.deterministic.verification_methods_evaluated +=
        baseline.evidence.iter().map(|e| e.methods.len()).sum::<usize>();
    report.deterministic.envelopes_exchanged += counters.envelopes;
    if report.outcome == "rejected" && report.reason.is_none() {
        report.reason =
            Some(format!("no attempt of {} passed every check; every attempt was undone", report.attempts.len()));
    }
    finish(report, &mut log)
}

fn display(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn log_session(log: &mut Log, label: &str, session: &Session) {
    let mut lines = Vec::new();
    for call in &session.calls {
        let answer = match &call.reply {
            agent_core::verify::Reply::Ok { .. } => "ok".to_string(),
            agent_core::verify::Reply::Err { code, .. } => code.clone(),
        };
        let phase =
            serde_json::to_value(call.phase).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        lines.push(format!("{phase} {}@{} {} → {answer}", call.capability.name, call.capability.version, call.input));
    }
    for evidence in &session.evidence {
        let methods: Vec<String> = evidence
            .methods
            .iter()
            .map(|m| {
                format!(
                    "{} {}",
                    m.id,
                    serde_json::to_value(m.status)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default()
                )
            })
            .collect();
        lines.push(format!(
            "evidence {}@{}: {} ({})",
            evidence.capability.name,
            evidence.capability.version,
            serde_json::to_value(evidence.outcome)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            methods.join(", ")
        ));
    }
    if let Some(failure) = &session.failure {
        lines.push(format!("did not run: {failure}"));
    }
    log.event(label, json!({ "session": session }), lines.join("\n"));
}

fn log_verdict(log: &mut Log, attempt: u32, verdict: &Verdict) {
    let mut lines = Vec::new();
    for check in &verdict.checks {
        let mark = if check.status == CheckStatus::Pass { "pass" } else { "FAIL" };
        lines.push(format!("{mark} [{}] {}", check.code, check.rule));
        if check.status == CheckStatus::Fail {
            lines.extend(check.violations.iter().map(|d| format!("     - {d}")));
        }
    }
    lines.push(format!("attempt {attempt}: {}", if verdict.accepted { "ACCEPTED" } else { "REJECTED" }));
    log.event("verify", json!({ "attempt": attempt, "verdict": verdict }), lines.join("\n"));
}
