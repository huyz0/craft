//! Non-secret local mutation intent with revision-checked send claims and evidence.

use std::io::{self, Write};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Duration;

use craft_core::contracts::{GrantRef, RemoteJob, ResourceRef, validate_identity};
use craft_core::control::InvocationControl;
use craft_core::error::{CraftError, ErrorCode};
use craft_core::execution::{HttpMethod, NativeCall, PreparedOperation};
use craft_core::input::{canonical_digest, parse_object};
use craft_core::registry::Registry;
use craft_core::storage::{
    JournalRecord, JournalStep, OperationBinding, OperationJournal, StoreError, SubmissionState,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::profiles::{ProfileStore, begin};

#[cfg(test)]
mod tests;

const MAX_RECORD_BYTES: usize = 1_048_576;
const MAX_RECORDS: i64 = 4_096;

struct Inner {
    profiles: Mutex<ProfileStore>,
    installation: String,
    #[cfg(test)]
    faults: craft_test_support::state::Faults,
}

#[derive(Clone)]
pub struct NativeJournal {
    inner: Arc<Inner>,
    control: InvocationControl,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRecord {
    format: u8,
    installation: String,
    record: JournalRecord,
}

#[derive(Serialize)]
struct RecordView<'a> {
    format: u8,
    installation: &'a str,
    record: &'a JournalRecord,
}

struct RecordBuffer(Vec<u8>);
impl Write for RecordBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .0
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > MAX_RECORD_BYTES)
        {
            return Err(io::Error::other("Journal record byte bound exceeded"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Trusted host inputs; the actor must come from authenticated session verification.
pub struct MutationAuthority<'a> {
    pub apply: bool,
    pub operation_id: &'a str,
    pub grant: &'a GrantRef,
    pub actor: &'a str,
}

/// A committed local send marker. Never clone, serialize or treat it as remote idempotency.
/// Dropping the claim leaves the marker requiring reconciliation rather than reopening it.
pub struct SendClaim {
    journal: NativeJournal,
    binding: OperationBinding,
    step: String,
    request_digest: String,
}

pub struct StepOutcome {
    pub state: SubmissionState,
    pub confirmed_resources: Vec<ResourceRef>,
    pub job: Option<RemoteJob>,
}

fn control_error(error: CraftError) -> StoreError {
    match error.code {
        ErrorCode::Cancelled => StoreError::Cancelled,
        ErrorCode::DeadlineExceeded => StoreError::DeadlineExceeded,
        ErrorCode::WriteOutcomeUnknown => StoreError::OutcomeUnknown,
        _ => StoreError::Unavailable,
    }
}

fn check(control: &InvocationControl) -> Result<(), StoreError> {
    control.check().map_err(control_error)
}

fn sql_error(error: rusqlite::Error) -> StoreError {
    if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull) {
        StoreError::TooLarge
    } else {
        StoreError::Unavailable
    }
}

fn claim_error(error: StoreError) -> CraftError {
    let code = match error {
        StoreError::Conflict => ErrorCode::OperationIdReused,
        StoreError::OutcomeUnknown => ErrorCode::WriteOutcomeUnknown,
        StoreError::Cancelled => ErrorCode::Cancelled,
        StoreError::DeadlineExceeded => ErrorCode::DeadlineExceeded,
        StoreError::Invalid | StoreError::TooLarge => ErrorCode::InputInvalid,
        _ => ErrorCode::Internal,
    };
    let mut result = CraftError::new(code, "Local mutation authority could not be established.");
    result.next_action = "Inspect the operation under the same state root; do not resend an unknown or previously started request.".into();
    result
}

impl NativeJournal {
    #[cfg(test)]
    pub(crate) fn fail_on(
        &self,
        point: craft_test_support::state::FaultPoint,
        boundary: u64,
    ) -> Result<(), StoreError> {
        self.inner.faults.fail_on(point, boundary)
    }

    /// Shares this installation's journal while bounding waits by the current invocation.
    pub fn for_invocation(&self, control: &InvocationControl) -> Self {
        Self {
            inner: self.inner.clone(),
            control: control.clone(),
        }
    }

    /// Reopens validated private profile state for typed non-secret journal metadata.
    pub fn open(profiles: &ProfileStore, control: &InvocationControl) -> Result<Self, StoreError> {
        check(control)?;
        let reopened = ProfileStore::open(profiles.path(), control).map_err(control_error)?;
        if reopened.settings() != profiles.settings() {
            return Err(StoreError::Conflict);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                installation: profiles.settings().installation_id.clone(),
                profiles: Mutex::new(reopened),
                #[cfg(test)]
                faults: Default::default(),
            }),
            control: control.clone(),
        })
    }

    fn lock(&self, evidence: bool) -> Result<MutexGuard<'_, ProfileStore>, StoreError> {
        loop {
            if !evidence {
                check(&self.control)?;
            }
            match self.inner.profiles.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(_)) => return Err(StoreError::Unavailable),
                Err(TryLockError::WouldBlock) if evidence => return Err(StoreError::Unavailable),
                Err(TryLockError::WouldBlock) => std::thread::sleep(
                    self.control
                        .remaining()
                        .map_err(control_error)?
                        .min(Duration::from_millis(10)),
                ),
            }
        }
    }

    fn transaction<'a>(
        &self,
        connection: &'a Connection,
        evidence: bool,
    ) -> Result<Transaction<'a>, StoreError> {
        if evidence {
            Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
                .map_err(sql_error)
        } else {
            begin(connection, &self.control).map_err(control_error)
        }
    }

    fn decode(&self, row: &Row<'_>, id: &str) -> Result<JournalRecord, StoreError> {
        let payload = row
            .get_ref(2)
            .map_err(|_| StoreError::Corrupt)?
            .as_str()
            .map_err(|_| StoreError::Corrupt)?;
        if payload.len() > MAX_RECORD_BYTES {
            return Err(StoreError::Corrupt);
        }
        let stored: StoredRecord = serde_json::from_value(
            parse_object(payload.as_bytes()).map_err(|_| StoreError::Corrupt)?,
        )
        .map_err(|_| StoreError::Corrupt)?;
        let revision: i64 = row.get(0).map_err(|_| StoreError::Corrupt)?;
        let grant: String = row.get(1).map_err(|_| StoreError::Corrupt)?;
        if stored.format != 1
            || stored.installation != self.inner.installation
            || stored.record.binding.operation_id != id
            || stored.record.binding.grant.id != grant
            || u64::try_from(revision).ok() != Some(stored.record.revision)
        {
            return Err(StoreError::Corrupt);
        }
        stored.record.validate().map_err(|_| StoreError::Corrupt)?;
        Ok(stored.record)
    }

    fn read(&self, connection: &Connection, id: &str) -> Result<Option<JournalRecord>, StoreError> {
        validate_identity("/operation_id", id).map_err(|_| StoreError::Invalid)?;
        #[cfg(test)]
        if self
            .inner
            .faults
            .hit(craft_test_support::state::FaultPoint::JournalRead)?
        {
            return Err(StoreError::Unavailable);
        }
        connection
            .query_row(
                "SELECT revision,grant_id,payload FROM operation_records WHERE operation_id=?1",
                [id],
                |row| Ok(self.decode(row, id)),
            )
            .optional()
            .map_err(sql_error)?
            .transpose()
    }

    fn encode(&self, record: &JournalRecord) -> Result<String, StoreError> {
        record.validate()?;
        if record.revision > i64::MAX as u64 {
            return Err(StoreError::TooLarge);
        }
        let mut buffer = RecordBuffer(Vec::new());
        serde_json::to_writer(
            &mut buffer,
            &RecordView {
                format: 1,
                installation: &self.inner.installation,
                record,
            },
        )
        .map_err(|error| {
            if error.is_io() {
                StoreError::TooLarge
            } else {
                StoreError::Invalid
            }
        })?;
        let bytes = String::from_utf8(buffer.0).map_err(|_| StoreError::Invalid)?;
        // A writer must not publish a record that its bounded strict reader would refuse.
        parse_object(bytes.as_bytes()).map_err(|_| StoreError::TooLarge)?;
        Ok(bytes)
    }

    fn commit(
        &self,
        transaction: Transaction<'_>,
        update: bool,
        evidence: bool,
    ) -> Result<(), StoreError> {
        if !evidence {
            check(&self.control)?;
        }
        #[cfg(test)]
        if self.inner.faults.hit(if update {
            craft_test_support::state::FaultPoint::JournalUpdateBeforeCommit
        } else {
            craft_test_support::state::FaultPoint::JournalBeginBeforeCommit
        })? {
            return Err(StoreError::Unavailable);
        }
        crate::profiles::commit(
            transaction,
            if evidence { None } else { Some(&self.control) },
        )
        .map_err(control_error)?;
        #[cfg(test)]
        if self.inner.faults.hit(if update {
            craft_test_support::state::FaultPoint::JournalUpdateAfterCommit
        } else {
            craft_test_support::state::FaultPoint::JournalBeginAfterCommit
        })? {
            return Err(StoreError::OutcomeUnknown);
        }
        #[cfg(not(test))]
        let _ = update;
        Ok(())
    }

    fn update(
        &self,
        connection: &Connection,
        previous: &JournalRecord,
        next: &JournalRecord,
    ) -> Result<(), StoreError> {
        previous.validate_successor(next)?;
        let payload = self.encode(next)?;
        let changed = connection.execute("UPDATE operation_records SET revision=?1,payload=?2 WHERE operation_id=?3 AND revision=?4", params![next.revision as i64, payload, next.binding.operation_id, previous.revision as i64]).map_err(sql_error)?;
        if changed != 1 {
            return Err(StoreError::Conflict);
        }
        Ok(())
    }

    /// Binds one exact declared native request, then commits its send marker before returning.
    /// Previously started, terminal, changed or uncertain claims never authorize another send.
    pub fn claim_send(
        &self,
        registry: &Registry,
        operation: &PreparedOperation,
        authority: &MutationAuthority<'_>,
        step: &str,
        call: &NativeCall,
    ) -> Result<SendClaim, CraftError> {
        if !authority.apply {
            return Err(CraftError::new(
                ErrorCode::ApplyRequired,
                "Resource mutations require explicit apply.",
            ));
        }
        check(&self.control).map_err(claim_error)?;
        validate_identity("/step", step)?;
        let descriptor = registry.prepared_descriptor(operation)?;
        let binding =
            operation.mutation_binding(authority.operation_id, authority.grant, authority.actor)?;
        let route = descriptor
            .routes
            .iter()
            .find(|route| route.id == call.route)
            .ok_or_else(|| {
                CraftError::new(ErrorCode::PolicyDenied, "Mutation route was not declared.")
            })?;
        route.validate_call(call)?;
        if matches!(route.method, HttpMethod::Get) {
            return Err(CraftError::new(
                ErrorCode::PolicyDenied,
                "Read routes do not obtain mutation claims.",
            ));
        }
        let request_digest = canonical_digest(
            &serde_json::json!({"route":call.route,"method":route.method,"parameters":call.path_parameters,"query":call.query,"body":call.body}),
        )?;
        let mut record = self.begin(&binding).map_err(claim_error)?;
        if let Some(existing) = record.steps.get(step)
            && (existing.state != SubmissionState::Prepared
                || existing
                    .request_digest
                    .as_ref()
                    .is_some_and(|digest| digest != &request_digest))
        {
            return Err(claim_error(StoreError::Conflict));
        }
        if record
            .steps
            .get(step)
            .is_none_or(|existing| existing.request_digest.is_none())
        {
            let mut prepared = record.clone();
            prepared.revision = prepared
                .revision
                .checked_add(1)
                .ok_or_else(|| claim_error(StoreError::TooLarge))?;
            prepared.steps.insert(
                step.into(),
                JournalStep {
                    request_digest: Some(request_digest.clone()),
                    ..Default::default()
                },
            );
            self.compare_and_swap(record.revision, &prepared)
                .map_err(claim_error)?;
            record = prepared;
        }
        let mut sending = record.clone();
        sending.revision = sending
            .revision
            .checked_add(1)
            .ok_or_else(|| claim_error(StoreError::TooLarge))?;
        sending
            .steps
            .get_mut(step)
            .ok_or_else(|| claim_error(StoreError::Corrupt))?
            .state = SubmissionState::SendStarted;
        self.compare_and_swap(record.revision, &sending)
            .map_err(claim_error)?;
        Ok(SendClaim {
            journal: self.clone(),
            binding,
            step: step.into(),
            request_digest,
        })
    }
}

