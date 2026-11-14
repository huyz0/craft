use super::*;

fn statuses(replies: &[(u16, &[(&str, &str)])]) -> Reply {
    Reply::Sequence(
        replies
            .iter()
            .map(|(status, headers)| {
                let body: &[u8] = if *status == 200 {
                    br#"{"ok":true}"#
                } else {
                    b""
                };
                Reply::json(*status, body, headers)
            })
            .collect(),
    )
}

fn host(server: &TlsServer, timeout: u64) -> Result<HttpContext, Box<dyn std::error::Error>> {
    context(
        server,
        TlsPolicy::AdditionalCaBundle(server.root()),
        control(timeout)?,
    )
}

#[test]
fn safe_reads_repeat_only_retryable_statuses_within_three_sends()
-> Result<(), Box<dyn std::error::Error>> {
    for status in [429, 502, 503, 504] {
        let server = TlsServer::start(statuses(&[(status, &[]), (200, &[])]))?;
        let response = host(&server, 10_000)?.invoke(&call())?;
        assert_eq!(response.status, 200, "{status}");
        assert_eq!(response.body["ok"], true);
        assert_eq!(server.finish()?.len(), 2, "{status}");
    }
    let server = TlsServer::start(statuses(&[
        (429, &[("Retry-After", "0")]),
        (503, &[("Retry-After", "0")]),
        (429, &[("Retry-After", "0"), ("x-request-id", "final-1")]),
    ]))?;
    let response = host(&server, 10_000)?.invoke(&call())?;
    // Exhaustion returns the final native result so the provider keeps its typed advice.
    assert_eq!(response.status, 429);
    assert_eq!(response.request_id.as_deref(), Some("final-1"));
    assert_eq!(response.retry_after, Some(RetryAfter::DelaySeconds(0)));
    assert_eq!(server.finish()?.len(), 3);
    Ok(())
}

#[test]
fn non_retryable_statuses_are_sent_once() -> Result<(), Box<dyn std::error::Error>> {
    for status in [400, 401, 403, 404, 409, 500] {
        let server = TlsServer::start(statuses(&[(status, &[("Retry-After", "0")])]))?;
        assert_eq!(host(&server, 10_000)?.invoke(&call())?.status, status);
        assert_eq!(server.finish()?.len(), 1, "{status}");
    }
    Ok(())
}

#[test]
fn advice_beyond_the_cap_or_the_remaining_budget_is_returned_without_waiting()
-> Result<(), Box<dyn std::error::Error>> {
    for (advice, timeout) in [("31", 60_000), ("3", 3_500)] {
        let server = TlsServer::start(statuses(&[(429, &[("Retry-After", advice)])]))?;
        let started = std::time::Instant::now();
        let response = host(&server, timeout)?.invoke(&call())?;
        assert_eq!(response.status, 429);
        assert!(started.elapsed() < Duration::from_secs(2), "{advice}");
        assert_eq!(server.finish()?.len(), 1, "{advice}");
    }
    // An HTTP date already in the past means no wait, not a negative or wrapped delay.
    let server = TlsServer::start(statuses(&[
        (503, &[("Retry-After", "Wed, 21 Oct 2015 07:28:00 GMT")]),
        (200, &[]),
    ]))?;
    assert_eq!(host(&server, 10_000)?.invoke(&call())?.status, 200);
    assert_eq!(server.finish()?.len(), 2);
    Ok(())
}

