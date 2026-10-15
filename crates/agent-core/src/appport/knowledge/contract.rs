//! The application contract: the manifest.
//!
//! Mirrors `AppPortManifest` and its parts from `@appport/protocol`
//! (`manifest.d.ts`, `materialization.d.ts`, `ui.d.ts`, `version.d.ts`).

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::capability::{CapabilityEntry, CapabilityVersion};
use super::constants::INTEGRATION_EXTENSION;
use super::integration::IntegrationDescriptor;
use super::literal::{ProtocolId, UiProtocolId};
use super::schema::Schema;

/// The application manifest (specification §30, §31, §40): the language-neutral
/// description of an AppPort application.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Manifest {
    pub protocol: ProtocolId,
    pub application: ApplicationDescriptor,
    pub capabilities: Vec<CapabilityEntry>,
    pub events: Vec<EventEntry>,
    pub transports: Vec<TransportDescriptor>,
    /// Highest conformance level this deployment implements (§76).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conformance: Option<ConformanceLevel>,
    /// Namespaced protocol extensions advertised by this deployment (§71).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    /// Non-sensitive deployment annotations. Carries the integration descriptor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialization: Option<MaterializationDeclaration>,
    /// Descriptive, not authoritative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiContribution>,
}

impl Manifest {
    /// Mirrors `findCapability`.
    pub fn capability(&self, name: &str) -> Option<&CapabilityEntry> {
        self.capabilities.iter().find(|c| c.name == name)
    }

    /// Mirrors `findCapabilityVersion`.
    pub fn capability_version(&self, name: &str, version: u32) -> Option<&CapabilityVersion> {
        self.capability(name)?.version(version)
    }

    /// Mirrors `findEvent`.
    pub fn event(&self, name: &str) -> Option<&EventEntry> {
        self.events.iter().find(|e| e.name == name)
    }

    /// Mirrors `hasTransport`.
    pub fn has_transport(&self, kind: &str) -> bool {
        self.transports.iter().any(|t| t.kind == kind)
    }

    /// Whether `extensions` advertises `extension`.
    pub fn advertises(&self, extension: &str) -> bool {
        self.extensions.as_ref().is_some_and(|e| e.iter().any(|x| x == extension))
    }

    /// Mirrors `getIntegration`: the `appport.integration/1` descriptor, if published.
    ///
    /// `Ok(None)` when absent (or not shaped like a descriptor, as in the JS
    /// helper); `Err` when it is present but does not parse as the published type.
    pub fn integration(&self) -> Result<Option<IntegrationDescriptor>, serde_json::Error> {
        let Some(value) = self.attributes.as_ref().and_then(|a| a.get(INTEGRATION_EXTENSION)) else {
            return Ok(None);
        };
        let has_capabilities = value.get("capabilities").is_some_and(Value::is_object);
        if !value.is_object() || !has_capabilities {
            return Ok(None);
        }
        serde_json::from_value(value.clone()).map(Some)
    }

    /// Every capability version as `(entry, definition)`, in manifest order.
    pub fn capability_versions(&self) -> impl Iterator<Item = (&CapabilityEntry, &CapabilityVersion)> {
        self.capabilities.iter().flat_map(|c| c.definitions.iter().map(move |d| (c, d)))
    }
}

/// `ApplicationDescriptor` (§41).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApplicationDescriptor {
    /// Stable, recommended reverse-domain identity.
    pub id: String,
    pub name: String,
    /// Application (not protocol) version.
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

/// `ConformanceLevel` (§76): 1 core, 2 events, 3 streams, 4 sessions, 5 portable runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "u8", into = "u8")]
pub struct ConformanceLevel(u8);

impl ConformanceLevel {
    pub const CORE: Self = Self(1);
    pub const EVENTS: Self = Self(2);
    pub const STREAMS: Self = Self(3);
    pub const SESSIONS: Self = Self(4);
    pub const PORTABLE_RUNTIME: Self = Self(5);

    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for ConformanceLevel {
    type Error = String;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if (1..=5).contains(&value) {
            Ok(Self(value))
        } else {
            Err(format!("conformance level must be 1..=5, got {value}"))
        }
    }
}

impl From<ConformanceLevel> for u8 {
    fn from(level: ConformanceLevel) -> u8 {
        level.0
    }
}

impl fmt::Display for ConformanceLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// `EventVersionEntry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EventVersion {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub payload_schema: Schema,
    /// Permissions a session must hold to observe this event.
    pub authorization: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
}

/// `EventManifestEntry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EventEntry {
    pub name: String,
    pub versions: Vec<u32>,
    pub latest_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub payload_schema: Schema,
    pub authorization: Vec<String>,
    pub definitions: Vec<EventVersion>,
}

/// `TransportDescriptor` (§40).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TransportDescriptor {
    /// `http`, `websocket`, `ipc`, `inprocess`, `webtransport`, …
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Map<String, Value>>,
}

/// `MaterializationDeclaration`: deployment-neutral targets an application supports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MaterializationDeclaration {
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Map<String, Value>>,
}

/// `UiCompositionRequirement`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum UiCompositionRequirement {
    Identity,
    Tenant,
    Application,
    Environment,
}

/// `UiProductIdentity`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiProductIdentity {
    pub id: String,
    pub version: String,
}

/// `UiSurface`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiSurface {
    pub id: String,
    pub title: String,
    pub route: String,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<String>>,
}

/// `UiNavigationContribution`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UiNavigationContribution {
    pub id: String,
    pub label: String,
    pub group: String,
    pub order: serde_json::Number,
    pub surface: String,
}

/// `UiContribution.composition`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiComposition {
    pub requires: Vec<UiCompositionRequirement>,
}

/// `UiContribution`: an optional, composable, descriptive UI contribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UiContribution {
    pub protocol: UiProtocolId,
    pub product: UiProductIdentity,
    pub surfaces: Vec<UiSurface>,
    pub navigation: Vec<UiNavigationContribution>,
    pub composition: UiComposition,
}
