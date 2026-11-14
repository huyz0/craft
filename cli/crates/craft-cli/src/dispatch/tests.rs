use super::*;

#[test]
fn policy_setup_dispatch_keeps_explicit_input_and_complete_configuration_receipts()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::policies::{self, PolicyArgs};
    let mut registry = catalog::local_registry()?;
    policies::register_candidates(&mut registry)?;
    let control = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        10_000,
    )?;
    let options = crate::profiles::InvocationOptions {
        state_root: None,
        profile: Some("factory"),
        request_id: "original-request",
        max_bytes: 16384,
    };
    for command in [policies::GET, policies::SET] {
        let input = registry.descriptor(command)?.example.clone();
        let action = Action::Policy {
            action: if command == policies::GET {
                crate::args::PolicyAction::Get(serde_json::from_value(input.clone())?)
            } else {
                crate::args::PolicyAction::Set(serde_json::from_value(input.clone())?)
            },
        };
        let action = decode_action(action, input)?;
        let expected = argument_value(&action)?;
        let data = json!({"configuration_write_state":"sent_unknown","proposed_policy":{"revision":2},"policy":null,"live_verified":false});
        let result = execute_policy_candidate(
            &registry,
            action,
            &[],
            &control,
            &options,
            |args, _, _, options| {
                let actual = match &args {
                    PolicyArgs::Get(v) => serde_json::to_value(v),
                    PolicyArgs::Set(v) => serde_json::to_value(v),
                };
                assert_eq!(actual.ok(), Some(expected));
                CommandResult {
                    envelope: Envelope::success(command, options.request_id.into(), data.clone()),
                    output_policy: crate::output::OutputPolicy::PreserveEvidence,
                }
            },
        )?;
        assert_eq!(result.envelope.request_id, "original-request");
        assert_eq!(result.envelope.data, Some(data));
    }
    Ok(())
}

#[test]
fn page_write_dispatch_preserves_exact_arguments_request_identity_and_receipts()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::confluence_pages::command::{self, PageWriteArgs};
    let mut registry = catalog::local_registry()?;
    command::register_candidates(&mut registry)?;
    let control = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        10_000,
    )?;
    let options = crate::profiles::InvocationOptions {
        state_root: None,
        profile: Some("factory"),
        request_id: "original-request",
        max_bytes: 16384,
    };
    for command in [
        craft_provider_confluence::pages::PAGE_CREATE,
        craft_provider_confluence::pages::PAGE_UPDATE,
    ] {
        let input = registry.descriptor(command)?.example.clone();
        let action = Action::Confluence {
            action: PageWriteArgs::decode(command, input.clone())?.into(),
        };
        let action = decode_action(action, input)?;
        let expected = argument_value(&action)?;
        assert_eq!(command_id(&action), command);
        let data = json!({"receipt":{"request_state":"sent_unknown"},"credential_cache_update":{"write_state":"confirmed"},"journal":{"retained":"complete"},"replay_authorized":false});
        let result = execute_page_write_candidate(
            &registry,
            action,
            &[],
            &control,
            &options,
            |args, _, _, options| {
                assert_eq!(args.arguments().ok(), Some(expected));
                CommandResult {
                    envelope: Envelope::success(command, options.request_id.into(), data.clone()),
                    output_policy: crate::output::OutputPolicy::PreserveEvidence,
                }
            },
        )?;
        assert_eq!(result.envelope.request_id, "original-request");
        assert_eq!(result.envelope.data, Some(data));
        assert!(matches!(
            result.output_policy,
            crate::output::OutputPolicy::PreserveEvidence
        ));
    }
    Ok(())
}

#[test]
fn confluence_dispatch_preserves_typed_filters_request_identity_and_complete_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::confluence_reads::command::{self, ConfluenceReadArgs};
    let mut registry = catalog::local_registry()?;
    command::register_candidates(&mut registry)?;
    let control = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        10_000,
    )?;
    let options = crate::profiles::InvocationOptions {
        state_root: None,
        profile: Some("factory"),
        request_id: "original-request",
        max_bytes: 16384,
    };
    for command in [
        "confluence.space.get",
        "confluence.space.list",
        "confluence.page.get",
        "confluence.page.list",
        "confluence.page.search",
    ] {
        let input = registry.descriptor(command)?.example.clone();
        let action = Action::Confluence {
            action: ConfluenceReadArgs::decode(command, input.clone())?.into(),
        };
        let action = decode_action(action, input)?;
        let expected = argument_value(&action)?;
        assert_eq!(command_id(&action), command);
        let data = json!({"items":[],"cached":false,"credential_cache_update":{"write_state":"confirmed"},"retained":"complete evidence"});
        let result = execute_confluence_candidate(
            &registry,
            action,
            &["id".into()],
            &control,
            &options,
            |args, fields, _, options| {
                assert_eq!(args.arguments().ok(), Some(expected));
                assert_eq!(fields, ["id"]);
                let mut envelope = Envelope::failure_with_data(
                    command,
                    options.request_id.into(),
                    CraftError::new(ErrorCode::PolicyDenied, "Captured policy changed."),
                    data.clone(),
                );
                envelope.warnings.push("Retain the renewal receipt.".into());
                CommandResult {
                    envelope,
                    output_policy: crate::output::OutputPolicy::PreserveEvidence,
                }
            },
        )?;
        assert_eq!(result.envelope.data, Some(data));
        assert_eq!(result.envelope.request_id, "original-request");
        assert_eq!(result.envelope.warnings, ["Retain the renewal receipt."]);
        assert!(matches!(
            result.output_policy,
            crate::output::OutputPolicy::PreserveEvidence
        ));
    }
    Ok(())
}

