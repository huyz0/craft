use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use craft_core::contracts::{CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    NativeCall, NativeResponse, OperationContext, PreparedOperation, RouteSegment,
};
use craft_core::pagination::NativePosition;
use craft_core::registry::Registry;
use craft_core::storage::Clock;
use craft_provider_jira::create_metadata::MetadataPrincipal;
use craft_provider_jira::issue_context::{
    self, EDIT, FieldPlan, IssueObservation, JiraIssueContext, TRANSITIONS,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraIssueContext)?;
    Ok(registry)
}
fn request(command: &str, issue: &str, fields: &[&str]) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "issue".into(),
            id: issue.into(),
        },
        input: json!({"issue":issue}),
        selection: Selection {
            fields: fields.iter().map(|value| (*value).into()).collect(),
            ..Selection::default()
        },
        preconditions: None,
    }
}
fn operation(command: &str, issue: &str, fields: &[&str]) -> Result<PreparedOperation, CraftError> {
    registry()?.prepare(request(command, issue, fields))
}
fn plan(operation: &PreparedOperation) -> Result<FieldPlan, CraftError> {
    FieldPlan::compile(
        &operation.request().command,
        &operation.request().selection.fields,
    )
}
fn principal() -> MetadataPrincipal {
    MetadataPrincipal {
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        actor: "account-one".into(),
    }
}
struct TestClock {
    wall: AtomicU64,
    monotonic: AtomicU64,
}
impl Default for TestClock {
    fn default() -> Self {
        Self {
            wall: AtomicU64::new(100_000),
            monotonic: AtomicU64::new(100_000),
        }
    }
}
impl Clock for TestClock {
    fn unix_millis(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn monotonic_millis(&self) -> u64 {
        self.monotonic.load(Ordering::SeqCst)
    }
}
fn state() -> Value {
    json!({"id":"10000","key":"ED-7","fields":{"project":{"id":"100"},"issuetype":{"id":"101"},"status":{"id":"102"},"updated":"2026-11-10T01:00:00.000+0000"},"self":"https://untrusted.invalid/never-follow"})
}
fn field() -> Value {
    json!({"key":"native-key","name":"Field 文字","required":true,"operations":["set","future-operation"],"schema":{"type":"future-type","items":"future-items","system":"summary","custom":"https://untrusted.invalid/plugin","customId":1,"configuration":{"$ref":"https://untrusted.invalid/schema"}},"hasDefaultValue":false,"allowedValues":[{"id":"1","instruction":"inert"}],"defaultValue":{"instruction":"inert"},"configuration":{"instruction":"inert"},"autoCompleteUrl":"https://untrusted.invalid/never-follow","private":"unselected-sentinel"})
}
fn metadata() -> Value {
    json!({"fields":{"customfield_1":field()}})
}
fn transition() -> Value {
    json!({"id":"31","name":"Complete","to":{"id":"103","name":"Done","description":"Destination","statusCategory":{"id":3,"key":"done","name":"Done"}},"hasScreen":true,"isAvailable":false,"isConditional":true,"isGlobal":false,"isInitial":false,"looped":false,"fields":{"customfield_1":field()}})
}
struct Context {
    responses: VecDeque<NativeResponse>,
    calls: Vec<NativeCall>,
    deny: bool,
}
fn response(body: Value) -> NativeResponse {
    NativeResponse {
        status: 200,
        body,
        request_id: None,
        retry_after: None,
        link_headers: None,
    }
}
fn context(body: Value) -> Context {
    Context {
        responses: vec![response(state()), response(body), response(state())].into(),
        calls: vec![],
        deny: false,
    }
}
impl OperationContext for Context {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, _: &PreparedOperation) -> Result<(), CraftError> {
        if self.deny {
            Err(CraftError::new(ErrorCode::PolicyDenied, "Binding denied."))
        } else {
            Ok(())
        }
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        self.responses
            .pop_front()
            .ok_or_else(|| CraftError::new(ErrorCode::Internal, "Unexpected fixture call."))
    }
    fn normalize_next(&self, _: &NativeCall, _: &str) -> Result<NativeCall, CraftError> {
        Err(CraftError::new(
            ErrorCode::Internal,
            "Metadata must not follow links.",
        ))
    }
}

