//! Integration metadata: what an application's data and capabilities *mean*.
//!
//! Mirrors the integration descriptor from `@appport/protocol` (`integration.d.ts`),
//! published at `manifest.attributes["appport.integration/1"]`. Nothing here grants
//! authority and nothing here is executed.
//!
//! An absent optional field means *the contract does not say*. It is never read as
//! "no" (no approval needed, no dependencies, complete coverage): that rule is part
//! of the AppPort contract, and [`CapabilityView`](super::CapabilityView) reports
//! such gaps explicitly as unknowns.

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::literal::SelfOwner;
use super::verification::{ProbeCall, StateExpectation, VerificationMethod};

/// `IntegrationDescriptor`; `capabilities` keys are `name@version`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IntegrationDescriptor {
    pub capabilities: IndexMap<String, CapabilityIntegration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mappings: Option<Vec<MappingReference>>,
    /// Dependencies of the application as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Vec<Dependency>>,
    /// State resources the application reads, changes or is the system of record for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Vec<ResourceDeclaration>>,
}

impl IntegrationDescriptor {
    /// The entry for `name@version`.
    pub fn capability(&self, name: &str, version: u32) -> Option<&CapabilityIntegration> {
        self.capabilities.get(&super::rules::integration_key(name, version))
    }

    pub fn resource(&self, id: &str) -> Option<&ResourceDeclaration> {
        self.resources.as_deref().unwrap_or_default().iter().find(|r| r.id == id)
    }

    /// Mirrors `isEmptyDescriptor`.
    pub fn is_empty(&self) -> bool {
        self.capabilities.values().all(CapabilityIntegration::is_empty)
            && self.mappings.as_ref().map_or(true, Vec::is_empty)
            && self.requires.as_ref().map_or(true, Vec::is_empty)
            && self.resources.as_ref().map_or(true, Vec::is_empty)
    }
}

/// `CapabilityIntegration`: per capability-version meaning.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityIntegration {
    /// Descriptive prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// The domain authority the capability belongs to. A semantic identifier, not a grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Vec<EffectDescriptor>>,
    /// Declared ways to confirm the effect happened, using evidence rather than the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<Vec<VerificationMethod>>,
    /// What the capability relies on. The author's understanding, not a discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Vec<Dependency>>,
    /// Which logical state resources it reads or changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Vec<ResourceAccess>>,
    /// Whether the declared lists claim to be complete. Absent means unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<DeclarationCoverage>,
    /// Absent means the contract does not say — never "no approval required".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preconditions: Option<Vec<Precondition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failures: Option<Vec<FailureDeclaration>>,
}

impl CapabilityIntegration {
    /// Mirrors `isEmptyIntegration`.
    pub fn is_empty(&self) -> bool {
        fn none<T>(list: &Option<Vec<T>>) -> bool {
            list.as_ref().map_or(true, Vec::is_empty)
        }
        self.purpose.is_none()
            && self.authority.is_none()
            && none(&self.effects)
            && none(&self.verify)
            && none(&self.requires)
            && none(&self.access)
            && none(&self.preconditions)
            && none(&self.failures)
            && self.approval.is_none()
            && self.coverage.is_none()
    }
}

/// `EffectDescriptor`: `type` is a semantic identifier, `description` is prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EffectDescriptor {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `ApprovalRequirement`: an approval a scope cannot express. AppPort never collects it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRequirement {
    pub required: bool,
    /// A role, not a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approver: Option<String>,
}

/// `Precondition.require`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PreconditionRequirement {
    /// The probed state must exist.
    Found,
    /// The probed state must not exist (e.g. no duplicate).
    NotFound,
}

/// `Precondition`: a condition on existing state, checked by re-reading it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Precondition {
    pub id: String,
    pub via: ProbeCall,
    pub require: PreconditionRequirement,
    /// Evaluated only when `require` is `found`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<Vec<StateExpectation>>,
}

/// `RetryClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RetryClass {
    /// Re-sending cannot duplicate the effect.
    Safe,
    /// Do not re-send.
    Unsafe,
    /// Run the declared verification first.
    AfterVerification,
}

/// `FailureEffect`: whether state may have changed despite the failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FailureEffect {
    None,
    Possible,
    Unknown,
}

