use super::*;
use craft_cli::issue_recovery::{self, PreparedRecovery, RecoveryInput, RecoveryPolicy};
use craft_core::storage::JournalRecord;
use std::sync::atomic::{AtomicBool, Ordering};
struct RecoveryClock {
    base: ManualClock,
    counter: Arc<std::sync::atomic::AtomicUsize>,
    journal: NativeJournal,
    enabled: bool,
    hits: std::sync::atomic::AtomicUsize,
    changed: AtomicBool,
}
impl Clock for RecoveryClock {
    fn unix_millis(&self) -> u64 {
        if self.enabled
            && self.counter.load(Ordering::SeqCst) >= 8
            && self.hits.fetch_add(1, Ordering::SeqCst) >= 2
            && !self.changed.load(Ordering::SeqCst)
        {
            let update = (|| -> Result<(), craft_core::storage::StoreError> {
                let old = self
                    .journal
                    .get(OPERATION)?
                    .ok_or(craft_core::storage::StoreError::Corrupt)?;
                let mut next = old.clone();
                next.revision += 1;
                let step = next
                    .steps
                    .get_mut(
                        issue_mutation::route(EDIT_ISSUE)
                            .map_err(|_| craft_core::storage::StoreError::Corrupt)?,
                    )
                    .ok_or(craft_core::storage::StoreError::Corrupt)?;
                step.state = SubmissionState::Confirmed;
                step.confirmed_resources.push(target());
                self.journal.compare_and_swap(old.revision, &next)
            })();
            self.changed.store(update.is_ok(), Ordering::SeqCst);
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}
fn token(scopes: &str) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(
        200,
        &serde_json::to_vec(
            &json!({"access_token":ACCESS_TOKEN,"expires_in":120,"token_type":"Bearer","scope":scopes}),
        )?,
        &[],
    ))
}

struct RecoveryPolicyClock {
    base: ManualClock,
    root: PathBuf,
    counter: Arc<std::sync::atomic::AtomicUsize>,
    threshold: usize,
    delay: usize,
    hits: std::sync::atomic::AtomicUsize,
    armed: AtomicBool,
    changed: AtomicBool,
    keep_enabled: bool,
}
impl Clock for RecoveryPolicyClock {
    fn unix_millis(&self) -> u64 {
        if self.armed.load(Ordering::SeqCst)
            && self.counter.load(Ordering::SeqCst) >= self.threshold
            && self.hits.fetch_add(1, Ordering::SeqCst) >= self.delay
            && !self.changed.load(Ordering::SeqCst)
        {
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                let control = control(CancellationToken::default())?;
                let store = ProfileStore::open(&self.root, &control)?;
                store.set_issue_recovery_policy(
                    "factory",
                    craft_runtime::issue_recovery_policy::IssueRecoveryPolicyUpdate {
                        target: ResourceRef {
                            resource_type: "project".into(),
                            id: "100".into(),
                            ..target()
                        },
                        expected_actor: "actor-one".into(),
                        enabled: self.keep_enabled,
                        expected_revision: Some(1),
                        operations: vec![
                            craft_runtime::issue_recovery_policy::IssueRecoveryOperation::Edit,
                        ],
                    },
                    true,
                    &control,
                    |_| Ok(()),
                )?;
                Ok(())
            })();
            self.changed.store(result.is_ok(), Ordering::SeqCst);
        }
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        self.base.monotonic_millis()
    }
}

