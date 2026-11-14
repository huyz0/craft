use std::collections::VecDeque;
use std::sync::Mutex;

use craft_core::execution::{NativeCall, NativeResponse};
use craft_test_support::state::ManualClock;

use super::*;

fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "10000".into(),
    }
}
fn policy() -> CreatePolicy {
    CreatePolicy {
        enabled: true,
        target: target(),
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        expected_actor: "actor-one".into(),
    }
}
fn request(input: Value) -> CanonicalRequest {
    CanonicalRequest {
        command: COMMAND.into(),
        target: target(),
        input,
        selection: Selection::default(),
        preconditions: None,
    }
}
fn prepared(input: Value) -> Result<PreparedCreate, CraftError> {
    PreparedCreate::prepare(request(input), "create-report", true, "request", policy())
}
fn basic() -> Value {
    json!({"issue_type_id":"10001","summary":"Example"})
}

fn qualify_basic(
    host: &PreparedCreate,
    mut rows: Vec<Value>,
) -> Result<(FrozenCreate, String), CraftError> {
    let calls = Arc::new(Mutex::new(Vec::new()));
    host.qualify(
        MetadataPrincipal {
            grant: policy().grant,
            actor: "actor-one".into(),
        },
        Arc::new(ManualClock::new(100)),
        |operation| {
            Ok(Box::new(Replay {
                digest: operation.digest().into(),
                responses: std::mem::take(&mut rows).into(),
                calls: calls.clone(),
            }))
        },
        &|| Ok(()),
    )
}

#[test]
fn reviewed_material_matches_fresh_payload_but_refuses_default_configuration_and_label_drift()
-> Result<(), Box<dyn std::error::Error>> {
    let host = prepared(basic())?;
    let (original, original_digest) = qualify_basic(&host, responses())?;
    let payload = original.call().body.clone().ok_or("payload")?;
    let host = host.constrain_reviewed_material(payload.clone(), original_digest.clone())?;
    host.check_reviewed_material(&original, &original_digest)?;
    for mode in [
        "default",
        "configuration",
        "project_label",
        "type_label",
        "schema",
    ] {
        let mut rows = responses();
        match mode {
            "default" => rows[2]["fields"][3]["defaultValue"] = json!("Changed default"),
            "configuration" => rows[2]["fields"][3]["configuration"] = json!({"scope":"changed"}),
            "project_label" => rows[0]["key"] = json!("OTHER"),
            "type_label" => rows[1]["issueTypes"][0]["name"] = json!("Changed"),
            _ => rows[2]["fields"][3]["schema"]["configuration"] = json!({"mode":"changed"}),
        }
        let (frozen, digest) = qualify_basic(&host, rows)?;
        assert_eq!(frozen.call().body, Some(payload.clone()), "{mode}");
        assert_eq!(
            host.check_reviewed_material(&frozen, &digest)
                .err()
                .map(|e| e.code),
            Some(ErrorCode::VersionConflict),
            "{mode}"
        );
    }
    let mut different = payload;
    different["fields"]["summary"] = json!("Other approved input");
    let changed =
        prepared(basic())?.constrain_reviewed_material(different, original_digest.clone())?;
    assert!(
        changed
            .check_reviewed_material(&original, &original_digest)
            .is_err()
    );
    Ok(())
}

