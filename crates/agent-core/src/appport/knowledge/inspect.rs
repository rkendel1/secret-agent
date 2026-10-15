//! Contract inspection: a small, precise, model-ready view of one capability version.
//!
//! A [`CapabilityView`] answers what an agent needs before calling or changing a
//! capability — its inputs and outputs (with required fields), authorization,
//! effect, evidence requirements, preconditions and failure semantics — and lists
//! what the contract *does not say* in `unknowns`, so nothing absent is mistaken
//! for "no".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::capability::{CapabilityEffect, CapabilityKind, ConcurrencyPolicy, IdempotencySupport};
use super::contract::Manifest;
use super::integration::{
    ApprovalRequirement, Dependency, EffectDescriptor, FailureDeclaration, IntegrationDescriptor, Precondition,
    ResourceAccess,
};
use super::rules::integration_key;
use super::schema::{AdditionalProperties, Schema, SchemaType};
use super::verification::{MethodKind, VerificationMethod, VerificationStrength};

/// One field of an input or output, flattened to a JSON Pointer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FieldView {
    /// RFC 6901 pointer; `*` addresses every array element.
    pub pointer: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<SchemaType>,
    /// Required at every level from the root: a caller must always send it /
    /// a consumer always receives it.
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "enum", default, skip_serializing_if = "Option::is_none")]
    pub enumeration: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concept: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

/// A declared verification method, summarized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MethodView {
    pub id: String,
    pub kind: MethodKind,
    pub strength: VerificationStrength,
    /// `name@version` of the observation capability a state method re-reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<String>,
    /// The event an event method looks for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
}

/// What evidence can confirm the capability's effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceRequirements {
    pub methods: Vec<MethodView>,
    /// True when at least one declared method can *establish* the effect (a state method).
    pub establishable: bool,
}

/// The model-ready view of one capability version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityView {
    /// `name@version`.
    pub capability: String,
    pub kind: CapabilityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deprecated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<CapabilityEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectDescriptor>,
    pub authorization: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency: Option<IdempotencySupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<ConcurrencyPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    pub inputs: Vec<FieldView>,
    pub outputs: Vec<FieldView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emits: Vec<String>,
    pub evidence: EvidenceRequirements,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<Precondition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<FailureDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<Dependency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access: Vec<ResourceAccess>,
    /// What the contract does not state. Each entry is *unknown*, never "no".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknowns: Vec<String>,
}

/// Inspects `name@version`. `None` when the manifest does not declare it.
pub fn inspect_capability(
    manifest: &Manifest,
    integration: Option<&IntegrationDescriptor>,
    name: &str,
    version: u32,
) -> Option<CapabilityView> {
    let definition = manifest.capability_version(name, version)?;
    let entry = integration.and_then(|i| i.capability(name, version));

    let methods: Vec<MethodView> = entry
        .and_then(|e| e.verify.as_ref())
        .into_iter()
        .flatten()
        .map(|method| match method {
            VerificationMethod::State(m) => MethodView {
                id: m.id.clone(),
                kind: MethodKind::State,
                strength: VerificationStrength::Establishes,
                probe: Some(integration_key(&m.via.capability, m.via.version)),
                event: None,
            },
            VerificationMethod::Event(m) => MethodView {
                id: m.id.clone(),
                kind: MethodKind::Event,
                strength: VerificationStrength::Corroborates,
                probe: None,
                event: Some(m.event.clone()),
            },
        })
        .collect();
    let establishable = methods.iter().any(|m| m.strength == VerificationStrength::Establishes);

    let mut unknowns = Vec::new();
    if definition.effect.is_none() {
        unknowns.push("effect".to_string());
    }
    if definition.idempotency.is_none() {
        unknowns.push("idempotency".to_string());
    }
    match entry {
        None => unknowns.push("integration".to_string()),
        Some(entry) => {
            if entry.approval.is_none() {
                unknowns.push("approval".to_string());
            }
            let coverage = entry.coverage.unwrap_or_default();
            for (key, value) in [
                ("requires", coverage.requires),
                ("access", coverage.access),
                ("preconditions", coverage.preconditions),
                ("failures", coverage.failures),
            ] {
                if value.is_none() {
                    unknowns.push(format!("coverage.{key}"));
                }
            }
        }
    }
    if methods.is_empty() && definition.effect != Some(CapabilityEffect::Observation) {
        unknowns.push("verification".to_string());
    }

    Some(CapabilityView {
        capability: integration_key(name, version),
        kind: definition.kind,
        description: definition.description.clone(),
        deprecated: definition.is_deprecated(),
        purpose: entry.and_then(|e| e.purpose.clone()),
        authority: entry.and_then(|e| e.authority.clone()),
        effect: definition.effect,
        effects: list(entry.and_then(|e| e.effects.as_ref())),
        authorization: definition.authorization.clone(),
        approval: entry.and_then(|e| e.approval.clone()),
        idempotency: definition.idempotency,
        concurrency: definition.concurrency.clone(),
        timeout_ms: definition.timeout_ms,
        inputs: fields(&definition.input_schema),
        outputs: fields(&definition.output_schema),
        emits: definition.emitted_events().to_vec(),
        evidence: EvidenceRequirements { methods, establishable },
        preconditions: list(entry.and_then(|e| e.preconditions.as_ref())),
        failures: list(entry.and_then(|e| e.failures.as_ref())),
        requires: list(entry.and_then(|e| e.requires.as_ref())),
        access: list(entry.and_then(|e| e.access.as_ref())),
        unknowns,
    })
}

