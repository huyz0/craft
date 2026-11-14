use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NativeField {
    field_id: String,
    key: String,
    name: String,
    operations: Vec<String>,
    required: bool,
    schema: NativeSchema,
    has_default_value: Option<bool>,
    allowed_values: Option<Vec<Value>>,
    #[serde(rename = "autoCompleteUrl")]
    _auto_complete_url: Option<String>,
    #[serde(rename = "configuration")]
    _configuration: Option<Value>,
    #[serde(rename = "defaultValue")]
    _default_value: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeSchema {
    #[serde(rename = "type")]
    kind: String,
    items: Option<String>,
    system: Option<String>,
    custom: Option<String>,
    custom_id: Option<u64>,
    #[serde(rename = "configuration")]
    _configuration: Option<Value>,
}

pub(super) fn parse(
    value: Value,
    project: &str,
    issue_type: &str,
    parent: Option<&str>,
) -> Result<FieldInfo, CraftError> {
    let field: NativeField = serde_json::from_value(value).map_err(|_| malformed())?;
    if field.field_id.is_empty()
        || field.field_id.len() > 128
        || !field
            .field_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || field.key != field.field_id
        || field.name.is_empty()
        || field.name.len() > 512
        || field.operations.len() > 16
        || field.operations.iter().any(|value| value.len() > 64)
        || field
            .allowed_values
            .as_ref()
            .is_some_and(|values| values.len() > 1000)
    {
        return Err(malformed());
    }
    let settable = field.operations.iter().any(|operation| operation == "set");
    let schema = if settable {
        compile(&field, project, issue_type, parent)
    } else {
        None
    };
    Ok(FieldInfo {
        id: field.field_id,
        name: field.name,
        required: field.required,
        has_default: field.has_default_value.unwrap_or(false),
        settable,
        unsupported_reason: schema
            .is_none()
            .then(|| "unqualified_field_shape_or_set_operation".into()),
        schema,
    })
}

fn compile(
    field: &NativeField,
    project: &str,
    issue_type: &str,
    parent: Option<&str>,
) -> Option<Value> {
    let native = &field.schema;
    if let Some(values) = field
        .allowed_values
        .as_ref()
        .filter(|values| !values.is_empty())
    {
        match field.field_id.as_str() {
            "project" | "issuetype" => {
                let selected = if field.field_id == "project" {
                    project
                } else {
                    issue_type
                };
                if !values
                    .iter()
                    .any(|value| value.get("id").and_then(Value::as_str) == Some(selected))
                {
                    return None;
                }
            }
            "summary" | "description" | "labels" => return None,
            "assignee" => {}
            "parent" => {
                let id = parent?;
                if !values
                    .iter()
                    .any(|value| value.get("id").and_then(Value::as_str) == Some(id))
                {
                    return None;
                }
            }
            _ if !matches!(native.kind.as_str(), "option" | "array") => return None,
            _ => {}
        }
    }
    if native.custom.is_none() && native.system.as_deref() == Some(field.field_id.as_str()) {
        return match (
            field.field_id.as_str(),
            native.kind.as_str(),
            native.items.as_deref(),
        ) {
            ("project", "project", None) => Some(
                json!({"type":"object","properties":{"id":{"const":project}},"required":["id"],"additionalProperties":false}),
            ),
            ("issuetype", "issuetype", None) => Some(
                json!({"type":"object","properties":{"id":{"const":issue_type}},"required":["id"],"additionalProperties":false}),
            ),
            ("summary", "string", None) => {
                Some(json!({"type":"string","minLength":1,"maxLength":255}))
            }
            ("parent","issuelink",None)=>parent.map(|id|json!({"type":"object","properties":{"id":{"const":id}},"required":["id"],"additionalProperties":false})),
            ("assignee","user",None)=>account_schema(field),
            ("description", "string", None) => {
                Some(json!({"type":"string","maxLength":32768,"x-craft-encoding":"adf_text"}))
            }
            ("labels", "array", Some("string")) => Some(
                json!({"type":"array","maxItems":100,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":255}}),
            ),
            _ => None,
        };
    }
    let suffix = field.field_id.strip_prefix("customfield_")?;
    if !numeric(suffix)
        || native.system.is_some()
        || native.custom_id.is_some_and(|id| id.to_string() != suffix)
    {
        return None;
    }
    let family = native
        .custom
        .as_deref()?
        .strip_prefix("com.atlassian.jira.plugin.system.customfieldtypes:")?;
    match (family,native.kind.as_str(),native.items.as_deref()) {
        ("textfield","string",None)=>Some(json!({"type":"string","maxLength":32768})),
        ("textarea","string",None)=>Some(json!({"type":"string","maxLength":32768,"x-craft-encoding":"adf_text"})),
        ("float","number",None)=>Some(json!({"type":"number"})),
        ("select","option",None)=>option_values(field).map(|values|json!({"enum":values})),
        ("multiselect"|"multicheckboxes","array",Some("option"))=>option_values(field).map(|values|json!({"type":"array","maxItems":1000,"uniqueItems":true,"items":{"enum":values}})),
        _=>None,
    }
}

fn account_schema(field: &NativeField) -> Option<Value> {
    if let Some(values) = field
        .allowed_values
        .as_ref()
        .filter(|values| !values.is_empty())
    {
        let mut seen = std::collections::BTreeSet::new();
        let values: Option<Vec<_>> = values
            .iter()
            .map(|value| {
                let id = value.get("accountId")?.as_str()?;
                if !accounts::valid_account(id) || !seen.insert(id) {
                    return None;
                }
                Some(json!({"accountId":id}))
            })
            .collect();
        return values.map(|values| json!({"enum":values}));
    }
    Some(
        json!({"type":"object","properties":{"accountId":{"type":"string","minLength":1,"maxLength":128}},"required":["accountId"],"additionalProperties":false}),
    )
}

fn option_values(field: &NativeField) -> Option<Vec<Value>> {
    let values = field.allowed_values.as_ref()?;
    if values.is_empty() {
        return None;
    }
    let mut ids = std::collections::BTreeSet::new();
    values
        .iter()
        .map(|value| {
            let id = value.as_object()?.get("id")?.as_str()?;
            if !numeric(id) || !ids.insert(id) {
                return None;
            }
            Some(json!({"id":id}))
        })
        .collect()
}

pub(super) fn is_textarea(field: &FieldInfo) -> bool {
    field
        .schema
        .as_ref()
        .is_some_and(|schema| schema["x-craft-encoding"] == "adf_text")
}
