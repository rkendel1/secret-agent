//! Inspection, graph, replacement boundaries and validation, on the real fixtures.

mod common;

use agent_core::appport::knowledge::*;
use serde_json::{json, Value};

fn orders() -> Contract {
    Contract::from_value(common::fixture("orders.manifest.json")).unwrap()
}

fn reference() -> Contract {
    Contract::from_value(common::fixture("reference.manifest.json")).unwrap()
}

fn pretty<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap() + "\n"
}

// ---------------------------------------------------------------------------
// Inspection
// ---------------------------------------------------------------------------

#[test]
fn inspect_reports_inputs_outputs_and_required_fields() {
    let view = orders().inspect("orders.place", 1).unwrap();
    let required: Vec<&str> = view.inputs.iter().filter(|f| f.required).map(|f| f.pointer.as_str()).collect();
    assert_eq!(required, ["/clientRef", "/sku", "/quantity"]);
    let note = view.inputs.iter().find(|f| f.pointer == "/note").unwrap();
    assert!(!note.required);
    let status = view.outputs.iter().find(|f| f.pointer == "/status").unwrap();
    assert_eq!(status.enumeration.as_ref().unwrap(), &vec![json!("pending"), json!("placed"), json!("cancelled")]);
    assert_eq!(status.concept.as_deref(), Some("https://schema.example.com/commerce/OrderStatus"));
    let total = view.outputs.iter().find(|f| f.pointer == "/total").unwrap();
    assert_eq!(total.unit.as_deref(), Some("[USD]"));
}

#[test]
fn inspect_reports_evidence_requirements() {
    let view = orders().inspect("orders.place", 1).unwrap();
    assert!(view.evidence.establishable);
    let kinds: Vec<(&str, VerificationStrength)> =
        view.evidence.methods.iter().map(|m| (m.id.as_str(), m.strength)).collect();
    assert_eq!(
        kinds,
        [("order-exists", VerificationStrength::Establishes), ("placed-event", VerificationStrength::Corroborates)]
    );
    assert_eq!(view.evidence.methods[0].probe.as_deref(), Some("orders.get@1"));
}

#[test]
fn absent_declarations_are_unknown_never_no() {
    // orders.get declares coverage for requires/access only, and no approval.
    let view = orders().inspect("orders.get", 1).unwrap();
    assert_eq!(view.unknowns, ["approval", "coverage.preconditions", "coverage.failures"]);
    // The reference app publishes no integration descriptor and leaves effect undeclared.
    let view = reference().inspect("documents.save", 2).unwrap();
    assert!(view.unknowns.contains(&"integration".to_string()));
    assert!(view.unknowns.contains(&"effect".to_string()));
    assert!(view.unknowns.contains(&"verification".to_string()));
    assert_eq!(view.effect, None);
}

#[test]
fn inspect_handles_every_capability_in_both_fixtures() {
    for contract in [orders(), reference()] {
        let views = contract.inspect_all();
        let expected: usize = contract.manifest().capabilities.iter().map(|c| c.definitions.len()).sum();
        assert_eq!(views.len(), expected);
    }
    assert!(orders().inspect("orders.place", 9).is_none());
    assert!(orders().inspect("orders.nope", 1).is_none());
}

#[test]
fn snapshot_capability_view() {
    common::snapshot("orders.place@1.view.json", &pretty(&orders().inspect("orders.place", 1).unwrap()));
    common::snapshot("documents.save@2.view.json", &pretty(&reference().inspect("documents.save", 2).unwrap()));
}

