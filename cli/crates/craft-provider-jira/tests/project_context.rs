use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::pagination::NativePosition;
use craft_core::registry::Registry;
use craft_provider_jira::project_context::{
    self, FieldPlan, GET, IdentityResolution, JiraProjectContext, STATUSES,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraProjectContext)?;
    Ok(registry)
}
fn request(command: &str, id: &str, fields: &[&str]) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "project".into(),
            id: id.into(),
        },
        input: json!({}),
        selection: Selection {
            fields: fields.iter().map(|field| (*field).into()).collect(),
            ..Selection::default()
        },
        preconditions: None,
    }
}
fn operation(command: &str, id: &str, fields: &[&str]) -> Result<PreparedOperation, CraftError> {
    registry()?.prepare(request(command, id, fields))
}
fn plan(op: &PreparedOperation) -> Result<FieldPlan, CraftError> {
    FieldPlan::compile(&op.request().command, &op.request().selection.fields)
}
fn project() -> Value {
    json!({"id":"10000","key":"ED","name":"Example 文字","projectTypeKey":"software","description":"private-sentinel","lead":{"emailAddress":"private-sentinel"},"self":"https://untrusted.invalid","versions":17})
}
fn group() -> Value {
    json!({"id":"3","name":"Task","subtask":false,"self":"https://untrusted.invalid","statuses":[{"id":"1","name":"Open","description":"private-sentinel","self":"https://untrusted.invalid","statusCategory":{"id":2,"key":"new","name":"To Do","colorName":"blue"}}]})
}
struct Context {
    body: Value,
    status: u16,
    calls: Vec<NativeCall>,
    deny: bool,
}
fn context(body: Value) -> Context {
    Context {
        body,
        status: 200,
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
            Err(CraftError::new(ErrorCode::PolicyDenied, "Wrong binding."))
        } else {
            Ok(())
        }
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
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
            ErrorCode::PolicyDenied,
            "Native status continuation does not exist.",
        ))
    }
}

#[test]
fn compiled_project_context_routes_scopes_and_schemas_match_the_pinned_source() -> TestResult {
    let source: Value = serde_json::from_slice(&std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../openspec/changes/build-cloud-cli/sources/jira-v3.json"
    ))?)?;
    for (command, path) in [
        (GET, "/rest/api/3/project/{projectIdOrKey}"),
        (STATUSES, "/rest/api/3/project/{projectIdOrKey}/statuses"),
    ] {
        let registry = registry()?;
        let descriptor = registry.descriptor(command)?;
        let route = &descriptor.routes[0];
        let actual = format!(
            "/{}",
            route
                .path
                .iter()
                .map(|part| match part {
                    craft_core::execution::RouteSegment::Literal(value) => value.clone(),
                    craft_core::execution::RouteSegment::Parameter(value) => format!("{{{value}}}"),
                })
                .collect::<Vec<_>>()
                .join("/")
        );
        assert_eq!(actual, path);
        assert_eq!(serde_json::to_value(route.method)?, json!("get"));
        assert!(route.query_parameters.is_empty());
        assert!(route.continuation_parameters.is_empty());
        let native = &source["paths"][path]["get"];
        assert!(
            native["x-atlassian-oauth2-scopes"]
                .as_array()
                .ok_or("Missing scopes")?
                .iter()
                .any(|scope| scope["state"] == "Current"
                    && scope["scopes"] == json!(["read:jira-work"]))
        );
        assert_eq!(descriptor.scope_alternatives, vec![vec!["read:jira-work"]]);
        let projection = descriptor.projection.as_ref().ok_or("Missing projection")?;
        assert!(matches!(
            projection.mode,
            craft_core::projection::ProjectionMode::Local
        ));
        let fields = projection.selectable_fields(&descriptor.response_schema)?;
        assert!(fields.contains(&"id".into()));
        for field in fields {
            assert!(registry.projection(command, &[field])?.is_some());
        }
    }
    assert!(FieldPlan::compile("jira.project.other", &[]).is_err());
    Ok(())
}

