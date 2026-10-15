//! Conformance of JSON values to AppPort schemas, and deterministic synthesis of
//! values that conform.
//!
//! The agent checks every output it observes against the declared output schema
//! itself, rather than trusting the runtime's (optional) output validation or the
//! model's description of what it wrote. Synthesis gives the agent inputs for
//! probing a capability from its contract alone.
//!
//! Covers the keyword subset of [`Schema`] (`@appport/protocol` `JSONSchema`).
//! `format` is checked for `date-time` and `email`; other formats are not checked.

use std::collections::BTreeMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::appport::knowledge::{AdditionalProperties, Schema, SchemaType};

const MAX_DEPTH: usize = 32;

/// One way a value fails its schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    /// JSON Pointer into the value.
    pub at: String,
    /// The schema keyword that failed.
    pub keyword: String,
    pub message: String,
}

/// Every violation of `schema` by `value`, in a deterministic order. Empty means conforming.
pub fn check(schema: &Schema, value: &Value) -> Vec<Violation> {
    let mut out = Vec::new();
    Checker { root: schema }.check(schema, value, "", 0, &mut out);
    out
}

/// Whether `value` conforms to `schema`.
pub fn conforms(schema: &Schema, value: &Value) -> bool {
    check(schema, value).is_empty()
}

struct Checker<'a> {
    root: &'a Schema,
}

