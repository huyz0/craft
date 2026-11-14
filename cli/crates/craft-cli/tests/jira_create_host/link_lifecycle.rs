use super::*;
use craft_cli::issue_links::recovery::{PreparedRecovery, ReconcileArgs};
use craft_cli::issue_links::{LinkEnvironment, PreparedLink};
use craft_provider_jira::issue_links::write::{self, LinkDeleteInput, LinkInput};
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Mutation, IssueMutationPolicyUpdate,
};
use craft_runtime::issue_recovery_policy::{
    IssueRecoveryOperation as Recovery, IssueRecoveryPolicyUpdate,
};

const LINK_OPERATION: &str = "exact-dependency";

fn expected() -> Result<LinkInput, serde_json::Error> {
    serde_json::from_value(
        json!({"inward_issue_id":"12000","outward_issue_id":"11000","inward_project_id":"20000","link_type":{"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks"}}),
    )
}
fn link(id: &str) -> Value {
    json!({"id":id,"inwardIssue":{"id":"12000"},"outwardIssue":{"id":"11000"},"type":{"id":"10000","name":"Blocks","inward":"is blocked by","outward":"blocks"}})
}
fn reply(value: Value) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(200, &serde_json::to_vec(&value)?, &[]))
}
fn configure(setup: &Setup) -> TestResult {
    let control = control(CancellationToken::default(), 30_000)?;
    for project in ["10000", "20000"] {
        let target = ResourceRef {
            id: project.into(),
            ..target()
        };
        setup.profiles.set_issue_mutation_policy(
            "factory",
            IssueMutationPolicyUpdate {
                target: target.clone(),
                expected_actor: "actor-one".into(),
                enabled: true,
                operations: vec![Mutation::Link, Mutation::LinkDelete],
                expected_revision: None,
            },
            true,
            &control,
            |_| Ok(()),
        )?;
        setup.profiles.set_issue_recovery_policy(
            "factory",
            IssueRecoveryPolicyUpdate {
                target,
                expected_actor: "actor-one".into(),
                enabled: true,
                operations: vec![Recovery::LinkCreate, Recovery::LinkDelete],
                expected_revision: None,
            },
            true,
            &control,
            |_| Ok(()),
        )?;
    }
    Ok(())
}
fn deletion_reads(mode: &str) -> Result<Vec<Reply>, serde_json::Error> {
    let types = json!({"issueLinkTypes":[link("90000")["type"].clone()]});
    let outward = json!({"id":"11000","fields":{"project":{"id":"10000"}}});
    let inward = json!({"id":"12000","fields":{"project":{"id":"20000"}}});
    let permissions = |allowed| json!({"permissions":{"BROWSE_PROJECTS":{"key":"BROWSE_PROJECTS","havePermission":true},"LINK_ISSUES":{"key":"LINK_ISSUES","havePermission":allowed}}});
    let mut rows = vec![
        types.clone(),
        link("90000"),
        outward.clone(),
        inward.clone(),
        permissions(!matches!(mode, "inward_only" | "permission")),
        permissions(!matches!(mode, "outward_only" | "permission")),
        outward,
        inward,
        link("90000"),
        types,
    ];
    if mode == "changed_link" {
        rows[8]["id"] = json!("90001");
        rows.truncate(9);
    } else if mode == "permission" {
        rows.truncate(6);
    }
    rows.into_iter().map(reply).collect()
}
fn deletion_case(mode: &str) -> TestResult {
    let setup = Setup::new()?;
    configure(&setup)?;
    let issuer = issuer(SCOPES)?;
    let mut replies = vec![identity(), identity()];
    replies.extend(deletion_reads(mode)?);
    let sent = !matches!(mode, "permission" | "changed_link");
    if sent {
        replies.push(match mode {
            "lost" => Reply::Disconnect,
            "refused" => Reply::json(404, b"{}", &[]),
            "unqualified" => Reply::json(201, b"", &[]),
            _ => Reply::json(204, b"", &[]),
        });
    }
    let count = replies.len();
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
    let prepare = || {
        PreparedLink::prepare_delete(
            LinkDeleteInput {
                link_id: "90000".into(),
                expected: expected()?,
            },
            target(),
            "factory",
            LINK_OPERATION,
            true,
            &setup.profiles,
            &control,
        )
        .map_err(Into::<Box<dyn std::error::Error>>::into)
    };
    let execute = |prepared: PreparedLink| {
        prepared.execute(
            LinkEnvironment {
                store: &setup.profiles,
                client: fixture.client(),
                locked: &locked,
                journal: &setup.journal,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control.clone(),
            },
            || Ok(()),
        )
    };
    let (data, error) = execute(prepare()?);
    let confirmed = matches!(mode, "confirmed" | "outward_only" | "inward_only");
    assert_eq!(error.is_none(), confirmed, "{mode}: {error:?}");
    assert_eq!(data.relation_deleted, confirmed);
    assert!(!data.relation_confirmed);
    assert_eq!(data.link_id.as_deref(), Some("90000"));
    assert!(!data.replay_authorized);
    assert_eq!(
        data.receipt.request_state,
        if confirmed {
            craft_core::contracts::RequestState::Confirmed
        } else if mode == "refused" {
            craft_core::contracts::RequestState::Failed
        } else if sent {
            craft_core::contracts::RequestState::SentUnknown
        } else {
            craft_core::contracts::RequestState::NotSent
        }
    );
    if sent {
        let saved = setup.journal.get(LINK_OPERATION)?;
        assert!(execute(prepare()?).1.is_some());
        assert_eq!(setup.journal.get(LINK_OPERATION)?, saved);
        assert_eq!(product.request_count(), count);
    } else {
        assert!(setup.journal.get(LINK_OPERATION)?.is_none());
    }
    let requests = product.finish()?;
    assert_eq!(requests.len(), count, "{mode}");
    let deletes: Vec<_> = requests.iter().filter(|r| r.method == "DELETE").collect();
    assert_eq!(deletes.len(), usize::from(sent));
    if sent {
        assert!(deletes[0].target.ends_with("/rest/api/3/issueLink/90000"));
        assert!(deletes[0].body.is_empty());
    }
    issuer.finish()?;
    drop(locked);
    drop(fixture);
    setup.assert_no_plaintext()?;
    setup.cleanup()
}
macro_rules! deletion {
    ($name:ident, $mode:literal) => {
        #[test]
        fn $name() -> TestResult {
            deletion_case($mode)
        }
    };
}
deletion!(deletion_confirms_exact_native_id_once, "confirmed");
deletion!(deletion_lost_response_never_replays, "lost");
deletion!(deletion_refused_response_never_replays, "refused");
deletion!(
    deletion_unqualified_response_retains_uncertainty,
    "unqualified"
);
deletion!(
    deletion_refuses_replacement_link_id_before_claim,
    "changed_link"
);
deletion!(
    deletion_requires_native_link_permission_on_an_endpoint,
    "permission"
);
deletion!(
    deletion_accepts_only_outward_native_link_permission,
    "outward_only"
);
deletion!(
    deletion_accepts_only_inward_native_link_permission,
    "inward_only"
);

