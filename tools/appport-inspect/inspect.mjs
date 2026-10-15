#!/usr/bin/env node
// Build-time inspection of the published @appport/* packages.
//
// Produces, under the target crate directory:
//
//   knowledge/appport-surface.json   the inspected surface: package versions, runtime
//                                    constants and the shape of every exported type the
//                                    Rust knowledge module mirrors (from the .d.ts files)
//   tests/fixtures/*.json            manifests and records produced by (or type-checked
//                                    against) the packages, plus parity cases computed
//                                    by the packages' own functions
//
// Usage:
//   node inspect.mjs --out <crate-dir>     regenerate in place
//   node inspect.mjs --check <crate-dir>   regenerate to a temp dir and fail on any
//                                          difference from what is checked in
//
// The Rust crate never runs Node. Its tests read these files and fail when the Rust
// types or primitives disagree with them; this script fails when the files disagree
// with the packages. Together they are the drift check.

import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import ts from "typescript";

import * as protocol from "@appport/protocol";
import * as core from "@appport/core";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

// ---------------------------------------------------------------------------
// 1. Which packages, at which versions
// ---------------------------------------------------------------------------

const INSPECTED = ["@appport/protocol", "@appport/core", "@appport/schema", "@appport/testing"];

function packageDir(name) {
  return join(here, "node_modules", ...name.split("/"));
}

function packageVersions() {
  const out = {};
  for (const name of INSPECTED) {
    out[name] = JSON.parse(readFileSync(join(packageDir(name), "package.json"), "utf8")).version;
  }
  return out;
}

// ---------------------------------------------------------------------------
// 2. Runtime constants (read from the packages' JavaScript, not their docs)
// ---------------------------------------------------------------------------

function constants() {
  const p = protocol;
  return {
    "@appport/protocol": {
      PROTOCOL_ID: p.PROTOCOL_ID,
      PROTOCOL_MAJOR: p.PROTOCOL_MAJOR,
      CONFORMANCE_LEVELS: p.CONFORMANCE_LEVELS,
      RESERVED_NAME_SEGMENTS: p.RESERVED_NAME_SEGMENTS,
      JSON_SCHEMA_DIALECT: p.JSON_SCHEMA_DIALECT,
      WELL_KNOWN_PATH: p.WELL_KNOWN_PATH,
      DEFAULT_HTTP_ENDPOINT: p.DEFAULT_HTTP_ENDPOINT,
      DEFAULT_WS_ENDPOINT: p.DEFAULT_WS_ENDPOINT,
      DEFAULT_MANIFEST_PATH: p.DEFAULT_MANIFEST_PATH,
      AUTHORIZATION_MODEL_VERSION: p.AUTHORIZATION_MODEL_VERSION,
      INTEGRATION_EXTENSION: p.INTEGRATION_EXTENSION,
      SEMANTIC_KEYWORDS: p.SEMANTIC_KEYWORDS,
      SEMANTIC_ENCODINGS: p.SEMANTIC_ENCODINGS,
      RETRY_CLASSES: p.RETRY_CLASSES,
      FAILURE_EFFECTS: p.FAILURE_EFFECTS,
      DEPENDENCY_RELATIONSHIPS: p.DEPENDENCY_RELATIONSHIPS,
      RESOURCE_KINDS: p.RESOURCE_KINDS,
      RESOURCE_OPERATIONS: p.RESOURCE_OPERATIONS,
      COVERAGE_KEYS: p.COVERAGE_KEYS,
      MAPPING_DIRECTIONS: p.MAPPING_DIRECTIONS,
      MAPPING_ORIGINS: p.MAPPING_ORIGINS,
      OPERATION_CAPABILITIES: p.OPERATION_CAPABILITIES,
      DIGEST_PATTERN: p.DIGEST_PATTERN.source,
      ANY_SCHEMA: p.ANY_SCHEMA,
      EMPTY_OBJECT_SCHEMA: p.EMPTY_OBJECT_SCHEMA
    },
    "@appport/core": {
      INVOCATION_EVIDENCE: core.INVOCATION_EVIDENCE,
      STATE_ACCESS_EVIDENCE: core.STATE_ACCESS_EVIDENCE,
      BUILTIN_CAPABILITY_NAMES: core.BUILTIN_CAPABILITY_NAMES
    }
  };
}

