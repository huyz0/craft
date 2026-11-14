use craft_core::contracts::{CanonicalRequest, GrantRef, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::registry::Registry;
use craft_core::storage::{Clock, SubmissionState};
use craft_provider_jira::create_metadata::MetadataPrincipal;
use craft_provider_jira::issue_context::mutation::actions::{
    self, ASSIGN_ISSUE, DELETE_ISSUE, JiraIssueActions, UNASSIGN_ISSUE,
};
use craft_provider_jira::issue_context::mutation::result;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "issue".into(),
        id: "10000".into(),
    }
}
fn principal() -> MetadataPrincipal {
    MetadataPrincipal {
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        actor: "actor-one".into(),
    }
}
fn expected() -> Value {
    json!({"id":"10000","key":"EX-1","project_id":"100","issue_type_id":"101","status_id":"102","updated":"2026-11-10T01:00:00.000+0000"})
}
fn state() -> Value {
    json!({"id":"10000","key":"EX-1","fields":{"project":{"id":"100"},"issuetype":{"id":"101"},"status":{"id":"102"},"updated":"2026-11-10T01:00:00.000+0000","assignee":null,"subtasks":[]}})
}
fn request(command: &str) -> CanonicalRequest {
    let mut input = json!({"issue":"10000","expected":expected()});
    if command == DELETE_ISSUE {
        input["subtasks"] = json!({"mode":"refuse"});
    } else {
        input["expected_assignee"] = json!({"kind":"unassigned"});
    }
    if command == ASSIGN_ISSUE {
        input["account_id"] = json!("account-one");
    }
    CanonicalRequest {
        command: command.into(),
        target: target(),
        input,
        selection: Selection::default(),
        preconditions: None,
    }
}
fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraIssueActions)?;
    Ok(registry)
}
fn pair(
    registry: &Registry,
    request: CanonicalRequest,
) -> Result<(PreparedOperation, PreparedOperation), CraftError> {
    let mut read = request.clone();
    read.command = actions::metadata_command(&request.command)?.into();
    Ok((registry.prepare(read)?, registry.prepare(request)?))
}
struct TestClock(AtomicU64, AtomicU64);
impl Default for TestClock {
    fn default() -> Self {
        Self(AtomicU64::new(100_000), AtomicU64::new(100_000))
    }
}
impl Clock for TestClock {
    fn unix_millis(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn monotonic_millis(&self) -> u64 {
        self.1.load(Ordering::SeqCst)
    }
}
struct Context {
    responses: VecDeque<NativeResponse>,
    calls: Vec<NativeCall>,
    revoke_after: Option<usize>,
}
impl Context {
    fn new(values: Vec<Value>) -> Self {
        Self {
            responses: values
                .into_iter()
                .map(|body| NativeResponse {
                    status: 200,
                    body,
                    retry_after: None,
                    link_headers: None,
                    request_id: None,
                })
                .collect(),
            calls: vec![],
            revoke_after: None,
        }
    }
}
impl OperationContext for Context {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, _: &PreparedOperation) -> Result<(), CraftError> {
        if self.revoke_after.is_some_and(|n| self.calls.len() >= n) {
            return Err(CraftError::new(ErrorCode::PolicyDenied, "Access revoked."));
        }
        Ok(())
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        self.responses
            .pop_front()
            .ok_or_else(|| CraftError::new(ErrorCode::Internal, "Unexpected native request."))
    }
    fn normalize_next(&self, _: &NativeCall, _: &str) -> Result<NativeCall, CraftError> {
        Err(CraftError::new(
            ErrorCode::Internal,
            "Must not follow links.",
        ))
    }
}