#[test]
fn issue_metadata_routes_and_query_parameters_match_pinned_api_without_admin_bypasses() -> TestResult
{
    let source: Value = serde_json::from_slice(&std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../openspec/changes/build-cloud-cli/sources/jira-v3.json"
    ))?)?;
    let registry = registry()?;
    for command in [EDIT, TRANSITIONS] {
        let descriptor = registry.descriptor(command)?;
        assert_eq!(descriptor.scope_alternatives, vec![vec!["read:jira-work"]]);
        assert_eq!(descriptor.routes.len(), 2);
        for route in &descriptor.routes {
            let path = format!(
                "/{}",
                route
                    .path
                    .iter()
                    .map(|segment| match segment {
                        RouteSegment::Literal(value) => value.clone(),
                        RouteSegment::Parameter(value) => format!("{{{value}}}"),
                    })
                    .collect::<Vec<_>>()
                    .join("/")
            );
            let native = &source["paths"][&path]["get"];
            assert!(native.is_object(), "{path}");
            assert_eq!(serde_json::to_value(route.method)?, json!("get"));
            let parameters = native["parameters"]
                .as_array()
                .ok_or("Missing parameters")?;
            for name in &route.query_parameters {
                assert!(
                    parameters
                        .iter()
                        .any(|parameter| parameter["name"] == *name),
                    "{path}: {name}"
                );
            }
            for bypass in [
                "overrideScreenSecurity",
                "overrideEditableFlag",
                "skipRemoteOnlyCondition",
            ] {
                assert!(!route.query_parameters.iter().any(|name| name == bypass));
            }
            assert!(route.continuation_parameters.is_empty());
        }
    }
    Ok(())
}

#[test]
fn bracket_reads_resolve_aliases_and_route_metadata_and_recheck_to_numeric_identity() -> TestResult
{
    for (selector, resolution) in [
        ("10000", "exact_id"),
        ("ed-7", "matching_key"),
        ("OLD-1", "resolved_key"),
    ] {
        let operation = operation(EDIT, selector, &[])?;
        let mut context = context(metadata());
        let live = issue_context::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            None,
            &principal(),
            &TestClock::default(),
        )?;
        assert_eq!(context.calls.len(), 3);
        assert_eq!(context.calls[0].path_parameters["issueIdOrKey"], selector);
        for index in [1, 2] {
            assert_eq!(
                context.calls[index].path_parameters["issueIdOrKey"],
                "10000"
            );
        }
        for index in [0, 2] {
            assert_eq!(context.calls[index].route, issue_context::IDENTITY_ROUTE);
            assert_eq!(
                context.calls[index].query,
                BTreeMap::from([
                    ("fields".into(), "project,issuetype,status,updated".into()),
                    ("fieldsByKeys".into(), "false".into()),
                    ("updateHistory".into(), "false".into()),
                    ("failFast".into(), "true".into()),
                ])
            );
        }
        assert!(context.calls[1].query.is_empty());
        let observation = live.page().context.as_ref().ok_or("Missing observation")?;
        assert_eq!(observation["resolution"], resolution);
        assert_eq!(observation["resolved_issue"]["id"], "10000");
        let typed: IssueObservation = serde_json::from_value(observation.clone())?;
        typed.validate_for_target(&operation.request().target)?;
        assert!(!serde_json::to_string(live.page())?.contains("untrusted"));
    }
    Ok(())
}

