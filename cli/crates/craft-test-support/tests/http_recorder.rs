use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use craft_core::contracts::{GrantRef, ResourceRef};
use craft_core::input::content_digest;
use craft_core::storage::{
    JournalRecord, OperationBinding, OperationJournal, StoreError, SubmissionState,
};
use craft_test_support::http::{Exchange, ExpectedRequest, HttpRecorder, Reply};
use craft_test_support::state::{FaultPoint, Faults, MemoryJournal};

fn request(method: &str, target: &str, body: &[u8]) -> ExpectedRequest {
    ExpectedRequest {
        method: method.into(),
        target: target.into(),
        headers: BTreeMap::new(),
        body: body.into(),
    }
}

fn send(address: SocketAddr, method: &str, target: &str, body: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: fixture\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    Ok(response)
}

#[test]
fn records_a_complete_write_before_disconnect() -> Result<(), Box<dyn std::error::Error>> {
    let body = br#"{"title":"sentinel"}"#;
    let fixture = HttpRecorder::start(vec![Exchange {
        request: request("POST", "/records", body),
        reply: Reply::DisconnectAfterRead,
    }])?;
    let response = send(fixture.address(), "POST", "/records", body)?;
    assert!(response.is_empty());
    assert_eq!(fixture.request_count(), 1);
    let observed = fixture.verify()?;
    assert_eq!(observed[0].body, body);
    assert_eq!(observed[0].target, "/records");
    Ok(())
}

#[test]
fn supports_delayed_rate_limits_and_malformed_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = HttpRecorder::start(vec![
        Exchange {
            request: request("GET", "/records", &[]),
            reply: Reply::Response {
                status: 429,
                headers: BTreeMap::from([("Retry-After".into(), "1".into())]),
                body: b"{}".to_vec(),
                delay: Duration::from_millis(60),
            },
        },
        Exchange {
            request: request("GET", "/malformed", &[]),
            reply: Reply::Response {
                status: 200,
                headers: BTreeMap::new(),
                body: b"{".to_vec(),
                delay: Duration::ZERO,
            },
        },
    ])?;
    let started = Instant::now();
    let rate_limit = String::from_utf8(send(fixture.address(), "GET", "/records", &[])?)?;
    assert!(started.elapsed() >= Duration::from_millis(40));
    assert!(rate_limit.contains("429") && rate_limit.contains("Retry-After: 1"));
    let malformed = send(fixture.address(), "GET", "/malformed", &[])?;
    assert!(malformed.ends_with(b"\r\n\r\n{"));
    assert_eq!(fixture.verify()?.len(), 2);
    Ok(())
}

