//! Linux Secret Service items. Locked collections and items are reported as locked; this
//! adapter never asks the service to unlock anything, so no prompt can appear.
use std::collections::HashMap;

use craft_core::storage::{SecretBytes, StoreError};
use secret_service::blocking::SecretService;
use secret_service::{EncryptionType, Error};

/// Attribute that holds the hashed per-installation item identity.
const ATTRIBUTE: &str = "craft_item";
const LABEL: &str = "Craft AI Factory credential";

fn connect() -> Result<SecretService<'static>, StoreError> {
    SecretService::connect(EncryptionType::Dh).map_err(|error| native_error(error, false))
}

pub(super) fn native_error(error: Error, mutating: bool) -> StoreError {
    match error {
        Error::Locked | Error::Prompt | Error::PromptDisconnected => StoreError::Locked,
        Error::Unavailable => StoreError::Unavailable,
        _ if mutating => StoreError::OutcomeUnknown,
        _ => StoreError::Unavailable,
    }
}

/// True when the login collection is locked, which hides whether any item exists.
pub(super) fn locked() -> Result<bool, StoreError> {
    let service = connect()?;
    persistent(&service)?;
    service
        .get_default_collection()
        .and_then(|collection| collection.is_locked())
        .map_err(|error| native_error(error, false))
}

fn attributes(target: &str) -> HashMap<&str, &str> {
    HashMap::from([(ATTRIBUTE, target)])
}

/// The default collection must be the persistent login collection, not the session alias.
fn persistent(service: &SecretService<'_>) -> Result<(), StoreError> {
    let default = service
        .get_default_collection()
        .map_err(|error| native_error(error, false))?;
    match service.get_collection_by_alias("session") {
        Ok(session) if session.collection_path == default.collection_path => {
            Err(StoreError::Unavailable)
        }
        _ => Ok(()),
    }
}

pub(super) fn read(target: &str, limit: usize) -> Result<Option<SecretBytes>, StoreError> {
    let service = connect()?;
    let found = service
        .search_items(attributes(target))
        .map_err(|error| native_error(error, false))?;
    if !found.locked.is_empty() {
        return Err(StoreError::Locked);
    }
    match found.unlocked.as_slice() {
        [] => Ok(None),
        [item] => {
            let bytes = item
                .get_secret()
                .map_err(|error| native_error(error, false))?;
            let secret = SecretBytes::new(bytes).map_err(|_| StoreError::Corrupt)?;
            if secret.expose().len() > limit {
                return Err(StoreError::Corrupt);
            }
            Ok(Some(secret))
        }
        _ => Err(StoreError::Conflict),
    }
}

pub(super) fn write(target: &str, secret: &SecretBytes) -> Result<(), StoreError> {
    let service = connect()?;
    persistent(&service)?;
    let collection = service
        .get_default_collection()
        .map_err(|error| native_error(error, false))?;
    if collection
        .is_locked()
        .map_err(|error| native_error(error, false))?
    {
        return Err(StoreError::Locked);
    }
    let found = service
        .search_items(attributes(target))
        .map_err(|error| native_error(error, false))?;
    if !found.locked.is_empty() {
        return Err(StoreError::Locked);
    }
    if found.unlocked.len() > 1 {
        return Err(StoreError::Conflict);
    }
    collection
        .create_item(
            LABEL,
            attributes(target),
            secret.expose(),
            true,
            "application/octet-stream",
        )
        .map(|_| ())
        .map_err(|error| native_error(error, true))
}

pub(super) fn delete(target: &str) -> Result<(), StoreError> {
    let service = connect()?;
    let found = service
        .search_items(attributes(target))
        .map_err(|error| native_error(error, false))?;
    if !found.locked.is_empty() {
        return Err(StoreError::Locked);
    }
    for item in &found.unlocked {
        item.delete().map_err(|error| native_error(error, true))?;
    }
    Ok(())
}
