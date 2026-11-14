use super::*;
use crate::artifacts::NativeArtifacts;
use crate::transport::MultipartFile;
use craft_core::storage::{ArtifactSpec, ArtifactStore};
use std::io::{Read, Write};
type TestResult = Result<(), Box<dyn std::error::Error>>;
const FILENAME: &str = "設計 \"draft\".bin";
fn upload() -> Result<(Registry, PreparedOperation), CraftError> {
    upload_snapshot("snapshot.bin", b"abc", FILENAME)
}
fn upload_snapshot(
    name: &str,
    bytes: &[u8],
    filename: &str,
) -> Result<(Registry, PreparedOperation), CraftError> {
    let (prototype, _) = prepared()?;
    let mut descriptor = prototype.descriptor("jira.fixture.mutate")?.clone();
    descriptor.id = "jira.attachment.upload".into();
    descriptor.input_schema = json!({"type":"object"});
    descriptor.response_example["command"] = json!(descriptor.id);
    descriptor.routes = vec![NativeRoute {
        id: "upload".into(),
        method: HttpMethod::Post,
        path: ["rest", "api", "3", "issue"]
            .into_iter()
            .map(|v| RouteSegment::Literal(v.into()))
            .chain([
                RouteSegment::Parameter("issueIdOrKey".into()),
                RouteSegment::Literal("attachments".into()),
            ])
            .collect(),
        query_parameters: vec![],
        continuation_parameters: vec![],
        repeated_query_parameters: vec![],
        response_max_bytes: 512,
    }];
    let mut registry = Registry::default();
    registry.register_typed_handler::<Value>(descriptor, Arc::new(Handler))?;
    let operation = registry.prepare(CanonicalRequest {
        command: "jira.attachment.upload".into(),
        target: target(),
        input: json!({"issue_id":"PAY-1","artifact_name":name,"digest":craft_core::input::content_digest(bytes),"size_bytes":bytes.len(),"filename":filename,"media_type":"application/octet-stream"}),
        selection: Selection::default(),
        preconditions: None,
    })?;
    Ok((registry, operation))
}
fn upload_call() -> NativeCall {
    NativeCall {
        route: "upload".into(),
        path_parameters: BTreeMap::from([("issueIdOrKey".into(), "PAY-1".into())]),
        query: BTreeMap::new(),
        repeated_query: BTreeMap::new(),
        body: None,
    }
}
fn file(fixture: &Fixture, bytes: &[u8]) -> Result<MultipartFile, Box<dyn std::error::Error>> {
    let artifacts =
        NativeArtifacts::open(&fixture.root.join("uploads"), 1_000_000, &control(60_000)?)?;
    let receipt = if let Some(receipt) = artifacts.receipt("snapshot.bin")? {
        receipt
    } else {
        let mut writer = artifacts.begin(ArtifactSpec {
            name: "snapshot.bin".into(),
            media_type: "application/octet-stream".into(),
            max_bytes: 1_000_000,
            source: Some(target()),
        })?;
        writer.write_all(bytes)?;
        writer.finish()?
    };
    Ok(MultipartFile {
        input: artifacts.open_verified_stream("snapshot.bin", &receipt.digest)?,
        filename: "設計 \"draft\".bin".into(),
        media_type: "application/octet-stream".into(),
    })
}
fn request<'a>(
    server: &TlsServer,
    registry: &'a Registry,
    operation: &'a PreparedOperation,
    grant: &GrantRef,
    control: InvocationControl,
    file: MultipartFile,
) -> Result<PreparedHttpMutation<'a>, Box<dyn std::error::Error>> {
    let mut context = HttpContext::bound(
        registry,
        operation,
        grant,
        session(grant)?,
        TlsPolicy::AdditionalCaBundle(server.root()),
        control,
    )?;
    context
        .endpoint
        .prefix
        .set_host(Some("localhost"))
        .map_err(|_| "host")?;
    context
        .endpoint
        .prefix
        .set_port(Some(server.address().port()))
        .map_err(|_| "port")?;
    Ok(PreparedHttpMutation::freeze_upload(
        registry,
        operation,
        context,
        &upload_call(),
        file,
    )?)
}
#[test]
fn multipart_upload_sends_exact_binary_and_fixed_headers_under_one_stable_claim() -> TestResult {
    let fixture = Fixture::new()?;
    let (_profiles, grant, journal) = fixture.setup()?;
    let bytes = (0..160_013).map(|n| (n % 256) as u8).collect::<Vec<_>>();
    let server = TlsServer::start(Reply::json(200, br#"[{"id":"10001"}]"#, &[]))?;
    let (registry, operation) = upload_snapshot("snapshot.bin", &bytes, FILENAME)?;
    let first = request(
        &server,
        &registry,
        &operation,
        &grant,
        control(60_000)?,
        file(&fixture, &bytes)?,
    )?;
    let second = request(
        &server,
        &registry,
        &operation,
        &grant,
        control(60_000)?,
        file(&fixture, &bytes)?,
    )?;
    assert_eq!(first.call.body, second.call.body);
    let manifest = first.call.body.clone().ok_or("manifest")?;
    let attempt = first.send(&journal, &authority(&grant), "upload")?;
    assert_eq!(
        attempt.response.as_ref().map_err(|_| "response")?.status,
        200
    );
    assert!(attempt.incomplete_upload_response.is_none());
    assert_eq!(
        attempt
            .pending_evidence
            .as_ref()
            .map_err(|_| "pending")?
            .steps["upload"]
            .state,
        SubmissionState::Unknown
    );
    assert!(second.send(&journal, &authority(&grant), "upload").is_err());
    let observed = server.finish()?;
    assert_eq!(observed.len(), 1);
    let wire = &observed[0];
    assert_eq!(wire.method, "POST");
    assert!(wire.target.ends_with("/rest/api/3/issue/PAY-1/attachments"));
    assert_eq!(
        wire.headers.get("x-atlassian-token").map(String::as_str),
        Some("no-check")
    );
    assert_eq!(
        wire.headers.get("authorization").map(String::as_str),
        Some("Bearer synthetic-bearer-sentinel")
    );
    assert!(!wire.headers.contains_key("transfer-encoding"));
    assert_eq!(
        wire.headers
            .get("content-length")
            .ok_or("length")?
            .parse::<usize>()?,
        wire.body.len()
    );
    let boundary = manifest["boundary"].as_str().ok_or("boundary")?;
    assert_eq!(boundary.len(), 70);
    assert_eq!(
        wire.headers.get("content-type").ok_or("type")?,
        &format!("multipart/form-data; boundary={boundary}")
    );
    let offset = wire
        .body
        .windows(4)
        .position(|b| b == b"\r\n\r\n")
        .ok_or("prefix")?
        + 4;
    assert!(
        std::str::from_utf8(&wire.body[..offset])?.contains("filename=\"設計 \\\"draft\\\".bin\"")
    );
    assert_eq!(&wire.body[offset..offset + bytes.len()], bytes.as_slice());
    assert_eq!(
        &wire.body[offset + bytes.len()..],
        format!("\r\n--{boundary}--\r\n").as_bytes()
    );
    assert_eq!(
        manifest["artifact"]["digest"],
        craft_core::input::content_digest(&bytes)
    );
    assert!(manifest["artifact"].get("local_path").is_none());
    Ok(())
}
#[test]
fn multipart_metadata_unconsumed_source_and_json_bypass_are_checked_before_claim() -> TestResult {
    for mode in [
        "filename",
        "path",
        "media",
        "consumed",
        "wrong_command",
        "json",
    ] {
        let fixture = Fixture::new()?;
        let (_profiles, grant, journal) = fixture.setup()?;
        let server = TlsServer::start(Reply::Sequence(vec![]))?;
        let (registry, operation) = if mode == "wrong_command" {
            prepared()?
        } else {
            upload()?
        };
        let mut input = file(&fixture, b"abc")?;
        match mode {
            "filename" => input.filename = "bad\r\nInjected: true".into(),
            "path" => input.filename = "../source".into(),
            "media" => input.media_type = "text/plain; charset=utf-8".into(),
            "consumed" => {
                let mut one = [0];
                input.input.read_exact(&mut one)?;
            }
            _ => {}
        }
        let refused = if mode == "json" {
            let mut context = HttpContext::bound(
                &registry,
                &operation,
                &grant,
                session(&grant)?,
                TlsPolicy::AdditionalCaBundle(server.root()),
                control(60_000)?,
            )?;
            context
                .endpoint
                .prefix
                .set_host(Some("localhost"))
                .map_err(|_| "host")?;
            PreparedHttpMutation::freeze(&registry, &operation, context, &upload_call()).is_err()
        } else {
            request(
                &server,
                &registry,
                &operation,
                &grant,
                control(60_000)?,
                input,
            )
            .is_err()
        };
        assert!(refused, "{mode}");
        assert!(journal.get("publish-summary")?.is_none());
        assert_eq!(server.connections(), 0);
        assert!(server.finish()?.is_empty());
    }
    Ok(())
}
#[test]
fn multipart_failure_statuses_disconnects_and_redirects_never_retry_or_auto_confirm() -> TestResult
{
    for mode in ["disconnect", "redirect", "oversize", "denied", "rate"] {
        let fixture = Fixture::new()?;
        let (_profiles, grant, journal) = fixture.setup()?;
        let response = match mode {
            "disconnect" => Reply::Disconnect,
            "redirect" => Reply::json(
                307,
                b"",
                &[("Location", "https://untrusted.invalid/private")],
            ),
            "oversize" => Reply::json(200, &vec![b' '; 600], &[]),
            "denied" => Reply::json(403, b"private-native", &[]),
            _ => Reply::json(429, b"private-native", &[("Retry-After", "7")]),
        };
        let server = TlsServer::start(response)?;
        let (registry, operation) = upload()?;
        let attempt = request(
            &server,
            &registry,
            &operation,
            &grant,
            control(60_000)?,
            file(&fixture, b"abc")?,
        )?
        .send(&journal, &authority(&grant), "upload")?;
        if matches!(mode, "denied" | "rate") {
            let response = attempt.response.as_ref().map_err(|_| "native status")?;
            assert_eq!(response.status, if mode == "denied" { 403 } else { 429 });
            assert!(response.body.is_null());
            if mode == "rate" {
                assert_eq!(
                    response.retry_after,
                    Some(craft_core::execution::RetryAfter::DelaySeconds(7))
                );
            }
        } else {
            assert!(attempt.response.is_err(), "{mode}");
            assert_eq!(
                attempt
                    .response
                    .as_ref()
                    .err()
                    .ok_or("error")?
                    .details
                    .write_state,
                RequestState::SentUnknown
            );
        }
        assert_eq!(
            journal.get("publish-summary")?.ok_or("record")?.steps["upload"].state,
            SubmissionState::Unknown
        );
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}
#[test]
fn multipart_guard_and_cancellation_after_claim_preserve_code_and_unknown_without_send()
-> TestResult {
    for mode in ["guard", "cancel"] {
        let fixture = Fixture::new()?;
        let (_profiles, grant, journal) = fixture.setup()?;
        let server = TlsServer::start(Reply::Sequence(vec![]))?;
        let (registry, operation) = upload()?;
        let token = CancellationToken::default();
        let invocation =
            InvocationControl::new(Arc::new(SystemClock::default()), token.clone(), 60_000)?;
        let upload = request(
            &server,
            &registry,
            &operation,
            &grant,
            invocation,
            file(&fixture, b"abc")?,
        )?;
        let attempt = upload.send_checked(&journal, &authority(&grant), "upload", || {
            if mode == "cancel" {
                token.cancel();
                Ok(())
            } else {
                Err(denied())
            }
        })?;
        assert_eq!(
            attempt.response.as_ref().err().ok_or("error")?.code,
            if mode == "cancel" {
                ErrorCode::Cancelled
            } else {
                ErrorCode::PolicyDenied
            }
        );
        assert_eq!(
            attempt
                .response
                .as_ref()
                .err()
                .ok_or("error")?
                .details
                .write_state,
            RequestState::SentUnknown
        );
        assert_eq!(
            journal.get("publish-summary")?.ok_or("journal")?.steps["upload"].state,
            SubmissionState::Unknown
        );
        assert_eq!(server.connections(), 0);
        assert!(server.finish()?.is_empty());
    }
    Ok(())
}
#[test]
fn multipart_guard_is_rechecked_during_the_exchange_without_replaying_upload() -> TestResult {
    let fixture = Fixture::new()?;
    let (_profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::Stall)?;
    let (registry, operation) = upload()?;
    let attempt = request(
        &server,
        &registry,
        &operation,
        &grant,
        control(60_000)?,
        file(&fixture, b"abc")?,
    )?
    .send_guarded(&journal, &authority(&grant), "upload", || {
        if server.request_count() > 0 {
            Err(denied())
        } else {
            Ok(())
        }
    })?;
    assert_eq!(
        attempt.response.as_ref().err().ok_or("error")?.code,
        ErrorCode::PolicyDenied
    );
    assert_eq!(
        journal.get("publish-summary")?.ok_or("journal")?.steps["upload"].state,
        SubmissionState::Unknown
    );
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[test]
fn incomplete_multipart_success_keeps_native_acknowledgment_without_qualifying_it() -> TestResult {
    let fixture = Fixture::new()?;
    let (_profiles, grant, _journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::Sequence(vec![]))?;
    let (registry, operation) = upload()?;
    let request = request(
        &server,
        &registry,
        &operation,
        &grant,
        control(60_000)?,
        file(&fixture, b"abc")?,
    )?;
    let state = request.upload.as_ref().ok_or("state")?;
    let response = NativeResponse {
        status: 200,
        body: json!([{"id":"10001"}]),
        request_id: Some("trace-one".into()),
        retry_after: None,
        link_headers: Some(vec![]),
    };
    let (response, retained) = qualify_upload_response(&request.context, state, Ok(response));
    let error = response.err().ok_or("expected unqualified source")?;
    assert_eq!(error.code, ErrorCode::WriteOutcomeUnknown);
    assert_eq!(error.details.write_state, RequestState::SentUnknown);
    assert_eq!(
        retained.ok_or("retained response")?.body,
        json!([{"id":"10001"}])
    );
    let cancelled = request.context.after_attempt(CraftError::new(
        ErrorCode::Cancelled,
        "Cancelled after claim.",
    ));
    let (response, retained) = qualify_upload_response(&request.context, state, Err(cancelled));
    assert_eq!(
        response.err().ok_or("cancelled")?.code,
        ErrorCode::Cancelled
    );
    assert!(retained.is_none());
    assert!(server.finish()?.is_empty());
    Ok(())
}
#[test]
fn multipart_body_has_exact_bounded_frames_and_verified_end_of_stream() -> TestResult {
    use http_body::Body;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};
    let fixture = Fixture::new()?;
    let (_profiles, grant, _journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::Sequence(vec![]))?;
    let bytes = vec![0xff; 160_013];
    let (registry, operation) = upload_snapshot("snapshot.bin", &bytes, FILENAME)?;
    let mut request = request(
        &server,
        &registry,
        &operation,
        &grant,
        control(60_000)?,
        file(&fixture, &bytes)?,
    )?;
    let expected = request
        .request
        .headers()
        .get("content-length")
        .ok_or("length")?
        .to_str()?
        .parse::<u64>()?;
    let state = request.upload.clone().ok_or("state")?;
    assert!(state.failure(true).is_some());
    let body = request.request.body_mut().as_mut().ok_or("body")?;
    let mut context = Context::from_waker(Waker::noop());
    let mut total = 0;
    loop {
        match Pin::new(&mut *body).poll_frame(&mut context) {
            Poll::Ready(Some(Ok(frame))) => {
                let data = frame.into_data().map_err(|_| "data")?;
                assert!(data.len() <= 65_536);
                total += data.len() as u64;
            }
            Poll::Ready(None) => break,
            Poll::Ready(Some(Err(error))) => return Err(error.into()),
            Poll::Pending => return Err("Local file body unexpectedly pending".into()),
        }
    }
    assert_eq!(total, expected);
    assert!(state.failure(true).is_none());
    assert!(body.is_end_stream());
    assert!(server.finish()?.is_empty());
    Ok(())
}

#[test]
fn empty_multipart_snapshot_has_verified_framing_without_a_file_data_frame() -> TestResult {
    let fixture = Fixture::new()?;
    let (_profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::json(200, b"[]", &[]))?;
    let (registry, operation) = upload_snapshot("snapshot.bin", b"", FILENAME)?;
    let request = request(
        &server,
        &registry,
        &operation,
        &grant,
        control(60_000)?,
        file(&fixture, b"")?,
    )?;
    let manifest = request.call.body.clone().ok_or("manifest")?;
    assert_eq!(manifest["artifact"]["bytes"], 0);
    let attempt = request.send(&journal, &authority(&grant), "upload")?;
    assert!(attempt.response.is_ok());
    assert!(attempt.incomplete_upload_response.is_none());
    let observed = server.finish()?;
    assert_eq!(observed.len(), 1);
    let body = &observed[0].body;
    let offset = body
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or("prefix")?
        + 4;
    assert_eq!(
        &body[offset..],
        format!(
            "\r\n--{}--\r\n",
            manifest["boundary"].as_str().ok_or("boundary")?
        )
        .as_bytes()
    );
    Ok(())
}
#[test]
fn multipart_guard_refusal_before_claim_creates_no_journal_and_sends_nothing() -> TestResult {
    let fixture = Fixture::new()?;
    let (_profiles, grant, journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::Sequence(vec![]))?;
    let (registry, operation) = upload()?;
    let refused = request(
        &server,
        &registry,
        &operation,
        &grant,
        control(60_000)?,
        file(&fixture, b"abc")?,
    )?
    .send_guarded(&journal, &authority(&grant), "upload", || Err(denied()));
    let error = refused.err().ok_or("refused")?;
    assert_eq!(error.code, ErrorCode::PolicyDenied);
    assert_eq!(error.details.write_state, RequestState::NotSent);
    assert!(journal.get("publish-summary")?.is_none());
    assert_eq!(server.connections(), 0);
    assert!(server.finish()?.is_empty());
    Ok(())
}

#[test]
fn multipart_source_failure_emits_no_closing_delimiter_or_complete_flag() -> TestResult {
    use http_body::Body;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};
    let fixture = Fixture::new()?;
    let (_profiles, grant, _journal) = fixture.setup()?;
    let server = TlsServer::start(Reply::Sequence(vec![]))?;
    let (registry, operation) = upload_snapshot("source", b"source", "source.bin")?;
    let token = CancellationToken::default();
    let source_control =
        InvocationControl::new(Arc::new(SystemClock::default()), token.clone(), 60_000)?;
    let artifacts = NativeArtifacts::open(&fixture.root.join("source"), 100, &source_control)?;
    let mut writer = artifacts.begin(ArtifactSpec {
        name: "source".into(),
        media_type: "application/octet-stream".into(),
        max_bytes: 100,
        source: None,
    })?;
    writer.write_all(b"source")?;
    let receipt = writer.finish()?;
    let input = MultipartFile {
        input: artifacts.open_verified_stream("source", &receipt.digest)?,
        filename: "source.bin".into(),
        media_type: "application/octet-stream".into(),
    };
    let mut request = request(
        &server,
        &registry,
        &operation,
        &grant,
        control(60_000)?,
        input,
    )?;
    let state = request.upload.clone().ok_or("state")?;
    let body = request.request.body_mut().as_mut().ok_or("body")?;
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(
        Pin::new(&mut *body).poll_frame(&mut context),
        Poll::Ready(Some(Ok(_)))
    ));
    token.cancel();
    assert!(matches!(
        Pin::new(&mut *body).poll_frame(&mut context),
        Poll::Ready(Some(Err(_)))
    ));
    assert!(matches!(
        Pin::new(&mut *body).poll_frame(&mut context),
        Poll::Ready(None)
    ));
    assert_eq!(
        state.failure(true).ok_or("incomplete source")?.code,
        ErrorCode::Cancelled
    );
    assert!(server.finish()?.is_empty());
    Ok(())
}

#[test]
fn multipart_rejects_snapshot_framing_or_issue_different_from_prepared_input_before_claim()
-> TestResult {
    for field in [
        "artifact_name",
        "digest",
        "size_bytes",
        "filename",
        "media_type",
        "issue_id",
    ] {
        let fixture = Fixture::new()?;
        let (_profiles, grant, journal) = fixture.setup()?;
        let server = TlsServer::start(Reply::Sequence(vec![]))?;
        let (registry, operation) = upload()?;
        let mut changed = operation.request().clone();
        changed.input[field] = match field {
            "digest" => json!(format!("sha256:{}", "b".repeat(64))),
            "size_bytes" => json!(4),
            "media_type" => json!("image/png"),
            _ => json!("different"),
        };
        let changed = registry.prepare(changed)?;
        assert!(
            request(
                &server,
                &registry,
                &changed,
                &grant,
                control(60_000)?,
                file(&fixture, b"abc")?
            )
            .is_err(),
            "{field}"
        );
        assert!(journal.get("publish-summary")?.is_none());
        assert_eq!(server.connections(), 0);
        assert!(server.finish()?.is_empty());
    }
    Ok(())
}
