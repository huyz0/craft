//! Request construction and answer interpretation for every Bitbucket write, against documented
//! response shapes. Live verification is recorded separately.
use craft_core::contracts::{CanonicalRequest, ResourceRef, Selection};
use craft_core::error::ErrorCode;
use craft_core::execution::{HttpMethod, NativeResponse, PreparedOperation};
use craft_core::registry::Registry;
use craft_core::storage::SubmissionState;
use craft_provider_bitbucket::{
    BitbucketContext, build, interpret, step, write_commands, write_entry,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const UUID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

fn request(command: &str, input: Value) -> CanonicalRequest {
    CanonicalRequest {
        command: command.into(),
        target: ResourceRef {
            provider: "bitbucket".into(),
            deployment: "cloud".into(),
            tenant: "team".into(),
            resource_type: "workspace".into(),
            id: "team".into(),
        },
        input,
        selection: Selection::default(),
        preconditions: None,
    }
}

fn prepared(command: &str, input: Value) -> Result<PreparedOperation, Box<dyn std::error::Error>> {
    let mut registry = Registry::default();
    registry.register_provider(&BitbucketContext)?;
    Ok(registry.prepare(request(command, input))?)
}

fn answer(status: u16, body: Value) -> NativeResponse {
    NativeResponse {
        status,
        body,
        request_id: None,
        retry_after: None,
        link_headers: None,
    }
}

fn pr(id: u64, state: &str) -> Value {
    json!({
        "id": id, "title": "Fix login", "description": "details", "state": state,
        "source": {"branch": {"name": "feature/login"}},
        "destination": {"branch": {"name": "main"}},
        "close_source_branch": true,
        "links": {"html": {"href": "https://bitbucket.org/team/service/pull-requests/1"}}
    })
}

#[test]
fn every_write_declares_one_route_and_a_matching_journal_step() -> TestResult {
    let mut registry = Registry::default();
    registry.register_provider(&BitbucketContext)?;
    assert_eq!(write_commands().count(), 10);
    for command in write_commands() {
        let descriptor = registry.descriptor(command)?;
        assert_eq!(descriptor.routes.len(), 1, "{command}");
        assert_eq!(Some(descriptor.routes[0].id.clone()), step(command));
        assert!(!matches!(descriptor.routes[0].method, HttpMethod::Get));
        assert!(write_entry(command).is_some());
    }
    Ok(())
}

#[test]
fn branch_create_posts_the_exact_commit_in_lowercase() -> TestResult {
    let call = build(&request(
        "bitbucket.branch.create",
        json!({"repo": "service", "name": "feature/login", "source_commit": COMMIT.to_uppercase()}),
    ))?;
    assert_eq!(call.route, "bitbucket-branch-create");
    assert_eq!(call.path_parameters["workspace"], "team");
    assert_eq!(call.path_parameters["repo"], "service");
    assert_eq!(
        call.body,
        Some(json!({"name": "feature/login", "target": {"hash": COMMIT}}))
    );
    Ok(())
}

#[test]
fn invalid_branch_names_and_short_hashes_are_refused_before_any_call() {
    for name in [
        "a b", "a..b", "-x", "/x", "x/", "x.lock", "a~b", "a:b", "a\\b", "a//b",
    ] {
        assert!(
            build(&request(
                "bitbucket.branch.create",
                json!({"repo": "service", "name": name, "source_commit": COMMIT}),
            ))
            .is_err(),
            "{name}"
        );
    }
    assert!(
        build(&request(
            "bitbucket.branch.create",
            json!({"repo": "service", "name": "x", "source_commit": "abc123"}),
        ))
        .is_err()
    );
}

#[test]
fn pull_request_create_maps_branches_and_reviewer_uuids() -> TestResult {
    let call = build(&request(
        "bitbucket.pullrequest.create",
        json!({
            "repo": "service", "title": "Fix login", "source_branch": "feature/login",
            "destination_branch": "main", "description": "details",
            "close_source_branch": true, "reviewer_uuids": [format!("{{{UUID}}}")]
        }),
    ))?;
    let body = call.body.ok_or("body")?;
    assert_eq!(body["source"]["branch"]["name"], "feature/login");
    assert_eq!(body["destination"]["branch"]["name"], "main");
    assert_eq!(body["reviewers"][0]["uuid"], format!("{{{UUID}}}"));
    assert_eq!(body["close_source_branch"], true);
    for bad in [json!(["nope"]), json!([UUID, UUID])] {
        assert!(
            build(&request(
                "bitbucket.pullrequest.create",
                json!({"repo": "service", "title": "t", "source_branch": "b", "reviewer_uuids": bad}),
            ))
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn update_needs_a_change_and_comment_shapes_are_exclusive() -> TestResult {
    assert!(
        build(&request(
            "bitbucket.pullrequest.update",
            json!({"repo": "service", "pull_request_id": 1}),
        ))
        .is_err()
    );
    let general = build(&request(
        "bitbucket.comment.add",
        json!({"repo": "service", "pull_request_id": 1, "text": "Looks good"}),
    ))?;
    assert_eq!(
        general.body,
        Some(json!({"content": {"raw": "Looks good"}}))
    );
    let inline = build(&request(
        "bitbucket.comment.add",
        json!({"repo": "service", "pull_request_id": 1, "text": "Rename", "path": "src/a.rs", "line": 12}),
    ))?;
    assert_eq!(
        inline.body,
        Some(json!({"content": {"raw": "Rename"}, "inline": {"path": "src/a.rs", "to": 12}}))
    );
    let reply = build(&request(
        "bitbucket.comment.add",
        json!({"repo": "service", "pull_request_id": 1, "text": "Done", "parent_id": 5}),
    ))?;
    assert_eq!(
        reply.body,
        Some(json!({"content": {"raw": "Done"}, "parent": {"id": 5}}))
    );
    for bad in [
        json!({"path": "src/a.rs"}),
        json!({"line": 3}),
        json!({"path": "src/a.rs", "line": 3, "parent_id": 5}),
    ] {
        let mut input = json!({"repo": "service", "pull_request_id": 1, "text": "x"});
        input
            .as_object_mut()
            .ok_or("object")?
            .extend(bad.as_object().ok_or("object")?.clone());
        assert!(build(&request("bitbucket.comment.add", input)).is_err());
    }
    Ok(())
}

#[test]
fn merge_requires_an_explicit_strategy_and_decline_carries_its_reason() -> TestResult {
    assert!(
        build(&request(
            "bitbucket.pullrequest.merge",
            json!({"repo": "service", "pull_request_id": 1}),
        ))
        .is_err()
    );
    assert!(
        build(&request(
            "bitbucket.pullrequest.merge",
            json!({"repo": "service", "pull_request_id": 1, "merge_strategy": "rebase"}),
        ))
        .is_err()
    );
    let merge = build(&request(
        "bitbucket.pullrequest.merge",
        json!({"repo": "service", "pull_request_id": 1, "merge_strategy": "squash", "close_source_branch": true}),
    ))?;
    let body = merge.body.ok_or("body")?;
    assert_eq!(body["merge_strategy"], "squash");
    assert_eq!(body["close_source_branch"], true);
    assert_eq!(merge.path_parameters["pull_request_id"], "1");
    let decline = build(&request(
        "bitbucket.pullrequest.decline",
        json!({"repo": "service", "pull_request_id": 2, "reason": "superseded"}),
    ))?;
    assert_eq!(decline.body, Some(json!({"reason": "superseded"})));
    Ok(())
}

#[test]
fn approvals_have_no_body_and_unknown_fields_are_refused() -> TestResult {
    for command in [
        "bitbucket.pullrequest.approve",
        "bitbucket.pullrequest.unapprove",
        "bitbucket.pullrequest.request-changes",
        "bitbucket.pullrequest.unrequest-changes",
    ] {
        let call = build(&request(
            command,
            json!({"repo": "service", "pull_request_id": 3}),
        ))?;
        assert!(call.body.is_none(), "{command}");
        assert!(
            build(&request(
                command,
                json!({"repo": "service", "pull_request_id": 3, "workspace": "other"}),
            ))
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn a_created_pull_request_is_confirmed_by_its_new_identity() -> TestResult {
    let operation = prepared(
        "bitbucket.pullrequest.create",
        json!({"repo": "service", "title": "Fix login", "source_branch": "feature/login", "destination_branch": "main"}),
    )?;
    let observation = interpret(&operation, &answer(201, pr(7, "OPEN")))?;
    assert_eq!(observation.state, SubmissionState::Confirmed);
    assert!(observation.desired_fields_verified && observation.error.is_none());
    assert_eq!(observation.resources[0].resource_type, "pullrequest");
    assert_eq!(observation.resources[0].id, "7");
    let item = observation.item.ok_or("item")?;
    assert_eq!(item["source_branch"], "feature/login");
    assert_eq!(
        item["url"],
        "https://bitbucket.org/team/service/pull-requests/1"
    );
    Ok(())
}

#[test]
fn a_success_that_does_not_show_the_change_is_a_partial_apply_not_a_clean_confirm() -> TestResult {
    let operation = prepared(
        "bitbucket.pullrequest.create",
        json!({"repo": "service", "title": "Different", "source_branch": "feature/login"}),
    )?;
    let observation = interpret(&operation, &answer(201, pr(7, "OPEN")))?;
    assert_eq!(observation.state, SubmissionState::Confirmed);
    assert!(!observation.desired_fields_verified);
    assert_eq!(
        observation.error.ok_or("error")?.code,
        ErrorCode::PartialApply
    );
    Ok(())
}

#[test]
fn merge_is_verified_by_state_and_an_async_acceptance_stays_unknown() -> TestResult {
    let operation = prepared(
        "bitbucket.pullrequest.merge",
        json!({"repo": "service", "pull_request_id": 7, "merge_strategy": "squash"}),
    )?;
    let merged = interpret(&operation, &answer(200, pr(7, "MERGED")))?;
    assert!(merged.desired_fields_verified && merged.error.is_none());
    assert_eq!(merged.resources[0].id, "7");
    let still_open = interpret(&operation, &answer(200, pr(7, "OPEN")))?;
    assert_eq!(
        still_open.error.ok_or("error")?.code,
        ErrorCode::PartialApply
    );
    let wrong_pull_request = interpret(&operation, &answer(200, pr(8, "MERGED")))?;
    assert_eq!(wrong_pull_request.state, SubmissionState::Unknown);
    let asynchronous = interpret(&operation, &answer(202, Value::Null))?;
    assert_eq!(asynchronous.state, SubmissionState::Unknown);
    assert!(asynchronous.resources.is_empty());
    Ok(())
}

#[test]
fn a_merge_conflict_is_a_clean_refusal_that_applied_nothing() -> TestResult {
    let operation = prepared(
        "bitbucket.pullrequest.merge",
        json!({"repo": "service", "pull_request_id": 7, "merge_strategy": "merge_commit"}),
    )?;
    let observation = interpret(&operation, &answer(409, Value::Null))?;
    assert_eq!(observation.state, SubmissionState::Failed);
    assert_eq!(
        observation.error.ok_or("error")?.code,
        ErrorCode::VersionConflict
    );
    Ok(())
}

#[test]
fn deleting_an_approval_succeeds_on_an_empty_204() -> TestResult {
    let operation = prepared(
        "bitbucket.pullrequest.unapprove",
        json!({"repo": "service", "pull_request_id": 9}),
    )?;
    let observation = interpret(&operation, &answer(204, Value::Null))?;
    assert_eq!(observation.state, SubmissionState::Confirmed);
    assert!(observation.desired_fields_verified);
    assert_eq!(observation.resources[0].id, "9");
    // A 200 on a POST-only command is not how an approval is withdrawn.
    let approve = prepared(
        "bitbucket.pullrequest.approve",
        json!({"repo": "service", "pull_request_id": 9}),
    )?;
    let ok = interpret(
        &approve,
        &answer(200, json!({"approved": true, "state": "approved"})),
    )?;
    assert!(ok.desired_fields_verified);
    let not_applied = interpret(&approve, &answer(200, json!({"approved": false})))?;
    assert_eq!(
        not_applied.error.ok_or("error")?.code,
        ErrorCode::PartialApply
    );
    Ok(())
}

#[test]
fn request_changes_is_verified_by_the_participant_state() -> TestResult {
    let operation = prepared(
        "bitbucket.pullrequest.request-changes",
        json!({"repo": "service", "pull_request_id": 9}),
    )?;
    let ok = interpret(
        &operation,
        &answer(
            200,
            json!({"approved": false, "state": "changes_requested"}),
        ),
    )?;
    assert!(ok.desired_fields_verified && ok.error.is_none());
    let silent = interpret(&operation, &answer(200, json!({"approved": false})))?;
    assert_eq!(silent.error.ok_or("error")?.code, ErrorCode::PartialApply);
    Ok(())
}

#[test]
fn a_created_comment_and_branch_are_identified_by_what_bitbucket_names() -> TestResult {
    let comment = prepared(
        "bitbucket.comment.add",
        json!({"repo": "service", "pull_request_id": 7, "text": "Looks good"}),
    )?;
    let observation = interpret(
        &comment,
        &answer(201, json!({"id": 55, "content": {"raw": "Looks good"}})),
    )?;
    assert_eq!(observation.resources[0].resource_type, "comment");
    assert_eq!(observation.resources[0].id, "55");
    assert!(observation.desired_fields_verified);
    let branch = prepared(
        "bitbucket.branch.create",
        json!({"repo": "service", "name": "feature/login", "source_commit": COMMIT}),
    )?;
    let observation = interpret(
        &branch,
        &answer(
            201,
            json!({"name": "feature/login", "target": {"hash": COMMIT}}),
        ),
    )?;
    assert_eq!(observation.resources[0].resource_type, "branch");
    assert_eq!(observation.resources[0].id, "feature/login");
    assert!(observation.desired_fields_verified);
    let other = interpret(
        &branch,
        &answer(201, json!({"name": "other", "target": {"hash": COMMIT}})),
    )?;
    assert!(!other.desired_fields_verified);
    assert_eq!(other.error.ok_or("error")?.code, ErrorCode::PartialApply);
    Ok(())
}

#[test]
fn refusals_are_failed_and_unclear_answers_are_unknown() -> TestResult {
    let operation = prepared(
        "bitbucket.pullrequest.decline",
        json!({"repo": "service", "pull_request_id": 7}),
    )?;
    for (status, code) in [
        (400, ErrorCode::InputInvalid),
        (401, ErrorCode::AuthRequired),
        (403, ErrorCode::PermissionDenied),
        (404, ErrorCode::InputInvalid),
        (429, ErrorCode::RateLimited),
    ] {
        let observation = interpret(&operation, &answer(status, Value::Null))?;
        assert_eq!(observation.state, SubmissionState::Failed, "{status}");
        assert_eq!(observation.error.ok_or("error")?.code, code, "{status}");
    }
    for status in [500, 502, 503, 201] {
        let observation = interpret(&operation, &answer(status, Value::Null))?;
        assert_eq!(observation.state, SubmissionState::Unknown, "{status}");
        assert_eq!(
            observation.error.ok_or("error")?.code,
            ErrorCode::WriteOutcomeUnknown
        );
    }
    Ok(())
}
