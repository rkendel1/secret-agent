//! Parity of the Rust primitives with `@appport/protocol`.
//!
//! Every expected value in `tests/fixtures/parity.json` was produced by calling
//! the published package's own function (see `tools/appport-inspect`).

mod common;

use agent_core::appport::knowledge::rules::*;
use agent_core::appport::knowledge::SemanticIssue;
use serde_json::{json, Value};

fn cases(name: &str) -> Vec<Value> {
    common::fixture("parity.json")[name].as_array().unwrap_or_else(|| panic!("no parity cases for {name}")).clone()
}

fn input(case: &Value) -> &str {
    case["input"].as_str().unwrap()
}

#[test]
fn validate_name_matches_js() {
    for case in cases("validateName") {
        let actual = serde_json::to_value(validate_name(input(&case))).unwrap();
        assert_eq!(actual, case["result"], "validateName({:?})", input(&case));
    }
}

#[test]
fn is_valid_permission_matches_js() {
    for case in cases("isValidPermission") {
        assert_eq!(json!(is_valid_permission(input(&case))), case["result"], "isValidPermission({:?})", input(&case));
    }
}

#[test]
fn is_valid_application_id_matches_js() {
    for case in cases("isValidApplicationId") {
        assert_eq!(
            json!(is_valid_application_id(input(&case))),
            case["result"],
            "isValidApplicationId({:?})",
            input(&case)
        );
    }
}

#[test]
fn parse_capability_ref_matches_js() {
    for case in cases("parseCapabilityRef") {
        let expected = &case["result"];
        // Documented divergence: versions are u32 here.
        let expected = match expected["version"].as_f64() {
            Some(v) if v > f64::from(u32::MAX) => &Value::Null,
            _ => expected,
        };
        let actual = serde_json::to_value(parse_capability_ref(input(&case))).unwrap();
        assert_eq!(&actual, expected, "parseCapabilityRef({:?})", input(&case));
    }
}

#[test]
fn version_constraints_match_js() {
    for case in cases("isVersionConstraint") {
        assert_eq!(
            json!(is_version_constraint(input(&case))),
            case["result"],
            "isVersionConstraint({:?})",
            input(&case)
        );
    }
    let satisfies = cases("satisfiesVersionConstraint");
    assert!(satisfies.len() > 30);
    for case in satisfies {
        let version = case["version"].as_u64().unwrap() as u32;
        let constraint = case["constraint"].as_str().unwrap();
        assert_eq!(
            json!(satisfies_version_constraint(version, constraint)),
            case["result"],
            "satisfiesVersionConstraint({version}, {constraint:?})"
        );
    }
}

#[test]
fn parse_dependency_ref_matches_js() {
    for case in cases("parseDependencyRef") {
        let actual = serde_json::to_value(parse_dependency_ref(input(&case))).unwrap();
        assert_eq!(actual, case["result"], "parseDependencyRef({:?})", input(&case));
    }
}

#[test]
fn is_digest_matches_js() {
    for case in cases("isDigest") {
        assert_eq!(json!(is_digest(input(&case))), case["result"], "isDigest({:?})", input(&case));
    }
}

#[test]
fn canonical_json_and_digests_match_js() {
    for case in cases("canonicalize") {
        let value = &case["value"];
        assert_eq!(canonicalize(value), case["canonical"].as_str().unwrap(), "canonicalize({value})");
        assert_eq!(digest_of(value), case["digest"].as_str().unwrap(), "digestOf({value})");
    }
}

#[test]
fn semantic_annotation_validation_matches_js() {
    for case in cases("validateSemanticAnnotations") {
        let expected: Vec<SemanticIssue> = serde_json::from_value(case["issues"].clone()).unwrap();
        let actual = validate_semantic_annotations(&case["schema"], "");
        assert_eq!(actual, expected, "validateSemanticAnnotations({})", case["schema"]);
    }
}

#[test]
fn every_parity_group_is_exercised() {
    let groups: Vec<String> = common::fixture("parity.json").as_object().unwrap().keys().cloned().collect();
    let known = [
        "validateName",
        "isValidPermission",
        "isValidApplicationId",
        "parseCapabilityRef",
        "isVersionConstraint",
        "satisfiesVersionConstraint",
        "parseDependencyRef",
        "isDigest",
        "canonicalize",
        "validateSemanticAnnotations",
    ];
    for group in &groups {
        assert!(known.contains(&group.as_str()), "parity group {group} has no Rust test");
    }
}
