use super::*;
use craft_core::contracts::{GrantRef, Selection};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn raw_attachment(author: &str) -> Value {
    json!({"id":"301","filename":"design.bin","size":4,"mimeType":"application/octet-stream",
        "created":"2026-11-14T00:00:00.000+0000","author":{"accountId":author},
        "content":"https://untrusted.invalid/private"})
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "200".into(),
    }
}
fn principal() -> MetadataPrincipal {
    MetadataPrincipal {
        grant: GrantRef {
            id: format!("gr_{}", "0".repeat(32)),
        },
        actor: "service-one".into(),
    }
}
fn value(author: &str, ownership: &str) -> Result<Value, CraftError> {
    let revision = parse_record(&raw_attachment(author))?.metadata_revision;
    Ok(
        json!({"issue_id":"100","attachment_id":"301","expected_revision":revision,"ownership":ownership}),
    )
}
fn operations(value: Value) -> Result<(PreparedOperation, PreparedOperation), CraftError> {
    let mut registry = Registry::default();
    register(&mut registry)?;
    let make = |command: &str| {
        registry.prepare(CanonicalRequest {
            command: command.into(),
            target: target(),
            input: value.clone(),
            selection: Selection::default(),
            preconditions: None,
        })
    };
    Ok((make(CONTEXT)?, make(DELETE)?))
}
fn response(status: u16, body: Value) -> NativeResponse {
    NativeResponse {
        status,
        body,
        request_id: None,
        retry_after: None,
        link_headers: None,
    }
}
fn issue(project: &str, listed: bool) -> Value {
    let rows = if listed {
        vec![raw_attachment("service-one")]
    } else {
        vec![]
    };
    json!({"id":"100","fields":{"project":{"id":project},"attachment":rows}})
}
fn permissions(own: bool, all: bool) -> Value {
    json!({"permissions":{
        "BROWSE_PROJECTS":{"key":"BROWSE_PROJECTS","havePermission":true},
        "DELETE_OWN_ATTACHMENTS":{"key":"DELETE_OWN_ATTACHMENTS","havePermission":own},
        "DELETE_ALL_ATTACHMENTS":{"key":"DELETE_ALL_ATTACHMENTS","havePermission":all}}})
}
fn replies(author: &str, own: bool, all: bool) -> Vec<NativeResponse> {
    [
        issue("200", true),
        raw_attachment(author),
        permissions(own, all),
        raw_attachment(author),
        issue("200", true),
    ]
    .into_iter()
    .map(|v| response(200, v))
    .collect()
}
struct TestClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl Clock for TestClock {
    fn unix_millis(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn monotonic_millis(&self) -> u64 {
        self.mono.load(Ordering::SeqCst)
    }
}
fn clock() -> Arc<TestClock> {
    Arc::new(TestClock {
        wall: AtomicU64::new(1000),
        mono: AtomicU64::new(100),
    })
}
struct Port {
    digest: String,
    replies: VecDeque<NativeResponse>,
    calls: Vec<NativeCall>,
}
impl OperationContext for Port {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, op: &PreparedOperation) -> Result<(), CraftError> {
        if op.digest() == self.digest {
            Ok(())
        } else {
            Err(CraftError::new(ErrorCode::PolicyDenied, "Wrong operation"))
        }
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        self.replies.pop_front().ok_or_else(malformed)
    }
}
fn port(read: &PreparedOperation, rows: Vec<NativeResponse>) -> Port {
    Port {
        digest: read.digest().into(),
        replies: rows.into(),
        calls: vec![],
    }
}
fn refused(
    input: Value,
    rows: Vec<NativeResponse>,
) -> Result<(CraftError, usize), Box<dyn std::error::Error>> {
    let (read, write) = operations(input)?;
    let mut p = port(&read, rows);
    let error = prepare(&read, &write, &mut p, principal(), clock())
        .err()
        .ok_or("preparation unexpectedly succeeded")?;
    Ok((error, p.calls.len()))
}
#[test]
fn preparation_reads_membership_metadata_and_permissions_before_freezing_delete() -> TestResult {
    let (read, write) = operations(value("service-one", "own")?)?;
    let mut port = port(&read, replies("service-one", true, false));
    let frozen = prepare(&read, &write, &mut port, principal(), clock())?;
    frozen.verify(&write, &principal())?;
    let routes: Vec<_> = port.calls.iter().map(|c| c.route.as_str()).collect();
    assert_eq!(routes, [ISSUE, METADATA, PERMISSIONS, METADATA, ISSUE]);
    assert_eq!(port.calls[0].query["fields"], "project,attachment");
    assert_eq!(
        port.calls[2].query["permissions"],
        "BROWSE_PROJECTS,DELETE_OWN_ATTACHMENTS,DELETE_ALL_ATTACHMENTS"
    );
    assert_eq!(port.calls[2].query["issueId"], "100");
    assert_eq!(frozen.call().route, DELETE_ROUTE);
    assert_eq!(frozen.call().path_parameters["id"], "301");
    assert!(frozen.call().body.is_none());
    assert_eq!(frozen.before().id, "301");
    Ok(())
}
#[test]
fn delete_route_is_the_documented_permanent_attachment_path() -> TestResult {
    let (_, write) = operations(value("service-one", "own")?)?;
    assert!(matches!(write.effect(), EffectClass::Permanent));
    let mut registry = Registry::default();
    register(&mut registry)?;
    let route = &registry.descriptor(DELETE)?.routes[0];
    assert!(matches!(route.method, HttpMethod::Delete));
    let path: Vec<String> = route
        .path
        .iter()
        .map(|segment| match segment {
            RouteSegment::Literal(v) => v.clone(),
            RouteSegment::Parameter(v) => format!("{{{v}}}"),
        })
        .collect();
    assert_eq!(path.join("/"), "rest/api/3/attachment/{id}");
    Ok(())
}
#[test]
fn own_intent_refuses_another_uploader_and_any_intent_requires_all_permission() -> TestResult {
    let (error, _) = refused(value("human-two", "own")?, replies("human-two", true, true))?;
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    let (error, _) = refused(
        value("human-two", "any")?,
        replies("human-two", true, false),
    )?;
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    let (read, write) = operations(value("human-two", "any")?)?;
    let mut p = port(&read, replies("human-two", false, true));
    prepare(&read, &write, &mut p, principal(), clock())?;
    let (error, _) = refused(
        value("service-one", "own")?,
        replies("service-one", false, false),
    )?;
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    Ok(())
}
#[test]
fn stale_revision_foreign_project_or_unlisted_attachment_refuses_before_send() -> TestResult {
    let mut input = value("service-one", "own")?;
    input["expected_revision"] = json!(format!("sha256:{}", "1".repeat(64)));
    let (error, _) = refused(input, replies("service-one", true, false))?;
    assert_eq!(error.code, ErrorCode::VersionConflict);
    for first in [issue("999", true), issue("200", false)] {
        let (error, calls) = refused(value("service-one", "own")?, vec![response(200, first)])?;
        assert_eq!(error.code, ErrorCode::VersionConflict);
        assert_eq!(calls, 1);
    }
    let (error, _) = refused(value("service-one", "own")?, vec![response(404, json!({}))])?;
    assert_eq!(error.code, ErrorCode::RemoteUnavailable);
    Ok(())
}
#[test]
fn expired_preparation_refuses_to_verify() -> TestResult {
    let (read, write) = operations(value("service-one", "own")?)?;
    let clock = clock();
    let mut p = port(&read, replies("service-one", true, false));
    let frozen = prepare(&read, &write, &mut p, principal(), clock.clone())?;
    clock.wall.fetch_add(30_000, Ordering::SeqCst);
    let error = frozen.verify(&write, &principal()).err().ok_or("expired")?;
    assert_eq!(error.code, ErrorCode::VersionConflict);
    Ok(())
}
#[test]
fn interpretation_confirms_only_204_and_separates_refusal_from_unknown() -> TestResult {
    let (_, write) = operations(value("service-one", "own")?)?;
    let done = interpret(&write, &response(204, Value::Null))?;
    assert_eq!(done.state, SubmissionState::Confirmed);
    assert!(done.acknowledged && done.error.is_none());
    assert_eq!(done.resources[0].resource_type, "attachment");
    assert_eq!(done.resources[0].id, "301");
    assert_eq!(done.resources[0].tenant, target().tenant);
    for (status, state) in [
        (403, SubmissionState::Failed),
        (404, SubmissionState::Failed),
        (200, SubmissionState::Unknown),
        (500, SubmissionState::Unknown),
        (429, SubmissionState::Unknown),
    ] {
        let seen = interpret(&write, &response(status, json!({})))?;
        assert_eq!(seen.state, state, "status {status}");
        assert!(seen.resources.is_empty() && !seen.acknowledged);
        let expected = if state == SubmissionState::Failed {
            RequestState::Failed
        } else {
            RequestState::SentUnknown
        };
        assert_eq!(seen.error.ok_or("error")?.details.write_state, expected);
    }
    Ok(())
}
#[test]
fn input_rejects_noncanonical_identities_and_revisions() -> TestResult {
    let mut registry = Registry::default();
    register(&mut registry)?;
    let zero = format!("sha256:{}", "0".repeat(64));
    for bad in [
        json!({"issue_id":"0100","attachment_id":"301","expected_revision":zero}),
        json!({"issue_id":"100","attachment_id":"x","expected_revision":zero}),
        json!({"issue_id":"100","attachment_id":"301","expected_revision":"sha256:ABC"}),
        json!({"issue_id":"100","attachment_id":"301","expected_revision":zero,"filename":"x"}),
    ] {
        let prepared = registry.prepare(CanonicalRequest {
            command: DELETE.into(),
            target: target(),
            input: bad,
            selection: Selection::default(),
            preconditions: None,
        });
        assert!(prepared.is_err());
    }
    Ok(())
}
