use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft_core::contracts::{GrantRef, ResourceRef};
use craft_core::control::CancellationToken;
use craft_core::storage::ArtifactStore;
use craft_test_support::paging::{Fault, Style, SyntheticService, dataset, ids};
use craft_test_support::state::ManualClock;
use serde_json::json;

use super::*;
use crate::artifacts::NativeArtifacts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Fixture {
    parent: PathBuf,
    root: PathBuf,
    clock: ManualClock,
    binding: ListBinding,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&parent)?;
        let parent = parent.canonicalize()?;
        let root = parent.join(random_id("scan-fixture-")?);
        let digest = canonical_digest(&json!({"revision":1}))?;
        Ok(Self {
            parent,
            root,
            clock: ManualClock::new(10_000),
            binding: ListBinding {
                operation: "jira.issue.search".into(),
                descriptor_digest: digest.clone(),
                schema_digest: digest.clone(),
                projection_digest: digest,
                api_revision: "v3".into(),
                target: ResourceRef {
                    provider: "jira".into(),
                    deployment: "cloud".into(),
                    tenant: "00000000-1111-2222-3333-444444444444".into(),
                    resource_type: "site".into(),
                    id: "00000000-1111-2222-3333-444444444444".into(),
                },
                grant: GrantRef {
                    id: format!("gr_{}", "0".repeat(32)),
                },
                actor: "actor-one".into(),
                query: BTreeMap::from([("jql".into(), json!("project = ED"))]),
            },
        })
    }

    fn control(&self, token: &CancellationToken) -> Result<InvocationControl, CraftError> {
        InvocationControl::new(Arc::new(self.clock.clone()), token.clone(), 60_000)
    }

    fn store(&self, control: &InvocationControl) -> Result<NativeArtifacts, StoreError> {
        NativeArtifacts::open(&self.root, MAX_STORED_BYTES * 2, control)
    }

    fn caps(items: u32, pages: u32, bytes: u64) -> ScanCaps {
        ScanCaps {
            max_items: items,
            max_pages: pages,
            max_stored_bytes: bytes,
        }
    }

    fn start(&self, caps: ScanCaps) -> Result<ScanCheckpoint, CraftError> {
        ScanCheckpoint::start(
            "jira.issue.search",
            json!({"jql":"project = ED"}),
            &self.binding,
            caps,
            self.clock.unix_millis(),
        )
    }

    /// One invocation: advance, then publish, returning the stored checkpoint and its receipt.
    fn run(
        &self,
        previous: &ScanCheckpoint,
        service: &mut SyntheticService,
        token: &CancellationToken,
    ) -> Result<(Advanced, Published), Box<dyn std::error::Error>> {
        let control = self.control(token)?;
        let project = |items: &[Value]| Ok(items.to_vec());
        let item_id = |item: &Value| item["id"].as_str().map(String::from);
        let advice = || Some(RetryAfter::DelaySeconds(30));
        let mut advanced = advance(
            previous,
            ScanInputs {
                source: service,
                binding: &self.binding,
                project: &project,
                item_id: &item_id,
                rate_advice: &advice,
                control: &control,
                clock: &self.clock,
            },
        )?;
        // Cancellation stops traversal but evidence publication uses a fresh bounded control,
        // as the CLI does, so fetched records are not lost.
        let evidence = self.control(&CancellationToken::default())?;
        let published = publish(&self.store(&evidence)?, &mut advanced)?;
        Ok((advanced, published))
    }

    fn records(
        &self,
        checkpoint: &ScanCheckpoint,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let control = self.control(&CancellationToken::default())?;
        let store = self.store(&control)?;
        let receipt = assemble(&store, checkpoint)?;
        let blob = store
            .read(&receipt.name, receipt.bytes.max(1))?
            .ok_or("export")?;
        let mut items = Vec::new();
        for line in blob
            .bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            items.push(serde_json::from_slice::<Value>(line)?);
        }
        Ok(ids(&items))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(path) = self.root.canonicalize()
            && path.starts_with(&self.parent)
            && path != self.parent
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn expected(count: usize) -> Vec<String> {
    ids(&dataset(count, 0))
}

