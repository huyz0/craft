use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use craft_core::contracts::{CanonicalRequest, GrantRef, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::registry::{ProviderModule, Registry};
use craft_core::storage::Clock;
use craft_provider_jira::create_metadata::MetadataPrincipal;
use craft_provider_jira::issue_context::mutation::{
    EDIT_ISSUE, JiraIssueMutations, TRANSITION_ISSUE,
};
use craft_provider_jira::issue_context::{self, FieldPlan, JiraIssueContext};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    JiraIssueContext.register(&mut registry)?;
    JiraIssueMutations.register(&mut registry)?;
    Ok(registry)
}
fn expected() -> Value {
    json!({"id":"10000","key":"ED-7","project_id":"100","issue_type_id":"101","status_id":"102","updated":"2026-11-10T01:00:00.000+0000"})
}
fn state() -> Value {
    json!({"id":"10000","key":"ED-7","fields":{"project":{"id":"100"},"issuetype":{"id":"101"},"status":{"id":"102"},"updated":expected()["updated"]}})
}
fn request(command: &str, input: Value) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "issue".into(),
            id: "10000".into(),
        },
        input,
        selection: Selection::default(),
        preconditions: None,
    }
}
fn write(command: &str, changes: Value) -> CanonicalRequest {
    let mut input = json!({"issue":"10000","expected":expected(),"changes":changes});
    if command == TRANSITION_ISSUE {
        input["transition_id"] = json!("31");
        input["expected_status_id"] = json!("103");
    }
    request(command, input)
}
fn read(command: &str) -> Result<PreparedOperation, CraftError> {
    let mut value = request(command, json!({"issue":"10000"}));
    if command == issue_context::TRANSITIONS {
        value.selection.fields = vec!["id".into(), "fields".into()];
        value.input["transition_id"] = json!("31");
    }
    registry()?.prepare(value)
}
fn field(name: &str, kind: &str, required: bool) -> Value {
    json!({"key":"different-native-key","name":name,"required":required,"operations":["set"],"schema":{"type":kind,"system":name}})
}
fn metadata() -> Value {
    let mut labels = field("labels", "array", false);
    labels["schema"]["items"] = json!("string");
    labels["operations"] = json!(["set", "add", "remove"]);
    json!({"fields":{"summary":field("summary","string",true),"description":field("description","string",false),"labels":labels}})
}
fn transitions(fields: Value) -> Value {
    json!({"transitions":[{"id":"31","name":"Complete","to":{"id":"103"},"hasScreen":true,"fields":fields}]})
}
fn principal() -> MetadataPrincipal {
    MetadataPrincipal {
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        actor: "account-one".into(),
    }
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
    responses: VecDeque<Value>,
    calls: Vec<NativeCall>,
}
impl Context {
    fn new(metadata: Value) -> Self {
        Self {
            responses: vec![state(), metadata, state()].into(),
            calls: vec![],
        }
    }
}
impl OperationContext for Context {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, _: &PreparedOperation) -> Result<(), CraftError> {
        Ok(())
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        Ok(NativeResponse {
            status: 200,
            body: self
                .responses
                .pop_front()
                .ok_or_else(|| CraftError::new(ErrorCode::Internal, "Unexpected call."))?,
            request_id: None,
            retry_after: None,
            link_headers: None,
        })
    }
    fn normalize_next(&self, _: &NativeCall, _: &str) -> Result<NativeCall, CraftError> {
        Err(CraftError::new(
            ErrorCode::Internal,
            "Must not follow links.",
        ))
    }
}
fn live(
    operation: &PreparedOperation,
    context: &mut Context,
    clock: &TestClock,
) -> Result<issue_context::LiveIssueContext, CraftError> {
    issue_context::read_context(
        operation,
        context,
        &FieldPlan::compile(
            &operation.request().command,
            &operation.request().selection.fields,
        )?,
        None,
        &principal(),
        clock,
    )
}