impl<'a> Checker<'a> {
    fn resolve(&self, schema: &'a Schema) -> Option<&'a Schema> {
        match schema.reference.as_deref() {
            None => Some(schema),
            Some(reference) => {
                let name = reference.strip_prefix("#/$defs/")?.replace("~1", "/").replace("~0", "~");
                self.root.defs.as_ref()?.get(&name)
            }
        }
    }

    fn check(&self, schema: &'a Schema, value: &Value, at: &str, depth: usize, out: &mut Vec<Violation>) {
        let mut fail = |keyword: &str, message: String| {
            out.push(Violation { at: at.to_string(), keyword: keyword.to_string(), message })
        };
        if depth > MAX_DEPTH {
            fail("$ref", "schema nests too deeply to check".into());
            return;
        }
        let Some(schema) = self.resolve(schema) else {
            fail("$ref", format!("unresolvable reference {:?}", schema.reference));
            return;
        };

        if let Some(types) = &schema.types {
            if !types.as_slice().iter().any(|t| is_type(value, *t)) {
                fail("type", format!("expected {}, got {}", type_names(types.as_slice()), describe(value)));
                return;
            }
        }
        if let Some(options) = &schema.enumeration {
            if !options.iter().any(|o| json_eq(o, value)) {
                fail("enum", format!("{} is not one of {}", compact(value), compact(&Value::Array(options.clone()))));
            }
        }
        if let Some(constant) = &schema.constant {
            if !json_eq(constant, value) {
                fail("const", format!("expected {}, got {}", compact(constant), compact(value)));
            }
        }

        match value {
            Value::Object(object) => self.check_object(schema, object, at, depth, out),
            Value::Array(items) => self.check_array(schema, items, at, depth, out),
            Value::String(text) => check_string(schema, text, at, out),
            Value::Number(number) => check_number(schema, number, at, out),
            _ => {}
        }

        for variant in schema.all_of.iter().flatten() {
            self.check(variant, value, at, depth + 1, out);
        }
        if let Some(variants) = &schema.any_of {
            if !variants.iter().any(|v| self.passes(v, value, at, depth)) {
                out.push(violation(at, "anyOf", "matches none of the anyOf variants"));
            }
        }
        if let Some(variants) = &schema.one_of {
            let matching = variants.iter().filter(|v| self.passes(v, value, at, depth)).count();
            if matching != 1 {
                out.push(violation(at, "oneOf", &format!("matches {matching} oneOf variants, expected exactly 1")));
            }
        }
        if let Some(not) = &schema.not {
            if self.passes(not, value, at, depth) {
                out.push(violation(at, "not", "matches a schema it must not match"));
            }
        }
    }

    fn passes(&self, schema: &'a Schema, value: &Value, at: &str, depth: usize) -> bool {
        let mut scratch = Vec::new();
        self.check(schema, value, at, depth + 1, &mut scratch);
        scratch.is_empty()
    }

    fn check_object(
        &self,
        schema: &'a Schema,
        object: &Map<String, Value>,
        at: &str,
        depth: usize,
        out: &mut Vec<Violation>,
    ) {
        for name in schema.required.iter().flatten() {
            if !object.contains_key(name) {
                out.push(violation(&child(at, name), "required", &format!("missing required property {name:?}")));
            }
        }
        for (name, member) in object {
            let pointer = child(at, name);
            if let Some(property) = schema.property(name) {
                self.check(property, member, &pointer, depth + 1, out);
                continue;
            }
            match &schema.additional_properties {
                Some(AdditionalProperties::Allowed(false)) => {
                    out.push(violation(&pointer, "additionalProperties", &format!("property {name:?} is not declared")))
                }
                Some(AdditionalProperties::Schema(extra)) => self.check(extra, member, &pointer, depth + 1, out),
                _ => {}
            }
            if let Some(names) = &schema.property_names {
                self.check(names, &Value::String(name.clone()), &pointer, depth + 1, out);
            }
        }
    }

    fn check_array(&self, schema: &'a Schema, items: &[Value], at: &str, depth: usize, out: &mut Vec<Violation>) {
        let len = items.len() as f64;
        if let Some(min) = schema.min_items.as_ref().and_then(Number::as_f64) {
            if len < min {
                out.push(violation(at, "minItems", &format!("{len} items, at least {min} required")));
            }
        }
        if let Some(max) = schema.max_items.as_ref().and_then(Number::as_f64) {
            if len > max {
                out.push(violation(at, "maxItems", &format!("{len} items, at most {max} allowed")));
            }
        }
        if schema.unique_items == Some(true) {
            for (i, a) in items.iter().enumerate() {
                if items[..i].iter().any(|b| json_eq(a, b)) {
                    out.push(violation(&child(at, &i.to_string()), "uniqueItems", "duplicate item"));
                }
            }
        }
        if let Some(item) = &schema.items {
            for (index, value) in items.iter().enumerate() {
                self.check(item, value, &child(at, &index.to_string()), depth + 1, out);
            }
        }
    }
}

fn check_string(schema: &Schema, text: &str, at: &str, out: &mut Vec<Violation>) {
    // JavaScript string length (UTF-16 code units), as the runtime measures it.
    let len = text.encode_utf16().count() as f64;
    if let Some(min) = schema.min_length.as_ref().and_then(Number::as_f64) {
        if len < min {
            out.push(violation(at, "minLength", &format!("length {len}, at least {min} required")));
        }
    }
    if let Some(max) = schema.max_length.as_ref().and_then(Number::as_f64) {
        if len > max {
            out.push(violation(at, "maxLength", &format!("length {len}, at most {max} allowed")));
        }
    }
    if let Some(pattern) = &schema.pattern {
        // A pattern the Rust engine cannot compile is left to the runtime.
        if let Ok(regex) = Regex::new(pattern) {
            if !regex.is_match(text) {
                out.push(violation(at, "pattern", &format!("{text:?} does not match /{pattern}/")));
            }
        }
    }
    match schema.format.as_deref() {
        Some("date-time") if !is_date_time(text) => {
            out.push(violation(at, "format", &format!("{text:?} is not an RFC 3339 date-time")))
        }
        Some("email") if !is_email(text) => out.push(violation(at, "format", &format!("{text:?} is not an email"))),
        _ => {}
    }
}

fn check_number(schema: &Schema, number: &Number, at: &str, out: &mut Vec<Violation>) {
    let Some(n) = number.as_f64() else { return };
    let bound = |b: &Option<Number>| b.as_ref().and_then(Number::as_f64);
    if let Some(min) = bound(&schema.minimum) {
        if n < min {
            out.push(violation(at, "minimum", &format!("{n} < {min}")));
        }
    }
    if let Some(max) = bound(&schema.maximum) {
        if n > max {
            out.push(violation(at, "maximum", &format!("{n} > {max}")));
        }
    }
    if let Some(min) = bound(&schema.exclusive_minimum) {
        if n <= min {
            out.push(violation(at, "exclusiveMinimum", &format!("{n} <= {min}")));
        }
    }
    if let Some(max) = bound(&schema.exclusive_maximum) {
        if n >= max {
            out.push(violation(at, "exclusiveMaximum", &format!("{n} >= {max}")));
        }
    }
    if let Some(step) = bound(&schema.multiple_of) {
        if step > 0.0 && ((n / step).round() * step - n).abs() > 1e-9 * n.abs().max(1.0) {
            out.push(violation(at, "multipleOf", &format!("{n} is not a multiple of {step}")));
        }
    }
}

fn violation(at: &str, keyword: &str, message: &str) -> Violation {
    Violation { at: at.to_string(), keyword: keyword.to_string(), message: message.to_string() }
}

fn child(at: &str, segment: &str) -> String {
    format!("{at}/{}", segment.replace('~', "~0").replace('/', "~1"))
}

fn is_type(value: &Value, t: SchemaType) -> bool {
    match t {
        SchemaType::Null => value.is_null(),
        SchemaType::Boolean => value.is_boolean(),
        SchemaType::Object => value.is_object(),
        SchemaType::Array => value.is_array(),
        SchemaType::Number => value.is_number(),
        SchemaType::Integer => match value {
            Value::Number(n) => n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0),
            _ => false,
        },
        SchemaType::String => value.is_string(),
    }
}