/// `FailureDeclaration`: what a failure code means for retrying and for state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FailureDeclaration {
    pub code: String,
    pub retry: RetryClass,
    pub effect: FailureEffect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `DependencyRelationship`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyRelationship {
    /// Calls the target while handling a request; the target may have effects.
    Invokes,
    /// Calls the target only to read; the target must be an observation capability.
    Observes,
    /// Hands work to the target without waiting for its result.
    Delegates,
    /// Behaviour depends on an event published elsewhere.
    ConsumesEvent,
}

/// `DependencyTarget`: a capability (`name` or `name@version`) or an event.
/// `application` absent means the same application.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum DependencyTarget {
    Capability(CapabilityTarget),
    Event(EventTarget),
}

impl DependencyTarget {
    pub fn application(&self) -> Option<&str> {
        match self {
            Self::Capability(t) => t.application.as_deref(),
            Self::Event(t) => t.application.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    pub capability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    pub event: String,
}

/// `Dependency`: how one capability (or the application) depends on another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    /// Unique within its list.
    pub id: String,
    pub relationship: DependencyRelationship,
    pub target: DependencyTarget,
    /// `false`: the dependent degrades or continues without it.
    pub required: bool,
    /// Required when the target is not pinned to a version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_constraint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

/// `RESOURCE_KINDS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    Entity,
    Collection,
    Counter,
    Document,
    Stream,
}

/// `ResourceDeclaration.owner`: who is the system of record.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ResourceOwner {
    /// `"self"`
    This(SelfOwner),
    /// `{ application }`
    Application(OwnerApplication),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerApplication {
    pub application: String,
}

/// `ResourceDeclaration`: a logical state resource. Identity, meaning and ownership
/// are separate claims; physical storage is never published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceDeclaration {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ResourceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concept: Option<String>,
    /// Absent: not stated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ResourceOwner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `ResourceAccess.mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccessMode {
    Reads,
    Changes,
}

/// `RESOURCE_OPERATIONS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResourceOperation {
    Create,
    Update,
    Delete,
}

/// `ResourceAccess`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceAccess {
    pub resource: String,
    pub mode: AccessMode,
    /// Only for `changes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Vec<ResourceOperation>>,
}

/// `Coverage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Coverage {
    Complete,
    Partial,
}

/// `DeclarationCoverage`. Each absent key means *unknown*, not complete.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeclarationCoverage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Coverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Coverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preconditions: Option<Coverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failures: Option<Coverage>,
}

/// `MappingDirection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MappingDirection {
    /// The artifact produces this application's capability input.
    Inbound,
    /// The artifact consumes the capability's output.
    Outbound,
}

/// `MappingOrigin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MappingOrigin {
    Provider,
    ThirdParty,
    Generated,
}

/// `MappingReference.peer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MappingPeer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
}

/// `MappingReference`: `(id, version)` is the identity, `digest` pins the bytes,
/// `uri` values are location hints only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MappingReference {
    pub id: String,
    pub version: u32,
    pub digest: String,
    pub direction: MappingDirection,
    /// The capability this mapping serves, as `name@version`.
    #[serde(rename = "for")]
    pub serves: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<MappingPeer>,
    pub origin: MappingOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<Vec<String>>,
    /// The artifact itself when embedded; its digest must equal `digest`.
    #[serde(default, deserialize_with = "super::literal::present", skip_serializing_if = "Option::is_none")]
    pub artifact: Option<Value>,
}

/// `SemanticEncoding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticEncoding {
    Iso8601,
    EpochSeconds,
    EpochMilliseconds,
}

/// `SemanticAnnotations`: the typed form of the `x-appport-*` schema keywords.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SemanticAnnotations {
    /// Absolute IRI naming the business concept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concept: Option<String>,
    /// UCUM unit code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Enum value → concept IRI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enum_concepts: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<SemanticEncoding>,
}

/// `SemanticIssue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SemanticIssue {
    /// JSON Pointer to the offending schema node.
    pub path: String,
    pub message: String,
}

/// `IntegrationIssue.level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IssueLevel {
    Error,
    Warning,
}

/// `IntegrationIssue`, as reported by the protocol's linters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IntegrationIssue {
    pub level: IssueLevel,
    pub path: String,
    pub message: String,
}
