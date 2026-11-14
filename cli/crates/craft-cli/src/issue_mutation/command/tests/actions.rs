use super::*;

fn args(command: &str, value: Value) -> Result<IssueMutationArgs, serde_json::Error> {
    match command {
        ASSIGN_ISSUE => serde_json::from_value(value).map(IssueMutationArgs::Assign),
        UNASSIGN_ISSUE => serde_json::from_value(value).map(IssueMutationArgs::Unassign),
        _ => serde_json::from_value(value).map(IssueMutationArgs::Delete),
    }
}
fn parsed(action: CandidateAction) -> Result<IssueMutationArgs, Box<dyn std::error::Error>> {
    let action: crate::args::JiraAction = action.into();
    match action {
        crate::args::JiraAction::Issue {
            action: crate::args::JiraIssueAction::Assign(args),
        } => Ok(IssueMutationArgs::Assign(*args)),
        crate::args::JiraAction::Issue {
            action: crate::args::JiraIssueAction::Unassign(args),
        } => Ok(IssueMutationArgs::Unassign(*args)),
        crate::args::JiraAction::Issue {
            action: crate::args::JiraIssueAction::Delete(args),
        } => Ok(IssueMutationArgs::Delete(*args)),
        _ => Err("expected action arguments".into()),
    }
}

fn arguments(command: &str, cascade: bool) -> Value {
    let mut value =
        json!({"operation_id":"issue-action","apply":true,"issue":"10000","expected":state()});
    if command == DELETE_ISSUE {
        value["subtasks"] = if cascade {
            json!({"mode":"delete_all","expected_visible_ids":["10001"]})
        } else {
            json!({"mode":"refuse"})
        };
    } else {
        value["expected_assignee"] = json!({"kind":"unassigned"});
    }
    if command == ASSIGN_ISSUE {
        value["account_id"] = json!("account-one");
    }
    value
}

#[test]
fn action_candidate_schemas_require_explicit_nonnull_intent_and_closed_objects() -> TestResult {
    let mut registry = Registry::default();
    register_candidates(&mut registry)?;
    for (command, cascade) in [
        (ASSIGN_ISSUE, false),
        (UNASSIGN_ISSUE, false),
        (DELETE_ISSUE, false),
        (DELETE_ISSUE, true),
    ] {
        let descriptor = registry.descriptor(command)?;
        let validator = jsonschema::validator_for(&descriptor.input_schema)?;
        let good = arguments(command, cascade);
        assert!(validator.is_valid(&good));
        registry.validate(command, &good)?;
        let mut required_fields = vec![
            "issue",
            "expected",
            "operation_id",
            if command == DELETE_ISSUE {
                "subtasks"
            } else {
                "expected_assignee"
            },
        ];
        if command == ASSIGN_ISSUE {
            required_fields.push("account_id");
        }
        for field in required_fields {
            let mut missing = good.clone();
            missing.as_object_mut().ok_or("object")?.remove(field);
            assert!(!validator.is_valid(&missing), "missing {field}");
            assert!(registry.validate(command, &missing).is_err());
            let mut null = good.clone();
            null[field] = Value::Null;
            assert!(!validator.is_valid(&null), "null {field}");
            assert!(registry.validate(command, &null).is_err());
        }
        for field in ["account", "name", "deleteSubtasks", "extra"] {
            let mut unknown = good.clone();
            unknown[field] = json!(true);
            assert!(!validator.is_valid(&unknown));
            assert!(registry.validate(command, &unknown).is_err());
        }
        let field = if command == DELETE_ISSUE {
            "subtasks"
        } else {
            "expected_assignee"
        };
        let mut nested = good.clone();
        nested[field]["extra"] = json!(true);
        assert!(!validator.is_valid(&nested));
        assert!(registry.validate(command, &nested).is_err());
    }
    Ok(())
}

#[test]
fn action_flags_and_json_agree_without_synthesizing_assignee_or_subtask_intent() -> TestResult {
    for (command, cascade, verb) in [
        (ASSIGN_ISSUE, false, "assign"),
        (UNASSIGN_ISSUE, false, "unassign"),
        (DELETE_ISSUE, false, "delete"),
        (DELETE_ISSUE, true, "delete"),
    ] {
        let value = arguments(command, cascade);
        let expected = serde_json::to_string(&state())?;
        let mut flags = vec![
            "candidate",
            "issue",
            verb,
            "--operation-id",
            "issue-action",
            "--apply",
            "--issue",
            "10000",
            "--expected",
            &expected,
        ];
        if command == ASSIGN_ISSUE {
            flags.extend(["--account-id", "account-one"]);
        }
        let absent = parsed(CandidateParser::try_parse_from(flags.clone())?.action)?;
        assert!(absent.validate(&[], &options(None)).is_err());
        let intent = serde_json::to_string(
            &value[if command == DELETE_ISSUE {
                "subtasks"
            } else {
                "expected_assignee"
            }],
        )?;
        flags.extend([
            if command == DELETE_ISSUE {
                "--subtasks"
            } else {
                "--expected-assignee"
            },
            &intent,
        ]);
        let flag_args = parsed(CandidateParser::try_parse_from(flags)?.action)?;
        let json_args = args(command, value)?;
        assert_eq!(flag_args.input()?, json_args.input()?);
        flag_args.validate(&[], &options(None))?;
        assert_eq!(
            json_args.operation(),
            if command == ASSIGN_ISSUE {
                Operation::Assign
            } else if command == UNASSIGN_ISSUE {
                Operation::Unassign
            } else if cascade {
                Operation::DeleteAllSubtasks
            } else {
                Operation::Delete
            }
        );
    }
    for (verb, flag, bad) in [
        (
            "unassign",
            "--expected-assignee",
            r#"{"kind":"unassigned","kind":"account"}"#,
        ),
        (
            "delete",
            "--subtasks",
            r#"{"mode":"refuse","mode":"delete_all"}"#,
        ),
    ] {
        assert!(CandidateParser::try_parse_from(["candidate", "issue", verb, flag, bad]).is_err());
    }
    Ok(())
}

