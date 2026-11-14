use super::*;
use craft_core::contracts::{GrantRef, Selection};
use craft_core::execution::NativeResponse;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
type TestResult = Result<(), Box<dyn std::error::Error>>;
struct TestClock {
    wall: AtomicU64,
    mono: AtomicU64,
}
impl TestClock {
    fn new() -> Self {
        Self {
            wall: AtomicU64::new(1000),
            mono: AtomicU64::new(0),
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
fn input() -> LinkInput {
    LinkInput {
        inward_issue_id: "10001".into(),
        outward_issue_id: "10002".into(),
        inward_project_id: "200".into(),
        link_type: LinkType {
            id: "10000".into(),
            name: "Blocks".into(),
            inward: "is blocked by".into(),
            outward: "blocks".into(),
        },
    }
}
fn target() -> ResourceRef {
    ResourceRef {
        provider: "jira".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "project".into(),
        id: "100".into(),
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
fn operations(value: &LinkInput) -> Result<(PreparedOperation, PreparedOperation), CraftError> {
    let mut registry = Registry::default();
    register(&mut registry)?;
    let make = |command: &str| {
        registry.prepare(CanonicalRequest {
            command: command.into(),
            target: target(),
            input: json!(value),
            selection: Selection::default(),
            preconditions: None,
        })
    };
    Ok((make(CONTEXT)?, make(CREATE)?))
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
    let types = json!({"issueLinkTypes":[input().link_type]});
    let outward = json!({"id":"10002","fields":{"project":{"id":"100"}}});
    let inward = json!({"id":"10001","fields":{"project":{"id":"200"}}});
    let permission = json!({"permissions":{"BROWSE_PROJECTS":{"key":"BROWSE_PROJECTS","havePermission":true},"LINK_ISSUES":{"key":"LINK_ISSUES","havePermission":true}}});
    [
        types.clone(),
        outward.clone(),
        inward.clone(),
        permission.clone(),
        permission,
        outward,
        inward,
        types,
    ]
    .into_iter()
    .map(|body| response(200, body))
    .collect()
}
struct Port {
    digest: String,
    replies: VecDeque<NativeResponse>,
    calls: Vec<NativeCall>,
}
impl Port {
    fn new(op: &PreparedOperation, replies: Vec<NativeResponse>) -> Self {
        Self {
            digest: op.digest().into(),
            replies: replies.into(),
            calls: vec![],
        }
    }
}
impl OperationContext for Port {
    fn checkpoint(&self) -> Result<(), CraftError> {
        Ok(())
    }
    fn verify_operation(&self, op: &PreparedOperation) -> Result<(), CraftError> {
        if op.digest() == self.digest {
            Ok(())
        } else {
            Err(CraftError::new(ErrorCode::PolicyDenied, "wrong port"))
        }
    }
    fn invoke(&mut self, call: &NativeCall) -> Result<NativeResponse, CraftError> {
        self.calls.push(call.clone());
        self.replies.pop_front().ok_or_else(malformed)
    }
}
#[test]
fn link_preparation_brackets_type_projects_and_permissions_and_freezes_exact_roles() -> TestResult {
    let (read, write) = operations(&input())?;
    let mut port = Port::new(&read, replies());
    let frozen = prepare_link(
        &read,
        &write,
        &mut port,
        principal(),
        Arc::new(TestClock::new()),
    )?;
    assert_eq!(port.calls.len(), 8);
    assert_eq!(
        port.calls[3].query["permissions"],
        "BROWSE_PROJECTS,LINK_ISSUES"
    );
    assert_eq!(port.calls[4].query["permissions"], "BROWSE_PROJECTS");
    assert_eq!(
        frozen.call().body,
        Some(
            json!({"type":{"id":"10000"},"inwardIssue":{"id":"10001"},"outwardIssue":{"id":"10002"}})
        )
    );
    frozen.verify(&write, &principal())?;
    let mut other = principal();
    other.actor = "different".into();
    assert!(frozen.verify(&write, &other).is_err());
    let mut changed = input();
    changed.outward_issue_id = "10003".into();
    assert!(
        frozen
            .verify(&operations(&changed)?.1, &principal())
            .is_err()
    );
    Ok(())
}
#[test]
fn disabled_changed_unreadable_or_unpermitted_link_evidence_never_freezes() -> TestResult {
    let (read, write) = operations(&input())?;
    for (index, field, value) in [
        (0, "type", json!("renamed")),
        (2, "project", json!("999")),
        (3, "permission", json!(false)),
        (4, "permission", json!(false)),
        (5, "project", json!("999")),
        (7, "type", json!("renamed")),
    ] {
        let mut rows = replies();
        match field {
            "type" => rows[index].body["issueLinkTypes"][0]["outward"] = value,
            "project" => rows[index].body["fields"]["project"]["id"] = value,
            _ => {
                rows[index].body["permissions"][if index == 3 {
                    "LINK_ISSUES"
                } else {
                    "BROWSE_PROJECTS"
                }]["havePermission"] = value
            }
        }
        let mut port = Port::new(&read, rows);
        assert!(
            prepare_link(
                &read,
                &write,
                &mut port,
                principal(),
                Arc::new(TestClock::new())
            )
            .is_err()
        );
        assert_eq!(port.calls.len(), index + 1);
    }
    let mut port = Port::new(&read, vec![response(404, json!({}))]);
    assert!(
        prepare_link(
            &read,
            &write,
            &mut port,
            principal(),
            Arc::new(TestClock::new())
        )
        .is_err()
    );
    assert_eq!(port.calls.len(), 1);
    Ok(())
}
#[test]
fn frozen_link_uses_original_wall_and_monotonic_window_and_detects_rollback() -> TestResult {
    for mode in ["wall", "mono", "rollback"] {
        let (read, write) = operations(&input())?;
        let mut port = Port::new(&read, replies());
        let clock = Arc::new(TestClock::new());
        let frozen = prepare_link(&read, &write, &mut port, principal(), clock.clone())?;
        if mode == "rollback" {
            clock.wall.store(2000, Ordering::SeqCst);
            frozen.verify(&write, &principal())?;
            clock.wall.store(1999, Ordering::SeqCst);
        } else if mode == "wall" {
            clock.wall.store(31000, Ordering::SeqCst);
        } else {
            clock.mono.store(30000, Ordering::SeqCst);
        }
        assert!(frozen.verify(&write, &principal()).is_err(), "{mode}");
    }
    Ok(())
}
#[test]
fn only_documented_link_acknowledgment_confirms_relation_without_new_creation_attribution() {
    for status in [200, 201, 204, 400, 401, 403, 404, 413, 429, 500] {
        let observed = interpret(
            &target(),
            &input(),
            &response(status, json!({"untrusted":"native-sentinel"})),
        );
        assert_eq!(
            observed.state,
            if status == 201 {
                SubmissionState::Confirmed
            } else if matches!(status, 400 | 401 | 404 | 413) {
                SubmissionState::Failed
            } else {
                SubmissionState::Unknown
            }
        );
        assert_eq!(observed.resources.len(), if status == 201 { 2 } else { 0 });
        if let Some(error) = observed.error {
            assert!(!error.message.contains("native-sentinel"));
        }
    }
}
