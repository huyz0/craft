use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use craft_core::contracts::validate_identity;
use craft_core::storage::{JournalRecord, OperationBinding, OperationJournal, StoreError};

use super::{FaultPoint, Faults};

#[derive(Clone)]
pub struct MemoryJournal {
    records: Arc<Mutex<BTreeMap<String, JournalRecord>>>,
    faults: Faults,
}

impl MemoryJournal {
    pub fn new(faults: Faults) -> Self {
        Self {
            records: Arc::default(),
            faults,
        }
    }
}

impl OperationJournal for MemoryJournal {
    fn get(&self, operation_id: &str) -> Result<Option<JournalRecord>, StoreError> {
        validate_identity("/operation_id", operation_id).map_err(|_| StoreError::Invalid)?;
        if self.faults.hit(FaultPoint::JournalRead)? {
            return Err(StoreError::Unavailable);
        }
        Ok(self
            .records
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .get(operation_id)
            .cloned())
    }

    fn begin(&self, binding: &OperationBinding) -> Result<JournalRecord, StoreError> {
        binding.validate()?;
        if self.faults.hit(FaultPoint::JournalBeginBeforeCommit)? {
            return Err(StoreError::Unavailable);
        }
        let mut records = self.records.lock().map_err(|_| StoreError::Unavailable)?;
        if let Some(record) = records.get(&binding.operation_id) {
            return if &record.binding == binding {
                Ok(record.clone())
            } else {
                Err(StoreError::Conflict)
            };
        }
        let record = JournalRecord::initial(binding.clone())?;
        records.insert(binding.operation_id.clone(), record.clone());
        if self.faults.hit(FaultPoint::JournalBeginAfterCommit)? {
            return Err(StoreError::OutcomeUnknown);
        }
        Ok(record)
    }

    fn compare_and_swap(
        &self,
        previous_revision: u64,
        next: &JournalRecord,
    ) -> Result<(), StoreError> {
        if self.faults.hit(FaultPoint::JournalUpdateBeforeCommit)? {
            return Err(StoreError::Unavailable);
        }
        let mut records = self.records.lock().map_err(|_| StoreError::Unavailable)?;
        let previous = records
            .get(&next.binding.operation_id)
            .ok_or(StoreError::Conflict)?;
        if previous.revision != previous_revision {
            return Err(StoreError::Conflict);
        }
        previous.validate_successor(next)?;
        records.insert(next.binding.operation_id.clone(), next.clone());
        if self.faults.hit(FaultPoint::JournalUpdateAfterCommit)? {
            return Err(StoreError::OutcomeUnknown);
        }
        Ok(())
    }
}
