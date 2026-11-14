use super::*;
use craft_core::metadata::FieldMapContract;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeField {
    required: bool,
    operations: Vec<String>,
    schema: NativeSchema,
    allowed_values: Option<Vec<Value>>,
    has_default_value: Option<bool>,
    default_value: Option<Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeSchema {
    #[serde(rename = "type")]
    kind: String,
    items: Option<String>,
    system: Option<String>,
    custom: Option<String>,
    custom_id: Option<u64>,
}

pub(super) struct QualifiedField {
    required: bool,
    operations: BTreeSet<String>,
    contract: FieldMapContract,
    target: ResourceRef,
    adf: bool,
    default: Option<Value>,
    id: String,
}

impl QualifiedField {
    pub fn compile(target: &ResourceRef, id: &str, raw: &Value) -> Result<Self, CraftError> {
        let field: NativeField = serde_json::from_value(raw.clone()).map_err(|_| malformed())?;
        let operations: BTreeSet<_> = field.operations.iter().cloned().collect();
        if field.operations.len() > 16
            || operations.len() != field.operations.len()
            || operations
                .iter()
                .any(|v| v.is_empty() || v.len() > 64 || v.chars().any(char::is_control))
            || field.allowed_values.as_ref().is_some_and(|values| {
                values.len() > 1000 || serde_json::to_vec(values).map_or(true, |v| v.len() > 65_536)
            })
        {
            return Err(malformed());
        }
        let (mut schema, adf) =
            shape(id, &field).ok_or_else(|| invalid("/changes", "unqualified_field_shape"))?;
        if !field.required && id != "summary" {
            schema = json!({"anyOf":[schema,{"type":"null"}]});
        }
        let contract =
            FieldMapContract::new(target.clone(), BTreeMap::from([(id.into(), schema)]))?;
        let default = field
            .has_default_value
            .filter(|v| *v)
            .and(field.default_value);
        Ok(Self {
            required: field.required,
            operations,
            contract,
            target: target.clone(),
            adf,
            default,
            id: id.into(),
        })
    }
    pub fn supports(&self, operation: &str) -> Result<(), CraftError> {
        if !self.operations.contains(operation) {
            return Err(invalid("/changes", "field_operation_not_in_metadata"));
        }
        Ok(())
    }
    fn check(&self, value: &Value) -> Result<(), CraftError> {
        if self.required && empty(value) {
            return Err(invalid("/changes", "required_field_cannot_be_empty"));
        }
        self.validate_contract(value)
    }

    fn validate_contract(&self, value: &Value) -> Result<(), CraftError> {
        self.contract
            .validate(&self.target, &json!({&self.id:value}))
            .map_err(|mut error| {
                let native = format!("/custom_fields/{}", self.id);
                let input = match self.id.as_str() {
                    "priority" => "/changes/priority_id".into(),
                    "resolution" => "/changes/resolution_id".into(),
                    id if id.starts_with("customfield_") => format!("/changes/custom_fields/{id}"),
                    id => format!("/changes/{id}"),
                };
                for issue in &mut error.details.issues {
                    if let Some(suffix) = issue.path.strip_prefix(&native) {
                        issue.path = format!("{input}{suffix}");
                    }
                }
                error
            })
    }
    pub fn encode(&self, value: &Value) -> Result<Value, CraftError> {
        self.supports("set")?;
        self.check(value)?;
        if self.adf && !value.is_null() {
            let text = value
                .as_str()
                .ok_or_else(|| invalid("/changes", "expected_text"))?;
            let content = if text.is_empty() {
                vec![]
            } else {
                vec![json!({"type":"text","text":text})]
            };
            return Ok(
                json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":content}]}),
            );
        }
        Ok(value.clone())
    }
    pub fn existing(&self, value: &Value) -> bool {
        let input = if self.adf && !value.is_null() {
            let Some(text) = adf_text(value) else {
                return false;
            };
            json!(text)
        } else if value.is_object() && value.get("id").is_some() {
            json!({"id":value["id"]})
        } else if let Some(values) = value.as_array() {
            Value::Array(
                values
                    .iter()
                    .map(|v| {
                        if v.is_object() && v.get("id").is_some() {
                            json!({"id":v["id"]})
                        } else {
                            v.clone()
                        }
                    })
                    .collect(),
            )
        } else {
            value.clone()
        };
        !empty(&input) && self.check(&input).is_ok()
    }
    pub fn has_qualified_default(&self) -> bool {
        self.default
            .as_ref()
            .is_some_and(|value| self.existing(value))
    }
    pub fn current_labels(&self, value: &Value) -> Result<BTreeSet<String>, CraftError> {
        self.validate_contract(value)?;
        serde_json::from_value(value.clone()).map_err(|_| malformed())
    }
}

pub(super) fn empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        _ => false,
    }
}

