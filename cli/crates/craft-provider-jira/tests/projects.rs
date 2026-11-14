use std::collections::BTreeMap;

use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::registry::Registry;
use craft_provider_jira::projects::{self, COMMAND, JiraProjects};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;
fn request(input: Value) -> CanonicalRequest {
    CanonicalRequest {
        command: COMMAND.into(),
        target: ResourceRef {
            provider: "jira".into(),
            deployment: "cloud".into(),
            tenant: "00000000-1111-2222-3333-444444444444".into(),
            resource_type: "site".into(),
            id: "00000000-1111-2222-3333-444444444444".into(),
        },
        input,
        selection: Selection::default(),
        preconditions: None,
    }
}
fn operation() -> Result<PreparedOperation, CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&JiraProjects)?;
    registry.prepare(request(json!({"query":"test & 文字","page_size":50})))
}
fn body() -> Value {
    json!({"startAt":0,"maxResults":50,"isLast":true,"values":[{"id":"10000","key":"ED","name":"Example","projectTypeKey":"software","avatarUrls":{"large":"https://untrusted.invalid"},"extension":"ignored"}]})
}
struct Context {
    body: Value,
    status: u16,
    calls: Vec<NativeCall>,
    next: String,
    change_filter: bool,
}
fn context(body: Value) -> Context {
    Context {
        body,
        status: 200,
        calls: Vec::new(),
        next: "50".into(),
        change_filter: false,
    }
}
impl OperationContext for Context {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        Ok(NativeResponse {
            body: self.body.clone(),
            status: self.status,
            request_id: None,
            retry_after: None,
            link_headers: None,
        })
    }
    fn normalize_next(&self, call: &NativeCall, _: &str) -> Result<NativeCall, CraftError> {
        let mut next = call.clone();
        next.query.insert("startAt".into(), self.next.clone());
        if self.change_filter {
            next.query.insert("query".into(), "other".into());
        }
        Ok(next)
    }
}

#[test]
fn project_page_normalizes_only_selected_metadata_and_declares_native_read_contract() -> TestResult
{
    let operation = operation()?;
    let mut context = context(body());
    let page = projects::read_page(&operation, &mut context, None)?;
    assert_eq!(
        page.items,
        vec![json!({"id":"10000","key":"ED","name":"Example","project_type":"software"})]
    );
    assert!(page.next.is_none());
    assert_eq!(context.calls.len(), 1);
    assert_eq!(
        context.calls[0].query,
        BTreeMap::from([
            ("startAt".into(), "0".into()),
            ("maxResults".into(), "50".into()),
            ("orderBy".into(), "key".into()),
            ("action".into(), "view".into()),
            ("query".into(), "test & 文字".into())
        ])
    );
    let mut registry = Registry::default();
    registry.register_provider(&JiraProjects)?;
    let descriptor = registry.descriptor(COMMAND)?;
    assert_eq!(descriptor.scope_alternatives, vec![vec!["read:jira-work"]]);
    assert_eq!(
        descriptor.routes[0].continuation_parameters,
        vec!["startAt"]
    );
    assert!(registry.execute(&operation, &mut context).is_err());
    assert_eq!(context.calls.len(), 1);
    Ok(())
}

#[test]
fn empty_short_pages_and_changing_totals_use_explicit_next_offset() -> TestResult {
    for values in [json!([]), body()["values"].clone()] {
        let mut value = body();
        value["values"] = values;
        value["isLast"] = json!(false);
        value["total"] = json!(0);
        value["nextPage"] =
            json!("https://site.atlassian.net/rest/api/3/project/search?startAt=50");
        let mut context = context(value);
        let page = projects::read_page(&operation()?, &mut context, None)?;
        assert_eq!(page.next.ok_or("Missing continuation")?.0["startAt"], "50");
        context.body = json!({"startAt":50,"maxResults":50,"isLast":true,"values":[],"total":900});
        let terminal = projects::read_page(
            &operation()?,
            &mut context,
            Some(&craft_core::pagination::NativePosition(BTreeMap::from([(
                "startAt".into(),
                "50".into(),
            )]))),
        )?;
        assert!(terminal.next.is_none());
        assert_eq!(context.calls.len(), 2);
    }
    Ok(())
}

