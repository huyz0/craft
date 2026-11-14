use std::collections::BTreeMap;

use craft_core::contracts::{CanonicalRequest, EffectClass, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{
    HttpMethod, NativeCall, NativeResponse, OperationContext, PreparedOperation,
};
use craft_core::pagination::NativePosition;
use craft_core::registry::Registry;
use craft_provider_jira::issue_reads::{
    self, FieldPlan, GET, IdentityResolution, JiraIssues, SEARCH,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn compiled_issue_routes_and_scopes_match_the_pinned_current_native_contract() -> TestResult {
    let source: Value = serde_json::from_slice(&std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../openspec/changes/build-cloud-cli/sources/jira-v3.json"
    ))?)?;
    for (command, path, method, response) in [
        (GET, "/rest/api/3/issue/{issueIdOrKey}", "get", "IssueBean"),
        (
            SEARCH,
            "/rest/api/3/search/jql",
            "post",
            "SearchAndReconcileResults",
        ),
    ] {
        let registry = registry()?;
        let descriptor = registry.descriptor(command)?;
        let route = &descriptor.routes[0];
        let actual_path = format!(
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
        assert_eq!(actual_path, path);
        assert_eq!(serde_json::to_value(route.method)?, json!(method));
        let native = &source["paths"][path][method];
        assert!(!native.is_null());
        let scopes = native["x-atlassian-oauth2-scopes"]
            .as_array()
            .ok_or("Missing scopes")?;
        for alternative in &descriptor.scope_alternatives {
            assert!(
                scopes
                    .iter()
                    .any(|scope| scope["state"] == "Current"
                        && scope["scopes"] == json!(alternative))
            );
        }
        let parameters = native["parameters"]
            .as_array()
            .ok_or("Missing parameters")?;
        for name in &route.query_parameters {
            assert!(
                parameters
                    .iter()
                    .any(|parameter| parameter["in"] == "query" && parameter["name"] == *name)
            );
        }
        assert_eq!(
            native["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            format!("#/components/schemas/{response}")
        );
    }
    let properties =
        &source["components"]["schemas"]["SearchAndReconcileRequestBean"]["properties"];
    for field in [
        "jql",
        "maxResults",
        "fields",
        "fieldsByKeys",
        "includeArchivedProjects",
        "reconcileIssues",
        "nextPageToken",
    ] {
        assert!(!properties[field].is_null(), "{field}");
    }
    Ok(())
}

#[test]
fn mock_response_structure_and_bytes_obey_the_real_transport_bounds() -> TestResult {
    let op = operation(GET, json!({"issue":"10000"}), &["id"])?;
    let mut nested = json!(null);
    for _ in 0..70 {
        nested = json!([nested]);
    }
    for extra in [nested, json!("x".repeat(1_048_576)), json!(vec![0; 10_001])] {
        let mut raw = issue();
        raw["unselected"] = extra;
        assert!(issue_reads::read_get(&op, &mut context(raw), &plan(&op)?).is_err());
    }
    Ok(())
}

fn registry() -> Result<Registry, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraIssues)?;
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
fn plan(operation: &PreparedOperation) -> Result<FieldPlan, CraftError> {
    FieldPlan::compile(
        &operation.request().command,
        &operation.request().selection.fields,
    )
}
fn issue() -> Value {
    json!({"id":"10000","key":"ED-1","self":"https://untrusted.invalid/issue",
        "fields":{"summary":"Résumé 文字","project":{"id":"10","key":"ED","self":"https://untrusted.invalid"},
        "status":{"id":"1","name":"Open","iconUrl":"https://untrusted.invalid"},
        "assignee":{"accountId":"private-account","displayName":"Private user","emailAddress":"private@example.invalid"},
        "description":{"type":"doc","version":1,"content":[]},"customfield_123":"private-sentinel"}})
}
struct Context {
    body: Value,
    status: u16,
    calls: Vec<NativeCall>,
    refuse_binding: bool,
}
fn context(body: Value) -> Context {
    Context {
        body,
        status: 200,
        calls: vec![],
        refuse_binding: false,
    }
}
impl OperationContext for Context {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, _: &PreparedOperation) -> Result<(), CraftError> {
        if self.refuse_binding {
            Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Wrong context binding.",
            ))
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
            "Returned URLs cannot authorize continuation.",
        ))
    }
}
fn search() -> Value {
    json!({"jql":"project = ED\nORDER BY key","page_size":50,"reconcile_issue_ids":["10000","9223372036854775807"]})
}