#[test]
fn assignment_uses_issue_specific_proof_and_freezes_only_account_id() -> TestResult {
    let registry = registry()?;
    let (read, write) = pair(&registry, request(ASSIGN_ISSUE))?;
    let clock = TestClock::default();
    let mut context = Context::new(vec![
        state(),
        json!([{"accountId":"account-one","active":true,"emailAddress":"ignored"}]),
        state(),
    ]);
    let frozen = actions::prepare_action(&read, &write, &mut context, &principal(), &clock)?;
    assert_eq!(context.calls.len(), 3);
    let query = &context.calls[1].query;
    assert_eq!(query.len(), 4);
    assert_eq!(query["issueId"], "10000");
    assert_eq!(query["accountId"], "account-one");
    assert_eq!(query["maxResults"], "2");
    assert_eq!(query["startAt"], "0");
    assert_eq!(
        context.calls[0].query["fields"],
        "project,issuetype,status,updated,assignee"
    );
    assert_eq!(frozen.call().body, Some(json!({"accountId":"account-one"})));
    assert!(frozen.call().query.is_empty());
    assert_eq!(frozen.call().path_parameters["issueIdOrKey"], "10000");
    assert_eq!(frozen.data().issue_before.id, "10000");
    assert!(frozen.action().is_some());
    frozen.verify(&write, &principal(), &clock)?;
    let mut other = principal();
    other.actor = "actor-two".into();
    assert!(frozen.verify(&write, &other, &clock).is_err());
    for (wall, mono) in [
        (160_000, 100_000),
        (100_000, 160_000),
        (99_999, 100_000),
        (100_000, 99_999),
    ] {
        clock.0.store(wall, Ordering::SeqCst);
        clock.1.store(mono, Ordering::SeqCst);
        assert_eq!(
            frozen
                .verify(&write, &principal(), &clock)
                .err()
                .ok_or("expected stale")?
                .code,
            ErrorCode::VersionConflict
        );
    }
    Ok(())
}

#[test]
fn unassignment_does_not_lookup_users_or_require_edit_metadata() -> TestResult {
    let registry = registry()?;
    let (read, write) = pair(&registry, request(UNASSIGN_ISSUE))?;
    let scopes = &registry
        .descriptor(&read.request().command)?
        .scope_alternatives;
    assert_eq!(scopes, &vec![vec!["read:jira-work".to_owned()]]);
    let mut context = Context::new(vec![state(), state()]);
    let frozen = actions::prepare_action(
        &read,
        &write,
        &mut context,
        &principal(),
        &TestClock::default(),
    )?;
    assert_eq!(context.calls.len(), 2);
    assert_eq!(frozen.call().body, Some(json!({"accountId":null})));
    Ok(())
}

#[test]
fn strict_action_input_rejects_omission_sentinels_and_unrelated_native_fields() -> TestResult {
    let registry = registry()?;
    for command in [ASSIGN_ISSUE, UNASSIGN_ISSUE, DELETE_ISSUE] {
        let good = request(command);
        for key in [
            "issue",
            "expected",
            if command == DELETE_ISSUE {
                "subtasks"
            } else {
                "expected_assignee"
            },
        ] {
            let mut bad = good.clone();
            bad.input.as_object_mut().ok_or("object")?.remove(key);
            assert!(registry.prepare(bad).is_err(), "{command} missing {key}");
        }
        for key in [
            "overrideScreenSecurity",
            "name",
            "notifyUsers",
            "project",
            "extra",
        ] {
            let mut bad = good.clone();
            bad.input[key] = json!(true);
            assert!(registry.prepare(bad).is_err(), "{command} accepts {key}");
        }
        let mut bad = good.clone();
        bad.input["expected"]["id"] = json!("20000");
        assert!(registry.prepare(bad).is_err());
        let mut bad = good.clone();
        bad.target.id = "20000".into();
        assert!(registry.prepare(bad).is_err());
    }
    for value in [
        Value::Null,
        json!("-1"),
        json!("null"),
        json!("unassigned"),
        json!("unknown"),
        json!(""),
        json!("person@example.com"),
        json!("display name"),
        json!("a\nb"),
    ] {
        let mut bad = request(ASSIGN_ISSUE);
        bad.input["account_id"] = value;
        assert!(registry.prepare(bad).is_err());
    }
    for children in [
        json!(["10001", "10001"]),
        json!(["10002", "10001"]),
        json!(["EX-2"]),
        json!(["10000"]),
        json!(null),
    ] {
        let mut bad = request(DELETE_ISSUE);
        bad.input["subtasks"] = json!({"mode":"delete_all","expected_visible_ids":children});
        assert!(registry.prepare(bad).is_err());
    }
    Ok(())
}

