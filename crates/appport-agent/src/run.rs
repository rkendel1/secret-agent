//! The loop.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use agent_core::appport::knowledge::{model_json, Contract};
use agent_core::observe::{observe, Project};
use agent_core::plan::{model_task, plan, Intent, Plan};
use agent_core::verify::{judge, CheckStatus, Session, Verdict};
use agent_model::{connect, Model, ModelConfig, Request};
use serde_json::json;

use crate::collect::{collect, discover, Counters};
use crate::edit;
use crate::host::HostConfig;
use crate::report::{now_rfc3339, Attempt, Context, Deterministic, Log, RunReport, Totals};

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
    let entry = project.application.clone().ok_or("no application entry point was found; nothing to run")?;

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

/// Runs the whole loop. `Ok` carries the report whether or not a change was accepted.
pub fn run(options: &RunOptions) -> Result<RunReport, String> {
    let mut log = Log::new(options.log.as_deref(), options.quiet).map_err(|e| format!("run log: {e}"))?;
    let started_at = now_rfc3339();
    let Prepared { project, contract, plan, mut deterministic } = prepare(options, &mut log)?;
    let entry = project.application.clone().expect("prepare checked the entry point");
    let root = project.root.clone();
    let mut model: Box<dyn Model> = connect(&options.model).map_err(|e| e.to_string())?;
    log.event("model", json!({ "model": model.describe() }), format!("model: {}", model.describe()));

    // Baseline: the application as it is.
    let mut counters = Counters::default();
    let baseline = collect(&options.host, &root, &entry, &plan, &contract, None, &mut counters);
    deterministic.application_runs += 1;
    if let Some(failure) = &baseline.failure {
        return Err(format!("the unchanged application could not be probed: {failure}"));
    }
    log_session(&mut log, "baseline", &baseline);
    let editable: Vec<String> = plan.editable().iter().map(|s| s.to_string()).collect();
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

    // Context for the model: the plan's files, read once.
    let mut files = BTreeMap::new();
    for file in &plan.context {
        files.insert(file.path.clone(), project.read(&file.path).map_err(|e| format!("{}: {e}", file.path))?);
    }
    let context = Context {
        files_sent: plan.context.len(),
        project_source_files: project.sources.len(),
        source_bytes_sent: files.values().map(String::len).sum(),
        project_source_bytes: project.source_bytes(),
        knowledge_bytes_sent: model_json(&plan.view).len(),
    };

    let mut attempts = Vec::new();
    let mut feedback: Vec<String> = Vec::new();
    let mut accepted = false;
    for attempt in 1..=options.max_attempts.max(1) {
        let began = Instant::now();
        let task = model_task(&plan, &files, &feedback);
        log.event(
            "model",
            json!({ "attempt": attempt, "task": &task }),
            format!(
                "attempt {attempt}: asking for the implementation ({} bytes of context{})",
                task.prompt.len(),
                if feedback.is_empty() { String::new() } else { format!(", {} violation(s) to fix", feedback.len()) }
            ),
        );
        let request =
            Request { system: task.system.clone(), prompt: task.prompt.clone(), max_tokens: options.model.max_tokens };
        let response = model.complete(&request).map_err(|e| e.to_string())?;
        log.event(
            "model",
            json!({ "attempt": attempt, "response": &response }),
            format!(
                "attempt {attempt}: {} answered ({} in / {} out tokens{})",
                response.model,
                response.usage.input_tokens,
                response.usage.output_tokens,
                if response.usage.estimated { ", estimated" } else { "" }
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
            duration_ms: 0,
        };

        let edits = match edit::parse(&response.text).and_then(|e| edit::targets(&e).map(|t| (e, t))) {
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
                attempts.push(record);
                continue;
            }
        };
        let (edits, targets) = edits;
        let outside: Vec<&String> = targets.iter().filter(|t| !editable.contains(t)).collect();
        let (candidate, applied, changed) = if outside.is_empty() {
            let applied = edit::apply(&root, &edits)?;
            let changed = applied.changed.clone();
            log.event(
                "apply",
                json!({ "attempt": attempt, "changed": &changed }),
                format!(
                    "attempt {attempt}: wrote {}",
                    if changed.is_empty() { "nothing (identical)".to_string() } else { changed.join(", ") }
                ),
            );
            let candidate = collect(&options.host, &root, &entry, &plan, &contract, Some(&baseline), &mut counters);
            deterministic.application_runs += 1;
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
        deterministic.verification_methods_evaluated +=
            candidate.evidence.iter().map(|e| e.methods.len()).sum::<usize>();
        record.files_changed = changed;
        record.duration_ms = began.elapsed().as_millis() as u64;
        if verdict.accepted {
            record.verdict = Some(verdict);
            attempts.push(record);
            accepted = true;
            break;
        }
        if let Some(applied) = applied {
            edit::rollback(&root, applied);
            log.event(
                "apply",
                json!({ "attempt": attempt }),
                format!("attempt {attempt}: rejected; the project is restored"),
            );
        }
        feedback = verdict.feedback();
        record.verdict = Some(verdict);
        attempts.push(record);
    }
    deterministic.verification_methods_evaluated += baseline.evidence.iter().map(|e| e.methods.len()).sum::<usize>();
    deterministic.envelopes_exchanged += counters.envelopes;

    let totals = Totals {
        model_calls: attempts.len(),
        input_tokens: attempts.iter().map(|a| a.usage.input_tokens).sum(),
        output_tokens: attempts.iter().map(|a| a.usage.output_tokens).sum(),
        tokens_estimated: attempts.iter().any(|a| a.usage.estimated),
        rejected_attempts: attempts.iter().filter(|a| !a.verdict.as_ref().is_some_and(|v| v.accepted)).count(),
        wall_ms: log.elapsed_ms() as u64,
    };
    let report = RunReport {
        intent: options.intent.clone(),
        project: display(&options.project),
        application: plan.application.clone(),
        model: model.describe(),
        outcome: if accepted { "accepted" } else { "rejected" }.to_string(),
        baseline_failures,
        attempts,
        totals,
        deterministic,
        context,
        started_at,
    };
    log.event(
        "done",
        json!({ "report": &report }),
        format!(
            "{}: {} model call(s), {} rejected; {} application run(s); model used for discovery 0 times, for verification 0 times",
            report.outcome.to_uppercase(),
            report.totals.model_calls,
            report.totals.rejected_attempts,
            report.deterministic.application_runs
        ),
    );
    if let Some(path) = &options.report {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&report).expect("report serializes") + "\n")
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(report)
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
