//! Immutable provider continuation over private SQLite state, independent of provider routing.

mod state;

use craft_core::contracts::Page;
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::{canonical_digest, parse_object};
use craft_core::pagination::{ListPage, ListRequest, NativePage, PageObservation, PageSource};
use craft_core::storage::Clock;

use crate::profiles::{ProfileStore, is_opaque_id, random_id};
use state::CursorState;

pub(super) const TABLE: &str = "CREATE TABLE provider_cursors (handle TEXT PRIMARY KEY COLLATE BINARY, expires INTEGER NOT NULL CHECK(expires>=0), digest TEXT NOT NULL CHECK(length(digest)=71), payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=1048576))";
const TTL_MS: u64 = 900_000;
const MAX_ITEMS: usize = 1_000;
const MAX_OUTPUT_ITEMS: u16 = 100;
const MAX_NATIVE_BYTES: usize = 524_288;
const MAX_HISTORY: usize = 256;

fn failure(code: ErrorCode) -> CraftError {
    let message = match code {
        ErrorCode::CursorInvalid => "Cursor must be an opaque handle issued by this state store.",
        ErrorCode::CursorUnavailable => "Cursor is missing or belongs to another installation.",
        ErrorCode::CursorExpired => "Cursor traversal has expired.",
        ErrorCode::CursorMismatch => "Cursor does not match the resolved list semantics.",
        ErrorCode::CursorCorrupt => "Stored cursor failed integrity or structure validation.",
        ErrorCode::CursorCapacity => {
            "Provider continuation exceeds the bounded cursor store capacity."
        }
        ErrorCode::PaginationStalled => {
            "Provider continuation repeated a previously requested position."
        }
        ErrorCode::ItemTooLarge => "The first complete item cannot fit the selected output budget.",
        ErrorCode::OutputBudgetTooSmall => {
            "Output budget cannot carry the complete paging recovery envelope."
        }
        _ => "Provider continuation could not be completed.",
    };
    let mut error = CraftError::new(code, message);
    error.next_action = match code {
        ErrorCode::ItemTooLarge | ErrorCode::OutputBudgetTooSmall => "Increase --max-bytes, retaining the recovery cursor when provided; changing selected fields requires an explicit new traversal.",
        ErrorCode::CursorCapacity => "Retain the original cursor and wait for expired state to be reclaimed, or explicitly start a new traversal with narrower filters or smaller provider pages.",
        ErrorCode::CursorMismatch => "Resume with the original command, target, query, identity and fields, or explicitly start a new traversal.",
        _ => "Inspect the original state root and provider continuation; start a new traversal explicitly if recovery is unavailable. No traversal was restarted automatically.",
    }.into();
    error
}

fn valid_context(context: Option<&serde_json::Value>) -> bool {
    context.is_none_or(|value| {
        value.is_object() && serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= 32_768)
    })
}

fn native_page(page: NativePage) -> Result<NativePage, CraftError> {
    if !valid_context(page.context.as_ref()) {
        return Err(failure(ErrorCode::CursorCapacity));
    }
    if page.items.len() > MAX_ITEMS || page.items.iter().any(|item| !item.is_object()) {
        return Err(failure(ErrorCode::CursorCapacity));
    }
    if let Some(next) = &page.next {
        next.validate().map_err(|_| {
            CraftError::new(
                ErrorCode::RemoteUnavailable,
                "Provider returned invalid native continuation.",
            )
        })?;
    }
    let bytes = serde_json::to_vec(&page).map_err(|_| failure(ErrorCode::CursorCorrupt))?;
    if bytes.len() > MAX_NATIVE_BYTES {
        return Err(failure(ErrorCode::CursorCapacity));
    }
    parse_object(&bytes).map_err(|_| {
        CraftError::new(
            ErrorCode::RemoteUnavailable,
            "Normalized provider page exceeds JSON structure bounds.",
        )
    })?;
    Ok(page)
}

/// Conservative fixed control sizes for host preflight before state/auth effects.
pub fn recovery_probe() -> ListPage {
    ListPage {
        context: None,
        items: Vec::new(),
        observation: PageObservation {
            observed_at: u64::MAX,
            page_digest: format!("sha256:{}", "f".repeat(64)),
        },
        cached: false,
        error: Some(failure(ErrorCode::ItemTooLarge)),
        page: Page {
            returned: usize::from(MAX_OUTPUT_ITEMS),
            has_more: true,
            next_cursor: Some(format!("cur_{}", "f".repeat(32))),
            truncated: false,
            stop_reason: "item_too_large".into(),
            consistency: "live".into(),
        },
    }
}

fn list_binding(request: &ListRequest<'_>) -> Result<String, CraftError> {
    let binding = request.binding.digest()?;
    if !(1..=MAX_OUTPUT_ITEMS).contains(&request.limit) {
        return Err(CraftError::field("/limit", "limit_out_of_range"));
    }
    if !is_opaque_id(&request.binding.grant.id, "gr_") {
        return Err(CraftError::field(
            "/grant",
            "expected_issued_grant_identity",
        ));
    }
    Ok(binding)
}

