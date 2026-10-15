//! Single-value ("literal") types.
//!
//! TypeScript expresses discriminants and fixed identifiers as literal types
//! (`kind: "state"`, `required: true`, `protocol: "appport/1"`). These types
//! carry exactly that one value, so a document with any other value is
//! rejected at parse time instead of being silently accepted.

use std::borrow::Cow;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Declares a unit type that (de)serializes as exactly one string.
macro_rules! string_literal {
    ($(#[$meta:meta])* $name:ident = $value:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name;

        impl $name {
            /// The only value this type accepts.
            pub const VALUE: &'static str = $value;
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str($value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = Cow::<'de, str>::deserialize(deserializer)?;
                if text == $value {
                    Ok($name)
                } else {
                    Err(de::Error::invalid_value(de::Unexpected::Str(&text), &concat!("\"", $value, "\"")))
                }
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }
            fn inline_schema() -> bool {
                true
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({ "type": "string", "const": $value })
            }
        }
    };
}

/// Declares a unit type that (de)serializes as exactly one boolean.
macro_rules! bool_literal {
    ($(#[$meta:meta])* $name:ident = $value:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name;

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_bool($value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                if bool::deserialize(deserializer)? == $value {
                    Ok($name)
                } else {
                    Err(de::Error::invalid_value(de::Unexpected::Bool(!$value), &stringify!($value)))
                }
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }
            fn inline_schema() -> bool {
                true
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({ "type": "boolean", "const": $value })
            }
        }
    };
}

bool_literal!(
    /// `true`, and nothing else.
    True = true
);
bool_literal!(
    /// `false`, and nothing else.
    False = false
);

string_literal!(
    /// `"appport/1"` — the protocol identifier (`PROTOCOL_ID`).
    ProtocolId = "appport/1"
);
string_literal!(
    /// `"AppPort/ui/1"` (`UI_PROTOCOL_ID`).
    UiProtocolId = "AppPort/ui/1"
);
string_literal!(
    /// `kind: "state"` on a state verification method.
    StateKind = "state"
);
string_literal!(
    /// `kind: "event"` on an event verification method.
    EventKind = "event"
);
string_literal!(
    /// `strength: "corroborating"` — an event can corroborate, never establish.
    Corroborating = "corroborating"
);
string_literal!(
    /// `type` of a [`VerificationEvidence`](super::VerificationEvidence) record.
    VerificationEvidenceType = "appport.verification-evidence/1"
);
string_literal!(
    /// `type` of an [`InvocationEvidence`](super::InvocationEvidence) record (`INVOCATION_EVIDENCE`).
    InvocationEvidenceType = "appport.evidence.invocation/1"
);
string_literal!(
    /// `type` of a [`StateAccessEvidence`](super::StateAccessEvidence) record (`STATE_ACCESS_EVIDENCE`).
    StateAccessEvidenceType = "appport.evidence.state-access/1"
);
string_literal!(
    /// `"self"`: the application is the system of record for a resource.
    SelfOwner = "self"
);
string_literal!(
    /// Handover status literal.
    Active = "active"
);
string_literal!(
    /// Handover status literal.
    Retired = "retired"
);
string_literal!(
    /// Handover status literal.
    Registered = "registered"
);

/// Deserializes an optional field so that an explicit `null` is kept as
/// `Some(null)` instead of collapsing into "absent". Used where `null` is a
/// meaningful value (`const`, `default`, `equals`, `observed`).
pub(crate) fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_accept_only_their_value() {
        assert_eq!(serde_json::from_str::<ProtocolId>("\"appport/1\"").unwrap(), ProtocolId);
        assert!(serde_json::from_str::<ProtocolId>("\"appport/2\"").is_err());
        assert_eq!(serde_json::to_string(&StateKind).unwrap(), "\"state\"");
        assert!(serde_json::from_str::<True>("true").is_ok());
        assert!(serde_json::from_str::<True>("false").is_err());
        assert!(serde_json::from_str::<False>("false").is_ok());
    }
}