#[test]
fn recovery_dispatch_preserves_original_input_and_complete_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::args::OperationAction;
    let original = json!({"command":"jira.issue.create","target":{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"project","id":"100"},"input":{"issue_type_id":"101","summary":"Reviewed"},"selection":craft_core::contracts::Selection::default(),"preconditions":null});
    let input = json!({"operation_id":"recover-create","original":original,"candidate_issue_ids":["10001"]});
    let action = Action::Operation {
        action: OperationAction::Reconcile(Box::new(serde_json::from_value(input.clone())?)),
    };
    assert_eq!(command_id(&action), "operation.reconcile");
    assert_eq!(
        argument_value(&decode_action(action, input.clone())?)?,
        input
    );
    let mut registry = crate::catalog::local_registry()?;
    crate::issue_recovery::command::register_candidate(&mut registry)?;
    let descriptor = registry.descriptor("operation.reconcile")?;
    registry.validate("operation.reconcile", &descriptor.example)?;
    let control = InvocationControl::new(
        std::sync::Arc::new(craft_runtime::control::SystemClock::default()),
        craft_core::control::CancellationToken::default(),
        10_000,
    )?;
    let options = crate::profiles::InvocationOptions {
        state_root: None,
        profile: Some("factory"),
        request_id: "recover-request",
        max_bytes: 262_144,
    };
    let data = json!({"initial_record":{"retained":"initial"},"latest_record":{"retained":"concurrent"},"observations":[],"credential_cache_update":{"write_state":"confirmed"},"replay_authorized":false});
    let result = execute_recovery_candidate(
        &registry,
        Action::Operation {
            action: OperationAction::Reconcile(Box::new(serde_json::from_value(input.clone())?)),
        },
        &["assessment".into()],
        &control,
        &options,
        |args, fields, _, _| {
            assert_eq!(serde_json::to_value(args).ok(), Some(input));
            assert_eq!(fields, ["assessment"]);
            let mut envelope = Envelope::failure_with_data(
                "operation.reconcile",
                "recover-request".into(),
                CraftError::new(ErrorCode::VersionConflict, "Journal changed."),
                data.clone(),
            );
            envelope.warnings.push("Retain both records.".into());
            CommandResult {
                envelope,
                output_policy: crate::output::OutputPolicy::PreserveEvidence,
            }
        },
    )?;
    assert_eq!(result.envelope.data, Some(data));
    assert_eq!(result.envelope.warnings, ["Retain both records."]);
    assert_eq!(result.envelope.request_id, "recover-request");
    assert!(matches!(
        result.output_policy,
        crate::output::OutputPolicy::PreserveEvidence
    ));
    Ok(())
}

#[test]
fn action_dispatch_preserves_assignment_and_cascade_intent_with_complete_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::args::{JiraAction, JiraIssueAction};
    use crate::issue_mutation::command::IssueMutationArgs;
    for (command, cascade) in [
        ("jira.issue.assign", false),
        ("jira.issue.unassign", false),
        ("jira.issue.delete", false),
        ("jira.issue.delete", true),
    ] {
        let mut input = json!({"operation_id":"issue-action","apply":true,"issue":"10000","expected":{"id":"10000","key":"ED-7","project_id":"100","issue_type_id":"101","status_id":"102","updated":"2026-11-10T01:00:00.000+0000"}});
        if command == "jira.issue.delete" {
            input["subtasks"] = if cascade {
                json!({"mode":"delete_all","expected_visible_ids":["10001"]})
            } else {
                json!({"mode":"refuse"})
            };
        } else {
            input["expected_assignee"] = json!({"kind":"unassigned"});
        }
        if command == "jira.issue.assign" {
            input["account_id"] = json!("account-one");
        }
        let action = Action::Jira {
            action: JiraAction::Issue {
                action: match command {
                    "jira.issue.assign" => {
                        JiraIssueAction::Assign(Box::new(serde_json::from_value(input.clone())?))
                    }
                    "jira.issue.unassign" => {
                        JiraIssueAction::Unassign(Box::new(serde_json::from_value(input.clone())?))
                    }
                    _ => JiraIssueAction::Delete(Box::new(serde_json::from_value(input.clone())?)),
                },
            },
        };
        let action = decode_action(action, input.clone())?;
        assert_eq!(command_id(&action), command);
        assert_eq!(argument_value(&action)?["expected"], input["expected"]);
        let mut registry = catalog::local_registry()?;
        crate::issue_mutation::command::register_candidates(&mut registry)?;
        let control = InvocationControl::new(
            Arc::new(SystemClock::default()),
            CancellationToken::default(),
            10_000,
        )?;
        let options = crate::profiles::InvocationOptions {
            state_root: None,
            profile: Some("factory"),
            request_id: "original-request",
            max_bytes: 16_384,
        };
        let data = json!({"receipt":{"operation_id":"issue-action","request_state":"sent_unknown","confirmed_resources":[]},"credential_cache_update":{"write_state":"confirmed"},"prior_record":{"retained":"evidence"}});
        let result = execute_mutation_candidate(
            &registry,
            action,
            &[],
            &control,
            &options,
            |args, _, _, options| {
                assert_eq!(args.operation().command(), command);
                match args {
                IssueMutationArgs::Assign(args)=>assert_eq!(args.account_id,"account-one"),
                IssueMutationArgs::Unassign(args)=>assert!(args.expected_assignee.is_some()),
                IssueMutationArgs::Delete(args)=>assert_eq!(matches!(args.subtasks,Some(craft_provider_jira::issue_context::mutation::actions::SubtaskDeletion::DeleteAll{..})),cascade),
                _=>panic!("non-action intent"),
            }
                let mut error =
                    CraftError::new(ErrorCode::WriteOutcomeUnknown, "Retain operation evidence.");
                error.details.write_state = craft_core::contracts::RequestState::SentUnknown;
                let mut envelope = Envelope::failure_with_data(
                    command,
                    options.request_id.into(),
                    error,
                    data.clone(),
                );
                envelope.warnings.push("Retained warning.".into());
                CommandResult {
                    envelope,
                    output_policy: crate::output::OutputPolicy::PreserveEvidence,
                }
            },
        )?;
        assert_eq!(result.envelope.data, Some(data));
        assert_eq!(result.envelope.request_id, "original-request");
        assert_eq!(result.envelope.warnings, vec!["Retained warning."]);
        assert_eq!(
            crate::output::encode(result.envelope, 16_384, result.output_policy)?.exit,
            ErrorCode::WriteOutcomeUnknown.exit_code()
        );
    }
    Ok(())
}