// ---------------------------------------------------------------------------
// 3. Type shapes, read from the published .d.ts files with the TypeScript checker
// ---------------------------------------------------------------------------

/** Exported types the Rust model mirrors, by package. Order is irrelevant; output is sorted. */
const TYPES = {
  "@appport/protocol": [
    // Contract
    "AppPortManifest", "ApplicationDescriptor", "TransportDescriptor", "EventManifestEntry", "EventVersionEntry",
    "JSONSchema", "JSONSchemaType", "MaterializationDeclaration",
    "UiContribution", "UiSurface", "UiNavigationContribution", "UiProductIdentity", "UiCompositionRequirement",
    // Capability
    "CapabilityManifestEntry", "CapabilityVersionEntry", "CapabilityKind", "ConcurrencyPolicy", "IdempotencySupport",
    "CapabilityEffect", "AuthorizationRequirement", "AuthorizationResource", "OperationStatus",
    // Integration
    "IntegrationDescriptor", "CapabilityIntegration", "EffectDescriptor", "ApprovalRequirement", "Precondition",
    "FailureDeclaration", "RetryClass", "FailureEffect", "Dependency", "DependencyRelationship", "DependencyTarget",
    "ResourceDeclaration", "ResourceAccess", "Coverage", "DeclarationCoverage", "MappingReference", "MappingDirection",
    "MappingOrigin", "SemanticAnnotations", "SemanticEncoding", "SemanticIssue", "IntegrationIssue",
    // Verification
    "VerificationMethod", "StateVerification", "EventVerification", "StateExpectation", "VerificationEvidence",
    "VerificationOutcome", "MethodStatus",
    // Replacement / compatibility
    "ChangeSeverity", "ManifestChange", "CompatibilityReport"
  ],
  "@appport/core": [
    "InvocationEvidence", "InvocationCallerEvidence", "InvocationOutcome", "StateAccessEvidence", "StateOperation",
    "ProviderHandoverRequest", "ProviderRegistrationExpectation", "ProviderHandoverRejection", "ProviderHandoverRecord"
  ]
};

const FORMAT = ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.UseAliasDefinedOutsideCurrentScope;

function createProgram(extraRoots = []) {
  const roots = Object.keys(TYPES).map((name) => join(packageDir(name), "dist", "index.d.ts"));
  return ts.createProgram([...roots, ...extraRoots], {
    strict: true,
    noEmit: true,
    skipLibCheck: true,
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.NodeNext,
    moduleResolution: ts.ModuleResolutionKind.NodeNext,
    baseUrl: here
  });
}

function typeShapes() {
  const program = createProgram();
  const checker = program.getTypeChecker();
  const out = {};
  for (const [pkg, names] of Object.entries(TYPES)) {
    const file = program.getSourceFile(join(packageDir(pkg), "dist", "index.d.ts"));
    const moduleSymbol = checker.getSymbolAtLocation(file);
    const exports = new Map(checker.getExportsOfModule(moduleSymbol).map((s) => [s.name, s]));
    out[pkg] = {};
    for (const name of [...names].sort()) {
      let symbol = exports.get(name);
      if (!symbol) throw new Error(`${pkg} no longer exports ${name}`);
      if (symbol.flags & ts.SymbolFlags.Alias) symbol = checker.getAliasedSymbol(symbol);
      out[pkg][name] = describe(checker, checker.getDeclaredTypeOfSymbol(symbol), 0);
    }
  }
  return out;
}

const isNullish = (t) => (t.flags & (ts.TypeFlags.Undefined | ts.TypeFlags.Void)) !== 0;

function literalValue(t) {
  if (t.isStringLiteral() || t.isNumberLiteral()) return t.value;
  if (t.flags & ts.TypeFlags.BooleanLiteral) return t.intrinsicName === "true";
  return undefined;
}

function isAnonymousObject(t) {
  return (t.flags & ts.TypeFlags.Object) !== 0 && (t.objectFlags & ts.ObjectFlags.Anonymous) !== 0 && !t.getCallSignatures().length;
}