fn recovery_case(mode: &str) -> TestResult {
    let deletion = mode.starts_with("delete_");
    let mode = mode.strip_prefix("delete_").unwrap_or(mode);
    let setup = Setup::new()?;
    configure(&setup)?;
    let issuer = issuer(SCOPES)?;
    let mut replies = vec![identity(), identity()];
    // Creation has no native link identity. Investigation discovers candidates from the original outward issue.
    let reads = deletion_reads("confirmed")?;
    if deletion {
        replies.extend(reads);
    } else {
        for index in [0, 2, 3, 4, 5, 6, 7, 9] {
            replies.push(reads[index].clone());
        }
    }
    replies.push(Reply::Disconnect);
    if !matches!(mode, "revoked" | "mismatch" | "excluded") {
        replies.push(identity());
        let links = if mode == "not_observed" {
            vec![]
        } else {
            vec![link("90000")]
        };
        replies.push(reply(
            json!({"id":"11000","fields":{"project":{"id":"10000"},"issuelinks":links}}),
        )?);
        replies.push(reply(
            json!({"id":"12000","fields":{"project":{"id":"20000"}}}),
        )?);
        if mode != "not_observed" {
            replies.push(match mode {
                "absent" => Reply::json(404, b"{}", &[]),
                "rate" => Reply::json(429, b"{}", &[("Retry-After", "60")]),
                "changed" => {
                    let mut row = link("90000");
                    row["type"]["outward"] = json!("changed");
                    reply(row)?
                }
                _ => reply(link("90000"))?,
            });
        }
    }
    let count = replies.len();
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
    let create = if deletion {
        PreparedLink::prepare_delete(
            LinkDeleteInput {
                link_id: "90000".into(),
                expected: expected()?,
            },
            target(),
            "factory",
            LINK_OPERATION,
            true,
            &setup.profiles,
            &control,
        )?
    } else {
        PreparedLink::prepare(
            expected()?,
            target(),
            "factory",
            LINK_OPERATION,
            true,
            &setup.profiles,
            &control,
        )?
    };
    let environment = || LinkEnvironment {
        store: &setup.profiles,
        client: fixture.client(),
        locked: &locked,
        journal: &setup.journal,
        tls: TlsPolicy::AdditionalCaBundle(product.root()),
        control: control.clone(),
    };
    let (lost, error) = create.execute(environment(), || Ok(()));
    assert!(error.is_some());
    assert_eq!(
        lost.receipt.request_state,
        craft_core::contracts::RequestState::SentUnknown
    );
    let saved = setup.journal.get(LINK_OPERATION)?;
    let mut original = CanonicalRequest {
        command: if deletion {
            write::DELETE
        } else {
            write::CREATE
        }
        .into(),
        target: target(),
        input: if deletion {
            json!({"link_id":"90000","expected":expected()?})
        } else {
            json!(expected()?)
        },
        selection: Selection::default(),
        preconditions: None,
    };
    if mode == "mismatch" {
        if deletion {
            original.input["link_id"] = json!("90001");
        } else {
            original.input["inward_issue_id"] = json!("13000");
        }
    }
    if mode == "excluded" {
        setup.profiles.set_issue_recovery_policy(
            "factory",
            IssueRecoveryPolicyUpdate {
                target: target(),
                expected_actor: "actor-one".into(),
                enabled: true,
                operations: vec![Recovery::LinkCreate],
                expected_revision: Some(1),
            },
            true,
            &control,
            |_| Ok(()),
        )?;
    }
    let host = PreparedRecovery::prepare(
        &ReconcileArgs {
            operation_id: LINK_OPERATION.into(),
            original: Some(original),
        },
        "factory",
        &setup.profiles,
        &control,
    );
    if mode == "excluded" {
        assert!(host.is_err());
        assert_eq!(setup.journal.get(LINK_OPERATION)?, saved);
        assert_eq!(product.finish()?.len(), count);
        issuer.finish()?;
        drop(locked);
        drop(fixture);
        setup.assert_no_plaintext()?;
        return setup.cleanup();
    }
    let host = host?;
    if mode == "revoked" {
        setup.profiles.set_issue_recovery_policy(
            "factory",
            IssueRecoveryPolicyUpdate {
                target: target(),
                expected_actor: "actor-one".into(),
                enabled: false,
                operations: vec![Recovery::LinkCreate, Recovery::LinkDelete],
                expected_revision: Some(1),
            },
            true,
            &control,
            |_| Ok(()),
        )?;
    }
    let (data, error) = host.execute(environment(), || Ok(()));
    assert_eq!(
        error.is_some(),
        matches!(mode, "rate" | "revoked" | "mismatch"),
        "{mode}: {error:?}"
    );
    assert!(!data.attribution_upgraded);
    assert!(!data.journal_updated);
    assert!(!data.replay_authorized);
    assert!(!data.atomic_snapshot);
    assert_eq!(setup.journal.get(LINK_OPERATION)?, saved);
    let current = serde_json::to_value(data.current_relation)?;
    assert_eq!(
        current,
        json!(match mode {
            "observed" => "observed",
            "not_observed" => "not_observed",
            "absent" => "absent_or_invisible",
            "changed" => "changed",
            _ => "uninspected",
        })
    );
    assert_eq!(data.observations_complete, error.is_none());
    let requests = product.finish()?;
    assert_eq!(requests.len(), count, "{mode}");
    assert_eq!(
        requests.iter().filter(|r| r.method == "POST").count(),
        usize::from(!deletion)
    );
    assert_eq!(
        requests.iter().filter(|r| r.method == "DELETE").count(),
        usize::from(deletion)
    );
    issuer.finish()?;
    drop(locked);
    drop(fixture);
    setup.assert_no_plaintext()?;
    setup.cleanup()
}
macro_rules! recovery {
    ($name:ident, $mode:literal) => {
        #[test]
        fn $name() -> TestResult {
            recovery_case($mode)
        }
    };
}
recovery!(
    recovery_observation_does_not_attribute_or_replay_lost_creation,
    "observed"
);
recovery!(
    recovery_no_candidate_does_not_prove_creation_failure,
    "not_observed"
);
recovery!(
    recovery_candidate_not_visible_does_not_authorize_replay,
    "absent"
);
recovery!(
    recovery_changed_type_does_not_promote_original_confirmation,
    "changed"
);
recovery!(recovery_rate_limit_retains_original_unknown_journal, "rate");
recovery!(recovery_revocation_refuses_before_identity_read, "revoked");
recovery!(
    recovery_original_binding_mismatch_refuses_before_identity_read,
    "mismatch"
);
recovery!(
    deletion_recovery_visible_link_does_not_prove_original_delete_failure,
    "delete_observed"
);
recovery!(
    deletion_recovery_invisible_link_does_not_prove_original_delete_success,
    "delete_absent"
);
recovery!(
    deletion_recovery_replacement_id_refuses_before_identity_read,
    "delete_mismatch"
);
recovery!(
    deletion_recovery_creation_allowance_does_not_grant_deletion_investigation,
    "delete_excluded"
);
