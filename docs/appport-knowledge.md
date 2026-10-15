# AppPort knowledge module

`crates/agent-core/src/appport/knowledge` is the agent's native, deterministic
understanding of AppPort. It is **derived from the published `@appport/*` npm
packages** and mechanically checked against them; it is not a hand-kept copy of
the spec.

## How it is derived

```
@appport/* on npm ──► tools/appport-inspect/inspect.mjs ──► crates/agent-core/knowledge/appport-surface.json
                       (Node, build time only)              crates/agent-core/tests/fixtures/*.json
                                                                        │
                    crates/agent-core (pure Rust + serde) ◄── cargo test ┘
```

`inspect.mjs` produces two kinds of output:

1. **The surface** (`knowledge/appport-surface.json`)
   - the exact package versions inspected;
   - runtime constants read from the packages' JavaScript (`PROTOCOL_ID`,
     `RESERVED_NAME_SEGMENTS`, `RETRY_CLASSES`, `DIGEST_PATTERN`, …);
   - the shape of every mirrored exported type, read from the `.d.ts` files with
     the TypeScript checker: property names, optionality, type text, literal
     values, union variants, and nested anonymous object shapes.
2. **Fixtures** (`tests/fixtures/`)
   - `reference.manifest.json`: the manifest of `@appport/testing`'s reference
     application, produced by `@appport/core`;
   - `orders.manifest.json`: an integration-rich application built with
     `createApplication` / `@appport/schema` (`fixtures-src/orders-app.mjs`).
     The runtime validates it on construction, and `inspect.mjs` refuses to emit
     it unless `lintIntegration`, `lintAgentContract` and `lintDependencyModel`
     report no errors;
   - `manifest-facts.json`: `contractDigest`, `digestOf`, the three lint
     results and `validateSemanticAnnotations` for every schema, as computed by
     the package;
   - `parity.json`: inputs (including edge cases) run through the package's own
     `validateName`, `isValidPermission`, `isValidApplicationId`,
     `parseCapabilityRef`, `isVersionConstraint`, `satisfiesVersionConstraint`,
     `parseDependencyRef`, `isDigest`, `canonicalize`/`digestOf` and
     `validateSemanticAnnotations`;
   - `compatibility.json`: a real `compareManifests` report;
   - `verification-evidence.json`, `runtime-evidence.json`, `handover.json`:
     shapes no package produces during inspection. They are written in
     `fixtures-src/typed-fixtures.ts` and **type-checked with `satisfies`
     against the published `.d.ts` files** before being emitted.

The Rust crate has no runtime dependency on Node or npm. Node is only needed to
regenerate or to run the drift check.

## How drift is caught

| Layer | Check | Fails when |
|---|---|---|
| packages → snapshot | `scripts/check-appport-drift.sh` (`--pinned` on every change, `--latest` daily in CI) | a published package changes anything recorded in the surface or fixtures, or stops exporting something inspection relies on |
| snapshot → Rust types | `tests/surface.rs` | a Rust type's serde shape (via `schemars`) differs from the recorded TS shape: a field added/removed/renamed, optionality changed, a literal value added, a union variant changed, a nested shape changed; or a TS type is inspected without a Rust mapping |
| snapshot → Rust constants | `tests/surface.rs` | any mirrored constant differs |
| fixtures → Rust model | `tests/fixtures.rs` | a real document does not parse, or does not round-trip exactly, or a digest differs |
| fixtures → Rust primitives | `tests/parity.rs` | a primitive disagrees with the package on any case |

`tests/surface.rs::drift_check_detects_divergence` mutates the surface nine
different ways and asserts each is caught, so the check cannot silently become
a no-op.

At runtime, `Contract::validate()` also reports `model.unmodeled` for any member
of a manifest the typed model did not capture — how a contract from a newer
AppPort shows up when the agent observes it.

### Updating to a new AppPort release

