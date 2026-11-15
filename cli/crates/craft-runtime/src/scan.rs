//! Bounded complete scans of one list traversal into immutable private artifacts.
//!
//! A scan never writes records to stdout. Each invocation stages one JSON Lines segment and one
//! checkpoint; the checkpoint is the only resumption authority and is published after its
//! segment. Native continuation is followed exactly as the list adapter returns it, a repeated
//! position or a long run of empty nonterminal pages stops as stalled, and only terminal
//! provider metadata makes a scan complete. Traversal is live: repeated IDs are counted, not
//! removed, and missing records cannot be detected.
use std::collections::BTreeSet;
use std::io::Write;
use std::time::Duration;

use craft_core::contracts::validate_identity;
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::RetryAfter;
use craft_core::input::{canonical_digest, content_digest, parse_object};
use craft_core::pagination::{ListBinding, NativePosition, PageSource, valid_digest};
use craft_core::storage::{ArtifactReceipt, ArtifactSpec, ArtifactStore, Clock, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::profiles::random_id;
pub use craft_core::scan::*;

/// Time kept free so the segment and checkpoint can be published inside the deadline.
pub const PUBLISH_RESERVE: Duration = Duration::from_secs(3);
const CHECKPOINT_MAX_BYTES: u64 = 16_777_216;
const FORMAT: u8 = 1;
pub const RECORDS_MEDIA_TYPE: &str = "application/x-ndjson";
pub const CHECKPOINT_MEDIA_TYPE: &str = "application/json";

/// Immutable resumption state. Its digest, not a path, selects it on resume and export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanCheckpoint {
    pub format: u8,
    pub scan_id: String,
    pub revision: u32,
    pub command: String,
    /// The list command's canonical typed arguments, used to rebuild its binding on resume.
    pub arguments: Value,
    pub binding_digest: String,
    pub caps: ScanCaps,
    pub totals: ScanTotals,
    pub segments: Vec<SegmentReceipt>,
    pub started: bool,
    pub next: Option<NativePosition>,
    /// Fetched, projected records that did not fit; served before the next native fetch.
    pub remainder: Vec<Value>,
    pub seen: Vec<String>,
    pub empty_run: u8,
    pub id_hashes: Vec<String>,
    pub outcome: ScanOutcome,
    pub limit: Option<ScanLimit>,
    pub complete: bool,
    pub updated_at: u64,
}

fn corrupt() -> CraftError {
    CraftError::new(
        ErrorCode::CursorCorrupt,
        "Scan checkpoint failed integrity or structure validation.",
    )
}

fn stalled(message: &str) -> CraftError {
    let mut error = CraftError::new(ErrorCode::PaginationStalled, message);
    error.next_action = "The provider continuation did not make progress. The checkpoint keeps every record already written; start a new scan with narrower filters if needed.".into();
    error
}

pub fn segment_name(scan_id: &str, revision: u32) -> String {
    format!("{scan_id}-r{revision:05}.jsonl")
}

pub fn checkpoint_name(scan_id: &str, revision: u32) -> String {
    format!("{scan_id}-r{revision:05}.checkpoint.json")
}

pub fn export_name(scan_id: &str, revision: u32) -> String {
    format!("{scan_id}-r{revision:05}.export.jsonl")
}

fn valid_scan_id(id: &str) -> bool {
    crate::profiles::is_opaque_id(id, "scan_")
}

fn id_hash(id: &str) -> String {
    content_digest(id.as_bytes())[7..23].to_owned()
}

impl ScanCheckpoint {
    pub fn start(
        command: &str,
        arguments: Value,
        binding: &ListBinding,
        caps: ScanCaps,
        now: u64,
    ) -> Result<Self, CraftError> {
        validate_identity("/command", command)?;
        caps.validate()?;
        Ok(Self {
            format: FORMAT,
            scan_id: random_id("scan_")?,
            revision: 0,
            command: command.into(),
            arguments,
            binding_digest: binding.digest()?,
            caps,
            totals: ScanTotals::default(),
            segments: Vec::new(),
            started: false,
            next: None,
            remainder: Vec::new(),
            seen: Vec::new(),
            empty_run: 0,
            id_hashes: Vec::new(),
            outcome: ScanOutcome::BudgetReached,
            limit: None,
            complete: false,
            updated_at: now,
        })
    }