#[test]
fn invalid_action_intent_target_apply_projection_and_budget_fail_before_state() -> TestResult {
    let files = Files::new()?;
    let absent = files.root.join("not-created");
    for command in [ASSIGN_ISSUE, UNASSIGN_ISSUE, DELETE_ISSUE] {
        for (field, bad) in [
            ("issue", json!("ED-7")),
            ("expected", json!({})),
            ("apply", json!(false)),
            ("operation_id", json!("")),
            ("cloud_id", json!("bad")),
            (
                if command == DELETE_ISSUE {
                    "subtasks"
                } else {
                    "expected_assignee"
                },
                Value::Null,
            ),
        ] {
            let mut value = arguments(command, false);
            value[field] = bad;
            if let Ok(args) = args(command, value) {
                assert!(
                    !execute(args, &[], &options(Some(&absent)), &control()?)
                        .envelope
                        .ok
                );
            }
            assert!(!absent.exists());
        }
        for (field, bad) in if command == ASSIGN_ISSUE {
            vec![
                ("account_id", json!("-1")),
                ("account_id", json!("person@example.com")),
            ]
        } else if command == UNASSIGN_ISSUE {
            vec![(
                "expected_assignee",
                json!({"kind":"account","account_id":"unknown"}),
            )]
        } else {
            vec![
                (
                    "subtasks",
                    json!({"mode":"delete_all","expected_visible_ids":["10000"]}),
                ),
                (
                    "subtasks",
                    json!({"mode":"delete_all","expected_visible_ids":["10002","10001"]}),
                ),
            ]
        } {
            let mut value = arguments(command, false);
            value[field] = bad;
            assert!(
                !execute(
                    args(command, value)?,
                    &[],
                    &options(Some(&absent)),
                    &control()?
                )
                .envelope
                .ok
            );
            assert!(!absent.exists());
        }
        for fields in [vec!["unknown".into()], vec![]] {
            let mut options = options(Some(&absent));
            if fields.is_empty() {
                options.max_bytes = 512;
            }
            assert!(
                !execute(
                    args(command, arguments(command, false))?,
                    &fields,
                    &options,
                    &control()?
                )
                .envelope
                .ok
            );
            assert!(!absent.exists());
        }
    }
    Ok(())
}

#[test]
fn action_candidates_require_independent_project_allowances_and_capture_the_exact_record()
-> TestResult {
    let files = Files::new()?;
    let store = files.setup()?;
    let control = control()?;
    let project = ResourceRef {
        resource_type: "project".into(),
        id: "100".into(),
        ..site("00000000-1111-2222-3333-444444444444")
    };
    let initial = store.set_issue_mutation_policy(
        "factory",
        IssueMutationPolicyUpdate {
            target: project.clone(),
            expected_actor: "actor-one".into(),
            enabled: true,
            operations: vec![Operation::Edit, Operation::Transition],
            expected_revision: None,
        },
        true,
        &control,
        |_| Ok(()),
    )?;
    let mut revision = initial.revision;
    for (command, cascade, operation) in [
        (ASSIGN_ISSUE, false, Operation::Assign),
        (UNASSIGN_ISSUE, false, Operation::Unassign),
        (DELETE_ISSUE, false, Operation::Delete),
        (DELETE_ISSUE, true, Operation::DeleteAllSubtasks),
    ] {
        assert!(
            PreparedInvocation::prepare(
                args(command, arguments(command, cascade))?,
                &[],
                &options(None),
                &store,
                &control
            )
            .is_err()
        );
        let record = store.set_issue_mutation_policy(
            "factory",
            IssueMutationPolicyUpdate {
                target: project.clone(),
                expected_actor: "actor-one".into(),
                enabled: true,
                operations: vec![operation],
                expected_revision: Some(revision),
            },
            true,
            &control,
            |_| Ok(()),
        )?;
        revision = record.revision;
        let prepared = PreparedInvocation::prepare(
            args(command, arguments(command, cascade))?,
            &[],
            &options(None),
            &store,
            &control,
        )?;
        assert_eq!(prepared.policy, record);
        assert_eq!(prepared.operation, operation);
        let mut wrong = arguments(command, cascade);
        wrong["expected"]["project_id"] = json!("200");
        assert!(
            PreparedInvocation::prepare(
                args(command, wrong)?,
                &[],
                &options(None),
                &store,
                &control
            )
            .is_err()
        );
        let mut wrong_site = arguments(command, cascade);
        wrong_site["cloud_id"] = json!("11111111-2222-3333-4444-555555555555");
        assert!(
            PreparedInvocation::prepare(
                args(command, wrong_site)?,
                &[],
                &options(None),
                &store,
                &control
            )
            .is_err()
        );
        store.set_issue_mutation_policy(
            "factory",
            IssueMutationPolicyUpdate {
                target: project.clone(),
                expected_actor: "actor-one".into(),
                enabled: false,
                operations: vec![operation],
                expected_revision: Some(revision),
            },
            true,
            &control,
            |_| Ok(()),
        )?;
        revision += 1;
        assert!(
            store
                .verify_issue_mutation_policy(&prepared.policy, prepared.operation, &control)
                .is_err()
        );
    }
    Ok(())
}
