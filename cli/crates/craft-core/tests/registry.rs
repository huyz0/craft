use craft_core::contracts::{EffectClass, Envelope};
use craft_core::error::ErrorCode;
use craft_core::registry::{
    OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema,
};
use serde_json::{Value, json};

fn descriptor() -> Result<OperationDescriptor, Box<dyn std::error::Error>> {
    Ok(OperationDescriptor {
        id: "sample.read".into(),
        provider: "sample".into(),
        contract_version: craft_core::contracts::SCHEMA_VERSION.into(),
        api_revision: "native".into(),
        description: "Read an identified sample.".into(),
        effect: EffectClass::Read,
        input_schema: json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}),
        response_schema: response_schema::<String>()?,
        example: json!({"id":"record"}),
        response_example: serde_json::to_value(Envelope::success(
            "sample.read",
            "example-request".into(),
            json!("value"),
        ))?,
        projection: None,
        scope_alternatives: Vec::new(),
        permissions: Vec::new(),
        tenant_conditions: Vec::new(),
        routes: Vec::new(),
        retry_class: RetryClass::SafeRead,
        reconciliation: ReconciliationClass::NotApplicable,
    })
}

#[test]
fn rejects_unknown_fields_without_echoing_values() -> Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    registry.register(descriptor()?)?;
    let error = registry
        .validate(
            "sample.read",
            &json!({"id":"record","secret":"sensitive-value"}),
        )
        .err()
        .ok_or("Expected a refusal")?;
    assert_eq!(error.code, ErrorCode::InputInvalid);
    assert!(!serde_json::to_string(&error)?.contains("sensitive-value"));
    Ok(())
}

#[test]
fn refuses_external_schema_references_and_duplicate_commands()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    registry.register(descriptor()?)?;
    assert!(registry.register(descriptor()?).is_err());
    let mut external = descriptor()?;
    external.id = "sample.external".into();
    external.input_schema = json!({"$ref":"https://untrusted.invalid/schema"});
    assert!(registry.register(external).is_err());
    Ok(())
}

#[test]
fn validates_command_data_and_success_failure_consistency() -> Result<(), Box<dyn std::error::Error>>
{
    let validator = jsonschema::validator_for(&response_schema::<String>()?)?;
    let success = serde_json::to_value(Envelope::success(
        "sample.read",
        "request".into(),
        json!("value"),
    ))?;
    assert!(validator.is_valid(&success));
    let mut invalid = success.clone();
    invalid["data"] = json!(42);
    assert!(!validator.is_valid(&invalid));
    invalid = success;
    invalid["ok"] = json!(false);
    assert!(!validator.is_valid(&invalid));
    invalid["error"] = Value::Null;
    assert!(!validator.is_valid(&invalid));
    Ok(())
}
