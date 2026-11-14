use super::*;
#[path = "actions/candidates.rs"]
mod candidates;

fn action_request(command: &str, cascade: bool) -> CanonicalRequest {
    let mut request = request(EDIT_ISSUE);
    request.command = command.into();
    request.input = json!({"issue":"10000","expected":request.input["expected"]});
    if command == DELETE_ISSUE {
        request.input["subtasks"] = if cascade {
            json!({"mode":"delete_all","expected_visible_ids":["10001"]})
        } else {
            json!({"mode":"refuse"})
        };
    } else {
        request.input["expected_assignee"] = json!({"kind":"unassigned"});
        if command == ASSIGN_ISSUE {
            request.input["account_id"] = json!("account-one");
        }
    }
    request
}
fn action_prepared(
    grant: &GrantRef,
    command: &str,
    cascade: bool,
) -> Result<PreparedIssueMutation, CraftError> {
    PreparedIssueMutation::prepare(
        action_request(command, cascade),
        OPERATION,
        true,
        "original-request",
        IssueMutationPolicy {
            enabled: true,
            project: ResourceRef {
                resource_type: "project".into(),
                id: "100".into(),
                ..target()
            },
            grant: grant.clone(),
            expected_actor: "actor-one".into(),
            permissions: vec![
                MutationPermission::Assign,
                MutationPermission::Unassign,
                MutationPermission::Delete,
                MutationPermission::DeleteAllSubtasks,
            ],
        },
    )
}
fn action_state(cascade: bool) -> Value {
    let mut state = state();
    state["fields"]["assignee"] = Value::Null;
    state["fields"]["subtasks"] = if cascade {
        json!([{"id":"10001"}])
    } else {
        json!([])
    };
    state
}
fn action_reads(command: &str, cascade: bool) -> Result<Vec<Reply>, serde_json::Error> {
    let mut replies = vec![reply(action_state(cascade))?];
    if command == ASSIGN_ISSUE {
        replies.push(reply(json!([{"accountId":"account-one","active":true}]))?);
    }
    replies.push(reply(action_state(cascade))?);
    Ok(replies)
}