#[test]
fn mutation_dispatch_preserves_decoded_intent_and_complete_recovery_result()
-> Result<(), Box<dyn std::error::Error>> {
    for command in ["jira.issue.update", "jira.transition.apply"] {
        let mut input = json!({"operation_id":"issue-change","apply":true,"issue":"10000","expected":{"id":"10000","key":"ED-7","project_id":"100","issue_type_id":"101","status_id":"102","updated":"2026-11-10T01:00:00.000+0000"},"changes":{"description":null}});
        if command.ends_with("apply") {
            input["transition_id"] = json!("31");
            input["expected_status_id"] = json!("103");
        }
        let action = if command.ends_with("update") {
            Action::Jira {
                action: crate::args::JiraAction::Issue {
                    action: crate::args::JiraIssueAction::Update(Box::new(serde_json::from_value(
                        input.clone(),
                    )?)),
                },
            }
        } else {
            Action::Jira {
                action: crate::args::JiraAction::Transition {
                    action: crate::args::JiraTransitionAction::Apply(Box::new(
                        serde_json::from_value(input.clone())?,
                    )),
                },
            }
        };
        let action = decode_action(action, input)?;
        assert_eq!(command_id(&action), command);
        assert_eq!(argument_value(&action)?["expected"]["project_id"], "100");
        let mut registry = catalog::local_registry()?;
        crate::issue_mutation::command::register_candidates(&mut registry)?;
        let control = InvocationControl::new(
            Arc::new(SystemClock::default()),
            CancellationToken::default(),
            10_000,
        )?;
        let options = crate::profiles::InvocationOptions {
            state_root: None,
            profile: Some("factory"),
            request_id: "original-request",
            max_bytes: 16_384,
        };
        let data = json!({"receipt":{"operation_id":"issue-change","request_state":"sent_unknown","confirmed_resources":[]},"credential_cache_update":{"write_state":"confirmed"},"prior_record":{"retained":"evidence"}});
        let result = execute_mutation_candidate(
            &registry,
            action,
            &["http_status".into()],
            &control,
            &options,
            |args, fields, _, options| {
                assert_eq!(args.operation().command(), command);
                assert_eq!(fields, ["http_status"]);
                let mut error =
                    CraftError::new(ErrorCode::WriteOutcomeUnknown, "Retain the operation.");
                error.details.write_state = craft_core::contracts::RequestState::SentUnknown;
                let mut envelope = Envelope::failure_with_data(
                    command,
                    options.request_id.into(),
                    error,
                    data.clone(),
                );
                envelope.warnings.push("Retained warning.".into());
                CommandResult {
                    envelope,
                    output_policy: crate::output::OutputPolicy::PreserveEvidence,
                }
            },
        )?;
        assert_eq!(result.envelope.request_id, "original-request");
        assert_eq!(result.envelope.data, Some(data));
        assert_eq!(result.envelope.warnings, vec!["Retained warning."]);
        let encoded = crate::output::encode(result.envelope, 16_384, result.output_policy)?;
        assert_eq!(encoded.exit, ErrorCode::WriteOutcomeUnknown.exit_code());
    }
    Ok(())
}