    pub fn validate(&self) -> Result<(), CraftError> {
        let bounded = self.format == FORMAT
            && valid_scan_id(&self.scan_id)
            && validate_identity("/command", &self.command).is_ok()
            && self.arguments.is_object()
            && valid_digest(&self.binding_digest)
            && self.caps.validate().is_ok()
            && self.totals.items <= MAX_ITEMS
            && self.totals.pages <= MAX_PAGES
            && self.totals.stored_bytes <= MAX_STORED_BYTES
            && self.segments.len() <= MAX_PAGES as usize
            && self.seen.len() <= MAX_PAGES as usize
            && self.id_hashes.len() <= MAX_ITEMS as usize
            && self.remainder.len() <= 1_000
            && self.remainder.iter().all(Value::is_object)
            && self.empty_run <= EMPTY_RUN_LIMIT
            && self.seen.iter().all(|digest| valid_digest(digest))
            && self
                .next
                .as_ref()
                .is_none_or(|next| next.validate().is_ok())
            && (self.complete
                == (self.started && self.next.is_none() && self.remainder.is_empty()))
            && (self.complete == (self.outcome == ScanOutcome::Complete));
        let mut items = 0u64;
        let mut bytes = 0u64;
        for (index, segment) in self.segments.iter().enumerate() {
            if !segment.name.starts_with(&self.scan_id)
                || !segment.name.ends_with(".jsonl")
                || !valid_digest(&segment.digest)
                || segment.items == 0
                || index > 0 && segment.name <= self.segments[index - 1].name
            {
                return Err(corrupt());
            }
            items += u64::from(segment.items);
            bytes += segment.bytes;
        }
        if !bounded || items != u64::from(self.totals.items) || bytes != self.totals.stored_bytes {
            return Err(corrupt());
        }
        Ok(())
    }

    fn encode(&self) -> Result<Vec<u8>, CraftError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| corrupt())?;
        if bytes.len() as u64 > CHECKPOINT_MAX_BYTES {
            return Err(CraftError::new(
                ErrorCode::CursorCapacity,
                "Scan checkpoint exceeds its bounded size.",
            ));
        }
        Ok(bytes)
    }
}

fn store_error(error: StoreError) -> CraftError {
    match error {
        StoreError::Cancelled => {
            CraftError::new(ErrorCode::Cancelled, "Scan storage was cancelled.")
        }
        StoreError::DeadlineExceeded => CraftError::new(
            ErrorCode::DeadlineExceeded,
            "Scan storage exceeded the deadline.",
        ),
        StoreError::Conflict => CraftError::new(
            ErrorCode::VersionConflict,
            "A scan artifact with this name already exists; the earlier checkpoint was kept.",
        ),
        StoreError::Corrupt | StoreError::Invalid => corrupt(),
        _ => CraftError::new(ErrorCode::Internal, "Scan artifacts could not be stored."),
    }
}

/// Loads a checkpoint by exact name and observed digest.
pub fn load(
    store: &dyn ArtifactStore,
    name: &str,
    digest: &str,
) -> Result<ScanCheckpoint, CraftError> {
    if !valid_digest(digest) {
        return Err(CraftError::field(
            "/checkpoint_digest",
            "expected_sha256_digest",
        ));
    }
    let blob = store
        .read(name, CHECKPOINT_MAX_BYTES)
        .map_err(store_error)?
        .ok_or_else(|| {
            CraftError::new(
                ErrorCode::CursorUnavailable,
                "Scan checkpoint was not found.",
            )
        })?;
    if blob.receipt.digest != digest || blob.receipt.media_type != CHECKPOINT_MEDIA_TYPE {
        return Err(CraftError::new(
            ErrorCode::CursorMismatch,
            "Scan checkpoint digest differs from the one supplied.",
        ));
    }
    let value = parse_object(&blob.bytes).map_err(|_| corrupt())?;
    let checkpoint: ScanCheckpoint = serde_json::from_value(value).map_err(|_| corrupt())?;
    checkpoint.validate()?;
    if checkpoint_name(&checkpoint.scan_id, checkpoint.revision) != name {
        return Err(corrupt());
    }
    Ok(checkpoint)
}

/// Applies a list command's own projection and validation to normalized records.
pub type Projector<'a> = &'a dyn Fn(&[Value]) -> Result<Vec<Value>, CraftError>;

pub struct ScanInputs<'a> {
    pub source: &'a mut dyn PageSource,
    pub binding: &'a ListBinding,
    pub project: Projector<'a>,
    pub item_id: &'a dyn Fn(&Value) -> Option<String>,
    /// Provider advice from the most recent native response, when the adapter retains it.
    pub rate_advice: &'a dyn Fn() -> Option<RetryAfter>,
    pub control: &'a InvocationControl,
    pub clock: &'a dyn Clock,
}

