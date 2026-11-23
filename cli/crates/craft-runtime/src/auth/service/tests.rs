use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_core::control::CancellationToken;
use craft_test_support::state::ManualClock;
use serde_json::{Value, json};

use super::*;
use crate::control::SystemClock;

use crate::tls_fixture::{Reply, TlsServer};

mod access_tests;
mod cache_tests;
mod identity_tests;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod native_tests;
mod static_grant_tests;

const TOKEN: &str = "service-bearer-sentinel";
const CLIENT_ID: &str = "synthetic client &+日本語";
const CLIENT_SECRET: &str = "synthetic-secret/+%=&+";

fn credentials() -> Result<ServiceCredentials, Box<dyn std::error::Error>> {
    Ok(ServiceCredentials::from_provisioning(SecretBytes::new(
        serde_json::to_vec(&json!({"client_id":CLIENT_ID,"client_secret":CLIENT_SECRET}))?,
    )?)?)
}
fn valid() -> Value {
    json!({"access_token":TOKEN,"expires_in":3600,"token_type":"bEaReR","scope":"read:jira-user read:confluence-user"})
}
fn control(token: CancellationToken, timeout: u64) -> Result<InvocationControl, CraftError> {
    InvocationControl::new(Arc::new(SystemClock::default()), token, timeout)
}
fn client(
    server: &TlsServer,
    clock: Arc<dyn Clock>,
    token: CancellationToken,
    timeout: u64,
) -> Result<ServiceOAuthClient, CraftError> {
    let mut client = ServiceOAuthClient::new(
        TlsPolicy::AdditionalCaBundle(server.root()),
        control(token, timeout)?,
        clock,
    )?;
    client.endpoint = reqwest::Url::parse(&format!(
        "https://localhost:{}/oauth/token",
        server.address().port()
    ))
    .map_err(|_| invalid_client())?;
    Ok(client)
}
fn no_leaks(failure: &AuthFailure) -> Result<(), Box<dyn std::error::Error>> {
    let output = format!(
        "{failure:?} {failure} {}",
        serde_json::to_string(&failure.error)?
    );
    for secret in [CLIENT_ID, CLIENT_SECRET, TOKEN, "error-body-sentinel"] {
        assert!(!output.contains(secret));
    }
    assert!(!failure.error.retryable);
    Ok(())
}