#[test]
fn reviewed_material_does_not_depend_on_field_order_or_metadata_observation_time()
-> Result<(), Box<dyn std::error::Error>> {
    let host = prepared(basic())?;
    let (_, original) = qualify_basic(&host, responses())?;
    let mut reversed = responses();
    reversed[2]["fields"]
        .as_array_mut()
        .ok_or("fields")?
        .reverse();
    let (_, reordered) = qualify_basic(&host, reversed)?;
    assert_eq!(original, reordered);
    for (payload, digest) in [
        (json!({"fields":{}}), "bad".into()),
        (
            json!({"fields":{},"other":1}),
            format!("sha256:{}", "0".repeat(64)),
        ),
    ] {
        assert!(
            prepared(basic())?
                .constrain_reviewed_material(payload, digest)
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn reviewed_option_labels_and_disabled_state_are_material_but_routing_urls_are_not()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = basic();
    input["custom_fields"] = json!({"customfield_10000":{"id":"50000"}});
    let mut rows = responses();
    let mut choice = json!({"fieldId":"customfield_10000","key":"customfield_10000","name":"Disposition","required":false,"operations":["set"],"schema":{"type":"option","custom":"com.atlassian.jira.plugin.system.customfieldtypes:select","customId":10000},"allowedValues":[{"id":"50000","value":"Approved","disabled":false,"self":"https://untrusted.invalid/old"}]});
    rows[2]["fields"]
        .as_array_mut()
        .ok_or("fields")?
        .push(choice.clone());
    rows[2]["total"] = json!(6);
    let host = prepared(input)?;
    let (frozen, digest) = qualify_basic(&host, rows.clone())?;
    let host = host.constrain_reviewed_material(
        frozen.call().body.clone().ok_or("payload")?,
        digest.clone(),
    )?;
    for mode in ["label", "disabled", "routing"] {
        let mut changed = rows.clone();
        match mode {
            "label" => choice["allowedValues"][0]["value"] = json!("Rejected"),
            "disabled" => {
                choice["allowedValues"][0]["value"] = json!("Approved");
                choice["allowedValues"][0]["disabled"] = json!(true);
            }
            _ => {
                choice["allowedValues"][0]["disabled"] = json!(false);
                choice["allowedValues"][0]["self"] = json!("https://untrusted.invalid/changed");
            }
        }
        changed[2]["fields"][5] = choice.clone();
        let (current, current_digest) = qualify_basic(&host, changed)?;
        assert_eq!(current.call().body, frozen.call().body);
        assert_eq!(
            host.check_reviewed_material(&current, &current_digest)
                .is_ok(),
            mode == "routing",
            "{mode}"
        );
    }
    Ok(())
}

#[test]
fn preparation_refuses_bad_input_policy_apply_and_output_without_opening_auth()
-> Result<(), Box<dyn std::error::Error>> {
    for input in [
        json!({"summary":"Example"}),
        json!({"issue_type_id":"bad","summary":"Example"}),
        json!({"issue_type_id":"10001","summary":" "}),
        json!({"issue_type_id":"10001","summary":"Example","labels":["bad label"]}),
        json!({"issue_type_id":"10001","summary":"Example","parent_issue_id":null}),
        json!({"issue_type_id":"10001","summary":"Example","assignee_account_id":"unknown"}),
        json!({"issue_type_id":"10001","summary":"Example","custom_fields":{"status":"Done"}}),
        json!({"issue_type_id":"10001","summary":"Example","unrelated":true}),
    ] {
        assert!(prepared(input).is_err());
    }
    let mut oversized = basic();
    oversized["description"] = json!("x".repeat(craft_core::input::MAX_INPUT_BYTES));
    assert!(prepared(oversized).is_err());
    let error = |result: Result<PreparedCreate, CraftError>| {
        result.err().ok_or("Expected preparation refusal")
    };
    assert_eq!(
        error(PreparedCreate::prepare(
            request(basic()),
            "create-report",
            false,
            "request",
            policy()
        ))?
        .code,
        ErrorCode::ApplyRequired
    );
    let mut denied = policy();
    denied.enabled = false;
    assert_eq!(
        error(PreparedCreate::prepare(
            request(basic()),
            "create-report",
            true,
            "request",
            denied
        ))?
        .code,
        ErrorCode::PolicyDenied
    );
    let mut other = policy();
    other.target.id = "20000".into();
    assert_eq!(
        error(PreparedCreate::prepare(
            request(basic()),
            "create-report",
            true,
            "request",
            other
        ))?
        .code,
        ErrorCode::PolicyDenied
    );
    let mut small = request(basic());
    small.selection.max_bytes = 512;
    assert_eq!(
        error(PreparedCreate::prepare(
            small,
            "create-report",
            true,
            "request",
            policy()
        ))?
        .code,
        ErrorCode::OutputBudgetTooSmall
    );
    let mut cursor = request(basic());
    cursor.selection.cursor = Some("ignored".into());
    assert!(PreparedCreate::prepare(cursor, "create-report", true, "request", policy()).is_err());
    let mut precondition = request(basic());
    precondition.preconditions = Some(craft_core::contracts::Preconditions {
        expected_version: Some(3),
        expected_digest: None,
        expected_parent: None,
    });
    assert!(
        PreparedCreate::prepare(precondition, "create-report", true, "request", policy()).is_err()
    );
    Ok(())
}

#[test]
fn logical_binding_includes_issue_type_and_generic_execution_cannot_send()
-> Result<(), Box<dyn std::error::Error>> {
    let first = prepared(basic())?;
    let mut input = basic();
    input["issue_type_id"] = json!("10002");
    let other = prepared(input)?;
    assert_ne!(first.binding.request_digest, other.binding.request_digest);
    let mut context = Replay {
        digest: first.create.digest().into(),
        responses: VecDeque::new(),
        calls: Arc::new(Mutex::new(Vec::new())),
    };
    assert_eq!(
        first
            .registry
            .execute(&first.create, &mut context)
            .err()
            .ok_or("Expected host requirement")?
            .code,
        ErrorCode::PolicyDenied
    );
    assert!(
        context
            .calls
            .lock()
            .map_err(|_| "Poisoned calls")?
            .is_empty()
    );
    Ok(())
}

struct Replay {
    digest: String,
    responses: VecDeque<Value>,
    calls: Arc<Mutex<Vec<String>>>,
}
impl OperationContext for Replay {
    fn verify_operation(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        if operation.digest() != self.digest {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Fixture operation changed.",
            ));
        }
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls
            .lock()
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Poisoned fixture calls."))?
            .push(call.route.clone());
        Ok(NativeResponse {
            status: 200,
            body: self.responses.pop_front().ok_or_else(|| {
                CraftError::new(ErrorCode::Internal, "Unexpected fixture request.")
            })?,
            request_id: None,
            retry_after: None,
            link_headers: None,
        })
    }
}
fn responses() -> Vec<Value> {
    let field = |id: &str, kind: &str, required: bool| json!({"fieldId":id,"key":id,"name":id,"required":required,"operations":["set"],"schema":{"type":kind,"system":id}});
    vec![
        json!({"id":"10000","key":"ED"}),
        json!({"startAt":0,"maxResults":50,"total":1,"issueTypes":[{"id":"10001","name":"Task","subtask":false}]}),
        json!({"startAt":0,"maxResults":50,"total":5,"fields":[field("project","project",true),field("issuetype","issuetype",true),field("summary","string",true),field("description","string",false),field("assignee","user",false)]}),
    ]
}