#[test]
fn state_drift_never_returns_live_context() -> TestResult {
    let operation = operation(EDIT, "ED-7", &[])?;
    for pointer in [
        "/id",
        "/key",
        "/fields/project/id",
        "/fields/issuetype/id",
        "/fields/status/id",
        "/fields/updated",
    ] {
        let mut context = context(metadata());
        *context.responses[2]
            .body
            .pointer_mut(pointer)
            .ok_or("Missing fixture state")? =
            json!(if pointer == "/key" { "ED-8" } else { "20000" });
        let error = issue_context::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            None,
            &principal(),
            &TestClock::default(),
        )
        .err()
        .ok_or("Expected state refusal")?;
        assert!(matches!(
            error.code,
            ErrorCode::VersionConflict | ErrorCode::RemoteUnavailable
        ));
        assert_eq!(context.calls.len(), 3);
    }
    Ok(())
}

#[test]
fn edit_metadata_validates_mandatory_members_even_when_unselected_and_preserves_inert_schema()
-> TestResult {
    let operation = operation(EDIT, "10000", &["id"])?;
    for name in ["key", "name", "required", "operations", "schema"] {
        let mut body = metadata();
        body["fields"]["customfield_1"]
            .as_object_mut()
            .ok_or("Missing field")?
            .remove(name);
        assert!(
            issue_context::read_context(
                &operation,
                &mut context(body),
                &plan(&operation)?,
                None,
                &principal(),
                &TestClock::default()
            )
            .is_err(),
            "{name}"
        );
    }
    let selected = self::operation(
        EDIT,
        "10000",
        &[
            "key",
            "schema",
            "operations",
            "configuration",
            "default_value",
            "allowed_values",
        ],
    )?;
    let live = issue_context::read_context(
        &selected,
        &mut context(metadata()),
        &plan(&selected)?,
        None,
        &principal(),
        &TestClock::default(),
    )?;
    let item = &live.page().items[0];
    assert_eq!(item["id"], "customfield_1");
    assert_eq!(item["key"], "native-key");
    assert_eq!(item["schema"]["type"], "future-type");
    assert_eq!(item["operations"], json!(["set", "future-operation"]));
    assert_eq!(item["default_value"], json!({"instruction":"inert"}));
    assert!(item.get("autoCompleteUrl").is_none());
    assert!(item.get("name").is_none());
    Ok(())
}

#[test]
fn selected_null_and_missing_values_remain_distinct() -> TestResult {
    let operation = operation(
        EDIT,
        "10000",
        &["default_value", "allowed_values", "has_default"],
    )?;
    let mut body = metadata();
    let field = body["fields"]["customfield_1"]
        .as_object_mut()
        .ok_or("Missing field")?;
    field.insert("defaultValue".into(), Value::Null);
    field.insert("allowedValues".into(), Value::Null);
    field.remove("hasDefaultValue");
    let live = issue_context::read_context(
        &operation,
        &mut context(body),
        &plan(&operation)?,
        None,
        &principal(),
        &TestClock::default(),
    )?;
    let item = &live.page().items[0];
    assert_eq!(item.get("default_value"), Some(&Value::Null));
    assert_eq!(item.get("allowed_values"), Some(&Value::Null));
    assert!(item.get("has_default").is_none());
    assert_eq!(item["fields_not_returned"], json!(["has_default"]));
    Ok(())
}