fn type_names(types: &[SchemaType]) -> String {
    types
        .iter()
        .map(|t| serde_json::to_value(t).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn compact(value: &Value) -> String {
    let text = value.to_string();
    if text.len() > 120 {
        format!("{}…", &text[..text.char_indices().nth(117).map_or(text.len(), |(i, _)| i)])
    } else {
        text
    }
}

/// JSON equality with JavaScript number semantics (`1` equals `1.0`).
pub fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_eq(a, b)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        _ => a == b,
    }
}

fn is_date_time(text: &str) -> bool {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN
        .get_or_init(|| {
            Regex::new(r"^\d{4}-\d{2}-\d{2}[Tt ]\d{2}:\d{2}:\d{2}(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$").expect("valid regex")
        })
        .is_match(text)
}

fn is_email(text: &str) -> bool {
    match text.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !text.chars().any(char::is_whitespace)
                && !domain.contains('@')
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Synthesis
// ---------------------------------------------------------------------------

/// Synthesizes a value that conforms to `schema`, deterministically from `seed`.
///
/// Only required object members are produced, so the value is the smallest a
/// caller must send. `overrides` maps JSON Pointers to values to place instead
/// (for example a known identifier). The result is not guaranteed to conform
/// (a `pattern` cannot be synthesized in general): callers check it with [`check`].
pub fn synthesize(schema: &Schema, seed: u64, overrides: &BTreeMap<String, Value>) -> Value {
    Synth { root: schema, seed, overrides }.value(schema, "", 0)
}

struct Synth<'a> {
    root: &'a Schema,
    seed: u64,
    overrides: &'a BTreeMap<String, Value>,
}