#[test]
fn every_paging_style_scans_to_completion_with_verified_segments() -> TestResult {
    for style in craft_test_support::paging::ALL_STYLES {
        let fixture = Fixture::new()?;
        let mut service = SyntheticService::new(style, 7, dataset(30, 4));
        let start = fixture.start(Fixture::caps(1_000, 100, MAX_STORED_BYTES))?;
        let (advanced, published) =
            fixture.run(&start, &mut service, &CancellationToken::default())?;
        let checkpoint = &advanced.checkpoint;
        assert!(advanced.error.is_none());
        assert_eq!(checkpoint.outcome, ScanOutcome::Complete);
        assert!(checkpoint.complete);
        assert_eq!(checkpoint.totals.items, 30);
        assert_eq!(checkpoint.totals.pages, 5);
        assert_eq!(service.fetches, 5);
        let segment = published.segment.ok_or("segment")?;
        assert_eq!(segment.items, 30);
        let control = fixture.control(&CancellationToken::default())?;
        let store = fixture.store(&control)?;
        let loaded = load(
            &store,
            &published.checkpoint.name,
            &published.checkpoint.digest,
        )?;
        assert_eq!(&loaded, checkpoint);
        assert_eq!(fixture.records(&loaded)?, expected(30));
        // A completed scan refuses another advance rather than restarting.
        let again = fixture.run(&loaded, &mut service, &CancellationToken::default());
        assert!(again.is_err(), "{style:?}");
        assert_eq!(service.fetches, 5);
    }
    Ok(())
}

#[test]
fn caps_stop_mid_page_and_resume_without_refetching_the_remainder() -> TestResult {
    let fixture = Fixture::new()?;
    let mut service = SyntheticService::new(Style::Token, 10, dataset(25, 0));
    let start = fixture.start(Fixture::caps(4, 100, MAX_STORED_BYTES))?;
    let (first, _) = fixture.run(&start, &mut service, &CancellationToken::default())?;
    assert_eq!(first.checkpoint.outcome, ScanOutcome::BudgetReached);
    assert_eq!(first.checkpoint.limit, Some(ScanLimit::Items));
    assert!(!first.checkpoint.complete);
    assert_eq!(first.checkpoint.remainder.len(), 6);
    assert_eq!(service.fetches, 1);
    let mut raised = first.checkpoint.clone();
    raised.caps.max_items = 1_000;
    let (second, _) = fixture.run(&raised, &mut service, &CancellationToken::default())?;
    assert_eq!(second.checkpoint.outcome, ScanOutcome::Complete);
    assert_eq!(service.fetches, 3);
    assert_eq!(fixture.records(&second.checkpoint)?, expected(25));
    // Byte cap: the first record that would overflow stays buffered.
    let fixture = Fixture::new()?;
    let mut service = SyntheticService::new(Style::Offset, 10, dataset(5, 200));
    let start = fixture.start(Fixture::caps(1_000, 100, 1_024))?;
    let (bytes, _) = fixture.run(&start, &mut service, &CancellationToken::default())?;
    assert_eq!(bytes.checkpoint.limit, Some(ScanLimit::StoredBytes));
    assert!(bytes.checkpoint.totals.stored_bytes <= 1_024);
    assert_eq!(
        bytes.checkpoint.totals.items as usize + bytes.checkpoint.remainder.len(),
        5
    );
    Ok(())
}

