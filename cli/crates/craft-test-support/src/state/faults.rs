use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use craft_core::storage::StoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FaultPoint {
    VaultRead,
    VaultWriteBeforeCommit,
    VaultWriteAfterCommit,
    VaultDeleteBeforeCommit,
    VaultDeleteAfterCommit,
    JournalRead,
    JournalBeginBeforeCommit,
    JournalBeginAfterCommit,
    JournalUpdateBeforeCommit,
    JournalUpdateAfterCommit,
    ArtifactBegin,
    ArtifactRead,
    ArtifactWrite,
    ArtifactBeforeCommit,
    ArtifactAfterCommit,
}

#[derive(Default)]
struct Schedule {
    armed: BTreeMap<FaultPoint, u64>,
    calls: BTreeMap<FaultPoint, u64>,
}

#[derive(Clone, Default)]
pub struct Faults(Arc<Mutex<Schedule>>);

impl Faults {
    /// Arms a one-shot fault relative to the next invocation of this boundary.
    pub fn fail_on(&self, point: FaultPoint, invocation: u64) -> Result<(), StoreError> {
        if invocation == 0 {
            return Err(StoreError::Invalid);
        }
        self.0
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .armed
            .insert(point, invocation);
        Ok(())
    }

    pub fn calls(&self, point: FaultPoint) -> Result<u64, StoreError> {
        Ok(*self
            .0
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .calls
            .get(&point)
            .unwrap_or(&0))
    }

    /// Advances a scheduled boundary for an injected native-host test adapter.
    pub fn hit(&self, point: FaultPoint) -> Result<bool, StoreError> {
        let mut schedule = self.0.lock().map_err(|_| StoreError::Unavailable)?;
        *schedule.calls.entry(point).or_default() += 1;
        match schedule.armed.get_mut(&point) {
            Some(remaining) if *remaining == 1 => {
                schedule.armed.remove(&point);
                Ok(true)
            }
            Some(remaining) => {
                *remaining -= 1;
                Ok(false)
            }
            None => Ok(false),
        }
    }
}
