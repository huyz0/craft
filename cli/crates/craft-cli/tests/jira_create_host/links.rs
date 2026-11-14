use super::*;
use craft_cli::issue_links::LinkEnvironment;
use craft_cli::issue_links::command::{LinkArgs, prepare};
use craft_core::contracts::RequestState;
use craft_provider_jira::issue_links::write::CREATE_ROUTE;
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Op, IssueMutationPolicyUpdate,
};
const LINK_OPERATION: &str = "dependency-link";
fn args() -> Result<LinkArgs, serde_json::Error> {
    serde_json::from_value(
        json!({"operation_id":LINK_OPERATION,"apply":true,"project_id":"10000","inward_project_id":"20000","inward_issue_id":"12000","outward_issue_id":"11000","link_type":{"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks"}}),
    )
}
fn configure(setup: &Setup, both: bool) -> TestResult {
    for id in if both {
        &["10000", "20000"][..]
    } else {
        &["10000"][..]
    } {
        setup.profiles.set_issue_mutation_policy(
            "factory",
            IssueMutationPolicyUpdate {
                target: ResourceRef {
                    id: (*id).into(),
                    ..target()
                },
                expected_actor: "actor-one".into(),
                enabled: true,
                operations: vec![Op::Link],
                expected_revision: None,
            },
            true,
            &control(CancellationToken::default(), 30_000)?,
            |_| Ok(()),
        )?;
    }
    Ok(())
}
fn disable(store: &ProfileStore) -> Result<(), CraftError> {
    store.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            enabled: false,
            operations: vec![Op::Link],
            expected_revision: Some(1),
        },
        true,
        &control(CancellationToken::default(), 30_000)?,
        |_| Ok(()),
    )?;
    Ok(())
}
struct ClaimClock {
    base: ManualClock,
    journal: NativeJournal,
    store: Mutex<ProfileStore>,
    mode: String,
    changed: AtomicBool,
    cancel: CancellationToken,
}
impl Clock for ClaimClock {
    fn unix_millis(&self) -> u64 {
        let claimed = self.mode.starts_with("claim_")
            && self.journal.get(LINK_OPERATION).is_ok_and(|record| {
                record.is_some_and(|r| {
                    r.steps
                        .get(CREATE_ROUTE)
                        .is_some_and(|s| s.state == SubmissionState::SendStarted)
                })
            });
        if claimed && !self.changed.swap(true, Ordering::SeqCst) {
            match self.mode.as_str() {
                "claim_expiry" => {
                    let _ = self.base.advance(Duration::from_secs(31));
                }
                "claim_cancel" => self.cancel.cancel(),
                _ => {
                    if let Ok(store) = self.store.lock() {
                        let _ = disable(&store);
                    }
                }
            }
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}
fn metadata(mode: &str) -> Result<Vec<Reply>, serde_json::Error> {
    let types = json!({"issueLinkTypes":[{"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks"}]});
    let outward = json!({"id":"11000","fields":{"project":{"id":"10000"}}});
    let inward = json!({"id":"12000","fields":{"project":{"id":"20000"}}});
    let permissions = json!({"permissions":{"BROWSE_PROJECTS":{"key":"BROWSE_PROJECTS","havePermission":true},"LINK_ISSUES":{"key":"LINK_ISSUES","havePermission":true}}});
    let mut rows = vec![
        types.clone(),
        outward.clone(),
        inward.clone(),
        permissions.clone(),
        permissions,
        outward,
        inward,
        types,
    ];
    let truncate = match mode {
        "permission" => {
            rows[3]["permissions"]["LINK_ISSUES"]["havePermission"] = json!(false);
            4
        }
        "project_drift" => {
            rows[6]["fields"]["project"]["id"] = json!("99999");
            7
        }
        "type_drift" => {
            rows[7]["issueLinkTypes"][0]["outward"] = json!("relates to");
            8
        }
        _ => 8,
    };
    rows.truncate(truncate);
    rows.into_iter()
        .map(|row| Ok(Reply::json(200, &serde_json::to_vec(&row)?, &[])))
        .collect()
}
fn run_case(mode: &str, boundary: Option<usize>) -> TestResult {
    let setup = Setup::new()?;
    configure(&setup, true)?;
    let scopes = if mode == "scope" {
        "read:jira-user read:jira-work"
    } else {
        "read:jira-user read:jira-work write:jira-work"
    };
    let issuer = issuer(scopes)?;
    let mut replies = vec![identity()];
    if mode != "before_auth" {
        replies.push(if mode == "wrong_actor" {
            Reply::json(200, br#"{"accountId":"other-actor","active":true}"#, &[])
        } else {
            identity()
        });
        if !matches!(mode, "scope" | "wrong_actor") {
            let mut reads = metadata(mode)?;
            if let Some(boundary) = boundary {
                reads.truncate(boundary);
            }
            replies.extend(reads);
            if matches!(mode, "confirmed" | "lost" | "refused" | "unqualified") {
                replies.push(match mode {
                    "lost" => Reply::Disconnect,
                    "refused" => Reply::json(400, b"{}", &[]),
                    "unqualified" => Reply::json(204, b"", &[]),
                    _ => Reply::json(201, b"", &[]),
                });
            }
        }
    }
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let cancel = CancellationToken::default();
    let base = ManualClock::new(1000);
    let clock = Arc::new(ClaimClock {
        base: base.clone(),
        journal: setup.journal.clone(),
        store: Mutex::new(ProfileStore::open(
            &setup.files.root,
            &control(CancellationToken::default(), 30_000)?,
        )?),
        mode: mode.into(),
        changed: AtomicBool::new(false),
        cancel: cancel.clone(),
    });
    let fixture = setup.fixture(&issuer, &product, clock.clone())?;
    let invocation = control(cancel, 30_000)?;
    let locked = fixture.lock(&setup.grant, &invocation)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    base.advance(Duration::from_secs(3600))?;
    let prepared = prepare(&args()?, "factory", &setup.profiles, &invocation)?;
    if mode == "before_auth" {
        disable(&setup.profiles)?;
    }
    let changed = AtomicBool::new(false);
    let calls = product.request_counter();
    let (data, error) = prepared.execute(
        LinkEnvironment {
            store: &setup.profiles,
            client: fixture.client(),
            locked: &locked,
            journal: &setup.journal,
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: invocation.clone(),
        },
        || {
            if let Some(boundary) = boundary
                && calls.load(Ordering::SeqCst) >= boundary + 2
                && !changed.swap(true, Ordering::SeqCst)
            {
                disable(&setup.profiles)?;
            }
            Ok(())
        },
    );
    let claimed = mode.starts_with("claim_");
    let sent = matches!(mode, "confirmed" | "lost" | "refused" | "unqualified");
    assert_eq!(error.is_none(), mode == "confirmed", "{mode}: {error:?}");
    assert_eq!(data.relation_confirmed, mode == "confirmed", "{mode}");
    assert!(data.link_id.is_none());
    assert!(data.new_link_created.is_none());
    assert!(!data.replay_authorized);
    let expected = if mode == "confirmed" {
        RequestState::Confirmed
    } else if mode == "refused" {
        RequestState::Failed
    } else if sent || claimed {
        RequestState::SentUnknown
    } else {
        RequestState::NotSent
    };
    assert_eq!(data.receipt.request_state, expected, "{mode}: {error:?}");
    assert_eq!(
        data.credential_cache_update.is_some(),
        !matches!(mode, "before_auth" | "wrong_actor"),
        "{mode}"
    );
    if let Some(error) = &error {
        assert_eq!(error.details.write_state, expected);
    }
    let stored = setup.journal.get(LINK_OPERATION)?;
    assert_eq!(stored.is_some(), sent || claimed, "{mode}");
    if let Some(saved) = &stored {
        assert_eq!(request_state(saved.steps[CREATE_ROUTE].state), expected);
    }
    if sent {
        let again = prepare(&args()?, "factory", &setup.profiles, &invocation)?;
        let (again, error) = again.execute(
            LinkEnvironment {
                store: &setup.profiles,
                client: fixture.client(),
                locked: &locked,
                journal: &setup.journal,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: invocation,
            },
            || Ok(()),
        );
        assert_eq!(error.map(|e| e.code), Some(ErrorCode::OperationIdReused));
        assert!(again.credential_cache_update.is_none());
        assert!(again.prior_record.is_some());
    }
    let requests = product.finish()?;
    assert_eq!(requests.len(), count, "{mode}");
    assert_eq!(
        requests.iter().filter(|r| r.method == "POST").count(),
        usize::from(sent)
    );
    if sent {
        let request = requests.last().ok_or("POST")?;
        assert!(request.target.ends_with("/rest/api/3/issueLink"));
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body)?,
            json!({"type":{"id":"10000"},"inwardIssue":{"id":"12000"},"outwardIssue":{"id":"11000"}})
        );
    }
    assert_eq!(
        issuer.finish()?.len(),
        if mode == "before_auth" { 1 } else { 2 }
    );
    if boundary.is_some() {
        assert!(changed.load(Ordering::SeqCst));
    }
    if claimed {
        assert!(clock.changed.load(Ordering::SeqCst));
    }
    drop(locked);
    drop(fixture);
    drop(clock);
    setup.assert_no_plaintext()?;
    setup.cleanup()?;
    Ok(())
}
fn request_state(state: SubmissionState) -> RequestState {
    match state {
        SubmissionState::Prepared => RequestState::NotSent,
        SubmissionState::Confirmed => RequestState::Confirmed,
        SubmissionState::Failed => RequestState::Failed,
        _ => RequestState::SentUnknown,
    }
}
macro_rules! case {
    ($name:ident,$mode:literal) => {
        #[test]
        fn $name() -> TestResult {
            run_case($mode, None)
        }
    };
}
case!(
    confirmed_link_retains_exact_endpoints_without_creation_attribution,
    "confirmed"
);
case!(
    lost_link_response_keeps_unknown_claim_and_never_replays,
    "lost"
);
case!(
    documented_link_refusal_keeps_failed_journal_without_replay,
    "refused"
);
case!(
    unqualified_link_acknowledgment_remains_unknown,
    "unqualified"
);
case!(link_scope_denial_precedes_resource_reads, "scope");
case!(
    link_actor_mismatch_does_not_commit_credential_cache,
    "wrong_actor"
);
case!(
    revoked_link_policy_stops_before_authentication,
    "before_auth"
);
case!(missing_outward_link_permission_prevents_claim, "permission");
case!(link_endpoint_project_drift_prevents_claim, "project_drift");
case!(native_link_label_drift_prevents_claim, "type_drift");
case!(
    link_claim_expiry_preserves_uncertainty_without_post,
    "claim_expiry"
);
case!(
    link_claim_revocation_preserves_uncertainty_without_post,
    "claim_revoke"
);
case!(
    link_claim_cancellation_preserves_uncertainty_without_post,
    "claim_cancel"
);
macro_rules! boundary {
    ($name:ident,$count:literal) => {
        #[test]
        fn $name() -> TestResult {
            run_case("boundary", Some($count))
        }
    };
}
boundary!(revoked_link_policy_after_first_type_read, 1);
boundary!(revoked_link_policy_after_outward_issue_read, 2);
boundary!(revoked_link_policy_after_inward_issue_read, 3);
boundary!(revoked_link_policy_after_outward_permission_read, 4);
boundary!(revoked_link_policy_after_inward_permission_read, 5);
boundary!(revoked_link_policy_after_outward_recheck, 6);
boundary!(revoked_link_policy_after_inward_recheck, 7);
boundary!(revoked_link_policy_after_type_recheck, 8);
#[test]
fn cross_project_link_requires_both_explicit_allowances() -> TestResult {
    let setup = Setup::new()?;
    configure(&setup, false)?;
    assert!(
        prepare(
            &args()?,
            "factory",
            &setup.profiles,
            &control(CancellationToken::default(), 30_000)?
        )
        .is_err()
    );
    assert!(setup.journal.get(LINK_OPERATION)?.is_none());
    Ok(())
}
