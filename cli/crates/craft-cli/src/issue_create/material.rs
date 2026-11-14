//! Preview and send qualification fingerprint the same selected native field material.
use std::collections::BTreeMap;

use craft_core::error::CraftError;
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_provider_jira::create_metadata::CreateContext;
use serde_json::{Value, json};

fn allowed_values(value: &Value) -> Result<Value, CraftError> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let rows = value
        .as_array()
        .filter(|rows| rows.len() <= 1000)
        .ok_or_else(|| {
            CraftError::field("/metadata/allowedValues", "expected_bounded_native_choices")
        })?;
    let mut selected = Vec::new();
    for row in rows {
        let source = row.as_object().ok_or_else(|| {
            CraftError::field("/metadata/allowedValues", "expected_native_choice_object")
        })?;
        let mut choice = serde_json::Map::new();
        for key in [
            "id",
            "value",
            "name",
            "key",
            "accountId",
            "displayName",
            "active",
            "disabled",
            "subtask",
        ] {
            if let Some(value) = source.get(key) {
                choice.insert(key.into(), value.clone());
            }
        }
        selected.push(Value::Object(choice));
    }
    selected.sort_by_cached_key(|row| row.to_string());
    Ok(Value::Array(selected))
}

pub(crate) fn observe_fields(
    call: &NativeCall,
    response: &NativeResponse,
    details: &mut BTreeMap<String, Value>,
) -> Result<(), CraftError> {
    if call.route != "metadata-fields" || response.status != 200 {
        return Ok(());
    }
    let fields = response
        .body
        .get("fields")
        .or_else(|| response.body.get("results"))
        .and_then(Value::as_array)
        .ok_or_else(|| CraftError::field("/metadata", "unreadable_native_fields"))?;
    for field in fields {
        let id = field["fieldId"]
            .as_str()
            .ok_or_else(|| CraftError::field("/metadata", "unreadable_native_field_identity"))?;
        let mut detail = json!({
            "default_value":field.get("defaultValue"),
            "configuration":field.get("configuration"),
            "native_schema":field.get("schema")
        });
        if let Some(choices) = field.get("allowedValues") {
            detail["allowed_values"] = allowed_values(choices)?;
        }
        if details.len() >= 2000 || details.insert(id.into(), detail).is_some() {
            return Err(CraftError::field(
                "/metadata",
                "duplicate_or_excess_native_field_identity",
            ));
        }
    }
    Ok(())
}

pub(crate) fn snapshot(context: &CreateContext, details: &BTreeMap<String, Value>) -> Value {
    let summary = context.summary();
    let mut fields = summary.fields.clone();
    fields.sort_by(|a, b| a.id.cmp(&b.id));
    json!({"target":summary.target,"project_key":summary.project_key,
        "issue_type_id":summary.issue_type_id,"issue_type_name":summary.issue_type_name,
        "subtask":summary.subtask,"parent":summary.parent,"fields":fields,"field_details":details})
}

pub(crate) struct MetadataGuard<'a> {
    pub native: &'a mut dyn OperationContext,
    pub check: &'a dyn Fn() -> Result<(), CraftError>,
    pub details: &'a mut BTreeMap<String, Value>,
}
impl OperationContext for MetadataGuard<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        (self.check)()?;
        self.native.checkpoint()
    }
    fn verify_operation(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        self.checkpoint()?;
        self.native.verify_operation(operation)
    }
    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        self.checkpoint()?;
        self.native.normalize_next(call, link)
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.checkpoint()?;
        let response = self.native.invoke(call)?;
        self.checkpoint()?;
        observe_fields(call, &response, self.details)?;
        Ok(response)
    }
}
