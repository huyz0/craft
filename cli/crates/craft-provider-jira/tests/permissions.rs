use craft_core::contracts::{CanonicalRequest, Envelope, GrantRef, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    NativeCall, NativeResponse, OperationContext, PreparedOperation, RouteSegment,
};
use craft_core::pagination::NativePosition;
use craft_core::registry::Registry;
use craft_core::storage::Clock;
use craft_provider_jira::create_metadata::MetadataPrincipal;
use craft_provider_jira::permissions::{self, COMMAND, FieldPlan, JiraPermissions};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraPermissions)?;
    Ok(registry)
}
fn request(input: Value, fields: &[&str]) -> CanonicalRequest {
    let cloud = "00000000-1111-2222-3333-444444444444";
    CanonicalRequest {
        command: COMMAND.into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: cloud.into(),
            resource_type: "site".into(),
            id: cloud.into(),
        },
        input,
        selection: Selection {
            fields: fields.iter().map(|field| (*field).into()).collect(),
            ..Selection::default()
        },
        preconditions: None,
    }
}
fn operation(input: Value, fields: &[&str]) -> Result<PreparedOperation, CraftError> {
    registry()?.prepare(request(input, fields))
}
fn plan(operation: &PreparedOperation) -> Result<FieldPlan, CraftError> {
    FieldPlan::compile(&operation.request().selection.fields)
}
fn principal() -> MetadataPrincipal {
    MetadataPrincipal {
        grant: GrantRef {
            id: "gr_00000000000000000000000000000000".into(),
        },
        actor: "actor-one".into(),
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
struct Context {
    body: Value,
    status: u16,
    calls: Vec<NativeCall>,
    deny: bool,
    expire: Option<Arc<TestClock>>,
}
fn context(body: Value) -> Context {
    Context {
        body,
        status: 200,
        calls: vec![],
        deny: false,
        expire: None,
    }
}
impl OperationContext for Context {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, _: &PreparedOperation) -> Result<(), CraftError> {
        if self.deny {
            Err(CraftError::new(ErrorCode::PolicyDenied, "Denied."))
        } else {
            Ok(())
        }
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        if let Some(clock) = &self.expire {
            clock.monotonic.store(160_000, Ordering::SeqCst);
        }
        Ok(NativeResponse {
            status: self.status,
            body: self.body.clone(),
            request_id: None,
            retry_after: None,
            link_headers: None,
        })
    }
    fn normalize_next(&self, _: &NativeCall, _: &str) -> Result<NativeCall, CraftError> {
        Err(CraftError::new(
            ErrorCode::Internal,
            "Unpaginated permissions must not follow links.",
        ))
    }
}
fn row(key: &str, decision: bool) -> Value {
    json!({"key":key,"id":"12","havePermission":decision,"type":"PROJECT","name":"Permission 文字","description":"Description","deprecatedKey":false,"self":"https://untrusted.invalid/never-follow","instruction":"unselected-sentinel"})
}
fn body() -> Value {
    json!({"permissions":{"EDIT_ISSUES":row("EDIT_ISSUES",true)}})
}

#[test]
fn permission_get_route_scopes_parameters_and_models_match_the_pinned_source() -> TestResult {
    let source: Value = serde_json::from_slice(&std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../openspec/changes/build-cloud-cli/sources/jira-v3.json"
    ))?)?;
    let native = &source["paths"]["/rest/api/3/mypermissions"]["get"];
    let registry = registry()?;
    let descriptor = registry.descriptor(COMMAND)?;
    assert_eq!(descriptor.scope_alternatives, vec![vec!["read:jira-work"]]);
    assert!(
        native["x-atlassian-oauth2-scopes"]
            .as_array()
            .ok_or("Missing scopes")?
            .iter()
            .any(
                |scope| scope["state"] == "Current" && scope["scopes"] == json!(["read:jira-work"])
            )
    );
    assert_eq!(descriptor.routes.len(), 1);
    let route = &descriptor.routes[0];
    assert_eq!(serde_json::to_value(route.method)?, json!("get"));
    assert_eq!(
        format!(
            "/{}",
            route
                .path
                .iter()
                .map(|part| match part {
                    RouteSegment::Literal(value) => value.as_str(),
                    _ => "unexpected",
                })
                .collect::<Vec<_>>()
                .join("/")
        ),
        "/rest/api/3/mypermissions"
    );
    for name in &route.query_parameters {
        assert!(
            native["parameters"]
                .as_array()
                .ok_or("Missing params")?
                .iter()
                .any(|p| p["name"] == *name)
        );
    }
    for excluded in ["accountId", "projectUuid", "projectConfigurationUuid"] {
        assert!(!route.query_parameters.iter().any(|p| p == excluded));
    }
    assert!(route.continuation_parameters.is_empty());
    assert_eq!(
        source["components"]["schemas"]["UserPermission"]["properties"]["type"]["enum"],
        json!(["GLOBAL", "PROJECT"])
    );
    Ok(())
}

