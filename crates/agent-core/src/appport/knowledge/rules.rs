//! Deterministic primitives, ported from `@appport/protocol`.
//!
//! Every function here is a faithful port of the named JavaScript function,
//! including its edge cases (JavaScript whitespace, `Number(...)` and
//! `parseInt(...)` coercions, UTF-16 lengths and key ordering). Parity is not
//! assumed: `tests/parity.rs` runs every case in `tests/fixtures/parity.json`,
//! whose expected results were produced by the published package itself.
//!
//! No model is involved in any of these checks.

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use super::constants::{RESERVED_NAME_SEGMENTS, SEMANTIC_ENCODINGS, SEMANTIC_KEYWORDS};
use super::integration::SemanticIssue;

/// JavaScript's `WhiteSpace` and `LineTerminator` code points (what `trim()`, `\s`,
/// `Number()` and `parseInt()` skip). Not the same set as `char::is_whitespace`.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

const JS_WS_CLASS: &str =
    r"\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static pattern")
}

// `\d` is written `[0-9]` throughout: JavaScript's `\d` is ASCII-only, Rust's is Unicode.
static NAME_PATTERN: LazyLock<Regex> = LazyLock::new(|| regex(r"^[a-z][a-z0-9]*(\.[a-z][a-z0-9]*)+$"));
static APPLICATION_ID: LazyLock<Regex> = LazyLock::new(|| regex(r"^[a-z][a-z0-9]*(\.[a-z0-9][a-z0-9-]*)+$"));
static VERSION_CONSTRAINT: LazyLock<Regex> = LazyLock::new(|| regex(r"^(\^|~|>=|>|<=|<)?[0-9]+(\.[0-9]+)*$"));
static DEPENDENCY_REF: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^([a-z][a-z0-9]*(?:\.[a-z][a-z0-9]*)+)(?:@([1-9][0-9]*))?$"));
static DIGEST_PATTERN: LazyLock<Regex> = LazyLock::new(|| regex(r"^sha256:[0-9a-f]{64}$"));
static IRI: LazyLock<Regex> = LazyLock::new(|| regex(&format!(r"^[A-Za-z][A-Za-z0-9+.-]*:[^{JS_WS_CLASS}]+$")));
static UNIT: LazyLock<Regex> = LazyLock::new(|| regex(&format!(r"^[^{JS_WS_CLASS}]+$")));

/// The source of the digest pattern, for comparison with `DIGEST_PATTERN.source`.
pub const DIGEST_PATTERN_SOURCE: &str = "^sha256:[0-9a-f]{64}$";

// ---------------------------------------------------------------------------
// Names (names.js)
// ---------------------------------------------------------------------------

/// Result of [`validate_name`]: mirrors `NameValidation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameValidation {
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
}

/// `validateName`: `<namespace>.<operation>`, lowercase ASCII, no transport words (§8).
pub fn validate_name(name: &str) -> NameValidation {
    let invalid = |reason: String| NameValidation { valid: false, reason: Some(reason) };
    if name.is_empty() {
        return invalid("name must be a non-empty string".into());
    }
    if name.encode_utf16().count() > 128 {
        return invalid("name must be at most 128 characters".into());
    }
    if !NAME_PATTERN.is_match(name) {
        return invalid("name must be lowercase <namespace>.<operation> using ASCII letters and digits".into());
    }
    if let Some(segment) = name.split('.').find(|s| RESERVED_NAME_SEGMENTS.contains(s)) {
        return invalid(format!("name must not contain the transport segment \"{segment}\""));
    }
    NameValidation { valid: true, reason: None }
}

/// `isValidName`.
pub fn is_valid_name(name: &str) -> bool {
    validate_name(name).valid
}

/// `isValidPermission`: the name pattern, without the transport-word rule (§34).
pub fn is_valid_permission(permission: &str) -> bool {
    NAME_PATTERN.is_match(permission)
}

