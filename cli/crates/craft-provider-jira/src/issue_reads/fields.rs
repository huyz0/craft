use super::{GET, IssueGetData, IssueSearchData, SEARCH, malformed};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::PreparedOperation;
use craft_core::presence::InputField;
use craft_core::projection::{ProjectionContract, ProjectionMode};
use craft_core::registry::response_schema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct AdfDocument {
    #[serde(rename = "type")]
    #[schemars(extend("const"="doc"))]
    pub kind: String,
    #[schemars(extend("const"=1))]
    pub version: u8,
    pub content: Vec<Value>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum IssueDescription {
    Adf(AdfDocument),
    Text(String),
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NamedRef {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectRef {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub key: InputField<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueTypeRef {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub name: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub subtask: InputField<bool>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssigneeRef {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub account_id: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub display_name: InputField<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueSummary {
    pub id: String,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub key: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub summary: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub project: InputField<ProjectRef>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub status: InputField<NamedRef>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub issue_type: InputField<IssueTypeRef>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub assignee: InputField<AssigneeRef>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub updated: InputField<String>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub labels: InputField<Vec<String>>,
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    pub description: InputField<IssueDescription>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields_not_returned: Vec<String>,
}

pub fn projection(command: &str) -> ProjectionContract {
    ProjectionContract {
        item_pointer: if command == GET { "/issue" } else { "/items" }.into(),
        always_returned: ["id", "fields_not_returned"].map(String::from).into(),
        default_selected: [
            "id",
            "key",
            "summary",
            "project.id",
            "project.key",
            "status.id",
            "status.name",
        ]
        .map(String::from)
        .into(),
        mode: ProjectionMode::Mixed,
    }
}

/// Compiled field paths are provider values, not a raw field-mask or expansion escape hatch.
#[derive(PartialEq, Eq)]
pub struct FieldPlan {
    command: String,
    selected: BTreeSet<String>,
    roots: BTreeSet<String>,
    native: Vec<String>,
}
impl FieldPlan {
    /// Compiles closed normalized field paths for GET or SEARCH without any I/O.
    /// Unknown commands, duplicate/unknown paths and variant-specific rich paths are refused.
    pub fn compile(command: &str, fields: &[String]) -> Result<Self, CraftError> {
        if !matches!(command, GET | SEARCH) {
            return Err(CraftError::field("/command", "expected_issue_read"));
        }
        let schema = if command == GET {
            response_schema::<IssueGetData>()?
        } else {
            response_schema::<IssueSearchData>()?
        };
        let contract = projection(command);
        contract.compile(&schema, fields)?;
        let selected: BTreeSet<_> = if fields.is_empty() {
            contract.default_selected
        } else {
            fields.to_vec()
        }
        .into_iter()
        .chain(contract.always_returned)
        .collect();
        let roots: BTreeSet<_> = selected
            .iter()
            .filter_map(|path| path.split('.').next().map(String::from))
            .collect();
        let mut native: BTreeSet<String> = roots
            .iter()
            .filter(|name| name.as_str() != "fields_not_returned")
            .map(|name| {
                if name == "issue_type" {
                    "issuetype".into()
                } else {
                    name.clone()
                }
            })
            .collect();
        if command == GET {
            native.insert("key".into());
        }
        Ok(Self {
            command: command.into(),
            selected,
            roots,
            native: native.into_iter().collect(),
        })
    }
    pub fn native_fields(&self) -> &[String] {
        &self.native
    }
    pub(super) fn verify(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        let expected = Self::compile(
            &operation.request().command,
            &operation.request().selection.fields,
        )?;
        if *self != expected {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Issue field plan differs from the prepared read.",
            ));
        }
        Ok(())
    }
    fn selected_member(&self, root: &str, member: &str) -> bool {
        self.selected.contains(root) || self.selected.contains(&format!("{root}.{member}"))
    }
    fn reference(
        &self,
        root: &str,
        value: &Value,
        fields: &[(&str, &str, usize)],
    ) -> Result<Value, CraftError> {
        if !value.is_object() {
            return Err(malformed());
        }
        let mut out = json!({});
        for (source, name, limit) in fields {
            if !self.selected_member(root, name) {
                continue;
            }
            let Some(value) = value.get(*source) else {
                continue;
            };
            out[*name] = if value.is_null() {
                Value::Null
            } else {
                text(value, *limit)?
            };
        }
        Ok(out)
    }
    /// Retains selected metadata and required issue identity, preserving omitted/null values.
    /// Malformed selected data fails; unrelated values and returned links supply no authority.
    pub fn normalize(&self, raw: &Value) -> Result<Value, CraftError> {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| super::numeric_id(id))
            .ok_or_else(malformed)?;
        let mut output = json!({"id":id});
        let needs_fields = self
            .roots
            .iter()
            .any(|root| !matches!(root.as_str(), "id" | "key" | "fields_not_returned"));
        let fields = match raw.get("fields").filter(|_| needs_fields) {
            None => None,
            Some(Value::Object(map)) => Some(map),
            _ => return Err(malformed()),
        };
        for root in &self.roots {
            if matches!(root.as_str(), "id" | "fields_not_returned") {
                continue;
            }
            let source = if root == "key" {
                raw.get("key")
            } else {
                fields.and_then(|fields| {
                    fields.get(if root == "issue_type" {
                        "issuetype"
                    } else {
                        root
                    })
                })
            };
            let Some(value) = source else { continue };
            let normalized = if value.is_null() {
                Value::Null
            } else {
                match root.as_str() {
                    "key" => json!(super::key(value.as_str().ok_or_else(malformed)?)?),
                    "summary" => text(value, 16_384)?,
                    "updated" => text(value, 128)?,
                    "project" => {
                        self.reference(root, value, &[("id", "id", 512), ("key", "key", 512)])?
                    }
                    "status" => {
                        self.reference(root, value, &[("id", "id", 512), ("name", "name", 4096)])?
                    }
                    "issue_type" => {
                        let mut out = self.reference(
                            root,
                            value,
                            &[("id", "id", 512), ("name", "name", 4096)],
                        )?;
                        if let Some(flag) = value
                            .get("subtask")
                            .filter(|_| self.selected_member(root, "subtask"))
                        {
                            if !flag.is_null() && !flag.is_boolean() {
                                return Err(malformed());
                            }
                            out["subtask"] = flag.clone();
                        }
                        out
                    }
                    "assignee" => self.reference(
                        root,
                        value,
                        &[
                            ("accountId", "account_id", 512),
                            ("displayName", "display_name", 4096),
                        ],
                    )?,
                    "labels" => {
                        let values = value
                            .as_array()
                            .filter(|values| values.len() <= 1000)
                            .ok_or_else(malformed)?;
                        for label in values {
                            text(label, 1024)?;
                        }
                        value.clone()
                    }
                    "description" => {
                        if value.is_string() {
                            text(value, 262_144)?;
                        } else if value.get("type").and_then(Value::as_str) != Some("doc")
                            || value.get("version").and_then(Value::as_u64) != Some(1)
                            || value.get("content").and_then(Value::as_array).is_none()
                        {
                            return Err(malformed());
                        }
                        if serde_json::to_vec(value).map_err(|_| malformed())?.len() > 262_144 {
                            return Err(malformed());
                        }
                        value.clone()
                    }
                    _ => {
                        return Err(CraftError::new(
                            ErrorCode::Internal,
                            "Unmapped issue field projection.",
                        ));
                    }
                }
            };
            output[root] = normalized;
        }
        let missing: Vec<_> = self
            .selected
            .iter()
            .filter(|path| path.as_str() != "fields_not_returned" && !present(&output, path))
            .cloned()
            .collect();
        if !missing.is_empty() {
            output["fields_not_returned"] = json!(missing);
        }
        serde_json::from_value::<IssueSummary>(output.clone()).map_err(|_| malformed())?;
        Ok(output)
    }
}
fn text(value: &Value, maximum: usize) -> Result<Value, CraftError> {
    value
        .as_str()
        .filter(|value| value.len() <= maximum && !value.contains('\0'))
        .ok_or_else(malformed)?;
    Ok(value.clone())
}
fn present(value: &Value, path: &str) -> bool {
    let mut node = value;
    for part in path.split('.') {
        if node.is_null() {
            return true;
        }
        let Some(next) = node.get(part) else {
            return false;
        };
        node = next;
    }
    true
}
