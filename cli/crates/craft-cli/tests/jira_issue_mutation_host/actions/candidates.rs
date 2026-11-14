use super::*;
use craft_cli::issue_mutation::command::{IssueMutationArgs, PreparedInvocation};
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Op, IssueMutationPolicyUpdate,
};

fn args(command: &str, cascade: bool) -> Result<IssueMutationArgs, serde_json::Error> {
    let mut input = action_request(command, cascade).input;
    input["operation_id"] = json!(OPERATION);
    input["apply"] = json!(true);
    match command {
        ASSIGN_ISSUE => serde_json::from_value(input).map(IssueMutationArgs::Assign),
        UNASSIGN_ISSUE => serde_json::from_value(input).map(IssueMutationArgs::Unassign),
        _ => serde_json::from_value(input).map(IssueMutationArgs::Delete),
    }
}
fn update(operations: Vec<Op>, revision: Option<u64>, enabled: bool) -> IssueMutationPolicyUpdate {
    IssueMutationPolicyUpdate {
        target: ResourceRef {
            resource_type: "project".into(),
            id: "100".into(),
            ..target()
        },
        expected_actor: "actor-one".into(),
        enabled,
        operations,
        expected_revision: revision,
    }
}
fn options(root: &Path) -> craft_cli::profiles::InvocationOptions<'_> {
    craft_cli::profiles::InvocationOptions {
        state_root: Some(root),
        profile: Some("factory"),
        request_id: "original-request",
        max_bytes: 16_384,
    }
}

