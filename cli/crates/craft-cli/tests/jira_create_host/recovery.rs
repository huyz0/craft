use super::*;
use craft_cli::issue_recovery::{self, PreparedRecovery, RecoveryInput, RecoveryPolicy};
fn token(scopes: &str) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(
        200,
        &serde_json::to_vec(
            &json!({"access_token":ACCESS_TOKEN,"expires_in":3600,"token_type":"Bearer","scope":scopes}),
        )?,
        &[],
    ))
}
fn observed(id: &str) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(
        200,
        &serde_json::to_vec(
            &json!({"id":id,"key":"ED-24","fields":{"project":{"id":"10000"},"issuetype":{"id":"10001"},"status":{"id":"102"},"updated":"2026-11-10T01:00:00.000+0000","assignee":null}}),
        )?,
        &[],
    ))
}
fn original() -> CanonicalRequest {
    CanonicalRequest {
        command: COMMAND.into(),
        target: target(),
        input: input(),
        selection: Selection {
            fields: vec!["issue_key".into()],
            ..Default::default()
        },
        preconditions: None,
    }
}

#[test]
fn typed_create_recovery_uses_separate_policy_and_retains_uncertainty_and_renewal() -> TestResult {
    use craft_cli::issue_recovery::command::{OperationReconcileArgs, PreparedInvocation};
    use craft_runtime::issue_recovery_policy::{
        IssueRecoveryOperation as Op, IssueRecoveryPolicyUpdate,
    };
    for mode in [
        "visible", "partial", "no_ids", "missing", "excluded", "revoked", "changed",
    ] {
        let setup = Setup::new()?;
        let issuer = TlsServer::start(Reply::Sequence(vec![
            token(SCOPES)?,
            token("read:jira-user read:jira-work")?,
        ]))?;
        let reads = matches!(mode, "visible" | "partial");
        let mut replies = vec![identity(), identity()];
        replies.extend(metadata()?);
        replies.push(Reply::Disconnect);
        if reads {
            replies.push(identity());
            replies.push(observed("10003")?);
            if mode == "partial" {
                replies.push(Reply::json(
                    429,
                    b"private-provider-text",
                    &[("Retry-After", "60")],
                ));
            }
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(ManualClock::new(1_000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let control = control(CancellationToken::default(), 30_000)?;
        let locked = fixture.lock(&setup.grant, &control)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let lost = host(&setup.grant, "actor-one")?.execute_cached(
            fixture.client(),
            &locked,
            &setup.journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control.clone(),
        );
        assert!(!lost.envelope.ok);
        let saved = setup.journal.get(OPERATION)?.ok_or("saved")?;
        setup.profiles.set_issue_recovery_policy(
            "factory",
            IssueRecoveryPolicyUpdate {
                target: target(),
                expected_actor: "actor-one".into(),
                enabled: true,
                expected_revision: None,
                operations: vec![if mode == "excluded" {
                    Op::Edit
                } else {
                    Op::Create
                }],
            },
            true,
            &control,
            |_| Ok(()),
        )?;
        let args = OperationReconcileArgs {
            saved: false,
            operation_id: if mode == "missing" {
                "absent-operation"
            } else {
                OPERATION
            }
            .into(),
            original: Some(original()),
            candidate_issue_ids: if mode == "partial" {
                vec!["10003".into(), "10004".into()]
            } else if mode == "no_ids" {
                vec![]
            } else {
                vec!["10003".into()]
            },
        };
        let options = craft_cli::profiles::InvocationOptions {
            state_root: Some(&setup.files.root),
            profile: Some("factory"),
            request_id: "candidate-recovery",
            max_bytes: 262_144,
        };
        let prepared = PreparedInvocation::prepare(
            args,
            &["assessment".into()],
            &options,
            &setup.profiles,
            &control,
        );
        if mode == "excluded" {
            assert!(prepared.is_err());
        } else {
            let prepared = prepared?;
            if matches!(mode, "revoked" | "changed") {
                setup.profiles.set_issue_recovery_policy(
                    "factory",
                    IssueRecoveryPolicyUpdate {
                        target: target(),
                        expected_actor: "actor-one".into(),
                        enabled: mode == "changed",
                        expected_revision: Some(1),
                        operations: vec![Op::Create],
                    },
                    true,
                    &control,
                    |_| Ok(()),
                )?;
            }
            clock.advance(Duration::from_secs(3_600))?;
            let result = prepared.execute_cached(
                &setup.profiles,
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control.clone(),
            );
            assert_eq!(
                result.envelope.ok,
                matches!(mode, "visible" | "no_ids" | "missing"),
                "{mode}: {:?}",
                result.envelope.error
            );
            assert!(
                jsonschema::validator_for(&issue_recovery::schema()?)?
                    .is_valid(&serde_json::to_value(&result.envelope)?)
            );
            let data = result.envelope.data.ok_or("data")?;
            assert_eq!(data["replay_authorized"], false);
            assert_eq!(data["attribution_upgraded"], false);
            assert_eq!(
                data["observations"].as_array().ok_or("observations")?.len(),
                usize::from(reads)
            );
            assert_eq!(data["credential_cache_update"].is_null(), !reads);
            if reads || mode == "no_ids" {
                assert_eq!(data["initial_record"], json!(saved));
                assert_eq!(data["assessment"], "unresolved");
            }
            if mode == "changed" {
                assert_eq!(
                    result.envelope.error.ok_or("changed")?.code,
                    ErrorCode::VersionConflict
                );
            }
            if mode == "partial" {
                assert_eq!(data["observations_complete"], false);
                assert!(!data["retry_after"].is_null());
            }
        }
        assert_eq!(setup.journal.get(OPERATION)?, Some(saved));
        let requests = product.finish()?;
        assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 1);
        assert!(requests.iter().skip(8).all(|r| r.method == "GET"));
        assert_eq!(
            requests.len(),
            if mode == "partial" {
                11
            } else if reads {
                10
            } else {
                8
            }
        );
        assert_eq!(issuer.finish()?.len(), if reads { 2 } else { 1 });
        drop(locked);
        setup.assert_no_plaintext()?;
    }
    Ok(())
}

#[test]
fn lost_create_investigation_keeps_candidates_unattributed_and_never_posts_again() -> TestResult {
    for mode in [
        "no_ids",
        "two_candidates",
        "partial",
        "confirmed",
        "confirmed_404",
        "changed_original",
    ] {
        let setup = Setup::new()?;
        let issuer = TlsServer::start(Reply::Sequence(vec![
            token(SCOPES)?,
            token("read:jira-user read:jira-work")?,
        ]))?;
        let confirmed = matches!(mode, "confirmed" | "confirmed_404");
        let mut replies = vec![identity(), identity()];
        replies.extend(metadata()?);
        replies.push(if confirmed {
            Reply::json(201, br#"{"id":"10003","key":"ED-24"}"#, &[])
        } else {
            Reply::Disconnect
        });
        if !matches!(mode, "no_ids" | "changed_original") {
            replies.push(identity());
            replies.push(if mode == "confirmed_404" {
                Reply::json(404, b"private-provider-text", &[])
            } else {
                observed("10003")?
            });
            if matches!(mode, "two_candidates" | "partial") {
                replies.push(if mode == "partial" {
                    Reply::json(429, b"private-provider-text", &[("Retry-After", "60")])
                } else {
                    observed("10004")?
                });
            }
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(ManualClock::new(1_000));
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let control = control(CancellationToken::default(), 30_000)?;
        let locked = fixture.lock(&setup.grant, &control)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let created = host(&setup.grant, "actor-one")?.execute_cached(
            fixture.client(),
            &locked,
            &setup.journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control.clone(),
        );
        assert_eq!(created.envelope.ok, confirmed);
        let saved = setup.journal.get(OPERATION)?.ok_or("journal")?;
        let mut original = original();
        if mode == "changed_original" {
            original.input["summary"] = json!("Different");
        }
        let candidates = if matches!(mode, "two_candidates" | "partial") {
            vec!["10003".into(), "10004".into()]
        } else {
            vec![]
        };
        let prepared = PreparedRecovery::prepare(
            RecoveryInput {
                operation_id: OPERATION.into(),
                original,
                candidate_issue_ids: candidates,
            },
            &["assessment".into()],
            262_144,
            "recovery-request",
            RecoveryPolicy {
                enabled: true,
                project: target(),
                grant: setup.grant.clone(),
                expected_actor: "actor-one".into(),
                allowed_commands: vec![COMMAND.into()],
            },
        )?;
        clock.advance(Duration::from_secs(3_600))?;
        let result = prepared.execute_cached_with_policy_check(
            fixture.client(),
            &locked,
            &setup.journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control,
            || Ok(()),
        );
        assert!(
            jsonschema::validator_for(&issue_recovery::schema()?)?
                .is_valid(&serde_json::to_value(&result.envelope)?)
        );
        assert_eq!(result.envelope.request_id, "recovery-request");
        assert_eq!(
            result.envelope.ok,
            !matches!(mode, "partial" | "changed_original")
        );
        let data = result.envelope.data.as_ref().ok_or("data")?;
        assert_eq!(data["initial_record"], json!(saved));
        assert_eq!(setup.journal.get(OPERATION)?, Some(saved));
        assert_eq!(
            data["assessment"],
            if confirmed {
                "recorded_confirmation"
            } else {
                "unresolved"
            }
        );
        assert_eq!(data["attribution_upgraded"], false);
        assert_eq!(data["journal_updated"], false);
        assert_eq!(data["replay_authorized"], false);
        assert_eq!(
            data["observations"].as_array().ok_or("observations")?.len(),
            match mode {
                "two_candidates" => 2,
                "partial" | "confirmed" | "confirmed_404" => 1,
                _ => 0,
            }
        );
        assert_eq!(
            data["credential_cache_update"].is_null(),
            matches!(mode, "no_ids" | "changed_original")
        );
        if mode == "confirmed_404" {
            assert_eq!(data["observations"][0]["visibility"], "absent_or_invisible");
        }
        if mode == "partial" {
            assert_eq!(data["observations_complete"], false);
            assert_eq!(
                result.envelope.error.as_ref().ok_or("rate")?.code,
                ErrorCode::RateLimited
            );
            assert!(!data["retry_after"].is_null());
        }
        if mode == "changed_original" {
            assert_eq!(
                result.envelope.error.as_ref().ok_or("binding")?.code,
                ErrorCode::OperationIdReused
            );
        }
        assert!(!serde_json::to_string(&result.envelope)?.contains("private-provider-text"));
        let requests = product.finish()?;
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
        assert!(
            requests
                .iter()
                .skip(8)
                .all(|request| request.method == "GET")
        );
        assert_eq!(
            requests.len(),
            match mode {
                "no_ids" | "changed_original" => 8,
                "two_candidates" | "partial" => 11,
                _ => 10,
            }
        );
        assert_eq!(
            issuer.finish()?.len(),
            if matches!(mode, "no_ids" | "changed_original") {
                1
            } else {
                2
            }
        );
        drop(locked);
        setup.assert_no_plaintext()?;
    }
    Ok(())
}