/// `isValidApplicationId`: reverse-domain, at most 190 UTF-16 units (§41).
pub fn is_valid_application_id(id: &str) -> bool {
    id.encode_utf16().count() <= 190 && APPLICATION_ID.is_match(id)
}

/// `namespaceOf` (`documents` for `documents.save`).
pub fn namespace_of(name: &str) -> &str {
    name.find('.').map_or("", |i| &name[..i])
}

/// `operationOf` (`save` for `documents.save`).
pub fn operation_of(name: &str) -> &str {
    name.rfind('.').map_or(name, |i| &name[i + 1..])
}

/// `capabilityRef` / `integrationKey`: `name@version`.
pub fn capability_ref(name: &str, version: u32) -> String {
    format!("{name}@{version}")
}

/// `integrationKey`.
pub fn integration_key(name: &str, version: u32) -> String {
    capability_ref(name, version)
}

/// A parsed `name@version` reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CapabilityRef {
    pub name: String,
    pub version: u32,
}

/// `parseCapabilityRef`. The version goes through JavaScript `Number(...)`, so
/// `documents.save@ 2` and `documents.save@0x2` are accepted, as they are in JS.
///
/// Divergence (documented, tested): JS accepts any integer version up to `1e308`;
/// capability versions are `u32` here, so larger values return `None`.
pub fn parse_capability_ref(reference: &str) -> Option<CapabilityRef> {
    let at = reference.rfind('@')?;
    let (name, version) = (&reference[..at], js_number(&reference[at + 1..]));
    if !is_valid_name(name) || !(version.is_finite() && version.fract() == 0.0) || version < 1.0 {
        return None;
    }
    (version <= f64::from(u32::MAX)).then(|| CapabilityRef { name: name.to_string(), version: version as u32 })
}

/// A parsed dependency target reference: `name` or `name@version`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DependencyRef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version: Option<u32>,
}

/// `parseDependencyRef`.
pub fn parse_dependency_ref(reference: &str) -> Option<DependencyRef> {
    let captures = DEPENDENCY_REF.captures(reference)?;
    let version = match captures.get(2) {
        Some(v) => Some(v.as_str().parse().ok()?),
        None => None,
    };
    Some(DependencyRef { name: captures[1].to_string(), version })
}

// ---------------------------------------------------------------------------
// Version constraints (integration.js)
// ---------------------------------------------------------------------------

/// `isVersionConstraint`: `1`, `^1`, `~1.2`, `>=2`, `>1`, `<=2`, `<2`.
pub fn is_version_constraint(value: &str) -> bool {
    VERSION_CONSTRAINT.is_match(js_trim(value))
}

/// `satisfiesVersionConstraint`: major-only semantics, as in the core resolver.
pub fn satisfies_version_constraint(version: u32, constraint: &str) -> bool {
    let c = js_trim(constraint);
    let v = f64::from(version);
    let cmp = |text: &str, op: fn(f64, f64) -> bool| js_parse_int(text).is_some_and(|n| op(v, n));
    if let Some(rest) = c.strip_prefix('^').or_else(|| c.strip_prefix('~')) {
        return cmp(rest.split('.').next().unwrap_or(""), |a, b| a == b);
    }
    if let Some(rest) = c.strip_prefix(">=") {
        return cmp(rest, |a, b| a >= b);
    }
    if let Some(rest) = c.strip_prefix('>') {
        return cmp(rest, |a, b| a > b);
    }
    if let Some(rest) = c.strip_prefix("<=") {
        return cmp(rest, |a, b| a <= b);
    }
    if let Some(rest) = c.strip_prefix('<') {
        return cmp(rest, |a, b| a < b);
    }
    cmp(c, |a, b| a == b)
}

// ---------------------------------------------------------------------------
// JavaScript numeric coercions
// ---------------------------------------------------------------------------

