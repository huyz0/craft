use std::collections::BTreeSet;

use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::input::{canonical_digest, content_digest, parse_object};
use craft_core::pagination::{NativePosition, PageObservation, valid_digest};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{MAX_HISTORY, MAX_ITEMS, TTL_MS, failure};
use crate::profiles::{ProfileStore, begin, commit, is_opaque_id, state_error};

const MAX_PAYLOAD: usize = 1_048_576;
const MAX_CURSORS: i64 = 256;
const MAX_TOTAL_BYTES: i64 = 16_777_216;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CursorState {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "craft_core::pagination::deserialize_page_context"
    )]
    pub(super) context: Option<Value>,
    format_version: u32,
    installation_id: String,
    binding: String,
    created_at: u64,
    expires: u64,
    pub(super) observation: PageObservation,
    pub(super) items: Vec<Value>,
    pub(super) next: Option<NativePosition>,
    pub(super) seen: Vec<String>,
}

impl CursorState {
    pub(super) fn new(
        installation_id: String,
        binding: String,
        now: u64,
    ) -> Result<Self, CraftError> {
        let expires = now
            .checked_add(TTL_MS)
            .filter(|expires| *expires <= i64::MAX as u64)
            .ok_or_else(|| failure(ErrorCode::CursorCorrupt))?;
        Ok(Self {
            context: None,
            format_version: 1,
            installation_id,
            binding,
            created_at: now,
            expires,
            observation: PageObservation {
                observed_at: now,
                page_digest: String::new(),
            },
            items: Vec::new(),
            next: None,
            seen: Vec::new(),
        })
    }

    pub(super) fn validate_time(&self, now: u64) -> Result<(), CraftError> {
        if self.expires
            != self
                .created_at
                .checked_add(TTL_MS)
                .ok_or_else(|| failure(ErrorCode::CursorCorrupt))?
            || self.created_at > self.observation.observed_at
            || self.observation.observed_at > now
        {
            return Err(failure(ErrorCode::CursorCorrupt));
        }
        if self.expires <= now {
            return Err(failure(ErrorCode::CursorExpired));
        }
        Ok(())
    }

    fn validate(&self, installation: &str, binding: &str, now: u64) -> Result<(), CraftError> {
        if self.format_version != 1
            || !super::valid_context(self.context.as_ref())
            || !is_opaque_id(&self.installation_id, "ins_")
            || !valid_digest(&self.binding)
            || !valid_digest(&self.observation.page_digest)
            || self.seen.is_empty()
            || self.seen.len() > MAX_HISTORY
            || self.seen.iter().any(|digest| !valid_digest(digest))
            || self.seen.iter().collect::<BTreeSet<_>>().len() != self.seen.len()
            || self.items.len() > MAX_ITEMS
            || self.items.iter().any(|item| !item.is_object())
            || (self.items.is_empty() && self.next.is_none())
        {
            return Err(failure(ErrorCode::CursorCorrupt));
        }
        if let Some(next) = &self.next {
            next.validate()
                .map_err(|_| failure(ErrorCode::CursorCorrupt))?;
            let digest = canonical_digest(
                &serde_json::to_value(Some(next)).map_err(|_| failure(ErrorCode::CursorCorrupt))?,
            )?;
            if self.seen.contains(&digest) {
                return Err(failure(ErrorCode::CursorCorrupt));
            }
        }
        self.validate_time(now)?;
        if self.installation_id != installation {
            return Err(failure(ErrorCode::CursorUnavailable));
        }
        if self.binding != binding {
            return Err(failure(ErrorCode::CursorMismatch));
        }
        Ok(())
    }
}

impl ProfileStore {
    pub(super) fn load_provider_cursor(
        &self,
        handle: &str,
        binding: &str,
        now: u64,
    ) -> Result<CursorState, CraftError> {
        if !is_opaque_id(handle, "cur_") {
            return Err(failure(ErrorCode::CursorInvalid));
        }
        let stored = self
            .connection
            .query_row(
                "SELECT expires,digest,payload FROM provider_cursors WHERE handle=?1",
                [handle],
                |row| {
                    let expires = u64::try_from(row.get::<_, i64>(0)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?;
                    let digest = row
                        .get_ref(1)?
                        .as_str()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?;
                    let payload = row
                        .get_ref(2)?
                        .as_str()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?;
                    if payload.len() > MAX_PAYLOAD
                        || !valid_digest(digest)
                        || content_digest(payload.as_bytes()) != digest
                    {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    let state: CursorState = serde_json::from_value(
                        parse_object(payload.as_bytes())
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    )
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                    if expires != state.expires {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    Ok(state)
                },
            )
            .optional()
            .map_err(|_| failure(ErrorCode::CursorCorrupt))?
            .ok_or_else(|| failure(ErrorCode::CursorUnavailable))?;
        stored.validate(&self.settings().installation_id, binding, now)?;
        Ok(stored)
    }

    pub(super) fn save_provider_cursor(
        &self,
        handle: &str,
        state: &CursorState,
        now: u64,
        control: &InvocationControl,
    ) -> Result<(), CraftError> {
        state.validate(&self.settings().installation_id, &state.binding, now)?;
        let payload =
            serde_json::to_string(state).map_err(|_| failure(ErrorCode::CursorCorrupt))?;
        if payload.len() > MAX_PAYLOAD {
            return Err(failure(ErrorCode::CursorCapacity));
        }
        // Apply the same bounded reader contract before publishing bytes another process must read.
        parse_object(payload.as_bytes()).map_err(|_| failure(ErrorCode::CursorCapacity))?;
        let transaction = begin(&self.connection, control)?;
        transaction
            .execute(
                "DELETE FROM provider_cursors WHERE expires<=?1",
                [i64::try_from(now).map_err(|_| failure(ErrorCode::CursorCorrupt))?],
            )
            .map_err(state_error)?;
        let (count, bytes): (i64,i64) = transaction.query_row("SELECT count(*),coalesce(sum(length(CAST(payload AS BLOB))),0) FROM provider_cursors", [], |row| Ok((row.get(0)?,row.get(1)?))).map_err(state_error)?;
        if count >= MAX_CURSORS
            || bytes
                .checked_add(payload.len() as i64)
                .is_none_or(|total| total > MAX_TOTAL_BYTES)
        {
            return Err(failure(ErrorCode::CursorCapacity));
        }
        transaction
            .execute(
                "INSERT INTO provider_cursors(handle,expires,digest,payload) VALUES(?1,?2,?3,?4)",
                params![
                    handle,
                    i64::try_from(state.expires).map_err(|_| failure(ErrorCode::CursorCorrupt))?,
                    content_digest(payload.as_bytes()),
                    payload
                ],
            )
            .map_err(state_error)?;
        control.check()?;
        commit(transaction, Some(control)).map_err(|error| {
            if error.code == ErrorCode::WriteOutcomeUnknown {
                let mut failure = CraftError::new(ErrorCode::Internal, "Cursor publication acknowledgment is unknown; no result page was released.");
                failure.next_action = "Retry the read with the original cursor, which was not consumed. A newly persisted unreachable cursor will expire normally.".into();
                failure
            } else {
                error
            }
        })
    }
}
