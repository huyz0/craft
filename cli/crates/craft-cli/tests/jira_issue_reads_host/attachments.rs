use super::*;
use crate::conformance::Observed;
use craft_cli::attachment_reads::command::{AttachmentListArgs, AttachmentReadArgs, PreparedRead};
use craft_cli::attachment_reads::{PreparedAttachmentRead, ReadEnvironment};
use craft_cli::profiles::InvocationOptions;
use craft_provider_jira::attachments as native;
fn metadata(id: &str) -> Value {
    json!({"id":id,"filename":format!("設計-{id}.txt"),"size":12,"mimeType":"text/plain","author":{"accountId":"human","displayName":"private-sentinel"},"content":"https://invalid.example/private-sentinel"})
}
fn prepare(
    setup: &Setup,
    command: &str,
    fields: &[&str],
    cursor: Option<&str>,
    max: u32,
) -> Result<PreparedAttachmentRead, CraftError> {
    PreparedAttachmentRead::prepare(
        CanonicalRequest {
            command: command.into(),
            target: target(),
            input: match command {
                native::GET => json!({"id":"101"}),
                native::LIST => json!({"issue_id":"100"}),
                _ => json!({}),
            },
            selection: Selection {
                fields: fields.iter().map(|f| (*f).into()).collect(),
                limit: 1,
                cursor: cursor.map(String::from),
                max_bytes: max,
            },
            preconditions: None,
        },
        "attachment-read",
        IssueReadPolicy {
            target: target(),
            grant: setup.grant.clone(),
            expected_actor: "actor-one".into(),
        },
    )
}
#[test]
fn attachment_lists_buffer_complete_issue_fields_without_refetching_or_url_output() -> TestResult {
    let setup = Setup::new()?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(json!({"id":"100","fields":{"attachment":[metadata("101"),metadata("102")]}}))?,
        identity(),
        identity(),
        reply(metadata("101"))?,
        identity(),
        reply(json!({"enabled":false,"uploadLimit":0}))?,
    ]))?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let c = control()?;
    let locked = fixture.lock(&setup.grant, &c)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let run = |host: &PreparedAttachmentRead| -> Result<Envelope, CraftError> {
        Ok(host
            .execute(
                ReadEnvironment {
                    store: &setup.store,
                    client: fixture.client(),
                    locked: &locked,
                    tls: TlsPolicy::AdditionalCaBundle(product.root()),
                    control: control()?,
                },
                || Ok(()),
            )
            .observed()
            .envelope)
    };
    let first = run(&prepare(&setup, native::LIST, &[], None, 262144)?)?;
    assert!(first.ok, "{:?}", first.error);
    assert_eq!(data(&first)?["items"][0]["id"], "101");
    assert!(data(&first)?["items"][0].get("author_account_id").is_none());
    let changed = run(&prepare(
        &setup,
        native::LIST,
        &["author_account_id"],
        Some(cursor(&first)?),
        262144,
    )?)?;
    assert!(!changed.ok);
    let second = run(&prepare(
        &setup,
        native::LIST,
        &[],
        Some(cursor(&first)?),
        262144,
    )?)?;
    assert!(second.ok, "{:?}", second.error);
    assert_eq!(data(&second)?["items"][0]["id"], "102");
    assert_eq!(data(&second)?["cached"], true);
    assert!(!second.page.as_ref().ok_or("page")?.has_more);
    let get = run(&prepare(
        &setup,
        native::GET,
        &["author_account_id"],
        None,
        262144,
    )?)?;
    assert!(get.ok, "{:?}", get.error);
    assert_eq!(data(&get)?["attachment"]["author_account_id"], "human");
    assert!(data(&get)?["attachment"].get("filename").is_none());
    assert!(!serde_json::to_string(&get)?.contains("private-sentinel"));
    let settings = run(&prepare(&setup, native::SETTINGS, &[], None, 262144)?)?;
    assert!(settings.ok, "{:?}", settings.error);
    assert_eq!(
        data(&settings)?["settings"],
        json!({"enabled":false,"upload_limit_bytes":0})
    );
    let registry = craft_cli::catalog::registry()?;
    for (envelope, command, fields) in [
        (&first, native::LIST, vec![]),
        (&get, native::GET, vec!["author_account_id".into()]),
        (&settings, native::SETTINGS, vec![]),
    ] {
        let schema = registry
            .projection(command, &fields)?
            .ok_or("projection")?
            .response_schema(&registry.descriptor(command)?.response_schema)?;
        assert!(jsonschema::validator_for(&schema)?.is_valid(&json!(envelope)));
    }
    let observed = product.finish()?;
    assert_eq!(observed.len(), 8);
    let issues: Vec<_> = observed
        .iter()
        .filter(|r| r.target.contains("/issue/100?"))
        .collect();
    assert_eq!(issues.len(), 1);
    assert!(issues[0].target.contains("fields=attachment"));
    assert!(observed.iter().all(|r| r.method == "GET"));
    issuer.finish()?;
    drop(locked);
    drop(fixture);
    setup.no_plaintext_credentials()?;
    Ok(())
}
#[test]
fn attachment_read_allowances_remain_explicit_and_revocation_precedes_auth() -> TestResult {
    let setup = Setup::new()?;
    let c = control()?;
    let options = InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("factory"),
        request_id: "attachment-policy",
        max_bytes: 262144,
    };
    let args = || {
        AttachmentReadArgs::List(AttachmentListArgs {
            issue_id: "100".into(),
            limit: 20,
            cursor: None,
            cloud_id: None,
        })
    };
    let configure = |operations, enabled, revision| {
        setup.store.set_issue_read_policy(
            "factory",
            IssueReadPolicyUpdate {
                target: target(),
                expected_actor: "actor-one".into(),
                operations,
                enabled,
                expected_revision: revision,
            },
            true,
            &c,
            |_| Ok(()),
        )
    };
    configure(vec![IssueReadOperation::Get], true, None)?;
    assert!(PreparedRead::prepare(args(), &[], &options, &setup.store, &c).is_err());
    configure(vec![IssueReadOperation::AttachmentList], true, Some(1))?;
    let prepared = PreparedRead::prepare(args(), &[], &options, &setup.store, &c)?;
    configure(vec![IssueReadOperation::AttachmentList], false, Some(2))?;
    let product = TlsServer::start(Reply::Sequence(vec![]))?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &c)?;
    let result = prepared
        .execute(ReadEnvironment {
            store: &setup.store,
            client: fixture.client(),
            locked: &locked,
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: control()?,
        })
        .observed();
    assert!(!result.envelope.ok);
    assert_eq!(
        result.envelope.error.map(|e| e.code),
        Some(ErrorCode::PolicyDenied)
    );
    assert_eq!(product.finish()?.len(), 0);
    assert_eq!(issuer.finish()?.len(), 0);
    drop(locked);
    drop(fixture);
    setup.no_plaintext_credentials()?;
    Ok(())
}
#[test]
fn attachment_late_policy_changes_suppress_native_metadata_and_keep_renewal() -> TestResult {
    let setup = Setup::new()?;
    setup.store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![IssueReadOperation::AttachmentGet],
            enabled: true,
            expected_revision: None,
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(metadata("101"))?,
    ]))?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let clock = Arc::new(IssuePolicyClock {
        base: ManualClock::new(1000),
        store: Mutex::new(ProfileStore::open(&setup.root, &control()?)?),
        requests: product.request_counter(),
        threshold: 3,
        operation: IssueReadOperation::AttachmentGet,
        mode: "remove_operation",
        changed: AtomicBool::new(false),
    });
    let fixture = setup.fixture(&issuer, &product, clock.clone())?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    clock.base.advance(Duration::from_secs(3600))?;
    let options = InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("factory"),
        request_id: "attachment-late-policy",
        max_bytes: 262144,
    };
    let host = PreparedRead::prepare(
        AttachmentReadArgs::Get(craft_cli::attachment_reads::command::AttachmentGetArgs {
            id: "101".into(),
            cloud_id: None,
        }),
        &[],
        &options,
        &setup.store,
        &control()?,
    )?;
    let result = host
        .execute(ReadEnvironment {
            store: &setup.store,
            client: fixture.client(),
            locked: &locked,
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: control()?,
        })
        .observed();
    assert!(!result.envelope.ok);
    assert!(data(&result.envelope)?["attachment"].is_null());
    assert!(!data(&result.envelope)?["credential_cache_update"].is_null());
    assert!(clock.changed.load(Ordering::SeqCst));
    assert_eq!(product.finish()?.len(), 3);
    assert_eq!(issuer.finish()?.len(), 2);
    drop(locked);
    drop(fixture);
    drop(clock);
    setup.no_plaintext_credentials()?;
    Ok(())
}
#[test]
fn attachment_missing_scope_and_remote_failures_preserve_cache_evidence_without_retry() -> TestResult
{
    for mode in ["scope", "denied", "missing", "malformed"] {
        let setup = Setup::new()?;
        let scopes = if mode == "scope" {
            "read:jira-user"
        } else {
            "read:jira-user read:jira-work"
        };
        let issuer = issuer(scopes)?;
        let mut rows = vec![identity(), identity()];
        if mode != "scope" {
            rows.push(match mode {
                "denied" => Reply::json(403, b"{}", &[]),
                "missing" => Reply::json(404, b"{}", &[]),
                _ => reply(json!({"id":"999"}))?,
            });
        }
        let count = rows.len();
        let product = TlsServer::start(Reply::Sequence(rows))?;
        let clock = Arc::new(ManualClock::new(1000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let locked = fixture.lock(&setup.grant, &control()?)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        clock.advance(Duration::from_secs(3600))?;
        let result = prepare(&setup, native::GET, &[], None, 262144)?
            .execute(
                ReadEnvironment {
                    store: &setup.store,
                    client: fixture.client(),
                    locked: &locked,
                    tls: TlsPolicy::AdditionalCaBundle(product.root()),
                    control: control()?,
                },
                || Ok(()),
            )
            .observed();
        assert!(!result.envelope.ok, "{mode}");
        assert!(!data(&result.envelope)?["credential_cache_update"].is_null());
        assert_eq!(product.finish()?.len(), count);
        assert_eq!(issuer.finish()?.len(), 2);
        drop(locked);
        drop(fixture);
        setup.no_plaintext_credentials()?;
    }
    Ok(())
}

#[test]
fn every_attachment_read_refuses_without_work_scope_and_stops_at_its_deadline() -> TestResult {
    for command in [native::GET, native::LIST, native::SETTINGS] {
        for case in ["scope", "deadline"] {
            let setup = Setup::new()?;
            let issuer = issuer(bounded_scopes(case))?;
            let product = TlsServer::start(Reply::Sequence(bounded_replies(case)))?;
            let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
            let locked = fixture.lock(&setup.grant, &control()?)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            let (invocation, worker) = if case == "deadline" {
                let (invocation, worker) = deadline_at_request(&product, 3)?;
                (invocation, Some(worker))
            } else {
                (control()?, None)
            };
            let envelope = prepare(&setup, command, &[], None, 262_144)?
                .execute(
                    ReadEnvironment {
                        store: &setup.store,
                        client: fixture.client(),
                        locked: &locked,
                        tls: TlsPolicy::AdditionalCaBundle(product.root()),
                        control: invocation,
                    },
                    || Ok(()),
                )
                .observed()
                .envelope;
            join_deadline(worker)?;
            assert_eq!(
                code(&envelope),
                Some(bounded_code(case)),
                "{command} {case}: {:?}",
                envelope.error
            );
            let sent = product.finish()?.len();
            assert_eq!(
                sent,
                if case == "scope" { 2 } else { 3 },
                "{command} {case}"
            );
            assert_eq!(issuer.finish()?.len(), 1);
        }
    }
    Ok(())
}
