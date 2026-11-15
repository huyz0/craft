//! Issue-search scans through the real service access, transport and search adapter.
use super::*;
use craft_core::scan::{ScanCaps, ScanOutcome};
use craft_runtime::artifacts::NativeArtifacts;
use craft_runtime::scan::{self, Advanced, ScanCheckpoint, ScanInputs};

type ScanAttempt = (
    Result<Advanced, CraftError>,
    Option<craft_core::contracts::CredentialCacheUpdate>,
);

fn scan_once(
    setup: &Setup,
    fixture: &ServiceFixture,
    product: &TlsServer,
    previous: &ScanCheckpoint,
    host: &PreparedIssueRead,
) -> Result<ScanAttempt, Box<dyn std::error::Error>> {
    let control = control()?;
    let locked = fixture.lock(&setup.grant, &control)?;
    let clock = SystemClock::default();
    let id = |item: &Value| item["id"].as_str().map(String::from);
    let mut run = |session: craft_cli::scans::ScanSession<'_>| {
        scan::advance(
            previous,
            ScanInputs {
                source: session.source,
                binding: session.binding,
                project: session.project,
                item_id: &id,
                rate_advice: session.rate_advice,
                control: &control,
                clock: &clock,
            },
        )
    };
    Ok(host.scan_cached(
        fixture.client(),
        &locked,
        TlsPolicy::AdditionalCaBundle(product.root()),
        control.clone(),
        || Ok(()),
        &mut run,
    ))
}

#[test]
fn issue_search_scan_follows_tokens_to_completion_and_keeps_rate_advice() -> TestResult {
    let setup = Setup::new()?;
    let issuer = issuer("read:jira-user read:jira-work")?;
    let product = TlsServer::start(Reply::Sequence(vec![
        identity(),
        identity(),
        reply(json!({"issues":[issue(1),issue(2)],"isLast":false,"nextPageToken":"t-2"}))?,
        Reply::json(429, br#"{}"#, &[("Retry-After", "90")]),
        identity(),
        reply(json!({"issues":[issue(3)],"isLast":true}))?,
    ]))?;
    let fixture = setup.fixture(&issuer, &product, Arc::new(ManualClock::new(1000)))?;
    let locked = fixture.lock(&setup.grant, &control()?)?;
    fixture.client().provision_grant(
        &locked,
        fixture.service_credentials()?,
        &[target().tenant_ref()],
        Some("actor-one"),
    )?;
    drop(locked);
    let host = prepared(
        &setup.grant,
        SEARCH,
        search_input(),
        &["summary"],
        None,
        20,
        16_777_216,
    )?;
    let start = ScanCheckpoint::start(
        SEARCH,
        json!({"input":search_input(),"record_fields":["summary"]}),
        host.binding(),
        ScanCaps {
            max_items: 100,
            max_pages: 10,
            max_stored_bytes: 1_048_576,
        },
        1000,
    )?;
    let artifacts = NativeArtifacts::open(&setup.root.join("scans"), 1 << 27, &control()?)?;
    let (first, cache) = scan_once(&setup, &fixture, &product, &start, &host)?;
    let mut first = first?;
    let _ = cache;
    assert_eq!(first.checkpoint.outcome, ScanOutcome::RateLimited);
    assert_eq!(
        first.error.as_ref().map(|e| e.code),
        Some(ErrorCode::RateLimited)
    );
    assert_eq!(
        first.retry_after,
        Some(craft_core::execution::RetryAfter::DelaySeconds(90))
    );
    assert_eq!(first.checkpoint.totals.items, 2);
    let published = scan::publish(&artifacts, &mut first)?;
    let loaded = scan::load(
        &artifacts,
        &published.checkpoint.name,
        &published.checkpoint.digest,
    )?;
    let (second, _) = scan_once(&setup, &fixture, &product, &loaded, &host)?;
    let mut second = second?;
    assert_eq!(second.checkpoint.outcome, ScanOutcome::Complete);
    scan::publish(&artifacts, &mut second)?;
    let export = scan::assemble(&artifacts, &second.checkpoint)?;
    let blob = craft_core::storage::ArtifactStore::read(&artifacts, &export.name, export.bytes)?
        .ok_or("export")?;
    let lines: Vec<Value> = blob
        .bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice)
        .collect::<Result<_, _>>()?;
    assert_eq!(
        lines
            .iter()
            .map(|line| line["id"].clone())
            .collect::<Vec<_>>(),
        vec![json!("1"), json!("2"), json!("3")]
    );
    // Records carry the selected projection only, never unselected private fields.
    assert!(!String::from_utf8_lossy(&blob.bytes).contains("private-sentinel"));
    let requests = product.finish()?;
    let searches: Vec<_> = requests
        .iter()
        .filter(|r| r.target.ends_with("/rest/api/3/search/jql"))
        .collect();
    // One page, the rate-limited token request, then the same token again on resume.
    assert_eq!(searches.len(), 3);
    for (index, token) in [(1, "t-2"), (2, "t-2")] {
        let body: Value = serde_json::from_slice(&searches[index].body)?;
        assert_eq!(body["nextPageToken"], token);
    }
    drop(fixture);
    setup.no_plaintext_credentials()
}
