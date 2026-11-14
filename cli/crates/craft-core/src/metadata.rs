use std::collections::BTreeMap;

use serde_json::Value;

use crate::contracts::ResourceRef;
use crate::error::{CraftError, ErrorCode, FieldIssue};
use crate::input::pointer_child;
use crate::registry::validate_local_references;

struct DeclaredField {
    schema: Value,
    validator: jsonschema::Validator,
}

/// Binds native field schemas to the exact project/type context that supplied them.
pub struct FieldMapContract {
    context: ResourceRef,
    fields: BTreeMap<String, DeclaredField>,
}

impl FieldMapContract {
    pub fn new(context: ResourceRef, schemas: BTreeMap<String, Value>) -> Result<Self, CraftError> {
        context.validate()?;
        let mut fields = BTreeMap::new();
        for (name, schema) in schemas {
            crate::contracts::validate_identity("/field_metadata/id", &name)?;
            validate_local_references(&schema)?;
            let validator = jsonschema::validator_for(&schema).map_err(|_| {
                CraftError::new(ErrorCode::Internal, "Invalid native field metadata schema.")
            })?;
            fields.insert(name, DeclaredField { schema, validator });
        }
        Ok(Self { context, fields })
    }

    pub fn schema(&self, name: &str) -> Option<&Value> {
        self.fields.get(name).map(|field| &field.schema)
    }

    pub fn validate(&self, context: &ResourceRef, input: &Value) -> Result<(), CraftError> {
        if context != &self.context {
            return Err(CraftError::field(
                "/custom_fields",
                "metadata_context_mismatch",
            ));
        }
        let values = input
            .as_object()
            .ok_or_else(|| CraftError::field("/custom_fields", "expected_object"))?;
        let mut issues = Vec::new();
        for (name, value) in values {
            let path = pointer_child("/custom_fields", name);
            match self.fields.get(name) {
                None => issues.push(FieldIssue::new(path, "field_not_in_metadata")),
                Some(field) => {
                    for error in field.validator.iter_errors(value).take(5 - issues.len()) {
                        issues.push(FieldIssue::new(
                            format!("{path}{}", error.instance_path()),
                            error.schema_path().to_string(),
                        ));
                    }
                }
            }
            if issues.len() >= 5 {
                break;
            }
        }
        if issues.is_empty() {
            return Ok(());
        }
        let mut error = CraftError::new(
            ErrorCode::InputInvalid,
            "Custom fields do not match target metadata.",
        );
        error.details.issues = issues;
        error.next_action = "Inspect field metadata for the exact target context.".into();
        Err(error)
    }
}
