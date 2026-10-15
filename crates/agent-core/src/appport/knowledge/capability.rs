//! Capabilities: what an application can be asked to do.
//!
//! Mirrors `CapabilityManifestEntry`, `CapabilityVersionEntry` and the
//! capability policy types from `@appport/protocol` (`manifest.d.ts`,
//! `authorization.d.ts`, `operations.d.ts`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::literal::{False, True};
use super::schema::Schema;

/// `CapabilityKind`: a single response, or a stream of values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityKind {
    Request,
    Stream,
}

/// `CapabilityEffect`: whether a capability only observes or may change state.
///
/// In a manifest the effect is optional. Absent means *the author did not say*,
/// never "observation": the runtime's internal default is deliberately not published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityEffect {
    Observation,
    Consequential,
}

/// `IdempotencySupport` (specification §44).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IdempotencySupport {
    Supported,
    Unsupported,
}

/// `ConcurrencyPolicy` (specification §45).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum ConcurrencyPolicy {
    Parallel,
    Serialized,
    Keyed { key: String },
}

/// `OperationStatus` for long-running operations (specification §47).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OperationStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl OperationStatus {
    /// Mirrors `isTerminalStatus`.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// `AuthorizationResource`: the resource shape an authorization decision uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AuthorizationResource {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Map<String, Value>>,
}

/// `AuthorizationRequirement`: the exact authorization semantics of a capability version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AuthorizationRequirement {
    /// `{ required: true, scopes, resource? }`
    Scoped(ScopedAuthorization),
    /// `{ required: false, public: true }`
    Public(PublicAuthorization),
}

/// The `required: true` arm of [`AuthorizationRequirement`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScopedAuthorization {
    pub required: True,
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<AuthorizationResource>,
}

/// The `required: false` arm of [`AuthorizationRequirement`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicAuthorization {
    pub required: False,
    pub public: True,
}

/// `CapabilityVersionEntry`: one version of a capability — everything needed to call it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityVersion {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub kind: CapabilityKind,
    pub input_schema: Schema,
    pub output_schema: Schema,
    /// Permissions required to invoke this version (§34).
    pub authorization: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<CapabilityEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_contract: Option<AuthorizationRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency: Option<IdempotencySupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<ConcurrencyPolicy>,
    /// Server-side maximum request duration in milliseconds (§46).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    /// Events this version may emit (§17).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emits: Option<Vec<String>>,
}

impl CapabilityVersion {
    pub fn is_deprecated(&self) -> bool {
        self.deprecated == Some(true)
    }

    pub fn emitted_events(&self) -> &[String] {
        self.emits.as_deref().unwrap_or_default()
    }
}

/// `CapabilityManifestEntry`: a capability as published in the manifest.
///
/// The top-level `input_schema`, `output_schema` and `authorization` mirror the
/// newest non-deprecated version; `definitions` carries every supported version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityEntry {
    pub name: String,
    pub versions: Vec<u32>,
    pub latest_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub kind: CapabilityKind,
    pub input_schema: Schema,
    pub output_schema: Schema,
    pub authorization: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<CapabilityEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_contract: Option<AuthorizationRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency: Option<IdempotencySupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<ConcurrencyPolicy>,
    pub definitions: Vec<CapabilityVersion>,
}

impl CapabilityEntry {
    /// One version's definition.
    pub fn version(&self, version: u32) -> Option<&CapabilityVersion> {
        self.definitions.iter().find(|d| d.version == version)
    }

    /// The definition the top-level fields mirror: newest non-deprecated, else newest.
    pub fn current(&self) -> Option<&CapabilityVersion> {
        self.definitions
            .iter()
            .filter(|d| !d.is_deprecated())
            .max_by_key(|d| d.version)
            .or_else(|| self.definitions.iter().max_by_key(|d| d.version))
    }
}
