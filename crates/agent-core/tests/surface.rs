//! Drift check, Rust side: the Rust model against `knowledge/appport-surface.json`.
//!
//! The surface file records, for every mirrored type, what the published `.d.ts`
//! declares (property names, optionality, literal values, union variants, nested
//! anonymous shapes) and the packages' runtime constants. Here each Rust type's
//! JSON Schema (from `schemars`, i.e. from the actual serde attributes) is
//! compared with it. Adding, removing or renaming a field, changing what is
//! optional, or adding an enum value on either side fails this test.
//!
//! The other half — the surface file against the live npm packages — is
//! `tools/appport-inspect --check` (run by `scripts/check-appport-drift.sh`).

mod common;

use std::collections::{BTreeMap, BTreeSet};

use agent_core::appport::knowledge::constants::*;
use agent_core::appport::knowledge::*;
use schemars::{schema_for, JsonSchema};
use serde_json::{json, Value};

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schema_for!(T)).unwrap()
}

/// TypeScript type name → the Rust type that mirrors it.
fn mapping() -> BTreeMap<(&'static str, &'static str), Value> {
    const P: &str = "@appport/protocol";
    const C: &str = "@appport/core";
    BTreeMap::from([
        ((P, "AppPortManifest"), schema::<Manifest>()),
        ((P, "ApplicationDescriptor"), schema::<ApplicationDescriptor>()),
        ((P, "TransportDescriptor"), schema::<TransportDescriptor>()),
        ((P, "EventManifestEntry"), schema::<EventEntry>()),
        ((P, "EventVersionEntry"), schema::<EventVersion>()),
        ((P, "JSONSchema"), schema::<Schema>()),
        ((P, "JSONSchemaType"), schema::<SchemaType>()),
        ((P, "MaterializationDeclaration"), schema::<MaterializationDeclaration>()),
        ((P, "UiContribution"), schema::<UiContribution>()),
        ((P, "UiSurface"), schema::<UiSurface>()),
        ((P, "UiNavigationContribution"), schema::<UiNavigationContribution>()),
        ((P, "UiProductIdentity"), schema::<UiProductIdentity>()),
        ((P, "UiCompositionRequirement"), schema::<UiCompositionRequirement>()),
        ((P, "CapabilityManifestEntry"), schema::<CapabilityEntry>()),
        ((P, "CapabilityVersionEntry"), schema::<CapabilityVersion>()),
        ((P, "CapabilityKind"), schema::<CapabilityKind>()),
        ((P, "ConcurrencyPolicy"), schema::<ConcurrencyPolicy>()),
        ((P, "IdempotencySupport"), schema::<IdempotencySupport>()),
        ((P, "CapabilityEffect"), schema::<CapabilityEffect>()),
        ((P, "AuthorizationRequirement"), schema::<AuthorizationRequirement>()),
        ((P, "AuthorizationResource"), schema::<AuthorizationResource>()),
        ((P, "OperationStatus"), schema::<OperationStatus>()),
        ((P, "IntegrationDescriptor"), schema::<IntegrationDescriptor>()),
        ((P, "CapabilityIntegration"), schema::<CapabilityIntegration>()),
        ((P, "EffectDescriptor"), schema::<EffectDescriptor>()),
        ((P, "ApprovalRequirement"), schema::<ApprovalRequirement>()),
        ((P, "Precondition"), schema::<Precondition>()),
        ((P, "FailureDeclaration"), schema::<FailureDeclaration>()),
        ((P, "RetryClass"), schema::<RetryClass>()),
        ((P, "FailureEffect"), schema::<FailureEffect>()),
        ((P, "Dependency"), schema::<Dependency>()),
        ((P, "DependencyRelationship"), schema::<DependencyRelationship>()),
        ((P, "DependencyTarget"), schema::<DependencyTarget>()),
        ((P, "ResourceDeclaration"), schema::<ResourceDeclaration>()),
        ((P, "ResourceAccess"), schema::<ResourceAccess>()),
        ((P, "Coverage"), schema::<Coverage>()),
        ((P, "DeclarationCoverage"), schema::<DeclarationCoverage>()),
        ((P, "MappingReference"), schema::<MappingReference>()),
        ((P, "MappingDirection"), schema::<MappingDirection>()),
        ((P, "MappingOrigin"), schema::<MappingOrigin>()),
        ((P, "SemanticAnnotations"), schema::<SemanticAnnotations>()),
        ((P, "SemanticEncoding"), schema::<SemanticEncoding>()),
        ((P, "SemanticIssue"), schema::<SemanticIssue>()),
        ((P, "IntegrationIssue"), schema::<IntegrationIssue>()),
        ((P, "VerificationMethod"), schema::<VerificationMethod>()),
        ((P, "StateVerification"), schema::<StateVerification>()),
        ((P, "EventVerification"), schema::<EventVerification>()),
        ((P, "StateExpectation"), schema::<StateExpectation>()),
        ((P, "VerificationEvidence"), schema::<VerificationEvidence>()),
        ((P, "VerificationOutcome"), schema::<VerificationOutcome>()),
        ((P, "MethodStatus"), schema::<MethodStatus>()),
        ((P, "ChangeSeverity"), schema::<ChangeSeverity>()),
        ((P, "ManifestChange"), schema::<ManifestChange>()),
        ((P, "CompatibilityReport"), schema::<CompatibilityReport>()),
        ((C, "InvocationEvidence"), schema::<InvocationEvidence>()),
        ((C, "InvocationCallerEvidence"), schema::<InvocationCallerEvidence>()),
        ((C, "InvocationOutcome"), schema::<InvocationOutcome>()),
        ((C, "StateAccessEvidence"), schema::<StateAccessEvidence>()),
        ((C, "StateOperation"), schema::<StateOperation>()),
        ((C, "ProviderHandoverRequest"), schema::<ProviderHandoverRequest>()),
        ((C, "ProviderRegistrationExpectation"), schema::<ProviderRegistrationExpectation>()),
        ((C, "ProviderHandoverRejection"), schema::<ProviderHandoverRejection>()),
        ((C, "ProviderHandoverRecord"), schema::<ProviderHandoverRecord>()),
    ])
}