#[test]
fn accepted_connections_wait_for_fragmented_headers_and_bodies()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = HttpRecorder::start(vec![Exchange {
        request: request("POST", "/fragmented", b"{}"),
        reply: Reply::Response {
            status: 200,
            headers: BTreeMap::new(),
            body: b"ok".to_vec(),
            delay: Duration::ZERO,
        },
    }])?;
    let mut stream = TcpStream::connect(fixture.address())?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let started = Instant::now();
    while fixture.request_count() == 0 {
        if started.elapsed() > Duration::from_secs(1) {
            return Err("Fixture did not accept the connection".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    stream.write_all(b"POST /fragmented HTTP/1.1\r\nHost: fixture\r\nContent-Len")?;
    std::thread::sleep(Duration::from_millis(15));
    stream.write_all(b"gth: 2\r\n\r\n{")?;
    std::thread::sleep(Duration::from_millis(15));
    stream.write_all(b"}")?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    assert!(response.ends_with(b"\r\n\r\nok"));
    assert_eq!(fixture.verify()?.len(), 1);
    Ok(())
}

#[test]
fn proves_zero_requests_and_rejects_missing_or_unexpected_requests()
-> Result<(), Box<dyn std::error::Error>> {
    let empty = HttpRecorder::start(Vec::new())?;
    assert_eq!(empty.request_count(), 0);
    assert!(empty.verify()?.is_empty());
    let missing = HttpRecorder::start(vec![Exchange {
        request: request("GET", "/expected", &[]),
        reply: Reply::DisconnectAfterRead,
    }])?;
    assert!(missing.verify().is_err());
    let unexpected = HttpRecorder::start(Vec::new())?;
    assert!(
        String::from_utf8(send(unexpected.address(), "GET", "/unexpected", &[])?)?.contains("500")
    );
    assert!(unexpected.verify().is_err());
    Ok(())
}

#[test]
fn shutdown_does_not_claim_a_delayed_response_was_delivered()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = HttpRecorder::start(vec![Exchange {
        request: request("GET", "/delayed", &[]),
        reply: Reply::Response {
            status: 200,
            headers: BTreeMap::new(),
            body: b"{}".to_vec(),
            delay: Duration::from_secs(2),
        },
    }])?;
    let mut stream = TcpStream::connect(fixture.address())?;
    stream.write_all(b"GET /delayed HTTP/1.1\r\nHost: fixture\r\n\r\n")?;
    let started = Instant::now();
    while fixture.request_count() == 0 {
        if started.elapsed() > Duration::from_secs(1) {
            return Err("Fixture did not accept request".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(fixture.verify().is_err());
    Ok(())
}

fn intent() -> OperationBinding {
    OperationBinding {
        operation_id: "create-record".into(),
        request_digest: content_digest(b"create"),
        descriptor_digest: content_digest(b"native"),
        target: ResourceRef {
            provider: "sample".into(),
            deployment: "cloud".into(),
            tenant: "tenant-one".into(),
            resource_type: "record".into(),
            id: "record-one".into(),
        },
        grant: GrantRef {
            id: "grant-one".into(),
        },
        actor: "actor-one".into(),
    }
}

fn persist_send_marker(journal: &dyn OperationJournal) -> Result<JournalRecord, StoreError> {
    let initial = journal.begin(&intent())?;
    if !initial.steps.is_empty() {
        return Err(StoreError::Conflict);
    }
    let mut prepared = initial.clone();
    prepared.revision += 1;
    prepared.steps.insert("create".into(), Default::default());
    prepared
        .steps
        .get_mut("create")
        .ok_or(StoreError::Corrupt)?
        .request_digest = Some(content_digest(b"native-create"));
    journal.compare_and_swap(initial.revision, &prepared)?;
    let mut sending = prepared.clone();
    sending.revision += 1;
    sending
        .steps
        .get_mut("create")
        .ok_or(StoreError::Corrupt)?
        .state = SubmissionState::SendStarted;
    journal.compare_and_swap(prepared.revision, &sending)?;
    Ok(sending)
}

#[test]
fn journal_faults_gate_a_real_send_and_preserve_uncertainty_after_disconnect()
-> Result<(), Box<dyn std::error::Error>> {
    let faults = Faults::default();
    let journal = MemoryJournal::new(faults.clone());
    let untouched = HttpRecorder::start(Vec::new())?;
    faults.fail_on(FaultPoint::JournalBeginBeforeCommit, 1)?;
    if persist_send_marker(&journal).is_ok() {
        let _ = send(untouched.address(), "POST", "/records", b"{}");
    }
    assert_eq!(untouched.request_count(), 0);
    assert!(untouched.verify()?.is_empty());
    let remote = HttpRecorder::start(vec![Exchange {
        request: request("POST", "/records", b"{}"),
        reply: Reply::DisconnectAfterRead,
    }])?;
    let sending = persist_send_marker(&journal)?;
    assert!(send(remote.address(), "POST", "/records", b"{}")?.is_empty());
    faults.fail_on(FaultPoint::JournalUpdateBeforeCommit, 1)?;
    let mut unknown = sending.clone();
    unknown.revision += 1;
    unknown
        .steps
        .get_mut("create")
        .ok_or("Missing send step")?
        .state = SubmissionState::Unknown;
    assert_eq!(
        journal.compare_and_swap(sending.revision, &unknown),
        Err(StoreError::Unavailable)
    );
    let reopened = journal.clone();
    let persisted = reopened
        .get("create-record")?
        .ok_or("Missing durable send marker")?;
    assert!(persisted.steps["create"].state.requires_reconciliation());
    assert_eq!(persist_send_marker(&reopened), Err(StoreError::Conflict));
    assert_eq!(remote.request_count(), 1);
    assert_eq!(remote.verify()?.len(), 1);
    Ok(())
}

struct EchoInput;

impl craft_core::execution::OperationHandler for EchoInput {
    fn validate(
        &self,
        request: &craft_core::contracts::CanonicalRequest,
    ) -> Result<(), craft_core::error::CraftError> {
        if request.input["id"].as_str() != Some(request.target.id.as_str()) {
            return Err(craft_core::error::CraftError::field(
                "/id",
                "target_identity_mismatch",
            ));
        }
        Ok(())
    }
    fn execute(
        &self,
        operation: &craft_core::execution::PreparedOperation,
        context: &mut dyn craft_core::execution::OperationContext,
    ) -> Result<serde_json::Value, craft_core::error::CraftError> {
        Ok(context
            .invoke(&craft_core::execution::NativeCall {
                repeated_query: std::collections::BTreeMap::new(),
                route: "create".into(),
                path_parameters: BTreeMap::new(),
                query: BTreeMap::new(),
                body: Some(operation.request().input.clone()),
            })?
            .body)
    }
}

struct LoopbackContext(SocketAddr);

impl craft_core::execution::OperationContext for LoopbackContext {
    fn checkpoint(&self) -> Result<(), craft_core::error::CraftError> {
        Ok(())
    }
    fn invoke(
        &mut self,
        call: &craft_core::execution::NativeCall,
    ) -> Result<craft_core::execution::NativeResponse, craft_core::error::CraftError> {
        use craft_core::error::{CraftError, ErrorCode};
        let bytes = serde_json::to_vec(&call.body)
            .map_err(|_| CraftError::new(ErrorCode::Internal, "Cannot encode fixture input."))?;
        let response = send(self.0, "POST", "/records", &bytes).map_err(|_| {
            CraftError::new(
                ErrorCode::WriteOutcomeUnknown,
                "Fixture response was unavailable.",
            )
        })?;
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| {
                CraftError::new(
                    ErrorCode::WriteOutcomeUnknown,
                    "Fixture response was incomplete.",
                )
            })?;
        let body = serde_json::from_slice(&response[split + 4..]).map_err(|_| {
            CraftError::new(
                ErrorCode::WriteOutcomeUnknown,
                "Fixture response was invalid.",
            )
        })?;
        Ok(craft_core::execution::NativeResponse {
            retry_after: None,
            link_headers: None,
            status: 200,
            body,
            request_id: None,
        })
    }
}

#[test]
fn malformed_requests_make_zero_calls_before_a_valid_unicode_request_reaches_the_host()
-> Result<(), Box<dyn std::error::Error>> {
    use craft_core::contracts::{
        CanonicalRequest, EffectClass, Envelope, SCHEMA_VERSION, Selection,
    };
    use craft_core::execution::{HttpMethod, NativeRoute, RouteSegment};
    use craft_core::registry::{
        OperationDescriptor, ReconciliationClass, Registry, RetryClass, response_schema,
    };
    use serde_json::{Value, json};
    let valid = json!({"id":"record-one","notes":"مرحبا 🦀\nlet total = 4;"});
    let bytes = serde_json::to_vec(&valid)?;
    let remote = HttpRecorder::start(vec![Exchange {
        request: request("POST", "/records", &bytes),
        reply: Reply::Response {
            status: 200,
            headers: BTreeMap::new(),
            body: bytes.clone(),
            delay: Duration::ZERO,
        },
    }])?;
    let mut registry = Registry::default();
    registry.register_handler(OperationDescriptor {
        id: "sample.create".into(), provider: "sample".into(), contract_version: SCHEMA_VERSION.into(), api_revision: "native-v1".into(), description: "Echo a fixture record.".into(), effect: EffectClass::Write,
        input_schema: json!({"type":"object","properties":{"id":{"type":"string"},"notes":{"type":"string"}},"required":["id","notes"],"additionalProperties":false}),
        response_schema: response_schema::<Value>()?, example: valid.clone(), response_example: serde_json::to_value(Envelope::success("sample.create", "example".into(), valid.clone()))?, projection: None,
        scope_alternatives: Vec::new(), permissions: Vec::new(), tenant_conditions: Vec::new(),
        routes: vec![NativeRoute {
repeated_query_parameters: Vec::new(), id: "create".into(), method: HttpMethod::Post, path: vec![RouteSegment::Literal("records".into())], query_parameters: Vec::new(), continuation_parameters: Vec::new(), response_max_bytes: 4096 }],
        retry_class: RetryClass::Never, reconciliation: ReconciliationClass::NotApplicable,
    }, std::sync::Arc::new(EchoInput))?;
    let mut host = LoopbackContext(remote.address());
    let invoke = |bytes: &[u8], host: &mut LoopbackContext| {
        let input = craft_core::input::parse_object(bytes)?;
        let operation = registry.prepare(CanonicalRequest {
            command: "sample.create".into(),
            target: intent().target,
            input,
            selection: Selection::default(),
            preconditions: None,
        })?;
        registry.execute(&operation, host)
    };
    for malformed in [
        br#"{"id":true,"notes":"private-sentinel"}"#.as_slice(),
        br#"{"id":"record-one","notes":"private-sentinel","extra":1}"#.as_slice(),
        br#"{"id":"record-one","notes":"private-sentinel","notes":"other"}"#.as_slice(),
        br#"{"id":"record-one","notes":{"value":false}}"#.as_slice(),
        b"{} {}".as_slice(),
    ] {
        let error = invoke(malformed, &mut host)
            .err()
            .ok_or("Expected local rejection")?;
        assert_eq!(
            error.details.write_state,
            craft_core::contracts::RequestState::NotSent
        );
        assert!(!serde_json::to_string(&error)?.contains("private-sentinel"));
    }
    assert_eq!(remote.request_count(), 0);
    assert_eq!(invoke(&bytes, &mut host)?, valid);
    assert_eq!(remote.request_count(), 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&remote.verify()?[0].body)?["notes"],
        valid["notes"]
    );
    Ok(())
}
