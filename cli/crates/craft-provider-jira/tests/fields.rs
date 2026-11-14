use std::collections::BTreeMap;

use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::pagination::NativePosition;
use craft_core::registry::Registry;
use craft_provider_jira::fields::{self, FieldPlan, JiraFields, LIST, SEARCH};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraFields)?;
    Ok(registry)
}
fn request(command: &str, input: Value, fields: &[&str]) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "site".into(),
            id: "00000000-1111-2222-3333-444444444444".into(),
        },
        input,
        selection: Selection {
            fields: fields.iter().map(|field| (*field).into()).collect(),
            ..Selection::default()
        },
        preconditions: None,
    }
}
fn operation(
    command: &str,
    input: Value,
    fields: &[&str],
) -> Result<PreparedOperation, CraftError> {
    registry()?.prepare(request(command, input, fields))
}
fn plan(op: &PreparedOperation) -> Result<FieldPlan, CraftError> {
    FieldPlan::compile(&op.request().command, &op.request().selection.fields)
}
fn field(id: &str) -> Value {
    json!({"id":id,"name":"Field 文字","schema":{"type":"array","items":"future-type","custom":"https://untrusted.invalid/plugin","customId":1,"configuration":{"$ref":"https://untrusted.invalid/schema","instructions":"private-sentinel"}},"description":"private-sentinel","custom":true,"key":id,"clauseNames":["cf[1]","Name, Literal"],"scope":{"type":"PROJECT","project":{"id":"10000","key":"ED","name":"Example","self":"https://untrusted.invalid"}},"stableId":"sfid:example","isLocked":false,"lastUsed":{"type":"NO_INFORMATION"},"screensCount":2,"projectsCount":2})
}
fn page(values: Value) -> Value {
    json!({"startAt":0,"maxResults":50,"isLast":true,"values":values})
}
struct Context {
    body: Value,
    status: u16,
    calls: Vec<NativeCall>,
    deny: bool,
    normalized: Option<NativeCall>,
}
fn context(body: Value) -> Context {
    Context {
        body,
        status: 200,
        calls: vec![],
        deny: false,
        normalized: None,
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
        Ok(NativeResponse {
            status: self.status,
            body: self.body.clone(),
            request_id: None,
            retry_after: None,
            link_headers: None,
        })
    }
    fn normalize_next(&self, call: &NativeCall, _: &str) -> Result<NativeCall, CraftError> {
        Ok(self.normalized.clone().unwrap_or_else(|| {
            let mut next = call.clone();
            next.query.insert("startAt".into(), "2".into());
            next
        }))
    }
}
#[test]
fn field_contracts_match_pinned_routes_scopes_ordering_and_local_projection() -> TestResult {
    let source: Value = serde_json::from_slice(&std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../openspec/changes/build-cloud-cli/sources/jira-v3.json"
    ))?)?;
    for (command, path) in [
        (LIST, "/rest/api/3/field"),
        (SEARCH, "/rest/api/3/field/search"),
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
                    craft_core::execution::RouteSegment::Literal(value) => value.as_str(),
                    _ => "",
                })
                .collect::<Vec<_>>()
                .join("/")
        );
        assert_eq!(actual, path);
        assert_eq!(serde_json::to_value(route.method)?, json!("get"));
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
        assert!(
            descriptor
                .permissions
                .iter()
                .all(|permission| !permission.contains("Administer"))
        );
        let projection = descriptor.projection.as_ref().ok_or("Missing projection")?;
        assert!(matches!(
            projection.mode,
            craft_core::projection::ProjectionMode::Local
        ));
        for selected in projection.selectable_fields(&descriptor.response_schema)? {
            let op = operation(command, json!({}), &[&selected])?;
            let mut ctx = context(if command == LIST {
                json!([field("customfield_1")])
            } else {
                page(json!([field("customfield_1")]))
            });
            let read = fields::read_page(&op, &mut ctx, &plan(&op)?, None)?;
            let data = json!({"target":op.request().target,"items":read.items,"coverage":fields::coverage(command)?,"observation":null,"cached":false,"credential_cache_update":null});
            let compiled = registry
                .projection(command, &[selected])?
                .ok_or("Missing projection")?;
            let envelope = craft_core::contracts::Envelope::success(
                command,
                "request".into(),
                compiled.data(&data)?,
            );
            assert!(
                jsonschema::validator_for(&compiled.response_schema(&descriptor.response_schema)?)?
                    .is_valid(&serde_json::to_value(envelope)?)
            );
        }
        if command == SEARCH {
            let params = native["parameters"].as_array().ok_or("Missing params")?;
            for repeated in &route.repeated_query_parameters {
                assert!(
                    params
                        .iter()
                        .any(|p| p["name"] == *repeated && p["schema"]["type"] == "array")
                );
            }
            let order =
                serde_json::to_value(craft_core::registry::schema_for::<fields::FieldOrder>()?)?;
            let expected = params
                .iter()
                .find(|p| p["name"] == "orderBy")
                .ok_or("Missing order")?["schema"]["enum"]
                .as_array()
                .ok_or("Missing enum")?;
            let actual = order["enum"].as_array().ok_or("Missing enum")?;
            assert_eq!(
                actual
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<std::collections::BTreeSet<_>>(),
                expected
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<std::collections::BTreeSet<_>>()
            );
        }
    }
    Ok(())
}
#[test]
fn invalid_field_search_inputs_and_targets_fail_during_preparation() -> TestResult {
    let registry = registry()?;
    for input in [
        json!({"ids":["summary"]}),
        json!({"ids":["customfield_01"]}),
        json!({"ids":["customfield_1","customfield_1"]}),
        json!({"ids":["customfield_9223372036854775808"]}),
        json!({"ids":vec!["customfield_1";101]}),
        json!({"types":["custom","custom"]}),
        json!({"types":["unknown"]}),
        json!({"project_ids":[1,1]}),
        json!({"project_ids":[0]}),
        json!({"project_ids":[1.0]}),
        json!({"project_ids":[9223372036854775808_u64]}),
        json!({"query":""}),
        json!({"query":"x\n"}),
        json!({"query":"x".repeat(257)}),
        json!({"page_size":0}),
        json!({"page_size":101}),
        json!({"order_by":"key"}),
        json!({"expand":"all"}),
        json!({"startAt":1}),
    ] {
        assert!(registry.prepare(request(SEARCH, input, &[])).is_err());
    }
    assert!(
        registry
            .prepare(request(LIST, json!({"query":"a"}), &[]))
            .is_err()
    );
    for key in ["id", "tenant", "resource_type", "preconditions"] {
        let mut request = request(SEARCH, json!({}), &[]);
        match key {
            "id" => request.target.id = "other".into(),
            "tenant" => request.target.tenant = "../other".into(),
            "resource_type" => request.target.resource_type = "project".into(),
            _ => request.preconditions = Some(serde_json::from_value(json!({}))?),
        }
        assert!(registry.prepare(request).is_err());
    }
    for (command, selected) in [
        (LIST, "description"),
        (SEARCH, "custom"),
        (SEARCH, "schema.configuration.$ref"),
    ] {
        assert!(FieldPlan::compile(command, &[selected.into()]).is_err());
    }
    Ok(())
}
#[test]
fn field_list_preserves_missing_null_and_inert_selected_metadata() -> TestResult {
    let op = operation(
        LIST,
        json!({}),
        &["name", "key", "schema", "scope.project.id", "clause_names"],
    )?;
    let mut raw = field("customfield_1");
    raw["name"] = Value::Null;
    raw.as_object_mut().ok_or("Object")?.remove("key");
    let mut ctx = context(json!([raw]));
    let page = fields::read_page(&op, &mut ctx, &plan(&op)?, None)?;
    assert!(page.next.is_none());
    assert_eq!(ctx.calls.len(), 1);
    assert!(ctx.calls[0].query.is_empty());
    assert!(ctx.calls[0].repeated_query.is_empty());
    let item = &page.items[0];
    assert!(item["name"].is_null());
    assert!(item.get("key").is_none());
    assert_eq!(item["fields_not_returned"], json!(["key"]));
    assert_eq!(
        item["schema"]["configuration"]["$ref"],
        "https://untrusted.invalid/schema"
    );
    assert_eq!(item["scope"]["project"], json!({"id":"10000"}));
    assert!(item.get("description").is_none());
    assert!(
        fields::read_page(
            &op,
            &mut ctx,
            &plan(&op)?,
            Some(&NativePosition(BTreeMap::from([(
                "startAt".into(),
                "1".into()
            )])))
        )
        .is_err()
    );
    assert_eq!(ctx.calls.len(), 1);
    Ok(())
}
#[test]
fn field_search_uses_repeated_filters_derived_expansions_and_advertised_clamped_offsets()
-> TestResult {
    let op = operation(
        SEARCH,
        json!({"ids":["customfield_1","customfield_2"],"types":["custom","system"],"project_ids":[10000,10001],"query":"literal, + & 文字","order_by":"+name","page_size":50}),
        &["key", "stable_id", "last_used.type", "projects_count"],
    )?;
    let mut body = page(json!([field("customfield_1")]));
    body["isLast"] = json!(false);
    body["maxResults"] = json!(2);
    body["nextPage"] =
        json!("https://craft-test.atlassian.net/rest/api/3/field/search?startAt=2&maxResults=2");
    body["total"] = json!(0);
    let mut ctx = context(body);
    let page = fields::read_page(&op, &mut ctx, &plan(&op)?, None)?;
    assert_eq!(page.next.as_ref().ok_or("Missing next")?.0["startAt"], "2");
    let call = &ctx.calls[0];
    assert_eq!(call.query["maxResults"], "50");
    assert_eq!(call.query["orderBy"], "+name");
    assert_eq!(call.query["query"], "literal, + & 文字");
    assert_eq!(call.query["expand"], "key,stableId,lastUsed");
    assert_eq!(
        call.repeated_query["id"],
        ["customfield_1", "customfield_2"]
    );
    assert_eq!(call.repeated_query["type"], ["custom", "system"]);
    assert_eq!(call.repeated_query["projectIds"], ["10000", "10001"]);
    ctx.body = json!({"startAt":2,"maxResults":2,"isLast":true,"values":[]});
    let next = fields::read_page(&op, &mut ctx, &plan(&op)?, page.next.as_ref())?;
    assert!(next.next.is_none());
    assert_eq!(ctx.calls[1].query["maxResults"], "50");
    assert_eq!(ctx.calls[1].query["startAt"], "2");
    assert_eq!(ctx.calls[1].repeated_query, ctx.calls[0].repeated_query);
    Ok(())
}
#[test]
fn ambiguous_or_malformed_field_pages_never_guess_completeness() -> TestResult {
    let op = operation(SEARCH, json!({}), &[])?;
    for mode in [
        "missing_last",
        "missing_start",
        "missing_max",
        "null_last",
        "false_without_link",
        "true_with_link",
        "bad_total",
        "bad_values",
        "excess",
        "wrong_start",
        "missing_required_name",
        "missing_required_type",
        "null_required_schema",
        "duplicate_ids",
    ] {
        let mut body = page(json!([field("customfield_1")]));
        match mode {
            "missing_last" => {
                body.as_object_mut().ok_or("Object")?.remove("isLast");
            }
            "missing_start" => {
                body.as_object_mut().ok_or("Object")?.remove("startAt");
            }
            "missing_max" => {
                body.as_object_mut().ok_or("Object")?.remove("maxResults");
            }
            "null_last" => body["isLast"] = Value::Null,
            "false_without_link" => body["isLast"] = json!(false),
            "true_with_link" => body["nextPage"] = json!("?startAt=2"),
            "bad_total" => body["total"] = json!(-1),
            "bad_values" => body["values"] = json!({}),
            "excess" => body["maxResults"] = json!(0),
            "wrong_start" => body["startAt"] = json!(1),
            "missing_required_name" => {
                body["values"][0]
                    .as_object_mut()
                    .ok_or("Object")?
                    .remove("name");
            }
            "missing_required_type" => {
                body["values"][0]["schema"]
                    .as_object_mut()
                    .ok_or("Object")?
                    .remove("type");
            }
            "null_required_schema" => body["values"][0]["schema"] = Value::Null,
            _ => body["values"] = json!([field("customfield_1"), field("customfield_1")]),
        }
        let mut ctx = context(body);
        assert!(
            fields::read_page(&op, &mut ctx, &plan(&op)?, None).is_err(),
            "{mode}"
        );
        assert_eq!(ctx.calls.len(), 1);
    }
    Ok(())
}
#[test]
fn field_continuation_defends_against_changed_normalized_calls_and_positions() -> TestResult {
    let op = operation(SEARCH, json!({"ids":["customfield_1"]}), &[])?;
    let mut ctx = context(
        json!({"startAt":0,"maxResults":50,"isLast":false,"nextPage":"?startAt=2","values":[]}),
    );
    fields::read_page(&op, &mut ctx, &plan(&op)?, None)?;
    for mode in [
        "route",
        "path",
        "body",
        "arrays",
        "filter",
        "offset",
        "canonical",
    ] {
        let mut normalized = ctx.calls[0].clone();
        normalized.query.insert("startAt".into(), "2".into());
        match mode {
            "route" => normalized.route = "other".into(),
            "path" => {
                normalized
                    .path_parameters
                    .insert("id".into(), "other".into());
            }
            "body" => normalized.body = Some(json!({})),
            "arrays" => {
                normalized.repeated_query.clear();
            }
            "filter" => {
                normalized.query.insert("orderBy".into(), "-name".into());
            }
            "offset" => {
                normalized.query.insert("startAt".into(), "0".into());
            }
            _ => {
                normalized.query.insert("startAt".into(), "02".into());
            }
        }
        ctx.normalized = Some(normalized);
        assert!(
            fields::read_page(&op, &mut ctx, &plan(&op)?, None).is_err(),
            "{mode}"
        );
    }
    let before = ctx.calls.len();
    for position in [
        BTreeMap::from([("other".into(), "1".into())]),
        BTreeMap::from([("startAt".into(), "01".into())]),
        BTreeMap::from([("startAt".into(), "1".into()), ("other".into(), "2".into())]),
    ] {
        assert!(
            fields::read_page(&op, &mut ctx, &plan(&op)?, Some(&NativePosition(position))).is_err()
        );
    }
    assert_eq!(ctx.calls.len(), before);
    Ok(())
}
#[test]
fn field_reads_preserve_denials_and_refuse_mismatched_plans_before_resource_calls() -> TestResult {
    let op = operation(SEARCH, json!({}), &[])?;
    for (status, code) in [
        (400, ErrorCode::InputInvalid),
        (401, ErrorCode::AuthRequired),
        (403, ErrorCode::PermissionDenied),
        (429, ErrorCode::RateLimited),
        (503, ErrorCode::RemoteUnavailable),
    ] {
        let mut ctx = context(json!({"message":"private-sentinel"}));
        ctx.status = status;
        let error = fields::read_page(&op, &mut ctx, &plan(&op)?, None)
            .err()
            .ok_or("Expected denial")?;
        assert_eq!(error.code, code);
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
        assert_eq!(ctx.calls.len(), 1);
    }
    let mut ctx = context(page(json!([])));
    ctx.deny = true;
    assert!(fields::read_page(&op, &mut ctx, &plan(&op)?, None).is_err());
    ctx.deny = false;
    assert!(
        fields::read_page(
            &op,
            &mut ctx,
            &FieldPlan::compile(SEARCH, &["description".into()])?,
            None
        )
        .is_err()
    );
    assert!(ctx.calls.is_empty());
    Ok(())
}
#[test]
fn field_list_limits_and_selected_type_errors_never_return_partial_records() -> TestResult {
    let filtered = operation(SEARCH, json!({"ids":["customfield_1"]}), &[])?;
    assert!(
        fields::read_page(
            &filtered,
            &mut context(page(json!([field("customfield_2")]))),
            &plan(&filtered)?,
            None
        )
        .is_err()
    );
    let op = operation(LIST, json!({}), &["schema.configuration"])?;
    for body in [
        json!([{"name":"Missing identity"}]),
        json!([field("same"), field("same")]),
        json!(vec![field("customfield_1"); 1001]),
        json!([{"id":"x","schema":{"configuration":{"large":"x".repeat(32_769)}}}]),
        json!([{"id":"x","schema":{"configuration":[]}}]),
    ] {
        let mut ctx = context(body);
        assert!(fields::read_page(&op, &mut ctx, &plan(&op)?, None).is_err());
    }
    let op = operation(LIST, json!({}), &["name"])?;
    let mut raw = field("customfield_1");
    raw["schema"] = json!(17);
    let mut ctx = context(json!([raw]));
    assert_eq!(
        fields::read_page(&op, &mut ctx, &plan(&op)?, None)?.items[0]["name"],
        "Field 文字"
    );
    Ok(())
}