#[test]
fn composed_metadata_and_account_reads_freeze_only_the_requested_body()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = basic();
    input["description"] = json!("Literal <tag>");
    input["assignee_account_id"] = json!("account-two");
    let host = prepared(input)?;
    let principal = MetadataPrincipal {
        grant: policy().grant,
        actor: "actor-one".into(),
    };
    let clock = Arc::new(ManualClock::new(100));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (frozen, _) = host.qualify(
        principal.clone(),
        clock.clone(),
        |operation| {
            let responses = if operation.request().command == create_metadata::COMMAND {
                responses()
            } else {
                vec![json!([{"accountId":"account-two","active":true}])]
            };
            Ok(Box::new(Replay {
                digest: operation.digest().into(),
                responses: responses.into(),
                calls: calls.clone(),
            }))
        },
        &|| Ok(()),
    )?;
    frozen.verify(&host.create, &principal)?;
    let body = frozen.call().body.as_ref().ok_or("Missing frozen body")?;
    assert_eq!(body["fields"]["issuetype"]["id"], "10001");
    assert_eq!(
        body["fields"]["assignee"],
        json!({"accountId":"account-two"})
    );
    assert_eq!(
        body["fields"]["description"]["content"][0]["content"][0]["text"],
        "Literal <tag>"
    );
    assert_eq!(
        *calls.lock().map_err(|_| "Poisoned calls")?,
        vec![
            "metadata-project",
            "metadata-types",
            "metadata-fields",
            "metadata-assignee"
        ]
    );
    clock.advance(std::time::Duration::from_secs(60))?;
    assert_eq!(
        frozen
            .verify(&host.create, &principal)
            .err()
            .ok_or("Expected expiry")?
            .code,
        ErrorCode::VersionConflict
    );
    Ok(())
}

