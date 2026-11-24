use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::order::WorkOrder;
use crate::review::{Draft, MergeObservation, ReviewRecord, VerificationHandoff};
use crate::{Result, reject, repository, snapshot};

pub struct Store(Connection);

pub enum CiDispatch {
    Paused,
    Idle,
    NeedsOperator(String),
    Ready(String),
}

#[derive(Clone, Copy)]
pub enum CiAttempt {
    Initial,
    Retry,
}

impl CiAttempt {
    pub fn can_start(self, dispatch: &CiDispatch, id: &str) -> bool {
        match (self, dispatch) {
            (Self::Initial, CiDispatch::Ready(current))
            | (Self::Retry, CiDispatch::NeedsOperator(current)) => current == id,
            _ => false,
        }
    }
}

#[derive(Serialize)]
pub struct CiPollBranch<'a> {
    pub name: &'a str,
    pub anchor: String,
    pub observed_head: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Delegated,
    Candidate,
    VerificationAttested,
    RepairRequested,
    Closed,
    Cancelled,
}

#[derive(Serialize, JsonSchema)]
pub struct Receipt {
    pub schema_version: u32,
    pub work_order: WorkOrder,
    pub state: State,
    pub candidate: Option<String>,
    pub draft: Option<Draft>,
    pub publication_hold: Option<String>,
    pub events: Vec<Event>,
    pub attestations: Vec<RecordedAttestation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_merge: Option<MergeObservation>,
    pub native_ci: NativeCi,
    pub snapshot_problem: Option<String>,
    pub review_ready: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeCi {
    Manual,
    Required,
    Pending,
    Passed,
    Unavailable,
}

#[derive(Serialize, JsonSchema)]
pub struct Event {
    pub kind: String,
    pub candidate: Option<String>,
    pub note: String,
}

#[derive(Serialize, JsonSchema)]
pub struct RecordedAttestation {
    pub kind: String,
    pub record: ReviewRecord,
}

impl Store {
    pub fn issue_snapshot_path(&self, id: &str) -> Result<std::path::PathBuf> {
        snapshot_path(&self.0, id).map(Into::into)
    }

    pub fn probe_writes(&self) -> Result<()> {
        self.0.execute_batch("BEGIN IMMEDIATE; ROLLBACK;")?;
        Ok(())
    }

    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS work_orders (
                 id TEXT PRIMARY KEY NOT NULL,
                 work_order TEXT NOT NULL,
                 active INTEGER NOT NULL CHECK (active IN (0, 1)),
                 state TEXT NOT NULL,
                 candidate TEXT
             );
             CREATE UNIQUE INDEX IF NOT EXISTS single_active_slot
                 ON work_orders (active) WHERE active = 1;
             CREATE TABLE IF NOT EXISTS events (
                 sequence INTEGER PRIMARY KEY,
                 work_id TEXT NOT NULL REFERENCES work_orders(id),
                 kind TEXT NOT NULL,
                 candidate TEXT,
                 note TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS drafts (
                 work_id TEXT PRIMARY KEY REFERENCES work_orders(id),
                 reference TEXT NOT NULL,
                 head TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS publication_holds (
                 work_id TEXT PRIMARY KEY REFERENCES work_orders(id),
                 reason TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS jira_snapshots (
                 work_id TEXT PRIMARY KEY REFERENCES work_orders(id),
                 path TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS attestations (
                 work_id TEXT NOT NULL REFERENCES work_orders(id),
                 kind TEXT NOT NULL,
                 evidence TEXT NOT NULL,
                 payload TEXT NOT NULL,
                 PRIMARY KEY (work_id, kind, evidence)
             );
             CREATE TABLE IF NOT EXISTS ci_intake_control (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 paused INTEGER NOT NULL CHECK (paused IN (0, 1))
             );
             CREATE TABLE IF NOT EXISTS ci_poll_receipts (
                 sequence INTEGER PRIMARY KEY,
                 observed_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 repository TEXT NOT NULL,
                 mode TEXT NOT NULL,
                 decision TEXT NOT NULL,
                 work_order_id TEXT,
                 reason TEXT,
                 failure TEXT
             );
             CREATE TABLE IF NOT EXISTS ci_poll_branch_observations (
                 sequence INTEGER PRIMARY KEY REFERENCES ci_poll_receipts(sequence),
                 payload TEXT NOT NULL CHECK (json_valid(payload))
             );
             CREATE TABLE IF NOT EXISTS authority_transfers (
                 epoch TEXT PRIMARY KEY,
                 queue_origin TEXT NOT NULL,
                 phase TEXT NOT NULL CHECK (phase IN ('frozen', 'released')),
                 archive TEXT NOT NULL,
                 ci_was_paused INTEGER NOT NULL CHECK (ci_was_paused IN (0, 1))
             );
             CREATE UNIQUE INDEX IF NOT EXISTS one_frozen_transfer
                 ON authority_transfers (phase) WHERE phase = 'frozen';
             CREATE TABLE IF NOT EXISTS archived_shared_orders (
                 work_id TEXT PRIMARY KEY,
                 epoch TEXT NOT NULL REFERENCES authority_transfers(epoch)
             );",
        )?;
        Ok(Self(db))
    }

    pub fn delegate(&mut self, order: WorkOrder, snapshot_path: Option<&Path>) -> Result<Receipt> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::stage_delegation(&tx, &order, snapshot_path)?;
        finish(tx, &order.id)
    }

    pub fn shadow_delegation(
        &mut self,
        order: &WorkOrder,
        snapshot_path: Option<&Path>,
    ) -> Result<&'static str> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = Self::stage_delegation(&tx, order, snapshot_path)?;
        tx.rollback()?;
        Ok(if existing {
            "already_admitted"
        } else {
            "would_admit"
        })
    }