/**
 * A stable, comparable description of a type:
 *   { kind: "literals", values }           closed string/number union
 *   { kind: "object", properties, index? } named or anonymous object
 *   { kind: "union", variants }            discriminated / structural union
 *   { kind: "other", text }                anything else (rendered by the checker)
 * Anonymous object types nested in properties (or arrays of them) are described too,
 * so a change inside e.g. `Precondition.via` is visible.
 */
function describe(checker, type, depth) {
  if (type.isUnion()) {
    const members = type.types.filter((t) => !isNullish(t));
    const values = members.map(literalValue);
    if (values.every((v) => v !== undefined)) {
      return { kind: "literals", values: [...new Set(values)].sort() };
    }
    if (members.every((t) => t.flags & ts.TypeFlags.Object)) {
      const variants = members.map((t) => describe(checker, t, depth + 1));
      variants.sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)));
      return { kind: "union", variants };
    }
  }
  const single = literalValue(type);
  if (single !== undefined) return { kind: "literals", values: [single] };
  if (type.flags & ts.TypeFlags.Object && !checker.isArrayType(type) && depth < 6) {
    const properties = {};
    for (const property of checker.getPropertiesOfType(type)) {
      const declaration = property.valueDeclaration ?? property.declarations?.[0];
      const propertyType = checker.getTypeOfSymbolAtLocation(property, declaration);
      const entry = {
        optional: (property.flags & ts.SymbolFlags.Optional) !== 0,
        type: checker.typeToString(propertyType, undefined, FORMAT)
      };
      const nested = nestedShape(checker, propertyType, depth);
      if (nested) entry.shape = nested;
      properties[property.name] = entry;
    }
    const result = { kind: "object", properties: sortKeys(properties) };
    const index = checker.getIndexInfosOfType(type).map(
      (info) => `[${checker.typeToString(info.keyType)}]: ${checker.typeToString(info.type, undefined, FORMAT)}`
    );
    if (index.length) result.index = index.sort();
    return result;
  }
  return { kind: "other", text: checker.typeToString(type, undefined, FORMAT) };
}

function nestedShape(checker, type, depth) {
  const nonNull = type.isUnion() ? type.types.filter((t) => !isNullish(t)) : [type];
  if (nonNull.length !== 1) {
    // Optional anonymous unions (e.g. `owner?: "self" | { application }`) are described by the type text.
    return undefined;
  }
  let t = nonNull[0];
  let array = false;
  if (checker.isArrayType(t)) {
    t = checker.getTypeArguments(t)[0];
    array = true;
  }
  if (!isAnonymousObject(t)) return undefined;
  const shape = describe(checker, t, depth + 1);
  return array ? { kind: "array", items: shape } : shape;
}

