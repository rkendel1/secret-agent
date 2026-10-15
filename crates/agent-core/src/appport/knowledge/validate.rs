//! Structural validation of a manifest against well-defined rules.
//!
//! Each rule cites where it comes from in the packages. These are the checks the
//! agent can make without a model and without running anything. The protocol's
//! own cross-reference linters (`lintIntegration`, `lintAgentContract`,
//! `lintDependencyModel`) are not ported in this PR; their results on the
//! fixtures are recorded so a later port can be checked against them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::capability::AuthorizationRequirement;
use super::constants::INTEGRATION_EXTENSION;
use super::contract::Manifest;
use super::integration::{DependencyTarget, IntegrationDescriptor, IssueLevel};
use super::rules::{
    digest_of, is_digest, is_valid_application_id, is_valid_permission, is_version_constraint, parse_dependency_ref,
    validate_name, validate_semantic_annotations,
};
use super::schema::Schema;
use super::verification::VerificationMethod;

/// One finding. `rule` is a stable code an agent (or a model) can key on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Issue {
    pub level: IssueLevel,
    pub rule: String,
    pub path: String,
    pub message: String,
}

impl Issue {
    pub fn is_error(&self) -> bool {
        self.level == IssueLevel::Error
    }
}

struct Report(Vec<Issue>);

impl Report {
    fn push(&mut self, level: IssueLevel, rule: &str, path: impl Into<String>, message: impl Into<String>) {
        self.0.push(Issue { level, rule: rule.into(), path: path.into(), message: message.into() });
    }
    fn error(&mut self, rule: &str, path: impl Into<String>, message: impl Into<String>) {
        self.push(IssueLevel::Error, rule, path, message);
    }
    fn warning(&mut self, rule: &str, path: impl Into<String>, message: impl Into<String>) {
        self.push(IssueLevel::Warning, rule, path, message);
    }
}

