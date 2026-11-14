use super::{EDIT, TRANSITIONS, malformed, response};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{OperationContext, PreparedOperation};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn projection(command: &str) -> ProjectionContract {
    ProjectionContract {
        item_pointer: "/items".into(),
        always_returned: vec!["id".into(), "fields_not_returned".into()],
        default_selected: if command == EDIT {
            vec!["id", "name", "required", "operations", "schema.type"]
        } else {
            vec![
                "id",
                "name",
                "to.id",
                "to.name",
                "has_screen",
                "is_available",
            ]
        }
        .into_iter()
        .map(String::from)
        .collect(),
        mode: ProjectionMode::Local,
    }
}
#[derive(PartialEq, Eq)]
pub struct FieldPlan {
    command: String,
    selected: BTreeSet<String>,
}
impl FieldPlan {
    pub fn compile(command: &str, fields: &[String]) -> Result<Self, CraftError> {
        let contract = projection(command);
        contract.compile(&response(command)?, fields)?;
        let selected = if fields.is_empty() {
            contract.default_selected
        } else {
            fields.to_vec()
        }
        .into_iter()
        .chain(contract.always_returned)
        .collect();
        Ok(Self {
            command: command.into(),
            selected,
        })
    }
    pub(super) fn verify(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        if *self
            != Self::compile(
                &operation.request().command,
                &operation.request().selection.fields,
            )?
        {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Issue metadata fields differ from the prepared operation.",
            ));
        }
        Ok(())
    }
    fn wants(&self, path: &str) -> bool {
        self.selected.iter().any(|selected| {
            selected == path
                || selected.starts_with(&format!("{path}."))
                || path.starts_with(&format!("{selected}."))
        })
    }
    pub fn transition_fields(&self) -> bool {
        self.command == TRANSITIONS && self.wants("fields")
    }
    fn object(
        &self,
        raw: &Value,
        prefix: &str,
        nodes: &[Node],
        control: &dyn OperationContext,
    ) -> Result<Value, CraftError> {
        if !raw.is_object() {
            return Err(malformed());
        }
        let mut out = json!({});
        for Node(name, source, shape) in nodes {
            let path = if prefix.is_empty() {
                (*name).into()
            } else {
                format!("{prefix}.{name}")
            };
            if !self.wants(&path) {
                continue;
            }
            if let Some(value) = raw.get(source) {
                out[*name] = if value.is_null() {
                    Value::Null
                } else {
                    match shape {
                        Shape::Object(children) => self.object(value, &path, children, control)?,
                        Shape::Fields => json!(self.fields(value, &path, control)?),
                        _ => {
                            shape.check(value)?;
                            value.clone()
                        }
                    }
                };
            }
        }
        Ok(out)
    }
    fn missing(&self, out: &mut Value, prefix: &str) {
        let missing: Vec<_> = self
            .selected
            .iter()
            .filter_map(|path| {
                let local = if prefix.is_empty() {
                    path.as_str()
                } else {
                    path.strip_prefix(&format!("{prefix}."))?
                };
                (local != "fields_not_returned"
                    && !present(out, &local.split('.').collect::<Vec<_>>()))
                .then_some(local.to_owned())
            })
            .collect();
        if !missing.is_empty() {
            out["fields_not_returned"] = json!(missing);
        }
    }
    pub(super) fn fields(
        &self,
        raw: &Value,
        prefix: &str,
        control: &dyn OperationContext,
    ) -> Result<Vec<Value>, CraftError> {
        let fields = raw
            .as_object()
            .filter(|fields| fields.len() <= 1000)
            .ok_or_else(malformed)?;
        let mut out = Vec::new();
        for (id, field) in fields {
            control.checkpoint()?;
            if id.is_empty()
                || id.len() > 128
                || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(malformed());
            }
            for name in ["key", "name"] {
                if field.get(name).and_then(Value::as_str).is_none_or(|value| {
                    value.is_empty() || value.len() > 4096 || value.contains('\0')
                }) {
                    return Err(malformed());
                }
            }
            if !field.get("required").is_some_and(Value::is_boolean) {
                return Err(malformed());
            }
            Shape::Operations.check(field.get("operations").ok_or_else(malformed)?)?;
            if field
                .get("schema")
                .and_then(|schema| schema.get("type"))
                .and_then(Value::as_str)
                .is_none_or(|kind| kind.is_empty() || kind.len() > 256 || kind.contains('\0'))
            {
                return Err(malformed());
            }
            let mut value = self.object(field, prefix, FIELD_NODES, control)?;
            value["id"] = json!(id);
            self.missing(&mut value, prefix);
            out.push(value);
        }
        Ok(out)
    }
    pub(super) fn transition(
        &self,
        raw: &Value,
        control: &dyn OperationContext,
    ) -> Result<Value, CraftError> {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(malformed)?;
        craft_core::contracts::validate_identity("/transition/id", id).map_err(|_| malformed())?;
        let mut out = self.object(raw, "", TRANSITION_NODES, control)?;
        out["id"] = json!(id);
        self.missing(&mut out, "");
        Ok(out)
    }
}
fn present(value: &Value, path: &[&str]) -> bool {
    if path.is_empty() || value.is_null() {
        return true;
    }
    if let Some(values) = value.as_array() {
        return values.iter().all(|value| present(value, path));
    }
    value
        .get(path[0])
        .is_some_and(|value| present(value, &path[1..]))
}
struct Node(&'static str, &'static str, Shape);
enum Shape {
    Text(usize),
    Bool,
    Integer,
    Operations,
    AllowedValues,
    Any,
    Configuration,
    Object(&'static [Node]),
    Fields,
}
impl Shape {
    fn check(&self, value: &Value) -> Result<(), CraftError> {
        let valid = match self {
            Self::Text(max) => value
                .as_str()
                .is_some_and(|text| text.len() <= *max && !text.contains('\0')),
            Self::Bool => value.is_boolean(),
            Self::Integer => value.as_i64().is_some_and(|value| value >= 0),
            Self::Operations => value.as_array().is_some_and(|values| {
                values.len() <= 16
                    && values.iter().all(|value| {
                        value.as_str().is_some_and(|text| {
                            !text.is_empty()
                                && text.len() <= 64
                                && !text.chars().any(char::is_control)
                        })
                    })
                    && values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == values.len()
            }),
            Self::AllowedValues => {
                value.as_array().is_some_and(|values| values.len() <= 1000)
                    && serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= 65_536)
            }
            Self::Any => serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= 32_768),
            Self::Configuration => {
                value.as_object().is_some_and(|values| values.len() <= 64)
                    && serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= 32_768)
            }
            _ => false,
        };
        if valid { Ok(()) } else { Err(malformed()) }
    }
}
const SCHEMA: &[Node] = &[
    Node("type", "type", Shape::Text(256)),
    Node("items", "items", Shape::Text(256)),
    Node("system", "system", Shape::Text(512)),
    Node("custom", "custom", Shape::Text(4096)),
    Node("custom_id", "customId", Shape::Integer),
    Node("configuration", "configuration", Shape::Configuration),
];
const FIELD_NODES: &[Node] = &[
    Node("key", "key", Shape::Text(4096)),
    Node("name", "name", Shape::Text(4096)),
    Node("required", "required", Shape::Bool),
    Node("operations", "operations", Shape::Operations),
    Node("schema", "schema", Shape::Object(SCHEMA)),
    Node("has_default", "hasDefaultValue", Shape::Bool),
    Node("allowed_values", "allowedValues", Shape::AllowedValues),
    Node("default_value", "defaultValue", Shape::Any),
    Node("configuration", "configuration", Shape::Configuration),
];
const CATEGORY: &[Node] = &[
    Node("id", "id", Shape::Integer),
    Node("key", "key", Shape::Text(512)),
    Node("name", "name", Shape::Text(4096)),
];
const STATUS: &[Node] = &[
    Node("id", "id", Shape::Text(512)),
    Node("name", "name", Shape::Text(4096)),
    Node("description", "description", Shape::Text(16_384)),
    Node("category", "statusCategory", Shape::Object(CATEGORY)),
];
const TRANSITION_NODES: &[Node] = &[
    Node("name", "name", Shape::Text(4096)),
    Node("to", "to", Shape::Object(STATUS)),
    Node("has_screen", "hasScreen", Shape::Bool),
    Node("is_available", "isAvailable", Shape::Bool),
    Node("is_conditional", "isConditional", Shape::Bool),
    Node("is_global", "isGlobal", Shape::Bool),
    Node("is_initial", "isInitial", Shape::Bool),
    Node("looped", "looped", Shape::Bool),
    Node("fields", "fields", Shape::Fields),
];