#[test]
fn permission_dispatch_retains_requested_context_unknown_decisions_and_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    crate::permission_reads::command::register_candidate(&mut registry)?;
    let action = Action::Jira {
        action: crate::args::JiraAction::Permission {
            action: crate::args::JiraPermissionAction::Check(serde_json::from_value(
                json!({"permissions":["EDIT_ISSUES"]}),
            )?),
        },
    };
    let action = decode_action(
        action,
        json!({"permissions":["EDIT_ISSUES","plugin:unknown"],"project":"ED","limit":3}),
    )?;
    assert_eq!(command_id(&action), "jira.permission.check");
    assert_eq!(argument_value(&action)?["project"], "ED");
    let control = InvocationControl::new(
        std::sync::Arc::new(craft_runtime::control::SystemClock::default()),
        craft_core::control::CancellationToken::default(),
        1000,
    )?;
    let options = crate::profiles::InvocationOptions {
        state_root: None,
        profile: None,
        request_id: "original-request",
        max_bytes: 16_384,
    };
    let data = json!({"context":{"kind":"project","requested_selector":"ED"},"items":[{"key":"plugin:unknown","returned":false,"have_permission":null}],"credential_cache_update":{"grant":{"id":"gr_00000000000000000000000000000000"},"write_state":"confirmed"}});
    let result = execute_with_candidates(
        &registry,
        action,
        &["name".into()],
        &control,
        &options,
        false,
        (
            |_, _, _, _| unreachable!("Create cannot receive permission arguments"),
            |_, _, _, _| unreachable!("Project cannot receive permission arguments"),
            |_, _, _, _| unreachable!("Issue cannot receive permission arguments"),
            |_, _, _, _| unreachable!("Fields cannot receive permission arguments"),
            |_, _, _, _| unreachable!("Metadata cannot receive permission arguments"),
            |args, fields, _, options| {
                assert_eq!(args.permissions, ["EDIT_ISSUES", "plugin:unknown"]);
                assert_eq!(args.project.as_deref(), Some("ED"));
                assert!(args.issue.is_none());
                assert_eq!(args.limit, 3);
                assert_eq!(fields, ["name"]);
                let mut envelope = Envelope::failure_with_data(
                    "jira.permission.check",
                    options.request_id.into(),
                    CraftError::new(ErrorCode::ItemTooLarge, "Complete permission retained."),
                    data.clone(),
                );
                envelope.page = Some(craft_core::contracts::Page {
                    returned: 1,
                    has_more: true,
                    next_cursor: Some("cur_00000000000000000000000000000000".into()),
                    truncated: true,
                    stop_reason: "item_too_large".into(),
                    consistency: "live".into(),
                });
                envelope.warnings.push("Retain recovery evidence.".into());
                CommandResult {
                    envelope,
                    output_policy: crate::output::OutputPolicy::PreserveEvidence,
                }
            },
        ),
    )?;
    assert_eq!(result.envelope.request_id, "original-request");
    assert_eq!(result.envelope.data, Some(data));
    assert_eq!(result.envelope.warnings, ["Retain recovery evidence."]);
    let output = crate::output::encode(result.envelope, 16_384, result.output_policy)?;
    let value: Value = serde_json::from_slice(&output.bytes)?;
    assert!(value["data"]["items"][0]["have_permission"].is_null());
    assert_eq!(
        value["page"]["next_cursor"],
        "cur_00000000000000000000000000000000"
    );
    Ok(())
}