impl OperationJournal for NativeJournal {
    fn get(&self, operation_id: &str) -> Result<Option<JournalRecord>, StoreError> {
        let profiles = self.lock(false)?;
        self.read(&profiles.connection, operation_id)
    }

    fn begin(&self, binding: &OperationBinding) -> Result<JournalRecord, StoreError> {
        binding.validate()?;
        let profiles = self.lock(false)?;
        let transaction = self.transaction(&profiles.connection, false)?;
        let issued: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM grants WHERE id=?1)",
                [&binding.grant.id],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if !issued {
            return Err(StoreError::Conflict);
        }
        if let Some(record) = self.read(&transaction, &binding.operation_id)? {
            return if &record.binding == binding {
                Ok(record)
            } else {
                Err(StoreError::Conflict)
            };
        }
        let count: i64 = transaction
            .query_row("SELECT count(*) FROM operation_records", [], |row| {
                row.get(0)
            })
            .map_err(sql_error)?;
        if count >= MAX_RECORDS {
            return Err(StoreError::TooLarge);
        }
        let record = JournalRecord::initial(binding.clone())?;
        let payload = self.encode(&record)?;
        transaction.execute("INSERT INTO operation_records(operation_id,grant_id,revision,payload) VALUES(?1,?2,0,?3)", params![binding.operation_id,binding.grant.id,payload]).map_err(sql_error)?;
        self.commit(transaction, false, false)?;
        Ok(record)
    }

    fn compare_and_swap(
        &self,
        previous_revision: u64,
        next: &JournalRecord,
    ) -> Result<(), StoreError> {
        next.validate()?;
        let profiles = self.lock(false)?;
        let transaction = self.transaction(&profiles.connection, false)?;
        let previous = self
            .read(&transaction, &next.binding.operation_id)?
            .ok_or(StoreError::Conflict)?;
        if previous.revision != previous_revision {
            return Err(StoreError::Conflict);
        }
        self.update(&transaction, &previous, next)?;
        self.commit(transaction, true, false)
    }
}