/// `Number(text)` for a string (ECMAScript `StringToNumber`). `NaN` when invalid.
pub fn js_number(text: &str) -> f64 {
    let t = js_trim(text);
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(digits) = t.strip_prefix(prefix) {
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return digits.chars().fold(0.0, |acc, c| acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap()));
        }
    }
    let (sign, body) = match t.as_bytes()[0] {
        b'+' => (1.0, &t[1..]),
        b'-' => (-1.0, &t[1..]),
        _ => (1.0, t),
    };
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }
    // StrUnsignedDecimalLiteral: digits [. digits] [e[+-]digits] | . digits [e…]
    let bytes = body.as_bytes();
    let mut i = 0;
    let int_digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        frac_digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        i += frac_digits;
    }
    if int_digits + frac_digits == 0 {
        return f64::NAN;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exp_digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        if exp_digits == 0 {
            return f64::NAN;
        }
        i += exp_digits;
    }
    if i != bytes.len() {
        return f64::NAN;
    }
    // Rust's float parser accepts exactly this grammar (plus forms excluded above).
    body.parse::<f64>().map_or(f64::NAN, |n| sign * n)
}

/// `Number.parseInt(text, 10)`. `None` stands for `NaN`.
pub fn js_parse_int(text: &str) -> Option<f64> {
    let t = text.trim_start_matches(is_js_whitespace);
    let (sign, body) = match t.chars().next() {
        Some('+') => (1.0, &t[1..]),
        Some('-') => (-1.0, &t[1..]),
        _ => (1.0, t),
    };
    let digits: String = body.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<f64>().ok().map(|n| sign * n)
}

/// ECMAScript `Number::toString` for a finite number (as used by `JSON.stringify`).
pub fn js_number_to_string(n: f64) -> String {
    if n == 0.0 {
        return "0".into(); // also -0
    }
    ryu_js::Buffer::new().format_finite(n).to_string()
}

// ---------------------------------------------------------------------------
// Canonical JSON and digests (digest.js)
// ---------------------------------------------------------------------------

/// Error for values with no canonical JSON form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonFiniteNumber;

/// `canonicalize`: RFC 8785 (JCS) — members sorted by UTF-16 code units, no
/// insignificant whitespace, ECMAScript number and string serialization.
pub fn canonicalize(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            // JSON numbers are finite; JS reads every number as a double.
            let f = n.as_f64().expect("serde_json numbers convert to f64");
            out.push_str(&js_number_to_string(f));
        }
        Value::String(s) => out.push_str(&json_string(s)),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| utf16_cmp(a, b));
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&json_string(key));
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
    }
}

/// Compares strings by UTF-16 code units (JavaScript's default sort order).
pub fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `JSON.stringify` of a string.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Lowercase hex SHA-256 of the UTF-8 bytes.
pub fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// `digestOf`: `sha256:` + hex of the canonical JSON.
pub fn digest_of(value: &Value) -> String {
    format!("sha256:{}", sha256_hex(&canonicalize(value)))
}

/// `isDigest`.
pub fn is_digest(value: &str) -> bool {
    DIGEST_PATTERN.is_match(value)
}

/// `contractDigest`: what an application *promises*, independent of where it is
/// served — the digest of the manifest without `transports` and `conformance`.
pub fn contract_digest_of_value(manifest: &Value) -> String {
    match manifest {
        Value::Object(map) => {
            let mut contract = map.clone();
            contract.remove("transports");
            contract.remove("conformance");
            digest_of(&Value::Object(contract))
        }
        other => digest_of(other),
    }
}

// ---------------------------------------------------------------------------
// Semantic annotations (integration.js)
// ---------------------------------------------------------------------------

/// `String(value)` for a JSON value.
pub fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => js_number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        // Array.prototype.join: null/undefined elements become "".
        Value::Array(items) => {
            items.iter().map(|v| if v.is_null() { String::new() } else { js_string(v) }).collect::<Vec<_>>().join(",")
        }
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `Object.entries` order: array-index keys ascending, then other keys in insertion order.
pub fn js_entries(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    fn array_index(key: &str) -> Option<u32> {
        let n: u32 = key.parse().ok()?;
        (n != u32::MAX && n.to_string() == key).then_some(n)
    }
    let (mut indexed, named): (Vec<_>, Vec<_>) = map.iter().partition(|(k, _)| array_index(k).is_some());
    indexed.sort_by_key(|(k, _)| array_index(k));
    indexed.extend(named);
    indexed
}