    pub fn set_ci_intake_paused(&mut self, paused: bool) -> Result<()> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !paused {
            ensure_local_authority(&tx)?;
        }
        tx.execute(
            "INSERT INTO ci_intake_control (id, paused) VALUES (1, ?1)
             ON CONFLICT (id) DO UPDATE SET paused = excluded.paused",
            [paused],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn freeze_for_cutover(&mut self, epoch: &str, origin: &str) -> Result<String> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<(String, String, String)> = tx
            .query_row(
                "SELECT queue_origin, phase, archive FROM authority_transfers WHERE epoch = ?1",
                [epoch],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((previous_origin, phase, archive)) = previous {
            if previous_origin != origin || phase != "frozen" {
                return reject(
                    "local cutover epoch already belongs to a different endpoint or was released",
                );
            }
            return Ok(archive);
        }
        ensure_local_authority(&tx)?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_orders WHERE active <> 0 OR state NOT IN ('closed', 'cancelled'))
                  OR EXISTS(SELECT 1 FROM publication_holds)",
            [],
            |row| row.get(0),
        )?;
        if active {
            return reject("drain local work and reconcile publication holds before cutover");
        }
        let ids = tx
            .prepare("SELECT id FROM work_orders ORDER BY id")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let receipts = ids
            .iter()
            .map(|id| read_receipt(&tx, id))
            .collect::<Result<Vec<_>>>()?;
        let archive = serde_json::to_string(&serde_json::json!({
            "schema_version": 1, "epoch": epoch, "receipts": receipts,
        }))?;
        if archive.len() > 512 * 1024 {
            return reject("local cutover archive exceeds 512 KiB");
        }
        let ci_was_paused: bool = tx.query_row(
            "SELECT COALESCE((SELECT paused FROM ci_intake_control WHERE id = 1), 0)",
            [],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO authority_transfers (epoch, queue_origin, phase, archive, ci_was_paused)
             VALUES (?1, ?2, 'frozen', ?3, ?4)",
            params![epoch, origin, archive, ci_was_paused],
        )?;
        tx.execute(
            "INSERT INTO ci_intake_control (id, paused) VALUES (1, 1)
             ON CONFLICT (id) DO UPDATE SET paused = 1",
            [],
        )?;
        tx.commit()?;
        Ok(archive)
    }

    pub fn local_cutover_archive(&self, epoch: &str, origin: &str) -> Result<String> {
        self.0
            .query_row(
                "SELECT archive FROM authority_transfers WHERE epoch = ?1 AND queue_origin = ?2",
                params![epoch, origin],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| "no local cutover matches this epoch and endpoint".into())
    }