#[test]
fn rate_limit_and_cancellation_keep_a_resumable_checkpoint() -> TestResult {
    for (fault, outcome) in [
        (Fault::RateLimited, ScanOutcome::RateLimited),
        (Fault::Cancelled, ScanOutcome::Cancelled),
        (Fault::Failure, ScanOutcome::Error),
    ] {
        let fixture = Fixture::new()?;
        let mut service =
            SyntheticService::new(Style::NextLink, 5, dataset(12, 0)).with_fault(1, fault);
        let start = fixture.start(Fixture::caps(1_000, 100, MAX_STORED_BYTES))?;
        let (first, published) =
            fixture.run(&start, &mut service, &CancellationToken::default())?;
        assert_eq!(first.checkpoint.outcome, outcome);
        assert!(!first.checkpoint.complete);
        assert_eq!(first.checkpoint.totals.items, 5);
        if outcome == ScanOutcome::RateLimited {
            assert_eq!(first.retry_after, Some(RetryAfter::DelaySeconds(30)));
            assert_eq!(
                first.error.as_ref().map(|e| e.code),
                Some(ErrorCode::RateLimited)
            );
        }
        let control = fixture.control(&CancellationToken::default())?;
        let loaded = load(
            &fixture.store(&control)?,
            &published.checkpoint.name,
            &published.checkpoint.digest,
        )?;
        let (second, _) = fixture.run(&loaded, &mut service, &CancellationToken::default())?;
        assert_eq!(second.checkpoint.outcome, ScanOutcome::Complete);
        assert_eq!(fixture.records(&second.checkpoint)?, expected(12));
        assert_eq!(second.checkpoint.segments.len(), 2);
    }
    Ok(())
}

#[test]
fn repeated_or_empty_continuation_stops_as_stalled_without_looping() -> TestResult {
    let fixture = Fixture::new()?;
    let mut service =
        SyntheticService::new(Style::Token, 3, dataset(9, 0)).with_fault(1, Fault::RepeatPosition);
    let start = fixture.start(Fixture::caps(1_000, 100, MAX_STORED_BYTES))?;
    // The repeated position is detected before it is requested a second time.
    let mut repeat = service;
    let (first, _) = fixture.run(&start, &mut repeat, &CancellationToken::default())?;
    assert_eq!(first.checkpoint.outcome, ScanOutcome::Error);
    assert_eq!(
        first.error.as_ref().map(|e| e.code),
        Some(ErrorCode::PaginationStalled)
    );
    assert_eq!(first.checkpoint.totals.items, 3);
    assert_eq!(repeat.fetches, 2);
    let fixture = Fixture::new()?;
    service = SyntheticService::new(Style::Offset, 3, dataset(9, 0));
    for fetch in 1..=8 {
        service = service.with_fault(fetch, Fault::Empty);
    }
    let start = fixture.start(Fixture::caps(1_000, 100, MAX_STORED_BYTES))?;
    let (empty, _) = fixture.run(&start, &mut service, &CancellationToken::default())?;
    assert_eq!(
        empty.error.as_ref().map(|e| e.code),
        Some(ErrorCode::PaginationStalled)
    );
    assert_eq!(empty.checkpoint.totals.empty_pages, 8);
    assert_eq!(service.fetches, 9);
    // Fewer empty pages than the limit are tolerated and the scan completes.
    let fixture = Fixture::new()?;
    let mut service = SyntheticService::new(Style::Offset, 3, dataset(9, 0))
        .with_fault(1, Fault::Empty)
        .with_fault(2, Fault::Empty);
    let start = fixture.start(Fixture::caps(1_000, 100, MAX_STORED_BYTES))?;
    let (tolerated, _) = fixture.run(&start, &mut service, &CancellationToken::default())?;
    assert_eq!(tolerated.checkpoint.outcome, ScanOutcome::Complete);
    assert_eq!(fixture.records(&tolerated.checkpoint)?, expected(9));
    Ok(())
}

