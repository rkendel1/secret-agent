//! Protocol constants, mirrored from the packages' runtime values.
//!
//! `tests/surface.rs` compares each of these with the value recorded in
//! `knowledge/appport-surface.json`, which `tools/appport-inspect` reads from
//! the packages' JavaScript (not their documentation).

/// The packages the knowledge module was derived from, and their versions.
pub const SOURCE_PACKAGES: [(&str, &str); 4] = [
    ("@appport/core", "1.2.0"),
    ("@appport/protocol", "1.1.0"),
    ("@appport/schema", "1.1.0"),
    ("@appport/testing", "1.1.3"),
];

/// `PROTOCOL_ID`.
pub const PROTOCOL_ID: &str = "appport/1";
/// `PROTOCOL_MAJOR`.
pub const PROTOCOL_MAJOR: u32 = 1;
/// `CONFORMANCE_LEVELS` (§76), lowest first.
pub const CONFORMANCE_LEVELS: [(&str, u8); 5] =
    [("core", 1), ("events", 2), ("streams", 3), ("sessions", 4), ("portableRuntime", 5)];
/// `RESERVED_NAME_SEGMENTS`: transport words that must never appear in a name.
pub const RESERVED_NAME_SEGMENTS: [&str; 12] = [
    "http",
    "https",
    "ws",
    "wss",
    "websocket",
    "webtransport",
    "ipc",
    "electron",
    "tauri",
    "rest",
    "grpc",
    "inprocess",
];
/// `JSON_SCHEMA_DIALECT`.
pub const JSON_SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";
/// `WELL_KNOWN_PATH`.
pub const WELL_KNOWN_PATH: &str = "/.well-known/appport";
/// `DEFAULT_HTTP_ENDPOINT`.
pub const DEFAULT_HTTP_ENDPOINT: &str = "/appport";
/// `DEFAULT_WS_ENDPOINT`.
pub const DEFAULT_WS_ENDPOINT: &str = "/appport/ws";
/// `DEFAULT_MANIFEST_PATH`.
pub const DEFAULT_MANIFEST_PATH: &str = "/appport/manifest";
/// `AUTHORIZATION_MODEL_VERSION`.
pub const AUTHORIZATION_MODEL_VERSION: u32 = 1;
/// `INTEGRATION_EXTENSION`: the `manifest.attributes` / `manifest.extensions` key.
pub const INTEGRATION_EXTENSION: &str = "appport.integration/1";

/// `SEMANTIC_KEYWORDS`.
pub struct SemanticKeywords {
    pub concept: &'static str,
    pub unit: &'static str,
    pub enum_concepts: &'static str,
    pub encoding: &'static str,
}

/// `SEMANTIC_KEYWORDS`: the `x-appport-*` schema keywords AppPort defines.
pub const SEMANTIC_KEYWORDS: SemanticKeywords = SemanticKeywords {
    concept: "x-appport-concept",
    unit: "x-appport-unit",
    enum_concepts: "x-appport-enum-concepts",
    encoding: "x-appport-encoding",
};

/// `SEMANTIC_ENCODINGS`.
pub const SEMANTIC_ENCODINGS: [&str; 3] = ["iso8601", "epoch-seconds", "epoch-milliseconds"];
/// `RETRY_CLASSES`.
pub const RETRY_CLASSES: [&str; 3] = ["safe", "unsafe", "after-verification"];
/// `FAILURE_EFFECTS`.
pub const FAILURE_EFFECTS: [&str; 3] = ["none", "possible", "unknown"];
/// `DEPENDENCY_RELATIONSHIPS`.
pub const DEPENDENCY_RELATIONSHIPS: [&str; 4] = ["invokes", "observes", "delegates", "consumes-event"];
/// `RESOURCE_KINDS`.
pub const RESOURCE_KINDS: [&str; 5] = ["entity", "collection", "counter", "document", "stream"];
/// `RESOURCE_OPERATIONS`.
pub const RESOURCE_OPERATIONS: [&str; 3] = ["create", "update", "delete"];
/// `COVERAGE_KEYS`.
pub const COVERAGE_KEYS: [&str; 4] = ["requires", "access", "preconditions", "failures"];
/// `MAPPING_DIRECTIONS`.
pub const MAPPING_DIRECTIONS: [&str; 2] = ["inbound", "outbound"];
/// `MAPPING_ORIGINS`.
pub const MAPPING_ORIGINS: [&str; 3] = ["provider", "third-party", "generated"];
/// `OPERATION_CAPABILITIES` (§47).
pub const OPERATION_CAPABILITIES: [(&str, &str); 3] =
    [("status", "operations.status"), ("cancel", "operations.cancel"), ("events", "operations.events")];

/// `@appport/core` `INVOCATION_EVIDENCE`.
pub const INVOCATION_EVIDENCE: &str = "appport.evidence.invocation/1";
/// `@appport/core` `STATE_ACCESS_EVIDENCE`.
pub const STATE_ACCESS_EVIDENCE: &str = "appport.evidence.state-access/1";
/// `@appport/core` `BUILTIN_CAPABILITY_NAMES`: reserved capabilities every application may serve.
pub const BUILTIN_CAPABILITY_NAMES: [&str; 2] = ["appport.manifest", "appport.ping"];
