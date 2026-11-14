use super::{malformed, response};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{OperationContext, PreparedOperation};
use craft_core::projection::{ProjectionContract, ProjectionMode};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn projection() -> ProjectionContract {
    ProjectionContract {
        item_pointer: "/items".into(),
        always_returned: vec![
            "key",
            "returned",
            "have_permission",
            "kind",
            "fields_not_returned",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        default_selected: vec!["key", "returned", "have_permission", "kind", "name"]
            .into_iter()
            .map(String::from)
            .collect(),
        mode: ProjectionMode::Local,
    }
}
#[derive(PartialEq, Eq)]
pub struct FieldPlan {
    selected: BTreeSet<String>,
}
impl FieldPlan {
    pub fn compile(fields: &[String]) -> Result<Self, CraftError> {
        let contract = projection();
        contract.compile(&response()?, fields)?;
        let selected = if fields.is_empty() {
            contract.default_selected
        } else {
            fields.to_vec()
        };
        Ok(Self {
            selected: selected
                .into_iter()
                .chain(contract.always_returned)
                .collect(),
        })
    }
    pub(super) fn verify(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        if *self != Self::compile(&operation.request().selection.fields)? {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Permission projection differs from the prepared operation.",
            ));
        }
        Ok(())
    }
    pub(super) fn normalize(
        &self,
        raw: &Value,
        requested: &[String],
        control: &dyn OperationContext,
    ) -> Result<Vec<Value>, CraftError> {
        let entries = raw
            .as_object()
            .filter(|entries| entries.len() <= 32)
            .ok_or_else(malformed)?;
        if entries.keys().any(|key| !requested.contains(key)) {
            return Err(malformed());
        }
        let mut items = Vec::new();
        for key in requested {
            control.checkpoint()?;
            let native = entries.get(key);
            if native.is_some_and(|value| !value.is_object()) {
                return Err(malformed());
            }
            if let Some(value) = native.and_then(|value| value.get("key"))
                && value.as_str() != Some(key.as_str())
            {
                return Err(malformed());
            }
            let decision = native.and_then(|value| value.get("havePermission"));
            if decision.is_some_and(|value| !value.is_boolean()) {
                return Err(malformed());
            }
            let mut item = json!({"key":key,"returned":native.is_some(),"have_permission":decision.and_then(Value::as_bool)});
            for (local, source, max) in [
                ("id", "id", 512),
                ("name", "name", 4096),
                ("description", "description", 16_384),
                ("kind", "type", 0),
                ("deprecated_key", "deprecatedKey", 0),
            ] {
                if !self.selected.contains(local) {
                    continue;
                }
                if let Some(value) = native.and_then(|value| value.get(source)) {
                    let valid = value.is_null()
                        || match local {
                            "kind" => matches!(value.as_str(), Some("GLOBAL" | "PROJECT")),
                            "deprecated_key" => value.is_boolean(),
                            _ => value
                                .as_str()
                                .is_some_and(|text| text.len() <= max && !text.contains('\0')),
                        };
                    if !valid {
                        return Err(malformed());
                    }
                    item[local] = value.clone();
                }
            }
            let mut missing: Vec<_> = self
                .selected
                .iter()
                .filter(|name| {
                    name.as_str() != "fields_not_returned" && item.get(name.as_str()).is_none()
                })
                .cloned()
                .collect();
            if decision.is_none() {
                missing.push("have_permission".into());
            }
            missing.sort();
            missing.dedup();
            if !missing.is_empty() {
                item["fields_not_returned"] = json!(missing);
            }
            items.push(item);
        }
        Ok(items)
    }
}
