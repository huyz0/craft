//! Provider-level mock fidelity: the shapes below follow Bitbucket Cloud's documented REST 2.0
//! responses. Live verification against the service is recorded separately.
use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{NativeCall, NativeResponse, OperationContext, PreparedOperation};
use craft_core::pagination::NativePosition;
use craft_core::registry::Registry;
use craft_provider_bitbucket::{BitbucketContext, commands, read_get, read_page};
use serde_json::{Value, json};
use std::collections::BTreeMap;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WORKSPACE: &str = "team";
const UUID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

struct Mock {
    status: u16,
    body: Value,
    calls: Vec<NativeCall>,
}
impl Mock {
    fn ok(body: Value) -> Self {
        Self {
            status: 200,
            body,
            calls: Vec::new(),
        }
    }
    fn status(status: u16) -> Self {
        Self {
            status,
            body: json!({"error": {"message": "native detail must not leak"}}),
            calls: Vec::new(),
        }
    }
}
impl OperationContext for Mock {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, _: &PreparedOperation) -> Result<(), CraftError> {
        Ok(())
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
    /// Mirrors the transport rule that matters here: only declared filters survive and the
    /// continuation parameter is replaced by the link's value.
    fn normalize_next(&self, call: &NativeCall, link: &str) -> Result<NativeCall, CraftError> {
        let query = link
            .split_once('?')
            .ok_or_else(|| CraftError::new(ErrorCode::PolicyDenied, "bad link"))?
            .1;
        let mut next = call.clone();
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            if name == "page" {
                next.query.insert("page".into(), value.into());
            }
        }
        Ok(next)
    }
}

fn target() -> ResourceRef {
    ResourceRef {
        provider: "bitbucket".into(),
        deployment: "cloud".into(),
        tenant: WORKSPACE.into(),
        resource_type: "workspace".into(),
        id: WORKSPACE.into(),
    }
}

fn prepare(command: &str, input: Value) -> Result<(Registry, PreparedOperation), CraftError> {
    let mut registry = Registry::default();
    registry.register_provider(&BitbucketContext)?;
    let operation = registry.prepare(CanonicalRequest {
        command: command.into(),
        target: target(),
        input,
        selection: Selection::default(),
        preconditions: None,
    })?;
    Ok((registry, operation))
}

fn position(token: &str) -> NativePosition {
    NativePosition(BTreeMap::from([("page".into(), token.into())]))
}

fn pull_request(id: u64) -> Value {
    json!({
        "type": "pullrequest", "id": id, "title": "Fix login", "description": "details",
        "state": "OPEN", "author": {"display_name": "Agent"},
        "source": {"branch": {"name": "feature/login"}, "repository": {"full_name": "team/service"}},
        "destination": {"branch": {"name": "main"}},
        "created_on": "2026-11-18T00:00:00.000000+00:00",
        "updated_on": "2026-11-18T01:00:00.000000+00:00",
        "comment_count": 2, "task_count": 1, "close_source_branch": true,
        "merge_commit": null,
        "reviewers": [{"display_name": "Reviewer"}],
        "participants": [{"user": {"display_name": "Reviewer"}, "role": "REVIEWER", "approved": true, "state": "approved"}],
        "links": {"html": {"href": "https://bitbucket.org/team/service/pull-requests/1"}}
    })
}

fn page(values: Vec<Value>, next: Option<&str>) -> Value {
    let mut body = json!({"pagelen": 2, "page": 1, "values": values});
    if let Some(next) = next {
        body["next"] = json!(next);
    }
    body
}

#[test]
fn every_declared_command_registers_with_a_read_descriptor() -> TestResult {
    let mut registry = Registry::default();
    registry.register_provider(&BitbucketContext)?;
    assert_eq!(commands().count(), 11);
    for command in commands() {
        assert!(registry.descriptor(command).is_ok(), "{command}");
    }
    Ok(())
}

#[test]
fn repository_get_uses_the_workspace_from_the_target_and_the_repo_from_input() -> TestResult {
    let (_, operation) = prepare("bitbucket.repository.get", json!({"repo": "service"}))?;
    let mut context = Mock::ok(json!({
        "type": "repository", "slug": "service", "uuid": "{0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0}",
        "name": "Service", "full_name": "team/service", "is_private": true, "language": "rust",
        "mainbranch": {"name": "main"}, "project": {"key": "PLAT"},
        "links": {"html": {"href": "https://bitbucket.org/team/service"}}
    }));
    let item = read_get(&operation, &mut context)?;
    assert_eq!(item["id"], "service");
    assert_eq!(item["uuid"], UUID);
    assert_eq!(item["main_branch"], "main");
    assert_eq!(item["project_key"], "PLAT");
    assert_eq!(context.calls.len(), 1);
    assert_eq!(context.calls[0].route, "bitbucket-repository-get");
    assert_eq!(context.calls[0].path_parameters["workspace"], WORKSPACE);
    assert_eq!(context.calls[0].path_parameters["repo"], "service");
    Ok(())
}

