use super::review_preparation as prep;
use super::*;
use craft_cli::planning_review::BindingEnvironment;
use craft_cli::planning_review::command::prepare_native;
use craft_cli::planning_review::verification::{VerifyArgs, verify_native};
use craft_core::contracts::RequestState;
use craft_core::storage::{
    ArtifactBlob, ArtifactReceipt, ArtifactSpec, ArtifactStore, ArtifactWriter, StoreError,
};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::review_policy::ReviewPolicyUpdate;
use std::io::{self, Write};
#[path = "ticket_application.rs"]
mod ticket_application;
const SCOPES: &str = "read:confluence-user read:page:confluence read:comment:confluence read:jira-user read:jira-work";
struct Prepared {
    setup: Setup,
    credentials: ServiceFixture,
    clock: Arc<ManualClock>,
    plan: Value,
    design: Value,
    statement: Value,
}
fn prepared(direction: &str) -> Result<Prepared, Box<dyn std::error::Error>> {
    prepared_with_reuse(direction, false)
}
fn prepared_with_reuse(
    direction: &str,
    reuse: bool,
) -> Result<Prepared, Box<dyn std::error::Error>> {
    use craft_runtime::confluence_read_policy::{
        ConfluenceReadOperation as Op, ConfluenceReadPolicyUpdate,
    };
    let setup = Setup::new()?;
    let jira_grant = prep::configure(&setup, true)?;
    let mut operations = all_reads();
    operations.extend([Op::CommentList, Op::CommentReplies]);
    setup.store.set_confluence_read_policy(
        "factory",
        ConfluenceReadPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            operations,
            enabled: true,
            expected_revision: Some(1),
        },
        true,
        &control()?,
        |_| Ok(()),
    )?;
    let (mut plan, design) = prep::sources(false, false)?;
    if reuse {
        prep::add_reuse(&mut plan)?;
    }
    let mut replies = vec![identity(), prep::jira_identity()];
    replies.extend(prep::reads("prepared", &plan, &design, false)?);
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let issuer = issuer(SCOPES)?;
    let clock = Arc::new(ManualClock::new(1000));
    let credentials = setup.fixture(&issuer, &product, clock.clone())?;
    let jira = ResourceRef {
        provider: "jira".into(),
        resource_type: "project".into(),
        id: "10000".into(),
        ..target()
    };
    for (grant, tenant) in [
        (&setup.grant, target().tenant_ref()),
        (&jira_grant, jira.tenant_ref()),
    ] {
        let locked = credentials.lock(grant, &control()?)?;
        credentials.client().provision_grant(
            &locked,
            credentials.service_credentials()?,
            &[tenant],
            Some("actor-one"),
        )?;
    }
    clock.advance(Duration::from_secs(3600))?;
    let preparation_control = control()?;
    let artifacts = NativeArtifacts::open(
        &setup.root.join("reviews"),
        32 * 1024 * 1024,
        &preparation_control,
    )?;
    let mut arguments = prep::args(false)?;
    arguments.binding.dependency_link_direction = Some(if direction == "outward" {
        craft_cli::planning_review::DependencyDirection::Outward
    } else {
        craft_cli::planning_review::DependencyDirection::Inward
    });
    let (data, error) = prepare_native(
        arguments,
        "factory",
        "prepare-for-verification",
        BindingEnvironment {
            store: &setup.store,
            credentials: &credentials,
            client: credentials.client(),
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: preparation_control,
        },
        &artifacts,
    );
    assert!(error.is_none(), "{error:?}");
    assert!(data.prepared);
    let statement = serde_json::to_value(data.statement.ok_or("statement")?)?;
    assert_eq!(product.finish()?.len(), count);
    assert_eq!(issuer.finish()?.len(), 4);
    clock.advance(Duration::from_secs(3600))?;
    Ok(Prepared {
        setup,
        credentials,
        clock,
        plan,
        design,
        statement,
    })
}
fn comment(statement: &Value, author: &str, version: u32) -> Result<Value, serde_json::Error> {
    let encoded = serde_json::to_string(statement)?
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    Ok(
        json!({"id":"201","pageId":"101","status":"current","version":{"number":version,"authorId":author,"createdAt":"2026-11-10T00:00:00.000Z"},"body":{"storage":{"representation":"storage","value":format!("<p>{encoded}</p>")}}}),
    )
}
fn inventory(replies: &mut Vec<Reply>, statement: &Value, mode: &str, second: bool) -> TestResult {
    replies.push(identity());
    if mode == "unreadable" {
        replies.push(Reply::json(403, b"{}", &[]));
        return Ok(());
    }
    if mode == "no_comments" {
        replies.push(reply(json!({"results":[],"_links":{}}))?);
        return Ok(());
    }
    let mut decision = statement.clone();
    if mode == "changes_requested" {
        decision["decision"] = json!("changes_requested");
    }
    let author = if mode == "agent_vote" {
        "actor-one"
    } else {
        "human-one"
    };
    let listed = comment(
        &decision,
        author,
        if mode == "replacement_approval" || (second && mode == "edited_comment") {
            2
        } else {
            1
        },
    )?;
    let mut current = listed.clone();
    if mode == "stale_list" {
        current["version"]["number"] = json!(2);
    }
    replies.push(reply(json!({"results":[listed],"_links":{}}))?);
    replies.push(reply(current.clone())?);
    replies.push(reply(json!({"results":[],"_links":{}}))?);
    replies.push(reply(current)?);
    Ok(())
}
fn verification_reads(
    prepared: &Prepared,
    mode: &str,
) -> Result<Vec<Reply>, Box<dyn std::error::Error>> {
    if mode == "policy_changed" {
        return Ok(vec![]);
    }
    if mode == "scope" {
        return Ok(vec![identity()]);
    }
    if mode == "wrong_actor" {
        return Ok(vec![Reply::json(
            200,
            br#"{"accountId":"another-actor","type":"known"}"#,
            &[],
        )]);
    }
    let mut replies = vec![identity()];
    let mut plan = prepared.plan.clone();
    if mode == "stale_binding" {
        plan["version"]["number"] = json!(4);
    }
    replies.extend(prep::reads(
        if mode == "stale_links" {
            "changed_link"
        } else {
            "prepared"
        },
        &plan,
        &prepared.design,
        false,
    )?);
    if matches!(mode, "stale_binding" | "stale_links") {
        return Ok(replies);
    }
    inventory(&mut replies, &prepared.statement, mode, false)?;
    if matches!(
        mode,
        "agent_vote" | "changes_requested" | "no_comments" | "unreadable" | "stale_list"
    ) {
        return Ok(replies);
    }
    replies.extend(prep::reads(
        "prepared",
        &prepared.plan,
        &prepared.design,
        false,
    )?);
    inventory(&mut replies, &prepared.statement, mode, true)?;
    if mode == "edited_comment" {
        return Ok(replies);
    }
    replies.extend(prep::reads(
        "prepared",
        &prepared.plan,
        &prepared.design,
        false,
    )?);
    replies.push(identity());
    Ok(replies)
}
struct Publication<'a> {
    store: &'a dyn ArtifactStore,
    clock: Arc<ManualClock>,
    root: PathBuf,
    mode: String,
}
struct PublishedWriter {
    inner: Box<dyn ArtifactWriter>,
    clock: Arc<ManualClock>,
    root: PathBuf,
    mode: String,
}
impl Write for PublishedWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.inner.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
impl ArtifactWriter for PublishedWriter {
    fn finish(self: Box<Self>) -> Result<ArtifactReceipt, StoreError> {
        let receipt = self.inner.finish()?;
        match self.mode.as_str() {
            "artifact_unknown" => return Err(StoreError::OutcomeUnknown),
            "post_expiry" => self.clock.advance(Duration::from_secs(31))?,
            "post_revocation" => {
                let update = (|| -> Result<(), CraftError> {
                    let control = control()?;
                    let store = ProfileStore::open(&self.root, &control)?;
                    store.set_review_policy(
                        "factory",
                        ReviewPolicyUpdate {
                            target: ResourceRef {
                                resource_type: "space".into(),
                                id: "100".into(),
                                ..target()
                            },
                            expected_actor: "actor-one".into(),
                            reviewers: vec!["human-one".into()],
                            excluded_actors: vec![],
                            quorum: 1,
                            enabled: false,
                            expected_revision: Some(1),
                        },
                        true,
                        &control,
                        |_| Ok(()),
                    )?;
                    Ok(())
                })();
                update.map_err(|_| StoreError::OutcomeUnknown)?;
            }
            _ => {}
        }
        Ok(receipt)
    }
}
impl ArtifactStore for Publication<'_> {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError> {
        Ok(Box::new(PublishedWriter {
            inner: self.store.begin(spec)?,
            clock: self.clock.clone(),
            root: self.root.clone(),
            mode: self.mode.clone(),
        }))
    }
    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError> {
        self.store.receipt(name)
    }
    fn read(&self, name: &str, max: u64) -> Result<Option<ArtifactBlob>, StoreError> {
        self.store.read(name, max)
    }
}
fn run_case(mode: &str) -> TestResult {
    let preview = mode.starts_with("preview");
    let success = matches!(mode, "approved" | "preview" | "preview_outward");
    let prepared = prepared(if mode == "preview_outward" {
        "outward"
    } else {
        "inward"
    })?;
    if mode == "policy_changed" {
        prepared.setup.store.set_review_policy(
            "factory",
            ReviewPolicyUpdate {
                target: ResourceRef {
                    resource_type: "space".into(),
                    id: "100".into(),
                    ..target()
                },
                expected_actor: "actor-one".into(),
                reviewers: vec!["human-one".into()],
                excluded_actors: vec![],
                quorum: 1,
                enabled: true,
                expected_revision: Some(1),
            },
            true,
            &control()?,
            |_| Ok(()),
        )?;
    }
    let replies = verification_reads(&prepared, if preview { "approved" } else { mode })?;
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let scopes = if mode == "scope" {
        "read:confluence-user read:page:confluence read:jira-user read:jira-work"
    } else {
        SCOPES
    };
    let issuer = issuer(scopes)?;
    let fixture = prepared
        .setup
        .fixture(&issuer, &product, prepared.clock.clone())?;
    let invocation = control()?;
    let artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &invocation,
    )?;
    let publication = Publication {
        store: &artifacts,
        clock: prepared.clock.clone(),
        root: prepared.setup.root.clone(),
        mode: if mode == "preview_unknown" {
            "artifact_unknown"
        } else {
            mode
        }
        .into(),
    };
    let invoke = if preview {
        craft_cli::planning_review::tickets::preview_native
    } else {
        verify_native
    };
    let (data, error) = invoke(
        VerifyArgs {
            review_id: "ledger-review".into(),
            apply: true,
        },
        "factory",
        "verify-run",
        BindingEnvironment {
            store: &prepared.setup.store,
            credentials: &prepared.credentials,
            client: fixture.client(),
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: invocation,
        },
        &publication,
    );
    assert_eq!(data.review_verified, success, "{mode}: {error:?}");
    assert_eq!(error.is_none(), success, "{mode}");
    assert_eq!(data.tickets_created, 0);
    let receipts = if matches!(mode, "policy_changed" | "wrong_actor") {
        0
    } else if mode == "scope" {
        1
    } else {
        2
    };
    assert_eq!(
        data.credential_cache_updates.len(),
        receipts,
        "{mode}: {error:?}"
    );
    assert!(
        data.credential_cache_updates
            .iter()
            .all(|r| r.write_state == RequestState::Confirmed)
    );
    if success {
        assert_eq!(data.approval_count, 1);
        assert_eq!(data.distinct_reviewers, 1);
        assert_eq!(data.artifact_write_state, RequestState::Confirmed);
        let persisted = NativeArtifacts::open(
            &prepared.setup.root.join("reviews"),
            32 * 1024 * 1024,
            &control()?,
        )?;
        let artifact = persisted
            .read(&data.verification_artifact_name, 32 * 1024 * 1024)?
            .ok_or("verification artifact")?;
        let proof: Value = serde_json::from_slice(&artifact.bytes)?;
        assert_eq!(proof["review"]["approvals"][0]["actor"], "human-one");
        assert_eq!(proof["comments"][0]["version_author_id"], "human-one");
        if preview {
            let summary = data.ticket_preview.as_ref().ok_or("preview summary")?;
            assert_eq!(
                (
                    summary.task_count,
                    summary.batch_count,
                    summary.dependency_count
                ),
                (2, 2, 1)
            );
            assert_eq!(
                proof["manifest"]["batches"],
                json!([["define-contract"], ["implement-contract"]])
            );
            let link = &proof["manifest"]["links"][0];
            assert_eq!(link["prerequisite_task_id"], "define-contract");
            assert_eq!(link["dependent_task_id"], "implement-contract");
            let (inward, outward, label) = if mode == "preview_outward" {
                ("implement-contract", "define-contract", "blocks")
            } else {
                ("define-contract", "implement-contract", "is blocked by")
            };
            assert_eq!(link["inward_task_id"], inward);
            assert_eq!(link["outward_task_id"], outward);
            assert_eq!(link["dependent_label"], label);
            assert_eq!(
                proof["manifest"]["context"]["tasks"][1]["payload"]["fields"]["summary"],
                "Implement contract"
            );
            assert_eq!(
                proof["ticket_preview"]["manifest_digest"],
                summary.manifest_digest
            );
        } else {
            assert!(data.ticket_preview.is_none());
        }

        assert_eq!(
            proof["preparation_artifact"]["name"],
            "review-ledger-review.json"
        );
    } else {
        assert_eq!(data.approval_count, 0);
        assert_eq!(data.distinct_reviewers, 0);
        assert!(data.binding_digest.is_none());
        assert!(data.ticket_preview.is_none());
        let state = if matches!(mode, "artifact_unknown" | "preview_unknown") {
            RequestState::SentUnknown
        } else if matches!(mode, "post_expiry" | "post_revocation") {
            RequestState::Confirmed
        } else {
            RequestState::NotSent
        };
        assert_eq!(data.artifact_write_state, state, "{mode}");
        assert_eq!(error.as_ref().ok_or("refusal")?.details.write_state, state);
        assert_eq!(
            data.verification_artifact.is_some(),
            state == RequestState::Confirmed
        );
    }
    if matches!(
        mode,
        "approved" | "artifact_unknown" | "preview" | "preview_outward" | "preview_unknown"
    ) {
        let repeated_control = control()?;
        let repeated_artifacts = NativeArtifacts::open(
            &prepared.setup.root.join("reviews"),
            32 * 1024 * 1024,
            &repeated_control,
        )?;
        let (again, refusal) = invoke(
            VerifyArgs {
                review_id: "ledger-review".into(),
                apply: true,
            },
            "factory",
            "verify-run",
            BindingEnvironment {
                store: &prepared.setup.store,
                credentials: &prepared.credentials,
                client: fixture.client(),
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: repeated_control,
            },
            &repeated_artifacts,
        );
        assert!(refusal.is_some());
        assert!(!again.review_verified);
        assert!(again.credential_cache_updates.is_empty());
        assert!(again.verification_artifact.is_some());
        assert_eq!(again.artifact_write_state, RequestState::NotSent);
    }
    let requests = product.finish()?;
    assert_eq!(requests.len(), count, "{mode}: {error:?}");
    assert!(requests.iter().all(|r| r.method == "GET"));
    let acquisitions = if mode == "wrong_actor" { 1 } else { receipts };
    assert_eq!(issuer.finish()?.len(), acquisitions, "{mode}");
    prepared.setup.no_secrets()?;
    Ok(())
}

