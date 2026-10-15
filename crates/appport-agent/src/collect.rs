//! Collects evidence by running the application: the probe plan, executed.
//!
//! The same plan runs before the change (the baseline) and after it (the
//! candidate). The candidate replays the baseline's regression inputs, so the
//! two are compared request for request. Each run is a fresh process, so state
//! a consequential call creates in one run never leaks into another.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use agent_core::appport::knowledge::{
    fields, CapabilityId, Contract, SchemaType, VerificationEvidence, VerificationEvidenceType, VerificationMethod,
};
use agent_core::conformance::{conforms, synthesize};
use agent_core::observe::ApplicationEntry;
use agent_core::plan::{harvest_concepts, Plan};
use agent_core::verify::{evaluate_event, evaluate_state, outcome, resolve_input, Call, Phase, Reply, Session};
use serde_json::{json, Value};

use crate::host::{Host, HostConfig};

/// Most existing records probed per capability.
const MAX_EXISTING: usize = 5;

/// The capability every AppPort runtime serves its manifest from.
pub fn manifest_capability() -> CapabilityId {
    CapabilityId { name: "appport.manifest".into(), version: 1 }
}

/// Starts the application with no permissions and reads its manifest.
pub fn discover(config: &HostConfig, root: &Path, entry: &ApplicationEntry) -> Result<(Value, usize), String> {
    let mut host = Host::start(config, root, entry, &[])?;
    let reply = host.request(&manifest_capability(), &json!({}));
    let sent = host.sent;
    host.close();
    match reply? {
        (Reply::Ok { output }, _) => Ok((output, sent)),
        (Reply::Err { code, message }, _) => Err(format!("appport.manifest@1 failed: {code}: {message}")),
    }
}

/// Counters for the run report.
#[derive(Debug, Default, Clone, Copy)]
pub struct Counters {
    pub envelopes: usize,
}

/// Runs the plan against the application. `replay` is the baseline whose
/// regression inputs are repeated; `None` derives them (the baseline itself).
pub fn collect(
    config: &HostConfig,
    root: &Path,
    entry: &ApplicationEntry,
    plan: &Plan,
    contract: &Contract,
    replay: Option<&Session>,
    counters: &mut Counters,
) -> Session {
    let mut session = Session::default();
    let mut host = match Host::start(config, root, entry, &plan.permissions) {
        Ok(host) => host,
        Err(e) => {
            session.failure = Some(e);
            return session;
        }
    };
    if let Err(e) = run(&mut host, plan, contract, replay, &mut session) {
        session.failure = Some(e);
    }
    counters.envelopes += host.sent;
    host.close();
    session
}

fn call(
    host: &mut Host,
    session: &mut Session,
    phase: Phase,
    c: &CapabilityId,
    input: Value,
) -> Result<(Reply, String), String> {
    let (reply, request_id) = host.request(c, &input)?;
    session.calls.push(Call { phase, capability: c.clone(), input, reply: reply.clone() });
    Ok((reply, request_id))
}