fn output(
    state: &CursorState,
    handle: &str,
    count: usize,
    cached: bool,
    byte_limited: bool,
    oversized: bool,
) -> ListPage {
    let has_more = count < state.items.len() || state.next.is_some();
    ListPage {
        context: state.context.clone(),
        items: state.items[..count].to_vec(),
        observation: state.observation.clone(),
        cached,
        error: oversized.then(|| failure(ErrorCode::ItemTooLarge)),
        page: Page {
            returned: count,
            has_more,
            next_cursor: has_more.then(|| handle.into()),
            truncated: count < state.items.len(),
            stop_reason: if oversized {
                "item_too_large"
            } else if byte_limited {
                "byte_limit"
            } else if count < state.items.len() {
                "item_limit"
            } else if state.next.is_some() {
                "native_page"
            } else {
                "exhausted"
            }
            .into(),
            consistency: "live".into(),
        },
    }
}

impl ProfileStore {
    /// Read-only cursor checks before credential acquisition; execution rechecks the same state.
    pub fn check_provider_cursor(
        &self,
        request: &ListRequest<'_>,
        clock: &dyn Clock,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        control.check()?;
        let binding = list_binding(request)?;
        if let Some(cursor) = request.cursor {
            self.load_provider_cursor(cursor, &binding, clock.unix_millis())?;
        }
        control.check()
    }
    /// `fits` must measure the complete projected response, including error and page control data,
    /// and be monotone for growing prefixes with continuation. It must not publish or perform I/O.
    /// The trusted source rechecks access even for a buffered replay. No cursor grants authority.
    pub fn provider_page(
        &self,
        request: ListRequest<'_>,
        clock: &dyn Clock,
        control: &InvocationControl,
        source: &mut dyn PageSource,
        mut fits: impl FnMut(&ListPage) -> Result<bool, CraftError>,
    ) -> Result<ListPage, CraftError> {
        control.check()?;
        let binding = list_binding(&request)?;
        let now = clock.unix_millis();
        let mut state = if let Some(handle) = request.cursor {
            self.load_provider_cursor(handle, &binding, now)?
        } else {
            CursorState::new(self.settings().installation_id.clone(), binding, now)?
        };
        let handle = random_id("cur_")?;
        // Reserve the error/continuation form before any access check or resource fetch.
        // All actual timestamps and returned counts fit in these fixed-width maxima.
        let preflight = recovery_probe();
        if !fits(&preflight)? {
            return Err(failure(ErrorCode::OutputBudgetTooSmall));
        }
        control.check()?;
        source.check_access(request.binding, control)?;
        control.check()?;
        let cached = !state.items.is_empty();
        if !cached {
            if state.seen.len() >= MAX_HISTORY {
                return Err(failure(ErrorCode::CursorCapacity));
            }
            let position = canonical_digest(
                &serde_json::to_value(&state.next)
                    .map_err(|_| failure(ErrorCode::CursorCorrupt))?,
            )?;
            if state.seen.contains(&position) {
                return Err(failure(ErrorCode::PaginationStalled));
            }
            let page = native_page(source.fetch(request.binding, state.next.as_ref(), control)?)?;
            control.check()?;
            state.seen.push(position);
            if let Some(next) = &page.next {
                let digest = canonical_digest(
                    &serde_json::to_value(Some(next))
                        .map_err(|_| failure(ErrorCode::CursorCorrupt))?,
                )?;
                if state.seen.contains(&digest) {
                    return Err(failure(ErrorCode::PaginationStalled));
                }
            }
            state.observation = PageObservation {
                observed_at: clock.unix_millis(),
                page_digest: canonical_digest(
                    &serde_json::to_value(&page).map_err(|_| failure(ErrorCode::CursorCorrupt))?,
                )?,
            };
            state.items = page.items;
            state.context = page.context;
            state.next = page.next;
        }
        let maximum = usize::from(request.limit).min(state.items.len());
        let mut selected = output(&state, &handle, maximum, cached, false, false);
        if !fits(&selected)? {
            control.check()?;
            // Once the full prefix fails, all smaller nonempty prefixes have continuation.
            let mut low = 1;
            let mut high = maximum.saturating_sub(1);
            let mut best = None;
            while low <= high {
                control.check()?;
                let middle = low + (high - low) / 2;
                let candidate = output(&state, &handle, middle, cached, true, false);
                if fits(&candidate)? {
                    best = Some(candidate);
                    low = middle + 1;
                } else {
                    high = middle - 1;
                }
            }
            selected = match best {
                Some(page) => page,
                None if !state.items.is_empty() => output(&state, &handle, 0, cached, true, true),
                None => return Err(failure(ErrorCode::OutputBudgetTooSmall)),
            };
            if !fits(&selected)? {
                return Err(failure(ErrorCode::OutputBudgetTooSmall));
            }
        }
        control.check()?;
        source.check_access(request.binding, control)?;
        control.check()?;
        let final_now = clock.unix_millis();
        state.validate_time(final_now)?;
        if selected.page.has_more {
            state.items.drain(..selected.items.len());
            self.save_provider_cursor(&handle, &state, final_now, control)?;
        }
        control.check()?;
        Ok(selected)
    }
}