#[test]
fn a_response_for_a_different_resource_is_malformed_not_trusted() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.pullrequest.get",
        json!({"repo": "service", "pull_request_id": 7}),
    )?;
    let mut context = Mock::ok(pull_request(8));
    let error = read_get(&operation, &mut context)
        .err()
        .ok_or("expected a mismatch")?;
    assert_eq!(error.code, ErrorCode::RemoteUnavailable);
    Ok(())
}

#[test]
fn pull_request_get_maps_branches_reviewers_and_participants() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.pullrequest.get",
        json!({"repo": "service", "pull_request_id": 1}),
    )?;
    let mut context = Mock::ok(pull_request(1));
    let item = read_get(&operation, &mut context)?;
    assert_eq!(item["id"], 1);
    assert_eq!(item["source_branch"], "feature/login");
    assert_eq!(item["destination_branch"], "main");
    assert_eq!(item["source_repository"], "team/service");
    assert_eq!(item["reviewers"], json!(["Reviewer"]));
    assert_eq!(item["participants"][0]["approved"], true);
    assert_eq!(item["close_source_branch"], true);
    assert_eq!(item["merge_commit"], Value::Null);
    assert_eq!(context.calls[0].path_parameters["pull_request_id"], "1");
    Ok(())
}

#[test]
fn pipeline_get_sends_the_braced_uuid_and_returns_the_bare_one() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.pipeline.get",
        json!({"repo": "service", "pipeline_uuid": format!("{{{}}}", UUID.to_uppercase())}),
    )?;
    let mut context = Mock::ok(json!({
        "uuid": format!("{{{UUID}}}"), "build_number": 42,
        "creator": {"display_name": "Agent"},
        "target": {"ref_type": "branch", "type": "pipeline_ref_target", "ref_name": "main", "commit": {"hash": "0123456789abcdef"}},
        "state": {"name": "COMPLETED", "result": {"name": "FAILED"}},
        "trigger": {"name": "PUSH"},
        "created_on": "2026-11-18T00:00:00Z", "completed_on": "2026-11-18T00:01:00Z",
        "duration_in_seconds": 60
    }));
    let item = read_get(&operation, &mut context)?;
    assert_eq!(item["id"], UUID);
    assert_eq!(item["result"], "FAILED");
    assert_eq!(item["ref_name"], "main");
    assert_eq!(item["duration_seconds"], 60);
    assert_eq!(
        context.calls[0].path_parameters["pipeline_uuid"],
        format!("{{{UUID}}}")
    );
    Ok(())
}

#[test]
fn a_running_pipeline_reports_its_stage_as_the_result() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.step.list",
        json!({"repo": "service", "pipeline_uuid": UUID}),
    )?;
    let mut context = Mock::ok(page(
        vec![json!({
            "uuid": "{aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}", "name": "Test",
            "state": {"name": "IN_PROGRESS", "stage": {"name": "RUNNING"}},
            "started_on": "2026-11-18T00:00:00Z"
        })],
        None,
    ));
    let page = read_page(&operation, &mut context, None)?;
    assert_eq!(page.items[0]["state"], "IN_PROGRESS");
    assert_eq!(page.items[0]["result"], "RUNNING");
    assert_eq!(page.items[0]["completed_on"], Value::Null);
    Ok(())
}

#[test]
fn pull_request_list_defaults_to_open_and_sends_page_length() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.pullrequest.list",
        json!({"repo": "service", "page_size": 2}),
    )?;
    let mut context = Mock::ok(page(vec![pull_request(1), pull_request(2)], None));
    let result = read_page(&operation, &mut context, None)?;
    assert_eq!(result.items.len(), 2);
    assert!(result.next.is_none());
    let query = &context.calls[0].query;
    assert_eq!(query["state"], "OPEN");
    assert_eq!(query["pagelen"], "2");
    assert!(!query.contains_key("page"));
    Ok(())
}

