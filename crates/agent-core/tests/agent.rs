//! The deterministic core of the loop, offline: observation, planning and
//! judgement against the sample project and the manifest its runtime publishes
//! (`tests/fixtures/customers.manifest.json`; the agent's e2e tests check it
//! still matches the running application).

mod common;

use std::path::PathBuf;

use agent_core::appport::knowledge::{model_json, CapabilityId, Contract};
use agent_core::observe::{observe, EntryKind, Project};
use agent_core::plan::{model_task, plan, Intent, Plan};
use agent_core::verify::{judge, Call, CheckStatus, Phase, Reply, Session};
use common::{fixture, snapshot};
use serde_json::{json, Value};

const INTENT: &str = "replace customers.lookup@1: read customers from the application's customer store";

fn project() -> Project {
    observe(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/customers-app")).unwrap()
}

fn contract() -> Contract {
    Contract::from_value(fixture("customers.manifest.json")).unwrap()
}

fn planned() -> Plan {
    plan(&Intent::parse(INTENT).unwrap(), &contract(), &project()).unwrap()
}

fn id(name: &str) -> CapabilityId {
    CapabilityId { name: name.into(), version: 1 }
}

#[test]
fn observes_the_project_on_disk() {
    let project = project();
    let entry = project.application.as_ref().unwrap();
    assert_eq!(
        (entry.module.as_str(), entry.export.as_str(), entry.kind),
        ("src/app.mjs", "createCustomersApplication", EntryKind::Factory)
    );
    let located: Vec<String> = project
        .capabilities
        .iter()
        .map(|c| format!("{}@{} → {}", c.name, c.version, c.handler.as_ref().unwrap().module))
        .collect();
    assert_eq!(
        located,
        [
            "customers.register@1 → src/handlers/customers.register.mjs",
            "customers.lookup@1 → src/handlers/customers.lookup.mjs",
            "customers.list@1 → src/handlers/customers.list.mjs",
            "billing.contact@1 → src/handlers/billing.contact.mjs",
        ]
    );
    assert!(project.findings.is_empty(), "{:?}", project.findings);
    let reaching: Vec<&str> =
        project.capabilities_reaching("src/handlers/customers.lookup.mjs").iter().map(|c| c.name.as_str()).collect();
    assert_eq!(reaching, ["customers.lookup", "billing.contact"]);
}

#[test]
fn plans_the_replacement_without_a_model() {
    let plan = planned();
    let summary = json!({
        "target": plan.target,
        "editable": plan.editable(),
        "context": plan.context,
        "permissions": plan.permissions,
        "probes": plan.probes,
        "checks": plan.checks,
        "decisions": plan.decisions,
    });
    snapshot("customers.lookup@1.plan.json", &(serde_json::to_string_pretty(&summary).unwrap() + "\n"));
}

#[test]
fn the_model_task_carries_the_contract_and_only_the_planned_files() {
    let plan = planned();
    let project = project();
    let files = plan.context.iter().map(|f| (f.path.clone(), project.read(&f.path).unwrap())).collect();
    let task = model_task(&plan, &files, &["[failures.declared] answered with INTERNAL_ERROR".into()]);
    assert!(task.prompt.contains(&model_json(&plan.view)));
    assert!(task.prompt.contains("## src/handlers/customers.lookup.mjs (EDITABLE"));
    assert!(task.prompt.contains("## src/store.mjs (read-only"));
    assert!(!task.prompt.contains("src/contract.mjs ("), "the contract is given as knowledge, not as a file to edit");
    assert!(!task.prompt.contains("billing.contact.mjs ("), "files the change does not need are not sent");
    assert!(task.prompt.contains("The previous attempt was rejected"));
}

#[test]
fn refuses_what_it_cannot_ground() {
    let refuse = |intent: &str| plan(&Intent::parse(intent).unwrap(), &contract(), &project()).unwrap_err().code;
    assert_eq!(refuse("replace customers.missing@1"), "target.undeclared");
    assert_eq!(refuse("replace customers.lookup@2"), "target.undeclared");
    assert_eq!(refuse("replace appport.ping@1"), "target.builtin");
}

fn reply(output: Value) -> Reply {
    Reply::Ok { output }
}

fn session(manifest: Value, lookup: Reply) -> Session {
    Session {
        failure: None,
        manifest: Some(manifest),
        calls: vec![
            Call {
                phase: Phase::Regression,
                capability: id("customers.lookup"),
                input: json!({ "customerId": "cus_1001" }),
                reply: lookup,
            },
            Call {
                phase: Phase::Regression,
                capability: id("billing.contact"),
                input: json!({ "customerId": "cus_1001" }),
                reply: reply(json!({ "customerId": "cus_1001", "name": "Ada Lovelace", "email": "ada@example.com" })),
            },
        ],
        evidence: Vec::new(),
    }
}

fn ada() -> Value {
    json!({ "customerId": "cus_1001", "name": "Ada Lovelace", "email": "ada@example.com", "tier": "gold", "since": "2019-03-04T09:00:00.000Z" })
}

fn status(plan: &Plan, baseline: &Session, candidate: &Session, code: &str) -> (CheckStatus, Vec<String>) {
    let verdict = judge(plan, baseline, candidate, &["src/handlers/customers.lookup.mjs".to_string()]);
    let check = verdict.checks.into_iter().find(|c| c.code == code).unwrap();
    (check.status, check.violations)
}

#[test]
fn judges_contract_identity_from_the_published_manifest() {
    let plan = planned();
    let manifest = fixture("customers.manifest.json");
    let baseline = session(manifest.clone(), reply(ada()));
    assert_eq!(status(&plan, &baseline, &baseline, "contract.identity").0, CheckStatus::Pass);

    // A replacement that (somehow) widened its declared failures changed the contract.
    let mut changed = manifest;
    let lookup =
        changed["capabilities"].as_array_mut().unwrap().iter_mut().find(|c| c["name"] == "customers.lookup").unwrap();
    lookup["definitions"][0]["description"] = json!("Reads one customer, faster.");
    let (outcome, violations) = status(&plan, &baseline, &session(changed, reply(ada())), "contract.identity");
    assert_eq!(outcome, CheckStatus::Fail);
    assert!(violations[0].starts_with("contractDigest changed"), "{violations:?}");
    assert!(violations[1].contains("customers.lookup@1's contract pin changed"), "{violations:?}");
}

#[test]
fn judges_outputs_and_regressions_against_the_contract_and_the_baseline() {
    let plan = planned();
    let manifest = fixture("customers.manifest.json");
    let baseline = session(manifest.clone(), reply(ada()));

    let mut diamond = ada();
    diamond["tier"] = json!("diamond");
    let candidate = session(manifest.clone(), reply(diamond));
    let (outcome, violations) = status(&plan, &baseline, &candidate, "output.conformance");
    assert_eq!(outcome, CheckStatus::Fail);
    assert!(violations[0].contains("enum at \"/tier\""), "{violations:?}");
    assert_eq!(status(&plan, &baseline, &candidate, "behavior.regression").0, CheckStatus::Fail);

    let missing = session(manifest, Reply::Err { code: "INTERNAL_ERROR".into(), message: "boom".into() });
    let (outcome, violations) = status(&plan, &baseline, &missing, "failures.declared");
    assert_eq!(outcome, CheckStatus::Fail);
    assert!(violations[0].contains("with INTERNAL_ERROR (boom); allowed: NOT_FOUND"), "{violations:?}");
}

#[test]
fn judges_the_edit_boundary() {
    let plan = planned();
    let baseline = session(fixture("customers.manifest.json"), reply(ada()));
    let verdict =
        judge(&plan, &baseline, &baseline, &["src/handlers/customers.lookup.mjs".into(), "src/contract.mjs".into()]);
    assert!(!verdict.accepted);
    // `verification` fails too: these hand-made sessions carry no evidence.
    assert_eq!(verdict.failed(), ["edit.boundary", "verification"]);
    assert!(verdict.feedback()[0].starts_with("[edit.boundary] src/contract.mjs is outside the change boundary"));
}