#[test]
fn action_candidates_use_exact_policy_through_auth_claim_and_frozen_boundaries() -> TestResult {
    for (command, cascade) in [
        (ASSIGN_ISSUE, false),
        (UNASSIGN_ISSUE, false),
        (DELETE_ISSUE, false),
        (DELETE_ISSUE, true),
    ] {
        for mode in [
            "success",
            "excluded",
            "before_auth",
            "revoked_claim",
            "changed_claim",
            "revoked_frozen",
            "lost",
            "refused",
        ] {
            let setup = Setup::new()?;
            let issuer = issuer(SCOPES)?;
            let mut replies = vec![identity()];
            if !matches!(mode, "excluded" | "before_auth") {
                replies.push(identity());
                replies.extend(action_reads(command, cascade)?);
            }
            if matches!(mode, "success" | "lost" | "refused") {
                replies.push(match mode {
                    "lost" => Reply::Disconnect,
                    "refused" => Reply::json(403, b"private-provider-text", &[]),
                    _ => Reply::json(204, b"", &[]),
                });
            }
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let operation = args(command, cascade)?.operation();
            let allowed = if mode == "excluded" {
                match operation {
                    Op::Delete => Op::DeleteAllSubtasks,
                    Op::DeleteAllSubtasks => Op::Delete,
                    Op::Assign => Op::Unassign,
                    _ => Op::Assign,
                }
            } else {
                operation
            };
            setup.profiles.set_issue_mutation_policy(
                "factory",
                update(vec![allowed], None, true),
                true,
                &control(CancellationToken::default())?,
                |_| Ok(()),
            )?;
            let prepared = PreparedInvocation::prepare(
                args(command, cascade)?,
                &[],
                &options(&setup.files.root),
                &setup.profiles,
                &control(CancellationToken::default())?,
            );
            if mode == "excluded" {
                assert_eq!(
                    prepared.err().ok_or("expected exclusion")?.code,
                    ErrorCode::PolicyDenied
                );
                assert_eq!(product.finish()?.len(), 0);
                assert_eq!(issuer.finish()?.len(), 0);
                continue;
            }
            let prepared = prepared?;
            let clock = Arc::new(CandidatePolicyClock {
                base: ManualClock::new(1_000),
                journal: setup.journal.clone(),
                root: setup.files.root.clone(),
                mode,
                operations: vec![operation],
                read_boundary: None,
                changed: std::sync::atomic::AtomicBool::new(false),
                claim_checks: std::sync::atomic::AtomicUsize::new(0),
            });
            let fixture = setup.fixture(&issuer, &product, clock.clone())?;
            let control = control(CancellationToken::default())?;
            let locked = fixture.lock(prepared.grant(), &control)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            clock.base.advance(Duration::from_secs(3_600))?;
            if mode == "before_auth" {
                setup.profiles.set_issue_mutation_policy(
                    "factory",
                    update(vec![operation], Some(1), false),
                    true,
                    &control,
                    |_| Ok(()),
                )?;
            }
            let result = prepared.execute_cached(
                &setup.profiles,
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control.clone(),
            );
            assert_schema(&result.envelope)?;
            assert_eq!(
                result.envelope.ok,
                mode == "success",
                "{command} {cascade} {mode}: {:?}",
                result.envelope.error
            );
            let data = result.envelope.data.as_ref().ok_or("data")?;
            assert_eq!(
                data["receipt"]["request_state"],
                match mode {
                    "success" => "confirmed",
                    "before_auth" => "not_sent",
                    "refused" => "failed",
                    _ => "sent_unknown",
                }
            );
            assert_eq!(
                data["credential_cache_update"].is_null(),
                mode == "before_auth"
            );
            assert_eq!(data["replay_authorized"], false);
            assert!(!serde_json::to_string(&result.envelope)?.contains("private-provider-text"));
            if matches!(mode, "revoked_claim" | "changed_claim" | "revoked_frozen") {
                assert!(clock.changed.load(std::sync::atomic::Ordering::SeqCst));
                assert_eq!(
                    result.envelope.error.as_ref().ok_or("policy error")?.code,
                    if mode == "changed_claim" {
                        ErrorCode::VersionConflict
                    } else {
                        ErrorCode::PolicyDenied
                    }
                );
                assert_eq!(
                    setup.journal.get(OPERATION)?.ok_or("journal")?.steps
                        [issue_mutation::route(command)?]
                    .state,
                    SubmissionState::Unknown
                );
            }
            if matches!(mode, "success" | "lost" | "refused") {
                let repeated = PreparedInvocation::prepare(
                    args(command, cascade)?,
                    &[],
                    &options(&setup.files.root),
                    &setup.profiles,
                    &control,
                )?;
                let repeated = repeated.execute_cached(
                    &setup.profiles,
                    fixture.client(),
                    &locked,
                    &setup.journal,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control,
                );
                assert_eq!(
                    repeated.envelope.error.as_ref().ok_or("repeat error")?.code,
                    ErrorCode::OperationIdReused
                );
            }
            let requests = product.finish()?;
            let reads = if command == ASSIGN_ISSUE { 5 } else { 4 };
            assert_eq!(
                requests.len(),
                if mode == "before_auth" {
                    1
                } else {
                    reads + usize::from(matches!(mode, "success" | "lost" | "refused"))
                }
            );
            if matches!(
                mode,
                "before_auth" | "revoked_claim" | "changed_claim" | "revoked_frozen"
            ) {
                assert!(requests.iter().all(|r| r.method == "GET"));
            }
            assert_eq!(
                issuer.finish()?.len(),
                if mode == "before_auth" { 1 } else { 2 }
            );
            drop(locked);
            setup.no_plaintext()?;
        }
    }
    Ok(())
}

