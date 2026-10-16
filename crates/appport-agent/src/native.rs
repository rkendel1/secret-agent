//! The AppPort-native workflow (experimental; off unless the three-arm
//! evaluation selects it).
//!
//! Here the model works *through* AppPort rather than receiving a summary of
//! it. Each reply is one tool call, and every tool is an existing interface:
//!
//! | tool | what it uses |
//! |---|---|
//! | `list_capabilities` | the running application's `appport.manifest@1` |
//! | `inspect` | `Contract::inspect` and `Contract::replacement_boundary` on that manifest, plus where the project declares and implements the capability |
//! | `read` | a project source file (read-only) |
//! | `invoke` | an **observation** capability on the running application, under exactly its declared permissions; consequential capabilities are refused |
//! | `propose` | the deterministic planner, edit boundary, evidence collection and judge the other arms use; the verdict is returned |
//! | `complete` | accepted only when a proposal has been verified |
//! | `limitation` | ends the run without a change |
//!
//! No new AppPort API and no second contract authority: everything the model
//! learns comes from the manifest the runtime publishes and from the project.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use agent_core::appport::knowledge::{model_json, CapabilityEffect, Contract};
use agent_core::observe::{normalize, observe, ApplicationEntry, Project};
use agent_core::plan::{plan, Intent, BUILTIN_PREFIX, FILE_CLOSE, FILE_OPEN};
use agent_core::verify::{judge, CheckStatus, Reply, Session, Verdict};
use agent_model::{connect, Model, ModelConfig, Request, Role, Turn, Usage};
use serde::Serialize;
use serde_json::{json, Value};

use crate::collect::{collect, discover, Counters};
use crate::edit;
use crate::host::{Host, HostConfig};
use crate::report::{now_rfc3339, Log};

/// Largest file `read` returns.
const READ_LIMIT: usize = 48 * 1024;

const ACTION_OPEN: &str = "=== ACTION: ";
const ACTION_CLOSE: &str = "=== END ACTION ===";

/// How to run the native workflow.
#[derive(Debug, Clone)]
pub struct NativeOptions {
    pub project: PathBuf,
    pub goal: String,
    /// `name@version`, when the task names it.
    pub target: Option<String>,
    pub model: ModelConfig,
    /// Most proposals (the same budget as the other arms' attempts).
    pub max_proposals: u32,
    /// Most model calls in total.
    pub max_turns: u32,
    pub host: HostConfig,
    pub log: Option<PathBuf>,
    pub quiet: bool,
}

/// One model turn.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeTurn {
    pub turn: u32,
    pub usage: Usage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// The tool called, or `unusable`.
    pub action: String,
    /// Bytes of the tool result sent back to the model.
    pub result_bytes: usize,
    pub duration_ms: u64,
}

/// One proposal and its verdict.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub target: String,
    pub files: Vec<String>,
    pub accepted: bool,
    pub outside_boundary: bool,
    pub contract_changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    /// Outcome of each declared verification the candidate ran.
    pub evidence: Vec<String>,
}

/// The report of one native run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeReport {
    /// `accepted`, `limitation`, or `incomplete` (turns ran out).
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub model: String,
    pub turns: Vec<NativeTurn>,
    pub tool_calls: BTreeMap<String, usize>,
    pub proposals: Vec<Proposal>,
    /// Completion calls refused because nothing verified was applied.
    pub refused_completions: usize,
    pub model_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tokens_estimated: bool,
    /// Bytes of AppPort discovery and inspection results sent to the model.
    pub discovery_bytes: usize,
    /// Bytes of every tool result sent to the model.
    pub tool_result_bytes: usize,
    pub application_runs: usize,
    pub envelopes: usize,
    pub rollbacks: usize,
    pub accepted_files: Vec<String>,
    pub wall_ms: u64,
    pub started_at: String,
}