    pub fn resume_after_cutover(
        &mut self,
        epoch: &str,
        origin: &str,
        returned_work: &[String],
    ) -> Result<()> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<(String, bool)> = tx.query_row(
            "SELECT phase, ci_was_paused FROM authority_transfers WHERE epoch = ?1 AND queue_origin = ?2",
            params![epoch, origin],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let (phase, paused) = previous.ok_or("no local cutover matches this epoch and endpoint")?;
        if phase == "frozen" {
            for work_id in returned_work {
                tx.execute(
                    "INSERT INTO archived_shared_orders (work_id, epoch) VALUES (?1, ?2)
                     ON CONFLICT (work_id) DO NOTHING",
                    params![work_id, epoch],
                )?;
            }
            tx.execute(
                "UPDATE authority_transfers SET phase = 'released' WHERE epoch = ?1",
                [epoch],
            )?;
            tx.execute(
                "UPDATE ci_intake_control SET paused = ?1 WHERE id = 1",
                [paused],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn record_ci_poll(
        &mut self,
        repository: &str,
        evidence: Result<Option<WorkOrder>>,
        snapshot_path: Option<&Path>,
        admit: bool,
        daily_limit: u32,
        branch: Option<CiPollBranch<'_>>,
    ) -> Result<serde_json::Value> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let paused: bool = tx.query_row(
            "SELECT COALESCE((SELECT paused FROM ci_intake_control WHERE id = 1), 0)",
            [],
            |row| row.get(0),
        )?;
        let mut work_order_id = None;
        let mut failure = None;
        let (decision, reason) = match evidence {
            Ok(Some(order)) => {
                work_order_id = Some(order.id.clone());
                failure = order
                    .ci_failure
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?;
                Self::poll_admission(&tx, &order, snapshot_path, paused, admit, daily_limit)?
            }
            Ok(None) => ("no_eligible_failure", None),
            Err(error) => ("blocked", Some(error.to_string())),
        };
        tx.execute(
            "INSERT INTO ci_poll_receipts (repository, mode, decision, work_order_id, reason, failure)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![repository, if admit { "admit" } else { "shadow" }, decision, work_order_id, reason, failure],
        )?;
        let sequence = tx.last_insert_rowid();
        let branch = branch.map(serde_json::to_value).transpose()?;
        if let Some(branch) = &branch {
            tx.execute(
                "INSERT INTO ci_poll_branch_observations (sequence, payload) VALUES (?1, ?2)",
                params![sequence, branch.to_string()],
            )?;
        }
        tx.commit()?;
        let mut receipt = serde_json::json!({
            "schema_version": 1, "sequence": sequence, "mode": if admit { "admit" } else { "shadow" },
            "repository": repository, "decision": decision, "reason": reason, "work_order_id": work_order_id,
            "ci_failure": failure.as_deref().map(serde_json::from_str::<serde_json::Value>).transpose()?,
        });
        if let Some(branch) = branch {
            receipt["branch"] = branch;
        }
        Ok(receipt)
    }

    pub fn ci_poll_history(&self, limit: u32) -> Result<Vec<serde_json::Value>> {
        let mut query = self.0.prepare(
            "SELECT json_object('schema_version', 1, 'sequence', sequence,
                'observed_at', observed_at, 'repository', repository, 'mode', mode,
                'decision', decision, 'reason', reason, 'work_order_id', work_order_id,
                'ci_failure', json(failure)), branch.payload
             FROM ci_poll_receipts LEFT JOIN ci_poll_branch_observations branch USING (sequence)
             ORDER BY sequence DESC LIMIT ?1",
        )?;
        query
            .query_map([limit], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .map(|row| {
                let (receipt, branch) = row?;
                let mut receipt: serde_json::Value = serde_json::from_str(&receipt)?;
                if let Some(branch) = branch {
                    receipt["branch"] = serde_json::from_str(&branch)?;
                }
                Ok(receipt)
            })
            .collect()
    }

    fn poll_admission(
        tx: &Transaction<'_>,
        order: &WorkOrder,
        snapshot_path: Option<&Path>,
        paused: bool,
        admit: bool,
        daily_limit: u32,
    ) -> Result<(&'static str, Option<String>)> {
        if paused {
            return Ok(("paused", Some("CI intake is paused".into())));
        }
        let admitted: u32 = tx.query_row(
            "SELECT COUNT(*) FROM ci_poll_receipts
             WHERE decision = 'admitted' AND observed_at > unixepoch() - 86400",
            [],
            |row| row.get(0),
        )?;
        tx.execute_batch("SAVEPOINT ci_poll")?;
        let staged = Self::stage_delegation(tx, order, snapshot_path);
        let decision = match staged {
            Ok(true) => ("already_admitted", None),
            Ok(false) if admitted >= daily_limit => (
                "budget_exhausted",
                Some("CI intake reached its rolling 24-hour admission limit".into()),
            ),
            Ok(false) if admit => ("admitted", None),
            Ok(false) => ("would_admit", None),
            Err(error) => ("blocked", Some(error.to_string())),
        };
        if decision.0 != "admitted" {
            tx.execute_batch("ROLLBACK TO ci_poll")?;
        }
        tx.execute_batch("RELEASE ci_poll")?;
        Ok(decision)
    }

    fn stage_delegation(
        tx: &Transaction<'_>,
        order: &WorkOrder,
        snapshot_path: Option<&Path>,
    ) -> Result<bool> {
        ensure_local_authority(tx)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT work_order FROM work_orders WHERE id = ?1",
                [&order.id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = &existing {
            if serde_json::from_str::<WorkOrder>(existing)? != *order {
                return reject("work order ID is already bound to different inputs");
            }
        } else {
            let archived: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM archived_shared_orders WHERE work_id = ?1)",
                [&order.id],
                |row| row.get(0),
            )?;
            if archived {
                return reject("work order was already archived from the shared queue");
            }
            if let Some(failure) = &order.ci_failure {
                let duplicate: Option<String> = match failure {
                    crate::order::CiFailure::Bitbucket(failure) => tx.query_row(
                        "SELECT id FROM work_orders
                     WHERE json_extract(work_order, '$.ci_failure.repository') = ?1
                       AND json_extract(work_order, '$.ci_failure.pipeline_uuid') = ?2
                       AND json_extract(work_order, '$.ci_failure.step_uuid') = ?3
                       AND json_extract(work_order, '$.ci_failure.completed_on') = ?4",
                        params![
                            failure.repository,
                            failure.pipeline_uuid,
                            failure.step_uuid,
                            failure.completed_on
                        ],
                        |row| row.get(0),
                    ),
                    crate::order::CiFailure::Github(failure) => tx.query_row(
                        "SELECT id FROM work_orders
                         WHERE json_extract(work_order, '$.ci_failure.repository') = ?1 COLLATE NOCASE
                           AND json_extract(work_order, '$.ci_failure.run_id') = ?2
                           AND json_extract(work_order, '$.ci_failure.run_attempt') = ?3
                           AND json_extract(work_order, '$.ci_failure.job_id') = ?4",
                        params![failure.repository, i64::try_from(failure.run_id)?,
                                i64::try_from(failure.run_attempt)?, i64::try_from(failure.job_id)?],
                        |row| row.get(0),
                    ),
                }.optional()?;
                if let Some(duplicate) = duplicate {
                    return reject(format!(
                        "this failed CI attempt is already delegated as {duplicate}"
                    ));
                }
            }
            let occupied: Option<String> = tx
                .query_row("SELECT id FROM work_orders WHERE active = 1", [], |row| {
                    row.get(0)
                })
                .optional()?;
            if let Some(id) = occupied {
                return reject(format!("the local review slot is occupied by {id}"));
            }
            tx.execute(
                "INSERT INTO work_orders (id, work_order, active, state)
                 VALUES (?1, ?2, 1, 'delegated')",
                params![order.id, serde_json::to_string(order)?],
            )?;
            record(tx, &order.id, "delegated", None, "")?;
            if let Some(failure) = &order.ci_failure {
                record(
                    tx,
                    &order.id,
                    "ci_failure_observed",
                    Some(failure.commit()),
                    failure.evidence_url(),
                )?;
            }
        }
        if let Some(path) = snapshot_path {
            attach_snapshot(tx, order, path)?;
        }
        check_snapshot(tx, order)?;
        Ok(existing.is_some())
    }

    pub fn pin_snapshot(&mut self, id: &str, path: &Path) -> Result<Receipt> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_local_authority(&tx)?;
        let (order, _, _) = read_order(&tx, id)?;
        attach_snapshot(&tx, &order, path)?;
        finish(tx, id)
    }

    pub fn verify_snapshot(&mut self, id: &str) -> Result<()> {
        let tx = self.0.transaction()?;
        let (order, _, _) = read_order(&tx, id)?;
        check_snapshot(&tx, &order)?;
        tx.commit()?;
        Ok(())
    }

    pub fn status(&mut self, id: &str) -> Result<Receipt> {
        finish(self.0.transaction()?, id)
    }