#[test]
fn prior_terminal_or_changed_operations_refuse_replay_and_retain_confirmed_receipts()
-> Result<(), Box<dyn std::error::Error>> {
    let host = prepared(basic())?;
    let mut prior = JournalRecord::initial(host.binding.clone())?;
    host.check_prior(Some(&prior))?;
    let resource = ResourceRef {
        resource_type: "issue".into(),
        id: "10002".into(),
        ..target()
    };
    prior.steps.insert(
        ROUTE.into(),
        craft_core::storage::JournalStep {
            state: SubmissionState::Confirmed,
            confirmed_resources: vec![resource],
            request_digest: Some(host.binding.request_digest.clone()),
            job: None,
        },
    );
    prior.revision = 3;
    let error = host
        .check_prior(Some(&prior))
        .err()
        .ok_or("Expected replay refusal")?;
    let result = host
        .completion
        .before_send_failure_with_record(error, Some(&prior));
    let data = result.envelope.data.ok_or("Missing receipt")?;
    assert_eq!(data["receipt"]["confirmed_resources"][0]["id"], "10002");
    assert_eq!(data["receipt"]["request_state"], "confirmed");
    assert_eq!(data["journal_revision"], 3);
    assert_eq!(data["replay_authorized"], false);
    let changed = prepared(basic())?;
    prior.binding.actor = "another-actor".into();
    assert_eq!(
        changed
            .check_prior(Some(&prior))
            .err()
            .ok_or("Expected binding refusal")?
            .code,
        ErrorCode::OperationIdReused
    );
    Ok(())
}

#[test]
fn projected_failure_keeps_cache_update_separate_from_an_unsent_issue()
-> Result<(), Box<dyn std::error::Error>> {
    let mut request = request(basic());
    request.selection.fields = vec!["issue_key".into()];
    let mut host = PreparedCreate::prepare(request, "create-report", true, "request", policy())?;
    host.completion
        .set_cache_update(Some(CredentialCacheUpdate {
            grant: policy().grant,
            write_state: RequestState::Confirmed,
            cleanup_pending: Some(true),
        }));
    let result = host.completion.before_send_failure(CraftError::new(
        ErrorCode::PermissionDenied,
        "Metadata refused.",
    ));
    let data = result.envelope.data.ok_or("Missing cache evidence")?;
    assert_eq!(data["receipt"]["request_state"], "not_sent");
    assert_eq!(data["credential_cache_update"]["write_state"], "confirmed");
    assert_eq!(data["credential_cache_update"]["cleanup_pending"], true);
    assert!(
        data["receipt"]["confirmed_resources"]
            .as_array()
            .ok_or("Missing resources")?
            .is_empty()
    );
    Ok(())
}

#[test]
fn read_failure_state_does_not_claim_an_issue_send_and_unknown_cache_write_stays_distinct()
-> Result<(), Box<dyn std::error::Error>> {
    let mut host = prepared(basic())?;
    host.completion
        .set_cache_update(Some(CredentialCacheUpdate {
            grant: policy().grant,
            write_state: RequestState::SentUnknown,
            cleanup_pending: None,
        }));
    let mut error = CraftError::new(ErrorCode::RemoteUnavailable, "Metadata request failed.");
    error.details.write_state = RequestState::Failed;
    let result = host.completion.before_send_failure(error);
    assert_eq!(
        result
            .envelope
            .error
            .as_ref()
            .map(|error| error.details.write_state),
        Some(RequestState::NotSent)
    );
    let data = result.envelope.data.ok_or("Missing separate effects")?;
    assert_eq!(data["receipt"]["request_state"], "not_sent");
    assert_eq!(
        data["credential_cache_update"]["write_state"],
        "sent_unknown"
    );
    assert!(data["credential_cache_update"]["cleanup_pending"].is_null());
    assert_eq!(data["replay_authorized"], false);
    Ok(())
}