#[test]
fn connection_failures_before_a_response_are_retried_but_bad_bodies_are_not()
-> Result<(), Box<dyn std::error::Error>> {
    let server = TlsServer::start(Reply::Sequence(vec![
        Reply::Disconnect,
        Reply::json(200, br#"{"ok":true}"#, &[]),
    ]))?;
    assert_eq!(host(&server, 10_000)?.invoke(&call())?.status, 200);
    assert_eq!(server.finish()?.len(), 2);

    let server = TlsServer::start(Reply::Disconnect)?;
    let error = host(&server, 10_000)?
        .invoke(&call())
        .err()
        .ok_or("Expected exhaustion")?;
    assert_eq!(error.code, ErrorCode::RemoteUnavailable);
    assert_eq!(error.details.write_state, RequestState::Failed);
    assert!(error.next_action.contains("3 bounded attempts"));
    assert_eq!(server.finish()?.len(), 3);

    let server = TlsServer::start(Reply::json(200, b"{not json", &[]))?;
    let error = host(&server, 10_000)?
        .invoke(&call())
        .err()
        .ok_or("Expected invalid body")?;
    assert_eq!(error.code, ErrorCode::RemoteUnavailable);
    assert!(error.next_action.contains("no automatic retry"));
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[test]
fn cancellation_interrupts_a_retry_wait_and_a_short_budget_skips_it()
-> Result<(), Box<dyn std::error::Error>> {
    for cancel in [true, false] {
        let server = TlsServer::start(statuses(&[(503, &[("Retry-After", "2")]), (200, &[])]))?;
        let token = CancellationToken::default();
        // 2.9 s cannot fit a 2 s wait plus the one-second floor, so no wait is started.
        let invocation = InvocationControl::new(
            Arc::new(SystemClock::default()),
            token.clone(),
            if cancel { 10_000 } else { 2_900 },
        )?;
        let mut host = context(
            &server,
            TlsPolicy::AdditionalCaBundle(server.root()),
            invocation,
        )?;
        let received = server.request_counter();
        let trigger = cancel.then(|| {
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                while received.load(std::sync::atomic::Ordering::SeqCst) == 0
                    && started.elapsed() < Duration::from_secs(3)
                {
                    std::thread::sleep(Duration::from_millis(2));
                }
                std::thread::sleep(Duration::from_millis(200));
                token.cancel();
            })
        });
        let started = std::time::Instant::now();
        let outcome = host.invoke(&call());
        if let Some(trigger) = trigger {
            trigger.join().map_err(|_| "Cancellation thread panicked")?;
            let error = outcome.err().ok_or("Expected cancellation")?;
            assert_eq!(error.code, ErrorCode::Cancelled);
            assert_eq!(error.details.write_state, RequestState::Failed);
            assert!(started.elapsed() < Duration::from_millis(1_500));
        } else {
            assert_eq!(outcome?.status, 503);
            assert!(started.elapsed() < Duration::from_secs(1));
        }
        assert_eq!(server.finish()?.len(), 1, "cancel={cancel}");
    }
    Ok(())
}

#[test]
fn a_context_labelled_as_a_write_never_repeats_its_send() -> Result<(), Box<dyn std::error::Error>>
{
    let server = TlsServer::start(Reply::json(503, b"", &[("Retry-After", "0")]))?;
    let mut host = host(&server, 10_000)?;
    host.effect = EffectClass::Write;
    host.routes[0].method = HttpMethod::Post;
    let mut call = call();
    call.body = Some(json!({"summary":"synthetic"}));
    assert_eq!(host.invoke(&call)?.status, 503);
    assert_eq!(server.finish()?.len(), 1);
    Ok(())
}

#[test]
fn only_sealed_safe_read_descriptors_opt_into_retry() -> Result<(), Box<dyn std::error::Error>> {
    let (registry, operation) = prepared(EffectClass::Read)?;
    let mut descriptor = registry.prepared_descriptor(&operation)?.clone();
    assert_eq!(
        retry::ReadRetry::for_descriptor(&descriptor),
        Some(retry::ReadRetry::STANDARD)
    );
    for class in [
        RetryClass::LocalRead,
        RetryClass::Never,
        RetryClass::ReconcileBeforeReplay,
    ] {
        descriptor.retry_class = class;
        assert_eq!(retry::ReadRetry::for_descriptor(&descriptor), None);
    }
    descriptor.retry_class = RetryClass::SafeRead;
    descriptor.effect = EffectClass::Write;
    assert_eq!(retry::ReadRetry::for_descriptor(&descriptor), None);
    Ok(())
}

#[test]
fn backoff_doubles_from_the_base_and_never_extends_the_deadline()
-> Result<(), Box<dyn std::error::Error>> {
    let policy = retry::ReadRetry::STANDARD;
    let long = control(60_000)?;
    assert_eq!(
        policy.wait(1, None, &long),
        Some(Duration::from_millis(250))
    );
    assert_eq!(
        policy.wait(2, None, &long),
        Some(Duration::from_millis(500))
    );
    assert_eq!(policy.wait(3, None, &long), None);
    assert_eq!(policy.wait(0, None, &long), None);
    let wide = retry::ReadRetry {
        attempts: 10,
        ..policy
    };
    assert_eq!(wide.wait(9, None, &long), Some(Duration::from_secs(4)));
    assert_eq!(
        policy.wait(1, Some(RetryAfter::DelaySeconds(30)), &long),
        Some(Duration::from_secs(30))
    );
    assert_eq!(
        policy.wait(1, Some(RetryAfter::DelaySeconds(31)), &long),
        None
    );
    assert_eq!(policy.wait(1, None, &control(1_100)?), None);
    Ok(())
}