#[test]
fn transition_filter_flags_and_field_expansion_derive_only_from_typed_selection() -> TestResult {
    for fields in [vec!["id"], vec!["fields.schema.type"]] {
        let mut request = request(TRANSITIONS, "ED-7", &fields);
        request.input = json!({"issue":"ED-7","transition_id":"31","include_unavailable":true,"sort_by_status":true});
        let operation = registry()?.prepare(request)?;
        let mut context = context(json!({"transitions":[transition()]}));
        let live = issue_context::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            None,
            &principal(),
            &TestClock::default(),
        )?;
        let query = &context.calls[1].query;
        assert_eq!(query["transitionId"], "31");
        assert_eq!(query["includeUnavailableTransitions"], "true");
        assert_eq!(query["sortByOpsBarAndStatus"], "true");
        assert_eq!(
            query.get("expand").map(String::as_str),
            fields[0]
                .starts_with("fields")
                .then_some("transitions.fields")
        );
        assert_eq!(live.page().items[0]["id"], "31");
        if fields[0].starts_with("fields") {
            assert_eq!(
                live.page().items[0]["fields"][0]["schema"]["type"],
                "future-type"
            );
        }
    }
    let operation = operation(TRANSITIONS, "10000", &[])?;
    for body in [
        json!({"transitions":[transition(), transition()]}),
        json!({"transitions":[{"name":"Missing ID"}]}),
    ] {
        assert!(
            issue_context::read_context(
                &operation,
                &mut context(body),
                &plan(&operation)?,
                None,
                &principal(),
                &TestClock::default()
            )
            .is_err()
        );
    }
    let mut request = request(TRANSITIONS, "10000", &[]);
    request.input["transition_id"] = json!("32");
    let filtered = registry()?.prepare(request)?;
    assert!(
        issue_context::read_context(
            &filtered,
            &mut context(json!({"transitions":[transition()]})),
            &plan(&filtered)?,
            None,
            &principal(),
            &TestClock::default()
        )
        .is_err()
    );
    let empty = issue_context::read_context(
        &operation,
        &mut context(json!({"transitions":[]})),
        &plan(&operation)?,
        None,
        &principal(),
        &TestClock::default(),
    )?;
    assert!(empty.page().items.is_empty());
    assert!(empty.page().context.is_some());
    Ok(())
}

#[test]
fn freshness_expires_on_either_clock_and_rejects_operation_or_principal_transfer() -> TestResult {
    let operation = operation(EDIT, "10000", &[])?;
    let clock = TestClock::default();
    let live = issue_context::read_context(
        &operation,
        &mut context(metadata()),
        &plan(&operation)?,
        None,
        &principal(),
        &clock,
    )?;
    for (wall, monotonic, valid) in [
        (159_999, 159_999, true),
        (160_000, 100_000, false),
        (100_000, 160_000, false),
        (99_999, 100_000, false),
        (100_000, 99_999, false),
    ] {
        clock.wall.store(wall, Ordering::SeqCst);
        clock.monotonic.store(monotonic, Ordering::SeqCst);
        assert_eq!(live.verify(&operation, &principal(), &clock).is_ok(), valid);
    }
    clock.wall.store(100_000, Ordering::SeqCst);
    clock.monotonic.store(100_000, Ordering::SeqCst);
    for change in ["actor", "grant"] {
        let mut other = principal();
        if change == "actor" {
            other.actor = "account-two".into();
        } else {
            other.grant.id = "gr_11111111111111111111111111111111".into();
        }
        assert_eq!(
            live.verify(&operation, &other, &clock)
                .err()
                .ok_or("Expected principal refusal")?
                .code,
            ErrorCode::PolicyDenied
        );
    }
    assert_eq!(
        live.verify(
            &self::operation(EDIT, "10000", &["id"])?,
            &principal(),
            &clock
        )
        .err()
        .ok_or("Expected operation refusal")?
        .code,
        ErrorCode::PolicyDenied
    );
    Ok(())
}