impl SendClaim {
    pub(crate) fn binding(&self) -> &OperationBinding {
        &self.binding
    }

    pub(crate) fn step(&self) -> &str {
        &self.step
    }

    /// Appends observed effect evidence through one immediate local transaction, even after
    /// cancellation. Failure leaves this caller's supplied receipt intact and authorizes no send.
    pub fn record_outcome(&self, outcome: &StepOutcome) -> Result<JournalRecord, StoreError> {
        if !matches!(
            outcome.state,
            SubmissionState::Confirmed | SubmissionState::Failed | SubmissionState::Unknown
        ) {
            return Err(StoreError::Invalid);
        }
        let profiles = self.journal.lock(true)?;
        let transaction = self.journal.transaction(&profiles.connection, true)?;
        let previous = self
            .journal
            .read(&transaction, &self.binding.operation_id)?
            .ok_or(StoreError::Conflict)?;
        if previous.binding != self.binding {
            return Err(StoreError::Conflict);
        }
        let mut next = previous.clone();
        next.revision = next.revision.checked_add(1).ok_or(StoreError::TooLarge)?;
        let step = next.steps.get_mut(&self.step).ok_or(StoreError::Conflict)?;
        if step.request_digest.as_ref() != Some(&self.request_digest) {
            return Err(StoreError::Conflict);
        }
        step.state = outcome.state;
        for resource in &outcome.confirmed_resources {
            if !step.confirmed_resources.contains(resource) {
                step.confirmed_resources.push(resource.clone());
            }
        }
        if let Some(job) = &outcome.job {
            step.job = Some(job.clone());
        }
        self.journal.update(&transaction, &previous, &next)?;
        self.journal.commit(transaction, true, true)?;
        Ok(next)
    }
}
