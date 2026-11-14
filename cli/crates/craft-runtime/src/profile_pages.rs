//! Immutable local continuation state. Cached pages preserve observations, not live permissions.

use craft_core::contracts::{Page, ProviderRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::parse_object;
use craft_core::profiles::{ProfileSummary, validate_profile_name};
use craft_core::storage::Clock;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::profiles::{
    ProfileStore, begin, commit, invalid_state, is_opaque_id, random_id, state_error,
};

const CURSOR_TTL_MS: u64 = 900_000;
const MAX_CURSOR_BYTES: usize = 131_072;
const MAX_CURSORS: i64 = 256;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorState {
    installation_id: String,
    binding: String,
    expires: u64,
    observed_at: u64,
    items: Vec<ProfileSummary>,
    after: Option<String>,
    native_more: bool,
}

pub struct ProfileListRequest<'a> {
    pub cursor: Option<&'a str>,
    /// Digest of command/schema and effective projection, excluding adjustable output budgets.
    pub binding: &'a str,
    pub limit: u16,
}

pub struct ProfilePage {
    pub items: Vec<ProfileSummary>,
    pub page: Page,
    pub observed_at: u64,
    pub cached: bool,
}

fn cursor_error(rule: &str) -> CraftError {
    let mut error = CraftError::field("/cursor", rule);
    error.next_action = "Use the original state root, command and fields; start an explicit new traversal if the cursor expired or was lost.".into();
    error
}

impl CursorState {
    fn validate(&self, binding: &str, installation: &str, now: u64) -> Result<(), CraftError> {
        if self.installation_id != installation {
            return Err(cursor_error("cursor_foreign"));
        }
        if self.binding != binding {
            return Err(cursor_error("cursor_binding_mismatch"));
        }
        if self.expires <= now {
            return Err(cursor_error("cursor_expired"));
        }
        if self.expires
            != self
                .observed_at
                .checked_add(CURSOR_TTL_MS)
                .ok_or_else(|| cursor_error("cursor_corrupt"))?
            || self.observed_at > now
            || self.items.len() > 101
            || (self.native_more && self.after.is_none())
        {
            return Err(cursor_error("cursor_corrupt"));
        }
        if let Some(name) = &self.after {
            validate_profile_name(name).map_err(|_| cursor_error("cursor_corrupt"))?;
        }
        let mut previous: Option<&str> = None;
        for item in &self.items {
            validate_profile_name(&item.name).map_err(|_| cursor_error("cursor_corrupt"))?;
            ProviderRef {
                provider: item.provider.clone(),
                deployment: item.deployment.clone(),
            }
            .validate()
            .map_err(|_| cursor_error("cursor_corrupt"))?;
            if let Some(tenant) = &item.tenant {
                validate_identity("/tenant", tenant).map_err(|_| cursor_error("cursor_corrupt"))?;
            }
            if previous.is_some_and(|name| name >= item.name.as_str())
                || self.after.as_ref().is_none_or(|after| &item.name > after)
            {
                return Err(cursor_error("cursor_corrupt"));
            }
            previous = Some(&item.name);
        }
        Ok(())
    }
}

