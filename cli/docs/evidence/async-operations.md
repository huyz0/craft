# Remote job polling

Date: **15 November 2026**, Australia/Sydney. Windows x86_64 MSVC, pinned Rust 1.99.1. The [design section](../../openspec/changes/build-cloud-cli/design.md#remote-job-polling) and the operation-recovery scenarios "Polling deadline", "Waiting on a recorded job" and "Nothing to poll" are the contract.

## What changed

- `craft_core::contracts::JobObservation` carries a polled job's state, native status, progress and elapsed time. Native result and message text never enter it.
- `crates/craft-provider-jira/src/tasks.rs` registers `jira.task.get` with one route, `GET /rest/api/3/task/{taskId}`, scope `read:jira-work`. `ENQUEUED` maps to accepted, `RUNNING` and `CANCEL_REQUESTED` to running, `COMPLETE` to succeeded, and `FAILED`, `CANCELLED` and `DEAD` to failed with the native status kept. Any other status, a mismatched task ID or an out-of-range progress value is malformed.
- `crates/craft-provider-confluence/src/longtasks.rs` registers `confluence.longtask.get` with `GET /wiki/rest/api/longtask/{id}`. `finished: false` is running; `finished: true` needs an explicit `successful` boolean. Task IDs must be path-safe.
- `NativeJournal::record_job_state` (`crates/craft-runtime/src/journal/mod.rs`) appends one polled state to an existing handle in one immediate transaction. It refuses a different handle, a missing job, a disallowed transition (including any move out of a terminal state) and never touches submission state, request digest or confirmed resources. An unchanged state is not a new revision.
- `operation wait` (`crates/craft-cli/src/job_wait.rs` and `job_wait/command.rs`) selects the step from local evidence first. No record, a step without a job (a started or unknown submission whose receipt was lost) or an already terminal job returns local data with no profile, credential or network access. Otherwise the selected service profile must match the job's provider and tenant and hold the record's grant, and the verified actor must equal the record's actor. The host registers only the status read, so there is no route that could submit work.
- Polling backs off from one to ten seconds within `min(--wait-ms, remaining budget − 1.5 s)`. 429 or 503 waits only when provider advice fits that window and otherwise stops `rate_limited` with the advice. 404 stops `not_visible` without changing state. Cancellation returns a failure envelope that still carries the latest job state. A state that would move backwards is refused without a journal change. If an observed change cannot be persisted, the response keeps it with `journal_current: false` and polling stops.
- `help --json` now leaves `provider` out of its default fields (it is still selectable), which brought the default output from 16,375 to 14,383 bytes with this command added.

## Checks

| Check | Result |
| --- | --- |
| `cargo test -p craft-provider-jira --lib tasks` | 2 passed: documented status mapping without leaking result text, malformed or mismatched evidence |
| `cargo test -p craft-provider-confluence --lib longtasks` | 2 passed: finished/successful mapping, ambiguous evidence and unsafe IDs |
| `cargo test -p craft-runtime --lib polled_job_states` | passed: one-way transitions, unchanged revision for a repeated state, wrong handle, missing step and terminal regression refused, state survives reopening |
| `cargo test -p craft-cli --test jira_issue_reads_host --features synthetic-acceptance job_wait` | 6 passed over the real service access and TLS transport (below) |
| `cargo test -p craft-cli --test operations waiting` | passed: local-only results from a fresh process |

Host cases and what they pin down:

| Case | Observation |
| --- | --- |
| `accepted_job_is_polled_to_failure_through_status_reads_only` | Accepted → `RUNNING` → `CANCELLED`: two status GETs to `/rest/api/3/task/77`, one 1 s pause, journal ends failed while the submission stays confirmed, no result text in output, response schema valid, no plaintext credentials on disk, every request a GET |
| `pending_job_stays_pending_with_its_handle_when_the_wait_ends` | `--wait-ms 0`: one read, `wait_elapsed`, journal and output keep the running handle |
| `rate_limited_or_missing_tasks_keep_the_recorded_state` | 429 with `Retry-After: 120` stops `rate_limited` with the advice and code `RATE_LIMITED`; 404 stops `not_visible`; both keep the accepted state |
| `cancellation_between_polls_keeps_the_observed_state` | Cancelled during the pause: `CANCELLED` failure envelope carrying the running state, which is also in the journal |
| `another_actor_or_unscoped_token_refuses_before_any_status_read` | Wrong actor is `POLICY_DENIED` and a token without `read:jira-work` is `PERMISSION_DENIED`, both with zero status reads |
| `confluence_long_task_reaches_success_through_its_own_route` | `/wiki/rest/api/longtask/5e2c-41` succeeds; messages stay out of the output |
| `waiting_without_a_pollable_handle_returns_local_evidence_and_sends_nothing` | Missing record, send-started and unknown submissions without a handle, and succeeded or failed jobs return `not_pollable` or `terminal` with zero polls, null credential cache and the journal unchanged; several jobs need `--step`; `--fields` is refused |

No public command submits an asynchronous job yet, so no lost-submission case can reach a provider: the only path is the local `not_pollable` result above. Confluence archive, page-tree copy and space deletion must record their handles through `SendClaim::record_outcome` when their tasks activate them.

Gate on the final source: `python tools/verify.py` passed planning, skill, contract, provider and dependency checks, formatting, both Clippy modes and `cargo test --workspace` (814 tests). Its composed-host stage then failed 16 Confluence review-verification cases at default parallelism, the known deadline pressure recorded in STATE.md; `confluence_context_host` passed all 69 with `--test-threads=4`, and the other eight host suites, the runtime synthetic-acceptance library tests and `tools/synthetic-acceptance.py` all passed.

## Live

On 15 November 2026 the CraftTest service profile ran `operation wait` against the test site with handles seeded only into the local `cli/target/live-state` journal by the opt-in `seed_live_job_handle_for_a_read_only_wait_probe` test (no site write). Jira task `1`: one status GET under `read:jira-work` with the verified service actor, HTTP 404, `stop_reason: not_visible`, recorded state left `accepted`. This confirms the route, gateway, scope and actor path; it does not observe a running or finished task. Confluence long task: `PERMISSION_DENIED` with zero polls, because credential `craft-granular-4` has neither long-task scope.

## Not covered

- No real asynchronous job was created on the test site; live checks are limited to the route, scope and identity path with a seeded local handle.
- The CraftTest credential has neither Confluence long-task scope (`read:confluence-space.summary` or `read:content.metadata:confluence`), so Confluence polling is refused before any request until the owner adds one.
- macOS and Linux runs.