#[test]
fn freshness_and_policy_are_rechecked_after_required_value_observation() -> TestResult {
    struct FinalContext<'a> {
        inner: Context,
        clock: &'a TestClock,
        revoke: bool,
    }
    impl OperationContext for FinalContext<'_> {
        fn checkpoint(&self) -> Result<(), CraftError> {
            if self.revoke && self.inner.calls.len() == 4 {
                return Err(CraftError::new(ErrorCode::PolicyDenied, "Policy changed."));
            }
            Ok(())
        }
        fn verify_operation(&self, _: &PreparedOperation) -> Result<(), CraftError> {
            Ok(())
        }
        fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
            let response = self.inner.invoke(call)?;
            if !self.revoke {
                self.clock.1.store(160_000, Ordering::SeqCst);
            }
            Ok(response)
        }
        fn normalize_next(&self, _: &NativeCall, _: &str) -> Result<NativeCall, CraftError> {
            Err(CraftError::new(
                ErrorCode::Internal,
                "Must not follow links.",
            ))
        }
    }
    for revoke in [false, true] {
        let read = read(issue_context::TRANSITIONS)?;
        let clock = TestClock::default();
        let mut context = Context::new(transitions(
            json!({"summary":field("summary","string",true)}),
        ));
        let live = live(&read, &mut context, &clock)?;
        let mut current = state();
        current["fields"]["summary"] = json!("Human summary");
        context.responses.push_back(current);
        let mut context = FinalContext {
            inner: context,
            clock: &clock,
            revoke,
        };
        let write = registry()?.prepare(write(TRANSITION_ISSUE, json!({})))?;
        let error = live
            .prepare_mutation(&read, &write, &mut context, &principal(), &clock)
            .err()
            .ok_or("expected refusal")?;
        assert_eq!(
            error.code,
            if revoke {
                ErrorCode::PolicyDenied
            } else {
                ErrorCode::VersionConflict
            }
        );
        assert_eq!(context.inner.calls.len(), 4);
    }
    Ok(())
}