#[test]
fn action_candidates_preserve_renewal_when_native_context_refuses_before_claim() -> TestResult {
    for (command, cascade) in [
        (ASSIGN_ISSUE, false),
        (UNASSIGN_ISSUE, false),
        (DELETE_ISSUE, false),
        (DELETE_ISSUE, true),
    ] {
        for mode in [
            "wrong_project",
            "state_drift",
            "context_drift",
            "metadata_refused",
        ] {
            let setup = Setup::new()?;
            let issuer = issuer(SCOPES)?;
            let mut replies = vec![identity(), identity()];
            if mode == "metadata_refused" {
                replies.push(Reply::json(403, b"private-provider-text", &[]));
            } else if mode == "wrong_project" {
                let mut before = action_state(cascade);
                before["fields"]["project"]["id"] = json!("999");
                replies.push(reply(before)?);
            } else {
                let mut reads = action_reads(command, cascade)?;
                reads.pop();
                let mut after = action_state(cascade);
                if mode == "state_drift" {
                    after["fields"]["status"]["id"] = json!("999");
                } else if command == DELETE_ISSUE {
                    after["fields"]["subtasks"] = json!([{"id":"10002"}]);
                } else {
                    after["fields"]["assignee"] = json!({"accountId":"other-account"});
                }
                reads.push(reply(after)?);
                replies.extend(reads);
            }
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let operation = args(command, cascade)?.operation();
            setup.profiles.set_issue_mutation_policy(
                "factory",
                update(vec![operation], None, true),
                true,
                &control(CancellationToken::default())?,
                |_| Ok(()),
            )?;
            let prepared = PreparedInvocation::prepare(
                args(command, cascade)?,
                &[],
                &options(&setup.files.root),
                &setup.profiles,
                &control(CancellationToken::default())?,
            )?;
            let clock = Arc::new(ManualClock::new(1_000));
            let fixture = setup.fixture(&issuer, &product, clock.clone())?;
            let control = control(CancellationToken::default())?;
            let locked = fixture.lock(prepared.grant(), &control)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            clock.advance(Duration::from_secs(3_600))?;
            let result = prepared.execute_cached(
                &setup.profiles,
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control,
            );
            assert_schema(&result.envelope)?;
            assert!(!result.envelope.ok);
            let data = result.envelope.data.as_ref().ok_or("data")?;
            assert_eq!(data["receipt"]["request_state"], "not_sent");
            assert!(!data["credential_cache_update"].is_null());
            assert!(setup.journal.get(OPERATION)?.is_none());
            assert!(!serde_json::to_string(&result.envelope)?.contains("private-provider-text"));
            let requests = product.finish()?;
            assert_eq!(
                requests.len(),
                if matches!(mode, "wrong_project" | "metadata_refused") {
                    3
                } else if command == ASSIGN_ISSUE {
                    5
                } else {
                    4
                }
            );
            assert!(requests.iter().all(|r| r.method == "GET"));
            assert_eq!(issuer.finish()?.len(), 2);
        }
    }
    Ok(())
}

#[test]
fn persisted_action_policy_revocation_stops_each_completed_read_boundary() -> TestResult {
    for (command, cascade) in [
        (ASSIGN_ISSUE, false),
        (UNASSIGN_ISSUE, false),
        (DELETE_ISSUE, false),
        (DELETE_ISSUE, true),
    ] {
        // Two identity probes precede native context reads; revoke after each completed read.
        for threshold in 3..=if command == ASSIGN_ISSUE { 5 } else { 4 } {
            let setup = Setup::new()?;
            let issuer = issuer(SCOPES)?;
            let mut replies = vec![identity(), identity()];
            replies.extend(action_reads(command, cascade)?);
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let operation = args(command, cascade)?.operation();
            setup.profiles.set_issue_mutation_policy(
                "factory",
                update(vec![operation], None, true),
                true,
                &control(CancellationToken::default())?,
                |_| Ok(()),
            )?;
            let prepared = PreparedInvocation::prepare(
                args(command, cascade)?,
                &[],
                &options(&setup.files.root),
                &setup.profiles,
                &control(CancellationToken::default())?,
            )?;
            let clock = Arc::new(CandidatePolicyClock {
                base: ManualClock::new(1_000),
                journal: setup.journal.clone(),
                root: setup.files.root.clone(),
                mode: "read_revoked",
                operations: vec![operation],
                read_boundary: Some((product.request_counter(), threshold)),
                changed: std::sync::atomic::AtomicBool::new(false),
                claim_checks: std::sync::atomic::AtomicUsize::new(0),
            });
            let fixture = setup.fixture(&issuer, &product, clock.clone())?;
            let control = control(CancellationToken::default())?;
            let locked = fixture.lock(prepared.grant(), &control)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            clock.base.advance(Duration::from_secs(3_600))?;
            let result = prepared.execute_cached(
                &setup.profiles,
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control,
            );
            assert_schema(&result.envelope)?;
            assert!(!result.envelope.ok, "{command} {threshold}");
            assert_eq!(
                result.envelope.error.as_ref().ok_or("policy error")?.code,
                ErrorCode::PolicyDenied
            );
            let data = result.envelope.data.as_ref().ok_or("data")?;
            assert_eq!(data["receipt"]["request_state"], "not_sent");
            assert!(!data["credential_cache_update"].is_null());
            assert!(clock.changed.load(std::sync::atomic::Ordering::SeqCst));
            assert!(setup.journal.get(OPERATION)?.is_none());
            let requests = product.finish()?;
            assert_eq!(requests.len(), threshold);
            assert!(requests.iter().all(|r| r.method == "GET"));
            assert_eq!(issuer.finish()?.len(), 2);
        }
    }
    Ok(())
}