#[test]
fn assignment_refuses_unqualified_accounts_and_observed_drift() -> TestResult {
    let registry = registry()?;
    let (read, write) = pair(&registry, request(ASSIGN_ISSUE))?;
    for account in [
        json!([]),
        json!([{"accountId":"other","active":true}]),
        json!([{"accountId":"account-one","active":false}]),
        json!([{"accountId":"account-one"}]),
        json!([{"accountId":"account-one","active":true},{"accountId":"account-one","active":true}]),
        json!({"accountId":"account-one","active":true}),
    ] {
        let mut context = Context::new(vec![state(), account, state()]);
        assert!(
            actions::prepare_action(
                &read,
                &write,
                &mut context,
                &principal(),
                &TestClock::default()
            )
            .is_err()
        );
        assert_eq!(context.calls.len(), 2);
    }
    for field in ["project", "issuetype", "status", "updated", "assignee"] {
        let mut changed = state();
        changed["fields"][field] = match field {
            "updated" => json!("changed"),
            "assignee" => json!({"accountId":"other"}),
            _ => json!({"id":"999"}),
        };
        let mut context = Context::new(vec![
            state(),
            json!([{"accountId":"account-one","active":true}]),
            changed,
        ]);
        assert_eq!(
            actions::prepare_action(
                &read,
                &write,
                &mut context,
                &principal(),
                &TestClock::default()
            )
            .err()
            .ok_or("expected drift")?
            .code,
            ErrorCode::VersionConflict
        );
    }
    Ok(())
}

