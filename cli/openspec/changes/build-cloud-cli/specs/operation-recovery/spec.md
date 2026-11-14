# Operation recovery

## ADDED Requirements

### Requirement: Durable mutation identity
Every product-resource write SHALL require explicit apply and operation identity bound to payload/target/actor, with intent persisted before sending and remote IDs recorded promptly.

#### Scenario: Changed replay
- **WHEN** a caller reuses an operation identifier with a different payload
- **THEN** the CLI refuses the request before mutation; the operation identifier is runtime data, not a planning label.

#### Scenario: Concurrent local send claims
- **WHEN** two processes claim the same prepared step under the same state root
- **THEN** exactly one committed send marker can produce a claim, changed actor/grant/request/step bindings are refused, and restarting a sent or unknown step cannot authorize another send.

#### Scenario: Journal-bound HTTP mutation
- **WHEN** the trusted host prepares a declared non-read JSON request for an unchanged operation and matching tenant/grant session
- **THEN** all route, URL, body, TLS and runtime preflight checks precede journal acquisition, and one consumed prepared request can attempt HTTP only after its exact native request obtains a committed send claim under explicit apply.
- **AND** claim refusal sends nothing, errors after a claim preserve uncertainty without replay, and a bounded HTTP response is not itself semantic confirmation. The host retains the response/error and local evidence-persistence disposition separately, and provider-qualified IDs/job outcomes can be recorded through the existing claim even after cancellation.

#### Scenario: Interrupted journal migration or commit
- **WHEN** schema migration, intent, send-marker or receipt persistence is interrupted or its acknowledgment is lost
- **THEN** reopening preserves either the prior or complete committed state, existing profile data remains intact, and an uncertain marker acknowledgment never produces permission to send.

#### Scenario: Confirmed effects after cancellation
- **WHEN** a provider result is confirmed before cancellation but journal recording has not completed
- **THEN** its existing claim can attempt immediate local evidence persistence without another send or contention wait, and any failure preserves the caller's known receipt for recovery.

#### Scenario: Local operation inspection
- **WHEN** a caller runs `operation get` with an exact operation identity under a selected local state root
- **THEN** the CLI returns the validated stored binding, revision and named step evidence, or explicit local absence, without reading credentials, contacting a provider, changing submission state or authorizing replay. Missing local evidence never establishes absence of a remote effect.
- **AND** unknown or started submissions and unknown job outcomes require reconciliation; projection retains operation identity, local presence, reconciliation uncertainty, lack of live verification and lack of replay authority. Invalid inputs and selections fail before opening state, corrupt/unavailable state is an error, and oversized output is refused without partial evidence masquerading as a complete record.

### Requirement: Uncertain and partial outcomes
Unknown create outcomes SHALL require reconciliation; partial workflows SHALL retain confirmed effects and resume without blind duplication or automatic destructive compensation.

#### Scenario: Lost create response
- **WHEN** a create reaches the provider but its response is lost
- **THEN** the CLI returns an unknown outcome and reconciliation finds or explicitly fails to establish the remote object before retry.

#### Scenario: Journal-bound Jira evidence inspection
- **WHEN** internal reconciliation receives the original supported write request, operation ID and explicit candidate IDs for a create
- **THEN** it verifies the exact original descriptor/payload/target/grant/actor binding against local evidence before auth, never creates a missing record, and reads only the original existing issue or bounded known/explicit create candidates through declared read-only routes under separate recovery authority.
- **AND** visible candidates, matching current values and absent-or-invisible responses remain observations, not effect attribution or replay permission; missing identities leave unknown creates explicitly unresolved without credential access.
- **AND** complete journal/cache evidence survives partial failures and final policy/journal changes; concurrent records are retained separately without overwrite, unauthorized observations are suppressed, and the journal is never reset or mutated by inspection.

#### Scenario: Explicit persisted recovery authority
- **WHEN** trusted administration configures recovery for a Jira Cloud service profile and exact numeric project
- **THEN** a separate record binds installation, profile, grant, project, expected actor and an explicit closed set of original write commands. Create, mutation and read policies confer no recovery authority, and recovery confers no write or replay authority.
- **AND** apply, complete receipt preflight and row-revision comparison precede a change; enabled actor/target transfers require disabling the old identity first. Captured policy must remain identical through execution, malformed/foreign records fail without repair, and competing updates cannot overwrite a winning revision.
- **AND** schema-nine migration creates an empty recovery table atomically, preserving existing policy, journal, cursor, profile and settings data. Migration conflicts roll back all changes without granting authority.