fn shape(id: &str, field: &NativeField) -> Option<(Value, bool)> {
    let schema = &field.schema;
    let bounded_text = || json!({"type":"string","maxLength":32768,"pattern":"^[^\\u0000]*$"});
    let labels = || json!({"type":"array","maxItems":100,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":255,"pattern":"^\\S+$"}});
    if schema.custom.is_none() && schema.system.as_deref() == Some(id) && schema.custom_id.is_none()
    {
        let nonempty_options = field
            .allowed_values
            .as_ref()
            .is_some_and(|values| !values.is_empty());
        return match (id, schema.kind.as_str(), schema.items.as_deref()) {
            ("summary", "string", None) if !nonempty_options => Some((
                json!({"type":"string","minLength":1,"maxLength":255}),
                false,
            )),
            ("description" | "environment", "string", None) if !nonempty_options => {
                Some((bounded_text(), true))
            }
            ("labels", "array", Some("string")) if !nonempty_options => Some((labels(), false)),
            ("priority", "priority", None) | ("resolution", "resolution", None) => {
                options(field).map(|values| (json!({"enum":values}), false))
            }
            _ => None,
        };
    }
    let suffix = id.strip_prefix("customfield_")?;
    if !numeric_id(suffix)
        || schema.system.is_some()
        || schema.custom_id.is_some_and(|id| id.to_string() != suffix)
    {
        return None;
    }
    let family = schema
        .custom
        .as_deref()?
        .strip_prefix("com.atlassian.jira.plugin.system.customfieldtypes:")?;
    let nonempty_options = field
        .allowed_values
        .as_ref()
        .is_some_and(|values| !values.is_empty());
    match (family,schema.kind.as_str(),schema.items.as_deref()) {
        ("textfield","string",None) if !nonempty_options => Some((bounded_text(),false)),
        ("textarea","string",None) if !nonempty_options => Some((bounded_text(),true)),
        ("float","number",None) if !nonempty_options => Some((json!({"type":"number"}),false)),
        ("select","option",None) => options(field).map(|values| (json!({"enum":values}),false)),
        ("multiselect"|"multicheckboxes","array",Some("option")) => options(field).map(|values| (json!({"type":"array","maxItems":1000,"uniqueItems":true,"items":{"enum":values}}),false)),
        _ => None,
    }
}
fn options(field: &NativeField) -> Option<Vec<Value>> {
    let values = field
        .allowed_values
        .as_ref()
        .filter(|values| !values.is_empty())?;
    let mut seen = BTreeSet::new();
    values
        .iter()
        .map(|value| {
            let id = value.get("id")?.as_str()?;
            (numeric_id(id) && seen.insert(id)).then(|| json!({"id":id}))
        })
        .collect()
}

fn adf_text(value: &Value) -> Option<String> {
    if value.get("type")?.as_str()? != "doc" || value.get("version")?.as_u64()? != 1 {
        return None;
    }
    fn visit(value: &Value, depth: usize, nodes: &mut usize, text: &mut String) -> Option<()> {
        *nodes += 1;
        if depth > 32 || *nodes > 1000 || text.len() > 32768 {
            return None;
        }
        match value.get("type")?.as_str()? {
            "text" => text.push_str(value.get("text")?.as_str()?),
            "hardBreak" => text.push('\n'),
            "doc" | "paragraph" | "heading" | "bulletList" | "orderedList" | "listItem"
            | "blockquote" | "codeBlock" => {
                for child in value.get("content")?.as_array()? {
                    visit(child, depth + 1, nodes, text)?;
                }
            }
            _ => return None,
        }
        (text.len() <= 32768).then_some(())
    }
    let mut text = String::new();
    visit(value, 0, &mut 0, &mut text)?;
    Some(text)
}
