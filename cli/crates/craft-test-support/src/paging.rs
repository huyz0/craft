//! Synthetic native list services in each provider paging style, with scripted faults.
//!
//! The service is deliberately dumb: it serves the dataset as it is at fetch time, so inserting
//! or deleting between fetches produces the duplicates and gaps a live provider can produce.
use std::collections::BTreeMap;

use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::pagination::{ListBinding, NativePage, NativePosition, PageSource};
use serde_json::{Value, json};

pub mod contract;

/// How the native service advertises its next page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// An opaque server token (Jira issue search `nextPageToken`).
    Token,
    /// A numeric offset (Jira `startAt` with `isLast`).
    Offset,
    /// A next link whose query carries an opaque cursor (Confluence v2 `_links.next`).
    NextLink,
}

pub const ALL_STYLES: [Style; 3] = [Style::Token, Style::Offset, Style::NextLink];

/// A scripted deviation applied to the native page with the given zero-based fetch index.
#[derive(Debug, Clone)]
pub enum Fault {
    RateLimited,
    Cancelled,
    Failure,
    /// Serve an empty page that advertises a fresh position for the same offset.
    Empty,
    /// Advertise the position that was requested, so traversal cannot progress.
    RepeatPosition,
    /// Insert a record at the front of the collection before serving.
    InsertFront(String),
    /// Delete the record with this ID, or the first record when empty, before serving.
    Delete(String),
}

pub struct SyntheticService {
    pub style: Style,
    pub page_size: usize,
    pub records: Vec<Value>,
    pub faults: BTreeMap<usize, Fault>,
    pub fetches: usize,
    pub access_checks: usize,
    /// Positions requested, in order, as the service received them.
    pub requested: Vec<Option<NativePosition>>,
    pub binding: Option<ListBinding>,
}

pub fn record(id: &str, padding: usize) -> Value {
    json!({"id": id, "title": format!("Record {id}"), "body": "x".repeat(padding)})
}

pub fn dataset(count: usize, padding: usize) -> Vec<Value> {
    (0..count)
        .map(|index| record(&format!("{:04}", index + 1), padding))
        .collect()
}

fn token(offset: usize) -> String {
    // Opaque to clients; the service decodes it back to an offset.
    format!("tok-{:x}", offset * 7919 + 13)
}

fn decode_token(value: &str) -> Option<usize> {
    let raw = usize::from_str_radix(value.strip_prefix("tok-")?, 16).ok()?;
    raw.checked_sub(13)
        .filter(|value| value % 7919 == 0)
        .map(|value| value / 7919)
}

impl SyntheticService {
    pub fn new(style: Style, page_size: usize, records: Vec<Value>) -> Self {
        Self {
            style,
            page_size,
            records,
            faults: BTreeMap::new(),
            fetches: 0,
            access_checks: 0,
            requested: Vec::new(),
            binding: None,
        }
    }

    pub fn with_fault(mut self, fetch: usize, fault: Fault) -> Self {
        self.faults.insert(fetch, fault);
        self
    }

    fn offset(&self, position: Option<&NativePosition>) -> Result<usize, CraftError> {
        let Some(position) = position else {
            return Ok(0);
        };
        let invalid = || CraftError::new(ErrorCode::CursorCorrupt, "Unknown native position.");
        match self.style {
            Style::Token => decode_token(position.0.get("nextPageToken").ok_or_else(invalid)?)
                .ok_or_else(invalid),
            Style::Offset => position
                .0
                .get("startAt")
                .and_then(|value| value.parse().ok())
                .ok_or_else(invalid),
            Style::NextLink => position
                .0
                .get("cursor")
                .and_then(|value| value.strip_prefix("c:"))
                .and_then(|value| value.parse().ok())
                .ok_or_else(invalid),
        }
    }

    fn position(&self, offset: usize) -> NativePosition {
        NativePosition(BTreeMap::from([match self.style {
            Style::Token => ("nextPageToken".into(), token(offset)),
            Style::Offset => ("startAt".into(), offset.to_string()),
            Style::NextLink => ("cursor".into(), format!("c:{offset}")),
        }]))
    }

    /// Serves one native page as the adapter would normalize it.
    pub fn serve(&mut self, position: Option<&NativePosition>) -> Result<NativePage, CraftError> {
        let index = self.fetches;
        self.fetches += 1;
        self.requested.push(position.cloned());
        let offset = self.offset(position)?;
        match self.faults.get(&index).cloned() {
            Some(Fault::RateLimited) => {
                return Err(CraftError::new(
                    ErrorCode::RateLimited,
                    "Synthetic rate limit.",
                ));
            }
            Some(Fault::Cancelled) => {
                return Err(CraftError::new(
                    ErrorCode::Cancelled,
                    "Synthetic cancellation.",
                ));
            }
            Some(Fault::Failure) => {
                return Err(CraftError::new(
                    ErrorCode::RemoteUnavailable,
                    "Synthetic failure.",
                ));
            }
            Some(Fault::Empty) => {
                // A distinct position for the same offset, so only the empty run is at fault.
                let mut next = self.position(offset);
                next.0.insert("probe".into(), index.to_string());
                return Ok(NativePage {
                    context: None,
                    items: Vec::new(),
                    next: Some(next),
                });
            }
            Some(Fault::RepeatPosition) => {
                return Ok(NativePage {
                    context: None,
                    items: Vec::new(),
                    next: Some(position.cloned().unwrap_or_else(|| self.position(0))),
                });
            }
            Some(Fault::InsertFront(id)) => self.records.insert(0, record(&id, 0)),
            Some(Fault::Delete(id)) if id.is_empty() => {
                if !self.records.is_empty() {
                    self.records.remove(0);
                }
            }
            Some(Fault::Delete(id)) => self.records.retain(|item| item["id"] != id),
            None => {}
        }
        let end = (offset + self.page_size).min(self.records.len());
        let items = self.records.get(offset..end).unwrap_or(&[]).to_vec();
        Ok(NativePage {
            context: None,
            items,
            next: (end < self.records.len()).then(|| self.position(end)),
        })
    }
}

impl PageSource for SyntheticService {
    fn check_access(
        &mut self,
        binding: &ListBinding,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        control.check()?;
        self.access_checks += 1;
        if self.binding.as_ref().is_some_and(|bound| bound != binding) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Synthetic binding changed.",
            ));
        }
        self.binding = Some(binding.clone());
        Ok(())
    }

    fn fetch(
        &mut self,
        binding: &ListBinding,
        position: Option<&NativePosition>,
        control: &InvocationControl,
    ) -> Result<NativePage, CraftError> {
        self.check_access(binding, control)?;
        self.serve(position)
    }
}

/// IDs in emitted order, for losslessness assertions.
pub fn ids(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| item["id"].as_str().map(String::from))
        .collect()
}

impl contract::CountingSource for SyntheticService {
    fn fetches(&self) -> usize {
        self.fetches
    }
}