impl<'a> Synth<'a> {
    fn value(&self, schema: &'a Schema, at: &str, depth: usize) -> Value {
        if let Some(value) = self.overrides.get(at) {
            return value.clone();
        }
        let checker = Checker { root: self.root };
        let Some(schema) = checker.resolve(schema) else { return Value::Null };
        if depth > MAX_DEPTH {
            return Value::Null;
        }
        if let Some(constant) = &schema.constant {
            return constant.clone();
        }
        if let Some(options) = schema.enumeration.as_ref().filter(|o| !o.is_empty()) {
            return options[(self.seed as usize) % options.len()].clone();
        }
        if let Some(default) = &schema.default {
            if conforms(schema, default) {
                return default.clone();
            }
        }
        if let Some(first) = schema.any_of.as_ref().or(schema.one_of.as_ref()).and_then(|v| {
            v.iter().find(|s| s.types.as_ref().map_or(true, |t| t.as_slice() != [SchemaType::Null])).or(v.first())
        }) {
            return self.value(first, at, depth + 1);
        }
        let types = schema.types.as_ref().map(|t| t.as_slice().to_vec()).unwrap_or_default();
        let pick = [
            SchemaType::Object,
            SchemaType::String,
            SchemaType::Integer,
            SchemaType::Number,
            SchemaType::Boolean,
            SchemaType::Array,
            SchemaType::Null,
        ]
        .into_iter()
        .find(|t| types.contains(t))
        .unwrap_or(if schema.properties.is_some() { SchemaType::Object } else { SchemaType::Null });
        match pick {
            SchemaType::Object => {
                let mut object = Map::new();
                for (name, property) in schema.properties.iter().flatten() {
                    let pointer = child(at, name);
                    if schema.requires(name) || self.overrides.contains_key(&pointer) {
                        object.insert(name.clone(), self.value(property, &pointer, depth + 1));
                    }
                }
                Value::Object(object)
            }
            SchemaType::String => Value::String(self.string(schema, at)),
            SchemaType::Integer | SchemaType::Number => self.number(schema, pick == SchemaType::Integer),
            SchemaType::Boolean => Value::Bool(true),
            SchemaType::Array => {
                let count = schema.min_items.as_ref().and_then(Number::as_u64).unwrap_or(0);
                let item = schema.items.as_deref();
                Value::Array(
                    (0..count)
                        .map(|i| item.map_or(Value::Null, |s| self.value(s, &child(at, &i.to_string()), depth + 1)))
                        .collect(),
                )
            }
            SchemaType::Null => Value::Null,
        }
    }

    fn string(&self, schema: &Schema, at: &str) -> String {
        let field = at.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("value");
        let mut text = match schema.format.as_deref() {
            Some("email") => format!("probe-{}-{field}@example.com", self.seed).to_lowercase(),
            Some("date-time") => "2026-01-01T00:00:00.000Z".to_string(),
            Some("date") => "2026-01-01".to_string(),
            Some("uri") => format!("https://example.com/probe/{}", self.seed),
            Some("uuid") => format!("00000000-0000-4000-8000-{:012x}", self.seed),
            _ => format!("probe-{}-{field}", self.seed),
        };
        let min = schema.min_length.as_ref().and_then(Number::as_u64).unwrap_or(0) as usize;
        while text.encode_utf16().count() < min {
            text.push('x');
        }
        if let Some(max) = schema.max_length.as_ref().and_then(Number::as_u64) {
            text = text.chars().take(max as usize).collect();
        }
        text
    }

    fn number(&self, schema: &Schema, integer: bool) -> Value {
        let f = |b: &Option<Number>| b.as_ref().and_then(Number::as_f64);
        let mut n = 1.0_f64;
        if let Some(min) = f(&schema.minimum) {
            n = n.max(min);
        }
        if let Some(min) = f(&schema.exclusive_minimum) {
            if n <= min {
                n = if integer { min.floor() + 1.0 } else { min + 1.0 };
            }
        }
        if let Some(max) = f(&schema.maximum) {
            n = n.min(max);
        }
        if integer {
            n = n.ceil();
            return Value::from(n as i64);
        }
        Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

/// Resolves an RFC 6901 pointer, treating `""` as the whole document.
pub fn at_pointer<'v>(value: &'v Value, pointer: &str) -> Option<&'v Value> {
    if pointer.is_empty() {
        Some(value)
    } else {
        value.pointer(pointer)
    }
}