#[test]
fn metadata_dispatch_preserves_issue_filters_and_complete_observation_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::args::{JiraAction, JiraMetadataAction, JiraTransitionAction};
    use crate::issue_context::command::{EditMetadataArgs, IssueContextArgs, TransitionListArgs};
    let control = InvocationControl::new(
        std::sync::Arc::new(craft_runtime::control::SystemClock::default()),
        craft_core::control::CancellationToken::default(),
        1000,
    )?;
    let options = crate::profiles::InvocationOptions {
        state_root: None,
        profile: None,
        request_id: "original-request",
        max_bytes: 16_384,
    };
    for transitions in [false, true] {
        let mut registry = Registry::default();
        crate::issue_context::command::register_candidates(&mut registry)?;
        let action = Action::Jira {
            action: if transitions {
                JiraAction::Transition {
                    action: JiraTransitionAction::List(
                        serde_json::from_value::<TransitionListArgs>(json!({"issue":"ED-1"}))?,
                    ),
                }
            } else {
                JiraAction::Metadata {
                    action: JiraMetadataAction::Edit(serde_json::from_value::<EditMetadataArgs>(
                        json!({"issue":"ED-1"}),
                    )?),
                }
            },
        };
        let input = if transitions {
            json!({"issue":"OLD-1","transition_id":"31","include_unavailable":true,"sort_by_status":true,"limit":3})
        } else {
            json!({"issue":"OLD-1","limit":3})
        };
        let action = decode_action(action, input.clone())?;
        let command = command_id(&action);
        let raw = argument_value(&action)?;
        assert_eq!(raw["issue"], "OLD-1");
        let data = json!({"items":[],"issue_observation":{"requested_issue":"OLD-1"},"credential_cache_update":{"grant":{"id":"gr_00000000000000000000000000000000"},"write_state":"confirmed"}});
        let result = execute_with_candidates(
            &registry,
            action,
            &["name".into()],
            &control,
            &options,
            false,
            (
                |_, _, _, _| unreachable!("Create cannot receive metadata arguments"),
                |_, _, _, _| unreachable!("Project cannot receive metadata arguments"),
                |_, _, _, _| unreachable!("Issue reads cannot receive metadata arguments"),
                |_, _, _, _| unreachable!("Field reads cannot receive metadata arguments"),
                |args, fields, _, options| {
                    match args {
                        IssueContextArgs::Edit(args) => {
                            assert_eq!(args.issue, "OLD-1");
                            assert_eq!(args.limit, 3);
                        }
                        IssueContextArgs::Transitions(args) => {
                            assert_eq!(args.issue, "OLD-1");
                            assert_eq!(args.limit, 3);
                            assert_eq!(args.transition_id.as_deref(), Some("31"));
                            assert!(args.include_unavailable && args.sort_by_status);
                        }
                    }
                    assert_eq!(fields, ["name"]);
                    let mut envelope = Envelope::failure_with_data(
                        command,
                        options.request_id.into(),
                        CraftError::new(ErrorCode::ItemTooLarge, "Complete metadata retained."),
                        data.clone(),
                    );
                    envelope.page = Some(craft_core::contracts::Page {
                        returned: 0,
                        has_more: true,
                        next_cursor: Some("cur_00000000000000000000000000000000".into()),
                        truncated: true,
                        stop_reason: "item_too_large".into(),
                        consistency: "live".into(),
                    });
                    envelope.warnings.push("Retain recovery evidence.".into());
                    CommandResult {
                        envelope,
                        output_policy: crate::output::OutputPolicy::PreserveEvidence,
                    }
                },
                |_, _, _, _| unreachable!("Permission handler cannot receive metadata arguments"),
            ),
        )?;
        assert_eq!(result.envelope.command, command);
        assert_eq!(result.envelope.request_id, "original-request");
        assert_eq!(result.envelope.data, Some(data));
        assert_eq!(result.envelope.warnings, ["Retain recovery evidence."]);
        let output = crate::output::encode(result.envelope, 16_384, result.output_policy)?;
        let value: Value = serde_json::from_slice(&output.bytes)?;
        assert_eq!(
            value["page"]["next_cursor"],
            "cur_00000000000000000000000000000000"
        );
        assert_eq!(
            value["data"]["issue_observation"]["requested_issue"],
            "OLD-1"
        );
    }
    Ok(())
}

#[test]
fn field_candidate_dispatch_keeps_typed_filters_and_complete_recovery_envelopes()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::args::{JiraAction, JiraFieldAction};
    use crate::field_reads::command::{FieldListArgs, FieldReadArgs, FieldSearchArgs};
    let control = InvocationControl::new(
        std::sync::Arc::new(craft_runtime::control::SystemClock::default()),
        craft_core::control::CancellationToken::default(),
        1000,
    )?;
    let options = crate::profiles::InvocationOptions {
        state_root: None,
        profile: None,
        request_id: "original-request",
        max_bytes: 16_384,
    };
    for search in [false, true] {
        let mut registry = Registry::default();
        crate::field_reads::command::register_candidates(&mut registry)?;
        let action = Action::Jira {
            action: JiraAction::Field {
                action: if search {
                    JiraFieldAction::Search(serde_json::from_value::<FieldSearchArgs>(json!({}))?)
                } else {
                    JiraFieldAction::List(serde_json::from_value::<FieldListArgs>(json!({}))?)
                },
            },
        };
        let input = if search {
            json!({"ids":["customfield_1"],"project_ids":[1],"query":"literal,query","limit":3})
        } else {
            json!({"limit":3})
        };
        let action = decode_action(action, input)?;
        let command = command_id(&action);
        let data = json!({"items":[],"credential_cache_update":{"grant":{"id":"gr_00000000000000000000000000000000"},"write_state":"confirmed"}});
        let result = execute_with_candidates(
            &registry,
            action,
            &["name".into()],
            &control,
            &options,
            false,
            (
                |_, _, _, _| unreachable!("Create cannot receive field arguments"),
                |_, _, _, _| unreachable!("Project cannot receive field arguments"),
                |_, _, _, _| unreachable!("Issue cannot receive field arguments"),
                |args, fields, _, options| {
                    match args {
                        FieldReadArgs::List(args) => assert_eq!(args.limit, 3),
                        FieldReadArgs::Search(args) => {
                            assert_eq!(args.limit, 3);
                            assert_eq!(args.ids, ["customfield_1"]);
                            assert_eq!(args.project_ids, [1]);
                            assert_eq!(args.query.as_deref(), Some("literal,query"));
                        }
                    }
                    assert_eq!(fields, ["name"]);
                    let mut envelope = Envelope::failure_with_data(
                        command,
                        options.request_id.into(),
                        CraftError::new(ErrorCode::ItemTooLarge, "Complete field retained."),
                        data.clone(),
                    );
                    envelope.page = Some(craft_core::contracts::Page {
                        returned: 0,
                        has_more: true,
                        next_cursor: Some("cur_00000000000000000000000000000000".into()),
                        truncated: true,
                        stop_reason: "item_too_large".into(),
                        consistency: "live".into(),
                    });
                    envelope.warnings.push("Retain recovery evidence.".into());
                    CommandResult {
                        envelope,
                        output_policy: crate::output::OutputPolicy::PreserveEvidence,
                    }
                },
                |_, _, _, _| unreachable!("Metadata handler cannot receive field arguments"),
                |_, _, _, _| unreachable!("Permission handler cannot receive other candidates"),
            ),
        )?;
        assert_eq!(result.envelope.command, command);
        assert_eq!(result.envelope.request_id, "original-request");
        assert_eq!(result.envelope.data, Some(data));
        assert_eq!(result.envelope.warnings, ["Retain recovery evidence."]);
        let bytes = crate::output::encode(result.envelope, 16_384, result.output_policy)?.bytes;
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?["page"]["next_cursor"],
            "cur_00000000000000000000000000000000"
        );
    }
    Ok(())
}
use craft_core::contracts::RequestState;
use craft_core::control::CancellationToken;
use craft_runtime::control::SystemClock;
use std::sync::Arc;

