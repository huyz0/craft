//! Public scan contract values shared by the runtime driver and the CLI envelope.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CraftError;

pub const MAX_ITEMS: u32 = 100_000;
pub const MAX_PAGES: u32 = 10_000;
pub const MIN_STORED_BYTES: u64 = 1_024;
pub const MAX_STORED_BYTES: u64 = 67_108_864;
/// Consecutive empty pages that still advertise continuation before the scan is stalled.
pub const EMPTY_RUN_LIMIT: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanOutcome {
    Complete,
    BudgetReached,
    RateLimited,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanLimit {
    Items,
    Pages,
    StoredBytes,
    Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanCaps {
    #[schemars(range(min = 1, max = 100_000))]
    pub max_items: u32,
    #[schemars(range(min = 1, max = 10_000))]
    pub max_pages: u32,
    #[schemars(range(min = 1_024, max = 67_108_864))]
    pub max_stored_bytes: u64,
}

impl ScanCaps {
    pub fn validate(&self) -> Result<(), CraftError> {
        if !(1..=MAX_ITEMS).contains(&self.max_items) {
            return Err(CraftError::field("/max_items", "scan_items_out_of_range"));
        }
        if !(1..=MAX_PAGES).contains(&self.max_pages) {
            return Err(CraftError::field("/max_pages", "scan_pages_out_of_range"));
        }
        if !(MIN_STORED_BYTES..=MAX_STORED_BYTES).contains(&self.max_stored_bytes) {
            return Err(CraftError::field(
                "/max_stored_bytes",
                "scan_bytes_out_of_range",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SegmentReceipt {
    pub name: String,
    pub digest: String,
    pub bytes: u64,
    pub items: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanTotals {
    pub items: u32,
    pub pages: u32,
    pub stored_bytes: u64,
    /// Records whose identity was already written by this scan. They stay in the output.
    pub duplicate_ids: u32,
    pub empty_pages: u32,
}
