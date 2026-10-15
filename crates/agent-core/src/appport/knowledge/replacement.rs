//! Replacement constraints: what must hold for one provider of a capability to
//! replace another.
//!
//! Two sources in the packages:
//!
//! - **Compatibility** (`@appport/protocol` `compatibility.d.ts`, spec §43): which
//!   contract changes break a consumer. Consumers *send* inputs and *receive*
//!   outputs, so narrowing inputs or widening outputs breaks them; enforceable
//!   behaviour and semantic identifiers must not change; prose may.
//! - **Provider handover** (`@appport/core` `registration.d.ts`,
//!   `application-lifecycle.d.ts`): the gates an atomic handover enforces, each
//!   with the rejection code it fails with.
//!
//! [`ReplacementBoundary`] combines both for one capability version, with the
//! graph context (who depends on it, what state it changes) from the contract.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::capability::{CapabilityEffect, IdempotencySupport};
use super::contract::Manifest;
use super::graph::{id, EdgeKind, Graph};
use super::integration::IntegrationDescriptor;
use super::literal::{Active, Registered, Retired};
use super::rules::{digest_of, integration_key};
use super::verification::{CapabilityId, VerificationStrength};

// ---------------------------------------------------------------------------
// Published types
// ---------------------------------------------------------------------------

/// `ChangeSeverity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeSeverity {
    Breaking,
    NonBreaking,
}

/// `ManifestChange`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ManifestChange {
    pub severity: ChangeSeverity,
    /// Machine-readable kind, e.g. `capability.removed`.
    pub kind: String,
    /// Where the change applies, e.g. `documents.save@1 input.content`.
    pub target: String,
    pub message: String,
}

/// `CompatibilityReport` (as produced by `compareManifests`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityReport {
    pub compatible: bool,
    pub breaking: Vec<ManifestChange>,
    pub non_breaking: Vec<ManifestChange>,
}

/// `ProviderRegistrationExpectation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRegistrationExpectation {
    pub id: String,
    pub registration_id: String,
    pub fingerprint: String,
}

/// `ProviderHandoverRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderHandoverRequest {
    pub capability: CapabilityId,
    /// Expected to be authoritative now: `active`, this exact registration.
    pub from: ProviderRegistrationExpectation,
    /// The prepared replacement: `registered`, not promoted, under its own id.
    pub to: ProviderRegistrationExpectation,
}

/// `ProviderHandoverRejection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderHandoverRejection {
    AtomicStoreUnsupported,
    SameProvider,
    FromNotFound,
    FromNotCurrent,
    FromNotActive,
    ToNotFound,
    ToNotCurrent,
    ToNotRegistered,
    CapabilityNotProvided,
    ContractMismatch,
    OtherActiveProvider,
    Conflict,
    GatewayUnsupported,
    ToNotValidated,
    PromotionChecksFailed,
    ReplacementNotAuthorized,
}

impl ProviderHandoverRejection {
    pub const ALL: [Self; 16] = [
        Self::AtomicStoreUnsupported,
        Self::SameProvider,
        Self::FromNotFound,
        Self::FromNotCurrent,
        Self::FromNotActive,
        Self::ToNotFound,
        Self::ToNotCurrent,
        Self::ToNotRegistered,
        Self::CapabilityNotProvided,
        Self::ContractMismatch,
        Self::OtherActiveProvider,
        Self::Conflict,
        Self::GatewayUnsupported,
        Self::ToNotValidated,
        Self::PromotionChecksFailed,
        Self::ReplacementNotAuthorized,
    ];
}

/// `ProviderHandoverRecord.from`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandoverSource {
    #[serde(flatten)]
    pub registration: ProviderRegistrationExpectation,
    pub status_before: Active,
    pub status_after: Retired,
    pub revision_after: u64,
}

/// `ProviderHandoverRecord.to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandoverTarget {
    #[serde(flatten)]
    pub registration: ProviderRegistrationExpectation,
    pub status_before: Registered,
    pub status_after: Active,
    pub revision_after: u64,
}

/// `ProviderHandoverRecord`: which registrations were authoritative before and after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHandoverRecord {
    pub handover_id: String,
    pub capability: CapabilityId,
    pub from: HandoverSource,
    pub to: HandoverTarget,
    pub at: String,
}

// ---------------------------------------------------------------------------
// Derived: the replacement boundary of one capability version
// ---------------------------------------------------------------------------

/// One gate the handover enforces, from `ApplicationLifecycleOperations.handover`
/// and `ApplicationRegistry.handover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct HandoverGate {
    pub id: &'static str,
    pub rule: &'static str,
    /// The rejection a failing gate produces. The source stays authoritative.
    pub rejection: ProviderHandoverRejection,
}

/// The gates, in the order the lifecycle documents them.
pub const HANDOVER_GATES: [HandoverGate; 8] = [
    HandoverGate { id: "distinct-provider", rule: "the replacement is a different provider, under its own id", rejection: ProviderHandoverRejection::SameProvider },
    HandoverGate { id: "source-active", rule: "the source provider is ACTIVE and is the exact expected registration", rejection: ProviderHandoverRejection::FromNotActive },
    HandoverGate { id: "target-registered", rule: "the replacement is REGISTERED (not promoted) and is the exact expected registration", rejection: ProviderHandoverRejection::ToNotRegistered },
    HandoverGate { id: "provides-capability", rule: "the replacement provides the capability version", rejection: ProviderHandoverRejection::CapabilityNotProvided },
    HandoverGate { id: "contract-match", rule: "the replacement's contract for the capability matches the source's", rejection: ProviderHandoverRejection::ContractMismatch },
    HandoverGate { id: "validated", rule: "the replacement's pending revision has a passing validation for its exact fingerprint", rejection: ProviderHandoverRejection::ToNotValidated },
    HandoverGate { id: "promotion-checks", rule: "the gateway's promotion checks pass for the replacement now", rejection: ProviderHandoverRejection::PromotionChecksFailed },
    HandoverGate { id: "consumer-grants", rule: "every consumer granted the capability from the source holds its own current grant for the replacement; no grant is copied", rejection: ProviderHandoverRejection::ReplacementNotAuthorized },
];