#[test]
fn model_json_is_compact_and_stable() {
    let view = orders().inspect("orders.get", 1).unwrap();
    let text = model_json(&view);
    assert!(!text.contains('\n') && !text.contains(": "));
    assert_eq!(text, model_json(&orders().inspect("orders.get", 1).unwrap()));
    // Absent fields are omitted rather than serialized as null.
    assert!(!text.contains("null"));
    assert!(text.len() < 2_000, "a capability slice should stay small: {} bytes", text.len());
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

#[test]
fn graph_contains_declared_dependencies_and_data_flows() {
    let graph = orders().graph();
    let place = "capability:com.example.orders/orders.place@1";
    let out: Vec<(EdgeKind, &str)> = graph.outgoing(place).map(|e| (e.kind, e.to.as_str())).collect();
    assert!(out.contains(&(EdgeKind::Invokes, "capability:com.example.inventory/inventory.reserve@1")));
    assert!(out.contains(&(EdgeKind::Observes, "capability:com.example.orders/orders.get@1")));
    assert!(out.contains(&(EdgeKind::ConsumesEvent, "event:com.example.pricing/pricing.changed")));
    assert!(out.contains(&(EdgeKind::Emits, "event:com.example.orders/orders.placed")));
    assert!(out.contains(&(EdgeKind::Changes, "resource:urn:example:orders:order")));
    assert!(out.contains(&(EdgeKind::VerifiedBy, "capability:com.example.orders/orders.get@1")));
    assert!(out.contains(&(EdgeKind::CheckedBy, "capability:com.example.orders/orders.byref@1")));
    // An unpinned local dependency resolves through its version constraint.
    let cancel = "capability:com.example.orders/orders.cancel@1";
    assert!(graph.outgoing(cancel).any(|e| e.kind == EdgeKind::Observes && e.to.ends_with("orders.get@1")));
}

#[test]
fn cross_application_targets_are_unresolved_not_invented() {
    let graph = orders().graph();
    let mut targets: Vec<&str> = graph.unresolved.iter().map(|u| u.target.as_str()).collect();
    targets.sort();
    assert_eq!(
        targets,
        [
            "capability:com.example.inventory/inventory.reserve@1",
            "capability:com.example.notifications/notifications.send@2",
            "event:com.example.pricing/pricing.changed",
        ]
    );
    let inventory = graph.node("capability:com.example.inventory/inventory.reserve@1").unwrap();
    assert!(!inventory.resolved);
    // The stock counter is owned by the inventory application, not by orders.
    let owner = graph.incoming("resource:urn:example:inventory:stock").find(|e| e.kind == EdgeKind::Owns).unwrap();
    assert_eq!(owner.from, "application:com.example.inventory");
}

#[test]
fn graph_is_deterministic() {
    assert_eq!(orders().graph(), orders().graph());
    common::snapshot("orders.graph.json", &pretty(&orders().graph()));
}

// ---------------------------------------------------------------------------
// Replacement boundaries
// ---------------------------------------------------------------------------

#[test]
fn replacement_boundary_lists_dependents_constraints_and_gates() {
    let contract = orders();
    let boundary = contract.replacement_boundary("orders.get", 1).unwrap();
    let dependents: Vec<(&str, EdgeKind)> =
        boundary.dependents.iter().map(|d| (d.node.as_str(), d.relationship)).collect();
    assert!(dependents.contains(&("capability:com.example.orders/orders.place@1", EdgeKind::Observes)));
    assert!(dependents.contains(&("capability:com.example.orders/orders.place@1", EdgeKind::VerifiedBy)));
    assert!(dependents.contains(&("capability:com.example.orders/orders.cancel@1", EdgeKind::CheckedBy)));
    let codes: Vec<&str> = boundary.constraints.iter().map(|c| c.code.as_str()).collect();
    assert!(codes.contains(&"effect") && codes.contains(&"idempotency") && codes.contains(&"input.accept-no-less"));
    assert_eq!(boundary.handover.len(), HANDOVER_GATES.len());
    assert!(boundary.handover.iter().any(|g| g.rejection == ProviderHandoverRejection::ReplacementNotAuthorized));
}

#[test]
fn contract_pin_changes_only_when_the_capability_contract_does() {
    let pin =
        |raw: Value| Contract::from_value(raw).unwrap().replacement_boundary("orders.place", 1).unwrap().contract_pin;
    let base = common::fixture("orders.manifest.json");
    let original = pin(base.clone());

    let mut moved = base.clone();
    moved["transports"] = json!([{ "kind": "websocket", "endpoint": "/appport/ws" }]);
    assert_eq!(pin(moved), original, "where it is served is not part of the contract");

    let mut other = base.clone();
    let get = other["capabilities"].as_array_mut().unwrap().iter_mut().find(|c| c["name"] == "orders.get").unwrap();
    get["description"] = json!("changed");
    assert_eq!(pin(other), original, "another capability changing does not move this pin");

    let mut changed = base;
    changed["attributes"][constants::INTEGRATION_EXTENSION]["capabilities"]["orders.place@1"]["failures"][0]["retry"] =
        json!("safe");
    assert_ne!(pin(changed), original, "failure semantics are part of the capability contract");
}

#[test]
fn snapshot_replacement_boundary() {
    common::snapshot(
        "orders.place@1.boundary.json",
        &pretty(&orders().replacement_boundary("orders.place", 1).unwrap()),
    );
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn rules_for(raw: Value) -> Vec<String> {
    let contract = Contract::from_value(raw).unwrap();
    contract.validate().into_iter().map(|i| i.rule).collect()
}

fn capability_mut<'a>(raw: &'a mut Value, name: &str) -> &'a mut Value {
    raw["capabilities"].as_array_mut().unwrap().iter_mut().find(|c| c["name"] == name).unwrap()
}