#[test]
fn create_dispatch_preserves_complete_partial_receipts_and_original_request_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = catalog::local_registry()?;
    crate::issue_create::command::register_candidate(&mut registry)?;
    let args = serde_json::from_value(
        json!({"operation_id":"create-report","apply":true,"issue_type_id":"10001","summary":"Example"}),
    )?;
    let action = Action::Jira {
        action: crate::args::JiraAction::Issue {
            action: crate::args::JiraIssueAction::Create(args),
        },
    };
    let control = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        10_000,
    )?;
    let options = crate::profiles::InvocationOptions {
        state_root: None,
        profile: Some("factory"),
        request_id: "original-request",
        max_bytes: 16_384,
    };
    let receipt = json!({"receipt":{"operation_id":"create-report","request_state":"confirmed","confirmed_resources":[{"provider":"jira","deployment":"cloud","tenant":"00000000-1111-2222-3333-444444444444","resource_type":"issue","id":"10003"}]},"credential_cache_update":{"grant":{"id":"gr_00000000000000000000000000000000"},"write_state":"confirmed","cleanup_pending":true}});
    let result = execute_with_candidates(
        &registry,
        action,
        &["issue_key".into()],
        &control,
        &options,
        false,
        (
            |args, fields, _, options| {
                assert_eq!(args.operation_id, "create-report");
                assert_eq!(fields, ["issue_key"]);
                let mut error = CraftError::new(
                    ErrorCode::PartialApply,
                    "Created issue with a secondary failure.",
                );
                error.details.write_state = RequestState::Confirmed;
                let mut envelope = Envelope::failure_with_data(
                    "jira.issue.create",
                    options.request_id.into(),
                    error,
                    receipt.clone(),
                );
                envelope.warnings.push("Retain this receipt.".into());
                CommandResult {
                    envelope,
                    output_policy: crate::output::OutputPolicy::PreserveEvidence,
                }
            },
            |_, _, _, _| unreachable!("Project handler must not receive create arguments"),
            |_, _, _, _| unreachable!("Issue read handler must not receive create arguments"),
            |_, _, _, _| unreachable!("Field handler cannot receive other candidates"),
            |_, _, _, _| unreachable!("Metadata handler cannot receive other candidates"),
            |_, _, _, _| unreachable!("Permission handler cannot receive other candidates"),
        ),
    )?;
    assert_eq!(result.envelope.data, Some(receipt));
    assert_eq!(result.envelope.warnings, vec!["Retain this receipt."]);
    assert_eq!(result.envelope.request_id, "original-request");
    assert!(matches!(
        result.output_policy,
        crate::output::OutputPolicy::PreserveEvidence
    ));
    let output = crate::output::encode(result.envelope, 16_384, result.output_policy)?;
    let value: Value = serde_json::from_slice(&output.bytes)?;
    assert_eq!(
        value["data"]["receipt"]["confirmed_resources"][0]["id"],
        "10003"
    );
    assert_eq!(output.exit, ErrorCode::PartialApply.exit_code());
    Ok(())
}