/// Validates a parsed manifest. Reports every problem, in a deterministic order.
pub fn validate_manifest(manifest: &Manifest) -> Vec<Issue> {
    let mut r = Report(Vec::new());

    // §41 application identity (names.js isValidApplicationId).
    if !is_valid_application_id(&manifest.application.id) {
        r.error(
            "application.id",
            "application.id",
            format!("\"{}\" is not a valid reverse-domain application id", manifest.application.id),
        );
    }

    // Capabilities (manifest.d.ts, names.js).
    let mut seen = std::collections::BTreeSet::new();
    for (index, entry) in manifest.capabilities.iter().enumerate() {
        let path = format!("capabilities[{index}]");
        let name = validate_name(&entry.name);
        if !name.valid {
            r.error("name", format!("{path}.name"), format!("\"{}\": {}", entry.name, name.reason.unwrap_or_default()));
        }
        if !seen.insert(entry.name.as_str()) {
            r.error("capability.duplicate", &path, format!("capability \"{}\" is declared more than once", entry.name));
        }
        check_versions(
            &mut r,
            &path,
            &entry.name,
            &entry.versions,
            entry.latest_version,
            entry.definitions.iter().map(|d| d.version),
        );
        for permission in &entry.authorization {
            check_permission(&mut r, &format!("{path}.authorization"), permission);
        }

        // "Top-level fields mirror the newest non-deprecated version" (CapabilityManifestEntry docs).
        if let Some(current) = entry.current() {
            let mirrors = entry.kind == current.kind
                && entry.input_schema == current.input_schema
                && entry.output_schema == current.output_schema
                && entry.authorization == current.authorization;
            if !mirrors {
                r.error(
                    "capability.mirror",
                    &path,
                    format!("top-level fields of \"{}\" do not mirror version {}", entry.name, current.version),
                );
            }
        }

        for definition in &entry.definitions {
            let at = format!("{}@{}", entry.name, definition.version);
            for permission in &definition.authorization {
                check_permission(&mut r, &format!("{at} authorization"), permission);
            }
            check_authorization_contract(
                &mut r,
                &at,
                &definition.authorization,
                definition.authorization_contract.as_ref(),
            );
            for event in definition.emitted_events() {
                let valid = validate_name(event);
                if !valid.valid {
                    r.error(
                        "name",
                        format!("{at} emits"),
                        format!("\"{event}\": {}", valid.reason.unwrap_or_default()),
                    );
                } else if manifest.event(event).is_none() {
                    r.warning(
                        "emits.undeclared",
                        format!("{at} emits"),
                        format!("emits \"{event}\", which this manifest does not declare"),
                    );
                }
            }
            check_schema(&mut r, &format!("{at} input"), &definition.input_schema);
            check_schema(&mut r, &format!("{at} output"), &definition.output_schema);
        }
    }

    // Events.
    let mut seen = std::collections::BTreeSet::new();
    for (index, entry) in manifest.events.iter().enumerate() {
        let path = format!("events[{index}]");
        let name = validate_name(&entry.name);
        if !name.valid {
            r.error("name", format!("{path}.name"), format!("\"{}\": {}", entry.name, name.reason.unwrap_or_default()));
        }
        if !seen.insert(entry.name.as_str()) {
            r.error("event.duplicate", &path, format!("event \"{}\" is declared more than once", entry.name));
        }
        check_versions(
            &mut r,
            &path,
            &entry.name,
            &entry.versions,
            entry.latest_version,
            entry.definitions.iter().map(|d| d.version),
        );
        for definition in &entry.definitions {
            for permission in &definition.authorization {
                check_permission(&mut r, &format!("{}@{} authorization", entry.name, definition.version), permission);
            }
            check_schema(&mut r, &format!("{}@{} payload", entry.name, definition.version), &definition.payload_schema);
        }
    }

    // Integration descriptor (integration.d.ts).
    match manifest.integration() {
        Err(error) => {
            r.error("integration.parse", format!("attributes[\"{INTEGRATION_EXTENSION}\"]"), error.to_string())
        }
        Ok(None) => {}
        Ok(Some(descriptor)) => {
            if !manifest.advertises(INTEGRATION_EXTENSION) {
                r.warning(
                    "integration.extension",
                    "extensions",
                    format!("an integration descriptor is published but \"{INTEGRATION_EXTENSION}\" is not advertised"),
                );
            }
            check_integration(&mut r, manifest, &descriptor);
        }
    }
    r.0
}

fn check_versions(
    r: &mut Report,
    path: &str,
    name: &str,
    versions: &[u32],
    latest: u32,
    defined: impl Iterator<Item = u32>,
) {
    let mut defined: Vec<u32> = defined.collect();
    defined.sort_unstable();
    let mut listed = versions.to_vec();
    listed.sort_unstable();
    if listed != defined {
        r.error(
            "versions.mismatch",
            format!("{path}.versions"),
            format!("\"{name}\" lists versions {versions:?} but defines {defined:?}"),
        );
    }
    if versions.contains(&0) {
        r.error("versions.invalid", format!("{path}.versions"), format!("\"{name}\": versions start at 1"));
    }
    if !versions.contains(&latest) {
        r.error(
            "versions.latest",
            format!("{path}.latestVersion"),
            format!("\"{name}\": latestVersion {latest} is not a listed version"),
        );
    }
}

fn check_permission(r: &mut Report, path: &str, permission: &str) {
    if !is_valid_permission(permission) {
        r.error("authorization.permission", path, format!("\"{permission}\" is not a valid permission (§34)"));
    }
}