fn list<T: Clone>(v: Option<&Vec<T>>) -> Vec<T> {
    v.cloned().unwrap_or_default()
}

const MAX_DEPTH: usize = 8;

/// Flattens a schema into fields. Follows `properties`, `items` (as `*`), local
/// `#/$defs/…` references and single-variant nullable unions. Other unions are
/// reported as one field with the union of their types.
pub fn fields(root: &Schema) -> Vec<FieldView> {
    let mut out = Vec::new();
    walk(root, root, "", true, 0, &mut out);
    out
}

fn resolve<'a>(root: &'a Schema, schema: &'a Schema) -> &'a Schema {
    let mut current = schema;
    for _ in 0..MAX_DEPTH {
        let Some(name) = current.reference.as_deref().and_then(|r| r.strip_prefix("#/$defs/")) else { break };
        match root.defs.as_ref().and_then(|d| d.get(&unescape(name))) {
            Some(target) => current = target,
            None => break,
        }
    }
    current
}

/// The single non-null variant of a nullable union, if that is what this is.
fn nullable_inner(schema: &Schema) -> Option<&Schema> {
    let variants = schema.any_of.as_ref().or(schema.one_of.as_ref())?;
    let non_null: Vec<&Schema> =
        variants.iter().filter(|v| v.types.as_ref().map_or(true, |t| t.as_slice() != [SchemaType::Null])).collect();
    (non_null.len() == 1 && variants.len() == 2).then(|| non_null[0])
}

fn types_of(root: &Schema, schema: &Schema) -> Vec<SchemaType> {
    let mut types: Vec<SchemaType> = schema.types.as_ref().map(|t| t.as_slice().to_vec()).unwrap_or_default();
    for variant in schema.any_of.iter().chain(schema.one_of.iter()).flatten() {
        types.extend(types_of(root, resolve(root, variant)));
    }
    types.sort();
    types.dedup();
    types
}

fn walk(root: &Schema, schema: &Schema, pointer: &str, required: bool, depth: usize, out: &mut Vec<FieldView>) {
    if depth > MAX_DEPTH {
        return;
    }
    let schema = resolve(root, schema);
    let body = nullable_inner(schema).map(|inner| resolve(root, inner)).unwrap_or(schema);
    if let Some(properties) = &body.properties {
        for (name, child) in properties {
            let child_pointer = format!("{pointer}/{}", escape(name));
            let child_required = required && body.requires(name);
            out.push(view(root, child, &child_pointer, child_required));
            walk(root, child, &child_pointer, child_required, depth + 1, out);
        }
    }
    if let Some(AdditionalProperties::Schema(extra)) = &body.additional_properties {
        if extra.types.is_some() || extra.properties.is_some() {
            let child_pointer = format!("{pointer}/*");
            out.push(view(root, extra, &child_pointer, false));
            walk(root, extra, &child_pointer, false, depth + 1, out);
        }
    }
    if let Some(items) = &body.items {
        let child_pointer = format!("{pointer}/*");
        let resolved = resolve(root, items);
        if resolved.properties.is_some() || nullable_inner(resolved).is_some_and(|i| i.properties.is_some()) {
            walk(root, items, &child_pointer, required, depth + 1, out);
        } else {
            out.push(view(root, items, &child_pointer, required));
        }
    }
}

fn view(root: &Schema, schema: &Schema, pointer: &str, required: bool) -> FieldView {
    let schema = resolve(root, schema);
    let body = nullable_inner(schema).map(|inner| resolve(root, inner)).unwrap_or(schema);
    let annotation = |key: &str| {
        body.extensions.get(key).or_else(|| schema.extensions.get(key)).and_then(Value::as_str).map(str::to_string)
    };
    FieldView {
        pointer: pointer.to_string(),
        types: types_of(root, schema),
        required,
        description: body.description.clone().or_else(|| schema.description.clone()),
        enumeration: body.enumeration.clone(),
        format: body.format.clone(),
        concept: annotation("x-appport-concept"),
        unit: annotation("x-appport-unit"),
        encoding: annotation("x-appport-encoding"),
    }
}

fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}
