use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;

use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};

use crate::error::{CraftError, ErrorCode, FieldIssue};

pub const MAX_INPUT_BYTES: usize = 1_048_576;
const MAX_DEPTH: usize = 64;
const MAX_CONTAINER_ITEMS: usize = 10_000;

struct JsonSeed<'a> {
    depth: usize,
    maximum_depth: usize,
    path: String,
    issue: &'a RefCell<Option<FieldIssue>>,
}

fn remember(issue: &RefCell<Option<FieldIssue>>, path: &str, rule: &str) {
    if issue.borrow().is_none() {
        issue.replace(Some(FieldIssue::new(path, rule)));
    }
}

pub(crate) fn pointer_child(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
        .chars()
        .take(257)
        .collect()
}

impl<'de> DeserializeSeed<'de> for JsonSeed<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if self.depth > self.maximum_depth {
            remember(self.issue, &self.path, "json_depth_limit_exceeded");
            return Err(D::Error::custom("JSON nesting limit exceeded"));
        }
        deserializer
            .deserialize_any(JsonVisitor {
                depth: self.depth,
                maximum_depth: self.maximum_depth,
                path: &self.path,
                issue: self.issue,
            })
            .inspect_err(|_| remember(self.issue, &self.path, "invalid_json"))
    }
}

struct JsonVisitor<'a> {
    depth: usize,
    maximum_depth: usize,
    path: &'a str,
    issue: &'a RefCell<Option<FieldIssue>>,
}

impl<'de> Visitor<'de> for JsonVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON without duplicate properties")
    }

    fn visit_bool<E: Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("Expected a finite number"))
    }
    fn visit_str<E: Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(JsonSeed {
            depth: self.depth + 1,
            maximum_depth: self.maximum_depth,
            path: pointer_child(self.path, &values.len().to_string()),
            issue: self.issue,
        })? {
            if values.len() == MAX_CONTAINER_ITEMS {
                remember(
                    self.issue,
                    &pointer_child(self.path, &values.len().to_string()),
                    "array_item_limit_exceeded",
                );
                return Err(A::Error::custom("Array limit exceeded"));
            }
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                remember(
                    self.issue,
                    &pointer_child(self.path, &key),
                    "duplicate_property",
                );
                return Err(A::Error::custom("Duplicate JSON property"));
            }
            if values.len() == MAX_CONTAINER_ITEMS {
                remember(
                    self.issue,
                    &pointer_child(self.path, &key),
                    "object_property_limit_exceeded",
                );
                return Err(A::Error::custom("Object property limit exceeded"));
            }
            let value = map.next_value_seed(JsonSeed {
                depth: self.depth + 1,
                maximum_depth: self.maximum_depth,
                path: pointer_child(self.path, &key),
                issue: self.issue,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

/// Parses a complete object before any typed operation can trigger external effects.
pub fn parse_object(bytes: &[u8]) -> Result<Value, CraftError> {
    let value = parse_json(bytes, MAX_INPUT_BYTES)?;
    if !value.is_object() {
        return Err(CraftError::field("", "expected_object"));
    }
    Ok(value)
}

/// Parses bounded response values with the same UTF-8, duplicate, depth and container checks.
pub fn parse_json(bytes: &[u8], maximum: usize) -> Result<Value, CraftError> {
    if maximum > 16_777_216 {
        return Err(CraftError::field("", "input_byte_limit_exceeded"));
    }
    parse_with_limits(bytes, maximum, MAX_DEPTH)
}

/// Parses stored document envelopes with bounded wrapper depth, without widening CLI or HTTP limits.
pub fn parse_document_json(bytes: &[u8], maximum: usize) -> Result<Value, CraftError> {
    if maximum > 33_554_432 {
        return Err(CraftError::field("", "input_byte_limit_exceeded"));
    }
    parse_with_limits(bytes, maximum, 96)
}

fn parse_with_limits(
    bytes: &[u8],
    maximum: usize,
    maximum_depth: usize,
) -> Result<Value, CraftError> {
    if maximum == 0 || bytes.len() > maximum {
        return Err(CraftError::field("", "input_byte_limit_exceeded"));
    }
    std::str::from_utf8(bytes).map_err(|_| CraftError::field("", "expected_utf8_json"))?;
    let issue = RefCell::new(None);
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = JsonSeed {
        depth: 0,
        maximum_depth,
        path: String::new(),
        issue: &issue,
    }
    .deserialize(&mut deserializer)
    .map_err(|syntax| {
        let mut error = CraftError::new(
            ErrorCode::InputInvalid,
            format!(
                "Invalid JSON at line {} column {}.",
                syntax.line(),
                syntax.column()
            ),
        );
        if let Some(issue) = issue.take() {
            error.details.issues.push(issue);
        }
        error
    })?;
    deserializer
        .end()
        .map_err(|_| CraftError::field("", "trailing_json"))?;
    Ok(value)
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let ordered: BTreeMap<_, _> = object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_value(value)))
                .collect();
            Value::Object(ordered.into_iter().collect())
        }
        Value::Array(array) => Value::Array(array.iter().map(canonical_value).collect()),
        other => other.clone(),
    }
}

/// Uses versioned Craft canonicalization, not a claim of universal JSON canonicalization.
pub fn canonical_digest(value: &Value) -> Result<String, CraftError> {
    let bytes = serde_json::to_vec(&canonical_value(value))
        .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode canonical request."))?;
    let mut digest = Sha256::new();
    digest.update(b"craft-json-v1\0");
    digest.update(bytes);
    Ok(hex_digest(digest))
}

/// Hashes exact artifact bytes, independently of JSON canonicalization.
pub fn content_digest(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex_digest(digest)
}

fn hex_digest(digest: Sha256) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(71);
    result.push_str("sha256:");
    for byte in digest.finalize() {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    result
}