/// One invocation's progress, not yet published.
pub struct Advanced {
    pub checkpoint: ScanCheckpoint,
    pub segment: Vec<u8>,
    pub segment_items: u32,
    pub error: Option<CraftError>,
    pub retry_after: Option<RetryAfter>,
}

fn position_digest(position: Option<&NativePosition>) -> Result<String, CraftError> {
    canonical_digest(&serde_json::to_value(position).map_err(|_| corrupt())?)
}

/// Advances from `previous` until a cap, terminal metadata or a failure. Fetch failures become
/// the outcome; only a mismatched or already complete checkpoint is refused outright.
pub fn advance(previous: &ScanCheckpoint, inputs: ScanInputs<'_>) -> Result<Advanced, CraftError> {
    previous.validate()?;
    if previous.binding_digest != inputs.binding.digest()? {
        return Err(CraftError::new(
            ErrorCode::CursorMismatch,
            "Scan checkpoint does not match the resolved list semantics.",
        ));
    }
    if previous.complete {
        let mut error =
            CraftError::new(ErrorCode::VersionConflict, "This scan is already complete.");
        error.next_action = "Export the checkpoint, or start a new scan for fresh data.".into();
        return Err(error);
    }
    let mut next = previous.clone();
    next.revision = next.revision.checked_add(1).ok_or_else(corrupt)?;
    next.limit = None;
    let mut ids: BTreeSet<String> = next.id_hashes.iter().cloned().collect();
    let mut segment = Vec::new();
    let mut segment_items = 0u32;
    let mut error = None;
    let mut outcome = ScanOutcome::BudgetReached;
    inputs.source.check_access(inputs.binding, inputs.control)?;
    'scan: loop {
        match inputs.control.remaining() {
            Ok(remaining) if remaining > PUBLISH_RESERVE => {}
            Ok(_) => {
                next.limit = Some(ScanLimit::Time);
                break;
            }
            Err(failure) => {
                outcome = ScanOutcome::Cancelled;
                error = Some(failure);
                break;
            }
        }
        if next.remainder.is_empty() {
            if next.started && next.next.is_none() {
                outcome = ScanOutcome::Complete;
                break;
            }
            if next.totals.pages >= next.caps.max_pages {
                next.limit = Some(ScanLimit::Pages);
                break;
            }
            let position = position_digest(next.next.as_ref())?;
            if next.seen.contains(&position) {
                outcome = ScanOutcome::Error;
                error = Some(stalled(
                    "Provider continuation repeated a requested position.",
                ));
                break;
            }
            let page = match inputs
                .source
                .fetch(inputs.binding, next.next.as_ref(), inputs.control)
            {
                Ok(page) => page,
                Err(failure) => {
                    outcome = match failure.code {
                        ErrorCode::RateLimited => ScanOutcome::RateLimited,
                        ErrorCode::Cancelled | ErrorCode::DeadlineExceeded => {
                            ScanOutcome::Cancelled
                        }
                        _ => ScanOutcome::Error,
                    };
                    error = Some(failure);
                    break;
                }
            };
            next.started = true;
            next.seen.push(position);
            next.totals.pages += 1;
            if let Some(following) = &page.next {
                following.validate().map_err(|_| {
                    CraftError::new(
                        ErrorCode::RemoteUnavailable,
                        "Provider returned invalid native continuation.",
                    )
                })?;
            }
            if page.items.is_empty() {
                next.totals.empty_pages += 1;
                if page.next.is_some() {
                    next.empty_run += 1;
                }
            } else {
                next.empty_run = 0;
            }
            next.next = page.next;
            let projected = (inputs.project)(&page.items)?;
            if projected.len() != page.items.len() || projected.iter().any(|item| !item.is_object())
            {
                return Err(CraftError::new(
                    ErrorCode::Internal,
                    "List projection changed the record count.",
                ));
            }
            next.remainder = projected;
            if next.empty_run >= EMPTY_RUN_LIMIT {
                outcome = ScanOutcome::Error;
                error = Some(stalled(
                    "Provider returned too many consecutive empty pages that still claimed continuation.",
                ));
                break;
            }
            continue;
        }
        while let Some(item) = next.remainder.first() {
            if next.totals.items >= next.caps.max_items {
                next.limit = Some(ScanLimit::Items);
                break 'scan;
            }
            let mut line = serde_json::to_vec(item).map_err(|_| corrupt())?;
            line.push(b'\n');
            if next.totals.stored_bytes + line.len() as u64 > next.caps.max_stored_bytes {
                if next.totals.items == 0 {
                    let mut failure = CraftError::new(
                        ErrorCode::ItemTooLarge,
                        "The first record does not fit the scan's stored-byte cap.",
                    );
                    failure.next_action =
                        "Raise --max-stored-bytes or select fewer fields in the list arguments."
                            .into();
                    outcome = ScanOutcome::Error;
                    error = Some(failure);
                } else {
                    next.limit = Some(ScanLimit::StoredBytes);
                }
                break 'scan;
            }
            if let Some(id) = (inputs.item_id)(item) {
                let hash = id_hash(&id);
                if !ids.insert(hash.clone()) {
                    next.totals.duplicate_ids += 1;
                } else {
                    next.id_hashes.push(hash);
                }
            }
            segment.write_all(&line).map_err(|_| corrupt())?;
            segment_items += 1;
            next.totals.items += 1;
            next.totals.stored_bytes += line.len() as u64;
            next.remainder.remove(0);
        }
    }
    if outcome == ScanOutcome::BudgetReached && next.limit.is_none() {
        next.limit = Some(ScanLimit::Time);
    }
    next.retry_outcome(outcome);
    next.updated_at = inputs.clock.unix_millis();
    let retry_after = if outcome == ScanOutcome::RateLimited {
        (inputs.rate_advice)()
    } else {
        None
    };
    Ok(Advanced {
        checkpoint: next,
        segment,
        segment_items,
        error,
        retry_after,
    })
}