    pub fn runner_event(&mut self, id: &str, kind: &str, note: &str) -> Result<()> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (_, _, head) = read_order(&tx, id)?;
        record(&tx, id, kind, head.as_deref(), note)?;
        tx.commit()?;
        Ok(())
    }

    pub fn ci_dispatch(&mut self) -> Result<CiDispatch> {
        let tx = self.0.transaction()?;
        read_ci_dispatch(&tx)
    }

    pub fn require_standard_runner(&self, current: &Receipt) -> Result<()> {
        if current.state == State::Delegated
            && current.work_order.ci_failure.is_some()
            && self.0.query_row(
                "SELECT EXISTS(SELECT 1 FROM ci_poll_receipts WHERE work_order_id = ?1 AND decision = 'admitted')",
                [&current.work_order.id],
                |row| row.get::<_, bool>(0),
            )?
        {
            return reject("start a scheduled repair with run-ci-repair so the forge failure is rechecked");
        }
        Ok(())
    }

    pub fn record_ci_recheck_block(&mut self, id: &str, attempt: CiAttempt) -> Result<()> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if attempt.can_start(&read_ci_dispatch(&tx)?, id) {
            let recorded: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE work_id = ?1 AND kind = 'ci_dispatch_blocked')",
                [id],
                |row| row.get(0),
            )?;
            if !recorded {
                record(
                    &tx,
                    id,
                    "ci_dispatch_blocked",
                    None,
                    "The pinned CI failure could not be confirmed before dispatch",
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn start_runner(
        &mut self,
        id: &str,
        note: &str,
        ci_attempt: Option<CiAttempt>,
    ) -> Result<()> {
        let Some(attempt) = ci_attempt else {
            return self.runner_event(id, "runner_started", note);
        };
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !attempt.can_start(&read_ci_dispatch(&tx)?, id) {
            return reject("scheduled CI repair is no longer eligible to start");
        }
        record(&tx, id, "runner_started", None, note)?;
        tx.commit()?;
        Ok(())
    }

    pub fn start_check(&mut self, id: &str, draft: &Draft) -> Result<i64> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_candidate(&tx, id, draft)?;
        record(&tx, id, "native_check_started", Some(&draft.head), "")?;
        let sequence = tx.last_insert_rowid();
        tx.commit()?;
        Ok(sequence)
    }

    pub fn check_event(
        &mut self,
        id: &str,
        draft: &Draft,
        sequence: i64,
        kind: &str,
        details: &str,
    ) -> Result<Receipt> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_candidate(&tx, id, draft)?;
        let latest: i64 = tx.query_row(
            "SELECT MAX(sequence) FROM events
             WHERE work_id = ?1 AND candidate = ?2 AND kind = 'native_check_started'",
            params![id, draft.head],
            |row| row.get(0),
        )?;
        if latest != sequence {
            return reject("a newer verification superseded this provider check");
        }
        if matches!(kind, "github_checks_observed" | "bitbucket_checks_observed")
            && let Some(pinned) = pinned_merge_approval_policy(&tx, id)?
        {
            let observation: serde_json::Value = serde_json::from_str(details)?;
            if observation["merge_approval_policy"].as_str() != Some(pinned.as_str()) {
                return reject("the merge approval policy is pinned for this work order");
            }
        }
        record(&tx, id, kind, Some(&draft.head), details)?;
        finish(tx, id)
    }

    pub fn submit(
        &mut self,
        id: &str,
        candidate: &str,
        replaces: Option<&str>,
        note: &str,
    ) -> Result<Receipt> {
        if note.trim().is_empty() {
            return reject("a handoff note is required");
        }
        let before = self.status(id)?;
        repository::validate_base(&before.work_order)?;
        repository::candidate_paths(&before.work_order, candidate)?;
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (order, state, current) = read_order(&tx, id)?;
        check_snapshot(&tx, &order)?;
        if state == State::Candidate && current.as_deref() == Some(candidate) {
            return finish(tx, id);
        }
        let start = submission_start(&order, state, current.as_deref(), candidate, replaces)?;
        repository::candidate_history_paths(&order, start, candidate)?;
        repository::retain_candidate(&before.work_order, candidate)?;
        tx.execute(
            "UPDATE work_orders SET state = 'candidate', candidate = ?2 WHERE id = ?1",
            params![id, candidate],
        )?;
        record(&tx, id, "candidate_submitted", Some(candidate), note)?;
        finish(tx, id)
    }

    pub fn submit_from_checkout(
        &mut self,
        id: &str,
        checkout: &Path,
        candidate: &str,
        replaces: Option<&str>,
        note: &str,
    ) -> Result<Receipt> {
        if note.trim().is_empty() {
            return reject("a handoff note is required");
        }
        self.verify_snapshot(id)?;
        let receipt = self.status(id)?;
        if receipt.state != State::Candidate || receipt.candidate.as_deref() != Some(candidate) {
            let order = &receipt.work_order;
            let start = submission_start(
                order,
                receipt.state,
                receipt.candidate.as_deref(),
                candidate,
                replaces,
            )?;
            repository::validate_base(order)?;
            repository::import_interactive_commit(order, checkout, start, candidate)?;
        }
        self.submit(id, candidate, replaces, note)
    }

    pub fn bind_draft(&mut self, id: &str, draft: Draft) -> Result<Receipt> {
        draft.validate()?;
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (order, state, candidate) = read_order(&tx, id)?;
        check_snapshot(&tx, &order)?;
        if let Some(failure) = &order.ci_failure {
            let prefix = failure.pull_request_prefix();
            if draft.reference.strip_prefix(&prefix).is_none_or(|number| {
                !number.bytes().all(|byte| byte.is_ascii_digit())
                    || number.parse::<u64>().ok().is_none_or(|id| id == 0)
            }) {
                return reject(
                    "bind the CI repair to a pull request in the repository that failed",
                );
            }
        }
        if candidate.as_deref() != Some(&draft.head) {
            return reject("the draft head differs from the current candidate");
        }
        if order.ci_verification_policy_digest.is_some()
            && read_draft(&tx, id)?.is_none()
            && !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE work_id = ?1 AND kind = 'ci_draft_verified' AND candidate = ?2)",
                params![id, draft.head],
                |row| row.get::<_, bool>(0),
            )?
        {
            return reject("the first draft requires pinned independent CI repair verification");
        }
        if let Some(previous) = read_draft(&tx, id)? {
            if previous.reference != draft.reference {
                return reject("work order is already bound to a different pull request");
            }
            if previous == draft && read_hold(&tx, id)?.is_none() {
                return finish(tx, id);
            }
        }
        if state != State::Candidate {
            return reject("a submitted candidate is required to bind a draft");
        }
        tx.execute(
            "INSERT INTO drafts (work_id, reference, head) VALUES (?1, ?2, ?3)
             ON CONFLICT(work_id) DO UPDATE SET head = excluded.head",
            params![id, draft.reference, draft.head],
        )?;
        if tx.execute("DELETE FROM publication_holds WHERE work_id = ?1", [id])? != 0 {
            record(
                &tx,
                id,
                "publication_reconciled",
                Some(&draft.head),
                &draft.reference,
            )?;
        }
        record(
            &tx,
            id,
            "draft_attested",
            Some(&draft.head),
            &draft.reference,
        )?;
        finish(tx, id)
    }

    pub fn retry_ci(&mut self, id: &str, previous: &str, next: &str) -> Result<Receipt> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (order, state, current) = read_order(&tx, id)?;
        check_snapshot(&tx, &order)?;
        if state != State::Candidate
            || current.as_deref() != Some(previous)
            || read_hold(&tx, id)?.is_some()
            || read_draft(&tx, id)?.is_none_or(|draft| draft.head != previous)
        {
            return reject(
                "a CI retry requires a published, unverified current candidate with no publication hold",
            );
        }
        repository::verify_retry_head(&order, previous, next)?;
        repository::retain_candidate(&order, next)?;
        tx.execute(
            "UPDATE work_orders SET candidate = ?2 WHERE id = ?1",
            params![id, next],
        )?;
        record(&tx, id, "ci_retry_head", Some(next), previous)?;
        finish(tx, id)
    }

    pub fn hold_publication(&mut self, id: &str, head: &str, reason: &str) -> Result<Receipt> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (_, state, current) = read_order(&tx, id)?;
        if state != State::Candidate
            || head.is_empty()
            || current.as_deref() != Some(head)
            || reason.trim().is_empty()
        {
            return reject("a publication hold needs the current candidate head and a reason");
        }
        if let Some(existing) = read_hold(&tx, id)? {
            if existing != reason {
                return reject("the candidate already has an unresolved publication");
            }
        } else {
            tx.execute(
                "INSERT INTO publication_holds (work_id, reason) VALUES (?1, ?2)",
                params![id, reason],
            )?;
            record(&tx, id, "publication_held", Some(head), reason)?;
        }
        finish(tx, id)
    }

    pub(crate) fn reserve_verified_ci_publication(
        &mut self,
        id: &str,
        head: &str,
        pin: &str,
        proof: &str,
    ) -> Result<()> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (order, state, candidate) = read_order(&tx, id)?;
        check_snapshot(&tx, &order)?;
        if state != State::Candidate
            || candidate.as_deref() != Some(head)
            || order.ci_verification_policy_digest.as_deref() != Some(pin)
            || read_draft(&tx, id)?.is_some()
            || read_hold(&tx, id)?.is_some()
        {
            return reject(
                "the verified candidate is stale or its draft publication is already reserved",
            );
        }
        tx.execute(
            "INSERT INTO publication_holds (work_id, reason) VALUES (?1, ?2)",
            params![id, "guarded GitHub draft publication requires confirmation"],
        )?;
        record(&tx, id, "ci_draft_verified", Some(head), proof)?;
        tx.commit()?;
        Ok(())
    }

    pub fn clear_publication(&mut self, id: &str, review: ReviewRecord) -> Result<Receipt> {
        review.validate()?;
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (_, state, current) = read_order(&tx, id)?;
        if state != State::Candidate
            || current.as_deref() != Some(&review.draft.head)
            || read_hold(&tx, id)?.is_none()
        {
            return reject("no publication hold matches this candidate");
        }
        if let Some(draft) = read_draft(&tx, id)?
            && draft.reference != review.draft.reference
        {
            return reject("the observation must concern the bound pull request");
        }
        tx.execute("DELETE FROM publication_holds WHERE work_id = ?1", [id])?;
        record(
            &tx,
            id,
            "publication_absence_attested",
            Some(&review.draft.head),
            &serde_json::to_string(&review)?,
        )?;
        finish(tx, id)
    }

    pub fn attest(
        &mut self,
        id: &str,
        action: Attestation,
        review: ReviewRecord,
    ) -> Result<Receipt> {
        self.attest_with_handoff(id, action, review, None)
    }

    pub fn issue_verification_handoff(
        &mut self,
        id: &str,
        head: &str,
        paths: Vec<String>,
    ) -> Result<VerificationHandoff> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (order, state, candidate) = read_order(&tx, id)?;
        check_snapshot(&tx, &order)?;
        let draft = read_draft(&tx, id)?
            .ok_or("bind the published draft before preparing a verification handoff")?;
        if candidate.as_deref() != Some(head)
            || draft.head != head
            || !matches!(state, State::Candidate | State::VerificationAttested)
            || read_hold(&tx, id)?.is_some()
        {
            return reject("the draft is no longer the current verification candidate");
        }
        if !matches!(
            native_ci(&tx, &order, Some(head))?,
            NativeCi::Manual | NativeCi::Passed
        ) {
            return reject(
                "native CI must pass for the current candidate before preparing a verification handoff",
            );
        }
        let native_check_sequence = native_check_sequence(&tx, id, head)?;
        let native_evidence = if let Some(sequence) = native_check_sequence {
            let note: String = tx.query_row(
                "SELECT note FROM events WHERE work_id = ?1 AND candidate = ?2 AND sequence > ?3
                 AND kind IN ('github_checks_observed', 'bitbucket_checks_observed')
                 ORDER BY sequence DESC LIMIT 1",
                params![id, head, sequence],
                |row| row.get(0),
            )?;
            Some(serde_json::from_str(&note)?)
        } else {
            None
        };
        let packet = VerificationHandoff {
            schema_version: 1,
            work_order_id: order.id,
            draft,
            base: order.base,
            issue: order.issue,
            intent: order.intent,
            acceptance: order.acceptance,
            scope: order.scope,
            changed_paths: paths,
            context: order.context,
            skill: order.skill,
            native_check_sequence,
            native_evidence,
        };
        record(
            &tx,
            id,
            "verification_handoff_issued",
            Some(head),
            &packet.digest()?,
        )?;
        tx.commit()?;
        Ok(packet)
    }

    pub fn attest_with_handoff(
        &mut self,
        id: &str,
        action: Attestation,
        review: ReviewRecord,
        handoff: Option<VerificationHandoff>,
    ) -> Result<Receipt> {
        self.attest_inner(id, action, review, handoff, None)
    }

    pub fn close_with_native_merge(
        &mut self,
        id: &str,
        merge: MergeObservation,
    ) -> Result<Receipt> {
        let review = merge.review_record();
        self.attest_inner(id, Attestation::Close, review, None, Some(merge))
    }

    fn attest_inner(
        &mut self,
        id: &str,
        action: Attestation,
        review: ReviewRecord,
        handoff: Option<VerificationHandoff>,
        merge: Option<MergeObservation>,
    ) -> Result<Receipt> {
        review.validate()?;
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (order, state, candidate) = read_order(&tx, id)?;
        check_snapshot(&tx, &order)?;
        if read_hold(&tx, id)?.is_some() {
            return reject("reconcile the unresolved publication before recording review evidence");
        }
        if merge.as_ref().is_some_and(|merge| {
            merge.pinned_base != order.base
                || candidate.as_deref() != Some(merge.draft.head.as_str())
        }) {
            return reject("the observed merge does not match the recorded candidate and base");
        }
        let (kind, next, active) = match action {
            Attestation::Verification => ("verification_attested", "verification_attested", true),
            Attestation::ChangesRequested => {
                ("changes_requested_attested", "repair_requested", true)
            }
            Attestation::Close => ("outcome_attested", "closed", false),
        };
        let binding = match handoff {
            Some(packet) => {
                let digest = packet.digest()?;
                if !matches!(action, Attestation::Verification)
                    || packet.work_order_id != id
                    || packet.base != order.base
                    || packet.draft != review.draft
                    || candidate.as_deref() != Some(packet.draft.head.as_str())
                    || read_draft(&tx, id)?.as_ref() != Some(&packet.draft)
                    || native_check_sequence(&tx, id, &packet.draft.head)?
                        != packet.native_check_sequence
                    || !tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM events WHERE work_id = ?1 AND candidate = ?2
                         AND kind = 'verification_handoff_issued' AND note = ?3)",
                        params![id, packet.draft.head, digest],
                        |row| row.get::<_, bool>(0),
                    )?
                {
                    return reject(
                        "verification handoff was changed or no longer matches the current draft and CI",
                    );
                }
                Some(serde_json::json!({
                    "handoff_digest": digest, "native_check_sequence": packet.native_check_sequence,
                    "evidence": review.evidence,
                }))
            }
            None => None,
        };
        let payload = serde_json::to_string(&review)?;
        let previous: Option<String> = tx.query_row(
            "SELECT payload FROM attestations WHERE work_id = ?1 AND kind = ?2 AND evidence = ?3",
            params![id, kind, review.evidence], |row| row.get(0),
        ).optional()?;
        if let Some(previous) = previous {
            if serde_json::from_str::<ReviewRecord>(&previous)? != review {
                return reject("the evidence reference is already bound to different inputs");
            }
            if let Some(merge) = merge
                && read_native_merge(&tx, id)?.as_ref() != Some(&merge)
            {
                return reject("this work order already recorded a different merge outcome");
            }
            return finish(tx, id);
        }
        if candidate.as_deref() != Some(&review.draft.head)
            || read_draft(&tx, id)?.as_ref() != Some(&review.draft)
        {
            return reject("the evidence does not describe the current pull request and candidate");
        }
        let ci = native_ci(&tx, &order, candidate.as_deref())?;
        if merge.is_some() && ci != NativeCi::Passed {
            return reject(
                "native CI must pass on the reviewed head before recording a forge merge",
            );
        }
        if matches!(action, Attestation::Verification | Attestation::Close)
            && matches!(
                ci,
                NativeCi::Required | NativeCi::Pending | NativeCi::Unavailable
            )
        {
            return reject(
                "native CI must pass for the current candidate before recording verification or closing",
            );
        }
        if matches!(action, Attestation::Verification)
            && binding.is_none()
            && ci_poll_admitted(&tx, id)?
        {
            return reject("an admitted CI repair requires an issued verification handoff");
        }
        if matches!(action, Attestation::Close)
            && !verification_binding_current(&tx, id, &review.draft.head)?
        {
            return reject(
                "record independent verification against the current CI handoff before closing",
            );
        }
        if matches!(action, Attestation::Close)
            && merge
                .as_ref()
                .is_none_or(|merge| merge.approvals.is_empty())
            && pinned_merge_approval_policy(&tx, id)?.is_some()
        {
            return reject(
                "this work order requires native approval evidence from the forge closure command",
            );
        }
        let valid = matches!(
            (action, state),
            (Attestation::Verification, State::Candidate)
                | (
                    Attestation::ChangesRequested | Attestation::Close,
                    State::VerificationAttested
                )
        ) || (matches!(action, Attestation::Verification)
            && state == State::VerificationAttested
            && binding.is_some())
            || (matches!(
                (action, state),
                (Attestation::ChangesRequested, State::Candidate)
            ) && ci == NativeCi::Unavailable);
        if !valid {
            return reject("the current state does not accept this attestation");
        }
        tx.execute(
            "INSERT INTO attestations (work_id, kind, evidence, payload) VALUES (?1, ?2, ?3, ?4)",
            params![id, kind, review.evidence, payload],
        )?;
        tx.execute(
            "UPDATE work_orders SET state = ?2, active = ?3 WHERE id = ?1",
            params![id, next, active],
        )?;
        record(&tx, id, kind, Some(&review.draft.head), &review.evidence)?;
        if let Some(merge) = merge {
            record(
                &tx,
                id,
                "native_merge_observed",
                Some(&merge.draft.head),
                &serde_json::to_string(&merge)?,
            )?;
        }
        if let Some(binding) = binding {
            record(
                &tx,
                id,
                "verification_handoff_attested",
                Some(&review.draft.head),
                &binding.to_string(),
            )?;
        }
        finish(tx, id)
    }

    pub fn cancel(&mut self, id: &str, reason: &str) -> Result<Receipt> {
        if reason.trim().is_empty() {
            return reject("a cancellation reason is required");
        }
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (_, state, candidate) = read_order(&tx, id)?;
        if read_hold(&tx, id)?.is_some() {
            return reject("reconcile the unresolved publication before releasing the slot");
        }
        if state == State::Closed {
            return reject("a closed work order cannot be cancelled");
        }
        if state != State::Cancelled {
            tx.execute(
                "UPDATE work_orders SET active = 0, state = 'cancelled' WHERE id = ?1",
                [id],
            )?;
            record(&tx, id, "cancelled", candidate.as_deref(), reason)?;
        }
        finish(tx, id)
    }
}