impl ProfileStore {
    /// Fetches at most one native page, caching every item omitted by item or byte limits.
    /// `fits` measures the exact projected envelope, including the proposed continuation handle.
    pub fn list_page(
        &self,
        request: ProfileListRequest<'_>,
        clock: &dyn Clock,
        control: &InvocationControl,
        mut fits: impl FnMut(&ProfilePage) -> Result<bool, CraftError>,
    ) -> Result<ProfilePage, CraftError> {
        control.check()?;
        validate_identity("/cursor_binding", request.binding)?;
        if !(1..=100).contains(&request.limit) {
            return Err(CraftError::field("/limit", "limit_out_of_range"));
        }
        let now = clock.unix_millis();
        let sql_now = i64::try_from(now).map_err(|_| invalid_state())?;
        let mut cached = false;
        let mut state = if let Some(handle) = request.cursor {
            if !is_opaque_id(handle, "cur_") {
                return Err(cursor_error("cursor_invalid"));
            }
            let stored = self
                .connection
                .query_row(
                    "SELECT expires,payload FROM profile_cursors WHERE handle=?1",
                    [handle],
                    |row| {
                        let expires = u64::try_from(row.get::<_, i64>(0)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?;
                        let payload = row
                            .get_ref(1)?
                            .as_str()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?;
                        if payload.len() > MAX_CURSOR_BYTES {
                            return Err(rusqlite::Error::InvalidQuery);
                        }
                        let state: CursorState = serde_json::from_value(
                            parse_object(payload.as_bytes())
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        )
                        .map_err(|_| rusqlite::Error::InvalidQuery)?;
                        Ok((expires, state))
                    },
                )
                .optional()
                .map_err(|_| cursor_error("cursor_corrupt"))?;
            let (expires, state) =
                stored.ok_or_else(|| cursor_error("cursor_missing_or_foreign"))?;
            if expires != state.expires {
                return Err(cursor_error("cursor_corrupt"));
            }
            state.validate(request.binding, &self.settings().installation_id, now)?;
            cached = true;
            state
        } else {
            CursorState {
                installation_id: self.settings().installation_id.clone(),
                binding: request.binding.into(),
                expires: 0,
                observed_at: 0,
                items: Vec::new(),
                after: None,
                native_more: true,
            }
        };
        if state.items.is_empty() && state.native_more {
            state.items = self.summaries_after(state.after.as_deref(), control)?;
            state.after = state.items.last().map(|item| item.name.clone());
            state.native_more = state.items.len() == 101;
            state.observed_at = now;
            state.expires = now.checked_add(CURSOR_TTL_MS).ok_or_else(invalid_state)?;
            cached = false;
        }
        let handle = random_id("cur_")?;
        let mut count = usize::from(request.limit).min(state.items.len());
        let mut byte_limited = false;
        let output = loop {
            let has_more = count < state.items.len() || state.native_more;
            let output = ProfilePage {
                items: state.items[..count].to_vec(),
                observed_at: state.observed_at,
                cached,
                page: Page {
                    returned: count,
                    has_more,
                    next_cursor: has_more.then(|| handle.clone()),
                    truncated: count < state.items.len(),
                    stop_reason: if byte_limited {
                        "byte_limit"
                    } else if count < state.items.len() {
                        "item_limit"
                    } else if state.native_more {
                        "native_page"
                    } else {
                        "exhausted"
                    }
                    .into(),
                    consistency: "live".into(),
                },
            };
            if fits(&output)? {
                break output;
            }
            if count == 0 || (count == 1 && !state.items.is_empty()) {
                let mut error = CraftError::new(
                    ErrorCode::OutputBudgetTooSmall,
                    "Output budget cannot carry one complete profile and continuation.",
                );
                error.next_action = "Increase --max-bytes or select fewer fields.".into();
                return Err(error);
            }
            count -= 1;
            byte_limited = true;
        };
        if output.page.has_more {
            state.items.drain(..count);
            let payload = serde_json::to_string(&state).map_err(|_| invalid_state())?;
            if payload.len() > MAX_CURSOR_BYTES {
                return Err(cursor_error("cursor_state_too_large"));
            }
            let transaction = begin(&self.connection, control)?;
            transaction
                .execute("DELETE FROM profile_cursors WHERE expires<=?1", [sql_now])
                .map_err(state_error)?;
            let cursors: i64 = transaction
                .query_row("SELECT count(*) FROM profile_cursors", [], |row| row.get(0))
                .map_err(state_error)?;
            if cursors >= MAX_CURSORS {
                return Err(cursor_error("cursor_capacity_reached"));
            }
            transaction
                .execute(
                    "INSERT INTO profile_cursors(handle,expires,payload) VALUES(?1,?2,?3)",
                    params![
                        handle,
                        i64::try_from(state.expires).map_err(|_| invalid_state())?,
                        payload
                    ],
                )
                .map_err(state_error)?;
            control.check()?;
            commit(transaction, Some(control))?;
        }
        Ok(output)
    }
}