#[test]
fn verifies_current_human_approval() -> TestResult {
    run_case("approved")
}

#[test]
fn excludes_service_actor_votes() -> TestResult {
    run_case("agent_vote")
}

#[test]
fn refuses_changes_requests() -> TestResult {
    run_case("changes_requested")
}

#[test]
fn cached_context_cannot_supply_missing_votes() -> TestResult {
    run_case("no_comments")
}

#[test]
fn refuses_unreadable_inventory() -> TestResult {
    run_case("unreadable")
}

#[test]
fn refuses_edited_comments() -> TestResult {
    run_case("edited_comment")
}

#[test]
fn refuses_stale_list_against_current_get() -> TestResult {
    run_case("stale_list")
}

#[test]
fn refuses_stale_binding() -> TestResult {
    run_case("stale_binding")
}

#[test]
fn refuses_changed_reviewer_policy() -> TestResult {
    run_case("policy_changed")
}

#[test]
fn qualifies_comment_scope_before_resources() -> TestResult {
    run_case("scope")
}

#[test]
fn rejects_wrong_execution_actor_before_cache_commit() -> TestResult {
    run_case("wrong_actor")
}

#[test]
fn retains_unknown_verification_publication() -> TestResult {
    run_case("artifact_unknown")
}

#[test]
fn refuses_expiry_after_confirmed_publication() -> TestResult {
    run_case("post_expiry")
}

