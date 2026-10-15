//! Deterministic verification: what was observed, and whether it is acceptable.
//!
//! The agent never asks a model whether a change is correct. It runs the
//! application, records what the runtime answered ([`Session`]), evaluates the
//! contract's declared verification methods against those answers
//! ([`evaluate_state`], [`evaluate_event`]), and accepts a change only when every
//! check the plan derived from the contract passes ([`judge`]).
//!
//! Evaluation semantics (the packages publish the evidence shape, not an evaluator):
//! - a **state** method calls its probe with resolved `$input`/`$output` references
//!   and compares each expectation. Any disagreement is `contradicted`; a probe
//!   that fails is `unavailable` with its error code; an unresolvable reference is
//!   `not-run`.
//! - an **event** method is `confirmed` when a matching event was delivered. An
//!   absent event proves nothing: it is `unavailable`, never `contradicted`.
//! - the outcome is `unverified` if any method is contradicted, `verified` if a
//!   state method confirmed, otherwise `unverifiable`.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::appport::knowledge::{
    rules, CapabilityId, Contract, EventVerification, MethodEvidence, MethodKind, MethodStatus, Mismatch, ProbeRecord,
    StateVerification, ValueRef, VerificationOutcome,
};
use crate::conformance::{self, at_pointer, json_eq};
use crate::plan::{CheckSpec, Plan};

/// What the runtime answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "lowercase")]
pub enum Reply {
    Ok { output: Value },
    Err { code: String, message: String },
}

impl Reply {
    pub fn output(&self) -> Option<&Value> {
        match self {
            Reply::Ok { output } => Some(output),
            Reply::Err { .. } => None,
        }
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            Reply::Ok { .. } => None,
            Reply::Err { code, .. } => Some(code),
        }
    }

    /// What is compared for regression: the output, or the error code (messages may change).
    fn comparable(&self) -> Value {
        match self {
            Reply::Ok { output } => serde_json::json!({ "output": output }),
            Reply::Err { code, .. } => serde_json::json!({ "error": code }),
        }
    }
}

/// Why a call was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Enumerate,
    Regression,
    Flow,
    Probe,
}

/// One request and its answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub phase: Phase,
    pub capability: CapabilityId,
    pub input: Value,
    pub reply: Reply,
}

/// Everything observed in one run of the application.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// Why the application could not be run, when it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// The manifest the runtime published (`appport.manifest@1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Value>,
    pub calls: Vec<Call>,
    pub evidence: Vec<crate::appport::knowledge::VerificationEvidence>,
}

impl Session {
    pub fn calls_to<'a>(&'a self, c: &'a CapabilityId, phase: Phase) -> impl Iterator<Item = &'a Call> + 'a {
        self.calls.iter().filter(move |call| call.phase == phase && call.capability == *c)
    }
}

/// Resolves a `ValueRef` against a request's input and response output.
pub fn resolve(reference: &ValueRef, input: &Value, output: Option<&Value>) -> Option<Value> {
    match reference {
        ValueRef::Input(r) => at_pointer(input, &r.pointer).cloned(),
        ValueRef::Output(r) => at_pointer(output?, &r.pointer).cloned(),
        ValueRef::Literal(v) => Some(v.clone()),
    }
}

/// Resolves a probe input map. `None` when any reference cannot be resolved.
pub fn resolve_input(input_map: &IndexMap<String, ValueRef>, input: &Value, output: Option<&Value>) -> Option<Value> {
    let mut object = serde_json::Map::new();
    for (key, reference) in input_map {
        object.insert(key.clone(), resolve(reference, input, output)?);
    }
    Some(Value::Object(object))
}

