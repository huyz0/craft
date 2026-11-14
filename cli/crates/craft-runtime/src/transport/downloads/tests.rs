use super::*;
use crate::tls_fixture::{Reply, TlsServer};
use crate::transport::tests as fixtures;
use craft_core::control::CancellationToken;
use craft_core::input::content_digest;
use craft_test_support::state::{FaultPoint, Faults, MemoryArtifacts};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn spec(operation: &PreparedOperation, maximum: u64) -> ArtifactSpec {
    ArtifactSpec {
        name: "download.bin".into(),
        media_type: "application/octet-stream".into(),
        max_bytes: maximum,
        source: Some(operation.request().target.clone()),
    }
}
fn prepared<'a>(
    registry: &'a Registry,
    operation: &'a PreparedOperation,
    server: &TlsServer,
    maximum: u64,
    expected: Option<&str>,
) -> Result<PreparedHttpDownload<'a>, Box<dyn std::error::Error>> {
    let context = fixtures::context(
        server,
        TlsPolicy::AdditionalCaBundle(server.root()),
        fixtures::control(3000)?,
    )?;
    Ok(PreparedHttpDownload::freeze(
        registry,
        operation,
        context,
        DownloadRequest {
            call: fixtures::call(),
            artifact: spec(operation, maximum),
            expected_digest: expected.map(str::to_owned),
        },
    )?)
}
fn raw(status: u16, body: &[u8], headers: &[(&str, String)]) -> Reply {
    let mut response = format!("HTTP/1.1 {status} Fixture\r\nConnection: close\r\n").into_bytes();
    for (name, value) in headers {
        response.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    response.extend_from_slice(b"\r\n");
    response.extend_from_slice(body);
    Reply::Raw(response)
}
fn complete(body: &[u8]) -> Reply {
    raw(
        200,
        body,
        &[
            ("Content-Length", body.len().to_string()),
            ("Content-Type", "Application/Octet-Stream".into()),
            ("atl-traceid", "binary-trace".into()),
        ],
    )
}
#[test]
fn complete_binary_entity_streams_beyond_json_budget_and_returns_exact_receipt() -> TestResult {
    let bytes = (0..160_013).map(|n| (n % 256) as u8).collect::<Vec<_>>();
    let server = TlsServer::start(complete(&bytes))?;
    let (registry, operation) = fixtures::prepared(EffectClass::Read)?;
    let store = MemoryArtifacts::new(Faults::default(), 200_000);
    let outcome = prepared(
        &registry,
        &operation,
        &server,
        200_000,
        Some(&content_digest(&bytes)),
    )?
    .receive(&store, || Ok(()));
    assert!(outcome.error.is_none(), "{:?}", outcome.error);
    assert_eq!(outcome.artifact_write_state, RequestState::Confirmed);
    assert_eq!(outcome.received_bytes, bytes.len() as u64);
    assert_eq!(
        outcome.response_media_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(outcome.provider_request_id.as_deref(), Some("binary-trace"));
    let artifact = store.read("download.bin", 200_000)?.ok_or("artifact")?;
    assert_eq!(artifact.bytes, bytes);
    assert_eq!(Some(artifact.receipt), outcome.artifact);
    let requests = server.finish()?;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].headers.get("accept").map(String::as_str),
        Some("*/*")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("accept-encoding")
            .map(String::as_str),
        Some("identity")
    );
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("Bearer synthetic-bearer-sentinel")
    );
    Ok(())
}
#[test]
fn chunked_and_empty_entities_publish_without_untrusted_filename_or_parameter_metadata()
-> TestResult {
    for (wire, expected) in [
        (
            raw(
                200,
                b"3\r\na\0b\r\n2\r\ncd\r\n0\r\n\r\n",
                &[
                    ("Transfer-Encoding", "chunked".into()),
                    ("Content-Type", "text/plain; private=hidden".into()),
                    (
                        "Content-Disposition",
                        "attachment; filename=../../secret".into(),
                    ),
                ],
            ),
            b"a\0bcd".as_slice(),
        ),
        (complete(b""), b"".as_slice()),
    ] {
        let server = TlsServer::start(wire)?;
        let (registry, operation) = fixtures::prepared(EffectClass::Read)?;
        let store = MemoryArtifacts::new(Faults::default(), 100);
        let outcome =
            prepared(&registry, &operation, &server, 100, None)?.receive(&store, || Ok(()));
        assert!(outcome.error.is_none(), "{:?}", outcome.error);
        assert_eq!(
            store.read("download.bin", 100)?.ok_or("artifact")?.bytes,
            expected
        );
        assert!(
            outcome
                .response_media_type
                .as_ref()
                .is_none_or(|s| !s.contains("private"))
        );
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}
#[test]
fn redirects_and_partial_or_error_statuses_never_publish_or_forward_credentials() -> TestResult {
    for status in [206, 303, 400, 401, 403, 404, 429, 500] {
        let destination = TlsServer::start(Reply::Sequence(vec![]))?;
        let server = TlsServer::start(raw(
            status,
            b"private-body",
            &[
                (
                    "Location",
                    format!("https://localhost:{}/secret", destination.address().port()),
                ),
                ("Content-Length", "12".into()),
                ("Retry-After", "7".into()),
            ],
        ))?;
        let (registry, operation) = fixtures::prepared(EffectClass::Read)?;
        let store = MemoryArtifacts::new(Faults::default(), 100);
        let outcome =
            prepared(&registry, &operation, &server, 100, None)?.receive(&store, || Ok(()));
        assert!(outcome.error.is_some());
        assert_eq!(outcome.http_status, Some(status));
        assert!(outcome.artifact.is_none() && store.receipt("download.bin")?.is_none());
        assert_eq!(outcome.artifact_write_state, RequestState::NotSent);
        assert_eq!(server.finish()?.len(), 1);
        assert_eq!(destination.finish()?.len(), 0);
    }
    Ok(())
}
#[test]
fn unsupported_or_repeated_transfer_codings_never_publish_entity_bytes() -> TestResult {
    for headers in [
        vec![("Transfer-Encoding", "gzip, chunked".into())],
        vec![("Transfer-Encoding", "gzip".into())],
        vec![("Transfer-Encoding", "chunked, chunked".into())],
        vec![
            ("Transfer-Encoding", "gzip".into()),
            ("Transfer-Encoding", "chunked".into()),
        ],
    ] {
        let server = TlsServer::start(raw(200, b"3\r\nabc\r\n0\r\n\r\n", &headers))?;
        let (registry, operation) = fixtures::prepared(EffectClass::Read)?;
        let faults = Faults::default();
        let store = MemoryArtifacts::new(faults.clone(), 100);
        let outcome =
            prepared(&registry, &operation, &server, 100, None)?.receive(&store, || Ok(()));
        assert!(outcome.error.is_some());
        assert!(outcome.artifact.is_none() && store.receipt("download.bin")?.is_none());
        assert_eq!(outcome.artifact_write_state, RequestState::NotSent);
        assert_eq!(faults.calls(FaultPoint::ArtifactBeforeCommit)?, 0);
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}
#[test]
fn malformed_encoded_truncated_oversized_or_wrong_digest_entities_never_finish_staging()
-> TestResult {
    let cases=[
        (raw(200,b"abc",&[("Content-Length","101".into())]),None),
        (raw(200,b"abc",&[("Content-Length","5".into())]),None),
        (raw(200,b"abc",&[("Content-Length","3".into()),("Content-Encoding","gzip".into())]),None),
        (raw(200,b"abc",&[("Content-Length","3".into()),("Content-Range","bytes 0-2/20".into())]),None),
        (raw(200,b"abc",&[("Content-Length","3".into()),("Content-Type","not-a-media-type".into())]),None),
        (raw(200,b"65\r\nxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\r\n0\r\n\r\n",&[("Transfer-Encoding","chunked".into())]),None),
        (complete(b"abc"),Some(format!("sha256:{}","0".repeat(64)))),
    ];
    for (wire, expected) in cases {
        let server = TlsServer::start(wire)?;
        let (registry, operation) = fixtures::prepared(EffectClass::Read)?;
        let faults = Faults::default();
        let store = MemoryArtifacts::new(faults.clone(), 100);
        let outcome = prepared(&registry, &operation, &server, 100, expected.as_deref())?
            .receive(&store, || Ok(()));
        assert!(outcome.error.is_some());
        assert!(outcome.artifact.is_none() && store.receipt("download.bin")?.is_none());
        assert_eq!(outcome.artifact_write_state, RequestState::NotSent);
        assert_eq!(faults.calls(FaultPoint::ArtifactBeforeCommit)?, 0);
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}
fn policy_case(mode: &str) -> TestResult {
    let server = TlsServer::start(if matches!(mode, "cancel" | "deadline") {
        Reply::Stall
    } else {
        complete(b"abc")
    })?;
    let (registry, operation) = fixtures::prepared(EffectClass::Read)?;
    let store = MemoryArtifacts::new(Faults::default(), 100);
    let token = CancellationToken::default();
    let clock = std::sync::Arc::new(craft_test_support::state::ManualClock::new(1000));
    let control = InvocationControl::new(clock.clone(), token.clone(), 3000)?;
    let context = fixtures::context(
        &server,
        TlsPolicy::AdditionalCaBundle(server.root()),
        control,
    )?;
    let download = PreparedHttpDownload::freeze(
        &registry,
        &operation,
        context,
        DownloadRequest {
            call: fixtures::call(),
            artifact: spec(&operation, 100),
            expected_digest: None,
        },
    )?;
    let outcome = download.receive(&store, || {
        if mode == "before" || mode == "after_headers" && server.request_count() > 0 {
            return Err(denied());
        }
        if server.request_count() > 0 {
            if mode == "cancel" {
                token.cancel();
            }
            if mode == "deadline" {
                clock
                    .advance(Duration::from_secs(4))
                    .map_err(|_| internal())?;
            }
        }
        Ok(())
    });
    let error = outcome.error.ok_or("Expected policy/control failure")?;
    assert_eq!(
        error.code,
        match mode {
            "cancel" => ErrorCode::Cancelled,
            "deadline" => ErrorCode::DeadlineExceeded,
            _ => ErrorCode::PolicyDenied,
        },
        "{mode}"
    );
    assert!(store.receipt("download.bin")?.is_none());
    assert_eq!(outcome.artifact_write_state, RequestState::NotSent);
    assert_eq!(
        server.finish()?.len(),
        usize::from(mode != "before"),
        "{mode}"
    );
    Ok(())
}
#[test]
fn policy_refusal_precedes_binary_get() -> TestResult {
    policy_case("before")
}
#[test]
fn policy_revocation_after_get_discards_binary_staging() -> TestResult {
    policy_case("after_headers")
}
#[test]
fn cancelled_inflight_binary_stream_does_not_publish_or_retry() -> TestResult {
    policy_case("cancel")
}
#[test]
fn expired_inflight_binary_stream_does_not_publish_or_retry() -> TestResult {
    policy_case("deadline")
}
#[test]
fn artifact_storage_failures_preserve_after_commit_uncertainty_without_refetch() -> TestResult {
    for fault in [
        FaultPoint::ArtifactBegin,
        FaultPoint::ArtifactWrite,
        FaultPoint::ArtifactBeforeCommit,
        FaultPoint::ArtifactAfterCommit,
    ] {
        let server = TlsServer::start(if fault == FaultPoint::ArtifactBegin {
            Reply::Sequence(vec![])
        } else {
            complete(b"abc")
        })?;
        let (registry, operation) = fixtures::prepared(EffectClass::Read)?;
        let faults = Faults::default();
        faults.fail_on(fault, 1)?;
        let store = MemoryArtifacts::new(faults, 100);
        let outcome =
            prepared(&registry, &operation, &server, 100, None)?.receive(&store, || Ok(()));
        assert!(outcome.error.is_some());
        assert!(outcome.artifact.is_none());
        assert_eq!(
            store.receipt("download.bin")?.is_some(),
            fault == FaultPoint::ArtifactAfterCommit
        );
        assert_eq!(
            outcome.artifact_write_state,
            if fault == FaultPoint::ArtifactAfterCommit {
                RequestState::SentUnknown
            } else if fault == FaultPoint::ArtifactBeforeCommit {
                RequestState::Failed
            } else {
                RequestState::NotSent
            }
        );
        assert_eq!(
            server.finish()?.len(),
            usize::from(fault != FaultPoint::ArtifactBegin)
        );
    }
    Ok(())
}
struct CancellingStore {
    inner: MemoryArtifacts,
    token: CancellationToken,
}
struct CancellingWriter {
    inner: Box<dyn ArtifactWriter>,
    token: CancellationToken,
}
impl std::io::Write for CancellingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
impl ArtifactWriter for CancellingWriter {
    fn finish(self: Box<Self>) -> Result<ArtifactReceipt, StoreError> {
        let receipt = self.inner.finish()?;
        self.token.cancel();
        Ok(receipt)
    }
}
impl ArtifactStore for CancellingStore {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError> {
        Ok(Box::new(CancellingWriter {
            inner: self.inner.begin(spec)?,
            token: self.token.clone(),
        }))
    }
    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError> {
        self.inner.receipt(name)
    }
    fn read(
        &self,
        name: &str,
        max: u64,
    ) -> Result<Option<craft_core::storage::ArtifactBlob>, StoreError> {
        self.inner.read(name, max)
    }
}
#[test]
fn confirmed_publication_survives_late_cancellation_and_existing_names_prevent_get() -> TestResult {
    let server = TlsServer::start(complete(b"abc"))?;
    let (registry, operation) = fixtures::prepared(EffectClass::Read)?;
    let token = CancellationToken::default();
    let store = CancellingStore {
        inner: MemoryArtifacts::new(Faults::default(), 100),
        token: token.clone(),
    };
    let context = fixtures::context(
        &server,
        TlsPolicy::AdditionalCaBundle(server.root()),
        InvocationControl::new(
            std::sync::Arc::new(crate::control::SystemClock::default()),
            token.clone(),
            3000,
        )?,
    )?;
    let download = PreparedHttpDownload::freeze(
        &registry,
        &operation,
        context,
        DownloadRequest {
            call: fixtures::call(),
            artifact: spec(&operation, 100),
            expected_digest: None,
        },
    )?;
    let outcome = download.receive(&store, || Ok(()));
    assert!(outcome.error.is_none() && outcome.artifact.is_some());
    assert_eq!(outcome.artifact_write_state, RequestState::Confirmed);
    assert!(token.is_cancelled());
    assert_eq!(server.finish()?.len(), 1);
    let server = TlsServer::start(Reply::Sequence(vec![]))?;
    let outcome =
        prepared(&registry, &operation, &server, 100, None)?.receive(&store.inner, || Ok(()));
    assert_eq!(
        outcome.error.map(|e| e.code),
        Some(ErrorCode::VersionConflict)
    );
    assert_eq!(server.finish()?.len(), 0);
    Ok(())
}
#[test]
fn bad_download_specs_and_undeclared_routes_are_local_refusals() -> TestResult {
    let server = TlsServer::start(Reply::Sequence(vec![]))?;
    let (registry, operation) = fixtures::prepared(EffectClass::Read)?;
    for mode in ["oversize", "digest", "source", "name", "route", "body"] {
        let mut request = DownloadRequest {
            call: fixtures::call(),
            artifact: spec(&operation, 100),
            expected_digest: None,
        };
        match mode {
            "oversize" => request.artifact.max_bytes = MAX_BYTES + 1,
            "digest" => request.expected_digest = Some("bad".into()),
            "source" => request.artifact.source = None,
            "name" => request.artifact.name = "../escape".into(),
            "route" => request.call.route = "unknown".into(),
            _ => request.call.body = Some(serde_json::json!({})),
        }
        let context = fixtures::context(
            &server,
            TlsPolicy::AdditionalCaBundle(server.root()),
            fixtures::control(3000)?,
        )?;
        assert!(
            PreparedHttpDownload::freeze(&registry, &operation, context, request).is_err(),
            "{mode}"
        );
    }
    assert_eq!(server.finish()?.len(), 0);
    Ok(())
}
#[test]
fn retryable_status_before_streaming_is_repeated_once_and_publishes_one_artifact() -> TestResult {
    let bytes = b"retried-binary".to_vec();
    let server = TlsServer::start(Reply::Sequence(vec![
        raw(
            503,
            b"busy",
            &[("Content-Length", "4".into()), ("Retry-After", "0".into())],
        ),
        complete(&bytes),
    ]))?;
    let (registry, operation) = fixtures::prepared(EffectClass::Read)?;
    let store = MemoryArtifacts::new(Faults::default(), 100);
    let outcome = prepared(&registry, &operation, &server, 100, None)?.receive(&store, || Ok(()));
    assert!(outcome.error.is_none(), "{:?}", outcome.error);
    assert_eq!(outcome.http_status, Some(200));
    assert_eq!(outcome.received_bytes, bytes.len() as u64);
    assert_eq!(outcome.artifact_write_state, RequestState::Confirmed);
    assert_eq!(server.finish()?.len(), 2);
    Ok(())
}