#[test]
fn paging_follows_the_next_link_page_token_and_resumes_with_it() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.pullrequest.list",
        json!({"repo": "service", "state": "MERGED", "page_size": 2}),
    )?;
    let link = "https://api.bitbucket.org/2.0/repositories/team/service/pullrequests?state=MERGED&pagelen=2&page=2";
    let mut context = Mock::ok(page(vec![pull_request(1), pull_request(2)], Some(link)));
    let first = read_page(&operation, &mut context, None)?;
    let next = first.next.ok_or("expected a next position")?;
    assert_eq!(next.0["page"], "2");
    assert_eq!(context.calls[0].query["state"], "MERGED");
    let mut context = Mock::ok(page(vec![pull_request(3)], None));
    let second = read_page(&operation, &mut context, Some(&next))?;
    assert!(second.next.is_none());
    assert_eq!(context.calls[0].query["page"], "2");
    Ok(())
}

#[test]
fn a_next_link_that_repeats_the_current_page_is_a_loop_not_progress() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.pipeline.list",
        json!({"repo": "service", "page_size": 2}),
    )?;
    let link = "https://api.bitbucket.org/2.0/repositories/team/service/pipelines/?page=2";
    let mut context = Mock::ok(page(vec![], Some(link)));
    let error = read_page(&operation, &mut context, Some(&position("2")))
        .err()
        .ok_or("expected a loop refusal")?;
    assert_eq!(error.code, ErrorCode::RemoteUnavailable);
    Ok(())
}

#[test]
fn a_page_larger_than_requested_or_with_duplicate_items_is_malformed() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.pullrequest.list",
        json!({"repo": "service", "page_size": 1}),
    )?;
    let mut context = Mock::ok(page(vec![pull_request(1), pull_request(2)], None));
    assert!(read_page(&operation, &mut context, None).is_err());
    let (_, operation) = prepare(
        "bitbucket.pullrequest.list",
        json!({"repo": "service", "page_size": 2}),
    )?;
    let mut context = Mock::ok(page(vec![pull_request(1), pull_request(1)], None));
    assert!(read_page(&operation, &mut context, None).is_err());
    let mut context = Mock::ok(json!({"pagelen": 2}));
    assert!(read_page(&operation, &mut context, None).is_err());
    Ok(())
}

#[test]
fn branch_name_filter_becomes_a_fixed_query_and_cannot_inject_query_syntax() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.branch.list",
        json!({"repo": "service", "name_contains": "feature/log-in"}),
    )?;
    let mut context = Mock::ok(page(
        vec![json!({
            "type": "branch", "name": "feature/log-in",
            "target": {"hash": "0123456789abcdef", "date": "2026-11-18T00:00:00Z",
                       "message": "wip", "author": {"raw": "Agent <a@example.invalid>", "user": {"display_name": "Agent"}}}
        })],
        None,
    ));
    let result = read_page(&operation, &mut context, None)?;
    assert_eq!(result.items[0]["id"], "feature/log-in");
    assert_eq!(result.items[0]["author"], "Agent");
    assert_eq!(context.calls[0].query["q"], "name ~ \"feature/log-in\"");
    for bad in ["x\" OR name ~ \"y", "a b", "x\\", ""] {
        assert!(
            prepare(
                "bitbucket.branch.list",
                json!({"repo": "service", "name_contains": bad})
            )
            .is_err(),
            "{bad}"
        );
    }
    Ok(())
}

#[test]
fn commit_list_maps_parents_and_filters_by_branch() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.commit.list",
        json!({"repo": "service", "branch": "feature/login"}),
    )?;
    let mut context = Mock::ok(page(
        vec![json!({
            "hash": "0123456789abcdef0123456789abcdef01234567", "message": "Fix login\n",
            "date": "2026-11-18T00:00:00Z", "author": {"raw": "Agent <a@example.invalid>"},
            "parents": [{"hash": "89abcdef0123456789abcdef0123456789abcdef"}],
            "links": {"html": {"href": "https://bitbucket.org/team/service/commits/0123"}}
        })],
        None,
    ));
    let result = read_page(&operation, &mut context, None)?;
    assert_eq!(result.items[0]["parents"].as_array().map(Vec::len), Some(1));
    assert_eq!(context.calls[0].query["include"], "feature/login");
    Ok(())
}

#[test]
fn diffstat_identifies_removed_files_by_their_old_path() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.pullrequest.diffstat",
        json!({"repo": "service", "pull_request_id": 1}),
    )?;
    let mut context = Mock::ok(page(
        vec![
            json!({"status": "modified", "lines_added": 3, "lines_removed": 1,
                   "old": {"path": "src/a.rs"}, "new": {"path": "src/a.rs"}}),
            json!({"status": "removed", "lines_added": 0, "lines_removed": 9,
                   "old": {"path": "src/b.rs"}, "new": null}),
        ],
        None,
    ));
    let result = read_page(&operation, &mut context, None)?;
    assert_eq!(result.items[1]["id"], "src/b.rs");
    assert_eq!(result.items[1]["new_path"], Value::Null);
    assert_eq!(result.items[1]["lines_removed"], 9);
    Ok(())
}

