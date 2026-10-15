//! Verification rules and evidence.
//!
//! *Declared* verification methods are part of the contract
//! (`integration.d.ts`: `VerificationMethod` and friends). *Evidence* is what a
//! runtime observed and is never published: `VerificationEvidence`
//! (`@appport/protocol`) and the runtime evidence records from `@appport/core`
//! (`evidence.d.ts`).
//!
//! The rule that matters most to an agent: only a **state** method can
//! *establish* that an effect happened. An **event** method can corroborate,
//! and its absence proves nothing. Likewise an absent runtime evidence record
//! shows nothing.

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::literal::{
    present, Corroborating, EventKind, InvocationEvidenceType, StateAccessEvidenceType, StateKind, True,
    VerificationEvidenceType,
};

/// `ValueRef`: a literal value, or a reference into the request input / response output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ValueRef {
    /// `{ "$input": "<pointer>" }`
    Input(InputRef),
    /// `{ "$output": "<pointer>" }`
    Output(OutputRef),
    /// Any other JSON value.
    Literal(Value),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputRef {
    #[serde(rename = "$input")]
    pub pointer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputRef {
    #[serde(rename = "$output")]
    pub pointer: String,
}

impl ValueRef {
    /// True for `$output` references (which need a response to resolve).
    pub fn needs_output(&self) -> bool {
        matches!(self, ValueRef::Output(_))
    }
}

/// A call to a declared observation capability (`via` on state methods and preconditions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProbeCall {
    pub capability: String,
    pub version: u32,
    pub input: IndexMap<String, ValueRef>,
}

/// `StateExpectation`: exactly one of `equals` / `exists`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StateExpectation {
    /// JSON Pointer into the probe's output.
    pub at: String,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub equals: Option<ValueRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<True>,
}

/// `StateVerification`: re-read state and compare. The only method that can establish an effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StateVerification {
    pub id: String,
    pub kind: StateKind,
    pub via: ProbeCall,
    pub expect: Vec<StateExpectation>,
}

/// `EventVerification`: look for an emitted event. Corroborates only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventVerification {
    pub id: String,
    pub kind: EventKind,
    pub event: String,
    /// JSON Pointer into the event payload → expected value.
    #[serde(rename = "match")]
    pub matches: IndexMap<String, ValueRef>,
    pub strength: Corroborating,
}

/// `VerificationMethod`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum VerificationMethod {
    State(StateVerification),
    Event(EventVerification),
}

/// How much a verification method can prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VerificationStrength {
    /// Can establish that the effect occurred (state methods).
    Establishes,
    /// Can only corroborate; absence proves nothing (event methods).
    Corroborates,
}

impl VerificationMethod {
    pub fn id(&self) -> &str {
        match self {
            Self::State(m) => &m.id,
            Self::Event(m) => &m.id,
        }
    }

    pub fn kind(&self) -> MethodKind {
        match self {
            Self::State(_) => MethodKind::State,
            Self::Event(_) => MethodKind::Event,
        }
    }

    pub fn strength(&self) -> VerificationStrength {
        match self {
            Self::State(_) => VerificationStrength::Establishes,
            Self::Event(_) => VerificationStrength::Corroborates,
        }
    }
}

/// `"state" | "event"` as used inside evidence records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MethodKind {
    State,
    Event,
}

/// `VerificationOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VerificationOutcome {
    Verified,
    Unverified,
    Unverifiable,
}

/// `MethodStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MethodStatus {
    Confirmed,
    Contradicted,
    Unavailable,
    NotRun,
}

/// A capability identity `{ name, version }` as used in evidence and handover records.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityId {
    pub name: String,
    pub version: u32,
}

/// `VerificationEvidence.methods[].probe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRecord {
    pub capability: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// `VerificationEvidence.methods[].mismatches[]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Mismatch {
    pub at: String,
    pub expected: Value,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub observed: Option<Value>,
}

/// `VerificationEvidence.methods[]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MethodEvidence {
    pub id: String,
    pub kind: MethodKind,
    pub status: MethodStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<ProbeRecord>,
    /// For a contradicted state method: where the observation disagreed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mismatches: Option<Vec<Mismatch>>,
    /// For an unavailable method: the error code that prevented it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// `VerificationEvidence`: that a declared method ran and what it found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VerificationEvidence {
    #[serde(rename = "type")]
    pub kind: VerificationEvidenceType,
    pub application: String,
    pub capability: CapabilityId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub checked_at: String,
    pub outcome: VerificationOutcome,
    pub methods: Vec<MethodEvidence>,
}

// ---------------------------------------------------------------------------
// Runtime evidence (@appport/core evidence.d.ts)
// ---------------------------------------------------------------------------

/// `InvocationOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvocationOutcome {
    Success,
    Failed,
    Cancelled,
    TimedOut,
    Refused,
}

/// `InvocationCallerEvidence.attribution`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Attribution {
    Capability,
    ApplicationOnly,
}

/// `InvocationCallerEvidence`. The capability is known only when supplied, never inferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvocationCallerEvidence {
    pub application: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<CapabilityId>,
    pub attribution: Attribution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// `InvocationEvidence.target`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvocationTarget {
    pub application: String,
    pub capability: CapabilityId,
}

/// `InvocationEvidence`: one mediated nested invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationEvidence {
    #[serde(rename = "type")]
    pub kind: InvocationEvidenceType,
    pub observed_at: String,
    pub invocation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_request_id: Option<String>,
    pub caller: InvocationCallerEvidence,
    pub target: InvocationTarget,
    pub outcome: InvocationOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `StateOperation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StateOperation {
    Read,
    Write,
    Delete,
    Query,
}

/// `StateAccessEvidence.resource`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "classification", rename_all = "lowercase")]
pub enum StateResourceClassification {
    /// The application declared this key belongs to a logical resource.
    Mapped { id: String },
    /// It did not.
    Unclassified,
}

/// `StateAccessEvidence.outcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StateAccessOutcome {
    Ok,
    Error,
}

/// `StateAccessEvidence`: one mediated state operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateAccessEvidence {
    #[serde(rename = "type")]
    pub kind: StateAccessEvidenceType,
    pub observed_at: String,
    /// Order operations were *started* within one journal.
    pub sequence: u64,
    pub application: String,
    pub invocation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<CapabilityId>,
    pub operation: StateOperation,
    pub resource: StateResourceClassification,
    pub outcome: StateAccessOutcome,
}

/// `RuntimeEvidence`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RuntimeEvidence {
    Invocation(InvocationEvidence),
    StateAccess(StateAccessEvidence),
}