/// The system prompt: the tools and the rules.
pub fn system_prompt() -> String {
    format!(
        "You are the implementation step of an agent that changes AppPort applications. You work only through the \
tools below: each reply is exactly one tool call. The agent runs every tool for you and returns the result.\n\n\
Tools:\n\
- list_capabilities: the capabilities the running application publishes. Input: {{}}\n\
- inspect: a capability's contract (inputs, outputs, effect, authorization, declared failures, verification \
methods), who depends on it, what a replacement must preserve, and which source module implements it. Input: \
{{\"capability\": \"name@version\"}}\n\
- read: one project source file. Input: {{\"path\": \"src/...\"}}\n\
- invoke: call an observation capability on the running application. Consequential capabilities are not \
permitted. Input: {{\"capability\": \"name@version\", \"input\": {{...}}}}\n\
- propose: replace the implementation of one capability. Put the complete new contents of each file you change \
after the action, as {FILE_OPEN}<path> ===\\n<contents>\\n{FILE_CLOSE}. Only the module implementing that capability may \
change. The agent runs the application and checks the change against the contract, its declared verification and \
the answers it gave before, and returns the verdict. Input: {{\"capability\": \"name@version\"}}\n\
- complete: finish. Accepted only after a proposal was verified. Input: {{\"summary\": \"...\"}}\n\
- limitation: finish without a change, when the goal cannot be met by changing an implementation alone (it needs \
a contract change or a capability the application does not have), or when a change cannot be verified. \
Input: {{\"reason\": \"...\"}}\n\n\
Format of every reply:\n{ACTION_OPEN}<tool> ===\n<JSON input>\n{ACTION_CLOSE}\n\
(for propose, followed by the file blocks)\n\n\
Rules: never change a contract or a test; report a limitation instead of proposing a change that would need one; \
call complete only after the agent reported a verified proposal."
    )
}

/// The first user message.
pub fn first_message(goal: &str, target: Option<&str>) -> String {
    let mut text = format!("# Task\n{goal}\n");
    if let Some(target) = target {
        text.push_str(&format!("\nCapability: {target}\n"));
    }
    text
}

/// A parsed reply: one action, its JSON input, and the reply text (for file blocks).
struct Action {
    tool: String,
    input: Value,
}

fn parse_action(text: &str) -> Result<Action, String> {
    let mut found = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim().strip_prefix(ACTION_OPEN.trim_end()) else { continue };
        let tool = rest.trim().trim_end_matches('=').trim().to_string();
        let mut body = Vec::new();
        let mut closed = false;
        for line in lines.by_ref() {
            if line.trim() == ACTION_CLOSE {
                closed = true;
                break;
            }
            body.push(line);
        }
        if !closed {
            return Err(format!("the {tool} action has no {ACTION_CLOSE} line"));
        }
        let raw = body.join("\n");
        let raw = raw.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
        let input = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_str(raw).map_err(|e| format!("the {tool} input is not JSON: {e}"))?
        };
        found.push(Action { tool, input });
    }
    match found.len() {
        0 => Err(format!("no `{ACTION_OPEN}<tool> ===` block")),
        1 => Ok(found.remove(0)),
        n => Err(format!("{n} actions in one reply; send exactly one")),
    }
}

fn parse_target(text: &str) -> Option<(String, u32)> {
    let (name, version) = text.trim().rsplit_once('@')?;
    Some((name.to_string(), version.parse().ok()?))
}

/// State of one native session.
struct Native<'a> {
    options: &'a NativeOptions,
    project: Project,
    entry: ApplicationEntry,
    contract: Contract,
    counters: Counters,
    application_runs: usize,
    baselines: BTreeMap<String, Session>,
    applied: Option<(String, edit::Applied, Vec<String>)>,
    report: NativeReport,
    /// A rollback that did not check out: the run stops, whatever the model says next.
    rollback_failure: Option<String>,
}

