use super::*;
use craft_cli::issue_links::reads::ReadEnvironment;
use craft_cli::issue_links::reads::command::{LinkListArgs, LinkReadArgs, PreparedRead};
use craft_runtime::issue_read_policy::{IssueReadOperation as Op, IssueReadPolicyUpdate};

fn link(id: &str) -> Value {
    json!({"id":id,"inwardIssue":{"id":"12000"},"type":{"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks"}})
}
fn policy(setup: &Setup, enabled: bool, revision: Option<u64>) -> Result<(), CraftError> {
    setup
        .profiles
        .set_issue_read_policy(
            "factory",
            IssueReadPolicyUpdate {
                target: ResourceRef {
                    resource_type: "site".into(),
                    id: target().tenant.clone(),
                    ..target()
                },
                expected_actor: "actor-one".into(),
                enabled,
                operations: vec![Op::LinkGet, Op::LinkList, Op::LinkTypes],
                expected_revision: revision,
            },
            true,
            &control(CancellationToken::default(), 30_000)?,
            |_| Ok(()),
        )
        .map(|_| ())
}
fn paging_case(revoke: bool) -> TestResult {
    let setup = Setup::new()?;
    policy(&setup, true, None)?;
    let issuer = issuer(SCOPES)?;
    let mut replies = vec![
        identity(),
        identity(),
        Reply::json(
            200,
            &serde_json::to_vec(
                &json!({"id":"11000","fields":{"issuelinks":[link("90000"),link("90001")]}}),
            )?,
            &[],
        ),
    ];
    if !revoke {
        replies.extend([identity(), identity()]);
    }
    let expected_requests = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let control = control(CancellationToken::default(), 30_000)?;
    let locked = fixture.lock(&setup.grant, &control)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let options = craft_cli::profiles::InvocationOptions {
        state_root: Some(&setup.files.root),
        profile: Some("factory"),
        request_id: "link-page",
        max_bytes: 262_144,
    };
    let fields = vec!["id".into()];
    let prepare = |cursor: Option<String>, issue: &str| {
        PreparedRead::prepare(
            LinkReadArgs::List(LinkListArgs {
                issue_id: issue.into(),
                cloud_id: None,
                limit: 1,
                cursor,
            }),
            &fields,
            &options,
            &setup.profiles,
            &control,
        )
    };
    let environment = || ReadEnvironment {
        store: &setup.profiles,
        client: fixture.client(),
        locked: &locked,
        tls: TlsPolicy::AdditionalCaBundle(product.root()),
        control: control.clone(),
    };
    let first = prepare(None, "11000")?.execute(environment()).envelope;
    assert!(first.ok, "{first:?}");
    let first_data = first.data.as_ref().ok_or("first data")?;
    assert_eq!(first_data["items"][0]["id"], "90000");
    assert_eq!(first_data["items"][0]["inward_issue_id"], "12000");
    assert!(first_data["items"][0]["link_type"].get("name").is_none());
    let cursor = first
        .page
        .as_ref()
        .and_then(|p| p.next_cursor.clone())
        .ok_or("cursor")?;
    assert!(cursor.starts_with("cur_"));
    let next = prepare(Some(cursor.clone()), "11000")?;
    if revoke {
        policy(&setup, false, Some(1))?;
        let denied = next.execute(environment()).envelope;
        assert!(!denied.ok);
        assert_eq!(
            denied.error.as_ref().map(|e| e.code),
            Some(ErrorCode::PolicyDenied)
        );
    } else {
        // Reopen state while the synthetic credential lock remains owned by this invocation.
        let reopened = ProfileStore::open(&setup.files.root, &control)?;
        let resumed = next
            .execute(ReadEnvironment {
                store: &reopened,
                ..environment()
            })
            .envelope;
        assert!(resumed.ok, "{resumed:?}");
        assert_eq!(
            resumed.data.as_ref().ok_or("resumed data")?["items"][0]["id"],
            "90001"
        );
        assert!(
            resumed
                .page
                .as_ref()
                .is_some_and(|p| p.next_cursor.is_none())
        );
        let replay = prepare(Some(cursor.clone()), "11000")?
            .execute(environment())
            .envelope;
        assert!(replay.ok);
        assert_eq!(
            replay.data.as_ref().ok_or("replay data")?["items"],
            resumed.data.as_ref().ok_or("resumed data")?["items"]
        );
        let mismatch = prepare(Some(cursor), "12000")?
            .execute(environment())
            .envelope;
        assert!(!mismatch.ok);
    }
    let requests = product.finish()?;
    assert_eq!(requests.len(), expected_requests);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.target.contains("/rest/api/3/issue/"))
            .count(),
        1
    );
    assert!(requests.iter().all(|r| r.method == "GET"));
    issuer.finish()?;
    drop(locked);
    drop(fixture);
    setup.assert_no_plaintext()?;
    setup.cleanup()
}
#[test]
fn link_cursor_preserves_projected_remainder_across_reopen_without_refetch() -> TestResult {
    paging_case(false)
}
#[test]
fn link_cursor_rechecks_policy_before_serving_saved_remainder() -> TestResult {
    paging_case(true)
}

fn direct_case(types: bool) -> TestResult {
    use craft_cli::issue_links::reads::command::{LinkGetArgs, LinkTypesArgs};
    let setup = Setup::new()?;
    policy(&setup, true, None)?;
    let issuer = issuer(SCOPES)?;
    let mut row = link("90000");
    row["outwardIssue"] = json!({"id":"11000"});
    let body = if types {
        json!({"issueLinkTypes":[row["type"].clone()]})
    } else {
        row
    };
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        Reply::json(200, &serde_json::to_vec(&body)?, &[]),
    ]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let control = control(CancellationToken::default(), 30_000)?;
    let locked = fixture.lock(&setup.grant, &control)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let args = if types {
        LinkReadArgs::Types(LinkTypesArgs {
            limit: 20,
            cursor: None,
            cloud_id: None,
        })
    } else {
        LinkReadArgs::Get(LinkGetArgs {
            link_id: "90000".into(),
            cloud_id: None,
        })
    };
    let options = craft_cli::profiles::InvocationOptions {
        state_root: Some(&setup.files.root),
        profile: Some("factory"),
        request_id: "native-link",
        max_bytes: 262_144,
    };
    let envelope = PreparedRead::prepare(args, &[], &options, &setup.profiles, &control)?
        .execute(ReadEnvironment {
            store: &setup.profiles,
            client: fixture.client(),
            locked: &locked,
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: control.clone(),
        })
        .envelope;
    assert!(envelope.ok, "{envelope:?}");
    let data = envelope.data.ok_or("data")?;
    if types {
        assert_eq!(data["items"][0]["id"], "10000");
        assert_eq!(data["items"][0]["outward"], "blocks");
    } else {
        assert_eq!(data["link"]["id"], "90000");
        assert_eq!(data["link"]["inward_issue_id"], "12000");
        assert_eq!(data["link"]["outward_issue_id"], "11000");
    }
    let requests = product.finish()?;
    assert_eq!(requests.len(), 3);
    assert!(requests.last().ok_or("read")?.target.ends_with(if types {
        "/rest/api/3/issueLinkType"
    } else {
        "/rest/api/3/issueLink/90000"
    }));
    issuer.finish()?;
    drop(locked);
    drop(fixture);
    setup.assert_no_plaintext()?;
    setup.cleanup()
}
#[test]
fn native_link_get_retains_directed_endpoints_and_exact_identity() -> TestResult {
    direct_case(false)
}
#[test]
fn native_link_types_retains_current_labels() -> TestResult {
    direct_case(true)
}
