//! Linux Secret Service items in the persistent login collection. Locked collections and
//! items are reported as locked; this adapter never asks the service to unlock anything.
//!
//! Residual risk: the library still runs a prompt if the service answers a create or delete
//! with one. That happens only when the collection is locked between the check and the call,
//! or with a provider that confirms every change (some KeePassXC settings). gnome-keyring with
//! an unlocked login collection does neither.
use std::collections::HashMap;

use craft_core::storage::{SecretBytes, StoreError};
use secret_service::blocking::{Collection, Item, SecretService};
use secret_service::{EncryptionType, Error};

/// Attribute that holds the hashed per-installation item identity.
const ATTRIBUTE: &str = "craft_item";
const LABEL: &str = "Craft AI Factory credential";

fn connect() -> Result<SecretService<'static>, StoreError> {
    SecretService::connect(EncryptionType::Dh).map_err(|error| native_error(error, false))
}

pub(super) fn native_error(error: Error, mutating: bool) -> StoreError {
    match error {
        // After a lost prompt connection the service may or may not have applied the change.
        Error::PromptDisconnected if mutating => StoreError::OutcomeUnknown,
        Error::Locked | Error::Prompt | Error::PromptDisconnected => StoreError::Locked,
        Error::Unavailable => StoreError::Unavailable,
        _ if mutating => StoreError::OutcomeUnknown,
        _ => StoreError::Unavailable,
    }
}

/// True when the login collection is locked, which hides whether any item exists.
pub(super) fn locked() -> Result<bool, StoreError> {
    let service = connect()?;
    match collection(&service) {
        Ok(_) => Ok(false),
        Err(StoreError::Locked) => Ok(true),
        Err(error) => Err(error),
    }
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

/// The unlocked login collection, refusing the session alias and any locked state.
fn collection<'a>(service: &'a SecretService<'a>) -> Result<Collection<'a>, StoreError> {
    persistent(service)?;
    let collection = service
        .get_default_collection()
        .map_err(|error| native_error(error, false))?;
    if collection
        .is_locked()
        .map_err(|error| native_error(error, false))?
    {
        return Err(StoreError::Locked);
    }
    Ok(collection)
}

/// Items for one target inside the login collection only; other collections are ignored.
fn find<'a>(collection: &'a Collection<'a>, target: &str) -> Result<Vec<Item<'a>>, StoreError> {
    let items = collection
        .search_items(attributes(target))
        .map_err(|error| native_error(error, false))?;
    for item in &items {
        if item
            .is_locked()
            .map_err(|error| native_error(error, false))?
        {
            return Err(StoreError::Locked);
        }
    }
    Ok(items)
}

pub(super) fn read(target: &str, limit: usize) -> Result<Option<SecretBytes>, StoreError> {
    let service = connect()?;
    let collection = collection(&service)?;
    let found = find(&collection, target)?;
    match found.as_slice() {
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
    let collection = collection(&service)?;
    if find(&collection, target)?.len() > 1 {
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
    let collection = collection(&service)?;
    for item in &find(&collection, target)? {
        item.delete().map_err(|error| native_error(error, true))?;
    }
    Ok(())
}