fn check_annotations(node: &Map<String, Value>, path: &str) -> Vec<SemanticIssue> {
    let mut issues = Vec::new();
    let mut push = |message: String| issues.push(SemanticIssue { path: path.to_string(), message });
    let [concept_kw, unit_kw, enum_kw, encoding_kw] = [
        SEMANTIC_KEYWORDS.concept,
        SEMANTIC_KEYWORDS.unit,
        SEMANTIC_KEYWORDS.enum_concepts,
        SEMANTIC_KEYWORDS.encoding,
    ];

    if let Some(concept) = node.get(concept_kw) {
        if !concept.as_str().is_some_and(|s| IRI.is_match(s)) {
            push(format!("{concept_kw} must be an absolute IRI"));
        }
    }
    if let Some(unit) = node.get(unit_kw) {
        if !unit.as_str().is_some_and(|s| UNIT.is_match(s)) {
            push(format!("{unit_kw} must be a non-empty unit code without whitespace"));
        }
    }
    if let Some(encoding) = node.get(encoding_kw) {
        if !encoding.as_str().is_some_and(|s| SEMANTIC_ENCODINGS.contains(&s)) {
            push(format!("{encoding_kw} must be one of: {}", SEMANTIC_ENCODINGS.join(", ")));
        }
    }
    if let Some(enum_concepts) = node.get(enum_kw) {
        match enum_concepts {
            Value::Object(map) => {
                let declared: Option<Vec<String>> =
                    node.get("enum").and_then(Value::as_array).map(|values| values.iter().map(js_string).collect());
                for (value, iri) in js_entries(map) {
                    if !iri.as_str().is_some_and(|s| IRI.is_match(s)) {
                        push(format!("{enum_kw}.{value} must be an absolute IRI"));
                    }
                    match &declared {
                        None => push(format!("{enum_kw} requires an enum on the same schema")),
                        Some(declared) if !declared.contains(value) => {
                            push(format!("{enum_kw} names \"{value}\", which is not a declared enum value"))
                        }
                        Some(_) => {}
                    }
                }
            }
            _ => push(format!("{enum_kw} must be an object of enum value → IRI")),
        }
    }
    issues
}

/// `validateSemanticAnnotations`: walks a schema tree and reports malformed
/// `x-appport-*` annotations. Other `x-` keywords are ignored.
///
/// Takes raw JSON so it also works on documents that do not parse as a
/// [`Schema`](super::Schema) (exactly like the JS function, which takes any object).
pub fn validate_semantic_annotations(schema: &Value, path: &str) -> Vec<SemanticIssue> {
    let Value::Object(node) = schema else {
        return Vec::new();
    };
    let mut issues = check_annotations(node, path);
    let mut visit = |child: &Value, child_path: String| {
        if child.is_object() {
            issues.extend(validate_semantic_annotations(child, &child_path));
        }
    };
    for key in ["properties", "$defs"] {
        match node.get(key) {
            Some(Value::Object(group)) => {
                for (name, child) in js_entries(group) {
                    visit(child, format!("{path}/{key}/{name}"));
                }
            }
            Some(Value::Array(group)) => {
                for (index, child) in group.iter().enumerate() {
                    visit(child, format!("{path}/{key}/{index}"));
                }
            }
            _ => {}
        }
    }
    for key in ["items", "additionalProperties", "propertyNames", "not"] {
        if let Some(child) = node.get(key) {
            visit(child, format!("{path}/{key}"));
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(list)) = node.get(key) {
            for (index, child) in list.iter().enumerate() {
                visit(child, format!("{path}/{key}/{index}"));
            }
        }
    }
    issues
}