#[test]
fn explicit_contexts_produce_one_exact_native_selector_and_report_evaluation_semantics()
-> TestResult {
    for (selector, value, parameter, kind, evaluation) in [
        ("global", "", None, "global", "any_project"),
        (
            "project",
            "100",
            Some("projectId"),
            "project",
            "any_qualifying_issue_or_project",
        ),
        (
            "project",
            "ED",
            Some("projectKey"),
            "project",
            "any_qualifying_issue_or_project",
        ),
        (
            "issue",
            "10000",
            Some("issueId"),
            "issue",
            "requested_issue_or_containing_project",
        ),
        (
            "issue",
            "OLD-1",
            Some("issueKey"),
            "issue",
            "requested_issue_or_containing_project",
        ),
        (
            "comment_id",
            "20000",
            Some("commentId"),
            "comment",
            "comment_visibility_and_parent_issue",
        ),
    ] {
        let mut input = json!({"permissions":["BROWSE_PROJECTS"]});
        if selector != "global" {
            input[selector] = json!(value);
        }
        let operation = operation(input, &[])?;
        let mut context =
            context(json!({"permissions":{"BROWSE_PROJECTS":row("BROWSE_PROJECTS",true)}}));
        let observed = permissions::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            None,
            &principal(),
            &TestClock::default(),
        )?;
        assert_eq!(context.calls.len(), 1);
        let call = &context.calls[0];
        assert_eq!(call.route, permissions::ROUTE);
        assert!(call.path_parameters.is_empty() && call.body.is_none());
        assert_eq!(call.query.len(), if parameter.is_some() { 2 } else { 1 });
        assert_eq!(call.query["permissions"], "BROWSE_PROJECTS");
        if let Some(parameter) = parameter {
            assert_eq!(call.query[parameter], value);
        }
        let context = serde_json::to_value(permissions::input(operation.request())?.context()?)?;
        assert_eq!(context["kind"], kind);
        assert_eq!(context["project_permission_evaluation"], evaluation);
        assert_eq!(
            context["requested_selector"],
            if parameter.is_some() {
                json!(value)
            } else {
                Value::Null
            }
        );
        assert!(observed.page().next.is_none() && observed.page().context.is_none());
    }
    Ok(())
}

#[test]
fn ordered_requested_results_keep_missing_entries_flags_and_native_false_distinct() -> TestResult {
    let operation = operation(
        json!({"permissions":["app.plugin:permission","EDIT_ISSUES","BROWSE_PROJECTS","NO_FLAG"]}),
        &[],
    )?;
    let mut raw = body();
    raw["permissions"]["BROWSE_PROJECTS"] = row("BROWSE_PROJECTS", false);
    raw["permissions"]["NO_FLAG"] = json!({"type":"GLOBAL"});
    let result = permissions::read_context(
        &operation,
        &mut context(raw),
        &plan(&operation)?,
        None,
        &principal(),
        &TestClock::default(),
    )?;
    let items = &result.page().items;
    assert_eq!(items.len(), 4);
    assert_eq!(
        items
            .iter()
            .map(|row| row["key"].as_str())
            .collect::<Vec<_>>(),
        vec![
            Some("app.plugin:permission"),
            Some("EDIT_ISSUES"),
            Some("BROWSE_PROJECTS"),
            Some("NO_FLAG")
        ]
    );
    assert_eq!(items[0]["returned"], false);
    assert!(items[0]["have_permission"].is_null());
    assert_eq!(items[1]["have_permission"], true);
    assert_eq!(items[2]["have_permission"], false);
    assert_eq!(items[3]["returned"], true);
    assert!(items[3]["have_permission"].is_null());
    assert_eq!(items[3]["kind"], "GLOBAL");
    for index in [0, 3] {
        assert!(
            items[index]["fields_not_returned"]
                .as_array()
                .ok_or("Missing omission record")?
                .contains(&json!("have_permission"))
        );
    }
    assert!(!serde_json::to_string(result.page())?.contains("unselected-sentinel"));
    Ok(())
}