#[test]
fn reflected_credentials_or_bearer_in_scope_metadata_are_refused_before_public_allocation()
-> Result<(), Box<dyn std::error::Error>> {
    for secret in [CLIENT_ID, CLIENT_SECRET, TOKEN] {
        let mut response = valid();
        response["scope"] = json!(format!("read:jira-user {secret}"));
        let issuer = TlsServer::start(Reply::json(200, &serde_json::to_vec(&response)?, &[]))?;
        let client = client(
            &issuer,
            Arc::new(ManualClock::new(1_000)),
            CancellationToken::default(),
            10_000,
        )?;
        let failure = client
            .acquire(&credentials()?)
            .err()
            .ok_or("Expected reflected secret refusal")?;
        no_leaks(&failure)?;
        assert_eq!(failure.credential_write_state, None);
        assert_eq!(issuer.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn service_request_is_fixed_form_encoded_single_attempt_and_secret_redacted()
-> Result<(), Box<dyn std::error::Error>> {
    let mut body = valid();
    body["refresh_token"] = json!("ignored-delegated-sentinel");
    body["extension"] = json!({"nested":[true,1,"ignored"]});
    let server = TlsServer::start(Reply::json(200, &serde_json::to_vec(&body)?, &[]))?;
    let clock = Arc::new(ManualClock::new(1_000));
    let client = client(&server, clock, CancellationToken::default(), 10_000)?;
    let credentials = credentials()?;
    assert!(!format!("{credentials:?}").contains(CLIENT_SECRET));
    let token = client.acquire(&credentials)?;
    assert!(!format!("{token:?}").contains(TOKEN));
    assert!(token.access_token.expose() == TOKEN.as_bytes());
    assert_eq!(token.metadata.expires_at_unix_ms, 3_601_000);
    assert_eq!(token.metadata.reacquire_at_unix_ms, 3_541_000);
    assert_eq!(
        token.metadata.scopes,
        Some(vec!["read:confluence-user".into(), "read:jira-user".into()])
    );
    let metadata = serde_json::to_string(token.metadata())?;
    assert!(
        !metadata.contains(TOKEN)
            && !metadata.contains(CLIENT_SECRET)
            && !metadata.contains("ignored-delegated-sentinel")
    );
    let requests = server.finish()?;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/oauth/token");
    assert_eq!(
        request.headers.get("content-type").map(String::as_str),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        request.headers.get("accept-encoding").map(String::as_str),
        Some("identity")
    );
    assert!(!request.headers.contains_key("authorization"));
    let fields: BTreeMap<_, _> = url::form_urlencoded::parse(&request.body)
        .into_owned()
        .collect();
    assert_eq!(fields.len(), 3);
    assert_eq!(
        fields.get("grant_type").map(String::as_str),
        Some("client_credentials")
    );
    assert!(
        fields
            .get("client_id")
            .is_some_and(|value| value == CLIENT_ID)
    );
    assert!(
        fields
            .get("client_secret")
            .is_some_and(|value| value == CLIENT_SECRET)
    );
    let production = ServiceOAuthClient::new(
        TlsPolicy::Platform,
        control(CancellationToken::default(), 10_000)?,
        Arc::new(SystemClock::default()),
    )?;
    assert_eq!(production.endpoint.as_str(), TOKEN_ENDPOINT);
    Ok(())
}

#[test]
fn malformed_provisioning_is_local_bounded_and_never_echoes_values()
-> Result<(), Box<dyn std::error::Error>> {
    for body in [
        b"{\"client_id\":\"error-body-sentinel\",\"client_secret\":\"x\",\"extra\":true}".to_vec(),
        b"{\"client_id\":\"a\",\"client_id\":\"error-body-sentinel\",\"client_secret\":\"x\"}"
            .to_vec(),
        b"{\"client_id\":\"a\",\"client_secret\":\"\"}".to_vec(),
        b"{\"client_id\":\"a\",\"client_secret\":\"line\\nbreak\"}".to_vec(),
        serde_json::to_vec(
            &json!({"client_id":"x".repeat(1_025),"client_secret":"error-body-sentinel"}),
        )?,
        serde_json::to_vec(&json!({"client_id":"x","client_secret":"x".repeat(65_537)}))?,
        vec![b' '; PROVISIONING_LIMIT + 1],
    ] {
        let error = ServiceCredentials::from_provisioning(SecretBytes::new(body)?)
            .err()
            .ok_or("Expected provisioning refusal")?;
        assert_eq!(error.code, ErrorCode::InputInvalid);
        assert!(!serde_json::to_string(&error)?.contains("error-body-sentinel"));
    }
    Ok(())
}

#[test]
fn response_extensions_are_ignored_but_scope_omission_remains_unknown()
-> Result<(), Box<dyn std::error::Error>> {
    let mut response = valid();
    response
        .as_object_mut()
        .ok_or("Expected response object")?
        .remove("scope");
    response["refresh_token"] = json!("never-refresh-this-service");
    let server = TlsServer::start(Reply::json(200, &serde_json::to_vec(&response)?, &[]))?;
    let client = client(
        &server,
        Arc::new(ManualClock::new(1_000)),
        CancellationToken::default(),
        10_000,
    )?;
    let token = client.acquire(&credentials()?)?;
    assert!(token.metadata.scopes.is_none());
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[test]
fn invalid_duplicate_missing_and_oversized_token_responses_never_issue_sessions()
-> Result<(), Box<dyn std::error::Error>> {
    let mut bodies = Vec::new();
    for (field, value) in [
        ("expires_in", json!(0)),
        ("expires_in", json!(-1)),
        ("expires_in", json!(1.5)),
        ("expires_in", json!(86_401)),
        ("expires_in", json!("3600")),
        ("token_type", json!("Basic")),
        ("access_token", json!("x=y")),
        ("access_token", json!("line\nbreak")),
        ("access_token", json!("x".repeat(16_385))),
        ("scope", Value::Null),
        ("scope", json!("a  b")),
        ("scope", json!("a\tb")),
        ("scope", json!("a\\b")),
        ("scope", json!("é")),
    ] {
        let mut body = valid();
        body[field] = value;
        bodies.push(serde_json::to_vec(&body)?);
    }
    for field in ["access_token", "token_type", "expires_in"] {
        let mut body = valid();
        body.as_object_mut().ok_or("Expected object")?.remove(field);
        bodies.push(serde_json::to_vec(&body)?);
    }
    bodies.extend([
        br#"{"access_token":"x","access_token":"y","expires_in":3600,"token_type":"Bearer"}"#.to_vec(),
        br#"{"access_token":"x","expires_in":3600,"token_type":"Bearer","extension":1,"extension":2}"#.to_vec(),
        b"null".to_vec(), b"[]".to_vec(), b"{} trailing".to_vec(), vec![b'x'; RESPONSE_LIMIT + 1],
    ]);
    for body in bodies {
        let server = TlsServer::start(Reply::json(200, &body, &[]))?;
        let client = client(
            &server,
            Arc::new(ManualClock::new(1_000)),
            CancellationToken::default(),
            10_000,
        )?;
        let failure = client
            .acquire(&credentials()?)
            .err()
            .ok_or("Expected token response refusal")?;
        no_leaks(&failure)?;
        assert_eq!(failure.error.details.write_state, RequestState::SentUnknown);
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn failed_statuses_are_scrubbed_preserve_typed_rate_advice_and_never_retry()
-> Result<(), Box<dyn std::error::Error>> {
    for (status, code) in [
        (400, ErrorCode::AuthRequired),
        (401, ErrorCode::AuthRequired),
        (403, ErrorCode::PermissionDenied),
        (429, ErrorCode::RateLimited),
        (500, ErrorCode::RemoteUnavailable),
    ] {
        let server = TlsServer::start(Reply::json(
            status,
            b"error-body-sentinel synthetic-secret/+%=&+",
            &[
                ("Retry-After", "17"),
                ("x-request-id", "error-body-sentinel"),
            ],
        ))?;
        let client = client(
            &server,
            Arc::new(ManualClock::new(1_000)),
            CancellationToken::default(),
            10_000,
        )?;
        let failure = client
            .acquire(&credentials()?)
            .err()
            .ok_or("Expected failure status")?;
        no_leaks(&failure)?;
        assert_eq!(failure.error.code, code);
        assert_eq!(failure.retry_after, Some(RetryAfter::DelaySeconds(17)));
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}

#[test]
fn redirects_and_invalid_tls_never_forward_service_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    let sink = TlsServer::start(Reply::Disconnect)?;
    let location = format!("https://localhost:{}/steal", sink.address().port());
    let server = TlsServer::start(Reply::json(
        307,
        b"error-body-sentinel",
        &[("Location", &location)],
    ))?;
    let host = client(
        &server,
        Arc::new(ManualClock::new(1_000)),
        CancellationToken::default(),
        10_000,
    )?;
    let failure = host
        .acquire(&credentials()?)
        .err()
        .ok_or("Expected redirect refusal")?;
    assert_eq!(failure.error.code, ErrorCode::PolicyDenied);
    no_leaks(&failure)?;
    assert_eq!(server.finish()?.len(), 1);
    assert_eq!(sink.connections(), 0);
    for wrong_host in [false, true] {
        let server = TlsServer::start(Reply::Disconnect)?;
        let mut host = client(
            &server,
            Arc::new(ManualClock::new(1_000)),
            CancellationToken::default(),
            10_000,
        )?;
        if wrong_host {
            host.endpoint
                .set_host(Some("127.0.0.1"))
                .map_err(|_| "Invalid fixture host")?;
        } else {
            host.client = client_builder(TlsPolicy::Platform)?.build()?;
        }
        no_leaks(
            &host
                .acquire(&credentials()?)
                .err()
                .ok_or("Expected TLS refusal")?,
        )?;
        assert!(server.connections() > 0);
        assert_eq!(server.finish()?.len(), 0);
    }
    Ok(())
}

#[test]
fn expiry_uses_returned_lifetime_and_both_clock_domains_without_exporting_tokens()
-> Result<(), Box<dyn std::error::Error>> {
    for backward in [false, true] {
        let server = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
        let clock = Arc::new(ManualClock::new(1_000));
        let client = client(&server, clock.clone(), CancellationToken::default(), 10_000)?;
        let token = client.acquire(&credentials()?)?;
        assert!(!token.needs_reacquisition());
        if backward {
            clock.set_wall(999)?;
        } else {
            clock.advance(Duration::from_secs(3_541))?;
            clock.set_wall(1_001)?;
        }
        assert!(token.needs_reacquisition());
        let error = token
            .into_host_session(
                TenantRef {
                    provider: "jira".into(),
                    deployment: "cloud".into(),
                    tenant: "00000000-1111-2222-3333-444444444444".into(),
                },
                GrantRef { id: "grant".into() },
            )
            .err()
            .ok_or("Expected expired session refusal")?;
        assert_eq!(error.code, ErrorCode::AuthRequired);
        assert_eq!(server.finish()?.len(), 1);
    }
    Ok(())
}

struct ExpiringClock(Arc<AtomicUsize>);
impl Clock for ExpiringClock {
    fn unix_millis(&self) -> u64 {
        1_000
    }
    fn monotonic_millis(&self) -> u64 {
        if self.0.load(Ordering::SeqCst) == 0 {
            0
        } else {
            3_600_000
        }
    }
}
#[test]
fn tokens_that_are_stale_by_response_time_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let server = TlsServer::start(Reply::json(200, &serde_json::to_vec(&valid())?, &[]))?;
    let client = client(
        &server,
        Arc::new(ExpiringClock(server.request_counter())),
        CancellationToken::default(),
        10_000,
    )?;
    no_leaks(
        &client
            .acquire(&credentials()?)
            .err()
            .ok_or("Expected stale response refusal")?,
    )?;
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[test]
fn cancellation_and_deadlines_stop_stalled_exchanges_without_replay()
-> Result<(), Box<dyn std::error::Error>> {
    for cancel in [false, true] {
        let server = TlsServer::start(Reply::Stall)?;
        let token = CancellationToken::default();
        let client = client(
            &server,
            Arc::new(ManualClock::new(1_000)),
            token.clone(),
            if cancel { 5_000 } else { 1_000 },
        )?;
        let trigger = if cancel {
            let received = server.request_counter();
            Some(std::thread::spawn(move || -> Result<(), &'static str> {
                let started = std::time::Instant::now();
                while received.load(Ordering::SeqCst) == 0 {
                    if started.elapsed() > Duration::from_secs(3) {
                        token.cancel();
                        return Err("No service request before cancellation");
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                token.cancel();
                Ok(())
            }))
        } else {
            None
        };
        let failure = client
            .acquire(&credentials()?)
            .err()
            .ok_or("Expected bounded stall failure")?;
        if let Some(trigger) = trigger {
            trigger
                .join()
                .map_err(|_| "Cancellation thread panicked")??;
        }
        assert_eq!(
            failure.error.code,
            if cancel {
                ErrorCode::Cancelled
            } else {
                ErrorCode::DeadlineExceeded
            }
        );
        no_leaks(&failure)?;
        assert_eq!(server.request_count(), 1);
        assert_eq!(server.finish()?.len(), 1);
    }
    let server = TlsServer::start(Reply::Disconnect)?;
    let token = CancellationToken::default();
    let client = client(
        &server,
        Arc::new(ManualClock::new(1_000)),
        token.clone(),
        10_000,
    )?;
    token.cancel();
    assert_eq!(
        client
            .acquire(&credentials()?)
            .err()
            .ok_or("Expected local cancellation")?
            .error
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(server.connections(), 0);
    Ok(())
}