#[test]
fn partial_edit_uses_private_metadata_and_preserves_omitted_required_fields() -> TestResult {
    let read = read(issue_context::EDIT)?;
    let clock = TestClock::default();
    let mut context = Context::new(metadata());
    let live = live(&read, &mut context, &clock)?;
    assert!(live.page().items[0].get("allowed_values").is_none());
    let write = registry()?.prepare(write(EDIT_ISSUE, json!({"description":"First\nSecond"})))?;
    let frozen = live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)?;
    assert_eq!(
        frozen.call().path_parameters,
        BTreeMap::from([("issueIdOrKey".into(), "10000".into())])
    );
    assert_eq!(
        frozen.call().query,
        BTreeMap::from([
            ("notifyUsers".into(), "true".into()),
            ("returnIssue".into(), "false".into())
        ])
    );
    assert_eq!(
        frozen.call().body,
        Some(
            json!({"fields":{"description":{"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"First\nSecond"}]}]}}})
        )
    );
    assert_eq!(context.calls.len(), 3);
    frozen.verify(&write, &principal(), &clock)?;
    Ok(())
}

#[test]
fn mutation_cursor_is_refused_by_the_provider_before_preparation() -> TestResult {
    let mut request = write(EDIT_ISSUE, json!({"summary":"New"}));
    request.selection.cursor = Some("unrelated-page".into());
    assert!(registry()?.prepare(request).is_err());
    Ok(())
}

#[test]
fn response_confirmation_comes_only_from_the_bound_documented_no_content_success() -> TestResult {
    use craft_core::storage::SubmissionState;
    use craft_provider_jira::issue_context::mutation::result::interpret;
    let target = write(EDIT_ISSUE, json!({"summary":"New"})).target;
    for command in [EDIT_ISSUE, TRANSITION_ISSUE] {
        for body in [
            Value::Null,
            json!({"id":"99999","self":"https://untrusted.invalid","errorMessages":["private-sentinel"]}),
        ] {
            let response = NativeResponse {
                status: 204,
                body: body.clone(),
                request_id: None,
                retry_after: None,
                link_headers: None,
            };
            let observed = interpret(command, &target, &response);
            assert_eq!(observed.state, SubmissionState::Confirmed);
            assert_eq!(observed.confirmed_resources, vec![target.clone()]);
            assert_eq!(observed.error.is_some(), !body.is_null());
        }
        for status in [200, 201, 202, 301, 403, 429, 500] {
            let observed = interpret(
                command,
                &target,
                &NativeResponse {
                    status,
                    body: json!({"id":target.id,"status":"matched","message":"private-sentinel"}),
                    request_id: None,
                    retry_after: None,
                    link_headers: None,
                },
            );
            if command == EDIT_ISSUE && status == 403 {
                assert_eq!(observed.state, SubmissionState::Failed);
            } else {
                assert_eq!(observed.state, SubmissionState::Unknown);
            }
            assert!(observed.confirmed_resources.is_empty());
            assert!(!serde_json::to_string(&observed.error)?.contains("private-sentinel"));
        }
    }
    for (command, statuses) in [
        (EDIT_ISSUE, vec![400, 401, 403, 404, 409, 422]),
        (TRANSITION_ISSUE, vec![400, 401, 404, 409, 413, 422]),
    ] {
        for status in statuses {
            let observed = interpret(
                command,
                &target,
                &NativeResponse {
                    status,
                    body: Value::Null,
                    request_id: None,
                    retry_after: None,
                    link_headers: None,
                },
            );
            assert_eq!(observed.state, SubmissionState::Failed);
            assert!(observed.confirmed_resources.is_empty());
        }
    }
    Ok(())
}

#[test]
fn required_transition_omissions_use_fresh_values_without_resubmitting_them() -> TestResult {
    let read = read(issue_context::TRANSITIONS)?;
    let clock = TestClock::default();
    let mut context = Context::new(transitions(
        json!({"summary":field("summary","string",true)}),
    ));
    let live = live(&read, &mut context, &clock)?;
    let mut current = state();
    current["fields"]["summary"] = json!("Human summary");
    context.responses.push_back(current);
    let write = registry()?.prepare(write(TRANSITION_ISSUE, json!({})))?;
    let frozen = live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)?;
    assert_eq!(frozen.call().body, Some(json!({"transition":{"id":"31"}})));
    assert_eq!(context.calls.len(), 4);
    assert_eq!(
        context.calls[3].query["fields"],
        "project,issuetype,status,updated,summary"
    );
    Ok(())
}

#[test]
fn frozen_body_cannot_transfer_operation_principal_or_outlive_metadata() -> TestResult {
    let read = read(issue_context::EDIT)?;
    let clock = TestClock::default();
    let mut context = Context::new(metadata());
    let live = live(&read, &mut context, &clock)?;
    let write = registry()?.prepare(write(EDIT_ISSUE, json!({"summary":"New summary"})))?;
    let frozen = live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)?;
    let mut other = principal();
    other.actor = "other-account".into();
    assert!(frozen.verify(&write, &other, &clock).is_err());
    let mut changed = write.request().clone();
    changed.input["changes"]["summary"] = json!("Different");
    assert!(
        frozen
            .verify(&registry()?.prepare(changed)?, &principal(), &clock)
            .is_err()
    );
    for (wall, mono) in [
        (160_000, 100_000),
        (100_000, 160_000),
        (99_999, 100_000),
        (100_000, 99_999),
    ] {
        clock.0.store(wall, Ordering::SeqCst);
        clock.1.store(mono, Ordering::SeqCst);
        assert!(frozen.verify(&write, &principal(), &clock).is_err());
    }
    Ok(())
}

#[test]
fn local_invalid_maps_targets_and_label_conflicts_fail_during_pure_preparation() -> TestResult {
    for changes in [
        json!({}),
        json!({"status":"Done"}),
        json!({"assignee_account_id":"account-one"}),
        json!({"summary":null}),
        json!({"summary":"  "}),
        json!({"summary":"line\nbreak"}),
        json!({"labels":["with space"]}),
        json!({"labels":["same","same"]}),
        json!({"labels":[],"label_delta":{"add":["new"]}}),
        json!({"label_delta":null}),
        json!({"label_delta":{}}),
        json!({"label_delta":{"add":["same"],"remove":["same"]}}),
        json!({"custom_fields":{"status":"Done"}}),
        json!({"custom_fields":{"customfield_bad":"value"}}),
        json!({"priority_id":"name"}),
        json!({"fields":{"summary":"raw"}}),
    ] {
        assert!(registry()?.prepare(write(EDIT_ISSUE, changes)).is_err());
    }
    for name in [
        "fields",
        "update",
        "transition",
        "properties",
        "historyMetadata",
        "overrideScreenSecurity",
        "notifyUsers",
    ] {
        let mut value = write(EDIT_ISSUE, json!({"summary":"New"}));
        value.input[name] = json!({});
        assert!(registry()?.prepare(value).is_err());
    }
    for id in ["ED-7", "another", "99999"] {
        let mut value = write(EDIT_ISSUE, json!({"summary":"New"}));
        value.input["issue"] = json!(id);
        assert!(registry()?.prepare(value).is_err());
    }
    let mut value = write(EDIT_ISSUE, json!({"summary":"New"}));
    value.preconditions = Some(craft_core::contracts::Preconditions {
        expected_version: Some(1),
        expected_digest: None,
        expected_parent: None,
    });
    assert!(registry()?.prepare(value).is_err());
    Ok(())
}

#[test]
fn changed_expected_issue_components_and_cross_tenant_context_cannot_prepare_a_write() -> TestResult
{
    let read = read(issue_context::EDIT)?;
    let clock = TestClock::default();
    let mut context = Context::new(metadata());
    let live = live(&read, &mut context, &clock)?;
    for name in ["key", "project_id", "issue_type_id", "status_id", "updated"] {
        let mut request = write(EDIT_ISSUE, json!({"summary":"New"}));
        request.input["expected"][name] = json!(if name == "key" { "ED-8" } else { "999" });
        let operation = registry()?.prepare(request)?;
        assert!(
            live.prepare_mutation(&read, &operation, &mut context, &principal(), &clock)
                .is_err()
        );
    }
    let mut request = write(EDIT_ISSUE, json!({"summary":"New"}));
    request.target.tenant = "00000000-1111-2222-3333-555555555555".into();
    assert!(
        live.prepare_mutation(
            &read,
            &registry()?.prepare(request)?,
            &mut context,
            &principal(),
            &clock
        )
        .is_err()
    );
    let mut other = principal();
    other.grant.id = "gr_11111111111111111111111111111111".into();
    let operation = registry()?.prepare(write(EDIT_ISSUE, json!({"summary":"New"})))?;
    assert!(
        live.prepare_mutation(&read, &operation, &mut context, &other, &clock)
            .is_err()
    );
    assert_eq!(context.calls.len(), 3);
    Ok(())
}

fn custom(family: &str, kind: &str, required: bool) -> Value {
    json!({"key":"descriptive-key","name":"Custom","required":required,"operations":["set"],
        "schema":{"type":kind,"custom":format!("com.atlassian.jira.plugin.system.customfieldtypes:{family}"),"customId":1}})
}

#[test]
fn custom_options_and_adf_are_compiled_from_private_metadata_not_native_configuration() -> TestResult
{
    for (mut field, value, encoded) in [
        (
            custom("textfield", "string", false),
            json!("Text"),
            json!("Text"),
        ),
        (custom("float", "number", false), json!(0), json!(0)),
        (
            custom("select", "option", false),
            json!({"id":"7"}),
            json!({"id":"7"}),
        ),
        (
            custom("multiselect", "array", false),
            json!([{"id":"7"}]),
            json!([{"id":"7"}]),
        ),
        (
            custom("textarea", "string", false),
            json!("Text"),
            json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"Text"}]}]}),
        ),
    ] {
        if field["schema"]["type"] == "option" || field["schema"]["type"] == "array" {
            field["allowedValues"] = json!([{"id":"7","label":"inert"}]);
        }
        if field["schema"]["type"] == "array" {
            field["schema"]["items"] = json!("option");
        }
        field["configuration"] = json!({"$ref":"https://untrusted.invalid/never-read"});
        let read = read(issue_context::EDIT)?;
        let clock = TestClock::default();
        let mut context = Context::new(json!({"fields":{"customfield_1":field}}));
        let live = live(&read, &mut context, &clock)?;
        let write = registry()?.prepare(write(
            EDIT_ISSUE,
            json!({"custom_fields":{"customfield_1":value}}),
        ))?;
        let frozen = live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)?;
        assert_eq!(
            frozen.call().body,
            Some(json!({"fields":{"customfield_1":encoded}}))
        );
        assert_eq!(context.calls.len(), 3);
    }
    Ok(())
}