#[test]
fn get_retains_actual_identity_and_selected_metadata_without_expansions_or_user_data() -> TestResult
{
    for (selector, resolution) in [
        ("10000", IdentityResolution::ExactId),
        ("ED", IdentityResolution::ExactKey),
    ] {
        let op = operation(GET, selector, &[])?;
        let mut context = context(project());
        let (out, kind) = project_context::read_get(&op, &mut context, &plan(&op)?)?;
        assert_eq!(kind, resolution);
        assert_eq!(
            out,
            json!({"id":"10000","key":"ED","name":"Example 文字","project_type":"software"})
        );
        assert_eq!(context.calls.len(), 1);
        assert_eq!(context.calls[0].path_parameters["projectIdOrKey"], selector);
        assert!(context.calls[0].query.is_empty());
        assert!(context.calls[0].body.is_none());
        assert!(registry()?.execute(&op, &mut context).is_err());
        assert_eq!(context.calls.len(), 1);
    }
    for selector in ["10001", "ed", "OLD"] {
        let op = operation(GET, selector, &[])?;
        assert!(project_context::read_get(&op, &mut context(project()), &plan(&op)?).is_err());
    }
    Ok(())
}

#[test]
fn get_missing_null_explicit_description_and_unselected_malformed_fields_are_distinct() -> TestResult
{
    let op = operation(GET, "ED", &["description", "archived", "deleted", "name"])?;
    let mut raw = project();
    raw["description"] = json!("Résumé 文字");
    raw["archived"] = Value::Null;
    raw["lead"] = json!(false);
    raw["simplified"] = json!(17);
    let (out, _) = project_context::read_get(&op, &mut context(raw.clone()), &plan(&op)?)?;
    assert_eq!(
        out,
        json!({"id":"10000","name":"Example 文字","description":"Résumé 文字","archived":null,"fields_not_returned":["deleted"]})
    );
    raw["description"] = json!("x".repeat(65_537));
    assert!(project_context::read_get(&op, &mut context(raw), &plan(&op)?).is_err());
    Ok(())
}

#[test]
fn status_groups_default_to_identity_and_names_without_flattening_or_continuation() -> TestResult {
    let op = operation(STATUSES, "ED", &[])?;
    let mut context = context(json!([group()]));
    let page = project_context::read_statuses(&op, &mut context, &plan(&op)?, None)?;
    assert_eq!(
        page.items,
        vec![json!({"id":"3","name":"Task","subtask":false,"statuses":[{"id":"1","name":"Open"}]})]
    );
    assert!(page.next.is_none());
    assert_eq!(context.calls.len(), 1);
    assert_eq!(context.calls[0].path_parameters["projectIdOrKey"], "ED");
    assert!(context.calls[0].query.is_empty());
    let mut other = group();
    other["id"] = json!("4");
    assert_eq!(
        project_context::read_statuses(
            &op,
            &mut context_for(json!([group(), other])),
            &plan(&op)?,
            None
        )?
        .items
        .len(),
        2
    );
    assert!(
        project_context::read_statuses(&op, &mut context_for(json!([])), &plan(&op)?, None)?
            .items
            .is_empty()
    );
    Ok(())
}
fn context_for(body: Value) -> Context {
    context(body)
}

#[test]
fn nested_status_selection_preserves_missing_null_and_category_types() -> TestResult {
    let op = operation(
        STATUSES,
        "10000",
        &[
            "statuses.name",
            "statuses.category.id",
            "statuses.category.name",
        ],
    )?;
    let raw = json!([{"id":"3","name":17,"statuses":[{"id":"1","name":null,"statusCategory":{"id":2,"name":null,"key":17}},{"id":"2","statusCategory":{}}]}]);
    let page = project_context::read_statuses(&op, &mut context(raw), &plan(&op)?, None)?;
    assert_eq!(
        page.items[0],
        json!({"id":"3","statuses":[{"name":null,"category":{"id":2,"name":null}},{"category":{}}],"fields_not_returned":["statuses.category.id","statuses.category.name","statuses.name"]})
    );
    let mut raw = group();
    raw["statuses"] = Value::Null;
    assert!(
        project_context::read_statuses(&op, &mut context(json!([raw])), &plan(&op)?, None)?.items
            [0]
        .get("fields_not_returned")
        .is_none()
    );
    Ok(())
}