// ---------------------------------------------------------------------------
// Reading the Rust side's JSON Schema
// ---------------------------------------------------------------------------

fn resolve<'a>(root: &'a Value, mut node: &'a Value) -> &'a Value {
    while let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let name = reference.trim_start_matches("#/$defs/");
        node = &root["$defs"][name];
    }
    node
}

/// Drops the `null` arm schemars adds for `Option<T>`.
fn strip_null<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    let node = resolve(root, node);
    for key in ["anyOf", "oneOf"] {
        if let Some(variants) = node.get(key).and_then(Value::as_array) {
            let rest: Vec<&Value> =
                variants.iter().filter(|v| resolve(root, v).get("type") != Some(&json!("null"))).collect();
            if rest.len() == 1 && variants.len() == 2 {
                return resolve(root, rest[0]);
            }
        }
    }
    node
}

fn variants<'a>(root: &'a Value, node: &'a Value) -> Option<Vec<&'a Value>> {
    let node = resolve(root, node);
    ["anyOf", "oneOf"]
        .iter()
        .find_map(|k| node.get(*k).and_then(Value::as_array))
        .map(|list| list.iter().map(|v| resolve(root, v)).collect())
}

/// `(properties, required)` of an object schema, merging `allOf` (serde `flatten`).
fn object<'a>(root: &'a Value, node: &'a Value) -> Option<(BTreeMap<String, &'a Value>, BTreeSet<String>)> {
    let node = resolve(root, node);
    let mut properties = BTreeMap::new();
    let mut required = BTreeSet::new();
    let mut found = false;
    if let Some(map) = node.get("properties").and_then(Value::as_object) {
        found = true;
        properties.extend(map.iter().map(|(k, v)| (k.clone(), v)));
    }
    if let Some(list) = node.get("required").and_then(Value::as_array) {
        required.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
    }
    for part in node.get("allOf").and_then(Value::as_array).into_iter().flatten() {
        if let Some((p, r)) = object(root, part) {
            found = true;
            properties.extend(p);
            required.extend(r);
        }
    }
    found.then_some((properties, required))
}