#[test]
fn issue_registry_constructs_typed_read_schemas_and_opaque_description_projection() -> TestResult {
    let registry = registry()?;
    for (command, method) in [(GET, HttpMethod::Get), (SEARCH, HttpMethod::Post)] {
        let descriptor = registry.descriptor(command)?;
        assert!(matches!(descriptor.effect, EffectClass::Read));
        assert_eq!(
            serde_json::to_value(descriptor.routes[0].method)?,
            serde_json::to_value(method)?
        );
        assert_eq!(descriptor.scope_alternatives, vec![vec!["read:jira-work"]]);
        let projection = descriptor.projection.as_ref().ok_or("Missing projection")?;
        let selectable = projection.selectable_fields(&descriptor.response_schema)?;
        assert!(selectable.contains(&"description".into()));
        assert!(!selectable.contains(&"description.content".into()));
        assert!(
            registry
                .projection(command, &["description.content".into()])
                .is_err()
        );
        for field in ["id", "status.name", "assignee.account_id", "description"] {
            assert!(registry.projection(command, &[field.into()])?.is_some());
        }
    }
    assert!(FieldPlan::compile("jira.issue.other", &[]).is_err());
    Ok(())
}

#[test]
fn default_get_mask_and_normalization_exclude_bodies_users_and_links() -> TestResult {
    let op = operation(GET, json!({"issue":"10000"}), &[])?;
    let mut context = context(issue());
    let (normalized, resolution) = issue_reads::read_get(&op, &mut context, &plan(&op)?)?;
    assert_eq!(resolution, IdentityResolution::ExactId);
    assert_eq!(
        normalized,
        json!({"id":"10000","key":"ED-1","summary":"Résumé 文字","project":{"id":"10","key":"ED"},"status":{"id":"1","name":"Open"}})
    );
    assert_eq!(context.calls.len(), 1);
    assert_eq!(
        context.calls[0].query,
        BTreeMap::from([
            ("fields".into(), "id,key,project,status,summary".into()),
            ("fieldsByKeys".into(), "false".into()),
            ("updateHistory".into(), "false".into()),
            ("failFast".into(), "true".into())
        ])
    );
    assert!(context.calls[0].body.is_none());
    assert_eq!(context.calls[0].path_parameters["issueIdOrKey"], "10000");
    assert!(registry()?.execute(&op, &mut context).is_err());
    assert_eq!(context.calls.len(), 1);
    Ok(())
}

#[test]
fn field_plan_mismatch_and_unbound_context_refuse_before_native_call() -> TestResult {
    let op = operation(GET, json!({"issue":"10000"}), &["summary"])?;
    let mut context = context(issue());
    for other in [
        FieldPlan::compile(GET, &["description".into()])?,
        FieldPlan::compile(SEARCH, &["summary".into()])?,
    ] {
        assert!(issue_reads::read_get(&op, &mut context, &other).is_err());
        assert!(context.calls.is_empty());
    }
    context.refuse_binding = true;
    assert!(issue_reads::read_get(&op, &mut context, &plan(&op)?).is_err());
    assert!(context.calls.is_empty());
    Ok(())
}

#[test]
fn missing_null_and_unselected_nested_values_remain_distinct() -> TestResult {
    let op = operation(
        GET,
        json!({"issue":"10000"}),
        &[
            "summary",
            "status.id",
            "project.key",
            "assignee.display_name",
        ],
    )?;
    let mut raw = issue();
    raw["fields"] =
        json!({"summary":null,"status":{"id":"1","name":17},"project":{},"assignee":null});
    let (normalized, _) = issue_reads::read_get(&op, &mut context(raw), &plan(&op)?)?;
    assert_eq!(
        normalized,
        json!({"id":"10000","summary":null,"status":{"id":"1"},"project":{},"assignee":null,"fields_not_returned":["project.key"]})
    );
    let projection = registry()?
        .projection(GET, &op.request().selection.fields)?
        .ok_or("Missing projection")?;
    let data = projection.data(&json!({"issue":normalized}))?;
    assert!(data["issue"]["summary"].is_null());
    assert_eq!(data["issue"]["fields_not_returned"], json!(["project.key"]));
    Ok(())
}