impl Native<'_> {
    fn capability_arg(&self, input: &Value) -> Result<(String, u32), String> {
        let text = input.get("capability").and_then(Value::as_str).ok_or("give \"capability\": \"name@version\"")?;
        parse_target(text).ok_or_else(|| format!("{text:?} is not name@version"))
    }

    fn list(&mut self) -> Result<Value, String> {
        let (manifest, envelopes) = discover(&self.options.host, &self.project.root, &self.entry)?;
        self.application_runs += 1;
        self.counters.envelopes += envelopes;
        self.contract = Contract::from_value(manifest).map_err(|e| e.to_string())?;
        let capabilities: Vec<Value> = self
            .contract
            .manifest()
            .capability_versions()
            .map(|(entry, d)| {
                json!({
                    "capability": format!("{}@{}", entry.name, d.version),
                    "kind": d.kind,
                    "effect": d.effect,
                    "description": d.description,
                    "authorization": d.authorization,
                    "builtin": entry.name.starts_with(BUILTIN_PREFIX),
                })
            })
            .collect();
        Ok(json!({ "application": self.contract.manifest().application, "capabilities": capabilities }))
    }

    fn inspect(&self, input: &Value) -> Result<Value, String> {
        let (name, version) = self.capability_arg(input)?;
        let view = self.contract.inspect(&name, version).ok_or_else(|| format!("{name}@{version} is not published"))?;
        let boundary = self.contract.replacement_boundary(&name, version);
        let source = self.project.capability(&name, version).map(|d| {
            json!({
                "declaredIn": format!("{}:{}", d.declared_in, d.line),
                "implementedIn": d.handler.as_ref().map(|h| h.module.clone()),
                "imports": d.handler.as_ref().and_then(|h| self.project.source(&h.module)).map(|s| s.imports.clone()),
            })
        });
        Ok(json!({
            "contract": serde_json::from_str::<Value>(&model_json(&view)).unwrap_or(Value::Null),
            "dependents": boundary.as_ref().map(|b| &b.dependents),
            "mustPreserve": boundary.as_ref().map(|b| &b.constraints),
            "verification": boundary.as_ref().map(|b| &b.verification),
            "source": source,
        }))
    }

    fn read(&self, input: &Value) -> Result<Value, String> {
        let path = input.get("path").and_then(Value::as_str).ok_or("give \"path\"")?;
        let path = normalize(path).ok_or_else(|| format!("{path} is not inside the project"))?;
        if self.project.source(&path).is_none() {
            return Err(format!("{path} is not a source file of this project"));
        }
        let text = self.project.read(&path).map_err(|e| e.to_string())?;
        if text.len() > READ_LIMIT {
            return Err(format!("{path} is larger than {READ_LIMIT} bytes"));
        }
        Ok(json!({ "path": path, "contents": text }))
    }

    fn invoke(&mut self, input: &Value) -> Result<Value, String> {
        let (name, version) = self.capability_arg(input)?;
        let definition = self
            .contract
            .manifest()
            .capability_version(&name, version)
            .ok_or_else(|| format!("{name}@{version} is not published"))?;
        if definition.effect != Some(CapabilityEffect::Observation) {
            return Err(format!(
                "{name}@{version} is not a declared observation; only observations may be invoked directly (consequential effects are exercised only by declared verification)"
            ));
        }
        let permissions = definition.authorization.clone();
        let call_input = input.get("input").cloned().unwrap_or_else(|| json!({}));
        let mut host = Host::start(&self.options.host, &self.project.root, &self.entry, &permissions)?;
        self.application_runs += 1;
        let capability = agent_core::appport::knowledge::CapabilityId { name: name.clone(), version };
        let reply = host.request(&capability, &call_input);
        self.counters.envelopes += host.sent;
        host.close();
        let (reply, _) = reply?;
        Ok(match reply {
            Reply::Ok { output } => json!({ "ok": true, "output": output }),
            Reply::Err { code, message } => json!({ "ok": false, "error": { "code": code, "message": message } }),
        })
    }

    fn propose(&mut self, input: &Value, text: &str) -> Result<Value, String> {
        if self.report.proposals.len() as u32 >= self.options.max_proposals {
            return Err(format!(
                "the proposal budget ({}) is spent; complete or report a limitation",
                self.options.max_proposals
            ));
        }
        if self.applied.is_some() {
            return Err("a verified change is already applied; complete or report a limitation".into());
        }
        let (name, version) = self.capability_arg(input)?;
        let target = format!("{name}@{version}");
        let mut proposal = Proposal {
            target: target.clone(),
            files: Vec::new(),
            accepted: false,
            outside_boundary: false,
            contract_changed: false,
            refusal: None,
            verdict: None,
            evidence: Vec::new(),
        };
        let intent = Intent { capability: name, version, goal: self.options.goal.clone() };
        let plan = match plan(&intent, &self.contract, &self.project) {
            Ok(plan) => plan,
            Err(refusal) => {
                proposal.refusal = Some(refusal.to_string());
                self.report.proposals.push(proposal);
                return Ok(json!({ "verified": false, "refused": refusal.to_string() }));
            }
        };
        let edits = edit::parse(text).and_then(|e| edit::targets(&e).map(|t| (e, t)));
        let (edits, targets) = match edits {
            Ok(parsed) => parsed,
            Err(problem) => {
                proposal.refusal = Some(problem.clone());
                self.report.proposals.push(proposal);
                return Ok(json!({ "verified": false, "problem": problem }));
            }
        };
        proposal.files = targets.clone();
        let editable: Vec<String> = plan.editable().iter().map(|s| s.to_string()).collect();
        let outside: Vec<&String> = targets.iter().filter(|t| !editable.contains(t)).collect();
        if !outside.is_empty() {
            proposal.outside_boundary = true;
            let problem = format!(
                "not applied: {} is outside the change boundary ({})",
                outside.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                editable.join(", ")
            );
            proposal.refusal = Some(problem.clone());
            self.report.proposals.push(proposal);
            return Ok(json!({ "verified": false, "problem": problem }));
        }
        if !self.baselines.contains_key(&target) {
            let baseline = collect(
                &self.options.host,
                &self.project.root,
                &self.entry,
                &plan,
                &self.contract,
                None,
                &mut self.counters,
            );
            self.application_runs += 1;
            if let Some(failure) = &baseline.failure {
                return Err(format!("the unchanged application could not be probed: {failure}"));
            }
            self.baselines.insert(target.clone(), baseline);
        }
        let applied = edit::apply(&self.project.root, &edits)?;
        let changed = applied.changed.clone();
        let baseline = &self.baselines[&target];
        let candidate = collect(
            &self.options.host,
            &self.project.root,
            &self.entry,
            &plan,
            &self.contract,
            Some(baseline),
            &mut self.counters,
        );
        self.application_runs += 1;
        let verdict = judge(&plan, baseline, &candidate, &changed);
        proposal.contract_changed = candidate.manifest.is_some()
            && verdict.checks.iter().any(|c| c.code == "contract.identity" && c.status == CheckStatus::Fail);
        proposal.evidence = candidate
            .evidence
            .iter()
            .map(|e| {
                let outcome = serde_json::to_value(e.outcome).ok().and_then(|v| v.as_str().map(str::to_string));
                format!("{}@{}: {}", e.capability.name, e.capability.version, outcome.unwrap_or_default())
            })
            .collect();
        proposal.accepted = verdict.accepted;
        let summary = json!({
            "verified": verdict.accepted,
            "checks": verdict.checks.iter().map(|c| json!({
                "check": c.code,
                "rule": c.rule,
                "passed": c.status == CheckStatus::Pass,
                "violations": c.violations,
            })).collect::<Vec<_>>(),
            "evidence": proposal.evidence,
        });
        proposal.verdict = Some(verdict.clone());
        if verdict.accepted {
            self.applied = Some((target, applied, changed));
        } else {
            let undone = edit::rollback(&self.project.root, applied);
            if undone.verified() {
                self.report.rollbacks += 1;
            } else {
                self.rollback_failure = Some(undone.describe());
            }
        }
        self.report.proposals.push(proposal);
        Ok(summary)
    }

    fn undo(&mut self) {
        if let Some((_, applied, _)) = self.applied.take() {
            let undone = edit::rollback(&self.project.root, applied);
            if undone.verified() {
                self.report.rollbacks += 1;
            } else {
                self.rollback_failure = Some(undone.describe());
            }
        }
    }
}

