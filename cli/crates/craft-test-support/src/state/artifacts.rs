use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use craft_core::input::content_digest;
use craft_core::storage::{
    ArtifactBlob, ArtifactReceipt, ArtifactSpec, ArtifactStore, ArtifactWriter, StoreError,
    validate_artifact_name,
};

use super::{FaultPoint, Faults};

struct StoredArtifact {
    receipt: ArtifactReceipt,
    bytes: Vec<u8>,
}

#[derive(Clone)]
pub struct MemoryArtifacts {
    entries: Arc<Mutex<BTreeMap<String, StoredArtifact>>>,
    capacity: usize,
    faults: Faults,
}

impl MemoryArtifacts {
    pub fn new(faults: Faults, capacity: usize) -> Self {
        Self {
            entries: Arc::default(),
            capacity,
            faults,
        }
    }
}

struct StagedArtifact {
    store: MemoryArtifacts,
    spec: ArtifactSpec,
    bytes: Vec<u8>,
    failed: bool,
}

impl Write for StagedArtifact {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let injected = self
            .store
            .faults
            .hit(FaultPoint::ArtifactWrite)
            .map_err(io::Error::other)?;
        let length = self.bytes.len().checked_add(bytes.len());
        if self.failed
            || injected
            || length.is_none_or(|length| {
                length as u64 > self.spec.max_bytes || length > self.store.capacity
            })
        {
            self.failed = true;
            return Err(io::Error::other("Artifact staging write failed"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(io::Error::other("Artifact staging write failed"));
        }
        Ok(())
    }
}

impl ArtifactWriter for StagedArtifact {
    fn finish(self: Box<Self>) -> Result<ArtifactReceipt, StoreError> {
        if self.failed || self.store.faults.hit(FaultPoint::ArtifactBeforeCommit)? {
            return Err(StoreError::Unavailable);
        }
        let mut entries = self
            .store
            .entries
            .lock()
            .map_err(|_| StoreError::Unavailable)?;
        if entries.contains_key(&self.spec.name) {
            return Err(StoreError::Conflict);
        }
        let used = entries.values().try_fold(0_usize, |sum, entry| {
            sum.checked_add(entry.bytes.len())
                .ok_or(StoreError::TooLarge)
        })?;
        if used
            .checked_add(self.bytes.len())
            .is_none_or(|total| total > self.store.capacity)
        {
            return Err(StoreError::TooLarge);
        }
        let receipt = ArtifactReceipt {
            name: self.spec.name,
            media_type: self.spec.media_type,
            digest: content_digest(&self.bytes),
            bytes: self.bytes.len() as u64,
            source: self.spec.source,
            local_path: None,
        };
        entries.insert(
            receipt.name.clone(),
            StoredArtifact {
                receipt: receipt.clone(),
                bytes: self.bytes,
            },
        );
        if self.store.faults.hit(FaultPoint::ArtifactAfterCommit)? {
            return Err(StoreError::OutcomeUnknown);
        }
        Ok(receipt)
    }
}

impl ArtifactStore for MemoryArtifacts {
    fn begin(&self, spec: ArtifactSpec) -> Result<Box<dyn ArtifactWriter>, StoreError> {
        spec.validate()?;
        if spec.max_bytes > self.capacity as u64 {
            return Err(StoreError::TooLarge);
        }
        if self.faults.hit(FaultPoint::ArtifactBegin)? {
            return Err(StoreError::Unavailable);
        }
        if self
            .entries
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .contains_key(&spec.name)
        {
            return Err(StoreError::Conflict);
        }
        Ok(Box::new(StagedArtifact {
            store: self.clone(),
            spec,
            bytes: Vec::new(),
            failed: false,
        }))
    }

    fn receipt(&self, name: &str) -> Result<Option<ArtifactReceipt>, StoreError> {
        validate_artifact_name(name)?;
        if self.faults.hit(FaultPoint::ArtifactRead)? {
            return Err(StoreError::Unavailable);
        }
        Ok(self
            .entries
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .get(name)
            .map(|entry| entry.receipt.clone()))
    }

    fn read(&self, name: &str, max_bytes: u64) -> Result<Option<ArtifactBlob>, StoreError> {
        validate_artifact_name(name)?;
        if max_bytes == 0 {
            return Err(StoreError::Invalid);
        }
        if self.faults.hit(FaultPoint::ArtifactRead)? {
            return Err(StoreError::Unavailable);
        }
        let entries = self.entries.lock().map_err(|_| StoreError::Unavailable)?;
        match entries.get(name) {
            Some(entry) if entry.bytes.len() as u64 > max_bytes => Err(StoreError::TooLarge),
            Some(entry) => Ok(Some(ArtifactBlob {
                receipt: entry.receipt.clone(),
                bytes: entry.bytes.clone(),
            })),
            None => Ok(None),
        }
    }
}
