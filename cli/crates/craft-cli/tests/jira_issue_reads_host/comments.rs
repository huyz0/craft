use super::*;
use crate::conformance::Observed;
use craft_cli::issue_comments::command::{CommentListArgs, CommentReadArgs, PreparedRead};
use craft_cli::issue_comments::{PreparedCommentRead, ReadEnvironment};
use craft_cli::profiles::InvocationOptions;
use craft_provider_jira::comments as native;
fn comment(id: &str) -> Value {
    json!({"id":id,"author":{"accountId":"human-one","emailAddress":"private-sentinel"},"updated":"2026-11-11T00:01:00.000+0000","visibility":{"type":"role","identifier":"Administrators","value":"Administrators"},"jsdPublic":false,"body":{"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"Keep scope 文字"}]}]}})
}
fn prepare(
    setup: &Setup,
    command: &str,
    fields: &[&str],
    cursor: Option<&str>,
) -> Result<PreparedCommentRead, CraftError> {
    PreparedCommentRead::prepare(
        CanonicalRequest {
            command: command.into(),
            target: target(),
            input: if command == native::GET {
                json!({"issue_id":"100","comment_id":"101"})
            } else {
                json!({"issue_id":"100","page_size":25})
            },
            selection: Selection {
                fields: fields.iter().map(|s| (*s).into()).collect(),
                limit: 1,
                cursor: cursor.map(String::from),
                max_bytes: 262_144,
            },
            preconditions: None,
        },
        "comment-read",
        IssueReadPolicy {
            target: target(),
            grant: setup.grant.clone(),
            expected_actor: "actor-one".into(),
        },
    )
}
#[test]
fn native_comment_pages_preserve_privacy_buffer_remainders_and_bind_projection() -> TestResult {
    let setup = Setup::new()?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(
            json!({"comments":[comment("101"),comment("102")],"startAt":0,"maxResults":2,"total":3}),
        )?,
        identity(),
        identity(),
        reply(json!({"comments":[comment("103")],"startAt":2,"maxResults":2,"total":3}))?,
        identity(),
        reply(comment("101"))?,
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
    let run = |host: &PreparedCommentRead| {
        Ok::<_, CraftError>(
            host.execute(
                ReadEnvironment {
                    store: &setup.store,
                    client: fixture.client(),
                    locked: &locked,
                    tls: TlsPolicy::AdditionalCaBundle(product.root()),
                    control: control()?,
                },
                || Ok(()),
            )
            .observed(),
        )
    };
    let first = run(&prepare(&setup, native::LIST, &[], None)?)?.envelope;
    assert!(first.ok, "{:?}", first.error);
    assert_eq!(data(&first)?["items"][0]["id"], "101");
    assert_eq!(data(&first)?["items"][0]["jsd_public"], false);
    assert_eq!(data(&first)?["items"][0]["body_state"], "not_requested");
    assert!(data(&first)?["items"][0].get("body").is_none());
    let changed = run(&prepare(
        &setup,
        native::LIST,
        &["body"],
        Some(cursor(&first)?),
    )?)?
    .envelope;
    assert!(!changed.ok);
    let second = run(&prepare(&setup, native::LIST, &[], Some(cursor(&first)?))?)?.envelope;
    assert!(second.ok, "{:?}", second.error);
    assert_eq!(data(&second)?["items"][0]["id"], "102");
    assert_eq!(data(&second)?["cached"], true);
    let third = run(&prepare(&setup, native::LIST, &[], Some(cursor(&second)?))?)?.envelope;
    assert!(third.ok, "{:?}", third.error);
    assert_eq!(data(&third)?["items"][0]["id"], "103");
    assert!(!third.page.as_ref().ok_or("page")?.has_more);
    let get = run(&prepare(&setup, native::GET, &["body"], None)?)?.envelope;
    assert!(get.ok, "{:?}", get.error);
    assert_eq!(data(&get)?["comment"]["body_state"], "returned");
    assert_eq!(data(&get)?["comment"]["body"]["type"], "doc");
    let registry = craft_cli::catalog::registry()?;
    for (envelope, command, fields) in [
        (&first, native::LIST, vec![]),
        (&get, native::GET, vec!["body".into()]),
    ] {
        let descriptor = registry.descriptor(command)?;
        let schema = registry
            .projection(command, &fields)?
            .ok_or("projection")?
            .response_schema(&descriptor.response_schema)?;
        assert!(jsonschema::validator_for(&schema)?.is_valid(&json!(envelope)));
    }
    let requests = product.finish()?;
    assert_eq!(requests.len(), 8);
    let pages: Vec<_> = requests
        .iter()
        .filter(|r| r.target.contains("/comment?"))
        .collect();
    assert_eq!(pages.len(), 2);
    assert!(pages[1].target.contains("startAt=2"));
    assert!(!serde_json::to_string(&get)?.contains("private-sentinel"));
    issuer.finish()?;
    drop(locked);
    setup.no_plaintext_credentials()?;
    Ok(())
}
#[test]
fn comments_require_explicit_allowance_and_recheck_captured_policy_before_auth() -> TestResult {
    let setup = Setup::new()?;
    let c = control()?;
    let options = InvocationOptions {
        state_root: Some(&setup.root),
        profile: Some("factory"),
        request_id: "comment-policy",
        max_bytes: 262_144,
    };
    let args = || {
        CommentReadArgs::List(CommentListArgs {
            issue_id: "100".into(),
            page_size: 25,
            limit: 20,
            cursor: None,
            cloud_id: None,
        })
    };
    setup.store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![IssueReadOperation::Get],
            enabled: true,
            expected_revision: None,
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    assert!(PreparedRead::prepare(args(), &[], &options, &setup.store, &c).is_err());
    setup.store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![IssueReadOperation::CommentList],
            enabled: true,
            expected_revision: Some(1),
        },
        true,
        &c,
        |_| Ok(()),
    )?;
    let prepared = PreparedRead::prepare(args(), &[], &options, &setup.store, &c)?;
    setup.store.set_issue_read_policy(
        "factory",
        IssueReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations: vec![IssueReadOperation::CommentList],
            enabled: false,
            expected_revision: Some(2),
        },
        true,
        &c,
        |_| Ok(()),
    )?;
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
            control: c,
        })
        .observed();
    assert!(!result.envelope.ok);
    assert!(product.finish()?.is_empty());
    assert!(issuer.finish()?.is_empty());
    Ok(())
}