/// A compatibility constraint a replacement must satisfy (spec §43).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Constraint {
    /// Stable code, e.g. `input.accept-no-less`.
    pub code: String,
    pub rule: String,
}

/// A local dependent, i.e. something in this contract that would break with the capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Dependent {
    pub node: String,
    pub relationship: EdgeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

/// How a replacement can be confirmed to behave, from the declared verification methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationHook {
    pub method: String,
    pub strength: VerificationStrength,
}

/// Everything a replacement of one capability version must respect.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplacementBoundary {
    pub capability: CapabilityId,
    /// Digest of the capability version entry together with its integration entry.
    /// Equal pins mean the replacement publishes an identical contract for it.
    pub contract_pin: String,
    pub constraints: Vec<Constraint>,
    /// Things in this contract that depend on the capability.
    pub dependents: Vec<Dependent>,
    /// The capability's own outgoing dependencies and data flows (graph edges).
    pub reaches: Vec<Dependent>,
    pub verification: Vec<VerificationHook>,
    pub handover: Vec<HandoverGate>,
}

impl ReplacementBoundary {
    /// Derives the boundary for `name@version`. `None` when the manifest does not declare it.
    pub fn derive(
        manifest: &Manifest,
        integration: Option<&IntegrationDescriptor>,
        graph: &Graph,
        name: &str,
        version: u32,
    ) -> Option<ReplacementBoundary> {
        let definition = manifest.capability_version(name, version)?;
        let entry = integration.and_then(|i| i.capability(name, version));
        let pin = digest_of(&json!({
            "capability": serde_json::to_value(definition).ok()?,
            "integration": serde_json::to_value(entry).ok()?,
        }));

        let c = |code: &str, rule: String| Constraint { code: code.into(), rule };
        let mut constraints = vec![
            c("identity", format!("serve {} under the same name and version", integration_key(name, version))),
            c("kind", format!("remain a {} capability", serde_json::to_value(definition.kind).ok()?.as_str()?)),
            c("input.accept-no-less", "accept every input the current input schema accepts: add no required field, remove no field, change no field type, narrow no enum, bound or pattern".into()),
            c("output.return-no-more", "return only what the current output schema allows: remove no field consumers receive, change no field type, widen no enum or bound".into()),
            c("semantics.identifiers", "keep every x-appport-* annotation on existing fields unchanged; new annotations are allowed".into()),
        ];
        match definition.effect {
            Some(effect) => constraints.push(c("effect", format!("keep the declared effect `{}`", match effect {
                CapabilityEffect::Observation => "observation",
                CapabilityEffect::Consequential => "consequential",
            }))),
            None => constraints.push(c("effect.unknown", "the current contract does not declare an effect; a replacement that declares one changes what agents may assume".into())),
        }
        if definition.idempotency == Some(IdempotencySupport::Supported) {
            constraints.push(c("idempotency", "keep idempotency `supported`".into()));
        }
        constraints.push(c(
            "authorization.no-tightening",
            if definition.authorization.is_empty() {
                "require no permission".into()
            } else {
                format!("require no permission beyond {}", definition.authorization.join(", "))
            },
        ));
        if !definition.emitted_events().is_empty() {
            constraints.push(c("events", format!("keep emitting {}", definition.emitted_events().join(", "))));
        }
        if let Some(entry) = entry {
            if let Some(authority) = &entry.authority {
                constraints.push(c("integration.authority", format!("keep the authority `{authority}`")));
            }
            let effects: Vec<&str> = entry.effects.iter().flatten().map(|e| e.kind.as_str()).collect();
            if !effects.is_empty() {
                constraints
                    .push(c("integration.effects", format!("keep the declared effect types {}", effects.join(", "))));
            }
        }

        let node = id::capability(&manifest.application.id, name, version);
        let to_dependent = |e: &super::graph::Edge, other: &str| Dependent {
            node: other.to_string(),
            relationship: e.kind,
            required: e.required,
            via: e.via.clone(),
        };
        let dependents =
            graph.incoming(&node).filter(|e| e.kind != EdgeKind::Declares).map(|e| to_dependent(e, &e.from)).collect();
        let reaches = graph.outgoing(&node).map(|e| to_dependent(e, &e.to)).collect();
        let verification = entry
            .and_then(|e| e.verify.as_ref())
            .map(|methods| {
                methods
                    .iter()
                    .map(|m| VerificationHook { method: m.id().to_string(), strength: m.strength() })
                    .collect()
            })
            .unwrap_or_default();

        Some(ReplacementBoundary {
            capability: CapabilityId { name: name.to_string(), version },
            contract_pin: pin,
            constraints,
            dependents,
            reaches,
            verification,
            handover: HANDOVER_GATES.to_vec(),
        })
    }
}
