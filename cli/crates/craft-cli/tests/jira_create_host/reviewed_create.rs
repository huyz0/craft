use super::*;
use crate::conformance::Observed;
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::registry::Registry;
use craft_provider_jira::create_metadata::{self, JiraMetadata, MetadataPrincipal};
use std::collections::VecDeque;

fn metadata_rows() -> Vec<Value> {
    let field = |id: &str, kind: &str, required| json!({"fieldId":id,"key":id,"name":id,"required":required,"operations":["set"],"schema":{"type":kind,"system":id}});
    vec![
        json!({"id":"10000","key":"ED"}),
        json!({"startAt":0,"maxResults":50,"total":1,"issueTypes":[{"id":"10001","name":"Task","subtask":false}]}),
        json!({"startAt":0,"maxResults":50,"total":4,"fields":[field("project","project",true),field("issuetype","issuetype",true),field("summary","string",true),field("description","string",false)]}),
    ]
}
struct PriorObservation {
    digest: String,
    rows: VecDeque<Value>,
}
impl OperationContext for PriorObservation {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, op: &PreparedOperation) -> Result<(), CraftError> {
        if op.digest() != self.digest {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Prior observation binding changed.",
            ));
        }
        Ok(())
    }
    fn invoke(&mut self, _: &NativeCall) -> Result<NativeResponse, CraftError> {
        Ok(NativeResponse {
            status: 200,
            body: self.rows.pop_front().ok_or_else(|| {
                CraftError::new(ErrorCode::Internal, "Unexpected prior observation read.")
            })?,
            request_id: None,
            retry_after: None,
            link_headers: None,
        })
    }
}
fn approved_metadata(grant: &GrantRef) -> Result<String, Box<dyn std::error::Error>> {
    let clock = Arc::new(ManualClock::new(100));
    let mut registry = Registry::default();
    registry.register_provider(&JiraMetadata::new(clock.clone()))?;
    let operation = registry.prepare(CanonicalRequest {
        command: create_metadata::COMMAND.into(),
        target: target(),
        input: json!({"issue_type_id":"10001","page_size":50,"parent_issue_id":null}),
        selection: Selection::default(),
        preconditions: None,
    })?;
    let rows = metadata_rows();
    let mut prior = PriorObservation {
        digest: operation.digest().into(),
        rows: rows.clone().into(),
    };
    let context = create_metadata::read_context(
        &operation,
        &mut prior,
        MetadataPrincipal {
            grant: grant.clone(),
            actor: "actor-one".into(),
        },
        clock,
    )?;
    let summary = context.summary();
    let mut fields = summary.fields.clone();
    fields.sort_by(|a, b| a.id.cmp(&b.id));
    let mut details = BTreeMap::new();
    for field in rows[2]["fields"].as_array().ok_or("fields")? {
        details.insert(
            field["fieldId"].as_str().ok_or("field identity")?,
            json!({"default_value":null,"configuration":null,"native_schema":field["schema"]}),
        );
    }
    Ok(craft_core::input::canonical_digest(
        &json!({"target":summary.target,"project_key":summary.project_key,"issue_type_id":summary.issue_type_id,"issue_type_name":summary.issue_type_name,"subtask":summary.subtask,"parent":summary.parent,"fields":fields,"field_details":details}),
    )?)
}
fn run_case(mode: &str, boundary: Option<usize>) -> TestResult {
    let setup = Setup::new()?;
    let invocation = control(CancellationToken::default(), 30_000)?;
    let policy = setup.profiles.set_create_policy(
        "factory",
        CreatePolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            enabled: true,
            expected_revision: None,
        },
        true,
        &invocation,
        |_| Ok(()),
    )?;
    let mut expected_payload =
        json!({"fields":{"project":{"id":"10000"},"issuetype":{"id":"10001"},"summary":"Example"}});
    if mode == "payload" {
        expected_payload["fields"]["summary"] = json!("Unapproved replacement");
    }
    let digest = if mode == "digest" {
        format!("sha256:{}", "0".repeat(64))
    } else {
        approved_metadata(&setup.grant)?
    };
    let mut rows = metadata_rows();
    if mode == "configuration" {
        rows[2]["fields"][3]["configuration"] = json!({"scope":"changed"});
    }
    if mode == "default" {
        rows[2]["fields"][3]["defaultValue"] = json!("Changed default");
    }
    if let Some(boundary) = boundary {
        rows.truncate(boundary);
    }
    let issuer = issuer(SCOPES)?;
    let mut replies = vec![identity(), identity()];
    for row in rows {
        replies.push(Reply::json(200, &serde_json::to_vec(&row)?, &[]));
    }
    if mode == "match" {
        replies.push(Reply::json(201, br#"{"id":"10003","key":"ED-24"}"#, &[]));
    }
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let clock = Arc::new(ManualClock::new(1000));
    let fixture = setup.fixture(&issuer, &product, clock.clone())?;
    let locked = fixture.lock(&setup.grant, &invocation)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    clock.advance(Duration::from_secs(3600))?;
    let prepared = PreparedCreate::prepare(
        CanonicalRequest {
            command: COMMAND.into(),
            target: target(),
            input: json!({"issue_type_id":"10001","summary":"Example"}),
            selection: Selection::default(),
            preconditions: None,
        },
        OPERATION,
        true,
        "reviewed-create",
        CreatePolicy {
            enabled: true,
            target: target(),
            grant: setup.grant.clone(),
            expected_actor: "actor-one".into(),
        },
    )?
    .constrain_reviewed_material(expected_payload, digest)?;
    let calls = product.request_counter();
    let revoked = AtomicBool::new(false);
    let result = prepared
        .execute_cached_with_policy_check(
            fixture.client(),
            &locked,
            &setup.journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            invocation.clone(),
            || {
                if let Some(boundary) = boundary
                    && calls.load(Ordering::SeqCst) >= boundary + 2
                    && !revoked.swap(true, Ordering::SeqCst)
                {
                    setup.profiles.set_create_policy(
                        "factory",
                        CreatePolicyUpdate {
                            target: target(),
                            expected_actor: "actor-one".into(),
                            enabled: false,
                            expected_revision: Some(1),
                        },
                        true,
                        &invocation,
                        |_| Ok(()),
                    )?;
                }
                setup.profiles.verify_create_policy(&policy, &invocation)
            },
        )
        .observed();
    let envelope = result.envelope;
    assert_eq!(envelope.ok, mode == "match", "{mode}: {envelope:?}");
    let data = envelope.data.ok_or("receipt")?;
    assert!(data["credential_cache_update"].is_object());
    assert_eq!(
        data["receipt"]["request_state"],
        if mode == "match" {
            "confirmed"
        } else {
            "not_sent"
        }
    );
    assert_eq!(setup.journal.get(OPERATION)?.is_some(), mode == "match");
    let requests = product.finish()?;
    assert_eq!(requests.len(), count);
    assert_eq!(
        requests.iter().filter(|r| r.method == "POST").count(),
        usize::from(mode == "match")
    );
    if mode == "match" {
        assert_eq!(
            serde_json::from_slice::<Value>(&requests.last().ok_or("post")?.body)?["fields"]["summary"],
            "Example"
        );
    }
    if boundary.is_some() {
        assert!(revoked.load(Ordering::SeqCst));
    }
    assert_eq!(issuer.finish()?.len(), 2);
    drop(locked);
    drop(fixture);
    drop(clock);
    setup.assert_no_plaintext()?;
    setup.cleanup()
}
macro_rules! case {
    ($name:ident,$mode:literal,$boundary:expr) => {
        #[test]
        fn $name() -> TestResult {
            run_case($mode, $boundary)
        }
    };
}
case!(
    reviewed_create_sends_only_matching_fresh_material,
    "match",
    None
);
case!(
    reviewed_payload_drift_preserves_cache_receipt_without_claim,
    "payload",
    None
);
case!(
    reviewed_metadata_digest_drift_prevents_claim,
    "digest",
    None
);
case!(
    reviewed_default_drift_prevents_claim_for_unchanged_payload,
    "default",
    None
);
case!(
    reviewed_configuration_drift_prevents_claim_for_unchanged_payload,
    "configuration",
    None
);
case!(
    reviewed_policy_revocation_after_project_read_stops_next_metadata_exchange,
    "revoked",
    Some(1)
);
case!(
    reviewed_policy_revocation_after_type_read_stops_field_exchange,
    "revoked",
    Some(2)
);
case!(
    reviewed_policy_revocation_after_fields_read_prevents_claim,
    "revoked",
    Some(3)
);