#[test]
fn unknown_input_projection_target_get_cursor_and_mismatched_plans_fail_before_calls() -> TestResult
{
    for selector in [
        " ED",
        "https://untrusted.invalid",
        "ED/other",
        "ED?expand=lead",
    ] {
        assert!(operation(GET, selector, &[]).is_err());
    }
    for command in [GET, STATUSES] {
        let mut request = request(command, "ED", &[]);
        request.input = json!({"expand":"lead"});
        assert!(registry()?.prepare(request).is_err());
    }
    let mut request = request(GET, "ED", &[]);
    request.selection.cursor = Some("opaque".into());
    assert!(registry()?.prepare(request).is_err());
    let op = operation(GET, "ED", &["id"])?;
    let mut context = context(project());
    assert!(
        project_context::read_get(
            &op,
            &mut context,
            &FieldPlan::compile(GET, &["description".into()])?
        )
        .is_err()
    );
    assert!(context.calls.is_empty());
    context.deny = true;
    assert!(project_context::read_get(&op, &mut context, &plan(&op)?).is_err());
    assert!(context.calls.is_empty());
    let op = operation(STATUSES, "ED", &[])?;
    let mut context = context_for(json!([]));
    assert!(
        project_context::read_statuses(
            &op,
            &mut context,
            &plan(&op)?,
            Some(&NativePosition(BTreeMap::from([(
                "startAt".into(),
                "1".into()
            )])))
        )
        .is_err()
    );
    assert!(context.calls.is_empty());
    assert!(FieldPlan::compile(GET, &["lead.email".into()]).is_err());
    Ok(())
}

#[test]
fn duplicate_group_or_within_group_status_ids_and_malformed_selected_values_refuse_page()
-> TestResult {
    let op = operation(STATUSES, "ED", &[])?;
    let mut duplicate = group();
    duplicate["statuses"] = json!([{"id":"1","name":"Open"},{"id":"1","name":"Other"}]);
    let mut malformed = group();
    malformed["statuses"][0]["name"] = json!(17);
    for raw in [
        json!([group(), group()]),
        json!([duplicate]),
        json!([malformed]),
        json!({}),
        json!([{"id":17}]),
        json!(vec![group(); 1001]),
    ] {
        assert!(project_context::read_statuses(&op, &mut context(raw), &plan(&op)?, None).is_err());
    }
    let id_only = operation(STATUSES, "ED", &["id"])?;
    let mut group = group();
    group["statuses"] = json!("private-sentinel");
    assert_eq!(
        project_context::read_statuses(
            &id_only,
            &mut context(json!([group])),
            &plan(&id_only)?,
            None
        )?
        .items,
        vec![json!({"id":"3"})]
    );
    Ok(())
}

#[test]
fn response_byte_structure_and_status_bounds_apply_even_to_mock_transport() -> TestResult {
    let op = operation(GET, "ED", &["id"])?;
    let mut nested = Value::Null;
    for _ in 0..70 {
        nested = json!([nested]);
    }
    for value in [nested, json!("x".repeat(1_048_576)), json!(vec![0; 10_001])] {
        let mut raw = project();
        raw["ignored"] = value;
        assert!(project_context::read_get(&op, &mut context(raw), &plan(&op)?).is_err());
    }
    Ok(())
}

#[test]
fn http_denial_is_sanitized_single_attempt_and_does_not_prove_absence() -> TestResult {
    for command in [GET, STATUSES] {
        let op = operation(command, "ED", &[])?;
        for (status, expected) in [
            (401, ErrorCode::AuthRequired),
            (403, ErrorCode::PermissionDenied),
            (404, ErrorCode::RemoteUnavailable),
            (429, ErrorCode::RateLimited),
            (500, ErrorCode::RemoteUnavailable),
        ] {
            let mut context = context(json!({"message":"private-sentinel"}));
            context.status = status;
            let error = if command == GET {
                project_context::read_get(&op, &mut context, &plan(&op)?).err()
            } else {
                project_context::read_statuses(&op, &mut context, &plan(&op)?, None).err()
            }
            .ok_or("Expected failure")?;
            assert_eq!(error.code, expected);
            if status == 404 {
                assert!(error.message.contains("does not establish absence"));
            }
            assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
            assert_eq!(context.calls.len(), 1);
        }
    }
    Ok(())
}