#[test]
fn identity_only_ignores_unselected_field_wrapper_but_selected_data_is_validated() -> TestResult {
    let op = operation(GET, json!({"issue":"10000"}), &["id"])?;
    let mut raw = issue();
    raw["fields"] = json!("private-sentinel");
    assert_eq!(
        issue_reads::read_get(&op, &mut context(raw.clone()), &plan(&op)?)?.0,
        json!({"id":"10000"})
    );
    let selected = operation(GET, json!({"issue":"10000"}), &["summary"])?;
    assert!(issue_reads::read_get(&selected, &mut context(raw), &plan(&selected)?).is_err());
    for (field, value) in [
        ("summary", json!(17)),
        ("labels", json!([17])),
        ("status", json!({"name":17})),
        (
            "description",
            json!({"type":"doc","version":2,"content":[]}),
        ),
    ] {
        let selected = operation(GET, json!({"issue":"10000"}), &[field])?;
        let mut raw = issue();
        raw["fields"][field] = value;
        assert!(
            issue_reads::read_get(&selected, &mut context(raw), &plan(&selected)?).is_err(),
            "{field}"
        );
    }
    Ok(())
}

#[test]
fn description_is_explicit_inert_bounded_adf_or_legacy_plaintext() -> TestResult {
    let op = operation(GET, json!({"issue":"10000"}), &["description"])?;
    let adf = json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"文字 résumé","marks":[{"type":"link","attrs":{"href":"https://untrusted.invalid"}}]}]}],"extension":{"inert":true}});
    for description in [adf, json!("Legacy 文字"), Value::Null] {
        let mut raw = issue();
        raw["fields"]["description"] = description.clone();
        let (item, _) = issue_reads::read_get(&op, &mut context(raw), &plan(&op)?)?;
        assert_eq!(item["description"], description);
    }
    let mut raw = issue();
    raw["fields"]["description"] = json!("x".repeat(262_145));
    assert!(issue_reads::read_get(&op, &mut context(raw), &plan(&op)?).is_err());
    Ok(())
}

#[test]
fn get_identity_resolution_retains_numeric_exactness_and_key_aliases() -> TestResult {
    for (requested, expected) in [
        ("10000", IdentityResolution::ExactId),
        ("ed-1", IdentityResolution::MatchingKey),
        ("OLD-7", IdentityResolution::ResolvedKey),
    ] {
        let op = operation(GET, json!({"issue":requested}), &["id"])?;
        let (item, resolution) = issue_reads::read_get(&op, &mut context(issue()), &plan(&op)?)?;
        assert_eq!(item["id"], "10000");
        assert_eq!(resolution, expected);
    }
    let op = operation(GET, json!({"issue":"10001"}), &[])?;
    assert!(issue_reads::read_get(&op, &mut context(issue()), &plan(&op)?).is_err());
    Ok(())
}

#[test]
fn local_input_and_get_cursor_fail_in_preparation() -> TestResult {
    for input in [
        json!({"issue":"ED-1/extra"}),
        json!({"issue":" ED-1"}),
        json!({"issue":"ED-1","expand":"*all"}),
    ] {
        assert!(operation(GET, input, &[]).is_err());
    }
    for input in [
        json!({"jql":" "}),
        json!({"jql":"project=ED\u{0}"}),
        json!({"jql":"project=ED","page_size":101}),
        json!({"jql":"project=ED","reconcile_issue_ids":["01"]}),
        json!({"jql":"project=ED","reconcile_issue_ids":["1","1"]}),
        json!({"jql":"project=ED","reconcile_issue_ids":["9223372036854775808"]}),
        json!({"jql":"project=ED","fields":["*all"]}),
    ] {
        assert!(operation(SEARCH, input, &[]).is_err());
    }
    let mut request = request(GET, json!({"issue":"10000"}), &[]);
    request.selection.cursor = Some("opaque".into());
    assert!(registry()?.prepare(request).is_err());
    Ok(())
}