#[test]
fn live_changes_are_counted_or_missed_but_never_called_a_snapshot() -> TestResult {
    let fixture = Fixture::new()?;
    // An insert at the front shifts offsets, so the next page repeats one record.
    let mut service = SyntheticService::new(Style::Offset, 4, dataset(8, 0))
        .with_fault(1, Fault::InsertFront("0000".into()));
    let start = fixture.start(Fixture::caps(1_000, 100, MAX_STORED_BYTES))?;
    let (inserted, _) = fixture.run(&start, &mut service, &CancellationToken::default())?;
    assert_eq!(inserted.checkpoint.outcome, ScanOutcome::Complete);
    assert_eq!(inserted.checkpoint.totals.duplicate_ids, 1);
    let emitted = fixture.records(&inserted.checkpoint)?;
    assert_eq!(emitted.iter().filter(|id| *id == "0004").count(), 2);
    // A delete before the next page shifts offsets the other way and a record is missed.
    let fixture = Fixture::new()?;
    let mut service = SyntheticService::new(Style::Offset, 4, dataset(8, 0))
        .with_fault(1, Fault::Delete("0001".into()));
    let start = fixture.start(Fixture::caps(1_000, 100, MAX_STORED_BYTES))?;
    let (deleted, _) = fixture.run(&start, &mut service, &CancellationToken::default())?;
    let emitted = fixture.records(&deleted.checkpoint)?;
    assert!(!emitted.contains(&"0005".to_string()));
    assert_eq!(deleted.checkpoint.totals.duplicate_ids, 0);
    Ok(())
}

#[test]
fn changed_binding_or_tampered_checkpoint_refuses_before_any_fetch() -> TestResult {
    let fixture = Fixture::new()?;
    let mut service = SyntheticService::new(Style::Token, 5, dataset(12, 0));
    let start = fixture.start(Fixture::caps(5, 100, MAX_STORED_BYTES))?;
    let (first, published) = fixture.run(&start, &mut service, &CancellationToken::default())?;
    let fetches = service.fetches;
    let mut changed = Fixture::new()?;
    changed
        .binding
        .query
        .insert("jql".into(), json!("project = OTHER"));
    let control = changed.control(&CancellationToken::default())?;
    let project = |items: &[Value]| Ok(items.to_vec());
    let item_id = |item: &Value| item["id"].as_str().map(String::from);
    let advice = || None;
    let refused = advance(
        &first.checkpoint,
        ScanInputs {
            source: &mut service,
            binding: &changed.binding,
            project: &project,
            item_id: &item_id,
            rate_advice: &advice,
            control: &control,
            clock: &changed.clock,
        },
    );
    assert_eq!(
        refused.err().map(|e| e.code),
        Some(ErrorCode::CursorMismatch)
    );
    assert_eq!(service.fetches, fetches);
    let control = fixture.control(&CancellationToken::default())?;
    let store = fixture.store(&control)?;
    let wrong = format!("sha256:{}", "0".repeat(64));
    assert_eq!(
        load(&store, &published.checkpoint.name, &wrong)
            .err()
            .map(|e| e.code),
        Some(ErrorCode::CursorMismatch)
    );
    let mut tampered = first.checkpoint.clone();
    tampered.totals.items += 1;
    assert_eq!(
        tampered.validate().err().map(|e| e.code),
        Some(ErrorCode::CursorCorrupt)
    );
    let mut claims = first.checkpoint.clone();
    claims.outcome = ScanOutcome::Complete;
    assert!(
        claims.validate().is_err(),
        "a partial scan cannot claim completeness"
    );
    Ok(())
}

#[test]
fn an_oversized_first_record_is_an_error_not_a_skip() -> TestResult {
    let fixture = Fixture::new()?;
    let mut service = SyntheticService::new(Style::Token, 5, dataset(3, 2_000));
    let start = fixture.start(Fixture::caps(1_000, 100, 1_024))?;
    let (advanced, _) = fixture.run(&start, &mut service, &CancellationToken::default())?;
    assert_eq!(
        advanced.error.as_ref().map(|e| e.code),
        Some(ErrorCode::ItemTooLarge)
    );
    assert_eq!(advanced.checkpoint.totals.items, 0);
    assert_eq!(advanced.checkpoint.remainder.len(), 3);
    Ok(())
}