#[test]
fn unsupported_shapes_unknown_option_ids_and_invalid_explicit_clears_are_refused() -> TestResult {
    let mut option = custom("select", "option", false);
    option["allowedValues"] = json!([{"id":"7"}]);
    let mut wrong_custom = custom("textfield", "string", false);
    wrong_custom["schema"]["customId"] = json!(2);
    let mut not_settable = custom("textfield", "string", false);
    not_settable["operations"] = json!(["add"]);
    let mut duplicate_option = option.clone();
    duplicate_option["allowedValues"] = json!([{"id":"7"},{"id":"7"}]);
    for (field, value) in [
        (custom("unknown", "string", false), json!("Text")),
        (custom("textfield", "string", true), Value::Null),
        (custom("textfield", "string", true), json!("   ")),
        (custom("float", "number", false), json!("0")),
        (wrong_custom, json!("Text")),
        (not_settable, json!("Text")),
        (option.clone(), json!({"id":"8"})),
        (option, json!({"id":"7","name":"extra"})),
        (duplicate_option, json!({"id":"7"})),
    ] {
        let read = read(issue_context::EDIT)?;
        let clock = TestClock::default();
        let mut context = Context::new(json!({"fields":{"customfield_1":field}}));
        let live = live(&read, &mut context, &clock)?;
        let write = registry()?.prepare(write(
            EDIT_ISSUE,
            json!({"custom_fields":{"customfield_1":value}}),
        ))?;
        assert!(
            live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)
                .is_err()
        );
        assert_eq!(context.calls.len(), 3);
    }
    Ok(())
}

