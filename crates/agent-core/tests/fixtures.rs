//! The Rust model against real documents produced by (or type-checked against)
//! the published packages.

mod common;

use agent_core::appport::knowledge::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

const MANIFESTS: [&str; 2] = ["reference.manifest.json", "orders.manifest.json"];

/// Parse into `T` and serialize back: nothing may be lost, added or changed.
fn round_trip<T: DeserializeOwned + Serialize>(value: &Value, what: &str) -> T {
    let typed: T = serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{what} does not parse: {e}"));
    assert_eq!(&serde_json::to_value(&typed).unwrap(), value, "{what} does not round-trip");
    typed
}

#[test]
fn manifests_round_trip_exactly() {
    for name in MANIFESTS {
        let raw = common::fixture(name);
        round_trip::<Manifest>(&raw, name);
        let contract = Contract::from_value(raw).unwrap();
        assert!(contract.unmodeled().is_empty(), "{name}: unmodeled members {:?}", contract.unmodeled());
    }
}

#[test]
fn integration_descriptor_round_trips() {
    let raw = common::fixture("orders.manifest.json");
    let descriptor = &raw["attributes"][constants::INTEGRATION_EXTENSION];
    let typed: IntegrationDescriptor = round_trip(descriptor, "orders integration descriptor");
    assert!(!typed.is_empty());
    assert_eq!(typed.capabilities.len(), 4);
    // The reference fixture publishes none.
    let reference = Contract::from_value(common::fixture("reference.manifest.json")).unwrap();
    assert!(reference.integration().is_none());
}

#[test]
fn contract_digests_match_the_package() {
    let facts = common::fixture("manifest-facts.json");
    for name in MANIFESTS {
        let raw = common::fixture(name);
        let contract = Contract::from_value(raw.clone()).unwrap();
        assert_eq!(contract.digest(), facts[name]["contractDigest"].as_str().unwrap(), "{name} contractDigest");
        assert_eq!(rules::digest_of(&raw), facts[name]["digest"].as_str().unwrap(), "{name} digestOf");
        // Digest of the *typed* manifest too: the model preserves identity.
        let typed = serde_json::to_value(contract.manifest()).unwrap();
        assert_eq!(rules::contract_digest_of_value(&typed), contract.digest(), "{name} typed digest");
    }
}

#[test]
fn semantic_annotations_on_real_schemas_match_the_package() {
    let facts = common::fixture("manifest-facts.json");
    for name in MANIFESTS {
        let contract = Contract::from_value(common::fixture(name)).unwrap();
        let expected = facts[name]["semanticIssues"].as_array().unwrap();
        let mut index = 0;
        let mut check = |at: String, schema: &Schema| {
            let entry = &expected[index];
            index += 1;
            assert_eq!(entry["at"].as_str().unwrap(), at);
            let issues = rules::validate_semantic_annotations(&serde_json::to_value(schema).unwrap(), "");
            assert_eq!(serde_json::to_value(issues).unwrap(), entry["issues"], "{name} {at}");
        };
        for (entry, definition) in contract.manifest().capability_versions() {
            check(format!("{}@{} input", entry.name, definition.version), &definition.input_schema);
            check(format!("{}@{} output", entry.name, definition.version), &definition.output_schema);
        }
        for event in &contract.manifest().events {
            for definition in &event.definitions {
                check(format!("{}@{} payload", event.name, definition.version), &definition.payload_schema);
            }
        }
        assert_eq!(index, expected.len());
    }
}

#[test]
fn real_manifests_pass_structural_validation() {
    let facts = common::fixture("manifest-facts.json");
    for name in MANIFESTS {
        let contract = Contract::from_value(common::fixture(name)).unwrap();
        let issues = contract.validate();
        let errors: Vec<_> = issues.iter().filter(|i| i.is_error()).collect();
        assert!(errors.is_empty(), "{name}: {errors:#?}");
        // The official linters agree there is nothing wrong.
        for linter in ["lintIntegration", "lintAgentContract", "lintDependencyModel"] {
            let reported: Vec<IntegrationIssue> = serde_json::from_value(facts[name]["lint"][linter].clone()).unwrap();
            assert!(reported.iter().all(|i| i.level != IssueLevel::Error), "{name} {linter}");
        }
    }
}

#[test]
fn evidence_and_handover_records_round_trip() {
    for (index, record) in common::fixture("verification-evidence.json").as_array().unwrap().iter().enumerate() {
        round_trip::<VerificationEvidence>(record, &format!("verification evidence {index}"));
    }
    let runtime = common::fixture("runtime-evidence.json");
    let parsed: Vec<RuntimeEvidence> = round_trip(&runtime, "runtime evidence");
    assert!(matches!(parsed[0], RuntimeEvidence::Invocation(_)));
    assert!(matches!(parsed[2], RuntimeEvidence::StateAccess(_)));

    let handover = common::fixture("handover.json");
    round_trip::<ProviderHandoverRequest>(&handover["request"], "handover request");
    round_trip::<ProviderHandoverRecord>(&handover["record"], "handover record");
    round_trip::<Vec<ProviderHandoverRejection>>(&handover["rejections"], "handover rejections");
}

#[test]
fn compatibility_report_round_trips() {
    let fixture = common::fixture("compatibility.json");
    let report: CompatibilityReport = round_trip(&fixture["report"], "compatibility report");
    assert!(!report.compatible);
    assert!(report.breaking.iter().any(|c| c.kind == "capability.version-removed"));
    round_trip::<Manifest>(&fixture["next"], "compatibility next manifest");
}

#[test]
fn documents_that_violate_the_contract_are_rejected() {
    let mut raw = common::fixture("reference.manifest.json");
    raw["protocol"] = "appport/2".into();
    assert!(Contract::from_value(raw).is_err(), "unknown protocol id");

    let mut raw = common::fixture("orders.manifest.json");
    let place = &mut raw["attributes"][constants::INTEGRATION_EXTENSION]["capabilities"]["orders.place@1"];
    place["verify"][1]["strength"] = "establishing".into();
    let contract = Contract::from_value(raw).unwrap();
    assert!(contract.integration().is_none());
    assert!(contract.validate().iter().any(|i| i.rule == "integration.parse"), "events can only corroborate");
}

#[test]
fn newer_members_are_reported_not_silently_dropped() {
    let mut raw = common::fixture("orders.manifest.json");
    raw["capabilities"][0]["definitions"][0]["costHint"] = "cheap".into();
    raw["attributes"][constants::INTEGRATION_EXTENSION]["capabilities"]["orders.place@1"]["sla"] = 5.into();
    let contract = Contract::from_value(raw).unwrap();
    assert_eq!(
        contract.unmodeled(),
        vec![
            "/capabilities/0/definitions/0/costHint".to_string(),
            "/attributes/appport.integration~11/capabilities/orders.place@1/sla".to_string()
        ]
    );
    assert!(contract.validate().iter().any(|i| i.rule == "model.unmodeled"));
}