fn read_ci_dispatch(tx: &Transaction<'_>) -> Result<CiDispatch> {
    ensure_local_authority(tx)?;
    let paused: bool = tx.query_row(
        "SELECT COALESCE((SELECT paused FROM ci_intake_control WHERE id = 1), 0)",
        [],
        |row| row.get(0),
    )?;
    if paused {
        return Ok(CiDispatch::Paused);
    }
    let pending: Option<(String, bool)> = tx.query_row(
        "SELECT w.id, EXISTS(SELECT 1 FROM events WHERE work_id = w.id AND kind = 'runner_started')
         FROM work_orders AS w
         WHERE w.active = 1 AND w.state = 'delegated' AND w.candidate IS NULL
           AND EXISTS(SELECT 1 FROM ci_poll_receipts WHERE work_order_id = w.id AND decision = 'admitted')",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    Ok(match pending {
        Some((id, true)) => CiDispatch::NeedsOperator(id),
        Some((id, false)) => CiDispatch::Ready(id),
        None => CiDispatch::Idle,
    })
}

fn ensure_local_authority(tx: &Transaction<'_>) -> Result<()> {
    let frozen: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM authority_transfers WHERE phase = 'frozen')",
        [],
        |row| row.get(0),
    )?;
    if frozen {
        return reject("local queue is frozen for shared cutover");
    }
    Ok(())
}