#[test]
fn validation_catches_broken_names_and_ids() {
    let mut raw = common::fixture("reference.manifest.json");
    raw["application"]["id"] = json!("Not An Id");
    capability_mut(&mut raw, "documents.open")["name"] = json!("http.open");
    let rules = rules_for(raw);
    assert!(rules.contains(&"application.id".into()));
    assert!(rules.contains(&"name".into()));
}

#[test]
fn validation_catches_inconsistent_versions_and_mirrors() {
    let mut raw = common::fixture("reference.manifest.json");
    let save = capability_mut(&mut raw, "documents.save");
    save["latestVersion"] = json!(3);
    save["versions"] = json!([1]);
    save["authorization"] = json!(["documents.other"]);
    let rules = rules_for(raw);
    assert!(rules.contains(&"versions.latest".into()));
    assert!(rules.contains(&"versions.mismatch".into()));
    assert!(rules.contains(&"capability.mirror".into()));
}

#[test]
fn validation_catches_integration_problems() {
    let mut raw = common::fixture("orders.manifest.json");
    let descriptor = &mut raw["attributes"][constants::INTEGRATION_EXTENSION];
    descriptor["capabilities"]["orders.ghost@1"] = json!({ "purpose": "no such capability" });
    descriptor["capabilities"]["orders.cancel@1"]["requires"][0].as_object_mut().unwrap().remove("versionConstraint");
    descriptor["capabilities"]["orders.cancel@1"]["verify"][0]["expect"][0]["exists"] = json!(true);
    descriptor["mappings"][0]["artifact"]["fields"]["qty"] = json!("/count");
    raw["extensions"] = json!([]);
    let rules = rules_for(raw);
    for expected in [
        "integration.key",
        "dependency.unpinned",
        "verification.expectation",
        "mapping.artifact-digest",
        "integration.extension",
    ] {
        assert!(rules.contains(&expected.to_string()), "expected {expected} in {rules:?}");
    }
}

#[test]
fn validation_catches_semantic_annotation_errors() {
    let mut raw = common::fixture("orders.manifest.json");
    let get = capability_mut(&mut raw, "orders.get");
    get["definitions"][0]["outputSchema"]["properties"]["placedAt"]["x-appport-encoding"] = json!("unix");
    let issues = Contract::from_value(raw).unwrap().validate();
    let issue = issues.iter().find(|i| i.rule == "schema.semantic").unwrap();
    assert_eq!(issue.path, "orders.get@1 output/properties/placedAt");
    assert_eq!(issue.message, "x-appport-encoding must be one of: iso8601, epoch-seconds, epoch-milliseconds");
}

#[test]
fn snapshot_validation_report() {
    let mut raw = common::fixture("orders.manifest.json");
    capability_mut(&mut raw, "orders.cancel")["definitions"][0]["authorization"] = json!(["Orders.Write"]);
    raw["attributes"][constants::INTEGRATION_EXTENSION]["capabilities"]["orders.place@1"]["requires"][2]
        ["versionConstraint"] = json!("v1");
    common::snapshot("orders.invalid.issues.json", &pretty(&Contract::from_value(raw).unwrap().validate()));
}