fn literals(root: &Value, node: &Value) -> Option<BTreeSet<String>> {
    let node = strip_null(root, node);
    let text = |v: &Value| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
    if let Some(values) = node.get("enum").and_then(Value::as_array) {
        return Some(values.iter().map(text).collect());
    }
    if let Some(value) = node.get("const") {
        return Some(BTreeSet::from([text(value)]));
    }
    let mut out = BTreeSet::new();
    for variant in variants(root, node)? {
        out.extend(literals(root, variant)?);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// String-literal values of a TS property type text such as `"a" | "b" | undefined`.
fn ts_inline_literals(text: &str) -> Option<BTreeSet<String>> {
    let parts: Vec<&str> = text.split(" | ").filter(|p| *p != "undefined").collect();
    parts
        .iter()
        .all(|p| p.len() >= 2 && p.starts_with('"') && p.ends_with('"'))
        .then(|| parts.iter().map(|p| p[1..p.len() - 1].to_string()).collect())
}

fn compare(root: &Value, rust: &Value, ts: &Value, at: &str, errors: &mut Vec<String>) {
    match ts["kind"].as_str().unwrap() {
        "literals" => {
            let expected: BTreeSet<String> = ts["values"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
                .collect();
            match literals(root, rust) {
                Some(actual) if actual == expected => {}
                actual => errors.push(format!("{at}: literal values differ: TS {expected:?}, Rust {actual:?}")),
            }
        }
        "object" => {
            let Some((properties, required)) = object(root, strip_null(root, rust)) else {
                return errors.push(format!("{at}: TS declares an object, Rust schema is not one"));
            };
            let ts_props = ts["properties"].as_object().unwrap();
            let ts_keys: BTreeSet<String> = ts_props.keys().cloned().collect();
            let rust_keys: BTreeSet<String> = properties.keys().cloned().collect();
            if ts_keys != rust_keys {
                errors.push(format!(
                    "{at}: properties differ: only in TS {:?}, only in Rust {:?}",
                    ts_keys.difference(&rust_keys).collect::<Vec<_>>(),
                    rust_keys.difference(&ts_keys).collect::<Vec<_>>()
                ));
                return;
            }
            for (name, property) in ts_props {
                let optional = property["optional"].as_bool().unwrap();
                if optional == required.contains(name) {
                    errors.push(format!(
                        "{at}.{name}: TS optional={optional}, Rust required={}",
                        required.contains(name)
                    ));
                }
                let child = properties[name];
                if let Some(expected) = ts_inline_literals(property["type"].as_str().unwrap()) {
                    match literals(root, child) {
                        Some(actual) if actual == expected => {}
                        actual => {
                            errors.push(format!("{at}.{name}: literal values differ: TS {expected:?}, Rust {actual:?}"))
                        }
                    }
                }
                if let Some(shape) = property.get("shape") {
                    let child = strip_null(root, child);
                    if shape["kind"] == "array" {
                        match child.get("items") {
                            Some(items) => compare(root, items, &shape["items"], &format!("{at}.{name}[]"), errors),
                            None => errors.push(format!("{at}.{name}: TS declares an array, Rust schema is not one")),
                        }
                    } else {
                        compare(root, child, shape, &format!("{at}.{name}"), errors);
                    }
                }
            }
        }
        "union" => {
            let Some(rust_variants) = variants(root, strip_null(root, rust)) else {
                return errors.push(format!("{at}: TS declares a union, Rust schema is not one"));
            };
            let key = |props: BTreeSet<String>, req: BTreeSet<String>| format!("{props:?} required {req:?}");
            let mut expected: Vec<String> = ts["variants"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    let props = v["properties"].as_object().unwrap();
                    key(
                        props.keys().cloned().collect(),
                        props.iter().filter(|(_, p)| p["optional"] == false).map(|(k, _)| k.clone()).collect(),
                    )
                })
                .collect();
            let mut actual: Vec<String> = rust_variants
                .iter()
                .filter_map(|v| object(root, v))
                .map(|(p, r)| key(p.keys().cloned().collect(), r))
                .collect();
            expected.sort();
            actual.sort();
            if expected != actual {
                errors.push(format!("{at}: union variants differ:\n  TS   {expected:?}\n  Rust {actual:?}"));
            }
            // Each TS variant must be matched in full (optionality, discriminant values, nested
            // shapes) by at least one Rust variant with the same members.
            for (index, variant) in ts["variants"].as_array().unwrap().iter().enumerate() {
                let wanted: BTreeSet<&String> = variant["properties"].as_object().unwrap().keys().collect();
                let mut attempts = Vec::new();
                let matched = rust_variants
                    .iter()
                    .filter(|v| object(root, v).is_some_and(|(p, _)| p.keys().collect::<BTreeSet<_>>() == wanted))
                    .any(|candidate| {
                        let mut sub = Vec::new();
                        compare(root, candidate, variant, &format!("{at}|{index}"), &mut sub);
                        let ok = sub.is_empty();
                        attempts.extend(sub);
                        ok
                    });
                if !matched {
                    errors.extend(attempts);
                }
            }
            // All discriminant values across variants, per property.
            let mut ts_literals: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for variant in ts["variants"].as_array().unwrap() {
                for (name, property) in variant["properties"].as_object().unwrap() {
                    if let Some(values) = ts_inline_literals(property["type"].as_str().unwrap()) {
                        ts_literals.entry(name.clone()).or_default().extend(values);
                    }
                }
            }
            for (name, expected) in ts_literals {
                let mut actual = BTreeSet::new();
                for variant in &rust_variants {
                    if let Some((props, _)) = object(root, variant) {
                        if let Some(values) = props.get(&name).and_then(|p| literals(root, p)) {
                            actual.extend(values);
                        }
                    }
                }
                if actual != expected {
                    errors.push(format!("{at}.{name}: discriminant values differ: TS {expected:?}, Rust {actual:?}"));
                }
            }
        }
        other => errors.push(format!("{at}: unsupported TS kind {other}")),
    }
}

fn drift_errors(surface: &Value) -> Vec<String> {
    let mapping = mapping();
    let mut errors = Vec::new();
    let mut covered = BTreeSet::new();
    for (package, types) in surface["types"].as_object().unwrap() {
        for (name, ts) in types.as_object().unwrap() {
            covered.insert((package.clone(), name.clone()));
            match mapping.iter().find(|((p, n), _)| *p == package && *n == name) {
                Some((_, rust)) => compare(rust, rust, ts, name, &mut errors),
                None => errors.push(format!("{package} {name}: inspected but has no Rust mapping")),
            }
        }
    }
    for (package, name) in mapping.keys() {
        if !covered.contains(&(package.to_string(), name.to_string())) {
            errors.push(format!("{package} {name}: mapped in Rust but not in the inspected surface"));
        }
    }
    errors
}

#[test]
fn rust_types_match_the_published_type_definitions() {
    let errors = drift_errors(&common::surface());
    assert!(errors.is_empty(), "the Rust model has drifted from @appport/*:\n{}", errors.join("\n"));
}

/// The check above must actually fail when the packages change. Each mutation
/// simulates a plausible upstream change to the inspected surface.
#[test]
fn drift_check_detects_divergence() {
    const P: &str = "@appport/protocol";
    type Mutation = (&'static str, fn(&mut Value));
    let mutations: [Mutation; 9] = [
        ("new optional field", |s| {
            s["types"][P]["CapabilityVersionEntry"]["properties"]["costHint"] =
                json!({"optional": true, "type": "string | undefined"});
        }),
        ("field became required", |s| {
            s["types"][P]["Dependency"]["properties"]["purpose"]["optional"] = json!(false);
        }),
        ("field removed", |s| {
            s["types"][P]["MappingReference"]["properties"].as_object_mut().unwrap().remove("uri");
        }),
        ("enum value added", |s| {
            s["types"][P]["RetryClass"]["values"].as_array_mut().unwrap().push(json!("never"));
        }),
        ("inline literal widened", |s| {
            s["types"][P]["ResourceAccess"]["properties"]["mode"]["type"] = json!("\"reads\" | \"changes\" | \"owns\"");
        }),
        ("nested anonymous shape changed", |s| {
            s["types"][P]["Precondition"]["properties"]["via"]["shape"]["properties"]["timeoutMs"] =
                json!({"optional": true, "type": "number | undefined"});
        }),
        ("union variant added", |s| {
            let variant = json!({"kind": "object", "properties": {"mode": {"optional": false, "type": "\"pooled\""}, "size": {"optional": false, "type": "number"}}});
            s["types"][P]["ConcurrencyPolicy"]["variants"].as_array_mut().unwrap().push(variant);
        }),
        ("discriminant value changed", |s| {
            s["types"][P]["EventVerification"]["properties"]["strength"]["type"] =
                json!("\"corroborating\" | \"establishing\"");
        }),
        ("new exported type", |s| {
            s["types"][P]["RetryBudget"] = json!({"kind": "object", "properties": {}});
        }),
    ];
    for (what, mutate) in mutations {
        let mut surface = common::surface();
        mutate(&mut surface);
        assert!(!drift_errors(&surface).is_empty(), "drift not detected: {what}");
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

fn as_set(values: impl IntoIterator<Item = impl ToString>) -> BTreeSet<String> {
    values.into_iter().map(|v| v.to_string()).collect()
}

fn list(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

#[test]
fn constants_match_the_packages_runtime_values() {
    let surface = common::surface();
    let p = &surface["constants"]["@appport/protocol"];
    let c = &surface["constants"]["@appport/core"];

    let packages: BTreeMap<String, String> = serde_json::from_value(surface["packages"].clone()).unwrap();
    let ours: BTreeMap<String, String> = SOURCE_PACKAGES.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
    assert_eq!(ours, packages, "SOURCE_PACKAGES must name the inspected versions");

    assert_eq!(p["PROTOCOL_ID"], PROTOCOL_ID);
    assert_eq!(p["PROTOCOL_ID"], ProtocolId::VALUE);
    assert_eq!(p["PROTOCOL_MAJOR"], PROTOCOL_MAJOR);
    let levels: serde_json::Map<String, Value> =
        CONFORMANCE_LEVELS.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    assert_eq!(p["CONFORMANCE_LEVELS"], Value::Object(levels));
    assert_eq!(list(&p["RESERVED_NAME_SEGMENTS"]), RESERVED_NAME_SEGMENTS);
    assert_eq!(p["JSON_SCHEMA_DIALECT"], JSON_SCHEMA_DIALECT);
    assert_eq!(p["WELL_KNOWN_PATH"], WELL_KNOWN_PATH);
    assert_eq!(p["DEFAULT_HTTP_ENDPOINT"], DEFAULT_HTTP_ENDPOINT);
    assert_eq!(p["DEFAULT_WS_ENDPOINT"], DEFAULT_WS_ENDPOINT);
    assert_eq!(p["DEFAULT_MANIFEST_PATH"], DEFAULT_MANIFEST_PATH);
    assert_eq!(p["AUTHORIZATION_MODEL_VERSION"], AUTHORIZATION_MODEL_VERSION);
    assert_eq!(p["INTEGRATION_EXTENSION"], INTEGRATION_EXTENSION);
    assert_eq!(
        p["SEMANTIC_KEYWORDS"],
        json!({
            "concept": SEMANTIC_KEYWORDS.concept,
            "unit": SEMANTIC_KEYWORDS.unit,
            "enumConcepts": SEMANTIC_KEYWORDS.enum_concepts,
            "encoding": SEMANTIC_KEYWORDS.encoding
        })
    );
    assert_eq!(list(&p["SEMANTIC_ENCODINGS"]), SEMANTIC_ENCODINGS);
    assert_eq!(list(&p["RETRY_CLASSES"]), RETRY_CLASSES);
    assert_eq!(list(&p["FAILURE_EFFECTS"]), FAILURE_EFFECTS);
    assert_eq!(list(&p["DEPENDENCY_RELATIONSHIPS"]), DEPENDENCY_RELATIONSHIPS);
    assert_eq!(list(&p["RESOURCE_KINDS"]), RESOURCE_KINDS);
    assert_eq!(list(&p["RESOURCE_OPERATIONS"]), RESOURCE_OPERATIONS);
    assert_eq!(list(&p["COVERAGE_KEYS"]), COVERAGE_KEYS);
    assert_eq!(list(&p["MAPPING_DIRECTIONS"]), MAPPING_DIRECTIONS);
    assert_eq!(list(&p["MAPPING_ORIGINS"]), MAPPING_ORIGINS);
    let operations: serde_json::Map<String, Value> =
        OPERATION_CAPABILITIES.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    assert_eq!(p["OPERATION_CAPABILITIES"], Value::Object(operations));
    assert_eq!(p["DIGEST_PATTERN"], rules::DIGEST_PATTERN_SOURCE);
    assert_eq!(c["INVOCATION_EVIDENCE"], INVOCATION_EVIDENCE);
    assert_eq!(c["INVOCATION_EVIDENCE"], InvocationEvidenceType::VALUE);
    assert_eq!(c["STATE_ACCESS_EVIDENCE"], STATE_ACCESS_EVIDENCE);
    assert_eq!(c["STATE_ACCESS_EVIDENCE"], StateAccessEvidenceType::VALUE);
    assert_eq!(list(&c["BUILTIN_CAPABILITY_NAMES"]), BUILTIN_CAPABILITY_NAMES);

    // The schema constants parse as the Rust Schema type and round-trip.
    for name in ["ANY_SCHEMA", "EMPTY_OBJECT_SCHEMA"] {
        let parsed: Schema = serde_json::from_value(p[name].clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), p[name], "{name}");
    }
}

#[test]
fn rust_enums_carry_exactly_the_published_value_lists() {
    let s = &common::surface()["constants"]["@appport/protocol"];
    let check = |constant: &str, rust: Value| {
        let rust_values = literals(&rust, &rust).unwrap();
        assert_eq!(rust_values, as_set(list(&s[constant])), "{constant}");
    };
    check("RETRY_CLASSES", schema::<RetryClass>());
    check("FAILURE_EFFECTS", schema::<FailureEffect>());
    check("DEPENDENCY_RELATIONSHIPS", schema::<DependencyRelationship>());
    check("RESOURCE_KINDS", schema::<ResourceKind>());
    check("RESOURCE_OPERATIONS", schema::<ResourceOperation>());
    check("MAPPING_DIRECTIONS", schema::<MappingDirection>());
    check("MAPPING_ORIGINS", schema::<MappingOrigin>());
    check("SEMANTIC_ENCODINGS", schema::<SemanticEncoding>());
    let coverage = schema::<DeclarationCoverage>();
    let (keys, _) = object(&coverage, &coverage).unwrap();
    assert_eq!(as_set(keys.keys()), as_set(list(&s["COVERAGE_KEYS"])), "COVERAGE_KEYS");
    assert_eq!(ProviderHandoverRejection::ALL.len(), {
        let r = schema::<ProviderHandoverRejection>();
        literals(&r, &r).unwrap().len()
    });
}