#[test]
fn typed_edit_recovery_rechecks_captured_policy_after_identity_observation_and_final_clock()
-> TestResult {
    use craft_cli::issue_recovery::command::{OperationReconcileArgs, PreparedInvocation};
    use craft_runtime::issue_recovery_policy::{
        IssueRecoveryOperation as Op, IssueRecoveryPolicyUpdate,
    };
    for mode in [
        "success",
        "after_identity",
        "after_observation",
        "final_clock",
        "changed_allowed",
    ] {
        let setup = Setup::new()?;
        let issuer = TlsServer::start(Reply::Sequence(vec![
            token(SCOPES)?,
            token("read:jira-user read:jira-work")?,
        ]))?;
        let mut replies = vec![identity(), identity()];
        replies.extend(metadata(EDIT_ISSUE, false)?);
        replies.push(Reply::Disconnect);
        replies.push(identity());
        if mode != "after_identity" {
            let mut observed = state();
            observed["fields"]["assignee"] = Value::Null;
            replies.push(reply(observed)?);
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(RecoveryPolicyClock {
            base: ManualClock::new(1_000),
            root: setup.files.root.clone(),
            counter: product.request_counter(),
            threshold: if mode == "after_identity" { 7 } else { 8 },
            delay: if mode == "final_clock" { 2 } else { 0 },
            hits: std::sync::atomic::AtomicUsize::new(0),
            armed: AtomicBool::new(false),
            changed: AtomicBool::new(false),
            keep_enabled: mode == "changed_allowed",
        });
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let control = control(CancellationToken::default())?;
        let locked = fixture.lock(&setup.grant, &control)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let lost = prepared(&setup.grant, EDIT_ISSUE, "actor-one")?.execute_cached(
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
                target: ResourceRef {
                    resource_type: "project".into(),
                    id: "100".into(),
                    ..target()
                },
                expected_actor: "actor-one".into(),
                enabled: true,
                expected_revision: None,
                operations: vec![Op::Edit],
            },
            true,
            &control,
            |_| Ok(()),
        )?;
        let options = craft_cli::profiles::InvocationOptions {
            state_root: Some(&setup.files.root),
            profile: Some("factory"),
            request_id: "edit-recovery",
            max_bytes: 262_144,
        };
        let candidate = PreparedInvocation::prepare(
            OperationReconcileArgs {
                saved: false,
                operation_id: OPERATION.into(),
                original: Some(request(EDIT_ISSUE)),
                candidate_issue_ids: vec![],
            },
            &[],
            &options,
            &setup.profiles,
            &control,
        )?;
        clock.base.advance(Duration::from_secs(3_600))?;
        clock.armed.store(mode != "success", Ordering::SeqCst);
        let result = candidate.execute_cached(
            &setup.profiles,
            fixture.client(),
            &locked,
            &setup.journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control,
        );
        assert_eq!(
            result.envelope.ok,
            mode == "success",
            "{mode}: {:?}",
            result.envelope.error
        );
        assert_eq!(
            clock.changed.load(Ordering::SeqCst),
            mode != "success",
            "{mode}"
        );
        let data = result.envelope.data.ok_or("data")?;
        assert_eq!(data["initial_record"], json!(saved));
        assert_eq!(data["credential_cache_update"]["write_state"], "confirmed");
        assert_eq!(
            data["observations"].as_array().ok_or("observations")?.len(),
            usize::from(mode == "success")
        );
        assert_eq!(data["assessment"], "unresolved");
        assert_eq!(data["replay_authorized"], false);
        assert_eq!(setup.journal.get(OPERATION)?, Some(saved));
        if mode == "changed_allowed" {
            assert_eq!(
                result.envelope.error.ok_or("changed")?.code,
                ErrorCode::VersionConflict
            );
        }
        let requests = product.finish()?;
        assert_eq!(requests.iter().filter(|r| r.method == "PUT").count(), 1);
        assert!(requests.iter().skip(6).all(|r| r.method == "GET"));
        assert_eq!(requests.len(), if mode == "after_identity" { 7 } else { 8 });
        assert_eq!(issuer.finish()?.len(), 2);
    }
    Ok(())
}
#[test]
fn unknown_edit_investigation_is_read_only_and_preserves_concurrent_and_cache_evidence()
-> TestResult {
    for mode in [
        "visible",
        "404",
        "rate",
        "project",
        "policy",
        "concurrent",
        "late_concurrent",
        "cancel",
        "binding",
        "missing",
        "budget",
        "actor",
        "scope",
    ] {
        let setup = Setup::new()?;
        let issuer = TlsServer::start(Reply::Sequence(vec![
            token(SCOPES)?,
            token(if mode == "scope" {
                "read:jira-user"
            } else {
                "read:jira-user read:jira-work"
            })?,
        ]))?;
        let mut replies = vec![identity(), identity()];
        replies.extend(metadata(EDIT_ISSUE, false)?);
        replies.push(Reply::Disconnect);
        if !matches!(mode, "binding" | "missing" | "budget" | "actor") {
            replies.push(identity());
            if mode != "scope" {
                let mut observed = state();
                observed["fields"]["assignee"] = Value::Null;
                if mode == "project" {
                    observed["fields"]["project"]["id"] = json!("999");
                }
                replies.push(match mode {
                    "404" => Reply::json(404, b"private-provider-text", &[]),
                    "rate" => Reply::json(429, b"private-provider-text", &[("Retry-After", "7")]),
                    _ => reply(observed)?,
                });
            }
        }
        let product = TlsServer::start(Reply::Sequence(replies))?;
        let clock = Arc::new(RecoveryClock {
            base: ManualClock::new(1_000),
            counter: product.request_counter(),
            journal: setup.journal.clone(),
            enabled: mode == "late_concurrent",
            hits: std::sync::atomic::AtomicUsize::new(0),
            changed: AtomicBool::new(false),
        });
        let fixture = setup.fixture(&issuer, &product, clock.clone())?;
        let cancel = CancellationToken::default();
        let control = control(cancel.clone())?;
        let locked = fixture.lock(&setup.grant, &control)?;
        fixture.client().provision_grant(
            &locked,
            fixture.service_credentials()?,
            &[target().tenant_ref()],
            Some("actor-one"),
        )?;
        let lost = prepared(&setup.grant, EDIT_ISSUE, "actor-one")?.execute_cached(
            fixture.client(),
            &locked,
            &setup.journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control.clone(),
        );
        assert!(!lost.envelope.ok);
        let saved = setup.journal.get(OPERATION)?.ok_or("journal")?;
        let mut original = request(EDIT_ISSUE);
        if mode == "binding" {
            original.input["changes"]["description"] = json!("Other");
        }
        let recovery = PreparedRecovery::prepare(
            RecoveryInput {
                operation_id: if mode == "missing" {
                    "absent-operation"
                } else {
                    OPERATION
                }
                .into(),
                original,
                candidate_issue_ids: vec![],
            },
            &[],
            if mode == "budget" { 512 } else { 262_144 },
            "recovery-request",
            RecoveryPolicy {
                enabled: true,
                project: ResourceRef {
                    resource_type: "project".into(),
                    id: "100".into(),
                    ..target()
                },
                grant: setup.grant.clone(),
                expected_actor: if mode == "actor" {
                    "actor-two"
                } else {
                    "actor-one"
                }
                .into(),
                allowed_commands: vec![EDIT_ISSUE.into()],
            },
        )?;
        clock.base.advance(Duration::from_secs(3_600))?;
        let changed = AtomicBool::new(false);
        let result = recovery.execute_cached_with_policy_check(
            fixture.client(),
            &locked,
            &setup.journal,
            TlsPolicy::AdditionalCaBundle(product.root()),
            control,
            || {
                if product.request_count() >= 8 {
                    if mode == "cancel" {
                        cancel.cancel();
                    }
                    if mode == "policy" {
                        return Err(CraftError::new(
                            ErrorCode::PolicyDenied,
                            "Recovery authority revoked.",
                        ));
                    }
                    if mode == "concurrent" && !changed.swap(true, Ordering::SeqCst) {
                        let mut next: JournalRecord = saved.clone();
                        next.revision += 1;
                        let step = next
                            .steps
                            .get_mut(issue_mutation::route(EDIT_ISSUE)?)
                            .ok_or_else(|| {
                                CraftError::new(ErrorCode::Internal, "Missing fixture step.")
                            })?;
                        step.state = SubmissionState::Confirmed;
                        step.confirmed_resources.push(target());
                        setup
                            .journal
                            .compare_and_swap(saved.revision, &next)
                            .map_err(|_| {
                                CraftError::new(
                                    ErrorCode::Internal,
                                    "Concurrent fixture update failed.",
                                )
                            })?;
                    }
                }
                Ok(())
            },
        );
        assert!(
            jsonschema::validator_for(&issue_recovery::schema()?)?
                .is_valid(&serde_json::to_value(&result.envelope)?)
        );
        assert_eq!(
            result.envelope.ok,
            matches!(mode, "visible" | "404" | "missing"),
            "{mode}: {:?}",
            result.envelope.error
        );
        let data = result.envelope.data.as_ref().ok_or("data")?;
        assert_eq!(data["journal_updated"], false);
        assert_eq!(data["replay_authorized"], false);
        assert_eq!(data["attribution_upgraded"], false);
        if mode == "missing" {
            assert_eq!(data["initial_record"], Value::Null);
            assert!(setup.journal.get("absent-operation")?.is_none());
        } else {
            assert_eq!(data["initial_record"], json!(saved));
            assert_eq!(data["assessment"], "unresolved");
        }
        if matches!(mode, "concurrent" | "late_concurrent") {
            assert_eq!(data["journal_matches_initial"], false);
            assert!(!data["latest_record"].is_null());
            assert_eq!(
                data["latest_record"]["steps"][issue_mutation::route(EDIT_ISSUE)?]["state"],
                "confirmed"
            );
            assert_eq!(
                result.envelope.error.as_ref().ok_or("conflict")?.code,
                ErrorCode::VersionConflict
            );
        } else {
            assert_eq!(setup.journal.get(OPERATION)?, Some(saved));
        }
        if mode == "cancel" {
            assert_eq!(
                result.envelope.error.as_ref().ok_or("cancellation")?.code,
                ErrorCode::Cancelled
            );
        }
        assert_eq!(
            data["observations"].as_array().ok_or("observations")?.len(),
            usize::from(matches!(mode, "visible" | "404"))
        );
        assert_eq!(
            data["credential_cache_update"].is_null(),
            matches!(mode, "binding" | "missing" | "budget" | "actor")
        );
        assert!(!serde_json::to_string(&result.envelope)?.contains("private-provider-text"));
        let requests = product.finish()?;
        assert_eq!(requests.iter().filter(|r| r.method == "PUT").count(), 1);
        assert!(requests.iter().skip(6).all(|r| r.method == "GET"));
        assert_eq!(
            requests.len(),
            if matches!(mode, "binding" | "missing" | "budget" | "actor") {
                6
            } else if mode == "scope" {
                7
            } else {
                8
            }
        );
        assert_eq!(
            issuer.finish()?.len(),
            if matches!(mode, "binding" | "missing" | "budget" | "actor") {
                1
            } else {
                2
            }
        );
        drop(locked);
        setup.no_plaintext()?;
    }
    Ok(())
}
