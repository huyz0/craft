//! Immutable non-secret artifacts under an explicitly selected private root.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use craft_core::control::InvocationControl;
use craft_core::error::ErrorCode;
use craft_core::input::{content_digest, parse_object};
use craft_core::storage::{
    ArtifactBlob, ArtifactReceipt, ArtifactSpec, ArtifactStore, ArtifactWriter, StoreError,
    validate_artifact_name,
};
use craft_platform::PrivateRoot;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::profiles::random_id;

mod inputs;
mod streams;
pub use streams::ArtifactInput;
mod outputs;
#[cfg(test)]
mod tests;

const METADATA_LIMIT: u64 = 16_384;
const STREAM_BYTES: usize = 65_536;
const CLEANUP_ENTRY_LIMIT: usize = 4_096;
const MAX_ARTIFACT_BYTES: u64 = 1_073_741_824;

#[derive(Clone)]
pub struct NativeArtifacts {
    root: Arc<PrivateRoot>,
    max_bytes: u64,
    control: InvocationControl,
    #[cfg(test)]
    fault: Arc<std::sync::atomic::AtomicU8>,
    #[cfg(test)]
    cancel_after_publish: Arc<std::sync::Mutex<Option<craft_core::control::CancellationToken>>>,
    #[cfg(test)]
    cancel_before_export: Arc<std::sync::Mutex<Option<craft_core::control::CancellationToken>>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u8,
    receipt: ArtifactReceipt,
}

fn io_error(error: io::Error) -> StoreError {
    if error.kind() == io::ErrorKind::AlreadyExists {
        StoreError::Conflict
    } else {
        StoreError::Unavailable
    }
}

fn control_error(error: craft_core::error::CraftError) -> StoreError {
    match error.code {
        ErrorCode::Cancelled => StoreError::Cancelled,
        ErrorCode::DeadlineExceeded => StoreError::DeadlineExceeded,
        _ => StoreError::Unavailable,
    }
}

fn check(control: &InvocationControl) -> Result<(), StoreError> {
    control.check().map_err(control_error)
}

