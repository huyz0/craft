use super::*;
use craft_cli::attachment_delete::{AttachmentDeleteEnvironment, PreparedAttachmentDelete};
use craft_provider_jira::attachments::parse_record;
use craft_provider_jira::comments::mutation::Ownership;
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Op, IssueMutationPolicyUpdate,
};
use craft_runtime::journal::NativeJournal;
fn project() -> ResourceRef {
    ResourceRef {
        resource_type: "project".into(),
        id: "200".into(),
        ..target()
    }
}
fn allow(setup: &Setup, operation: Op) -> Result<(), CraftError> {
    setup.store.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            target: project(),
            expected_actor: "actor-one".into(),
            operations: vec![operation],
            enabled: true,
            expected_revision: None,
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    Ok(())
}
fn raw() -> Value {
    json!({"id":"301","filename":"design.bin","size":4,"mimeType":"application/octet-stream",
        "author":{"accountId":"actor-one","displayName":"private-sentinel"},
        "content":"https://untrusted.invalid/private-sentinel"})
}
fn request(ownership: Ownership) -> Result<CanonicalRequest, CraftError> {
    let revision = parse_record(&raw())?.metadata_revision;
    Ok(CanonicalRequest {
        command: craft_provider_jira::attachment_delete::DELETE.into(),
        target: project(),
        input: json!({"issue_id":"100","attachment_id":"301","expected_revision":revision,"ownership":ownership}),
        selection: Selection {
            max_bytes: 8_388_608,
            ..Selection::default()
        },
        preconditions: None,
    })
}
fn context() -> Result<Vec<Reply>, serde_json::Error> {
    let issue = json!({"id":"100","fields":{"project":{"id":"200"},"attachment":[raw()]}});
    let permissions = json!({"permissions":{
        "BROWSE_PROJECTS":{"key":"BROWSE_PROJECTS","havePermission":true},
        "DELETE_OWN_ATTACHMENTS":{"key":"DELETE_OWN_ATTACHMENTS","havePermission":true},
        "DELETE_ALL_ATTACHMENTS":{"key":"DELETE_ALL_ATTACHMENTS","havePermission":false}}});
    Ok(vec![
        reply(issue.clone())?,
        reply(raw())?,
        reply(permissions)?,
        reply(raw())?,
        reply(issue)?,
    ])
}
#[test]
fn delete_sends_one_exact_route_and_confirmed_operation_never_sends_again() -> TestResult {
    let setup = Setup::new()?;
    allow(&setup, Op::AttachmentDeleteOwn)?;
    let mut rows = vec![identity(), identity()];
    rows.extend(context()?);
    rows.push(Reply::json(204, b"", &[]));
    let product = TlsServer::start(Reply::Sequence(rows))?;
    let issuer = issuer("read:jira-user read:jira-work write:jira-work")?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let journal = NativeJournal::open(&setup.store, &control()?)?;
    let run = || -> Result<(Value, Option<CraftError>), Box<dyn std::error::Error>> {
        let prepared = PreparedAttachmentDelete::prepare(
            request(Ownership::Own)?,
            "remove-design",
            true,
            "factory",
            &setup.store,
            &control()?,
        )?;
        let (data, error) = prepared.execute(
            AttachmentDeleteEnvironment {
                store: &setup.store,
                client: fixture.client(),
                locked: &locked,
                journal: &journal,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control()?,
            },
            || Ok(()),
        );
        Ok((serde_json::to_value(data)?, error))
    };
    let (first, error) = run()?;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(first["receipt"]["request_state"], "confirmed");
    assert_eq!(
        first["receipt"]["confirmed_resources"][0]["resource_type"],
        "attachment"
    );
    assert_eq!(first["receipt"]["confirmed_resources"][0]["id"], "301");
    assert_eq!(first["before"]["id"], "301");
    assert_eq!(first["http_status"], 204);
    assert!(!first.to_string().contains("private-sentinel"));
    let mut registry = craft_core::registry::Registry::default();
    craft_cli::attachment_delete::command::register(&mut registry)?;
    let envelope = Envelope::success("jira.attachment.delete", "req".into(), first.clone());
    assert!(
        jsonschema::validator_for(
            &registry
                .descriptor("jira.attachment.delete")?
                .response_schema
        )?
        .is_valid(&json!(envelope))
    );
    let (repeat, error) = run()?;
    assert_eq!(
        error.map(|e| e.code),
        Some(ErrorCode::OperationIdReused),
        "a confirmed deletion is never replayed"
    );
    assert_eq!(repeat["receipt"]["request_state"], "confirmed");
    let observed = product.finish()?;
    let deletes = observed
        .iter()
        .filter(|r| r.method == "DELETE")
        .collect::<Vec<_>>();
    assert_eq!(deletes.len(), 1);
    assert!(
        deletes[0].target.starts_with("/ex/jira/")
            && deletes[0].target.ends_with("/rest/api/3/attachment/301"),
        "{}",
        deletes[0].target
    );
    assert_eq!(observed.len(), 8);
    assert_eq!(issuer.finish()?.len(), 1);
    drop(locked);
    drop(fixture);
    setup.no_plaintext_credentials()?;
    Ok(())
}
#[test]
fn own_allowance_does_not_authorize_any_intent_before_network() -> TestResult {
    let setup = Setup::new()?;
    allow(&setup, Op::AttachmentDeleteOwn)?;
    let error = PreparedAttachmentDelete::prepare(
        request(Ownership::Any)?,
        "remove-any",
        true,
        "factory",
        &setup.store,
        &control()?,
    )
    .err()
    .ok_or("own allowance accepted any intent")?;
    assert_eq!(error.code, ErrorCode::PolicyDenied);
    let error = PreparedAttachmentDelete::prepare(
        request(Ownership::Own)?,
        "remove-own",
        false,
        "factory",
        &setup.store,
        &control()?,
    )
    .err()
    .ok_or("missing apply accepted")?;
    assert_eq!(error.code, ErrorCode::ApplyRequired);
    Ok(())
}
#[test]
fn lost_delete_response_is_unknown_and_blocks_replay() -> TestResult {
    let setup = Setup::new()?;
    allow(&setup, Op::AttachmentDeleteOwn)?;
    let mut rows = vec![identity(), identity()];
    rows.extend(context()?);
    rows.push(Reply::Disconnect);
    let product = TlsServer::start(Reply::Sequence(rows))?;
    let issuer = issuer("read:jira-user read:jira-work write:jira-work")?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let journal = NativeJournal::open(&setup.store, &control()?)?;
    let run = || -> Result<(Value, Option<CraftError>), Box<dyn std::error::Error>> {
        let (data, error) = PreparedAttachmentDelete::prepare(
            request(Ownership::Own)?,
            "remove-lost",
            true,
            "factory",
            &setup.store,
            &control()?,
        )?
        .execute(
            AttachmentDeleteEnvironment {
                store: &setup.store,
                client: fixture.client(),
                locked: &locked,
                journal: &journal,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control()?,
            },
            || Ok(()),
        );
        Ok((serde_json::to_value(data)?, error))
    };
    let (first, error) = run()?;
    assert!(error.is_some());
    assert_eq!(first["receipt"]["request_state"], "sent_unknown");
    assert_eq!(first["reconciliation_required"], true);
    let (_, error) = run()?;
    assert_eq!(error.map(|e| e.code), Some(ErrorCode::OperationIdReused));
    assert_eq!(product.finish()?.len(), 8);
    assert_eq!(issuer.finish()?.len(), 1);
    Ok(())
}