#[test]
fn retains_confirmed_artifact_after_revocation() -> TestResult {
    run_case("post_revocation")
}

#[test]
fn previews_exact_payloads_batches_and_inward_dependency_label() -> TestResult {
    run_case("preview")
}
#[test]
fn previews_explicit_outward_dependency_label_without_guessing() -> TestResult {
    run_case("preview_outward")
}
#[test]
fn refuses_changed_native_link_semantics() -> TestResult {
    run_case("stale_links")
}
#[test]
fn preview_retains_unknown_local_publication_without_approval_claim() -> TestResult {
    run_case("preview_unknown")
}

fn revalidation_case(mode: &str) -> TestResult {
    use craft_cli::planning_review::verification::revalidate_ticket_manifest;
    let prepared = prepared("inward")?;
    let replies = verification_reads(&prepared, "approved")?;
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let issuer = issuer(SCOPES)?;
    let fixture = prepared
        .setup
        .fixture(&issuer, &product, prepared.clock.clone())?;
    let preview_control = control()?;
    let preview_artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &preview_control,
    )?;
    let (preview, error) = craft_cli::planning_review::tickets::preview_native(
        VerifyArgs {
            review_id: "ledger-review".into(),
            apply: true,
        },
        "factory",
        "original-ticket-preview",
        BindingEnvironment {
            store: &prepared.setup.store,
            credentials: &prepared.credentials,
            client: fixture.client(),
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: preview_control,
        },
        &preview_artifacts,
    );
    assert!(error.is_none(), "{error:?}");
    let receipt = preview.verification_artifact.ok_or("preview receipt")?;
    let manifest_digest = preview.ticket_preview.ok_or("summary")?.manifest_digest;
    assert_eq!(product.finish()?.len(), count);
    issuer.finish()?;
    drop(fixture);
    prepared.clock.advance(Duration::from_secs(3600))?;
    let native_mode = match mode {
        "replacement" => "replacement_approval",
        "source" => "stale_binding",
        "withdrawn" => "no_comments",
        _ => "approved",
    };
    let replies = verification_reads(&prepared, native_mode)?;
    let count = replies.len();
    let product = TlsServer::start(Reply::Sequence(replies))?;
    let issuer = super::issuer(SCOPES)?;
    let fixture = prepared
        .setup
        .fixture(&issuer, &product, prepared.clock.clone())?;
    let batch_control = control()?;
    let batch_artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &batch_control,
    )?;
    let result = revalidate_ticket_manifest(
        "ledger-review",
        &receipt.name,
        "factory",
        "batch-revalidation",
        BindingEnvironment {
            store: &prepared.setup.store,
            credentials: &prepared.credentials,
            client: fixture.client(),
            tls: TlsPolicy::AdditionalCaBundle(product.root()),
            control: batch_control,
        },
        &batch_artifacts,
    );
    let success = matches!(mode, "matching" | "expiry" | "revoked");
    assert_eq!(
        result.error.is_none(),
        success,
        "{mode}: {:?}",
        result.error
    );
    assert_eq!(result.validated.is_some(), success);
    assert!(!result.credential_cache_updates.is_empty());
    if let Some(validated) = &result.validated {
        validated.check()?;
        assert_eq!(
            validated.manifest().summary()?.manifest_digest,
            manifest_digest
        );
        assert_eq!(validated.review().approvals().len(), 1);
        if mode == "expiry" {
            prepared.clock.advance(Duration::from_secs(30))?;
            assert!(validated.check().is_err());
        }
        if mode == "revoked" {
            prepared.setup.store.set_review_policy(
                "factory",
                ReviewPolicyUpdate {
                    target: ResourceRef {
                        resource_type: "space".into(),
                        id: "100".into(),
                        ..target()
                    },
                    expected_actor: "actor-one".into(),
                    reviewers: vec!["human-one".into()],
                    excluded_actors: vec![],
                    quorum: 1,
                    enabled: false,
                    expected_revision: Some(1),
                },
                true,
                &control()?,
                |_| Ok(()),
            )?;
            assert!(validated.check().is_err());
        }
    }
    let final_artifacts = NativeArtifacts::open(
        &prepared.setup.root.join("reviews"),
        32 * 1024 * 1024,
        &control()?,
    )?;
    assert_eq!(final_artifacts.receipt(&receipt.name)?, Some(receipt));
    let requests = product.finish()?;
    assert_eq!(requests.len(), count);
    assert!(requests.iter().all(|r| r.method == "GET"));
    issuer.finish()?;
    Ok(())
}
macro_rules! revalidation {
    ($name:ident,$mode:literal) => {
        #[test]
        fn $name() -> TestResult {
            revalidation_case($mode)
        }
    };
}
revalidation!(
    revalidates_original_ticket_preview_with_fresh_live_observation,
    "matching"
);
revalidation!(
    revalidation_refuses_a_new_version_of_an_otherwise_valid_original_approval,
    "replacement"
);
revalidation!(
    revalidation_refuses_changed_source_before_ticket_materialization,
    "source"
);
revalidation!(
    revalidation_refuses_withdrawn_original_approval,
    "withdrawn"
);
revalidation!(
    revalidated_batch_preserves_original_observation_expiry,
    "expiry"
);
revalidation!(
    revalidated_batch_refuses_captured_policy_revocation,
    "revoked"
);