#[test]
fn issue_actions_send_exact_native_calls_and_preserve_outcomes_without_replay() -> TestResult {
    for (command, cascade) in [
        (ASSIGN_ISSUE, false),
        (UNASSIGN_ISSUE, false),
        (DELETE_ISSUE, false),
        (DELETE_ISSUE, true),
    ] {
        for mode in ["success", "renew", "lost", "refused", "rate"] {
            let setup = Setup::new()?;
            let issuer = issuer(SCOPES)?;
            let mut replies = vec![identity(), identity()];
            replies.extend(action_reads(command, cascade)?);
            replies.push(match mode {
                "lost" => Reply::Disconnect,
                "refused" => Reply::json(403, b"private-provider-text", &[]),
                "rate" => Reply::json(429, b"private-provider-text", &[("Retry-After", "7")]),
                _ => Reply::json(204, b"", &[("X-Request-Id", "action-native")]),
            });
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let clock = Arc::new(ManualClock::new(1_000));
            let fixture = setup.fixture(&issuer, &product, clock.clone())?;
            let control = control(CancellationToken::default())?;
            let locked = fixture.lock(&setup.grant, &control)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            if mode == "renew" {
                clock.advance(Duration::from_secs(3_600))?;
            }
            let result = action_prepared(&setup.grant, command, cascade)?.execute_cached(
                fixture.client(),
                &locked,
                &setup.journal,
                TlsPolicy::AdditionalCaBundle(product.root()),
                control.clone(),
            );
            assert_schema(&result.envelope)?;
            assert_eq!(
                result.envelope.ok,
                matches!(mode, "success" | "renew"),
                "{command} {mode}: {:?}",
                result.envelope.error
            );
            let data = result.envelope.data.as_ref().ok_or("data")?;
            let outcome = match mode {
                "success" | "renew" => "confirmed",
                "refused" => "failed",
                _ => "sent_unknown",
            };
            assert_eq!(data["receipt"]["request_state"], outcome);
            assert_eq!(
                data["receipt"]["confirmed_resources"],
                if outcome == "confirmed" {
                    json!([target()])
                } else {
                    json!([])
                }
            );
            assert_eq!(data["credential_cache_update"].is_null(), mode != "renew");
            assert_eq!(data["replay_authorized"], false);
            assert!(!serde_json::to_string(&result.envelope)?.contains("private-provider-text"));
            let repeated = action_prepared(&setup.grant, command, cascade)?.execute_cached(
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
            let requests = product.finish()?;
            assert_eq!(requests.len(), if command == ASSIGN_ISSUE { 6 } else { 5 });
            let last = requests.last().ok_or("write")?;
            assert_eq!(
                last.method,
                if command == DELETE_ISSUE {
                    "DELETE"
                } else {
                    "PUT"
                }
            );
            if command == DELETE_ISSUE {
                assert!(last.target.ends_with(if cascade {
                    "/rest/api/3/issue/10000?deleteSubtasks=true"
                } else {
                    "/rest/api/3/issue/10000?deleteSubtasks=false"
                }));
                assert!(last.body.is_empty());
            } else {
                assert!(last.target.ends_with("/rest/api/3/issue/10000/assignee"));
                assert_eq!(
                    serde_json::from_slice::<Value>(&last.body)?,
                    if command == ASSIGN_ISSUE {
                        json!({"accountId":"account-one"})
                    } else {
                        json!({"accountId":null})
                    }
                );
            }
            assert!(requests.iter().all(|r| !r.target.contains("editmeta")));
            if command == ASSIGN_ISSUE {
                let account = &requests[3];
                assert!(account.target.contains("issueId=10000"));
                assert!(account.target.contains("accountId=account-one"));
                assert!(!account.target.contains("project="));
            }
            assert_eq!(issuer.finish()?.len(), if mode == "renew" { 2 } else { 1 });
            drop(locked);
            setup.no_plaintext()?;
        }
    }
    Ok(())
}

#[test]
fn issue_actions_refuse_claim_time_expiry_policy_revocation_and_cancellation() -> TestResult {
    for (command, cascade) in [
        (ASSIGN_ISSUE, false),
        (UNASSIGN_ISSUE, false),
        (DELETE_ISSUE, false),
        (DELETE_ISSUE, true),
    ] {
        for mode in ["expire", "revoke", "cancel"] {
            let setup = Setup::new()?;
            let issuer = issuer(SCOPES)?;
            let mut replies = vec![identity(), identity()];
            replies.extend(action_reads(command, cascade)?);
            let product = TlsServer::start(Reply::Sequence(replies))?;
            let clock = Arc::new(ClaimClock {
                base: ManualClock::new(1_000),
                journal: setup.journal.clone(),
                expire: mode == "expire",
            });
            let fixture = setup.fixture(&issuer, &product, clock)?;
            let token = CancellationToken::default();
            let control = control(token.clone())?;
            let locked = fixture.lock(&setup.grant, &control)?;
            fixture.client().provision_grant(
                &locked,
                fixture.service_credentials()?,
                &[target().tenant_ref()],
                Some("actor-one"),
            )?;
            let result = action_prepared(&setup.grant, command, cascade)?
                .execute_cached_with_policy_check(
                    fixture.client(),
                    &locked,
                    &setup.journal,
                    TlsPolicy::AdditionalCaBundle(product.root()),
                    control,
                    || {
                        let claimed = setup
                            .journal
                            .get(OPERATION)
                            .map_err(|_| {
                                CraftError::new(ErrorCode::Internal, "Cannot inspect action claim.")
                            })?
                            .is_some_and(|record| {
                                record
                                    .steps
                                    .values()
                                    .any(|step| step.state == SubmissionState::SendStarted)
                            });
                        if claimed && mode == "cancel" {
                            token.cancel();
                        }
                        if claimed && mode == "revoke" {
                            return Err(CraftError::new(
                                ErrorCode::PolicyDenied,
                                "Action policy revoked.",
                            ));
                        }
                        Ok(())
                    },
                );
            assert_schema(&result.envelope)?;
            assert!(!result.envelope.ok);
            assert_eq!(
                result.envelope.data.as_ref().ok_or("data")?["receipt"]["request_state"],
                "sent_unknown"
            );
            assert_eq!(
                setup.journal.get(OPERATION)?.ok_or("journal")?.steps
                    [issue_mutation::route(command)?]
                .state,
                SubmissionState::Unknown
            );
            let requests = product.finish()?;
            assert_eq!(requests.len(), if command == ASSIGN_ISSUE { 5 } else { 4 });
            assert!(requests.iter().all(|r| r.method == "GET"));
            assert_eq!(issuer.finish()?.len(), 1);
        }
    }
    Ok(())
}
