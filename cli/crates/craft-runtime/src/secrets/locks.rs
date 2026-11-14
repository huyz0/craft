use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use craft_core::contracts::GrantRef;
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_platform::PrivateRoot;

use crate::profiles::is_opaque_id;

pub(super) struct GrantLocks {
    root: Arc<PrivateRoot>,
    installation: String,
}

pub(super) struct GrantLock {
    _file: File,
    _root: Arc<PrivateRoot>,
    pub(super) grant: String,
    pub(super) installation: String,
}

impl GrantLocks {
    pub(super) fn open(path: &Path, installation: &str) -> Result<Self, CraftError> {
        if !is_opaque_id(installation, "ins_") {
            return Err(CraftError::field(
                "/installation",
                "expected_installation_identity",
            ));
        }
        let root = PrivateRoot::open(path).map_err(|_| {
            CraftError::new(
                ErrorCode::PermissionDenied,
                "Credential lock root is inaccessible or not private.",
            )
        })?;
        Ok(Self {
            root: Arc::new(root),
            installation: installation.into(),
        })
    }

    pub(super) fn acquire(
        &self,
        grant: &GrantRef,
        control: &InvocationControl,
    ) -> Result<GrantLock, CraftError> {
        control.check()?;
        if !is_opaque_id(&grant.id, "gr_") {
            return Err(CraftError::field(
                "/grant",
                "expected_issued_grant_identity",
            ));
        }
        let file = self
            .root
            .open_file(&format!("lock-{}-{}.lck", self.installation, grant.id))
            .map_err(|_| {
                CraftError::new(
                    ErrorCode::PermissionDenied,
                    "Cannot open the private grant lock.",
                )
            })?;
        loop {
            control.check()?;
            match file.try_lock() {
                Ok(()) => {
                    return Ok(GrantLock {
                        _file: file,
                        _root: self.root.clone(),
                        grant: grant.id.clone(),
                        installation: self.installation.clone(),
                    });
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(control.remaining()?.min(Duration::from_millis(10)))
                }
                Err(std::fs::TryLockError::Error(_)) => {
                    return Err(CraftError::new(
                        ErrorCode::KeyringUnavailable,
                        "Grant locking is unavailable on this state filesystem.",
                    ));
                }
            }
        }
    }
}