#[test]
fn deletion_retains_native_refusal_and_explicit_all_subtasks_semantics() -> TestResult {
    let registry = registry()?;
    let (read, write) = pair(&registry, request(DELETE_ISSUE))?;
    let mut context = Context::new(vec![state(), state()]);
    let frozen = actions::prepare_action(
        &read,
        &write,
        &mut context,
        &principal(),
        &TestClock::default(),
    )?;
    assert!(frozen.call().body.is_none());
    assert_eq!(frozen.call().query["deleteSubtasks"], "false");
    let mut parent = state();
    parent["fields"]["subtasks"] = json!([{"id":"10002"},{"id":"10001","key":"inert"}]);
    let mut context = Context::new(vec![parent.clone()]);
    assert!(
        actions::prepare_action(
            &read,
            &write,
            &mut context,
            &principal(),
            &TestClock::default()
        )
        .is_err()
    );
    let mut cascade = request(DELETE_ISSUE);
    cascade.input["subtasks"] =
        json!({"mode":"delete_all","expected_visible_ids":["10001","10002"]});
    let (read, write) = pair(&registry, cascade)?;
    let mut context = Context::new(vec![parent.clone(), parent.clone()]);
    let frozen = actions::prepare_action(
        &read,
        &write,
        &mut context,
        &principal(),
        &TestClock::default(),
    )?;
    assert_eq!(frozen.call().query["deleteSubtasks"], "true");
    let evidence = serde_json::to_value(frozen.action())?;
    assert_eq!(evidence["observed_subtasks"], json!(["10001", "10002"]));
    assert_eq!(evidence["hierarchy_complete"], false);
    assert_eq!(evidence["includes_unobserved_subtasks"], true);
    for bad in [
        json!([]),
        json!([{"id":"10001"}]),
        json!([{"id":"10001"},{"id":"10001"}]),
        Value::Null,
    ] {
        let mut after = parent.clone();
        after["fields"]["subtasks"] = bad;
        let mut context = Context::new(vec![parent.clone(), after]);
        assert!(
            actions::prepare_action(
                &read,
                &write,
                &mut context,
                &principal(),
                &TestClock::default()
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn action_preparation_rechecks_binding_and_final_access() -> TestResult {
    let registry = registry()?;
    let (read, write) = pair(&registry, request(UNASSIGN_ISSUE))?;
    let mut context = Context::new(vec![state(), state()]);
    context.revoke_after = Some(2);
    assert_eq!(
        actions::prepare_action(
            &read,
            &write,
            &mut context,
            &principal(),
            &TestClock::default()
        )
        .err()
        .ok_or("expected revoke")?
        .code,
        ErrorCode::PolicyDenied
    );
    let mut context = Context::new(vec![]);
    let (other, _) = pair(&registry, request(ASSIGN_ISSUE))?;
    assert!(
        actions::prepare_action(
            &other,
            &write,
            &mut context,
            &principal(),
            &TestClock::default()
        )
        .is_err()
    );
    assert!(context.calls.is_empty());
    Ok(())
}

#[test]
fn action_descriptors_have_strict_examples_scopes_and_closed_read_routes() -> TestResult {
    let registry = registry()?;
    for command in [ASSIGN_ISSUE, UNASSIGN_ISSUE, DELETE_ISSUE] {
        for name in [command, actions::metadata_command(command)?] {
            let descriptor = registry.descriptor(name)?;
            assert!(
                jsonschema::validator_for(&descriptor.input_schema)?.is_valid(&descriptor.example)
            );
            assert!(
                jsonschema::validator_for(&descriptor.response_schema)?
                    .is_valid(&descriptor.response_example)
            );
            let mut missing = descriptor.example.clone();
            missing
                .as_object_mut()
                .ok_or("input")?
                .remove(if command == DELETE_ISSUE {
                    "subtasks"
                } else {
                    "expected_assignee"
                });
            assert!(!jsonschema::validator_for(&descriptor.input_schema)?.is_valid(&missing));
            let read = name != command;
            assert_eq!(
                serde_json::to_value(descriptor.effect)?,
                if read {
                    json!("read")
                } else if command == DELETE_ISSUE {
                    json!("permanent")
                } else {
                    json!("write")
                }
            );
            assert_eq!(
                descriptor.routes.len(),
                if read && command == ASSIGN_ISSUE {
                    2
                } else {
                    1
                }
            );
            assert_eq!(
                descriptor.scope_alternatives,
                if read {
                    if command == ASSIGN_ISSUE {
                        vec![vec![
                            "read:jira-work".to_owned(),
                            "read:jira-user".to_owned(),
                        ]]
                    } else {
                        vec![vec!["read:jira-work".to_owned()]]
                    }
                } else {
                    vec![vec!["write:jira-work".to_owned()]]
                }
            );
        }
    }
    Ok(())
}

#[test]
fn missing_assignee_and_malformed_or_oversized_hierarchy_never_become_absence() -> TestResult {
    let registry = registry()?;
    for command in [UNASSIGN_ISSUE, DELETE_ISSUE] {
        let (read, write) = pair(&registry, request(command))?;
        for value in [
            None,
            Some(json!({})),
            Some(json!([])),
            Some(json!("unknown")),
            Some(Value::Null),
        ] {
            if command == UNASSIGN_ISSUE && value == Some(Value::Null) {
                continue;
            }
            if command == DELETE_ISSUE && value == Some(json!([])) {
                continue;
            }
            let mut invalid = state();
            let key = if command == DELETE_ISSUE {
                "subtasks"
            } else {
                "assignee"
            };
            let fields = invalid["fields"].as_object_mut().ok_or("fields")?;
            if let Some(value) = value {
                fields.insert(key.into(), value);
            } else {
                fields.remove(key);
            }
            let mut context = Context::new(vec![invalid]);
            assert!(
                actions::prepare_action(
                    &read,
                    &write,
                    &mut context,
                    &principal(),
                    &TestClock::default()
                )
                .is_err()
            );
        }
    }
    let mut input = request(DELETE_ISSUE);
    let ids: Vec<_> = (10001..10258).map(|id| id.to_string()).collect();
    input.input["subtasks"] = json!({"mode":"delete_all","expected_visible_ids":ids});
    assert!(registry.prepare(input).is_err());
    let (read, write) = pair(&registry, request(DELETE_ISSUE))?;
    let mut oversized = state();
    oversized["fields"]["subtasks"] =
        json!(ids.iter().map(|id| json!({"id":id})).collect::<Vec<_>>());
    let mut context = Context::new(vec![oversized]);
    assert!(
        actions::prepare_action(
            &read,
            &write,
            &mut context,
            &principal(),
            &TestClock::default()
        )
        .is_err()
    );
    Ok(())
}

struct AgingContext<'a> {
    inner: Context,
    clock: &'a TestClock,
    at: usize,
    wall: u64,
    monotonic: u64,
}
impl OperationContext for AgingContext<'_> {
    fn checkpoint(&self) -> Result<(), CraftError> {
        self.inner.checkpoint()
    }
    fn verify_operation(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        self.inner.verify_operation(operation)
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        let response = self.inner.invoke(call)?;
        if self.inner.calls.len() == self.at {
            self.clock.0.store(self.wall, Ordering::SeqCst);
            self.clock.1.store(self.monotonic, Ordering::SeqCst);
        }
        Ok(response)
    }
    fn normalize_next(&self, call: &NativeCall, next: &str) -> Result<NativeCall, CraftError> {
        self.inner.normalize_next(call, next)
    }
}

#[test]
fn freshness_is_not_restarted_by_user_proof_or_final_issue_read() -> TestResult {
    let registry = registry()?;
    let (read, write) = pair(&registry, request(ASSIGN_ISSUE))?;
    for at in [1, 2, 3] {
        for (wall, monotonic) in [
            (160_000, 100_000),
            (100_000, 160_000),
            (99_999, 100_000),
            (100_000, 99_999),
        ] {
            let clock = TestClock::default();
            let inner = Context::new(vec![
                state(),
                json!([{"accountId":"account-one","active":true}]),
                state(),
            ]);
            let mut context = AgingContext {
                inner,
                clock: &clock,
                at,
                wall,
                monotonic,
            };
            assert_eq!(
                actions::prepare_action(&read, &write, &mut context, &principal(), &clock)
                    .err()
                    .ok_or("expected expiry")?
                    .code,
                ErrorCode::VersionConflict
            );
            assert_eq!(context.inner.calls.len(), at);
        }
    }
    Ok(())
}

#[test]
fn oversized_user_response_is_remote_failure_and_never_qualifies_account() -> TestResult {
    let registry = registry()?;
    let (read, write) = pair(&registry, request(ASSIGN_ISSUE))?;
    let mut context = Context::new(vec![
        state(),
        json!([{"accountId":"account-one","active":true,"displayName":"x".repeat(65_536)}]),
    ]);
    assert_eq!(
        actions::prepare_action(
            &read,
            &write,
            &mut context,
            &principal(),
            &TestClock::default()
        )
        .err()
        .ok_or("oversized response")?
        .code,
        ErrorCode::RemoteUnavailable
    );
    Ok(())
}

#[test]
fn action_outcomes_confirm_only_documented_acknowledgments_and_refusals() -> TestResult {
    for command in [ASSIGN_ISSUE, UNASSIGN_ISSUE, DELETE_ISSUE] {
        for status in [200, 201, 202, 204, 400, 401, 403, 404, 409, 422, 429, 500] {
            let response = NativeResponse {
                status,
                body: json!({"id":"10000","errorMessages":["inert provider text"]}),
                retry_after: None,
                link_headers: None,
                request_id: None,
            };
            let outcome = result::interpret(command, &target(), &response);
            let expected = if status == 204 {
                SubmissionState::Confirmed
            } else if matches!(status, 400 | 403 | 404)
                || (command == DELETE_ISSUE && status == 401)
            {
                SubmissionState::Failed
            } else {
                SubmissionState::Unknown
            };
            assert_eq!(outcome.state, expected, "{command} {status}");
            assert_eq!(
                outcome.confirmed_resources.len(),
                usize::from(status == 204)
            );
            assert!(outcome.error.is_some());
        }
    }
    Ok(())
}