#[test]
fn missing_comment_scope_prevents_resource_get_and_late_revocation_suppresses_body() -> TestResult {
    for late in [false, true] {
        let setup = Setup::new()?;
        setup.store.set_issue_read_policy(
            "factory",
            IssueReadPolicyUpdate {
                target: target(),
                expected_actor: "actor-one".into(),
                operations: vec![IssueReadOperation::CommentList],
                enabled: true,
                expected_revision: None,
            },
            true,
            &control()?,
            |_| Ok(()),
        )?;
        let mut replies = vec![identity(), identity()];
        if late {
            replies.push(reply(
                json!({"comments":[comment("101")],"startAt":0,"maxResults":25,"total":1}),
            )?);
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let issuer = issuer(if late {
            "read:jira-user read:jira-work"
        } else {
            "read:jira-user"
        })?;
        let clock = Arc::new(IssuePolicyClock {
            base: ManualClock::new(1000),
            store: Mutex::new(ProfileStore::open(&setup.root, &control()?)?),
            requests: product.request_counter(),
            threshold: if late { 3 } else { usize::MAX },
            operation: IssueReadOperation::CommentList,
            mode: "remove_operation",
            changed: AtomicBool::new(false),
        });
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let c = control()?;
        let locked = fixture.lock(&setup.grant, &c)?;
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
            request_id: "comment-late-policy",
            max_bytes: 262_144,
        };
        let prepared = PreparedRead::prepare(
            CommentReadArgs::List(CommentListArgs {
                issue_id: "100".into(),
                page_size: 25,
                limit: 20,
                cursor: None,
                cloud_id: None,
            }),
            &["body".into()],
            &options,
            &setup.store,
            &c,
        )?;
        let result = prepared
            .execute(ReadEnvironment {
                store: &setup.store,
                client: fixture.client(),
                locked: &locked,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: c,
            })
            .observed();
        assert!(!result.envelope.ok);
        assert!(
            data(&result.envelope)?["items"]
                .as_array()
                .ok_or("items")?
                .is_empty()
        );
        assert!(!data(&result.envelope)?["credential_cache_update"].is_null());
        assert_eq!(clock.changed.load(Ordering::SeqCst), late);
        let requests = product.finish()?;
        assert_eq!(requests.len(), if late { 3 } else { 2 });
        assert!(requests.iter().all(|r| r.method == "GET"));
        issuer.finish()?;
        drop(locked);
        setup.no_plaintext_credentials()?;
    }
    Ok(())
}

#[test]
fn every_comment_read_refuses_without_work_scope_and_stops_at_its_deadline() -> TestResult {
    for command in [native::GET, native::LIST] {
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
            let envelope = prepare(&setup, command, &[], None)?
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