fn check_candidate(tx: &Transaction<'_>, id: &str, draft: &Draft) -> Result<()> {
    let (order, state, head) = read_order(tx, id)?;
    check_snapshot(tx, &order)?;
    if !matches!(state, State::Candidate | State::VerificationAttested)
        || head.as_deref() != Some(draft.head.as_str())
        || read_draft(tx, id)?.as_ref() != Some(draft)
        || read_hold(tx, id)?.is_some()
    {
        return reject("pull request, candidate, or publication hold changed during verification");
    }
    Ok(())
}

fn attach_snapshot(tx: &Transaction<'_>, order: &WorkOrder, path: &Path) -> Result<()> {
    let path = path.canonicalize()?;
    snapshot::verify(order, &path)?;
    let path = path
        .to_str()
        .ok_or("the canonical issue snapshot path is not Unicode")?;
    let changed = tx.execute(
        "INSERT INTO jira_snapshots (work_id, path) VALUES (?1, ?2)
         ON CONFLICT(work_id) DO UPDATE SET path = excluded.path
         WHERE jira_snapshots.path <> excluded.path",
        params![order.id, path],
    )?;
    if changed != 0 {
        let event = if snapshot::reference(order)?.is_some_and(snapshot::is_github_issue) {
            "github_issue_snapshot_verified"
        } else {
            "jira_snapshot_verified"
        };
        record(tx, &order.id, event, None, &order.issue)?;
    }
    Ok(())
}

