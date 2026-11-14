use super::*;
use craft_core::contracts::{GrantRef, Selection};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn body(text: &str) -> Value {
    json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":text}]}]})
}
fn raw_comment() -> Value {
    json!({"id":"101","body":body("Original"),"author":{"accountId":"service-one"},
        "updateAuthor":{"accountId":"human-two"},"updated":"2026-11-11T00:01:00.000+0000",
        "visibility":{"type":"group","identifier":"stable-group","value":"Team"},"jsdPublic":false})
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "200".into(),
    }
}
fn principal() -> MetadataPrincipal {
    MetadataPrincipal {
        grant: GrantRef {
            id: format!("gr_{}", "0".repeat(32)),
        },
        actor: "service-one".into(),
    }
}
fn value(command: &str) -> Result<Value, CraftError> {
    if command == CREATE {
        return Ok(
            json!({"issue_id":"100","body":body("Replacement"),"visibility":{"mode":"project"}}),
        );
    }
    let revision = parse_record(&raw_comment(), "100", true)?.revision;
    let mut input =
        json!({"issue_id":"100","comment_id":"101","expected_revision":revision,"ownership":"own"});
    if command == UPDATE {
        input["body"] = body("Replacement");
    }
    Ok(input)
}
fn operations(
    command: &str,
    value: Value,
) -> Result<(PreparedOperation, PreparedOperation), CraftError> {
    let mut registry = Registry::default();
    register(&mut registry)?;
    let make = |command: &str| {
        registry.prepare(CanonicalRequest {
            command: command.into(),
            target: target(),
            input: value.clone(),
            selection: Selection::default(),
            preconditions: None,
        })
    };
    Ok((make(context_command(command)?)?, make(command)?))
}
fn response(status: u16, body: Value) -> NativeResponse {
    NativeResponse {
        status,
        body,
        request_id: None,
        retry_after: None,
        link_headers: None,
    }
}
fn replies(command: &str) -> Vec<NativeResponse> {
    let project = json!({"id":"100","fields":{"project":{"id":"200"}}});
    let keys = match command {
        CREATE => vec!["BROWSE_PROJECTS", "ADD_COMMENTS"],
        UPDATE => vec!["BROWSE_PROJECTS", "EDIT_OWN_COMMENTS", "EDIT_ALL_COMMENTS"],
        _ => vec![
            "BROWSE_PROJECTS",
            "DELETE_OWN_COMMENTS",
            "DELETE_ALL_COMMENTS",
        ],
    };
    let mut permissions = json!({"permissions":{}});
    for key in keys {
        permissions["permissions"][key] = json!({"key":key,"havePermission":!key.contains("ALL")});
    }
    let rows = if command == CREATE {
        vec![project.clone(), permissions, project]
    } else {
        vec![
            project.clone(),
            raw_comment(),
            permissions,
            raw_comment(),
            project,
        ]
    };
    rows.into_iter().map(|v| response(200, v)).collect()
}
struct TestClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl TestClock {
    fn new() -> Self {
        Self {
            wall: AtomicU64::new(1000),
            mono: AtomicU64::new(100),
        }
    }
}
impl Clock for TestClock {
    fn unix_millis(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn monotonic_millis(&self) -> u64 {
        self.mono.load(Ordering::SeqCst)
    }
}
struct Port {
    digest: String,
    replies: VecDeque<NativeResponse>,
    calls: Vec<NativeCall>,
}
impl Port {
    fn new(op: &PreparedOperation, rows: Vec<NativeResponse>) -> Self {
        Self {
            digest: op.digest().into(),
            replies: rows.into(),
            calls: vec![],
        }
    }
}
impl OperationContext for Port {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, op: &PreparedOperation) -> Result<(), CraftError> {
        if op.digest() == self.digest {
            Ok(())
        } else {
            Err(CraftError::new(ErrorCode::PolicyDenied, "Wrong operation"))
        }
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        self.replies.pop_front().ok_or_else(malformed)
    }
}
#[test]
fn preparation_freezes_exact_native_payload_without_overwriting_omitted_fields() -> TestResult {
    for command in [CREATE, UPDATE, DELETE] {
        let (read, write) = operations(command, value(command)?)?;
        let mut port = Port::new(&read, replies(command));
        let frozen = prepare(
            &read,
            &write,
            &mut port,
            principal(),
            Arc::new(TestClock::new()),
        )?;
        frozen.verify(&write, &principal())?;
        assert_eq!(port.calls.len(), if command == CREATE { 3 } else { 5 });
        assert_eq!(frozen.call.path_parameters["issueIdOrKey"], "100");
        match command {
            CREATE => assert_eq!(
                frozen.call.body,
                Some(json!({"body":body("Replacement"),"visibility":null}))
            ),
            UPDATE => {
                assert_eq!(frozen.call.body, Some(json!({"body":body("Replacement")})));
                assert_eq!(
                    frozen.before().and_then(|r| r.author_account_id.as_deref()),
                    Some("service-one")
                );
            }
            _ => assert!(frozen.call.body.is_none()),
        }
        assert!(port.calls.iter().all(|c| c.body.is_none()));
    }
    let mut update = value(UPDATE)?;
    update.as_object_mut().ok_or("object")?.remove("body");
    update["visibility"] = json!({"mode":"project"});
    let (read, write) = operations(UPDATE, update)?;
    let mut port = Port::new(&read, replies(UPDATE));
    assert_eq!(
        prepare(
            &read,
            &write,
            &mut port,
            principal(),
            Arc::new(TestClock::new())
        )?
        .call
        .body,
        Some(json!({"visibility":null}))
    );
    Ok(())
}
#[test]
fn own_and_any_intents_use_original_author_and_distinct_native_permissions() -> TestResult {
    for command in [UPDATE, DELETE] {
        let own_key = if command == UPDATE {
            "EDIT_OWN_COMMENTS"
        } else {
            "DELETE_OWN_COMMENTS"
        };
        let all_key = if command == UPDATE {
            "EDIT_ALL_COMMENTS"
        } else {
            "DELETE_ALL_COMMENTS"
        };
        for (author, editor, ownership, own, all, allowed) in [
            (Some("service-one"), "other", "own", true, false, true),
            (Some("service-one"), "other", "own", false, true, true),
            (Some("other"), "service-one", "own", true, true, false),
            (None, "service-one", "own", true, true, false),
            (Some("service-one"), "other", "own", false, false, false),
            (Some("other"), "service-one", "any", true, false, false),
            (Some("other"), "service-one", "any", false, true, true),
            (None, "service-one", "any", false, true, true),
        ] {
            let mut rows = replies(command);
            for index in [1, 3] {
                rows[index].body["author"] = author
                    .map(|id| json!({"accountId":id}))
                    .unwrap_or(Value::Null);
                rows[index].body["updateAuthor"]["accountId"] = json!(editor);
            }
            rows[2].body["permissions"][own_key]["havePermission"] = json!(own);
            rows[2].body["permissions"][all_key]["havePermission"] = json!(all);
            let mut input = value(command)?;
            input["ownership"] = json!(ownership);
            input["expected_revision"] = json!(parse_record(&rows[1].body, "100", true)?.revision);
            let (read, write) = operations(command, input)?;
            let mut port = Port::new(&read, rows);
            assert_eq!(
                prepare(
                    &read,
                    &write,
                    &mut port,
                    principal(),
                    Arc::new(TestClock::new())
                )
                .is_ok(),
                allowed,
                "{command} {ownership} {author:?} {own} {all}"
            );
        }
    }
    Ok(())
}
#[test]
fn changed_hidden_or_unpermitted_evidence_never_freezes_a_mutation() -> TestResult {
    let (read, write) = operations(UPDATE, value(UPDATE)?)?;
    for mode in [
        "project_before",
        "project_after",
        "id",
        "revision",
        "privacy",
        "author",
        "missing_body",
        "browse",
        "missing_permission",
        "permission_type",
        "error",
        "hidden",
    ] {
        let mut rows = replies(UPDATE);
        match mode {
            "project_before" => rows[0].body["fields"]["project"]["id"] = json!("999"),
            "project_after" => rows[4].body["fields"]["project"]["id"] = json!("999"),
            "id" => rows[1].body["id"] = json!("999"),
            "revision" => rows[3].body["body"] = body("Concurrent edit"),
            "privacy" => rows[3].body["jsdPublic"] = json!(true),
            "author" => rows[3].body["author"]["accountId"] = json!("other"),
            "missing_body" => {
                rows[1].body.as_object_mut().ok_or("object")?.remove("body");
            }
            "browse" => {
                rows[2].body["permissions"]["BROWSE_PROJECTS"]["havePermission"] = json!(false)
            }
            "missing_permission" => {
                rows[2].body["permissions"]
                    .as_object_mut()
                    .ok_or("permissions")?
                    .remove("EDIT_ALL_COMMENTS");
            }
            "permission_type" => {
                rows[2].body["permissions"]["EDIT_OWN_COMMENTS"]["havePermission"] = json!("true")
            }
            "error" => rows[1].body["warningMessages"] = json!([]),
            _ => rows[1].status = 404,
        }
        let mut port = Port::new(&read, rows);
        assert!(
            prepare(
                &read,
                &write,
                &mut port,
                principal(),
                Arc::new(TestClock::new())
            )
            .is_err(),
            "{mode}"
        );
    }
    Ok(())
}
#[test]
fn frozen_comment_refuses_changed_binding_principal_and_both_clock_failures() -> TestResult {
    for clock_change in ["wall_expired", "mono_expired", "wall_back", "mono_back"] {
        let clock = Arc::new(TestClock::new());
        let (read, write) = operations(CREATE, value(CREATE)?)?;
        let mut port = Port::new(&read, replies(CREATE));
        let frozen = prepare(&read, &write, &mut port, principal(), clock.clone())?;
        let mut other = principal();
        other.actor = "other".into();
        assert!(frozen.verify(&write, &other).is_err());
        other = principal();
        other.grant.id = format!("gr_{}", "1".repeat(32));
        assert!(frozen.verify(&write, &other).is_err());
        let mut input = value(CREATE)?;
        input["issue_id"] = json!("999");
        assert!(
            frozen
                .verify(&operations(CREATE, input)?.1, &principal())
                .is_err()
        );
        match clock_change {
            "wall_expired" => clock.wall.store(31_000, Ordering::SeqCst),
            "mono_expired" => clock.mono.store(30_100, Ordering::SeqCst),
            "wall_back" => clock.wall.store(999, Ordering::SeqCst),
            _ => clock.mono.store(99, Ordering::SeqCst),
        }
        assert!(frozen.verify(&write, &principal()).is_err());
    }
    Ok(())
}
#[test]
fn typed_input_rejects_null_noop_rich_text_and_visibility_mistakes_locally() -> TestResult {
    for (command, mode) in [
        (UPDATE, "null_body"),
        (UPDATE, "null_visibility"),
        (UPDATE, "noop"),
        (UPDATE, "fingerprint"),
        (UPDATE, "visibility"),
        (CREATE, "unknown_body"),
        (CREATE, "heading"),
        (CREATE, "javascript"),
        (CREATE, "empty_text"),
        (CREATE, "version"),
        (CREATE, "unknown_input"),
    ] {
        let mut input = value(command)?;
        match mode {
            "null_body" => input["body"] = Value::Null,
            "null_visibility" => input["visibility"] = Value::Null,
            "noop" => {
                input.as_object_mut().ok_or("object")?.remove("body");
            }
            "fingerprint" => input["expected_revision"] = json!("sha256:wrong"),
            "visibility" => input["visibility"] = json!({"mode":"group","value":"Mutable name"}),
            "unknown_body" => {
                input["body"]["content"][0] = json!({"type":"extension","attrs":{"run":"ignored"}})
            }
            "heading" => {
                input["body"]["content"][0] =
                    json!({"type":"heading","attrs":{"level":7},"content":[]})
            }
            "javascript" => {
                input["body"]["content"][0]["content"][0]["marks"] =
                    json!([{"type":"link","attrs":{"href":"javascript:alert(1)"}}])
            }
            "empty_text" => input["body"]["content"][0]["content"][0]["text"] = json!(""),
            "version" => input["body"]["version"] = json!(2),
            _ => input["author"] = json!({"accountId":"other"}),
        }
        assert!(operations(command, input).is_err(), "{mode}");
    }
    let mut registry = Registry::default();
    register(&mut registry)?;
    let schema = &registry.descriptor(UPDATE)?.input_schema;
    let validator = jsonschema::validator_for(schema)?;
    assert!(validator.is_valid(&value(UPDATE)?));
    let mut input = value(UPDATE)?;
    input["body"] = Value::Null;
    assert!(!validator.is_valid(&input));
    input = value(UPDATE)?;
    input.as_object_mut().ok_or("object")?.remove("body");
    input["visibility"] = json!({"mode":"project"});
    assert!(validator.is_valid(&input));
    Ok(())
}
#[test]
fn acknowledged_ids_survive_missing_or_malformed_optional_comment_evidence() -> TestResult {
    let (_, create) = operations(CREATE, value(CREATE)?)?;
    let mut returned = raw_comment();
    returned["body"] = body("Replacement");
    returned["visibility"] = Value::Null;
    let full = interpret(&create, &response(201, returned.clone()))?;
    assert!(
        full.state == SubmissionState::Confirmed
            && full.acknowledged
            && full.desired_fields_verified
            && full.error.is_none()
    );
    for mode in [
        "missing_body",
        "malformed_privacy",
        "wrong_association",
        "different_body",
    ] {
        let mut raw = returned.clone();
        match mode {
            "missing_body" => {
                raw.as_object_mut().ok_or("object")?.remove("body");
            }
            "malformed_privacy" => raw["jsdPublic"] = json!("false"),
            "wrong_association" => raw["issueId"] = json!("999"),
            _ => raw["body"] = body("Different"),
        }
        let result = interpret(&create, &response(201, raw))?;
        assert!(
            result.state == SubmissionState::Confirmed
                && result.acknowledged
                && !result.desired_fields_verified
        );
        assert_eq!(result.resources[0].id, "101");
        assert!(result.error.is_some(), "{mode}");
    }
    let unknown = interpret(&create, &response(201, json!({"body":body("Replacement")})))?;
    assert!(
        unknown.state == SubmissionState::Unknown
            && unknown.acknowledged
            && unknown.resources.is_empty()
    );
    let (_, update) = operations(UPDATE, value(UPDATE)?)?;
    for raw in [Value::Null, json!({"id":"999","body":body("Replacement")})] {
        let result = interpret(&update, &response(200, raw))?;
        assert!(
            result.state == SubmissionState::Confirmed
                && !result.desired_fields_verified
                && result.comment.is_none()
        );
        assert_eq!(result.resources[0].id, "101");
    }
    let (_, delete) = operations(DELETE, value(DELETE)?)?;
    let deleted = interpret(&delete, &response(204, Value::Null))?;
    assert!(deleted.state == SubmissionState::Confirmed && deleted.desired_fields_verified);
    Ok(())
}
#[test]
fn refused_and_unqualified_native_statuses_never_become_replay_authority() -> TestResult {
    for command in [CREATE, UPDATE, DELETE] {
        let (_, write) = operations(command, value(command)?)?;
        for status in [400, 401, 403, 404, 405, 413, 429, 500, 202] {
            let result = interpret(&write, &response(status, json!({"id":"101"})))?;
            assert!(!result.acknowledged && result.resources.is_empty());
            assert!(
                result.state
                    == if matches!(status, 400 | 401 | 404)
                        || command == CREATE && status == 413
                        || command == DELETE && status == 405
                    {
                        SubmissionState::Failed
                    } else {
                        SubmissionState::Unknown
                    }
            );
            let error = result.error.ok_or("missing status error")?;
            assert!(
                error.details.write_state
                    == if result.state == SubmissionState::Failed {
                        RequestState::Failed
                    } else {
                        RequestState::SentUnknown
                    }
            );
        }
    }
    Ok(())
}