fn run(
    host: &mut Host,
    plan: &Plan,
    contract: &Contract,
    replay: Option<&Session>,
    session: &mut Session,
) -> Result<(), String> {
    match host.request(&manifest_capability(), &json!({}))? {
        (Reply::Ok { output }, _) => session.manifest = Some(output),
        (Reply::Err { code, message }, _) => return Err(format!("appport.manifest@1 failed: {code}: {message}")),
    }
    let manifest = contract.manifest();

    // 1. Discover existing state, by concept.
    let mut pool: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for c in &plan.probes.enumerators {
        let Some(definition) = manifest.capability_version(&c.name, c.version) else { continue };
        let input = synthesize(&definition.input_schema, 0, &BTreeMap::new());
        if let (Reply::Ok { output }, _) = call(host, session, Phase::Enumerate, c, input)? {
            for (concept, values) in harvest_concepts(&definition.output_schema, &output) {
                let bucket = pool.entry(concept).or_default();
                for value in values {
                    if !bucket.contains(&value) {
                        bucket.push(value);
                    }
                }
            }
        }
    }

    // 2. Probe observations on existing state (before anything changes it).
    let inputs: Vec<(CapabilityId, Value)> = match replay {
        Some(baseline) => baseline
            .calls
            .iter()
            .filter(|c| c.phase == Phase::Regression)
            .map(|c| (c.capability.clone(), c.input.clone()))
            .collect(),
        None => plan
            .probes
            .regression
            .iter()
            .flat_map(|c| regression_inputs(contract, c, &pool).into_iter().map(move |input| (c.clone(), input)))
            .collect(),
    };
    for (c, input) in inputs {
        call(host, session, Phase::Regression, &c, input)?;
    }

    // 3. Run each flow and evaluate its declared verification methods.
    let integration = contract.integration();
    let mut subscribed = BTreeSet::new();
    for (index, flow) in plan.probes.flows.iter().enumerate() {
        let c = &flow.capability;
        let Some(definition) = manifest.capability_version(&c.name, c.version) else { continue };
        let methods: Vec<VerificationMethod> = integration
            .and_then(|i| i.capability(&c.name, c.version))
            .and_then(|e| e.verify.clone())
            .unwrap_or_default();
        for method in &methods {
            if let VerificationMethod::Event(e) = method {
                if subscribed.insert(e.event.clone()) {
                    host.subscribe(&e.event)?;
                }
            }
        }
        let input = synthesize(&definition.input_schema, index as u64 + 1, &BTreeMap::new());
        if !conforms(&definition.input_schema, &input) {
            // The contract's input cannot be synthesized (e.g. a pattern); the
            // flow is recorded as not run rather than guessed at.
            session.calls.push(Call {
                phase: Phase::Flow,
                capability: c.clone(),
                input,
                reply: Reply::Err {
                    code: "AGENT_INPUT".into(),
                    message: "no conforming input could be synthesized".into(),
                },
            });
            continue;
        }
        let (reply, request_id) = call(host, session, Phase::Flow, c, input.clone())?;
        let Reply::Ok { output } = reply else { continue };

        let mut results = Vec::new();
        for method in &methods {
            match method {
                VerificationMethod::State(state) => {
                    let probe = CapabilityId { name: state.via.capability.clone(), version: state.via.version };
                    let probed = match resolve_input(&state.via.input, &input, Some(&output)) {
                        Some(probe_input) => Some(call(host, session, Phase::Probe, &probe, probe_input)?),
                        None => None,
                    };
                    results.push(evaluate_state(
                        state,
                        &input,
                        &output,
                        probed.as_ref().map(|(r, id)| (r, Some(id.clone()))),
                    ));
                }
                VerificationMethod::Event(event) => {
                    let payloads = host.events_named(&event.event)?;
                    results.push(evaluate_event(event, &input, &output, &payloads));
                }
            }
        }
        session.evidence.push(VerificationEvidence {
            kind: VerificationEvidenceType,
            application: plan.application.clone(),
            capability: c.clone(),
            request_id: Some(request_id),
            checked_at: crate::report::now_rfc3339(),
            outcome: outcome(&results),
            methods: results,
        });
    }
    Ok(())
}

/// Inputs for probing an observation on existing state: each known value of each
/// concept-annotated input field, and one value no record has.
fn regression_inputs(contract: &Contract, c: &CapabilityId, pool: &BTreeMap<String, Vec<Value>>) -> Vec<Value> {
    let Some(definition) = contract.manifest().capability_version(&c.name, c.version) else { return Vec::new() };
    let schema = &definition.input_schema;
    let mut inputs: Vec<Value> = Vec::new();
    let mut push = |value: Value| {
        if conforms(schema, &value) && !inputs.contains(&value) {
            inputs.push(value);
        }
    };
    let keyed: Vec<_> =
        fields(schema).into_iter().filter(|f| f.required && f.concept.is_some() && !f.pointer.contains('*')).collect();
    for field in &keyed {
        let known = pool.get(field.concept.as_deref().unwrap_or_default()).cloned().unwrap_or_default();
        for value in known.into_iter().take(MAX_EXISTING) {
            push(synthesize(schema, 0, &BTreeMap::from([(field.pointer.clone(), value)])));
        }
        if field.types.contains(&SchemaType::String) {
            let absent = Value::String(format!("absent-{}-{}", c.name, c.version));
            push(synthesize(schema, 0, &BTreeMap::from([(field.pointer.clone(), absent)])));
        }
    }
    if keyed.is_empty() {
        push(synthesize(schema, 0, &BTreeMap::new()));
    }
    inputs
}
