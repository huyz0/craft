use std::collections::BTreeMap;

use craft_core::contracts::ResourceRef;
use craft_core::metadata::FieldMapContract;
use serde_json::json;

fn context() -> ResourceRef {
    ResourceRef {
        provider: "sample".into(),
        deployment: "cloud".into(),
        tenant: "tenant-one".into(),
        resource_type: "field_context".into(),
        id: "project-one:issue-kind".into(),
    }
}

#[test]
fn binds_field_types_and_ranges_to_the_exact_metadata_context()
-> Result<(), Box<dyn std::error::Error>> {
    let contract = FieldMapContract::new(
        context(),
        BTreeMap::from([
            (
                "budget".into(),
                json!({"type":"integer","minimum":0,"maximum":1000}),
            ),
            (
                "notes".into(),
                json!({"$defs":{"Text":{"type":"string","maxLength":1000}},"$ref":"#/$defs/Text"}),
            ),
        ]),
    )?;
    let valid = json!({"budget":42,"notes":"مرحبا 🦀\nlet total = 4;"});
    contract.validate(&context(), &valid)?;
    assert_eq!(valid["notes"], "مرحبا 🦀\nlet total = 4;");
    for invalid in [
        json!({"budget":"42"}),
        json!({"budget":1001}),
        json!({"unknown":"private-sentinel"}),
    ] {
        let error = contract
            .validate(&context(), &invalid)
            .err()
            .ok_or("Expected metadata refusal")?;
        assert!(error.details.issues[0].path.starts_with("/custom_fields/"));
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    let mut other = context();
    other.tenant = "tenant-two".into();
    assert!(contract.validate(&other, &valid).is_err());
    other = context();
    other.id = "project-two:issue-kind".into();
    assert!(contract.validate(&other, &valid).is_err());
    assert!(contract.schema("notes").is_some());
    Ok(())
}

#[test]
fn refuses_remote_field_schema_resolution() {
    assert!(
        FieldMapContract::new(
            context(),
            BTreeMap::from([(
                "notes".into(),
                json!({"$ref":"https://untrusted.invalid/schema"})
            ),])
        )
        .is_err()
    );
}
