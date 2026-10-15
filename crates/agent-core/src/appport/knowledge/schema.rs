//! The wire-level schema dialect: AppPort's JSON Schema subset.
//!
//! Mirrors `JSONSchema` / `JSONSchemaType` from `@appport/protocol` (`jsonschema.d.ts`).

use std::collections::BTreeMap;

use indexmap::IndexMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use super::literal::present;

/// `JSONSchemaType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    Null,
    Boolean,
    Object,
    Array,
    Number,
    Integer,
    String,
}

impl SchemaType {
    /// Every value, in declaration order.
    pub const ALL: [SchemaType; 7] = [
        SchemaType::Null,
        SchemaType::Boolean,
        SchemaType::Object,
        SchemaType::Array,
        SchemaType::Number,
        SchemaType::Integer,
        SchemaType::String,
    ];
}

/// `type`: one type or a list of types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TypeSet {
    One(SchemaType),
    Many(Vec<SchemaType>),
}

impl TypeSet {
    /// The declared types as a slice.
    pub fn as_slice(&self) -> &[SchemaType] {
        match self {
            TypeSet::One(one) => std::slice::from_ref(one),
            TypeSet::Many(many) => many,
        }
    }
}

/// `additionalProperties`: a boolean or a schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AdditionalProperties {
    Allowed(bool),
    Schema(Box<Schema>),
}

/// `JSONSchema` — a pragmatic subset of draft 2020-12 (specification §32).
///
/// Numeric keywords keep the exact JSON number (`serde_json::Number`) so a
/// manifest round-trips byte-for-byte meaningfully (`0` stays `0`, not `0.0`).
/// Keywords outside the subset — in practice the `x-*` vendor annotations,
/// including AppPort's own `x-appport-*` semantic keywords — are kept in
/// [`Schema::extensions`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Schema {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(rename = "$id", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "$ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(rename = "$defs", default, skip_serializing_if = "Option::is_none")]
    pub defs: Option<IndexMap<String, Schema>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub types: Option<TypeSet>,
    #[serde(rename = "enum", default, skip_serializing_if = "Option::is_none")]
    pub enumeration: Option<Vec<Value>>,
    #[serde(rename = "const", default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub constant: Option<Value>,
    /// Kept in declaration order: property order is meaningful to readers and to models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<IndexMap<String, Schema>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    #[serde(rename = "additionalProperties", default, skip_serializing_if = "Option::is_none")]
    pub additional_properties: Option<AdditionalProperties>,
    #[serde(rename = "propertyNames", default, skip_serializing_if = "Option::is_none")]
    pub property_names: Option<Box<Schema>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<Schema>>,
    #[serde(rename = "minItems", default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<Number>,
    #[serde(rename = "maxItems", default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<Number>,
    #[serde(rename = "uniqueItems", default, skip_serializing_if = "Option::is_none")]
    pub unique_items: Option<bool>,
    #[serde(rename = "minLength", default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<Number>,
    #[serde(rename = "maxLength", default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<Number>,
    #[serde(rename = "exclusiveMinimum", default, skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<Number>,
    #[serde(rename = "exclusiveMaximum", default, skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<Number>,
    #[serde(rename = "multipleOf", default, skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<Number>,
    #[serde(rename = "anyOf", default, skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<Schema>>,
    #[serde(rename = "oneOf", default, skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<Schema>>,
    #[serde(rename = "allOf", default, skip_serializing_if = "Option::is_none")]
    pub all_of: Option<Vec<Schema>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not: Option<Box<Schema>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    /// `x-*` keywords (`[key: \`x-${string}\`]: unknown`).
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl Schema {
    /// The subschema for a named property.
    pub fn property(&self, name: &str) -> Option<&Schema> {
        self.properties.as_ref()?.get(name)
    }

    /// Declared property names, in declaration order.
    pub fn property_names_in_order(&self) -> Vec<&str> {
        self.properties.as_ref().map(|p| p.keys().map(String::as_str).collect()).unwrap_or_default()
    }

    /// Whether `name` is listed in `required`.
    pub fn requires(&self, name: &str) -> bool {
        self.required.as_ref().is_some_and(|r| r.iter().any(|n| n == name))
    }

    /// The `x-appport-*` keywords on this node (not its children).
    pub fn appport_annotations(&self) -> BTreeMap<&str, &Value> {
        self.extensions
            .iter()
            .filter(|(key, _)| key.starts_with("x-appport-"))
            .map(|(key, value)| (key.as_str(), value))
            .collect()
    }
}
