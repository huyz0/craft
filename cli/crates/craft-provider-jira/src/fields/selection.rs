use std::collections::BTreeSet;

use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::PreparedOperation;
use craft_core::projection::{ProjectionContract, ProjectionMode};
use serde_json::{Value, json};

use super::{LIST, SEARCH, malformed, models::*, response};

pub fn projection(command: &str) -> ProjectionContract {
    ProjectionContract {
        item_pointer: "/items".into(),
        always_returned: vec!["id".into(), "fields_not_returned".into()],
        default_selected: if command == LIST {
            vec!["id", "name", "custom", "schema.type"]
        } else {
            vec!["id", "name", "schema.type"]
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
                "Field projection differs from the prepared discovery operation.",
            ));
        }
        Ok(())
    }
    fn selects(&self, path: &str) -> bool {
        self.selected.iter().any(|selected| {
            selected == path
                || path.starts_with(&format!("{selected}."))
                || selected.starts_with(&format!("{path}."))
        })
    }
    pub fn expansions(&self) -> Vec<&'static str> {
        if self.command != SEARCH {
            return Vec::new();
        }
        [
            ("key", "key"),
            ("stable_id", "stableId"),
            ("last_used", "lastUsed"),
            ("screens_count", "screensCount"),
            ("contexts_count", "contextsCount"),
            ("is_locked", "isLocked"),
            ("searcher_key", "searcherKey"),
        ]
        .into_iter()
        .filter_map(|(path, native)| self.selects(path).then_some(native))
        .collect()
    }
    fn object(&self, raw: &Value, prefix: &str, nodes: &[Node]) -> Result<Value, CraftError> {
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
            if !self.selects(&path) {
                continue;
            }
            if let Some(value) = raw.get(source) {
                out[*name] = if value.is_null() {
                    Value::Null
                } else {
                    match shape {
                        Shape::Object(children) => self.object(value, &path, children)?,
                        _ => shape.check(value)?,
                    }
                };
            }
        }
        Ok(out)
    }
    pub(super) fn normalize(&self, raw: &Value) -> Result<Value, CraftError> {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control))
            .ok_or_else(malformed)?;
        if self.command == SEARCH {
            Shape::Text(4096).check(
                raw.get("name")
                    .filter(|value| value.as_str().is_some_and(|name| !name.is_empty()))
                    .ok_or_else(malformed)?,
            )?;
            // Some system fields (for example the issue key, parent and thumbnail)
            // come back from live search with no schema at all. Absence is reported
            // through `fields_not_returned`; a schema that is present must be typed.
            if let Some(schema) = raw.get("schema") {
                Shape::Text(256).check(
                    schema
                        .get("type")
                        .filter(|value| value.as_str().is_some_and(|kind| !kind.is_empty()))
                        .ok_or_else(malformed)?,
                )?;
            }
        }
        let nodes = if self.command == LIST {
            LIST_NODES
        } else {
            SEARCH_NODES
        };
        let mut out = self.object(raw, "", nodes)?;
        out["id"] = json!(id);
        let missing: Vec<_> = self
            .selected
            .iter()
            .filter(|path| {
                path.as_str() != "fields_not_returned"
                    && !present(&out, &path.split('.').collect::<Vec<_>>())
            })
            .cloned()
            .collect();
        if !missing.is_empty() {
            out["fields_not_returned"] = json!(missing);
        }
        if self.command == LIST {
            serde_json::from_value::<ListedField>(out.clone()).map_err(|_| malformed())?;
        } else {
            serde_json::from_value::<SearchedField>(out.clone()).map_err(|_| malformed())?;
        }
        Ok(out)
    }
}
fn present(value: &Value, path: &[&str]) -> bool {
    path.is_empty()
        || value.is_null()
        || value
            .get(path[0])
            .is_some_and(|value| present(value, &path[1..]))
}
struct Node(&'static str, &'static str, Shape);
enum Shape {
    Text(usize),
    Bool,
    Integer,
    TextList,
    Configuration,
    Object(&'static [Node]),
}
impl Shape {
    fn check(&self, value: &Value) -> Result<Value, CraftError> {
        let valid = match self {
            Self::Text(max) => value
                .as_str()
                .is_some_and(|text| text.len() <= *max && !text.contains('\0')),
            Self::Bool => value.is_boolean(),
            Self::Integer => value.as_i64().is_some_and(|number| number >= 0),
            Self::TextList => value.as_array().is_some_and(|values| {
                values.len() <= 100
                    && values.iter().all(|value| {
                        value
                            .as_str()
                            .is_some_and(|text| text.len() <= 1024 && !text.contains('\0'))
                    })
                    && values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == values.len()
            }),
            Self::Configuration => {
                value.as_object().is_some_and(|map| map.len() <= 64)
                    && serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= 32_768)
            }
            Self::Object(_) => false,
        };
        if !valid {
            return Err(malformed());
        }
        Ok(value.clone())
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
const SCOPE_PROJECT: &[Node] = &[
    Node("id", "id", Shape::Text(512)),
    Node("key", "key", Shape::Text(512)),
    Node("name", "name", Shape::Text(4096)),
];
const SCOPE: &[Node] = &[
    Node("type", "type", Shape::Text(128)),
    Node("project", "project", Shape::Object(SCOPE_PROJECT)),
];
const LAST_USED: &[Node] = &[
    Node("type", "type", Shape::Text(128)),
    Node("value", "value", Shape::Text(128)),
];
const LIST_NODES: &[Node] = &[
    Node("name", "name", Shape::Text(4096)),
    Node("key", "key", Shape::Text(512)),
    Node("custom", "custom", Shape::Bool),
    Node("navigable", "navigable", Shape::Bool),
    Node("orderable", "orderable", Shape::Bool),
    Node("searchable", "searchable", Shape::Bool),
    Node("clause_names", "clauseNames", Shape::TextList),
    Node("schema", "schema", Shape::Object(SCHEMA)),
    Node("scope", "scope", Shape::Object(SCOPE)),
];
const SEARCH_NODES: &[Node] = &[
    Node("name", "name", Shape::Text(4096)),
    Node("key", "key", Shape::Text(512)),
    Node("schema", "schema", Shape::Object(SCHEMA)),
    Node("description", "description", Shape::Text(65_536)),
    Node("stable_id", "stableId", Shape::Text(512)),
    Node("is_locked", "isLocked", Shape::Bool),
    Node("is_unscreenable", "isUnscreenable", Shape::Bool),
    Node("searcher_key", "searcherKey", Shape::Text(4096)),
    Node("screens_count", "screensCount", Shape::Integer),
    Node("contexts_count", "contextsCount", Shape::Integer),
    Node("projects_count", "projectsCount", Shape::Integer),
    Node("last_used", "lastUsed", Shape::Object(LAST_USED)),
    Node("translated_name", "translatedName", Shape::Text(4096)),
    Node(
        "translated_description",
        "translatedDescription",
        Shape::Text(65_536),
    ),
    Node("type_display_name", "typeDisplayName", Shape::Text(4096)),
];