#[test]
fn enhanced_search_preserves_jql_reconciliation_and_opaque_token_without_url_following()
-> TestResult {
    let op = operation(SEARCH, search(), &["summary", "issue_type.name"])?;
    let position = NativePosition(BTreeMap::from([(
        "nextPageToken".into(),
        "https://untrusted.invalid/?opaque=1".into(),
    )]));
    let mut context =
        context(json!({"issues":[issue()],"isLast":false,"nextPageToken":"next 文字 /?="}));
    let page = issue_reads::read_search_page(&op, &mut context, &plan(&op)?, Some(&position))?;
    assert_eq!(context.calls.len(), 1);
    let sent = context.calls[0]
        .body
        .as_ref()
        .ok_or("Missing search body")?;
    assert_eq!(
        sent,
        &json!({"jql":search()["jql"],"maxResults":50,"fields":["id","issuetype","summary"],"fieldsByKeys":false,"includeArchivedProjects":false,"reconcileIssues":[10000,9223372036854775807_i64],"nextPageToken":position.0["nextPageToken"]})
    );
    assert!(context.calls[0].query.is_empty());
    assert_eq!(
        page.next.ok_or("Missing token")?.0["nextPageToken"],
        "next 文字 /?="
    );
    assert_eq!(
        page.items[0]["fields_not_returned"],
        json!(["issue_type.name"])
    );
    Ok(())
}

#[test]
fn empty_short_and_terminal_search_pages_follow_only_explicit_token() -> TestResult {
    let op = operation(SEARCH, search(), &["id"])?;
    for issues in [json!([]), json!([issue()])] {
        let page = issue_reads::read_search_page(
            &op,
            &mut context(json!({"issues":issues,"isLast":false,"nextPageToken":"next"})),
            &plan(&op)?,
            None,
        )?;
        assert!(page.next.is_some());
    }
    for body in [
        json!({"issues":[]}),
        json!({"issues":[],"nextPageToken":null}),
        json!({"issues":[],"isLast":true,"nextPageToken":null}),
    ] {
        assert!(
            issue_reads::read_search_page(&op, &mut context(body), &plan(&op)?, None)?
                .next
                .is_none()
        );
    }
    Ok(())
}

#[test]
fn ambiguous_tokens_warnings_duplicate_items_and_overfull_pages_release_no_page() -> TestResult {
    let op = operation(SEARCH, json!({"jql":"project=ED","page_size":1}), &["id"])?;
    for body in [
        json!({"issues":[],"isLast":false}),
        json!({"issues":[],"isLast":true,"nextPageToken":"next"}),
        json!({"issues":[],"nextPageToken":""}),
        json!({"issues":[],"nextPageToken":17}),
        json!({"issues":[],"isLast":null}),
        json!({"issues":[],"warnings":null}),
        json!({"issues":[],"warnings":[{"message":"private-sentinel"}]}),
        json!({"issues":[issue(),issue()]}),
        json!({"issues":{}}),
    ] {
        let error = issue_reads::read_search_page(&op, &mut context(body), &plan(&op)?, None)
            .err()
            .ok_or("Expected page refusal")?;
        assert_eq!(error.code, ErrorCode::RemoteUnavailable);
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    let larger = operation(SEARCH, search(), &["id"])?;
    assert!(
        issue_reads::read_search_page(
            &larger,
            &mut context(json!({"issues":[issue(),issue()]})),
            &plan(&larger)?,
            None
        )
        .is_err()
    );
    let mut context = context(json!({"issues":[],"warnings":[]}));
    assert!(
        issue_reads::read_search_page(
            &op,
            &mut context,
            &FieldPlan::compile(SEARCH, &["description".into()])?,
            None
        )
        .is_err()
    );
    assert!(context.calls.is_empty());
    assert!(
        issue_reads::read_search_page(
            &op,
            &mut context,
            &plan(&op)?,
            Some(&NativePosition(BTreeMap::from([(
                "wrong".into(),
                "token".into()
            )])))
        )
        .is_err()
    );
    assert!(context.calls.is_empty());
    Ok(())
}

#[test]
fn http_failures_are_single_attempt_sanitized_and_do_not_prove_absence() -> TestResult {
    let op = operation(GET, json!({"issue":"10000"}), &[])?;
    for (status, code) in [
        (400, ErrorCode::InputInvalid),
        (401, ErrorCode::AuthRequired),
        (403, ErrorCode::PermissionDenied),
        (404, ErrorCode::RemoteUnavailable),
        (429, ErrorCode::RateLimited),
        (500, ErrorCode::RemoteUnavailable),
    ] {
        let mut context = context(json!({"message":"private-sentinel"}));
        context.status = status;
        let error = issue_reads::read_get(&op, &mut context, &plan(&op)?)
            .err()
            .ok_or("Expected failure")?;
        assert_eq!(error.code, code);
        if status == 404 {
            assert!(error.message.contains("does not establish absence"));
        }
        assert_eq!(context.calls.len(), 1);
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    Ok(())
}