/// Every value found at a field pointer that may contain `*` (each array element
/// or object member), as produced by [`crate::appport::knowledge::fields`].
pub fn values_at<'v>(value: &'v Value, pointer: &str) -> Vec<&'v Value> {
    let mut current = vec![value];
    for segment in pointer.split('/').skip(1) {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        current = current
            .into_iter()
            .flat_map(|v| match (v, segment.as_str()) {
                (Value::Array(items), "*") => items.iter().collect::<Vec<_>>(),
                (Value::Object(members), "*") => members.values().collect(),
                (Value::Object(members), name) => members.get(name).into_iter().collect(),
                (Value::Array(items), index) => {
                    index.parse::<usize>().ok().and_then(|i| items.get(i)).into_iter().collect()
                }
                _ => Vec::new(),
            })
            .collect();
    }
    current
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema(value: Value) -> Schema {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn checks_types_required_enum_and_additional_properties() {
        let s = schema(json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "minLength": 1 },
                "tier": { "type": "string", "enum": ["standard", "gold"] },
                "count": { "type": "integer", "minimum": 0 }
            },
            "required": ["id", "tier"],
            "additionalProperties": false
        }));
        assert!(conforms(&s, &json!({ "id": "a", "tier": "gold", "count": 2.0 })));
        let issues = check(&s, &json!({ "tier": "platinum", "count": -1, "extra": true }));
        let keywords: Vec<(&str, &str)> = issues.iter().map(|v| (v.at.as_str(), v.keyword.as_str())).collect();
        assert_eq!(
            keywords,
            [("/id", "required"), ("/tier", "enum"), ("/count", "minimum"), ("/extra", "additionalProperties")]
        );
        assert_eq!(check(&s, &json!([]))[0].keyword, "type");
    }

    #[test]
    fn checks_formats_refs_and_unions() {
        let s = schema(json!({
            "$defs": { "when": { "type": "string", "format": "date-time" } },
            "type": "object",
            "properties": {
                "at": { "$ref": "#/$defs/when" },
                "mail": { "type": "string", "format": "email" },
                "maybe": { "anyOf": [{ "type": "null" }, { "type": "integer" }] }
            }
        }));
        assert!(conforms(&s, &json!({ "at": "2020-01-01T00:00:00Z", "mail": "a@b.co", "maybe": null })));
        let issues = check(&s, &json!({ "at": "yesterday", "mail": "nope", "maybe": "x" }));
        assert_eq!(issues.iter().map(|v| v.keyword.as_str()).collect::<Vec<_>>(), ["format", "format", "anyOf"]);
    }

    #[test]
    fn synthesizes_conforming_minimal_values() {
        let s = schema(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 12 },
                "email": { "type": "string", "format": "email" },
                "tier": { "type": "string", "enum": ["standard", "gold", "platinum"] },
                "qty": { "type": "integer", "exclusiveMinimum": 3 },
                "note": { "type": "string" }
            },
            "required": ["name", "email", "tier", "qty"]
        }));
        let value = synthesize(&s, 1, &BTreeMap::new());
        assert!(conforms(&s, &value), "{value}");
        assert_eq!(value["tier"], "gold");
        assert_eq!(value["qty"], 4);
        assert!(value.get("note").is_none());
        let pinned = synthesize(&s, 1, &BTreeMap::from([("/note".to_string(), json!("kept"))]));
        assert_eq!(pinned["note"], "kept");
        assert_eq!(synthesize(&s, 1, &BTreeMap::new()), value, "deterministic");
    }

    #[test]
    fn finds_values_through_wildcards() {
        let v = json!({ "customers": [{ "id": "a" }, { "id": "b" }] });
        assert_eq!(values_at(&v, "/customers/*/id"), [&json!("a"), &json!("b")]);
        assert_eq!(values_at(&v, "/customers/1/id"), [&json!("b")]);
        assert!(values_at(&v, "/missing").is_empty());
    }
}