#### Scenario: Typed recovery invocation
- **WHEN** an internal recovery candidate receives flags or structured input
- **THEN** the original canonical request is required, remains unchanged, and is validated with operation identity, create-only candidate IDs, output selection and budget before private state access. Normal profile/site selection must agree with its exact original target; it cannot rewrite that target or supply actor/grant authority.
- **AND** the candidate captures and rechecks the separate recovery record, preserves complete evidence through dispatch, and opens credentials only after the host establishes journal-bound read targets. Public activation requires separate qualification.

#### Scenario: Public journal-bound investigation
- **WHEN** `operation reconcile` enters public discovery and invocation
- **THEN** discovery identifies its Jira service dependency without claiming live verification, the exact typed original request reaches the existing GET-only host under independent recovery policy, and generic registry execution remains refused.
- **AND** missing or malformed original intent fails before state, local unresolved paths require no credential access, and observations never change journal attribution or authorize replay.

#### Scenario: Saved canonical source for standalone issue writes
- **WHEN** a standalone issue create/update/assign/unassign/delete or transition apply can start under its exact policy and journal binding
- **THEN** the CLI confirms an immutable private artifact containing the canonical native request and binding before native acquisition/sending. Conflicting, corrupt or uncertain source persistence stops the new attempt without overwriting an earlier source or reopening a claim.
- **AND** compact public receipts distinguish source persistence from native effects. Native descriptors and binding digests remain unchanged; public response metadata must not invalidate historical journals.
- **WHEN** `operation reconcile --saved` selects the recorded operation ID
- **THEN** the saved original is strictly validated and passed through the same current recovery-policy and exact journal-binding checks as an explicit original, without granting write authority. Explicit original and saved selection are mutually exclusive. Old records without saved sources remain explicit gaps.

### Requirement: Bounded transport and async jobs
HTTP and job polling SHALL obey total deadlines, cancellation, provider rate advice and operation-specific retries, with accepted/running/completed/failed/unknown states kept distinct.

#### Scenario: Untrusted routing and continuation
- **WHEN** a provider call supplies an undeclared route/query, unsafe identity, cross-host redirect or a next link for another product/cloud ID/path
- **THEN** credentials are attached only after host-owned routing validation, redirects are not followed, and next links cannot replace the validated origin or operation route.

#### Scenario: Declared repeated GET query filters
- **WHEN** a read adapter declares repeated native GET query parameters
- **THEN** bounded vectors encode as separate values without CSV/raw-query interpolation, scalar/repeated name collisions and undeclared/oversized arrays fail before connection, and existing scalar operation descriptors/claims stay unchanged.
- **AND** continuation preserves exact bound vector values/order, rejects scalar or continuation duplicates and changed/added/partial arrays, and cannot broaden GET-array support into mutation routing or a different credential destination.

#### Scenario: Bounded JSON and cancelled I/O
- **WHEN** a response exceeds the declared byte bound, contains invalid JSON, stalls or the invocation is cancelled
- **THEN** the transport returns a sanitized bounded error, performs no automatic replay, preserves any possible mutation uncertainty and stops awaiting network I/O within the invocation budget.

#### Scenario: Enterprise TLS policy
- **WHEN** a trusted launcher supplies additional CA roots or the peer has an invalid certificate/hostname
- **THEN** bounded additional roots may supplement platform trust, certificate/hostname validation remains mandatory, and repository content cannot choose a proxy, raw credential destination or verification bypass.

#### Scenario: Polling deadline
- **WHEN** an accepted job remains pending when the caller's budget ends
- **THEN** the result remains pending with a durable handle rather than claiming completion or launching another job.

### Requirement: Shared mutation authority
Cross-machine claims/deduplication SHALL use an explicitly configured trusted mutation/lease authority. Standalone mode SHALL declare advisory or single-writer consistency.

#### Scenario: Two workers race
- **WHEN** two machines attempt the same protected plan application
- **THEN** only an authorized lease holder can mutate under the configured authority; local locks or Jira properties alone do not claim exclusivity.