fn finish_digest(digest: Sha256) -> String {
    let mut result = String::from("sha256:");
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest.finalize() {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    result
}

fn object_name(name: &str) -> Result<String, StoreError> {
    validate_artifact_name(name)?;
    Ok(format!("object-{}", &content_digest(name.as_bytes())[7..]))
}

fn staging_name(name: &str) -> bool {
    crate::profiles::is_opaque_id(name, "pending_")
}

impl NativeArtifacts {
    /// The launcher selects this private allowed root; provider data must not select it.
    /// Per-request budgets cannot exceed the supplied ceiling or the 1 GiB hard bound.
    pub fn open(
        root: &Path,
        max_bytes: u64,
        control: &InvocationControl,
    ) -> Result<Self, StoreError> {
        check(control)?;
        if !(1..=MAX_ARTIFACT_BYTES).contains(&max_bytes) {
            return Err(StoreError::Invalid);
        }
        Ok(Self {
            root: Arc::new(PrivateRoot::open(root).map_err(io_error)?),
            max_bytes,
            control: control.clone(),
            #[cfg(test)]
            fault: Arc::default(),
            #[cfg(test)]
            cancel_after_publish: Arc::default(),
            #[cfg(test)]
            cancel_before_export: Arc::default(),
        })
    }

    fn lock(&self) -> Result<File, StoreError> {
        check(&self.control)?;
        let file = self.root.open_file("artifact.lock").map_err(io_error)?;
        loop {
            check(&self.control)?;
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(
                        self.control
                            .remaining()
                            .map_err(control_error)?
                            .min(Duration::from_millis(10)),
                    );
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(io_error(error)),
            }
        }
    }

    fn local_path(&self, name: &str) -> Result<String, StoreError> {
        self.root
            .path()
            .join(object_name(name)?)
            .join("data")
            .to_str()
            .map(str::to_owned)
            .ok_or(StoreError::Invalid)
    }

    fn open_artifact(&self, name: &str) -> Result<Option<(ArtifactReceipt, File)>, StoreError> {
        check(&self.control)?;
        let directory = match self.root.directory(&object_name(name)?) {
            Ok(root) => root,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        let mut metadata = directory.read_file("receipt.json").map_err(io_error)?;
        if metadata.metadata().map_err(io_error)?.len() > METADATA_LIMIT {
            return Err(StoreError::Corrupt);
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut metadata)
            .take(METADATA_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() as u64 > METADATA_LIMIT {
            return Err(StoreError::Corrupt);
        }
        let record: Manifest =
            serde_json::from_value(parse_object(&bytes).map_err(|_| StoreError::Corrupt)?)
                .map_err(|_| StoreError::Corrupt)?;
        let mut receipt = record.receipt;
        if record.format != 1 || receipt.name != name || receipt.local_path.is_some() {
            return Err(StoreError::Corrupt);
        }
        if receipt.bytes > self.max_bytes {
            return Err(StoreError::TooLarge);
        }
        ArtifactSpec {
            name: receipt.name.clone(),
            media_type: receipt.media_type.clone(),
            max_bytes: self.max_bytes,
            source: receipt.source.clone(),
        }
        .validate()
        .map_err(|_| StoreError::Corrupt)?;
        let data = directory.read_file("data").map_err(io_error)?;
        if data.metadata().map_err(io_error)?.len() != receipt.bytes {
            return Err(StoreError::Corrupt);
        }
        receipt.local_path = Some(self.local_path(name)?);
        Ok(Some((receipt, data)))
    }

    fn verify(
        &self,
        receipt: &ArtifactReceipt,
        file: &mut File,
        mut output: Option<&mut dyn Write>,
    ) -> Result<(), StoreError> {
        let mut digest = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0; STREAM_BYTES];
        loop {
            check(&self.control)?;
            let count = file.read(&mut buffer).map_err(io_error)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or(StoreError::TooLarge)?;
            if total > receipt.bytes || total > self.max_bytes {
                return Err(StoreError::Corrupt);
            }
            digest.update(&buffer[..count]);
            if let Some(bytes) = output.as_deref_mut() {
                bytes.write_all(&buffer[..count]).map_err(io_error)?;
            }
        }
        if total != receipt.bytes || finish_digest(digest) != receipt.digest {
            return Err(StoreError::Corrupt);
        }
        Ok(())
    }

    fn remove_staging(&self, name: &str) -> Result<(), StoreError> {
        if !staging_name(name) {
            return Err(StoreError::Invalid);
        }
        let directory = match self.root.directory(name) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error(error)),
        };
        // Validate all entries before deleting any; unknown children keep staging intact.
        for (index, entry) in fs::read_dir(directory.path())
            .map_err(io_error)?
            .enumerate()
        {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name();
            let name = name.to_str().ok_or(StoreError::Corrupt)?;
            if index >= 3 || !matches!(name, "data" | "receipt.json" | "lease") {
                return Err(StoreError::Corrupt);
            }
            directory.read_file(name).map_err(io_error)?;
        }
        for entry in ["data", "receipt.json", "lease"] {
            match fs::remove_file(directory.path().join(entry)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
        }
        let path = directory.path().to_owned();
        drop(directory);
        fs::remove_dir(path).map_err(io_error)
    }

    /// Reclaims generated staging only after acquiring its lease; live writers are skipped.
    /// Final artifacts and unrecognized entries are never removed by this operation.
    /// An error can follow earlier reclaimed entries; repeating cleanup is safe.
    pub fn cleanup_pending(&self) -> Result<usize, StoreError> {
        let _lock = self.lock()?;
        let mut removed = 0;
        for (index, entry) in fs::read_dir(self.root.path())
            .map_err(io_error)?
            .enumerate()
        {
            check(&self.control)?;
            if index >= CLEANUP_ENTRY_LIMIT {
                return Err(StoreError::TooLarge);
            }
            let entry = entry.map_err(io_error)?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !staging_name(&name) {
                continue;
            }
            let directory = self.root.directory(&name).map_err(io_error)?;
            // A crash during create-new staging can precede lease creation. The root lock
            // excludes a live creator in that window; never manufacture a missing input.
            let lease = match directory.lock_file("lease") {
                Ok(lease) => Some(lease),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(io_error(error)),
            };
            if let Some(lease) = &lease {
                match lease.try_lock() {
                    Ok(()) => {}
                    Err(std::fs::TryLockError::WouldBlock) => continue,
                    Err(std::fs::TryLockError::Error(error)) => return Err(io_error(error)),
                }
            }
            drop(lease);
            drop(directory);
            self.remove_staging(&name)?;
            removed += 1;
        }
        Ok(removed)
    }
}

struct StagedArtifact {
    store: NativeArtifacts,
    spec: ArtifactSpec,
    name: String,
    data: Option<File>,
    lease: Option<File>,
    directory: Option<PrivateRoot>,
    digest: Sha256,
    bytes: u64,
    failed: Option<StoreError>,
}

impl StagedArtifact {
    fn close(&mut self) {
        self.data.take();
        self.lease.take();
        self.directory.take();
    }

    fn status(&self) -> Result<(), StoreError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        check(&self.store.control)
    }
}

impl Write for StagedArtifact {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = (|| {
            self.status()?;
            if self
                .bytes
                .checked_add(bytes.len() as u64)
                .is_none_or(|total| total > self.spec.max_bytes)
            {
                return Err(StoreError::TooLarge);
            }
            let count = self
                .data
                .as_mut()
                .ok_or(StoreError::Unavailable)?
                .write(bytes)
                .map_err(io_error)?;
            self.digest.update(&bytes[..count]);
            self.bytes += count as u64;
            Ok(count)
        })();
        if let Err(error) = result {
            self.failed = Some(error);
        }
        result.map_err(io::Error::other)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.status().map_err(io::Error::other)?;
        let result = self
            .data
            .as_mut()
            .ok_or_else(|| io::Error::other("Artifact writer is closed"))?
            .flush();
        if result.is_err() {
            self.failed = Some(StoreError::Unavailable);
        }
        result
    }
}

