use super::*;
use craft_core::contracts::RequestState;
use std::sync::atomic::{AtomicU64, Ordering};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn target() -> ResourceRef {
    ResourceRef {
        provider: "confluence".into(),
        deployment: "cloud".into(),
        tenant: "00000000-1111-2222-3333-444444444444".into(),
        resource_type: "site".into(),
        id: "00000000-1111-2222-3333-444444444444".into(),
    }
}
fn request() -> CanonicalRequest {
    CanonicalRequest {
        command: PAGE_CREATE.into(),
        target: target(),
        input: json!({"space_id":"100","title":"Shared plan","placement":{"kind":"root"},"body":{"representation":"storage","value":"<p>Review</p>"}}),
        selection: Selection::default(),
        preconditions: None,
    }
}
fn policy() -> PageWritePolicy {
    PageWritePolicy {
        target: target(),
        space_id: "100".into(),
        grant: GrantRef {
            id: "service-grant".into(),
        },
        expected_actor: "service-actor".into(),
        enabled: true,
        allow_create: true,
        allow_update: false,
    }
}
fn prepared() -> Result<PreparedPagePublication, CraftError> {
    PreparedPagePublication::prepare(request(), "publish-plan", true, "request", policy())
}

#[test]
fn page_publication_validates_apply_target_intent_and_receipt_capacity_without_io() -> TestResult {
    for mode in [
        "apply",
        "space",
        "target",
        "actor",
        "disabled",
        "operation",
        "budget",
        "malformed",
    ] {
        let mut p = policy();
        let mut r = request();
        match mode {
            "space" => p.space_id = "200".into(),
            "target" => p.target.id = "11111111-1111-2222-3333-444444444444".into(),
            "actor" => p.expected_actor = "anonymous".into(),
            "disabled" => p.enabled = false,
            "operation" => p.allow_create = false,
            "budget" => r.selection.max_bytes = 512,
            "malformed" => r.input["status"] = json!("draft"),
            _ => {}
        }
        assert!(
            PreparedPagePublication::prepare(r, "publish-plan", mode != "apply", "request", p)
                .is_err(),
            "{mode}"
        );
    }
    assert!(prepared().is_ok());
    Ok(())
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
#[test]
fn original_page_evidence_age_rejects_either_expired_or_backward_clock() -> TestResult {
    for (wall, mono) in [(31_000, 1000), (1000, 31_000), (999, 1000), (1000, 999)] {
        let clock = Arc::new(TestClock {
            wall: AtomicU64::new(1000),
            mono: AtomicU64::new(1000),
        });
        let freshness = Freshness::capture(clock.clone());
        clock.wall.store(wall, Ordering::SeqCst);
        clock.mono.store(mono, Ordering::SeqCst);
        assert_eq!(
            freshness.check().err().ok_or("expiry")?.code,
            ErrorCode::VersionConflict
        );
    }
    Ok(())
}
#[test]
fn uncertain_claim_receipt_survives_a_failure_before_transport_returns_an_attempt() -> TestResult {
    let mut error = CraftError::new(
        ErrorCode::WriteOutcomeUnknown,
        "Claim commit was not acknowledged.",
    );
    error.details.write_state = RequestState::SentUnknown;
    let result = prepared()?.completion.before_send(error, None);
    assert_eq!(
        result.envelope.data.ok_or("data")?["receipt"]["request_state"],
        "sent_unknown"
    );
    let prepared = prepared()?;
    let mut prior = JournalRecord::initial(prepared.binding.clone())?;
    prior
        .steps
        .insert(pages::CREATE_ROUTE.into(), Default::default());
    let error = CraftError::new(
        ErrorCode::WriteOutcomeUnknown,
        "Claim commit was not acknowledged.",
    );
    let result = prepared.completion.claim_failure(error, Some(&prior));
    assert_eq!(
        result.envelope.data.ok_or("data")?["receipt"]["request_state"],
        "sent_unknown"
    );
    Ok(())
}