/// Evaluates a state method given what its probe answered (`None`: not run).
pub fn evaluate_state(
    method: &StateVerification,
    input: &Value,
    output: &Value,
    probe: Option<(&Reply, Option<String>)>,
) -> MethodEvidence {
    let record = |request_id: Option<String>| ProbeRecord {
        capability: method.via.capability.clone(),
        version: method.via.version,
        request_id,
    };
    let mut evidence = MethodEvidence {
        id: method.id.clone(),
        kind: MethodKind::State,
        status: MethodStatus::NotRun,
        probe: None,
        mismatches: None,
        error_code: None,
    };
    let Some((reply, request_id)) = probe else { return evidence };
    evidence.probe = Some(record(request_id));
    let observed = match reply {
        Reply::Err { code, .. } => {
            evidence.status = MethodStatus::Unavailable;
            evidence.error_code = Some(code.clone());
            return evidence;
        }
        Reply::Ok { output } => output,
    };
    let mut mismatches = Vec::new();
    for expectation in &method.expect {
        let actual = at_pointer(observed, &expectation.at);
        if let Some(expected) = &expectation.equals {
            match resolve(expected, input, Some(output)) {
                Some(expected) if actual.is_some_and(|a| json_eq(a, &expected)) => {}
                Some(expected) => {
                    mismatches.push(Mismatch { at: expectation.at.clone(), expected, observed: actual.cloned() })
                }
                None => mismatches.push(Mismatch {
                    at: expectation.at.clone(),
                    expected: serde_json::to_value(expected).unwrap_or(Value::Null),
                    observed: actual.cloned(),
                }),
            }
        } else if expectation.exists.is_some() && actual.is_none() {
            mismatches.push(Mismatch {
                at: expectation.at.clone(),
                expected: serde_json::json!({ "exists": true }),
                observed: None,
            });
        }
    }
    if mismatches.is_empty() {
        evidence.status = MethodStatus::Confirmed;
    } else {
        evidence.status = MethodStatus::Contradicted;
        evidence.mismatches = Some(mismatches);
    }
    evidence
}

/// Evaluates an event method against the payloads of the delivered events of its name.
pub fn evaluate_event(method: &EventVerification, input: &Value, output: &Value, payloads: &[Value]) -> MethodEvidence {
    let matched = payloads.iter().any(|payload| {
        method.matches.iter().all(|(pointer, expected)| {
            resolve(expected, input, Some(output))
                .is_some_and(|expected| at_pointer(payload, pointer).is_some_and(|a| json_eq(a, &expected)))
        })
    });
    MethodEvidence {
        id: method.id.clone(),
        kind: MethodKind::Event,
        status: if matched { MethodStatus::Confirmed } else { MethodStatus::Unavailable },
        probe: None,
        mismatches: None,
        error_code: None,
    }
}

/// The outcome of a set of method results.
pub fn outcome(methods: &[MethodEvidence]) -> VerificationOutcome {
    if methods.iter().any(|m| m.status == MethodStatus::Contradicted) {
        VerificationOutcome::Unverified
    } else if methods.iter().any(|m| m.kind == MethodKind::State && m.status == MethodStatus::Confirmed) {
        VerificationOutcome::Verified
    } else {
        VerificationOutcome::Unverifiable
    }
}

// ---------------------------------------------------------------------------
// Judgement
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckStatus {
    Pass,
    Fail,
}

/// The result of one acceptance criterion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub code: String,
    pub rule: String,
    pub status: CheckStatus,
    /// Every violation found. Empty when the check passes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub violations: Vec<String>,
    /// What was observed that does not fail the check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Accept or reject, with the reasons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub accepted: bool,
    pub checks: Vec<CheckResult>,
}

impl Verdict {
    /// Violations to hand back as tightened constraints on a retry.
    pub fn feedback(&self) -> Vec<String> {
        self.checks
            .iter()
            .filter(|c| c.status == CheckStatus::Fail)
            .flat_map(|c| c.violations.iter().map(move |d| format!("[{}] {d}", c.code)))
            .collect()
    }