impl Drop for StagedArtifact {
    fn drop(&mut self) {
        // Never wait in Drop: a failed begin may still own this process's root lock.
        // A busy lock leaves closed staging for explicit cleanup on the next run.
        if let Ok(lock) = self.store.root.open_file("artifact.lock")
            && lock.try_lock().is_ok()
        {
            self.close();
            let _ = self.store.remove_staging(&self.name);
        }
    }
}

impl ArtifactWriter for StagedArtifact {
    fn finish(mut self: Box<Self>) -> Result<ArtifactReceipt, StoreError> {
        self.status()?;
        let mut receipt = ArtifactReceipt {
            name: self.spec.name.clone(),
            media_type: self.spec.media_type.clone(),
            digest: finish_digest(self.digest.clone()),
            bytes: self.bytes,
            source: self.spec.source.clone(),
            local_path: None,
        };
        let bytes = serde_json::to_vec(&Manifest {
            format: 1,
            receipt: receipt.clone(),
        })
        .map_err(|_| StoreError::Invalid)?;
        if bytes.len() as u64 > METADATA_LIMIT {
            return Err(StoreError::TooLarge);
        }
        let directory = self.directory.as_ref().ok_or(StoreError::Unavailable)?;
        let mut metadata = directory
            .create_new_file("receipt.json")
            .map_err(io_error)?;
        metadata.write_all(&bytes).map_err(io_error)?;
        metadata.sync_all().map_err(io_error)?;
        self.data
            .as_ref()
            .ok_or(StoreError::Unavailable)?
            .sync_all()
            .map_err(io_error)?;
        drop(metadata);
        receipt.local_path = Some(self.store.local_path(&self.spec.name)?);
        let destination = object_name(&self.spec.name)?;
        let _lock = self.store.lock()?;
        self.status()?;
        #[cfg(test)]
        if self.store.fault.load(std::sync::atomic::Ordering::SeqCst) == 1 {
            return Err(StoreError::Unavailable);
        }
        self.close();
        if self
            .store
            .root
            .publish_directory(&self.name, &destination)
            .is_err()
        {
            if fs::symlink_metadata(self.store.root.path().join(&self.name)).is_ok()
                && fs::symlink_metadata(self.store.root.path().join(&destination)).is_ok()
            {
                return Err(StoreError::Conflict);
            }
            // A failed acknowledgment alone cannot prove that the atomic move did not occur.
            return Err(StoreError::OutcomeUnknown);
        }
        #[cfg(test)]
        if self.store.fault.load(std::sync::atomic::Ordering::SeqCst) == 2 {
            return Err(StoreError::OutcomeUnknown);
        }
        #[cfg(test)]
        if let Some(token) = self
            .store
            .cancel_after_publish
            .lock()
            .map_err(|_| StoreError::OutcomeUnknown)?
            .take()
        {
            token.cancel();
        }
        // Publication succeeded. Later cancellation must not erase this confirmed local result.
        Ok(receipt)
    }
}

impl ArtifactStore for NativeArtifacts {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError> {
        check(&self.control)?;
        spec.validate()?;
        if spec.max_bytes > self.max_bytes {
            return Err(StoreError::TooLarge);
        }
        let _lock = self.lock()?;
        match fs::symlink_metadata(self.root.path().join(object_name(&spec.name)?)) {
            Ok(_) => return Err(StoreError::Conflict),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
        let name = random_id("pending_").map_err(|_| StoreError::Unavailable)?;
        let directory = self.root.create_directory(&name).map_err(io_error)?;
        let mut staged = StagedArtifact {
            store: self.clone(),
            spec,
            name,
            data: None,
            lease: None,
            directory: Some(directory),
            digest: Sha256::new(),
            bytes: 0,
            failed: None,
        };
        let directory = staged.directory.as_ref().ok_or(StoreError::Unavailable)?;
        let lease = directory.create_new_file("lease").map_err(io_error)?;
        lease.try_lock().map_err(|_| StoreError::Unavailable)?;
        staged.lease = Some(lease);
        staged.data = Some(directory.create_new_file("data").map_err(io_error)?);
        Ok(Box::new(staged))
    }

    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError> {
        let Some((receipt, mut data)) = self.open_artifact(name)? else {
            return Ok(None);
        };
        self.verify(&receipt, &mut data, None)?;
        Ok(Some(receipt))
    }

    fn read(&self, name: &str, max_bytes: u64) -> Result<Option<ArtifactBlob>, StoreError> {
        if max_bytes == 0 {
            return Err(StoreError::Invalid);
        }
        let Some((receipt, mut data)) = self.open_artifact(name)? else {
            return Ok(None);
        };
        if receipt.bytes > max_bytes {
            return Err(StoreError::TooLarge);
        }
        let mut bytes = Vec::new();
        self.verify(&receipt, &mut data, Some(&mut bytes))?;
        Ok(Some(ArtifactBlob { receipt, bytes }))
    }
}