impl ScanCheckpoint {
    fn retry_outcome(&mut self, outcome: ScanOutcome) {
        self.complete = outcome == ScanOutcome::Complete;
        self.outcome = outcome;
        if self.complete {
            self.limit = None;
        }
    }
}

pub struct Published {
    pub checkpoint: ArtifactReceipt,
    pub segment: Option<SegmentReceipt>,
}

fn write(
    store: &dyn ArtifactStore,
    name: String,
    media_type: &str,
    bytes: &[u8],
) -> Result<ArtifactReceipt, CraftError> {
    let mut writer = store
        .begin(ArtifactSpec {
            name,
            media_type: media_type.into(),
            max_bytes: (bytes.len() as u64).max(1),
            source: None,
        })
        .map_err(store_error)?;
    writer.write_all(bytes).map_err(|_| {
        CraftError::new(
            ErrorCode::Internal,
            "Scan artifact bytes could not be staged.",
        )
    })?;
    writer.finish().map_err(store_error)
}

/// Publishes the segment, then the checkpoint that names it. A failure leaves the previous
/// checkpoint as the resumption point; an orphan segment is never referenced.
pub fn publish(
    store: &dyn ArtifactStore,
    advanced: &mut Advanced,
) -> Result<Published, CraftError> {
    let checkpoint = &mut advanced.checkpoint;
    let segment = if advanced.segment.is_empty() {
        None
    } else {
        let name = segment_name(&checkpoint.scan_id, checkpoint.revision);
        let receipt = write(store, name.clone(), RECORDS_MEDIA_TYPE, &advanced.segment)?;
        if receipt.bytes != advanced.segment.len() as u64 {
            return Err(corrupt());
        }
        let segment = SegmentReceipt {
            name,
            digest: receipt.digest,
            bytes: receipt.bytes,
            items: advanced.segment_items,
        };
        checkpoint.segments.push(segment.clone());
        Some(segment)
    };
    let bytes = checkpoint.encode()?;
    let receipt = write(
        store,
        checkpoint_name(&checkpoint.scan_id, checkpoint.revision),
        CHECKPOINT_MEDIA_TYPE,
        &bytes,
    )?;
    Ok(Published {
        checkpoint: receipt,
        segment,
    })
}

/// Builds one verified JSON Lines artifact from every segment, in order, for guarded export.
pub fn assemble(
    store: &dyn ArtifactStore,
    checkpoint: &ScanCheckpoint,
) -> Result<ArtifactReceipt, CraftError> {
    checkpoint.validate()?;
    let name = export_name(&checkpoint.scan_id, checkpoint.revision);
    let mut records = Vec::new();
    for segment in &checkpoint.segments {
        let blob = store
            .read(&segment.name, segment.bytes.max(1))
            .map_err(store_error)?
            .ok_or_else(corrupt)?;
        if blob.receipt.digest != segment.digest || blob.receipt.bytes != segment.bytes {
            return Err(corrupt());
        }
        records.extend_from_slice(&blob.bytes);
    }
    if let Some(existing) = store.receipt(&name).map_err(store_error)? {
        if existing.digest != content_digest(&records) {
            return Err(corrupt());
        }
        return Ok(existing);
    }
    write(store, name, RECORDS_MEDIA_TYPE, &records)
}

#[cfg(test)]
mod tests;