#[test]
fn malformed_native_metadata_warnings_and_denials_do_not_leak_or_retry() -> TestResult {
    let operation = operation(
        EDIT,
        "10000",
        &["configuration", "allowed_values", "default_value"],
    )?;
    for body in [
        json!({"fields":{},"warningMessages":["private-sentinel"]}),
        json!({"fields":{},"errorMessages":["private-sentinel"]}),
        json!({"fields":null}),
        json!({"fields":{},"warningMessages":null}),
    ] {
        let mut context = context(body);
        let error = issue_context::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            None,
            &principal(),
            &TestClock::default(),
        )
        .err()
        .ok_or("Expected malformed refusal")?;
        assert_eq!(context.calls.len(), 2);
        assert!(!error.message.contains("private-sentinel"));
    }
    for (status, code) in [
        (401, ErrorCode::AuthRequired),
        (403, ErrorCode::PermissionDenied),
        (404, ErrorCode::RemoteUnavailable),
        (429, ErrorCode::RateLimited),
        (503, ErrorCode::RemoteUnavailable),
    ] {
        let mut context = context(metadata());
        context.responses[1].status = status;
        context.responses[1].body = json!({"errorMessages":["private-sentinel"]});
        let error = issue_context::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            None,
            &principal(),
            &TestClock::default(),
        )
        .err()
        .ok_or("Expected HTTP refusal")?;
        assert_eq!(error.code, code);
        assert!(!error.retryable);
        assert_eq!(context.calls.len(), 2);
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    for (name, value) in [
        ("configuration", json!({"x":"x".repeat(32_768)})),
        ("allowedValues", json!(vec![json!({}); 1001])),
        ("defaultValue", json!("x".repeat(32_768))),
    ] {
        let mut body = metadata();
        body["fields"]["customfield_1"][name] = value;
        assert!(
            issue_context::read_context(
                &operation,
                &mut context(body),
                &plan(&operation)?,
                None,
                &principal(),
                &TestClock::default()
            )
            .is_err(),
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn local_input_plan_cursor_and_access_refusals_precede_resource_reads() -> TestResult {
    for command in [EDIT, TRANSITIONS] {
        for bypass in [
            "overrideScreenSecurity",
            "overrideEditableFlag",
            "skipRemoteOnlyCondition",
            "status",
        ] {
            let mut request = request(command, "10000", &[]);
            request.input[bypass] = json!(true);
            assert!(registry()?.prepare(request).is_err());
        }
        let mut wrong_target = request(command, "10000", &[]);
        wrong_target.target.id = "10001".into();
        assert!(registry()?.prepare(wrong_target).is_err());
    }
    let operation = operation(EDIT, "10000", &[])?;
    let mut context = context(metadata());
    let position = NativePosition(BTreeMap::from([("cursor".into(), "untrusted".into())]));
    assert!(
        issue_context::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            Some(&position),
            &principal(),
            &TestClock::default()
        )
        .is_err()
    );
    let other_plan = FieldPlan::compile(EDIT, &["id".into()])?;
    assert!(
        issue_context::read_context(
            &operation,
            &mut context,
            &other_plan,
            None,
            &principal(),
            &TestClock::default()
        )
        .is_err()
    );
    context.deny = true;
    assert!(
        issue_context::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            None,
            &principal(),
            &TestClock::default()
        )
        .is_err()
    );
    assert!(context.calls.is_empty());
    Ok(())
}

#[test]
fn every_selectable_field_produces_schema_valid_projected_output() -> TestResult {
    let registry = registry()?;
    for command in [EDIT, TRANSITIONS] {
        let descriptor = registry.descriptor(command)?;
        let projection = descriptor.projection.as_ref().ok_or("Missing projection")?;
        for selected in projection.selectable_fields(&descriptor.response_schema)? {
            let operation = registry.prepare(request(command, "10000", &[&selected]))?;
            let body = if command == EDIT {
                metadata()
            } else {
                json!({"transitions":[transition()]})
            };
            let live = issue_context::read_context(
                &operation,
                &mut context(body),
                &plan(&operation)?,
                None,
                &principal(),
                &TestClock::default(),
            )?;
            let compiled = registry
                .projection(command, std::slice::from_ref(&selected))?
                .ok_or("Missing compiled projection")?;
            let data = json!({"target":operation.request().target,"items":live.page().items,"issue_observation":live.page().context,"observation":null,"cached":false,"credential_cache_update":null,"atomic_snapshot":false,"mutation_authorized":false});
            let output = Envelope::success(command, "request-one".into(), compiled.data(&data)?);
            let validator =
                jsonschema::validator_for(&compiled.response_schema(&descriptor.response_schema)?)?;
            let value = serde_json::to_value(output)?;
            let errors: Vec<_> = validator
                .iter_errors(&value)
                .map(|error| error.to_string())
                .collect();
            assert!(errors.is_empty(), "{command}/{selected}: {errors:?}");
        }
    }
    Ok(())
}