function sortKeys(object) {
  return Object.fromEntries(Object.entries(object).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
}

// ---------------------------------------------------------------------------
// 4. Fixtures produced by the packages
// ---------------------------------------------------------------------------

async function loadTypedFixtures() {
  // Type-check fixtures-src/typed-fixtures.ts against the published .d.ts files, then evaluate it.
  const source = join(here, "fixtures-src", "typed-fixtures.ts");
  const program = createProgram([source]);
  const diagnostics = ts.getPreEmitDiagnostics(program).filter((d) => d.file?.fileName === source);
  if (diagnostics.length) {
    const text = ts.formatDiagnostics(diagnostics, {
      getCanonicalFileName: (f) => f,
      getCurrentDirectory: () => here,
      getNewLine: () => "\n"
    });
    throw new Error(`typed fixtures no longer match the published types:\n${text}`);
  }
  const js = ts.transpileModule(readFileSync(source, "utf8"), {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 }
  }).outputText;
  const temp = mkdtempSync(join(tmpdir(), "appport-typed-"));
  const file = join(temp, "typed-fixtures.mjs");
  writeFileSync(file, js);
  try {
    return await import(pathToFileURL(file).href);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}

async function referenceManifest() {
  // `@appport/testing`'s index also loads its vitest-based conformance suite, whose
  // runner is not a declared dependency, so the fixture module is loaded directly.
  const fixture = await import(pathToFileURL(join(packageDir("@appport/testing"), "dist", "fixture.js")).href);
  return fixture.createTestApplication().application.manifest();
}

function lintAll(manifest) {
  const issues = {
    lintIntegration: protocol.lintIntegration(manifest),
    lintAgentContract: protocol.lintAgentContract(manifest),
    lintDependencyModel: protocol.lintDependencyModel(manifest)
  };
  const errors = Object.values(issues).flat().filter((issue) => issue.level === "error");
  if (errors.length) throw new Error(`fixture manifest fails the official linters: ${JSON.stringify(errors)}`);
  return issues;
}

function schemasOf(manifest) {
  const out = [];
  for (const capability of manifest.capabilities) {
    for (const definition of capability.definitions) {
      out.push([`${capability.name}@${definition.version} input`, definition.inputSchema]);
      out.push([`${capability.name}@${definition.version} output`, definition.outputSchema]);
    }
  }
  for (const event of manifest.events) {
    for (const definition of event.definitions) out.push([`${event.name}@${definition.version} payload`, definition.payloadSchema]);
  }
  return out;
}

// ---------------------------------------------------------------------------
// 5. Parity cases: inputs run through the packages' own functions
// ---------------------------------------------------------------------------

const NAMES = [
  "documents.save", "documents", "Documents.save", "a.b.c", "http.get", "documents.http", "d1.x2", "1a.b", "a..b",
  "a.b.", ".a.b", "docs.inprocess", "a.b-c", "a.b_c", "", "x." + "a".repeat(126), "x." + "a".repeat(127), "orders.v2.place",
  "grpcx.call", "a.rest"
];
const APPLICATION_IDS = [
  "com.example.sherpa", "com.example", "com", "com.Example", "com.example.my-app", "com.1x", "com.-x", "com.x-",
  "dev.appport.testfixture", "a." + "b".repeat(188), "a." + "b".repeat(187), "com..x", "1com.x"
];
const CAPABILITY_REFS = [
  "documents.save@1", "documents.save@0", "documents.save@1.5", "documents.save", "http.get@1", "documents.save@01",
  "documents.save@ 2", "documents.save@2 ", "a.b@2@3", "documents.save@", "documents.save@1e1", "documents.save@0x2",
  "documents.save@-1", "documents.save@+3", "documents.save@1.0", "documents.save@Infinity", "documents.save@0b11",
  "documents.save@9007199254740993", "@1", "documents.save@\t4\n", "documents.save@1_0"
];
const VERSION_CONSTRAINTS = [
  "1", "^1", "~1.2", ">=2", ">1", "<=2", "<2", "1.2.3", " ^1 ", "v1", "^", "=1", "^1.x", "", ">= 2", "~0", "^10", "<1"
];
const DEPENDENCY_REFS = [
  "orders.get", "orders.get@1", "orders.get@0", "orders.get@01", "Orders.get", "orders@1", "orders.get@1.0",
  "orders.get@12", "a.b.c@3", "http.get@1"
];
const DIGESTS = [
  "sha256:" + "0123456789abcdef".repeat(4), "sha256:" + "0123456789ABCDEF".repeat(4), "sha256:abc", "sha1:" + "a".repeat(64),
  "sha256:" + "g".repeat(64), "SHA256:" + "a".repeat(64)
];
const CANONICAL_VALUES = [
  null, true, 0, -0, 1, -1, 0.1, 1.5, 1e21, 1e-7, 123456789012345680000, 5e-324, 1.7976931348623157e308, 9007199254740993,
  0.30000000000000004, "plain", "quote \" backslash \\ slash /", "control \u0001\u001f\b\f\n\r\t", "unicode é € 😀", "  ",
  [], {}, [1, [2, [3]]],
  { b: 1, a: 2, A: 3, "": 4, aa: 5 },
  { "é": 1, e: 2, "😀": 3, "￿": 4, "": 5, "\u0080": 6 },
  { nested: { z: [true, false, null], y: { x: 1.25 } } }
];

const SEMANTIC_CASES = [
  { "x-appport-concept": "https://schema.example.com/A" },
  { "x-appport-concept": "not an iri" },
  { "x-appport-concept": "urn:x" },
  { "x-appport-concept": 7 },
  { "x-appport-unit": "h" },
  { "x-appport-unit": "per hour" },
  { "x-appport-unit": "" },
  { "x-appport-encoding": "iso8601" },
  { "x-appport-encoding": "rfc3339" },
  { enum: ["a", "b"], "x-appport-enum-concepts": { a: "urn:a", b: "urn:b" } },
  { enum: ["a"], "x-appport-enum-concepts": { a: "urn:a", c: "urn:c" } },
  { "x-appport-enum-concepts": { a: "urn:a" } },
  { enum: [1, 2], "x-appport-enum-concepts": { 1: "urn:one", 3: "bad iri" } },
  { enum: ["a"], "x-appport-enum-concepts": ["urn:a"] },
  {
    type: "object",
    "x-vendor-thing": "ignored",
    properties: {
      when: { type: "string", "x-appport-encoding": "unix" },
      list: { type: "array", items: { "x-appport-unit": "a b" } },
      choice: { anyOf: [{ "x-appport-concept": "x" }, { oneOf: [{ "x-appport-concept": "y" }] }] }
    },
    $defs: { d: { allOf: [{ not: { "x-appport-concept": "z" } }] } },
    additionalProperties: { "x-appport-unit": " " },
    propertyNames: { "x-appport-encoding": 1 }
  }
];

function parityCases() {
  const p = protocol;
  return {
    validateName: NAMES.map((input) => ({ input, result: p.validateName(input) })),
    isValidPermission: NAMES.map((input) => ({ input, result: p.isValidPermission(input) })),
    isValidApplicationId: APPLICATION_IDS.map((input) => ({ input, result: p.isValidApplicationId(input) })),
    parseCapabilityRef: CAPABILITY_REFS.map((input) => ({ input, result: p.parseCapabilityRef(input) })),
    isVersionConstraint: VERSION_CONSTRAINTS.map((input) => ({ input, result: p.isVersionConstraint(input) })),
    satisfiesVersionConstraint: VERSION_CONSTRAINTS.filter((c) => p.isVersionConstraint(c)).flatMap((constraint) =>
      [0, 1, 2, 3, 10, 11].map((version) => ({ version, constraint, result: p.satisfiesVersionConstraint(version, constraint) }))
    ),
    parseDependencyRef: DEPENDENCY_REFS.map((input) => ({ input, result: p.parseDependencyRef(input) ?? null })),
    isDigest: DIGESTS.map((input) => ({ input, result: p.isDigest(input) })),
    canonicalize: CANONICAL_VALUES.map((value) => ({ value, canonical: p.canonicalize(value), digest: p.digestOf(value) })),
    validateSemanticAnnotations: SEMANTIC_CASES.map((schema) => ({ schema, issues: p.validateSemanticAnnotations(schema) }))
  };
}

// ---------------------------------------------------------------------------
// 6. Compatibility: a real report from compareManifests
// ---------------------------------------------------------------------------

function compatibilityFixture(previous) {
  const next = structuredClone(previous);
  const save = next.capabilities.find((c) => c.name === "documents.save");
  // A replacement that drops a version, tightens authorization and adds an optional input field.
  save.definitions = save.definitions.filter((d) => d.version !== 2);
  save.versions = [1];
  save.latestVersion = 1;
  const v1 = save.definitions[0];
  v1.authorization = [...v1.authorization, "documents.admin"];
  v1.inputSchema = structuredClone(v1.inputSchema);
  v1.inputSchema.properties = { ...v1.inputSchema.properties, tags: { type: "array", items: { type: "string" } } };
  Object.assign(save, { inputSchema: v1.inputSchema, outputSchema: v1.outputSchema, authorization: v1.authorization });
  return { previous: "reference.manifest.json", change: "documents.save: drop @2, require documents.admin on @1, add optional input.tags", report: protocol.compareManifests(previous, next), next };
}

// ---------------------------------------------------------------------------
// Assemble
// ---------------------------------------------------------------------------

async function build() {
  const reference = await referenceManifest();
  const { createOrdersApplication } = await import("./fixtures-src/orders-app.mjs");
  const orders = createOrdersApplication().manifest();
  const typed = await loadTypedFixtures();

  const manifests = { "reference.manifest.json": reference, "orders.manifest.json": orders };
  const manifestFacts = {};
  for (const [file, manifest] of Object.entries(manifests)) {
    manifestFacts[file] = {
      contractDigest: protocol.contractDigest(manifest),
      digest: protocol.digestOf(manifest),
      lint: lintAll(manifest),
      semanticIssues: schemasOf(manifest).map(([at, schema]) => ({ at, issues: protocol.validateSemanticAnnotations(schema) }))
    };
  }

  const compat = compatibilityFixture(reference);

  const files = {
    "knowledge/appport-surface.json": {
      $comment: "Generated by tools/appport-inspect/inspect.mjs from the published @appport/* packages. Do not edit.",
      packages: packageVersions(),
      constants: constants(),
      types: typeShapes()
    },
    "tests/fixtures/reference.manifest.json": reference,
    "tests/fixtures/orders.manifest.json": orders,
    "tests/fixtures/manifest-facts.json": manifestFacts,
    "tests/fixtures/parity.json": parityCases(),
    "tests/fixtures/compatibility.json": compat,
    "tests/fixtures/verification-evidence.json": typed.verificationEvidence,
    "tests/fixtures/runtime-evidence.json": typed.runtimeEvidence,
    "tests/fixtures/handover.json": {
      request: typed.handoverRequest,
      record: typed.handoverRecord,
      rejections: typed.handoverRejections
    }
  };
  return Object.fromEntries(Object.entries(files).map(([path, value]) => [path, JSON.stringify(value, null, 2) + "\n"]));
}

function write(root, files) {
  for (const [path, text] of Object.entries(files)) {
    const target = join(root, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, text);
  }
}

function check(root, files) {
  const drifted = [];
  for (const [path, text] of Object.entries(files)) {
    const target = join(root, path);
    const current = existsSync(target) ? readFileSync(target, "utf8") : undefined;
    if (current === text) continue;
    if (path === "knowledge/appport-surface.json" && current) {
      // A version bump with an identical surface is not drift; anything else is.
      const a = JSON.parse(current), b = JSON.parse(text);
      const strip = ({ packages: _p, ...rest }) => JSON.stringify(rest);
      if (strip(a) === strip(b)) {
        console.warn(`notice: package versions changed with an identical surface: ${JSON.stringify(a.packages)} -> ${JSON.stringify(b.packages)}`);
        continue;
      }
    }
    drifted.push(path);
  }
  if (drifted.length) {
    const temp = mkdtempSync(join(tmpdir(), "appport-drift-"));
    write(temp, files);
    console.error("AppPort drift detected. These files no longer match the published @appport/* packages:");
    for (const path of drifted) console.error(`  ${path}\n    diff -u ${join(root, path)} ${join(temp, path)}`);
    console.error("Review the change, update the Rust model if needed, then run `npm run generate` and commit.");
    process.exit(1);
  }
  console.log("No AppPort drift: the checked-in surface and fixtures match the installed @appport/* packages.");
}

const [mode, target] = process.argv.slice(2);
if (!["--out", "--check"].includes(mode) || !target) {
  console.error("usage: node inspect.mjs --out <crate-dir> | --check <crate-dir>");
  process.exit(2);
}
let files;
try {
  files = await build();
} catch (error) {
  console.error("AppPort drift detected: inspection could not run against the installed @appport/* packages.");
  console.error(`  ${error instanceof Error ? error.message : String(error)}`);
  console.error(`  installed: ${JSON.stringify(packageVersions())}`);
  console.error("The packages no longer provide something the knowledge module relies on. Review the release, update");
  console.error("tools/appport-inspect and the Rust model together, then run `npm run generate` and commit.");
  process.exit(1);
}
if (mode === "--out") {
  write(resolve(target), files);
  console.log(`Wrote ${Object.keys(files).length} files under ${resolve(target)}`);
} else {
  check(resolve(target), files);
}
