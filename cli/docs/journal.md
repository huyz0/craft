# Local mutation journal

`craft-runtime::journal::NativeJournal` stores non-secret mutation metadata in the validated private SQLite state. The public [`operation get`](operations.md) command reads these local records. The [internal Jira reconciliation host](jira-reconciliation.md) can inspect bound known/explicit issue IDs without changing the journal. Public provider refresh, operation listing/wait/reconciliation and production cloud mutation execution remain unfinished.

## Stored identity and migration

Opening profile state upgrades schema one through two (adding `operation_records`) to three (adding separate `create_policies`) in one transaction. Existing settings, profiles, grants, preferences, cursors and journal rows remain intact. Settings retain format version one and operation records retain their existing format. Conflicting tables, unexpected journal/policy schema or triggers, foreign applications and future versions are refused rather than repaired. The private database and persistent rollback journal remain pinned; full synchronization, the 64 MiB cap and bounded waits still apply. See [create policy and migration](create-command-policy.md).

Each operation records its installation, operation ID, logical request digest, descriptor digest, target, issued grant, effective actor, revision and named steps. `PreparedOperation::mutation_binding` excludes output-only selection/budgets from the logical digest; the full sealed execution digest remains unchanged. Reusing an ID with changed request, target, actor, grant or descriptor conflicts. Actor metadata is supplied by the trusted host after authentication; the journal does not verify remote identity or permission.

Each step binds one canonical native-request digest, submission state, confirmed resource IDs and optional async handle. Once bound, its digest cannot change or disappear. Transitions cannot erase IDs, replace a job identity, regress terminal submission/job state or reopen a send. Records are limited to 1 MiB of strict JSON and 1,000 steps; the store holds at most 4,096 operations. Serialization stops at the byte ceiling instead of allocating an unbounded JSON string. Raw input bodies, credential material and arbitrary provider responses are outside this API.

## Claim before send

`claim_send` checks explicit apply, a valid operation ID, unchanged prepared descriptor, declared route and exact native call before recording intent. It binds a prepared step, then commits `send_started` before returning a non-cloneable `SendClaim`. A read operation/GET route cannot claim mutation authority. Stable step names belong to host/provider code; changing the step name is not an authorized retry strategy.

Same-root writers use immediate SQLite transactions and revision comparison. Only one claimant advances a prepared step. Previously started/unknown or terminal steps require inspection/reconciliation; no new claim is returned. If a commit acknowledgment is uncertain, no claim is returned even when the marker later proves durable. The caller must not send without a confirmed claim or infer that an uncertain marker means no remote effect.

The candidate [journal-bound HTTP host API](journaled-http.md) now enforces this ordering for one private prepared JSON request and keeps the HTTP result separate from local evidence persistence. It reuses the HTTP invocation's control for journal waits. It is not connected to a public provider/CLI mutation path and still requires consequential review and qualified provider/host recovery composition.

SQLite `BUSY`/`LOCKED` during an active commit can retry only `COMMIT` within the invocation budget. The transaction body is never replayed. Other commit errors retain uncertainty. A deadline during contention drops the uncommitted transaction. These are local database rules, not a retry policy for provider requests.

## Record observed effects

An existing claim's `record_outcome` appends confirmed IDs/job evidence under the original binding and native-request digest. It can attempt one immediate local transaction after cancellation/deadline, without waiting for a mutex/database reader or authorizing another send. If persistence fails, the caller still owns the supplied outcome and must retain it in its result/recovery path. Unknown receipt acknowledgment is reconciled by reading the operation, not by resending its request.

Claims survive process death as send markers; unfinished transactions roll back on reopen. Local SQLite does not establish remote idempotency, exactly-once execution or cross-machine fencing. No automatic record deletion, destructive compensation or marker reset is provided. Production HTTP mutation composition, provider-specific reconciliation, real disk/power-loss cases, other platforms and consequential independent review remain required. See [journal evidence](evidence/mutation-journal.md), [local inspection evidence](evidence/operation-inspection.md) and the [design](../openspec/changes/build-cloud-cli/design.md#local-mutation-journal).