#[test]
fn project_list_dispatch_preserves_recovery_cursor_cache_effects_and_request_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let mut registry = catalog::local_registry()?;
    crate::project_list::command::register_candidate(&mut registry)?;
    let action = Action::Jira {
        action: crate::args::JiraAction::Project {
            action: crate::args::JiraProjectAction::List(serde_json::from_value(json!({}))?),
        },
    };
    let action = decode_action(action, json!({"limit":7,"query":"literal"}))?;
    assert_eq!(command_id(&action), "jira.project.list");
    assert_eq!(argument_value(&action)?["limit"], 7);
    let control = InvocationControl::new(
        Arc::new(SystemClock::default()),
        CancellationToken::default(),
        10_000,
    )?;
    let options = crate::profiles::InvocationOptions {
        state_root: None,
        profile: Some("factory"),
        request_id: "original-request",
        max_bytes: 16_384,
    };
    let data = json!({"items":[],"credential_cache_update":{"grant":{"id":"gr_00000000000000000000000000000000"},"write_state":"confirmed","cleanup_pending":false}});
    let result = execute_with_candidates(
        &registry,
        action,
        &["name".into()],
        &control,
        &options,
        false,
        (
            |_, _, _, _| unreachable!("Create must not receive list arguments"),
            |args, fields, _, options| {
                let crate::args::JiraProjectAction::List(args) = args else {
                    unreachable!("Project list arguments required");
                };
                assert_eq!(args.limit, 7);
                assert_eq!(fields, ["name"]);
                let mut envelope = Envelope::failure_with_data(
                    "jira.project.list",
                    options.request_id.into(),
                    CraftError::new(ErrorCode::ItemTooLarge, "First item retained."),
                    data.clone(),
                );
                envelope.page = Some(craft_core::contracts::Page {
                    returned: 0,
                    has_more: true,
                    next_cursor: Some("cur_00000000000000000000000000000000".into()),
                    truncated: true,
                    stop_reason: "item_too_large".into(),
                    consistency: "live".into(),
                });
                envelope.warnings.push("Retain the cursor.".into());
                CommandResult {
                    envelope,
                    output_policy: crate::output::OutputPolicy::PreserveEvidence,
                }
            },
            |_, _, _, _| unreachable!("Issue read handler must not receive project-list arguments"),
            |_, _, _, _| unreachable!("Field handler cannot receive other candidates"),
            |_, _, _, _| unreachable!("Metadata handler cannot receive other candidates"),
            |_, _, _, _| unreachable!("Permission handler cannot receive other candidates"),
        ),
    )?;
    assert_eq!(result.envelope.data, Some(data));
    assert_eq!(result.envelope.request_id, "original-request");
    assert_eq!(result.envelope.warnings, vec!["Retain the cursor."]);
    let encoded = crate::output::encode(result.envelope, 16_384, result.output_policy)?;
    let value: Value = serde_json::from_slice(&encoded.bytes)?;
    assert_eq!(
        value["page"]["next_cursor"],
        "cur_00000000000000000000000000000000"
    );
    assert_eq!(
        value["data"]["credential_cache_update"]["write_state"],
        "confirmed"
    );
    assert_eq!(encoded.exit, 10);
    Ok(())
}

#[test]
fn issue_read_dispatch_keeps_operation_arguments_and_complete_result_envelopes()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::args::{JiraAction, JiraIssueAction};
    use crate::issue_reads::command::{IssueReadArgs, register_candidates};
    for command in ["jira.issue.get", "jira.issue.search"] {
        let mut registry = catalog::local_registry()?;
        register_candidates(&mut registry)?;
        let action = Action::Jira {
            action: JiraAction::Issue {
                action: if command.ends_with("get") {
                    JiraIssueAction::Get(serde_json::from_value(json!({"issue":"old"}))?)
                } else {
                    JiraIssueAction::Search(serde_json::from_value(json!({"jql":"old"}))?)
                },
            },
        };
        let input = if command.ends_with("get") {
            json!({"issue":"ED-7"})
        } else {
            json!({"jql":"project = ED","reconcile_issue_ids":["7"],"limit":1})
        };
        let action = decode_action(action, input)?;
        assert_eq!(command_id(&action), command);
        let value = argument_value(&action)?;
        assert_eq!(
            value[if command.ends_with("get") {
                "issue"
            } else {
                "jql"
            }],
            if command.ends_with("get") {
                json!("ED-7")
            } else {
                json!("project = ED")
            }
        );
        let control = InvocationControl::new(
            Arc::new(SystemClock::default()),
            CancellationToken::default(),
            10_000,
        )?;
        let options = crate::profiles::InvocationOptions {
            state_root: None,
            profile: Some("factory"),
            request_id: "original-request",
            max_bytes: 16_384,
        };
        let data = json!({"credential_cache_update":{"grant":{"id":"gr_00000000000000000000000000000000"},"write_state":"confirmed","cleanup_pending":false}});
        let result = execute_with_candidates(
            &registry,
            action,
            &["summary".into()],
            &control,
            &options,
            false,
            (
                |_, _, _, _| unreachable!("Create cannot receive issue reads"),
                |_, _, _, _| unreachable!("Project listing cannot receive issue reads"),
                |args, fields, _, options| {
                    match args {
                        IssueReadArgs::Get(args) => assert_eq!(args.issue, "ED-7"),
                        IssueReadArgs::Search(args) => {
                            assert_eq!(args.reconcile_issue_ids, vec!["7"]);
                            assert_eq!(args.limit, 1);
                        }
                    }
                    assert_eq!(fields, ["summary"]);
                    let mut envelope = Envelope::failure_with_data(
                        command,
                        options.request_id.into(),
                        CraftError::new(ErrorCode::ItemTooLarge, "Selected item retained."),
                        data.clone(),
                    );
                    if command.ends_with("search") {
                        envelope.page = Some(craft_core::contracts::Page {
                            returned: 0,
                            has_more: true,
                            next_cursor: Some("cur_00000000000000000000000000000000".into()),
                            truncated: true,
                            stop_reason: "item_too_large".into(),
                            consistency: "live".into(),
                        });
                    }
                    envelope.warnings.push("Retain recovery evidence.".into());
                    CommandResult {
                        envelope,
                        output_policy: crate::output::OutputPolicy::PreserveEvidence,
                    }
                },
                |_, _, _, _| unreachable!("Field handler cannot receive other candidates"),
                |_, _, _, _| unreachable!("Metadata handler cannot receive other candidates"),
                |_, _, _, _| unreachable!("Permission handler cannot receive other candidates"),
            ),
        )?;
        assert_eq!(result.envelope.command, command);
        assert_eq!(result.envelope.request_id, "original-request");
        assert_eq!(result.envelope.data, Some(data));
        assert_eq!(result.envelope.warnings, vec!["Retain recovery evidence."]);
        let output = crate::output::encode(result.envelope, 16_384, result.output_policy)?;
        let value: Value = serde_json::from_slice(&output.bytes)?;
        assert_eq!(
            value["data"]["credential_cache_update"]["write_state"],
            "confirmed"
        );
        assert_eq!(output.exit, ErrorCode::ItemTooLarge.exit_code());
        if command.ends_with("search") {
            assert_eq!(
                value["page"]["next_cursor"],
                "cur_00000000000000000000000000000000"
            );
        }
    }
    Ok(())
}

