use super::{GET, ProjectGetData, ProjectStatusesData, STATUSES, malformed};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::PreparedOperation;
use craft_core::presence::InputField;
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::response_schema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectDetails {
    pub id: String,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub key: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub project_type: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub style: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub simplified: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub archived: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub deleted: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub description: InputField<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields_not_returned: Vec<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusCategory {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub id: InputField<i64>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub key: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusSummary {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub description: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub category: InputField<StatusCategory>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusGroup {
    pub id: String,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub subtask: InputField<bool>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub statuses: InputField<Vec<StatusSummary>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields_not_returned: Vec<String>,
}
pub fn projection(command: &str) -> ProjectionContract {
    ProjectionContract {
        item_pointer: if command == GET { "/project" } else { "/items" }.into(),
        always_returned: vec!["id".into(), "fields_not_returned".into()],
        default_selected: if command == GET {
            vec!["id", "key", "name", "project_type"]
        } else {
            vec!["id", "name", "subtask", "statuses.id", "statuses.name"]
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
    /// Compiles closed field paths locally; unknown commands/paths and duplicates fail without I/O.
    pub fn compile(command: &str, fields: &[String]) -> Result<Self, CraftError> {
        if !matches!(command, GET | STATUSES) {
            return Err(CraftError::field(
                "/command",
                "expected_project_context_read",
            ));
        }
        let schema = if command == GET {
            response_schema::<ProjectGetData>()?
        } else {
            response_schema::<ProjectStatusesData>()?
        };
        let contract = projection(command);
        contract.compile(&schema, fields)?;
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
                "Project context field plan differs from the prepared read.",
            ));
        }
        Ok(())
    }
    fn selects(&self, path: &str) -> bool {
        self.selected.iter().any(|chosen| {
            chosen == path
                || path.starts_with(&format!("{chosen}."))
                || chosen.starts_with(&format!("{path}."))
        })
    }
    fn value(
        &self,
        out: &mut Value,
        raw: &Value,
        path: &str,
        source: &str,
        output: &str,
        kind: Kind,
    ) -> Result<(), CraftError> {
        if !self.selects(path) {
            return Ok(());
        }
        if let Some(value) = raw.get(source) {
            out[output] = if value.is_null() {
                Value::Null
            } else {
                kind.check(value)?
            };
        }
        Ok(())
    }
    fn missing(&self, out: &mut Value) {
        let missing: Vec<_> = self
            .selected
            .iter()
            .filter(|path| {
                path.as_str() != "fields_not_returned"
                    && !present(out, &path.split('.').collect::<Vec<_>>())
            })
            .cloned()
            .collect();
        if !missing.is_empty() {
            out["fields_not_returned"] = json!(missing);
        }
    }
    pub(super) fn normalize_project(&self, raw: &Value) -> Result<Value, CraftError> {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| super::numeric(id))
            .ok_or_else(malformed)?;
        let mut out = json!({"id":id});
        for (source, name, kind) in [
            ("key", "key", Kind::Text(512)),
            ("name", "name", Kind::Text(4096)),
            ("projectTypeKey", "project_type", Kind::Text(128)),
            ("style", "style", Kind::Text(128)),
            ("simplified", "simplified", Kind::Bool),
            ("archived", "archived", Kind::Bool),
            ("deleted", "deleted", Kind::Bool),
            ("description", "description", Kind::Text(65_536)),
        ] {
            self.value(&mut out, raw, name, source, name, kind)?;
        }
        self.missing(&mut out);
        serde_json::from_value::<ProjectDetails>(out.clone()).map_err(|_| malformed())?;
        Ok(out)
    }
    fn normalize_status(&self, raw: &Value) -> Result<Value, CraftError> {
        if !raw.is_object() {
            return Err(malformed());
        }
        let mut out = json!({});
        for (name, kind) in [
            ("id", Kind::Text(512)),
            ("name", Kind::Text(4096)),
            ("description", Kind::Text(16_384)),
        ] {
            self.value(&mut out, raw, &format!("statuses.{name}"), name, name, kind)?;
        }
        if self.selects("statuses.category")
            && let Some(category) = raw.get("statusCategory")
        {
            if category.is_null() {
                out["category"] = Value::Null;
            } else {
                if !category.is_object() {
                    return Err(malformed());
                }
                let mut value = json!({});
                for (name, kind) in [
                    ("id", Kind::Integer),
                    ("key", Kind::Text(512)),
                    ("name", Kind::Text(4096)),
                ] {
                    self.value(
                        &mut value,
                        category,
                        &format!("statuses.category.{name}"),
                        name,
                        name,
                        kind,
                    )?;
                }
                out["category"] = value;
            }
        }
        Ok(out)
    }
    pub(super) fn normalize_group(&self, raw: &Value) -> Result<Value, CraftError> {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| super::numeric(id))
            .ok_or_else(malformed)?;
        let mut out = json!({"id":id});
        self.value(&mut out, raw, "name", "name", "name", Kind::Text(4096))?;
        self.value(&mut out, raw, "subtask", "subtask", "subtask", Kind::Bool)?;
        if self.selects("statuses")
            && let Some(statuses) = raw.get("statuses")
        {
            if statuses.is_null() {
                out["statuses"] = Value::Null;
            } else {
                let statuses = statuses
                    .as_array()
                    .filter(|values| values.len() <= 1000)
                    .ok_or_else(malformed)?;
                let mut ids = BTreeSet::new();
                let mut values = Vec::new();
                for status in statuses {
                    let id = status
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| super::numeric(id))
                        .ok_or_else(malformed)?;
                    if !ids.insert(id) {
                        return Err(malformed());
                    }
                    values.push(self.normalize_status(status)?);
                }
                out["statuses"] = json!(values);
            }
        }
        self.missing(&mut out);
        serde_json::from_value::<StatusGroup>(out.clone()).map_err(|_| malformed())?;
        Ok(out)
    }
}
enum Kind {
    Text(usize),
    Bool,
    Integer,
}
impl Kind {
    fn check(&self, value: &Value) -> Result<Value, CraftError> {
        let valid = match self {
            Self::Text(max) => value
                .as_str()
                .is_some_and(|text| text.len() <= *max && !text.contains('\0')),
            Self::Bool => value.is_boolean(),
            Self::Integer => value.as_i64().is_some(),
        };
        if !valid {
            return Err(malformed());
        }
        Ok(value.clone())
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