    pub fn failed(&self) -> Vec<&str> {
        self.checks.iter().filter(|c| c.status == CheckStatus::Fail).map(|c| c.code.as_str()).collect()
    }
}

/// Judges a candidate against the baseline. `changed` lists the files the change touched.
pub fn judge(plan: &Plan, baseline: &Session, candidate: &Session, changed: &[String]) -> Verdict {
    let checks = plan.checks.iter().map(|spec| run_check(spec, plan, baseline, candidate, changed)).collect::<Vec<_>>();
    // A check the plan did not list cannot pass a change; every listed check must pass.
    Verdict { accepted: checks.iter().all(|c| c.status == CheckStatus::Pass), checks }
}

fn key(c: &CapabilityId) -> String {
    rules::integration_key(&c.name, c.version)
}

#[derive(Default)]
struct Tally {
    failed: bool,
    violations: Vec<String>,
    notes: Vec<String>,
}

impl Tally {
    fn fail(&mut self, violation: String) {
        self.failed = true;
        self.violations.push(violation);
    }

    fn note(&mut self, note: String) {
        self.notes.push(note);
    }

    fn finish(self, spec: &CheckSpec) -> CheckResult {
        CheckResult {
            code: spec.code.clone(),
            rule: spec.rule.clone(),
            status: if self.failed { CheckStatus::Fail } else { CheckStatus::Pass },
            violations: self.violations,
            notes: self.notes,
        }
    }
}

