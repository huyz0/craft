use craft_core::contracts::{
    CanonicalRequest, Envelope, GrantRef, JobState, OperationReceipt, Preconditions, ProviderRef,
    RemoteJob, RequestState, ResourceRef, Selection, TenantRef,
};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::{canonical_digest, parse_object};
use craft_core::presence::InputField;
use craft_core::registry::{response_schema, schema_for};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[test]
fn rejects_duplicate_properties_at_any_depth() {
    assert!(parse_object(br#"{"target":{"id":"a","id":"b"}}"#).is_err());
}

#[test]
fn stored_document_envelopes_do_not_widen_cli_or_response_input_limits()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_core::input::{parse_document_json, parse_json};
    let bytes = serde_json::to_vec(&json!({"body":"x".repeat(16_777_216)}))?;
    assert!(parse_document_json(&bytes, 33_554_432).is_ok());
    assert!(parse_json(&bytes, 16_777_216).is_err());
    assert!(parse_json(b"{}", 33_554_432).is_err());
    assert!(parse_object(&bytes).is_err());
    assert!(parse_document_json(b"{}", 33_554_433).is_err());
    let nested = format!("{}0{}", "[".repeat(80), "]".repeat(80));
    assert!(parse_document_json(nested.as_bytes(), 33_554_432).is_ok());
    assert!(parse_json(nested.as_bytes(), 16_777_216).is_err());
    let too_deep = format!("{}0{}", "[".repeat(97), "]".repeat(97));
    assert!(parse_document_json(too_deep.as_bytes(), 33_554_432).is_err());
    for invalid in [br#"{"a":1,"a":2}"#.as_slice(), b"{} trailing", &[0xff]] {
        assert!(parse_document_json(invalid, 33_554_432).is_err());
    }
    let crowded = serde_json::to_vec(&vec![0; 10_001])?;
    assert!(parse_document_json(&crowded, 33_554_432).is_err());
    Ok(())
}

#[test]
fn reports_exact_escaped_paths_without_echoing_values() -> Result<(), Box<dyn std::error::Error>> {
    let error = parse_object(br#"{"items":[{"a/b~c":"private-sentinel","a/b~c":"other"}]}"#)
        .err()
        .ok_or("Expected duplicate refusal")?;
    assert_eq!(error.details.issues[0].path, "/items/0/a~1b~0c");
    assert_eq!(error.details.issues[0].rule, "duplicate_property");
    assert!(!error.details.issues[0].path_truncated);
    assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    let syntax = parse_object(br#"{"items":[{"title":}]}"#)
        .err()
        .ok_or("Expected syntax refusal")?;
    assert_eq!(syntax.details.issues[0].path, "/items/0/title");
    let key = "a".repeat(300);
    let long = format!("{{\"{key}\":1,\"{key}\":2}}");
    let bounded = parse_object(long.as_bytes())
        .err()
        .ok_or("Expected bounded path")?;
    assert!(bounded.details.issues[0].path_truncated);
    assert_eq!(bounded.details.issues[0].path.chars().count(), 256);
    assert!(parse_object(&[b'{', b'"', 0xff, b'"', b':', b'0', b'}']).is_err());
    Ok(())
}

#[test]
fn rejects_nonobjects_and_trailing_input() {
    assert!(parse_object(br#"[1,2]"#).is_err());
    assert!(parse_object(br#"{} {}"#).is_err());
}

#[test]
fn bounded_response_arrays_keep_strict_json_without_relaxing_request_objects()
-> Result<(), Box<dyn std::error::Error>> {
    let array = br#"[{"id":"one"}]"#;
    assert!(craft_core::input::parse_json(array, 512)?.is_array());
    assert!(parse_object(array).is_err());
    assert!(craft_core::input::parse_json(br#"[{"id":"one","id":"two"}]"#, 512).is_err());
    assert!(craft_core::input::parse_json(&vec![b' '; 513], 512).is_err());
    assert!(craft_core::input::parse_json(b"{}", 16_777_217).is_err());
    Ok(())
}

#[test]
fn preserves_multiline_and_international_content() -> Result<(), Box<dyn std::error::Error>> {
    let value = parse_object("{\"body\":\"مرحبا 🦀\\nlet total = 4;\"}".as_bytes())?;
    assert_eq!(value["body"], "مرحبا 🦀\nlet total = 4;");
    Ok(())
}

#[test]
fn digest_is_independent_of_object_property_order() -> Result<(), Box<dyn std::error::Error>> {
    let first = parse_object(br#"{"b":2,"a":{"z":1,"x":0}}"#)?;
    let second = parse_object(br#"{"a":{"x":0,"z":1},"b":2}"#)?;
    assert_eq!(canonical_digest(&first)?, canonical_digest(&second)?);
    assert_ne!(
        canonical_digest(&first)?,
        canonical_digest(&json!({"a": 1}))?
    );
    Ok(())
}

#[test]
fn rejects_unknown_selection_fields_and_out_of_range_limits() {
    assert!(serde_json::from_value::<Selection>(json!({"limti":20})).is_err());
    assert!(serde_json::from_value::<Selection>(json!({"limit":"20"})).is_err());
    let selection = Selection {
        limit: 0,
        ..Selection::default()
    };
    assert!(selection.validate().is_err());
}

#[test]
fn validates_identity_without_changing_opaque_values() {
    let mut resource = ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "tenant-one".into(),
        resource_type: "issue".into(),
        id: "PAY-1".into(),
    };
    assert!(resource.validate().is_ok());
    resource.id = " PAY-1".into();
    assert!(resource.validate().is_err());
    resource.id = "PAY-1?fields=summary".into();
    assert!(resource.validate().is_err());
}

#[test]
fn distinguishes_confirmed_submission_from_remote_completion()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(serde_json::to_value(RequestState::Confirmed)?, "confirmed");
    assert_eq!(serde_json::to_value(JobState::Accepted)?, "accepted");
    assert_ne!(
        serde_json::to_value(JobState::Accepted)?,
        serde_json::to_value(JobState::Succeeded)?
    );
    Ok(())
}

#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Patch {
    #[serde(default, skip_serializing_if = "InputField::is_omitted")]
    title: InputField<String>,
}

#[test]
fn preserves_omitted_null_and_replacement_patch_values() -> Result<(), Box<dyn std::error::Error>> {
    let validator = jsonschema::validator_for(&schema_for::<Patch>()?)?;
    for (wire, expected) in [
        (json!({}), InputField::Omitted),
        (json!({"title":null}), InputField::Null),
        (json!({"title":"مرحبا"}), InputField::Value("مرحبا".into())),
    ] {
        assert!(validator.is_valid(&wire));
        let parsed: Patch = serde_json::from_value(wire.clone())?;
        assert_eq!(parsed.title, expected);
        assert_eq!(serde_json::to_value(parsed)?, wire);
    }
    assert!(!validator.is_valid(&json!({"title":42})));
    assert!(serde_json::from_value::<Patch>(json!({"title":42})).is_err());
    assert!(serde_json::to_value(InputField::<String>::Omitted).is_err());
    Ok(())
}

fn resource() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "tenant-one".into(),
        resource_type: "issue".into(),
        id: "PAY-1".into(),
    }
}

#[test]
fn keeps_provider_tenant_and_grant_references_distinct() -> Result<(), Box<dyn std::error::Error>> {
    let target = resource();
    let tenant = target.tenant_ref();
    assert_eq!(
        tenant.provider_ref(),
        ProviderRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
        }
    );
    tenant.validate()?;
    let grant = GrantRef {
        id: "grant-local".into(),
    };
    grant.validate()?;
    assert_ne!(
        tenant,
        TenantRef {
            tenant: "tenant-two".into(),
            ..tenant.clone()
        }
    );
    assert!(
        serde_json::from_value::<GrantRef>(json!({"id":"grant-local","token":"secret"})).is_err()
    );
    assert!(
        GrantRef {
            id: " grant-local".into()
        }
        .validate()
        .is_err()
    );
    assert!(serde_json::from_value::<JobState>(json!(true)).is_err());
    assert!(serde_json::from_value::<RequestState>(json!("accepted")).is_err());
    Ok(())
}

#[test]
fn validates_canonical_targets_and_preconditions() -> Result<(), Box<dyn std::error::Error>> {
    let mut request = CanonicalRequest {
        command: "jira.issue.update".into(),
        target: resource(),
        input: json!({"summary":"Updated"}),
        selection: Selection::default(),
        preconditions: Some(Preconditions {
            expected_version: Some(7),
            expected_digest: Some(canonical_digest(&json!({"summary":"Original"}))?),
            expected_parent: Some(resource()),
        }),
    };
    request.validate()?;
    let original_digest = canonical_digest(&serde_json::to_value(&request)?)?;
    request.target.tenant = "tenant-two".into();
    assert!(request.validate().is_err());
    assert_ne!(
        original_digest,
        canonical_digest(&serde_json::to_value(&request)?)?
    );
    request.target = resource();
    if let Some(preconditions) = &mut request.preconditions {
        preconditions.expected_digest = Some("sha256:invalid".into());
    }
    assert!(request.validate().is_err());
    request.preconditions = None;
    request.input = json!([]);
    assert!(request.validate().is_err());
    Ok(())
}

#[test]
fn retains_confirmed_effects_in_partial_and_uncertain_failures()
-> Result<(), Box<dyn std::error::Error>> {
    let validator = jsonschema::validator_for(&response_schema::<OperationReceipt>()?)?;
    for (code, state) in [
        (ErrorCode::PartialApply, RequestState::Failed),
        (ErrorCode::WriteOutcomeUnknown, RequestState::SentUnknown),
    ] {
        let receipt = OperationReceipt {
            operation_id: "operation-local".into(),
            request_state: state,
            confirmed_resources: vec![resource()],
            job: Some(RemoteJob {
                resource: resource(),
                state: JobState::Accepted,
            }),
        };
        let mut error = CraftError::new(code, "Operation requires reconciliation.");
        error.details.write_state = state;
        let result = serde_json::to_value(Envelope::failure_with_data(
            "jira.issue.update",
            "request-local".into(),
            error,
            serde_json::to_value(receipt)?,
        ))?;
        assert!(validator.is_valid(&result));
        assert_eq!(result["ok"], false);
        assert_eq!(result["data"]["confirmed_resources"][0]["id"], "PAY-1");
        assert_eq!(result["data"]["job"]["state"], "accepted");
    }
    Ok(())
}

#[test]
fn rejects_excessive_input_size_depth_and_container_items() {
    let oversized = vec![b' '; craft_core::input::MAX_INPUT_BYTES + 1];
    assert!(parse_object(&oversized).is_err());
    let deep = format!("{{\"nested\":{}0{}}}", "[".repeat(66), "]".repeat(66));
    assert!(parse_object(deep.as_bytes()).is_err());
    let large = format!("{{\"items\":[{}]}}", vec!["0"; 10_001].join(","));
    assert!(parse_object(large.as_bytes()).is_err());
    let near_bound = format!("{{\"items\":[{}]}}", vec!["0"; 10_000].join(","));
    assert!(parse_object(near_bound.as_bytes()).is_ok());
}