fn check_snapshot(tx: &Transaction<'_>, order: &WorkOrder) -> Result<()> {
    if snapshot::reference(order)?.is_none() {
        return Ok(());
    }
    snapshot::verify(order, Path::new(&snapshot_path(tx, &order.id)?))
}

fn snapshot_path(connection: &Connection, id: &str) -> Result<String> {
    connection
        .query_row(
            "SELECT path FROM jira_snapshots WHERE work_id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| "attach the digest-pinned issue snapshot before proceeding".into())
}

#[derive(Clone, Copy)]
pub enum Attestation {
    Verification,
    ChangesRequested,
    Close,
}

fn read_draft(tx: &Transaction<'_>, id: &str) -> Result<Option<Draft>> {
    Ok(tx
        .query_row(
            "SELECT reference, head FROM drafts WHERE work_id = ?1",
            [id],
            |row| {
                Ok(Draft {
                    reference: row.get(0)?,
                    head: row.get(1)?,
                })
            },
        )
        .optional()?)
}

fn read_native_merge(tx: &Transaction<'_>, id: &str) -> Result<Option<MergeObservation>> {
    let payload: Option<String> = tx.query_row(
        "SELECT note FROM events WHERE work_id = ?1 AND kind = 'native_merge_observed' ORDER BY sequence DESC LIMIT 1",
        [id], |row| row.get(0),
    ).optional()?;
    Ok(payload.as_deref().map(serde_json::from_str).transpose()?)
}

fn read_hold(tx: &Transaction<'_>, id: &str) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT reason FROM publication_holds WHERE work_id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?)
}

fn record(
    tx: &Transaction<'_>,
    id: &str,
    kind: &str,
    candidate: Option<&str>,
    note: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO events (work_id, kind, candidate, note) VALUES (?1, ?2, ?3, ?4)",
        params![id, kind, candidate, note],
    )?;
    Ok(())
}

fn submission_start<'a>(
    order: &'a WorkOrder,
    state: State,
    current: Option<&str>,
    candidate: &str,
    replaces: Option<&'a str>,
) -> Result<&'a str> {
    match (state, replaces) {
        (State::Delegated, None) => Ok(&order.base),
        (State::RepairRequested, Some(previous))
            if current == Some(previous) && previous != candidate =>
        {
            Ok(previous)
        }
        _ => reject(
            "only an active delegated order or a repair against the expected prior head can accept a candidate",
        ),
    }
}