/// `authorizationContract` is derived from `authorization`: scopes required, or public
/// (`@appport/contract-builder` manifest.d.ts).
fn check_authorization_contract(
    r: &mut Report,
    at: &str,
    authorization: &[String],
    contract: Option<&AuthorizationRequirement>,
) {
    let consistent = match contract {
        None => true,
        Some(AuthorizationRequirement::Public(_)) => authorization.is_empty(),
        Some(AuthorizationRequirement::Scoped(scoped)) => {
            let mut a = authorization.to_vec();
            let mut b = scoped.scopes.clone();
            a.sort();
            b.sort();
            a == b
        }
    };
    if !consistent {
        r.error(
            "authorization.contract",
            format!("{at} authorizationContract"),
            "authorizationContract disagrees with authorization",
        );
    }
}

fn check_schema(r: &mut Report, at: &str, schema: &Schema) {
    let Ok(value) = serde_json::to_value(schema) else { return };
    for issue in validate_semantic_annotations(&value, "") {
        r.error("schema.semantic", format!("{at}{}", issue.path), issue.message);
    }
}

fn check_integration(r: &mut Report, manifest: &Manifest, descriptor: &IntegrationDescriptor) {
    for (key, entry) in &descriptor.capabilities {
        let path = format!("integration.capabilities[\"{key}\"]");
        match parse_dependency_ref(key) {
            Some(reference)
                if reference.version.is_some_and(|v| manifest.capability_version(&reference.name, v).is_some()) => {}
            _ => r.error(
                "integration.key",
                &path,
                format!("\"{key}\" is not a capability version this manifest declares"),
            ),
        }
        for (index, dependency) in entry.requires.iter().flatten().enumerate() {
            check_dependency(r, &format!("{path}.requires[{index}]"), dependency);
        }
        for (index, method) in entry.verify.iter().flatten().enumerate() {
            if let VerificationMethod::State(state) = method {
                for (i, expectation) in state.expect.iter().enumerate() {
                    if expectation.equals.is_some() == expectation.exists.is_some() {
                        r.error(
                            "verification.expectation",
                            format!("{path}.verify[{index}].expect[{i}]"),
                            "give exactly one of equals / exists",
                        );
                    }
                }
            }
        }
        for (index, precondition) in entry.preconditions.iter().flatten().enumerate() {
            for (i, expectation) in precondition.expect.iter().flatten().enumerate() {
                if expectation.equals.is_some() == expectation.exists.is_some() {
                    r.error(
                        "verification.expectation",
                        format!("{path}.preconditions[{index}].expect[{i}]"),
                        "give exactly one of equals / exists",
                    );
                }
            }
        }
    }
    for (index, dependency) in descriptor.requires.iter().flatten().enumerate() {
        check_dependency(r, &format!("integration.requires[{index}]"), dependency);
    }
    for (index, mapping) in descriptor.mappings.iter().flatten().enumerate() {
        let path = format!("integration.mappings[{index}]");
        if !is_digest(&mapping.digest) {
            r.error("mapping.digest", format!("{path}.digest"), "digest must be sha256:<64 lowercase hex>");
        } else if let Some(artifact) = &mapping.artifact {
            if digest_of(artifact) != mapping.digest {
                r.error(
                    "mapping.artifact-digest",
                    format!("{path}.artifact"),
                    "the embedded artifact does not hash to digest",
                );
            }
        }
    }
}

fn check_dependency(r: &mut Report, path: &str, dependency: &super::integration::Dependency) {
    if let Some(constraint) = &dependency.version_constraint {
        if !is_version_constraint(constraint) {
            r.error(
                "dependency.version-constraint",
                format!("{path}.versionConstraint"),
                format!("\"{constraint}\" is not a valid version constraint"),
            );
        }
    }
    if let DependencyTarget::Capability(target) = &dependency.target {
        match parse_dependency_ref(&target.capability) {
            None => r.error("dependency.target", format!("{path}.target.capability"), "must be name or name@version"),
            Some(reference) if reference.version.is_none() && dependency.version_constraint.is_none() => {
                r.error("dependency.unpinned", path, "an unpinned target needs a versionConstraint")
            }
            Some(_) => {}
        }
    }
}