#[test]
fn project_context_dispatch_preserves_decoded_target_selection_and_complete_receipts()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::args::{JiraAction, JiraProjectAction};
    for command in ["jira.project.get", "jira.project.statuses"] {
        let mut registry = catalog::local_registry()?;
        crate::project_context::command::register_candidates(&mut registry)?;
        let action = Action::Jira {
            action: JiraAction::Project {
                action: if command.ends_with("get") {
                    JiraProjectAction::Get(serde_json::from_value(json!({}))?)
                } else {
                    JiraProjectAction::Statuses(serde_json::from_value(json!({}))?)
                },
            },
        };
        let action = decode_action(
            action,
            if command.ends_with("get") {
                json!({"project":"10000"})
            } else {
                json!({"project":"10000","limit":7})
            },
        )?;
        assert_eq!(command_id(&action), command);
        assert_eq!(argument_value(&action)?["project"], "10000");
        let control = InvocationControl::new(
            Arc::new(SystemClock::default()),
            CancellationToken::default(),
            10_000,
        )?;
        let options = crate::profiles::InvocationOptions {
            state_root: None,
            profile: Some("factory"),
            request_id: "original-request",
            max_bytes: 16_384,
        };
        let data = json!({"credential_cache_update":{"grant":{"id":"gr_00000000000000000000000000000000"},"write_state":"confirmed","cleanup_pending":false}});
        let result = execute_with_candidates(
            &registry,
            action,
            &["name".into()],
            &control,
            &options,
            false,
            (
                |_, _, _, _| unreachable!("Create cannot receive project context"),
                |action, fields, _, options| {
                    match action {
                        JiraProjectAction::Get(args) => {
                            assert_eq!(args.project.as_deref(), Some("10000"))
                        }
                        JiraProjectAction::Statuses(args) => {
                            assert_eq!(args.project.as_deref(), Some("10000"));
                            assert_eq!(args.limit, 7);
                        }
                        JiraProjectAction::List(_) => {
                            unreachable!("List cannot receive project context")
                        }
                    }
                    assert_eq!(fields, ["name"]);
                    let mut envelope = Envelope::failure_with_data(
                        command,
                        options.request_id.into(),
                        CraftError::new(ErrorCode::ItemTooLarge, "Retained selected item."),
                        data.clone(),
                    );
                    if command.ends_with("statuses") {
                        envelope.page = Some(craft_core::contracts::Page {
                            returned: 0,
                            has_more: true,
                            next_cursor: Some("cur_00000000000000000000000000000000".into()),
                            truncated: true,
                            stop_reason: "item_too_large".into(),
                            consistency: "live".into(),
                        });
                    }
                    envelope.warnings.push("Retain recovery evidence.".into());
                    CommandResult {
                        envelope,
                        output_policy: crate::output::OutputPolicy::PreserveEvidence,
                    }
                },
                |_, _, _, _| unreachable!("Issue cannot receive project context"),
                |_, _, _, _| unreachable!("Field handler cannot receive other candidates"),
                |_, _, _, _| unreachable!("Metadata handler cannot receive other candidates"),
                |_, _, _, _| unreachable!("Permission handler cannot receive other candidates"),
            ),
        )?;
        assert_eq!(result.envelope.command, command);
        assert_eq!(result.envelope.request_id, "original-request");
        assert_eq!(result.envelope.data, Some(data));
        assert_eq!(result.envelope.warnings, vec!["Retain recovery evidence."]);
        let output = crate::output::encode(result.envelope, 16_384, result.output_policy)?;
        let value: Value = serde_json::from_slice(&output.bytes)?;
        assert_eq!(
            value["data"]["credential_cache_update"]["write_state"],
            "confirmed"
        );
        assert_eq!(output.exit, ErrorCode::ItemTooLarge.exit_code());
        if command.ends_with("statuses") {
            assert_eq!(
                value["page"]["next_cursor"],
                "cur_00000000000000000000000000000000"
            );
        }
    }
    Ok(())
}