```sh
cd tools/appport-inspect
npm install --save-exact @appport/protocol@<v> @appport/core@<v> @appport/schema@<v> @appport/testing@<v>
npm run generate                 # rewrites the surface and fixtures
git diff ../../crates/agent-core # review what AppPort changed
cargo test -p agent-core         # shows exactly which Rust types/primitives must follow
```

Then update `SOURCE_PACKAGES` in `constants.rs`, the Rust types, and the
mapping below. Snapshot tests are re-blessed with `UPDATE_SNAPSHOTS=1`.

## Mapping

Source packages: `@appport/protocol` 1.1.0, `@appport/core` 1.2.0
(fixtures also use `@appport/schema` 1.1.0 and `@appport/testing` 1.1.3).

### Contract — `contract.rs`, `schema.rs`

| TypeScript (`@appport/protocol`) | Rust | Notes |
|---|---|---|
| `AppPortManifest` | `Manifest` | `protocol` is the literal type `ProtocolId` |
| `ApplicationDescriptor` | `ApplicationDescriptor` | |
| `ConformanceLevel` | `ConformanceLevel` | validated `1..=5` |
| `EventManifestEntry` / `EventVersionEntry` | `EventEntry` / `EventVersion` | |
| `TransportDescriptor` | `TransportDescriptor` | |
| `MaterializationDeclaration` | `MaterializationDeclaration` | |
| `UiContribution`, `UiSurface`, `UiNavigationContribution`, `UiProductIdentity`, `UiCompositionRequirement` | same names; `composition` → `UiComposition` | |
| `JSONSchema` | `Schema` | numbers kept as exact `serde_json::Number`; `properties`/`$defs` keep declaration order; `x-*` keywords in `extensions`; `const`/`default` keep explicit `null` |
| `JSONSchemaType` | `SchemaType` | `type` → `TypeSet` (one or many) |
| `findCapability`, `findCapabilityVersion`, `findEvent`, `hasTransport`, `getIntegration` | `Manifest::capability`, `capability_version`, `event`, `has_transport`, `integration` | |

### Capability — `capability.rs`

| TypeScript | Rust | Notes |
|---|---|---|
| `CapabilityManifestEntry` | `CapabilityEntry` | `current()` = newest non-deprecated version |
| `CapabilityVersionEntry` | `CapabilityVersion` | |
| `CapabilityKind`, `CapabilityEffect`, `IdempotencySupport`, `OperationStatus` | same names | absent `effect` is *unknown*, never `observation` |
| `ConcurrencyPolicy` | `ConcurrencyPolicy` | serde-tagged on `mode` |
| `AuthorizationRequirement` | `AuthorizationRequirement::{Scoped, Public}` | `required: true/false` are literal types |
| `AuthorizationResource` | `AuthorizationResource` | `type` → `kind` |

### Integration — `integration.rs`

| TypeScript | Rust | Notes |
|---|---|---|
| `IntegrationDescriptor` | `IntegrationDescriptor` | keys `name@version`, order kept |
| `CapabilityIntegration` | `CapabilityIntegration` | |
| `EffectDescriptor` | `EffectDescriptor` | `type` → `kind` |
| `ApprovalRequirement`, `Precondition`, `FailureDeclaration` | same names | `Precondition.require` → `PreconditionRequirement` |
| `RetryClass`, `FailureEffect`, `Coverage`, `DeclarationCoverage` | same names | |
| `Dependency`, `DependencyRelationship` | same names | |
| `DependencyTarget` | `DependencyTarget::{Capability, Event}` | |
| `ResourceDeclaration`, `ResourceAccess` | same names | `owner` → `ResourceOwner::{This, Application}`; `RESOURCE_KINDS` → `ResourceKind`; `RESOURCE_OPERATIONS` → `ResourceOperation`; `mode` → `AccessMode` |
| `MappingReference`, `MappingDirection`, `MappingOrigin` | same names | `for` → `serves`; `peer` → `MappingPeer` |
| `SemanticAnnotations`, `SemanticEncoding`, `SemanticIssue`, `IntegrationIssue` | same names | issue `level` → `IssueLevel` |