/// Runs the native workflow on `options.project` (a copy; it is modified).
pub fn run_native(options: &NativeOptions) -> Result<NativeReport, String> {
    let mut log = Log::new(options.log.as_deref(), options.quiet).map_err(|e| e.to_string())?;
    let mut model: Box<dyn Model> = connect(&options.model).map_err(|e| e.to_string())?;
    let project = observe(&options.project).map_err(|e| format!("{}: {e}", options.project.display()))?;
    let entry = project.application.clone().ok_or("no AppPort application entry point was found")?;
    // The harness reads the contract once to plan proposals; the model sees only what tools return.
    let (manifest, envelopes) = discover(&options.host, &project.root, &entry)?;
    let contract = Contract::from_value(manifest).map_err(|e| e.to_string())?;
    let mut state = Native {
        options,
        project,
        entry,
        contract,
        counters: Counters { envelopes },
        application_runs: 1,
        baselines: BTreeMap::new(),
        applied: None,
        rollback_failure: None,
        report: NativeReport {
            outcome: "incomplete".into(),
            reason: None,
            model: model.describe(),
            turns: Vec::new(),
            tool_calls: BTreeMap::new(),
            proposals: Vec::new(),
            refused_completions: 0,
            model_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            tokens_estimated: false,
            discovery_bytes: 0,
            tool_result_bytes: 0,
            application_runs: 0,
            envelopes: 0,
            rollbacks: 0,
            accepted_files: Vec::new(),
            wall_ms: 0,
            started_at: now_rfc3339(),
        },
    };

    let system = system_prompt();
    let mut history: Vec<Turn> = Vec::new();
    let mut prompt = first_message(&options.goal, options.target.as_deref());
    let mut finished = false;
    for turn in 1..=options.max_turns.max(1) {
        let began = Instant::now();
        let request = Request {
            system: system.clone(),
            history: history.clone(),
            prompt: prompt.clone(),
            max_tokens: options.model.max_tokens,
        };
        log.event("model", json!({ "turn": turn, "prompt": &prompt }), format!("turn {turn}"));
        let response = model.complete(&request).map_err(|e| e.to_string())?;
        log.event("model", json!({ "turn": turn, "response": &response }), "");
        history.push(Turn { role: Role::User, text: prompt.clone(), raw: None });
        history.push(Turn { role: Role::Assistant, text: response.text.clone(), raw: response.raw.clone() });

        let (action, result): (String, Result<Value, String>) = match parse_action(&response.text) {
            Err(problem) => ("unusable".into(), Err(format!("reply not understood: {problem}"))),
            Ok(action) => {
                let tool = action.tool.clone();
                let result = match tool.as_str() {
                    "list_capabilities" => state.list(),
                    "inspect" => state.inspect(&action.input),
                    "read" => state.read(&action.input),
                    "invoke" => state.invoke(&action.input),
                    "propose" => state.propose(&action.input, &response.text),
                    "complete" => {
                        if let Some((_, _, changed)) = &state.applied {
                            state.report.outcome = "accepted".into();
                            state.report.reason =
                                action.input.get("summary").and_then(Value::as_str).map(str::to_string);
                            state.report.accepted_files = changed.clone();
                            finished = true;
                            Ok(json!({ "accepted": true }))
                        } else {
                            state.report.refused_completions += 1;
                            Err("refused: no verified proposal is applied".into())
                        }
                    }
                    "limitation" => {
                        state.undo();
                        state.report.outcome = "limitation".into();
                        state.report.reason = action.input.get("reason").and_then(Value::as_str).map(str::to_string);
                        finished = true;
                        Ok(json!({ "recorded": true }))
                    }
                    other => Err(format!("unknown tool {other:?}")),
                };
                (tool, result)
            }
        };
        *state.report.tool_calls.entry(action.clone()).or_insert(0) += 1;
        let result_text = match &result {
            Ok(value) => serde_json::to_string_pretty(value).unwrap_or_default(),
            Err(problem) => json!({ "error": problem }).to_string(),
        };
        if matches!(action.as_str(), "list_capabilities" | "inspect") {
            state.report.discovery_bytes += result_text.len();
        }
        state.report.tool_result_bytes += result_text.len();
        log.event(
            "tool",
            json!({ "turn": turn, "action": &action, "result": &result_text }),
            format!(
                "turn {turn}: {action} → {} bytes{}",
                result_text.len(),
                if result.is_err() { " (error)" } else { "" }
            ),
        );
        state.report.turns.push(NativeTurn {
            turn,
            usage: response.usage,
            stop_reason: response.stop_reason.clone(),
            action: action.clone(),
            result_bytes: result_text.len(),
            duration_ms: began.elapsed().as_millis() as u64,
        });
        if finished || state.rollback_failure.is_some() {
            break;
        }
        prompt = format!("Result of {action}:\n```json\n{result_text}\n```");
    }
    if !finished {
        state.undo();
    }
    if let Some(problem) = state.rollback_failure.take() {
        state.report.outcome = "rollback-failed".into();
        state.report.reason = Some(format!("a change could not be undone and checked: {problem}"));
    }
    let mut report = state.report;
    report.model_calls = report.turns.len();
    report.input_tokens = report.turns.iter().map(|t| t.usage.input_tokens).sum();
    report.output_tokens = report.turns.iter().map(|t| t.usage.output_tokens).sum();
    report.tokens_estimated = report.turns.iter().any(|t| t.usage.estimated);
    report.application_runs = state.application_runs;
    report.envelopes = state.counters.envelopes;
    report.wall_ms = log.elapsed_ms() as u64;
    log.event(
        "done",
        json!({ "report": &report }),
        format!("{}: {} model call(s)", report.outcome.to_uppercase(), report.model_calls),
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exactly_one_action() {
        let a = parse_action("thinking...\n=== ACTION: inspect ===\n{\"capability\": \"a.b@1\"}\n=== END ACTION ===")
            .unwrap();
        assert_eq!((a.tool.as_str(), a.input["capability"].as_str()), ("inspect", Some("a.b@1")));
        assert_eq!(parse_action("=== ACTION: list_capabilities ===\n=== END ACTION ===").unwrap().input, json!({}));
        assert!(parse_action("no action").is_err());
        assert!(parse_action("=== ACTION: a ===\n{}\n=== END ACTION ===\n=== ACTION: b ===\n{}\n=== END ACTION ===")
            .is_err());
        assert!(parse_action("=== ACTION: a ===\nnot json\n=== END ACTION ===").is_err());
    }

    #[test]
    fn parses_targets() {
        assert_eq!(parse_target("customers.lookup@1"), Some(("customers.lookup".into(), 1)));
        assert_eq!(parse_target("customers.lookup"), None);
    }
}