#[test]
fn optional_clear_empty_replacement_and_typed_label_delta_remain_distinct() -> TestResult {
    for (changes, body) in [
        (
            json!({"description":null}),
            json!({"fields":{"description":null}}),
        ),
        (json!({"labels":[]}), json!({"fields":{"labels":[]}})),
        (json!({"labels":null}), json!({"fields":{"labels":null}})),
        (
            json!({"label_delta":{"add":["new"],"remove":["old"]}}),
            json!({"update":{"labels":[{"add":"new"},{"remove":"old"}]}}),
        ),
    ] {
        let read = read(issue_context::EDIT)?;
        let clock = TestClock::default();
        let mut context = Context::new(metadata());
        let live = live(&read, &mut context, &clock)?;
        let write = registry()?.prepare(write(EDIT_ISSUE, changes))?;
        assert_eq!(
            live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)?
                .call()
                .body,
            Some(body)
        );
    }
    Ok(())
}

#[test]
fn transitions_require_exact_available_id_destination_and_expanded_screen_fields() -> TestResult {
    for change in [
        json!({"isAvailable":false}),
        json!({"isAvailable":null}),
        json!({"to":{"id":"999"}}),
        json!({"id":"32"}),
        json!({"fields":null}),
        json!({"hasScreen":null}),
    ] {
        let mut metadata = transitions(json!({}));
        for (key, value) in change.as_object().ok_or("object")? {
            metadata["transitions"][0][key] = value.clone();
        }
        let read = read(issue_context::TRANSITIONS)?;
        let clock = TestClock::default();
        let mut context = Context::new(metadata);
        let result = live(&read, &mut context, &clock);
        if let Ok(live) = result {
            let write = registry()?.prepare(write(TRANSITION_ISSUE, json!({})))?;
            assert!(
                live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)
                    .is_err()
            );
        }
        assert!(context.calls.len() <= 3);
    }
    for command in [issue_context::EDIT, issue_context::TRANSITIONS] {
        let mut request = request(command, json!({"issue":"10000"}));
        request.selection.fields = vec!["id".into()];
        let read = registry()?.prepare(request)?;
        let clock = TestClock::default();
        let mut context = Context::new(if command == issue_context::EDIT {
            metadata()
        } else {
            transitions(json!({}))
        });
        let live = live(&read, &mut context, &clock)?;
        let write = registry()?.prepare(write(TRANSITION_ISSUE, json!({})))?;
        assert!(
            live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn unavailable_transition_listing_needs_explicit_true_eligibility() -> TestResult {
    for available in [None, Some(json!(false)), Some(json!(true))] {
        let mut request = request(
            issue_context::TRANSITIONS,
            json!({"issue":"10000","include_unavailable":true}),
        );
        request.selection.fields = vec!["fields".into()];
        let read = registry()?.prepare(request)?;
        let mut metadata = transitions(json!({}));
        if let Some(value) = &available {
            metadata["transitions"][0]["isAvailable"] = value.clone();
        }
        let clock = TestClock::default();
        let mut context = Context::new(metadata);
        let live = live(&read, &mut context, &clock)?;
        let write = registry()?.prepare(write(TRANSITION_ISSUE, json!({})))?;
        assert_eq!(
            live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)
                .is_ok(),
            available == Some(json!(true))
        );
        assert_eq!(context.calls.len(), 3);
    }
    Ok(())
}

#[test]
fn missing_empty_and_changed_current_transition_values_fail_without_a_mutation() -> TestResult {
    for current in [
        None,
        Some(Value::Null),
        Some(json!("")),
        Some(json!("  ")),
        Some(json!({"unexpected":"shape"})),
    ] {
        let read = read(issue_context::TRANSITIONS)?;
        let clock = TestClock::default();
        let mut context = Context::new(transitions(
            json!({"summary":field("summary","string",true)}),
        ));
        let live = live(&read, &mut context, &clock)?;
        let mut raw = state();
        if let Some(value) = current {
            raw["fields"]["summary"] = value;
        }
        context.responses.push_back(raw);
        let write = registry()?.prepare(write(TRANSITION_ISSUE, json!({})))?;
        assert!(
            live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)
                .is_err()
        );
        assert_eq!(context.calls.len(), 4);
    }
    let read = read(issue_context::TRANSITIONS)?;
    let clock = TestClock::default();
    let mut context = Context::new(transitions(
        json!({"summary":field("summary","string",true)}),
    ));
    let live = live(&read, &mut context, &clock)?;
    let mut current = state();
    current["fields"]["summary"] = json!("Human");
    current["fields"]["updated"] = json!("changed");
    context.responses.push_back(current);
    let write = registry()?.prepare(write(TRANSITION_ISSUE, json!({})))?;
    assert!(
        live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)
            .is_err()
    );
    Ok(())
}