fn run_check(
    spec: &CheckSpec,
    plan: &Plan,
    baseline: &Session,
    candidate: &Session,
    changed: &[String],
) -> CheckResult {
    let mut t = Tally::default();

    // Nothing else is meaningful if the application did not run.
    if spec.code != "edit.boundary" {
        if let Some(failure) = &candidate.failure {
            t.fail(format!("the application did not run: {failure}"));
            return t.finish(spec);
        }
    }

    match spec.code.as_str() {
        "edit.boundary" => {
            let allowed: BTreeSet<&str> = plan.editable().into_iter().collect();
            for path in changed {
                if !allowed.contains(path.as_str()) {
                    t.fail(format!(
                        "{path} is outside the change boundary (allowed: {})",
                        allowed.iter().copied().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
            if changed.is_empty() {
                t.fail("the change touched no file".into());
            }
        }
        "runtime.load" => {
            if candidate.manifest.is_none() {
                t.fail("appport.manifest@1 returned no manifest".into());
            }
        }
        "contract.identity" => match candidate.manifest.as_ref().map(|m| Contract::from_value(m.clone())) {
            Some(Ok(contract)) => {
                let digest = contract.digest();
                if digest != plan.contract_digest {
                    t.fail(format!("contractDigest changed: {} → {digest}", plan.contract_digest));
                }
                match contract.replacement_boundary(&plan.target.name, plan.target.version) {
                    Some(boundary) if boundary.contract_pin == plan.boundary.contract_pin => {}
                    Some(boundary) => t.fail(format!(
                        "{}'s contract pin changed: {} → {}",
                        key(&plan.target),
                        plan.boundary.contract_pin,
                        boundary.contract_pin
                    )),
                    None => t.fail(format!("{} is no longer published", key(&plan.target))),
                }
                if !t.failed {
                    t.note(format!("contractDigest {digest}"));
                }
            }
            Some(Err(e)) => t.fail(format!("the published manifest no longer parses: {e}")),
            None => t.fail("no manifest was published".into()),
        },
        "contract.valid" => match candidate.manifest.as_ref().map(|m| Contract::from_value(m.clone())) {
            Some(Ok(contract)) => {
                for issue in contract.validate().into_iter().filter(|i| i.is_error()) {
                    t.fail(format!("{} at {}: {}", issue.rule, issue.path, issue.message));
                }
            }
            _ => t.fail("no parseable manifest was published".into()),
        },
        "output.conformance" => {
            let contract = candidate.manifest.as_ref().and_then(|m| Contract::from_value(m.clone()).ok());
            let mut checked = 0;
            for call in &candidate.calls {
                let Some(output) = call.reply.output() else { continue };
                let Some(definition) = contract
                    .as_ref()
                    .and_then(|c| c.manifest().capability_version(&call.capability.name, call.capability.version))
                else {
                    continue;
                };
                checked += 1;
                for violation in conformance::check(&definition.output_schema, output) {
                    t.fail(format!(
                        "{} output for input {}: {} at {:?}: {}",
                        key(&call.capability),
                        call.input,
                        violation.keyword,
                        violation.at,
                        violation.message
                    ));
                }
            }
            if !t.failed {
                t.note(format!("{checked} output(s) conform"));
            }
        }
        "failures.declared" => {
            let declared: BTreeSet<&str> = plan.view.failures.iter().map(|f| f.code.as_str()).collect();
            let before: BTreeSet<&str> =
                baseline.calls.iter().filter(|c| c.capability == plan.target).filter_map(|c| c.reply.code()).collect();
            let allowed = if declared.is_empty() { &before } else { &declared };
            for call in candidate.calls.iter().filter(|c| c.capability == plan.target) {
                if let Some(code) = call.reply.code() {
                    if !allowed.contains(code) {
                        let message = match &call.reply {
                            Reply::Err { message, .. } => message.as_str(),
                            Reply::Ok { .. } => "",
                        };
                        t.fail(format!(
                            "{} answered {} with {code} ({message}); allowed: {}",
                            key(&plan.target),
                            call.input,
                            allowed.iter().copied().collect::<Vec<_>>().join(", ")
                        ));
                    }
                }
            }
        }
        "behavior.regression" => {
            let mut compared = 0;
            for c in &plan.probes.regression {
                let before: BTreeMap<String, &Call> = baseline
                    .calls_to(c, Phase::Regression)
                    .map(|call| (rules::canonicalize(&call.input), call))
                    .collect();
                let after: BTreeMap<String, &Call> = candidate
                    .calls_to(c, Phase::Regression)
                    .map(|call| (rules::canonicalize(&call.input), call))
                    .collect();
                for (input, old) in &before {
                    compared += 1;
                    match after.get(input) {
                        None => t.fail(format!("{} was not probed with {input} after the change", key(c))),
                        Some(new) if old.reply.comparable() != new.reply.comparable() => t.fail(format!(
                            "{} {input}: was {}, now {}",
                            key(c),
                            old.reply.comparable(),
                            new.reply.comparable()
                        )),
                        Some(_) => {}
                    }
                }
            }
            if compared == 0 {
                t.fail("no regression probe ran in the baseline".into());
            } else if !t.failed {
                t.note(format!("{compared} answer(s) on existing state unchanged"));
            }
        }
        "verification" => {
            let Some(flow) = plan.probes.flows.iter().find(|f| spec.subject.as_ref() == Some(&f.capability)) else {
                t.fail("no flow for this check".into());
                return t.finish(spec);
            };
            let records: Vec<_> = candidate.evidence.iter().filter(|e| e.capability == flow.capability).collect();
            if records.is_empty() {
                t.fail(format!(
                    "{} did not run or was refused: {}",
                    key(&flow.capability),
                    flow_failure(candidate, &flow.capability)
                ));
            }
            for record in records {
                for method in &record.methods {
                    let status = serde_json::to_value(method.status).unwrap_or_default();
                    let status = status.as_str().unwrap_or_default();
                    match (method.kind, method.status) {
                        (_, MethodStatus::Contradicted) => {
                            let mismatches = method
                                .mismatches
                                .iter()
                                .flatten()
                                .map(|m| {
                                    format!(
                                        "at {} expected {} observed {}",
                                        m.at,
                                        m.expected,
                                        m.observed.as_ref().map_or("nothing".to_string(), Value::to_string)
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("; ");
                            t.fail(format!("{}#{} contradicted: {mismatches}", key(&flow.capability), method.id))
                        }
                        (MethodKind::State, MethodStatus::Confirmed) => {
                            t.note(format!("{}#{} confirmed (establishes)", key(&flow.capability), method.id))
                        }
                        (MethodKind::State, _) => t.fail(format!(
                            "{}#{} {status}{}: a state method must confirm",
                            key(&flow.capability),
                            method.id,
                            method.error_code.as_ref().map(|c| format!(" ({c})")).unwrap_or_default()
                        )),
                        (MethodKind::Event, MethodStatus::Confirmed) => {
                            t.note(format!("{}#{} confirmed (corroborates)", key(&flow.capability), method.id))
                        }
                        (MethodKind::Event, _) => t.note(format!(
                            "{}#{} {status} (an absent event proves nothing)",
                            key(&flow.capability),
                            method.id
                        )),
                    }
                }
            }
        }
        other => t.fail(format!("unknown check {other}")),
    }
    t.finish(spec)
}

fn flow_failure(session: &Session, c: &CapabilityId) -> String {
    session
        .calls_to(c, Phase::Flow)
        .filter_map(|call| match &call.reply {
            Reply::Err { code, message } => Some(format!("{code}: {message}")),
            Reply::Ok { .. } => None,
        })
        .next()
        .unwrap_or_else(|| "no call was recorded".into())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn state() -> StateVerification {
        serde_json::from_value(json!({
            "id": "readable",
            "kind": "state",
            "via": { "capability": "c.get", "version": 1, "input": { "id": { "$output": "/id" } } },
            "expect": [{ "at": "/email", "equals": { "$input": "/email" } }, { "at": "/since", "exists": true }]
        }))
        .unwrap()
    }

    #[test]
    fn state_methods_confirm_contradict_or_are_unavailable() {
        let method = state();
        let input = json!({ "email": "a@x.io" });
        let output = json!({ "id": "c1" });
        assert_eq!(resolve_input(&method.via.input, &input, Some(&output)), Some(json!({ "id": "c1" })));

        let good = Reply::Ok { output: json!({ "email": "a@x.io", "since": "2020" }) };
        assert_eq!(evaluate_state(&method, &input, &output, Some((&good, None))).status, MethodStatus::Confirmed);

        let bad = Reply::Ok { output: json!({ "email": "b@x.io" }) };
        let evidence = evaluate_state(&method, &input, &output, Some((&bad, Some("r1".into()))));
        assert_eq!(evidence.status, MethodStatus::Contradicted);
        assert_eq!(evidence.mismatches.as_ref().unwrap().len(), 2);
        assert_eq!(evidence.probe.unwrap().request_id.as_deref(), Some("r1"));

        let missing = Reply::Err { code: "NOT_FOUND".into(), message: "no".into() };
        let evidence = evaluate_state(&method, &input, &output, Some((&missing, None)));
        assert_eq!((evidence.status, evidence.error_code.as_deref()), (MethodStatus::Unavailable, Some("NOT_FOUND")));
        assert_eq!(evaluate_state(&method, &input, &output, None).status, MethodStatus::NotRun);
    }

    #[test]
    fn event_absence_is_never_a_contradiction() {
        let method: EventVerification = serde_json::from_value(json!({
            "id": "evt", "kind": "event", "event": "c.created",
            "match": { "/id": { "$output": "/id" } }, "strength": "corroborating"
        }))
        .unwrap();
        let output = json!({ "id": "c1" });
        assert_eq!(
            evaluate_event(&method, &json!({}), &output, &[json!({ "id": "c1" })]).status,
            MethodStatus::Confirmed
        );
        let absent = evaluate_event(&method, &json!({}), &output, &[json!({ "id": "c2" })]);
        assert_eq!(absent.status, MethodStatus::Unavailable);
        assert_eq!(outcome(&[absent]), VerificationOutcome::Unverifiable);
    }
}