### Verification — `verification.rs`

| TypeScript | Rust | Notes |
|---|---|---|
| `VerificationMethod` | `VerificationMethod::{State, Event}` | `strength()`: state *establishes*, event *corroborates* |
| `StateVerification`, `EventVerification` | same names | `kind`/`strength` are literal types; `match` → `matches`; `via` → `ProbeCall` |
| `StateExpectation` | `StateExpectation` | `equals: null` is kept |
| `ValueRef` | `ValueRef::{Input, Output, Literal}` | |
| `VerificationEvidence`, `VerificationOutcome`, `MethodStatus` | same names | `methods[]` → `MethodEvidence`, `mismatches[]` → `Mismatch`, `probe` → `ProbeRecord` |
| `@appport/core` `InvocationEvidence`, `InvocationCallerEvidence`, `InvocationOutcome` | same names | `attribution` → `Attribution`; `target` → `InvocationTarget` |
| `@appport/core` `StateAccessEvidence`, `StateOperation` | same names | `resource` → `StateResourceClassification` |
| `@appport/core` `RuntimeEvidence` | `RuntimeEvidence` | |

### Replacement — `replacement.rs`

| TypeScript | Rust | Notes |
|---|---|---|
| `ChangeSeverity`, `ManifestChange`, `CompatibilityReport` | same names | types only; `compareManifests` is not ported yet |
| `@appport/core` `ProviderHandoverRequest`, `ProviderRegistrationExpectation`, `ProviderHandoverRejection`, `ProviderHandoverRecord` | same names | record `from`/`to` → `HandoverSource`/`HandoverTarget` |

### Derived (Rust-only) — `graph.rs`, `inspect.rs`, `replacement.rs`, `validate.rs`

| Rust | What it is |
|---|---|
| `Contract` | entry point: parse, `inspect`, `graph`, `replacement_boundary`, `validate`, `digest`, `unmodeled` |
| `CapabilityView` | model-ready slice of one capability version; `unknowns` lists what the contract does not state |
| `Graph` | declared graph of one application: dependencies, emits, reads/changes, verification/precondition probes, ownership; cross-application targets are `resolved: false` and listed in `unresolved` |
| `ReplacementBoundary` | compatibility constraints (§43), dependents in the contract, a `contract_pin` digest, and the handover gates with their rejection codes |
| `Issue` / `validate_manifest` | structural rules with stable codes (see `validate.rs`) |
| `model_json` | compact, stable JSON for handing a slice to any model |

### Primitives — `rules.rs` (all parity-tested against the package)

`validateName`, `isValidName`, `isValidPermission`, `isValidApplicationId`,
`namespaceOf`, `operationOf`, `capabilityRef`, `integrationKey`,
`parseCapabilityRef`, `parseDependencyRef`, `isVersionConstraint`,
`satisfiesVersionConstraint`, `canonicalize` (RFC 8785), `sha256`, `digestOf`,
`isDigest`, `contractDigest`, `validateSemanticAnnotations` → snake_case
equivalents.

Faithfulness details that matter: JavaScript whitespace (not Rust's
`is_whitespace`), `Number(...)` and `parseInt(...)` coercions, UTF-16 lengths
and key ordering, `Object.entries` ordering, ECMAScript number formatting, and
correctly rounded float parsing (`serde_json/float_roundtrip`).

**Documented divergence:** `parse_capability_ref` returns `None` for versions
above `u32::MAX`; JavaScript accepts any integer up to `1e308`.

## Deliberately not in this PR

- Porting `lintIntegration`, `lintAgentContract`, `lintDependencyModel` and
  `compareManifests` (their outputs on the fixtures are recorded, ready to
  parity-test a port).
- Multi-application or live graph construction; project discovery; replacement
  workflows; prompting/context assembly; any provider code.