#[test]
fn transition_default_flag_alone_is_not_a_required_value_proof() -> TestResult {
    for default in [
        None,
        Some(Value::Null),
        Some(json!("")),
        Some(json!("Default summary")),
    ] {
        let read = read(issue_context::TRANSITIONS)?;
        let clock = TestClock::default();
        let mut field = field("summary", "string", true);
        field["hasDefaultValue"] = json!(true);
        if let Some(value) = &default {
            field["defaultValue"] = value.clone();
        }
        let mut context = Context::new(transitions(json!({"summary":field})));
        let live = live(&read, &mut context, &clock)?;
        let mut current = state();
        current["fields"]["summary"] = json!("Human");
        context.responses.push_back(current);
        let write = registry()?.prepare(write(TRANSITION_ISSUE, json!({})))?;
        let frozen = live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)?;
        assert_eq!(
            context.calls.len(),
            if default == Some(json!("Default summary")) {
                3
            } else {
                4
            }
        );
        assert_eq!(frozen.call().body, Some(json!({"transition":{"id":"31"}})));
    }
    Ok(())
}

#[test]
fn required_label_deltas_can_fill_empty_current_values_and_cannot_remove_the_last_label()
-> TestResult {
    for (current, delta, allowed) in [
        (json!([]), json!({"add":["new"]}), true),
        (json!(["last"]), json!({"remove":["last"]}), false),
        (json!(["keep", "old"]), json!({"remove":["old"]}), true),
    ] {
        let read = read(issue_context::EDIT)?;
        let clock = TestClock::default();
        let mut metadata = metadata();
        metadata["fields"]["labels"]["required"] = json!(true);
        let mut context = Context::new(metadata);
        let live = live(&read, &mut context, &clock)?;
        let mut raw = state();
        raw["fields"]["labels"] = current;
        context.responses.push_back(raw);
        let write = registry()?.prepare(write(EDIT_ISSUE, json!({"label_delta":delta})))?;
        assert_eq!(
            live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)
                .is_ok(),
            allowed
        );
        assert_eq!(context.calls.len(), 4);
    }
    Ok(())
}

#[test]
fn required_value_read_rejects_provider_error_maps_even_when_fields_are_present() -> TestResult {
    let read = read(issue_context::TRANSITIONS)?;
    let clock = TestClock::default();
    let mut context = Context::new(transitions(
        json!({"summary":field("summary","string",true)}),
    ));
    let live = live(&read, &mut context, &clock)?;
    let mut current = state();
    current["fields"]["summary"] = json!("Human");
    current["errors"] = json!({"summary":"Unqualified response"});
    context.responses.push_back(current);
    let write = registry()?.prepare(write(TRANSITION_ISSUE, json!({})))?;
    assert!(
        live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)
            .is_err()
    );
    Ok(())
}