#[test]
fn malformed_keys_contexts_impersonation_and_unavailable_queries_fail_before_reads() -> TestResult {
    for input in [
        json!({"permissions":[]}),
        json!({"permissions":["EDIT_ISSUES","EDIT_ISSUES"]}),
        json!({"permissions":["EDIT_ISSUES,BROWSE_PROJECTS"]}),
        json!({"permissions":["bad key"]}),
        json!({"permissions":["BROWSE_PROJECTS"],"project":"ED","issue":"ED-1"}),
        json!({"permissions":["EDIT_ISSUES"],"comment_id":"2"}),
        json!({"permissions":["BROWSE_PROJECTS"],"comment_id":"2/3"}),
        json!({"permissions":["EDIT_ISSUES"],"project":"../ED"}),
        json!({"permissions":["EDIT_ISSUES"],"issue":"bad"}),
        json!({"permissions":["EDIT_ISSUES"],"account_id":"other"}),
        json!({"permissions":["EDIT_ISSUES"],"projectUuid":"claimed"}),
    ] {
        assert!(
            registry()?.prepare(request(input.clone(), &[])).is_err(),
            "{input}"
        );
    }
    for values in [
        vec!["x".repeat(257)],
        (0..33).map(|i| format!("PERMISSION_{i}")).collect(),
    ] {
        assert!(operation(json!({"permissions":values}), &[]).is_err());
    }
    let operation = operation(json!({"permissions":["EDIT_ISSUES"]}), &[])?;
    let mut context = context(body());
    let position = NativePosition(BTreeMap::from([("cursor".into(), "untrusted".into())]));
    assert!(
        permissions::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            Some(&position),
            &principal(),
            &TestClock::default()
        )
        .is_err()
    );
    let other_plan = FieldPlan::compile(&["description".into()])?;
    assert!(
        permissions::read_context(
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
        permissions::read_context(
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
fn unexpected_or_malformed_permission_evidence_never_returns_partial_items() -> TestResult {
    let operation = operation(
        json!({"permissions":["EDIT_ISSUES"]}),
        &["name", "description"],
    )?;
    for mode in [
        "extra",
        "key",
        "decision",
        "null_decision",
        "kind",
        "row",
        "missing_map",
        "map",
        "name",
        "description",
        "warnings",
        "errors",
    ] {
        let mut raw = body();
        match mode {
            "extra" => raw["permissions"]["ADMINISTER"] = row("ADMINISTER", true),
            "key" => raw["permissions"]["EDIT_ISSUES"]["key"] = json!("ADMINISTER"),
            "decision" => raw["permissions"]["EDIT_ISSUES"]["havePermission"] = json!("true"),
            "null_decision" => raw["permissions"]["EDIT_ISSUES"]["havePermission"] = Value::Null,
            "kind" => raw["permissions"]["EDIT_ISSUES"]["type"] = json!("FUTURE"),
            "row" => raw["permissions"]["EDIT_ISSUES"] = json!([]),
            "missing_map" => {
                raw = json!({});
            }
            "map" => raw["permissions"] = Value::Null,
            "name" => raw["permissions"]["EDIT_ISSUES"]["name"] = json!("x".repeat(4097)),
            "description" => {
                raw["permissions"]["EDIT_ISSUES"]["description"] = json!("x".repeat(16_385))
            }
            "warnings" => raw["warningMessages"] = json!(["private-sentinel"]),
            _ => raw["errors"] = json!({"permission":"private-sentinel"}),
        }
        let mut context = context(raw);
        let error = permissions::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            None,
            &principal(),
            &TestClock::default(),
        )
        .err()
        .ok_or("Expected evidence refusal")?;
        assert_eq!(error.code, ErrorCode::RemoteUnavailable, "{mode}");
        assert_eq!(context.calls.len(), 1);
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    Ok(())
}

#[test]
fn permission_failures_do_not_infer_denial_absence_or_automatic_retry() -> TestResult {
    let operation = operation(json!({"permissions":["EDIT_ISSUES"]}), &[])?;
    for (status, code) in [
        (400, ErrorCode::InputInvalid),
        (401, ErrorCode::AuthRequired),
        (403, ErrorCode::PermissionDenied),
        (404, ErrorCode::RemoteUnavailable),
        (429, ErrorCode::RateLimited),
        (503, ErrorCode::RemoteUnavailable),
    ] {
        let mut context = context(json!({"errorMessages":["private-sentinel"]}));
        context.status = status;
        let error = permissions::read_context(
            &operation,
            &mut context,
            &plan(&operation)?,
            None,
            &principal(),
            &TestClock::default(),
        )
        .err()
        .ok_or("Expected status failure")?;
        assert_eq!(error.code, code);
        assert!(!error.retryable);
        assert_eq!(context.calls.len(), 1);
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    Ok(())
}

#[test]
fn permission_observation_checks_both_clocks_including_read_time_and_exact_principal_operation()
-> TestResult {
    let operation = operation(json!({"permissions":["EDIT_ISSUES"]}), &[])?;
    let clock = Arc::new(TestClock::default());
    let observed = permissions::read_context(
        &operation,
        &mut context(body()),
        &plan(&operation)?,
        None,
        &principal(),
        clock.as_ref(),
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
        assert_eq!(
            observed
                .verify(&operation, &principal(), clock.as_ref())
                .is_ok(),
            valid
        );
    }
    clock.wall.store(100_000, Ordering::SeqCst);
    clock.monotonic.store(100_000, Ordering::SeqCst);
    let other = self::operation(json!({"permissions":["EDIT_ISSUES"],"issue":"ED-1"}), &[])?;
    assert!(
        observed
            .verify(&other, &principal(), clock.as_ref())
            .is_err()
    );
    for change in ["actor", "grant"] {
        let mut other = principal();
        if change == "actor" {
            other.actor = "actor-two".into();
        } else {
            other.grant.id = "gr_11111111111111111111111111111111".into();
        }
        assert!(observed.verify(&operation, &other, clock.as_ref()).is_err());
    }
    let mut slow = context(body());
    slow.expire = Some(clock.clone());
    assert_eq!(
        permissions::read_context(
            &operation,
            &mut slow,
            &plan(&operation)?,
            None,
            &principal(),
            clock.as_ref()
        )
        .err()
        .ok_or("Expected read expiry")?
        .code,
        ErrorCode::VersionConflict
    );
    Ok(())
}

#[test]
fn every_permission_projection_and_missing_result_matches_its_generated_response_schema()
-> TestResult {
    let registry = registry()?;
    let descriptor = registry.descriptor(COMMAND)?;
    let contract = descriptor.projection.as_ref().ok_or("Missing projection")?;
    for selected in contract.selectable_fields(&descriptor.response_schema)? {
        for raw in [body(), json!({"permissions":{}})] {
            let operation = registry.prepare(request(
                json!({"permissions":["EDIT_ISSUES"]}),
                &[&selected],
            ))?;
            let observed = permissions::read_context(
                &operation,
                &mut context(raw),
                &plan(&operation)?,
                None,
                &principal(),
                &TestClock::default(),
            )?;
            let input = permissions::input(operation.request())?;
            let projection = registry
                .projection(COMMAND, std::slice::from_ref(&selected))?
                .ok_or("Missing compiled projection")?;
            let data = json!({"target":operation.request().target,"context":input.context()?,"requested_permissions":input.permissions,"items":observed.page().items,"observation":null,"cached":false,"credential_cache_update":null,"atomic_snapshot":false,"mutation_authorized":false});
            let envelope = serde_json::to_value(Envelope::success(
                COMMAND,
                "request".into(),
                projection.data(&data)?,
            ))?;
            let validator = jsonschema::validator_for(
                &projection.response_schema(&descriptor.response_schema)?,
            )?;
            let errors: Vec<_> = validator
                .iter_errors(&envelope)
                .map(|error| error.to_string())
                .collect();
            assert!(errors.is_empty(), "{selected}: {errors:?}");
            let item = &envelope["data"]["items"][0];
            for field in ["key", "returned", "have_permission"] {
                assert!(item.get(field).is_some());
            }
        }
    }
    Ok(())
}