fn read_order(tx: &Transaction<'_>, id: &str) -> Result<(WorkOrder, State, Option<String>)> {
    let (order, state, candidate): (String, String, Option<String>) = tx
        .query_row(
            "SELECT work_order, state, candidate FROM work_orders WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .ok_or("unknown work order")?;
    let state = match state.as_str() {
        "delegated" => State::Delegated,
        "candidate" => State::Candidate,
        "verification_attested" => State::VerificationAttested,
        "repair_requested" => State::RepairRequested,
        "closed" => State::Closed,
        "cancelled" => State::Cancelled,
        _ => return reject("unknown stored state"),
    };
    Ok((serde_json::from_str(&order)?, state, candidate))
}

fn finish(tx: Transaction<'_>, id: &str) -> Result<Receipt> {
    let receipt = read_receipt(&tx, id)?;
    tx.commit()?;
    Ok(receipt)
}

fn read_receipt(tx: &Transaction<'_>, id: &str) -> Result<Receipt> {
    let (work_order, state, candidate) = read_order(tx, id)?;
    let draft = read_draft(tx, id)?;
    let publication_hold = read_hold(tx, id)?;
    let native_ci = native_ci(tx, &work_order, candidate.as_deref())?;
    let snapshot_problem = if matches!(state, State::Candidate | State::VerificationAttested) {
        check_snapshot(tx, &work_order)
            .err()
            .map(|error| error.to_string())
    } else {
        None
    };
    let review_ready = state == State::VerificationAttested
        && publication_hold.is_none()
        && snapshot_problem.is_none()
        && candidate
            .as_deref()
            .is_some_and(|head| draft.as_ref().is_some_and(|draft| draft.head == head))
        && matches!(native_ci, NativeCi::Manual | NativeCi::Passed)
        && verification_binding_current(tx, id, candidate.as_deref().unwrap_or_default())?;
    let attestations = {
        let mut query =
            tx.prepare("SELECT kind, payload FROM attestations WHERE work_id = ?1 ORDER BY rowid")?;
        query
            .query_map([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| -> Result<_> {
                let (kind, payload) = row?;
                Ok(RecordedAttestation {
                    kind,
                    record: serde_json::from_str(&payload)?,
                })
            })
            .collect::<Result<Vec<_>>>()?
    };
    let events = {
        let mut query = tx.prepare(
            "SELECT kind, candidate, note FROM events WHERE work_id = ?1 ORDER BY sequence",
        )?;
        query
            .query_map([id], |row| {
                Ok(Event {
                    kind: row.get(0)?,
                    candidate: row.get(1)?,
                    note: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?
    };
    Ok(Receipt {
        schema_version: 1,
        work_order,
        state,
        candidate,
        draft,
        publication_hold,
        events,
        attestations,
        native_merge: if state == State::Closed {
            read_native_merge(tx, id)?
        } else {
            None
        },
        native_ci,
        snapshot_problem,
        review_ready,
    })
}

fn ci_poll_admitted(tx: &Transaction<'_>, id: &str) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM ci_poll_receipts WHERE work_order_id = ?1 AND decision = 'admitted')",
        [id], |row| row.get(0),
    )?)
}

fn pinned_merge_approval_policy(tx: &Transaction<'_>, id: &str) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT json_extract(note, '$.merge_approval_policy')
         FROM events WHERE work_id = ?1 AND kind IN ('github_checks_observed', 'bitbucket_checks_observed')
         AND json_type(note, '$.merge_approval_policy') = 'text'
         ORDER BY sequence DESC LIMIT 1",
            [id],
            |row| row.get(0),
        )
        .optional()?)
}

fn native_check_sequence(tx: &Transaction<'_>, id: &str, head: &str) -> Result<Option<i64>> {
    Ok(tx.query_row(
        "SELECT MAX(sequence) FROM events WHERE work_id = ?1 AND candidate = ?2 AND kind = 'native_check_started'",
        params![id, head], |row| row.get(0),
    )?)
}

fn verification_binding_current(tx: &Transaction<'_>, id: &str, head: &str) -> Result<bool> {
    let binding: Option<String> = tx
        .query_row(
            "SELECT note FROM events WHERE work_id = ?1 AND candidate = ?2
         AND kind = 'verification_handoff_attested' ORDER BY sequence DESC LIMIT 1",
            params![id, head],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(binding) = binding {
        let note: serde_json::Value = serde_json::from_str(&binding)?;
        return Ok(note["native_check_sequence"]
            == serde_json::json!(native_check_sequence(tx, id, head)?));
    }
    Ok(!ci_poll_admitted(tx, id)?)
}

fn native_ci(tx: &Transaction<'_>, order: &WorkOrder, head: Option<&str>) -> Result<NativeCi> {
    let latest: Option<(String, String, String)> = tx
        .query_row(
            "SELECT candidate, kind, note FROM events
         WHERE work_id = ?1 AND kind IN
            ('bitbucket_checks_observed', 'bitbucket_checks_unavailable',
             'github_checks_observed', 'github_checks_unavailable', 'native_check_started')
         ORDER BY (candidate IS ?2) DESC, sequence DESC LIMIT 1",
            params![order.id, head],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(match latest {
        Some((candidate, kind, note)) if Some(candidate.as_str()) == head => match kind.as_str() {
            "native_check_started" => NativeCi::Pending,
            "github_checks_observed" | "bitbucket_checks_observed" => {
                let evidence: serde_json::Value = serde_json::from_str(&note)?;
                if evidence["head"] == candidate
                    && evidence["base"] == order.base
                    && (kind == "github_checks_observed" || evidence["transport"] == "twg")
                {
                    NativeCi::Passed
                } else {
                    NativeCi::Unavailable
                }
            }
            _ => NativeCi::Unavailable,
        },
        Some(_) => NativeCi::Required,
        None if order.require_native_checks => NativeCi::Required,
        None => NativeCi::Manual,
    })
}