#[test]
fn write_routes_match_pinned_native_methods_queries_and_success_contracts() -> TestResult {
    use craft_core::execution::{HttpMethod, RouteSegment};
    let source: Value = serde_json::from_slice(&std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../openspec/changes/build-cloud-cli/sources/jira-v3.json"
    ))?)?;
    let registry = registry()?;
    for (command, method, verb) in [
        (EDIT_ISSUE, HttpMethod::Put, "put"),
        (TRANSITION_ISSUE, HttpMethod::Post, "post"),
    ] {
        let descriptor = registry.descriptor(command)?;
        assert_eq!(descriptor.routes.len(), 1);
        assert_eq!(descriptor.scope_alternatives, vec![vec!["write:jira-work"]]);
        let route = &descriptor.routes[0];
        assert_eq!(
            serde_json::to_value(route.method)?,
            serde_json::to_value(method)?
        );
        let path = format!(
            "/{}",
            route
                .path
                .iter()
                .map(|part| match part {
                    RouteSegment::Literal(v) => v.clone(),
                    RouteSegment::Parameter(v) => format!("{{{v}}}"),
                })
                .collect::<Vec<_>>()
                .join("/")
        );
        let native = &source["paths"][&path][verb];
        assert!(native.is_object());
        assert!(native["responses"].get("204").is_some());
        for query in &route.query_parameters {
            assert!(
                native["parameters"]
                    .as_array()
                    .ok_or("parameters")?
                    .iter()
                    .any(|value| value["in"] == "query" && value["name"] == *query)
            );
        }
        assert!(
            !route
                .query_parameters
                .iter()
                .any(|name| name.starts_with("override"))
        );
        assert!(route.continuation_parameters.is_empty());
        assert!(route.repeated_query_parameters.is_empty());
        if command == TRANSITION_ISSUE {
            assert!(route.query_parameters.is_empty());
        }
    }
    Ok(())
}

#[test]
fn untouched_unknown_required_edit_fields_are_preserved_but_transition_requirements_need_qualification()
-> TestResult {
    for (command, allowed) in [(EDIT_ISSUE, true), (TRANSITION_ISSUE, false)] {
        let metadata = json!({"summary":field("summary","string",true),"customfield_1":custom("unqualified-plugin","string",true)});
        let read = read(if command == EDIT_ISSUE {
            issue_context::EDIT
        } else {
            issue_context::TRANSITIONS
        })?;
        let clock = TestClock::default();
        let mut context = Context::new(if command == EDIT_ISSUE {
            json!({"fields":metadata})
        } else {
            transitions(metadata)
        });
        let live = live(&read, &mut context, &clock)?;
        let write = registry()?.prepare(write(command, json!({"summary":"New"})))?;
        let result = live.prepare_mutation(&read, &write, &mut context, &principal(), &clock);
        assert_eq!(result.is_ok(), allowed);
        if let Ok(frozen) = result {
            assert_eq!(
                frozen.call().body,
                Some(json!({"fields":{"summary":"New"}}))
            );
        }
        assert_eq!(context.calls.len(), 3);
    }
    Ok(())
}

#[test]
fn label_deltas_require_each_native_operation_even_when_replacement_is_allowed() -> TestResult {
    for operations in [
        json!(["set"]),
        json!(["set", "add"]),
        json!(["set", "remove"]),
    ] {
        let read = read(issue_context::EDIT)?;
        let clock = TestClock::default();
        let mut metadata = metadata();
        metadata["fields"]["labels"]["operations"] = operations;
        let mut context = Context::new(metadata);
        let live = live(&read, &mut context, &clock)?;
        let write = registry()?.prepare(write(
            EDIT_ISSUE,
            json!({"label_delta":{"add":["new"],"remove":["old"]}}),
        ))?;
        assert!(
            live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)
                .is_err()
        );
        assert_eq!(context.calls.len(), 3);
    }
    Ok(())
}

#[test]
fn metadata_operation_transfer_and_expired_preparation_fail_before_current_value_reads()
-> TestResult {
    let read = read(issue_context::TRANSITIONS)?;
    let clock = TestClock::default();
    let mut context = Context::new(transitions(
        json!({"summary":field("summary","string",true)}),
    ));
    let live = live(&read, &mut context, &clock)?;
    let write = registry()?.prepare(write(TRANSITION_ISSUE, json!({})))?;
    let mut changed = read.request().clone();
    changed.selection.fields.push("name".into());
    let changed = registry()?.prepare(changed)?;
    assert!(
        live.prepare_mutation(&changed, &write, &mut context, &principal(), &clock)
            .is_err()
    );
    clock.1.store(160_000, Ordering::SeqCst);
    assert!(
        live.prepare_mutation(&read, &write, &mut context, &principal(), &clock)
            .is_err()
    );
    assert_eq!(context.calls.len(), 3);
    Ok(())
}