#[test]
fn malformed_page_metadata_and_duplicate_or_invalid_projects_fail_without_leaking_body()
-> TestResult {
    for field in [
        "offset",
        "maximum",
        "count",
        "total",
        "terminal",
        "contradiction",
        "ambiguous",
        "duplicate_id",
        "duplicate_key",
        "id_type",
        "name",
        "project_type",
    ] {
        let mut value = body();
        match field {
            "offset" => value["startAt"] = json!(1),
            "maximum" => value["maxResults"] = json!(0),
            "count" => {
                value["maxResults"] = json!(1);
                value["values"] = json!([{"id":"1"},{"id":"2"}]);
            }
            "total" => value["total"] = json!("private-sentinel"),
            "terminal" => value["isLast"] = Value::Null,
            "contradiction" => value["nextPage"] = json!("private-sentinel"),
            "ambiguous" => {
                value
                    .as_object_mut()
                    .ok_or("Expected object")?
                    .remove("isLast");
            }
            "duplicate_id" | "duplicate_key" => {
                let mut other = value["values"][0].clone();
                if field == "duplicate_id" {
                    other["key"] = json!("OTHER");
                } else {
                    other["id"] = json!("10001");
                }
                value["values"]
                    .as_array_mut()
                    .ok_or("Expected array")?
                    .push(other);
            }
            "id_type" => value["values"][0]["id"] = json!(1),
            "name" => value["values"][0]["name"] = json!("private-sentinel\n"),
            _ => value["values"][0]["projectTypeKey"] = Value::Null,
        }
        let error = projects::read_page(&operation()?, &mut context(value), None)
            .err()
            .ok_or("Expected qualification failure")?;
        assert_eq!(error.code, ErrorCode::RemoteUnavailable, "{field}");
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    Ok(())
}

#[test]
fn forged_continuation_or_http_denial_never_becomes_project_data() -> TestResult {
    for mode in ["repeat", "filter", "401", "403", "429", "500"] {
        let mut value = body();
        value["isLast"] = json!(false);
        value["nextPage"] = json!("next");
        let mut context = context(value);
        let expected = match mode {
            "repeat" => {
                context.next = "0".into();
                ErrorCode::PaginationStalled
            }
            "filter" => {
                context.change_filter = true;
                ErrorCode::PaginationStalled
            }
            "401" => {
                context.status = 401;
                ErrorCode::AuthRequired
            }
            "403" => {
                context.status = 403;
                ErrorCode::PermissionDenied
            }
            "429" => {
                context.status = 429;
                ErrorCode::RateLimited
            }
            _ => {
                context.status = 500;
                ErrorCode::RemoteUnavailable
            }
        };
        assert_eq!(
            projects::read_page(&operation()?, &mut context, None)
                .err()
                .ok_or("Expected denial")?
                .code,
            expected
        );
        assert_eq!(context.calls.len(), 1);
    }
    Ok(())
}

#[test]
fn unknown_input_invalid_query_page_size_and_target_are_refused_in_pure_preparation() -> TestResult
{
    let mut registry = Registry::default();
    registry.register_provider(&JiraProjects)?;
    for input in [
        json!({"extra":true}),
        json!({"page_size":0}),
        json!({"page_size":101}),
        json!({"query":""}),
        json!({"query":"x\n"}),
    ] {
        assert!(registry.prepare(request(input)).is_err());
    }
    let mut wrong = request(json!({}));
    wrong.target.resource_type = "project".into();
    assert!(registry.prepare(wrong).is_err());
    Ok(())
}
