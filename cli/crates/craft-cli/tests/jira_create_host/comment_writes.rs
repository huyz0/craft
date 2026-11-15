use super::*;
use crate::conformance::ObservedAs;
use craft_cli::comment_writes::{CommentWriteEnvironment, MINIMUM_OUTPUT, PreparedCommentWrite};
use craft_provider_jira::comments::{mutation as native, parse_record};
use craft_runtime::issue_mutation_policy::{
    IssueMutationOperation as Op, IssueMutationPolicyUpdate,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
const COMMENT_OPERATION: &str = "review-comment";
#[test]
fn comment_output_capacity_is_refused_before_credentials_or_native_effects() -> TestResult {
    let setup = Setup::new()?;
    let mut request = request(native::CREATE)?;
    request.selection.max_bytes = 16_384;
    let result = PreparedCommentWrite::prepare(
        request,
        COMMENT_OPERATION,
        true,
        "factory",
        &setup.profiles,
        &control(CancellationToken::default(), 30_000)?,
    );
    assert_eq!(
        result.err().map(|e| e.code),
        Some(ErrorCode::OutputBudgetTooSmall)
    );
    assert!(setup.journal.get(COMMENT_OPERATION)?.is_none());
    setup.cleanup()
}
struct RevokingClock {
    base: ManualClock,
    armed: Arc<AtomicBool>,
    revoked: Arc<AtomicBool>,
    store: Mutex<ProfileStore>,
    control: InvocationControl,
}
impl Clock for RevokingClock {
    fn unix_millis(&self) -> u64 {
        self.base.unix_millis()
    }
    fn monotonic_millis(&self) -> u64 {
        if self.armed.swap(false, Ordering::SeqCst)
            && let Ok(store) = self.store.lock()
            && store
                .set_issue_mutation_policy(
                    "factory",
                    IssueMutationPolicyUpdate {
                        target: target(),
                        expected_actor: "actor-one".into(),
                        enabled: false,
                        operations: vec![Op::CommentCreate],
                        expected_revision: Some(1),
                    },
                    true,
                    &self.control,
                    |_| Ok(()),
                )
                .is_ok()
        {
            self.revoked.store(true, Ordering::SeqCst);
        }
        self.base.monotonic_millis()
    }
}
fn body(text: &str) -> Value {
    json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":text}]}]})
}
fn comment() -> Value {
    json!({"id":"101","body":body("Original"),"author":{"accountId":"actor-one"},"updateAuthor":{"accountId":"human-other"},"jsdPublic":false})
}
fn request(command: &str) -> Result<CanonicalRequest, CraftError> {
    let mut input = json!({"issue_id":"10001"});
    if command == native::CREATE {
        input["body"] = body("Reviewed");
        input["visibility"] = json!({"mode":"project"});
    } else {
        input["comment_id"] = json!("101");
        input["expected_revision"] = json!(parse_record(&comment(), "10001", true)?.revision);
        if command == native::UPDATE {
            input["body"] = body("Reviewed");
        }
    }
    Ok(CanonicalRequest {
        command: command.into(),
        target: target(),
        input,
        selection: Selection {
            max_bytes: MINIMUM_OUTPUT as u32,
            ..Selection::default()
        },
        preconditions: None,
    })
}
fn configure(setup: &Setup, enabled: bool, revision: Option<u64>) -> TestResult {
    setup.profiles.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            target: target(),
            expected_actor: "actor-one".into(),
            enabled,
            operations: vec![
                Op::CommentCreate,
                Op::CommentUpdateOwn,
                Op::CommentUpdateAny,
                Op::CommentDeleteOwn,
                Op::CommentDeleteAny,
            ],
            expected_revision: revision,
        },
        true,
        &control(CancellationToken::default(), 30_000)?,
        |_| Ok(()),
    )?;
    Ok(())
}
fn reply(value: Value) -> Result<Reply, serde_json::Error> {
    Ok(Reply::json(200, &serde_json::to_vec(&value)?, &[]))
}
fn case(command: &str, mode: &str) -> TestResult {
    let setup = Setup::new()?;
    configure(&setup, true, None)?;
    let mut request = request(command)?;
    let mut original = comment();
    if mode == "other_author" {
        original["author"]["accountId"] = json!("human-other");
        request.input["expected_revision"] =
            json!(parse_record(&original, "10001", true)?.revision);
    }
    if mode == "any_denied" {
        request.input["ownership"] = json!("any");
    }
    let issuer = issuer(SCOPES)?;
    let project = json!({"id":"10001","fields":{"project":{"id":"10000"}}});
    let (own, all) = if command == native::UPDATE {
        ("EDIT_OWN_COMMENTS", "EDIT_ALL_COMMENTS")
    } else {
        ("DELETE_OWN_COMMENTS", "DELETE_ALL_COMMENTS")
    };
    let mut permissions =
        json!({"permissions":{"BROWSE_PROJECTS":{"key":"BROWSE_PROJECTS","havePermission":true}}});
    if command == native::CREATE {
        permissions["permissions"]["ADD_COMMENTS"] =
            json!({"key":"ADD_COMMENTS","havePermission":true});
    } else {
        permissions["permissions"][own] = json!({"key":own,"havePermission":true});
        permissions["permissions"][all] = json!({"key":all,"havePermission":false});
    }
    let mut rows = vec![identity()];
    if mode != "revoked_before_auth" {
        rows.push(identity());
        rows.push(reply(project.clone())?);
        if command != native::CREATE {
            rows.push(reply(original.clone())?);
        }
        if mode != "other_author" {
            rows.push(reply(permissions)?);
            if mode != "any_denied" {
                if command != native::CREATE {
                    if mode == "fingerprint_drift" {
                        original["body"] = body("Human edit");
                    }
                    rows.push(reply(original)?);
                }
                if mode != "fingerprint_drift" {
                    rows.push(reply(project)?);
                }
            }
        }
    }
    let sent = !matches!(
        mode,
        "revoked_before_auth"
            | "revoked_claim"
            | "late_revoked"
            | "other_author"
            | "any_denied"
            | "fingerprint_drift"
    );
    if sent {
        rows.push(match mode {
            "lost" => Reply::Disconnect,
            "refused" => Reply::json(400, b"{}", &[]),
            "unqualified" => Reply::json(403, b"{}", &[]),
            "incomplete" => Reply::json(
                if command == native::CREATE { 201 } else { 200 },
                br#"{"id":"101","jsdPublic":"malformed"}"#,
                &[],
            ),
            _ if command == native::DELETE => Reply::json(204, b"", &[]),
            _ => Reply::json(
                if command == native::CREATE { 201 } else { 200 },
                &serde_json::to_vec(
                    &json!({"id":"101","body":body("Reviewed"),"jsdPublic":false}),
                )?,
                &[],
            ),
        });
    }
    let count = rows.len();
    let product = TlsServer::start(Reply::Sequence(rows))?;
    let armed = Arc::new(AtomicBool::new(false));
    let revoked = Arc::new(AtomicBool::new(false));
    let clock: Arc<dyn Clock> = if mode == "late_revoked" {
        Arc::new(RevokingClock {
            base: ManualClock::new(1000),
            armed: armed.clone(),
            revoked: revoked.clone(),
            store: Mutex::new(ProfileStore::open(
                &setup.files.root,
                &control(CancellationToken::default(), 30_000)?,
            )?),
            control: control(CancellationToken::default(), 30_000)?,
        })
    } else {
        Arc::new(ManualClock::new(1000))
    };
    let fixture = setup.fixture(&issuer, &product, clock.clone())?;
    let control = control(CancellationToken::default(), 30_000)?;
    let locked = fixture.lock(&setup.grant, &control)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    let prepare = || {
        PreparedCommentWrite::prepare(
            request.clone(),
            COMMENT_OPERATION,
            true,
            "factory",
            &setup.profiles,
            &control,
        )
    };
    let prepared = prepare()?;
    if mode == "revoked_before_auth" {
        configure(&setup, false, Some(1))?;
    }
    let changed = AtomicBool::new(false);
    let postclaim_checks = AtomicUsize::new(0);
    let execute = |prepared: PreparedCommentWrite| {
        prepared.execute(
            CommentWriteEnvironment {
                store: &setup.profiles,
                client: fixture.client(),
                locked: &locked,
                journal: &setup.journal,
                tls: TlsPolicy::AdditionalCaBundle(product.root()),
                control: control.clone(),
            },
            || {
                if mode == "late_revoked"
                    && setup
                        .journal
                        .get(COMMENT_OPERATION)
                        .map_err(|_| CraftError::new(ErrorCode::Internal, "journal"))?
                        .is_some_and(|r| {
                            r.steps
                                .values()
                                .any(|s| s.state == SubmissionState::SendStarted)
                        })
                    && postclaim_checks.fetch_add(1, Ordering::SeqCst) == 1
                {
                    armed.store(true, Ordering::SeqCst);
                }
                if mode == "revoked_claim"
                    && setup
                        .journal
                        .get(COMMENT_OPERATION)
                        .map_err(|_| CraftError::new(ErrorCode::Internal, "journal"))?
                        .is_some_and(|r| {
                            r.steps
                                .values()
                                .any(|s| s.state == SubmissionState::SendStarted)
                        })
                    && !changed.swap(true, Ordering::SeqCst)
                {
                    setup.profiles.set_issue_mutation_policy(
                        "factory",
                        IssueMutationPolicyUpdate {
                            target: target(),
                            expected_actor: "actor-one".into(),
                            enabled: false,
                            operations: vec![Op::CommentCreate],
                            expected_revision: Some(1),
                        },
                        true,
                        &control,
                        |_| Ok(()),
                    )?;
                }
                Ok(())
            },
        )
    };
    let (data, error) = execute(prepared).observed_as(command);
    let acknowledged = matches!(mode, "confirmed" | "incomplete");
    assert_eq!(
        error.is_none(),
        mode == "confirmed",
        "{command} {mode} {error:?}"
    );
    assert_eq!(data.acknowledged, acknowledged);
    assert_eq!(data.desired_fields_verified, mode == "confirmed");
    assert!(!data.replay_authorized && !data.atomic_revision_check);
    assert!(data.before.as_ref().is_none_or(|r| r.body.is_none()));
    assert!(data.comment.as_ref().is_none_or(|r| r.body.is_none()));
    let expected = if acknowledged {
        craft_core::contracts::RequestState::Confirmed
    } else if mode == "refused" {
        craft_core::contracts::RequestState::Failed
    } else if sent || matches!(mode, "revoked_claim" | "late_revoked") {
        craft_core::contracts::RequestState::SentUnknown
    } else {
        craft_core::contracts::RequestState::NotSent
    };
    assert_eq!(data.receipt.request_state, expected, "{command} {mode}");
    if sent || matches!(mode, "revoked_claim" | "late_revoked") {
        let saved = setup.journal.get(COMMENT_OPERATION)?;
        assert!(saved.is_some());
        if !matches!(mode, "revoked_claim" | "late_revoked") {
            assert!(execute(prepare()?).observed_as(command).1.is_some());
            assert_eq!(setup.journal.get(COMMENT_OPERATION)?, saved);
            assert_eq!(product.request_count(), count);
        }
    } else {
        assert!(setup.journal.get(COMMENT_OPERATION)?.is_none());
    }
    let observed = product.finish()?;
    assert_eq!(observed.len(), count);
    let writes: Vec<_> = observed
        .iter()
        .filter(|r| matches!(r.method.as_str(), "POST" | "PUT" | "DELETE"))
        .collect();
    assert_eq!(writes.len(), usize::from(sent));
    if sent {
        let suffix = if command == native::CREATE {
            "/rest/api/3/issue/10001/comment"
        } else {
            "/rest/api/3/issue/10001/comment/101"
        };
        assert!(writes[0].target.ends_with(suffix));
        if command == native::DELETE {
            assert!(writes[0].body.is_empty());
        } else {
            let raw: Value = serde_json::from_slice(&writes[0].body)?;
            assert_eq!(raw["body"], body("Reviewed"));
            if command == native::CREATE {
                assert_eq!(raw["visibility"], Value::Null);
            } else {
                assert!(raw.get("visibility").is_none());
            }
            assert!(raw.get("ownership").is_none() && raw.get("expected_revision").is_none());
        }
    }
    assert_eq!(issuer.finish()?.len(), 1);
    drop(locked);
    drop(fixture);
    drop(clock);
    if mode == "late_revoked" {
        assert!(revoked.load(Ordering::SeqCst));
    }
    setup.assert_no_plaintext()?;
    setup.cleanup()?;
    Ok(())
}
#[test]
fn comment_create_is_one_claimed_post() -> TestResult {
    case(native::CREATE, "confirmed")
}
#[test]
fn comment_update_is_one_claimed_put() -> TestResult {
    case(native::UPDATE, "confirmed")
}
#[test]
fn comment_delete_is_one_claimed_delete() -> TestResult {
    case(native::DELETE, "confirmed")
}
#[test]
fn lost_comment_create_response_cannot_send_again() -> TestResult {
    case(native::CREATE, "lost")
}
#[test]
fn lost_comment_delete_response_cannot_send_again() -> TestResult {
    case(native::DELETE, "lost")
}
#[test]
fn refused_comment_write_cannot_send_again() -> TestResult {
    case(native::UPDATE, "refused")
}
#[test]
fn unqualified_comment_status_retains_uncertainty() -> TestResult {
    case(native::UPDATE, "unqualified")
}
#[test]
fn acknowledged_comment_id_survives_incomplete_response() -> TestResult {
    case(native::CREATE, "incomplete")
}
#[test]
fn comment_original_author_denial_precedes_claim() -> TestResult {
    case(native::DELETE, "other_author")
}
#[test]
fn comment_any_intent_requires_native_all_permission() -> TestResult {
    case(native::UPDATE, "any_denied")
}
#[test]
fn comment_fingerprint_drift_precedes_claim() -> TestResult {
    case(native::UPDATE, "fingerprint_drift")
}
#[test]
fn comment_policy_revocation_precedes_authentication() -> TestResult {
    case(native::CREATE, "revoked_before_auth")
}
#[test]
fn comment_policy_revocation_after_claim_prevents_send() -> TestResult {
    case(native::CREATE, "revoked_claim")
}
#[test]
fn comment_policy_revocation_during_final_freshness_observation_prevents_send() -> TestResult {
    case(native::CREATE, "late_revoked")
}
