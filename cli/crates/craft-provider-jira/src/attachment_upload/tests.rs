use super::*;
use craft_core::contracts::{GrantRef, Selection};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "200".into(),
    }
}
fn value() -> Value {
    json!({"issue_id":"100","artifact_name":"snapshot.bin","digest":format!("sha256:{}","a".repeat(64)),"source_selection_digest":format!("sha256:{}","0".repeat(64)),"size_bytes":12,"filename":"設計.bin","media_type":"application/octet-stream"})
}
fn principal() -> MetadataPrincipal {
    MetadataPrincipal {
        grant: GrantRef {
            id: format!("gr_{}", "0".repeat(32)),
        },
        actor: "service-one".into(),
    }
}
fn operations(input: Value) -> Result<(PreparedOperation, PreparedOperation), CraftError> {
    let mut registry = Registry::default();
    register(&mut registry)?;
    let make = |command: &str| {
        registry.prepare(CanonicalRequest {
            command: command.into(),
            target: target(),
            input: input.clone(),
            selection: Selection::default(),
            preconditions: None,
        })
    };
    Ok((make(CONTEXT)?, make(UPLOAD)?))
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
fn replies() -> Vec<NativeResponse> {
    let issue = json!({"id":"100","fields":{"project":{"id":"200"}}});
    let settings = json!({"enabled":true,"uploadLimit":1000});
    vec![issue.clone(),settings.clone(),json!({"permissions":{"BROWSE_PROJECTS":{"key":"BROWSE_PROJECTS","havePermission":true},"CREATE_ATTACHMENTS":{"key":"CREATE_ATTACHMENTS","havePermission":true}}}),settings,issue].into_iter().map(|v|response(200,v)).collect()
}
struct TestClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl TestClock {
    fn new() -> Self {
        Self {
            wall: AtomicU64::new(1000),
            mono: AtomicU64::new(100),
        }
    }
}
impl Clock for TestClock {
    fn unix_millis(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn monotonic_millis(&self) -> u64 {
        self.mono.load(Ordering::SeqCst)
    }
}
struct Port {
    digest: String,
    rows: VecDeque<NativeResponse>,
    calls: Vec<NativeCall>,
}
impl Port {
    fn new(op: &PreparedOperation, rows: Vec<NativeResponse>) -> Self {
        Self {
            digest: op.digest().into(),
            rows: rows.into(),
            calls: vec![],
        }
    }
}
impl OperationContext for Port {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, operation: &PreparedOperation) -> Result<(), CraftError> {
        if operation.digest() == self.digest {
            Ok(())
        } else {
            Err(CraftError::new(ErrorCode::PolicyDenied, "Foreign context"))
        }
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        self.rows.pop_front().ok_or_else(malformed)
    }
}
#[test]
fn upload_context_brackets_project_and_settings_with_exact_permission_keys() -> TestResult {
    let (read, write) = operations(value())?;
    let mut port = Port::new(&read, replies());
    let clock = Arc::new(TestClock::new());
    let frozen = prepare(&read, &write, &mut port, principal(), clock)?;
    frozen.verify(&write, &principal())?;
    assert_eq!(port.calls.len(), 5);
    assert_eq!(
        port.calls
            .iter()
            .map(|c| c.route.as_str())
            .collect::<Vec<_>>(),
        vec![ISSUE, SETTINGS, PERMISSIONS, SETTINGS, ISSUE]
    );
    assert_eq!(
        port.calls[0].query,
        BTreeMap::from([
            ("fields".into(), "project".into()),
            ("updateHistory".into(), "false".into()),
            ("failFast".into(), "true".into())
        ])
    );
    assert_eq!(
        port.calls[2].query,
        BTreeMap::from([
            ("issueId".into(), "100".into()),
            (
                "permissions".into(),
                "BROWSE_PROJECTS,CREATE_ATTACHMENTS".into()
            )
        ])
    );
    assert_eq!(frozen.call().route, UPLOAD_ROUTE);
    assert_eq!(frozen.call().path_parameters["issueIdOrKey"], "100");
    assert!(frozen.call().body.is_none());
    assert_eq!(frozen.input().size_bytes, 12);
    assert_eq!(frozen.settings().upload_limit_bytes, Some(1000));
    Ok(())
}
#[test]
fn disabled_unknown_oversize_denied_changed_or_malformed_context_never_prepares_upload()
-> TestResult {
    for mode in [
        "disabled",
        "unknown_enabled",
        "unknown_limit",
        "oversize",
        "permission",
        "malformed_permission",
        "changed_settings",
        "changed_project",
        "wrong_issue",
        "warning",
        "denied",
    ] {
        let (read, write) = operations(value())?;
        let mut rows = replies();
        match mode {
            "disabled" => rows[1].body["enabled"] = json!(false),
            "unknown_enabled" => rows[1].body["enabled"] = Value::Null,
            "unknown_limit" => rows[1].body["uploadLimit"] = Value::Null,
            "oversize" => rows[1].body["uploadLimit"] = json!(11),
            "permission" => {
                rows[2].body["permissions"]["CREATE_ATTACHMENTS"]["havePermission"] = json!(false)
            }
            "malformed_permission" => {
                rows[2].body["permissions"]["CREATE_ATTACHMENTS"]["key"] = json!("ADD_COMMENTS")
            }
            "changed_settings" => rows[3].body["uploadLimit"] = json!(2000),
            "changed_project" => rows[4].body["fields"]["project"]["id"] = json!("201"),
            "wrong_issue" => rows[0].body["id"] = json!("101"),
            "warning" => rows[1].body["warningMessages"] = json!(["incomplete"]),
            _ => rows[0].status = 403,
        }
        let mut port = Port::new(&read, rows);
        assert!(
            prepare(
                &read,
                &write,
                &mut port,
                principal(),
                Arc::new(TestClock::new())
            )
            .is_err(),
            "{mode}"
        );
        assert!(port.calls.iter().all(|c| c.route != UPLOAD_ROUTE));
    }
    Ok(())
}
#[test]
fn frozen_upload_rejects_operation_principal_and_dual_clock_drift() -> TestResult {
    for mode in [
        "operation",
        "actor",
        "grant",
        "wall_back",
        "mono_back",
        "wall_expired",
        "mono_expired",
    ] {
        let (read, write) = operations(value())?;
        let clock = Arc::new(TestClock::new());
        let mut port = Port::new(&read, replies());
        let frozen = prepare(&read, &write, &mut port, principal(), clock.clone())?;
        let mut changed = principal();
        let mut input = value();
        match mode {
            "operation" => input["filename"] = json!("changed.bin"),
            "actor" => changed.actor = "other".into(),
            "grant" => changed.grant.id = format!("gr_{}", "1".repeat(32)),
            "wall_back" => clock.wall.store(999, Ordering::SeqCst),
            "mono_back" => clock.mono.store(99, Ordering::SeqCst),
            "wall_expired" => clock.wall.store(31000, Ordering::SeqCst),
            _ => clock.mono.store(30100, Ordering::SeqCst),
        }
        let (_, operation) = operations(input)?;
        assert!(frozen.verify(&operation, &changed).is_err(), "{mode}");
    }
    Ok(())
}
#[test]
fn invalid_upload_snapshot_metadata_is_refused_by_preparation() -> TestResult {
    for mode in [
        "issue", "name", "digest", "size", "filename", "media", "unknown",
    ] {
        let mut input = value();
        match mode {
            "issue" => input["issue_id"] = json!("01"),
            "name" => input["artifact_name"] = json!(".."),
            "digest" => input["digest"] = json!(format!("sha256:{}", "A".repeat(64))),
            "size" => input["size_bytes"] = json!(1_073_741_825u64),
            "filename" => input["filename"] = json!("bad\r\nInjected: x"),
            "media" => input["media_type"] = json!("text/plain; charset=utf-8"),
            _ => input["content_url"] = json!("https://untrusted.invalid"),
        };
        assert!(operations(input).is_err(), "{mode}");
    }
    Ok(())
}
fn attachment(id: &str) -> Value {
    json!({"id":id,"filename":"設計.bin","size":12,"mimeType":"application/pdf","author":{"accountId":"service-one","displayName":"private-sentinel"},"content":"https://untrusted.invalid/private-sentinel"})
}
#[test]
fn upload_acknowledgments_preserve_known_ids_without_inventing_remote_digest_verification()
-> TestResult {
    let (_, write) = operations(value())?;
    let full = interpret(&write, &response(200, json!([attachment("101")])))?;
    assert_eq!(full.state, SubmissionState::Confirmed);
    assert!(full.desired_fields_verified);
    assert!(full.error.is_none());
    assert_eq!(full.resources[0].resource_type, "attachment");
    assert_eq!(full.resources[0].id, "101");
    assert!(!serde_json::to_string(&full.attachment)?.contains("private-sentinel"));
    let missing = interpret(&write, &response(200, json!([{"id":"101"}])))?;
    assert_eq!(missing.state, SubmissionState::Confirmed);
    assert!(!missing.desired_fields_verified);
    assert_eq!(missing.error.ok_or("error")?.code, ErrorCode::PartialApply);
    let extra = interpret(
        &write,
        &response(200, json!([attachment("101"), attachment("102")])),
    )?;
    assert_eq!(extra.state, SubmissionState::Confirmed);
    assert_eq!(extra.resources.len(), 2);
    assert!(!extra.desired_fields_verified);
    assert!(extra.error.is_some());
    let unknown = interpret(
        &write,
        &response(200, json!([attachment("101"),{"id":"invalid"}])),
    )?;
    assert_eq!(unknown.state, SubmissionState::Unknown);
    assert_eq!(unknown.resources.len(), 1);
    assert!(!unknown.acknowledged_ids_complete);
    let capped = interpret(
        &write,
        &response(
            200,
            Value::Array((1..=1001).map(|n| json!({"id":n.to_string()})).collect()),
        ),
    )?;
    assert_eq!(capped.state, SubmissionState::Unknown);
    assert_eq!(capped.resources.len(), 1000);
    assert!(!capped.acknowledged_ids_complete);
    Ok(())
}
#[test]
fn unqualified_upload_statuses_and_empty_or_malformed_arrays_never_authorize_replay() -> TestResult
{
    let (_, write) = operations(value())?;
    for status in [201, 204, 400, 401, 403, 404, 413, 429, 500] {
        let observed = interpret(&write, &response(status, json!({"id":"101"})))?;
        assert_eq!(
            observed.state,
            if matches!(status, 401 | 403 | 404 | 413) {
                SubmissionState::Failed
            } else {
                SubmissionState::Unknown
            }
        );
        assert!(!observed.acknowledged);
        assert!(observed.resources.is_empty());
        assert!(observed.error.is_some());
    }
    for body in [json!([]), json!({"id":"101"}), json!([{"id":true}])] {
        let observed = interpret(&write, &response(200, body))?;
        assert_eq!(observed.state, SubmissionState::Unknown);
        assert!(observed.resources.is_empty());
        assert!(!observed.desired_fields_verified);
        assert!(observed.error.is_some());
    }
    Ok(())
}

#[test]
fn upload_context_pairing_and_port_identity_are_checked_before_reads() -> TestResult {
    let (read, write) = operations(value())?;
    let mut changed = value();
    changed["digest"] = json!(format!("sha256:{}", "b".repeat(64)));
    let (_, other) = operations(changed)?;
    let mut port = Port::new(&read, replies());
    assert!(
        prepare(
            &read,
            &other,
            &mut port,
            principal(),
            Arc::new(TestClock::new())
        )
        .is_err()
    );
    assert!(port.calls.is_empty());
    port.digest = "foreign".into();
    assert!(
        prepare(
            &read,
            &write,
            &mut port,
            principal(),
            Arc::new(TestClock::new())
        )
        .is_err()
    );
    assert!(port.calls.is_empty());
    Ok(())
}