#[test]
fn comments_keep_inline_position_and_parent() -> TestResult {
    let (_, operation) = prepare(
        "bitbucket.comment.list",
        json!({"repo": "service", "pull_request_id": 1}),
    )?;
    let mut context = Mock::ok(page(
        vec![
            json!({"id": 10, "content": {"raw": "Rename this"}, "user": {"display_name": "Reviewer"},
                   "created_on": "2026-11-18T00:00:00Z", "deleted": false,
                   "inline": {"path": "src/a.rs", "from": null, "to": 12}}),
            json!({"id": 11, "content": {"raw": "Done"}, "user": {"display_name": "Agent"},
                   "parent": {"id": 10}, "deleted": false}),
        ],
        None,
    ));
    let result = read_page(&operation, &mut context, None)?;
    assert_eq!(result.items[0]["inline"]["to_line"], 12);
    assert_eq!(result.items[0]["inline"]["from_line"], Value::Null);
    assert_eq!(result.items[1]["parent_id"], 10);
    assert_eq!(result.items[1]["inline"], Value::Null);
    Ok(())
}

#[test]
fn native_failures_map_to_codes_without_leaking_native_detail() -> TestResult {
    let expected = [
        (400, ErrorCode::InputInvalid),
        (401, ErrorCode::AuthRequired),
        (403, ErrorCode::PermissionDenied),
        (404, ErrorCode::RemoteUnavailable),
        (429, ErrorCode::RateLimited),
        (500, ErrorCode::RemoteUnavailable),
    ];
    for (status, code) in expected {
        let (_, operation) = prepare("bitbucket.repository.get", json!({"repo": "service"}))?;
        let error = read_get(&operation, &mut Mock::status(status))
            .err()
            .ok_or("expected a failure")?;
        assert_eq!(error.code, code, "{status}");
        assert!(!error.message.contains("native detail"));
    }
    Ok(())
}

#[test]
fn local_input_that_could_change_routing_is_refused_before_any_call() {
    let cases = [
        ("bitbucket.repository.get", json!({"repo": "../other"})),
        ("bitbucket.repository.get", json!({"repo": "Service"})),
        ("bitbucket.repository.get", json!({"repo": "a/b"})),
        (
            "bitbucket.repository.get",
            json!({"repo": "service", "workspace": "other"}),
        ),
        (
            "bitbucket.pullrequest.get",
            json!({"repo": "service", "pull_request_id": 0}),
        ),
        (
            "bitbucket.pipeline.get",
            json!({"repo": "service", "pipeline_uuid": "not-a-uuid"}),
        ),
        (
            "bitbucket.pullrequest.list",
            json!({"repo": "service", "state": "open"}),
        ),
        (
            "bitbucket.pullrequest.list",
            json!({"repo": "service", "page_size": 51}),
        ),
        ("bitbucket.repository.list", json!({"page_size": 0})),
    ];
    for (command, input) in cases {
        assert!(
            prepare(command, input.clone()).is_err(),
            "{command} {input}"
        );
    }
}

#[test]
fn a_target_for_another_provider_or_scope_is_refused() -> TestResult {
    let mut registry = Registry::default();
    registry.register_provider(&BitbucketContext)?;
    for mutate in [
        |t: &mut ResourceRef| t.provider = "trello".into(),
        |t: &mut ResourceRef| t.resource_type = "repository".into(),
        |t: &mut ResourceRef| t.id = "other".into(),
        |t: &mut ResourceRef| t.tenant = "Bad Slug".into(),
    ] {
        let mut bad = target();
        mutate(&mut bad);
        let result = registry.prepare(CanonicalRequest {
            command: "bitbucket.repository.list".into(),
            target: bad,
            input: json!({}),
            selection: Selection::default(),
            preconditions: None,
        });
        assert!(result.is_err());
    }
    Ok(())
}

#[test]
fn a_cursor_position_must_be_a_single_bounded_page_token() -> TestResult {
    let (_, operation) = prepare("bitbucket.pipeline.list", json!({"repo": "service"}))?;
    for bad in [
        NativePosition(BTreeMap::from([("offset".into(), "1".into())])),
        position("a b"),
        position("../x"),
        position(&"a".repeat(257)),
    ] {
        let mut context = Mock::ok(page(vec![], None));
        assert!(read_page(&operation, &mut context, Some(&bad)).is_err());
        assert!(context.calls.is_empty());
    }
    Ok(())
}
