# Agent interface

## ADDED Requirements

### Requirement: Deterministic invocation
The CLI SHALL emit one versioned JSON envelope and stable exit category, remain noninteractive during normal agent operations, and expose only implemented commands.

#### Scenario: Invalid command
- **WHEN** an unknown command or flag is supplied
- **THEN** the CLI returns a bounded structured error without a browser, prompt, remote write or full help dump.

#### Scenario: Complete operation result
- **WHEN** a handler returns a success or failure envelope containing recovery data, warnings or paging
- **THEN** dispatch and output preserve that envelope and the invocation request ID, deriving the exit category from the retained result rather than wrapping its data as a new success.

### Requirement: Strict request validation
The CLI SHALL reject duplicate/unknown JSON keys, wrong types, conflicting flag/file inputs and invalid target identities without silent correction. Local malformed requests SHALL make no network or auth-maintenance call.

#### Scenario: Wrong field
- **WHEN** an update contains an unknown field or a duplicated key
- **THEN** the call is rejected locally with a field path and schema hint; valid multiline and international content remains supported.

#### Scenario: Structured input already used stdin
- **WHEN** a command's argument object came from stdin and also selects stdin for secret credentials
- **THEN** the invocation retains that input-source fact and refuses a second read before credential access or acquisition.

#### Scenario: Previewing a mutation
- **WHEN** an agent runs `operation preview` for a mutating command and its exact input
- **THEN** the input is validated with the command's own rules, and the result reports the canonical input digest, descriptor digest, effect, destructiveness, retry and reconciliation classes, operation identity and the supplied precondition fields, with no private state opened, no journal record and no network request.
- **AND** with `--check-remote` and an explicit profile, only the declared read that observes the write's existing target runs, under that read's own policy; creates and local-only writes refuse remote preview, and every result states that no write was sent, no lock is held and no approval was granted. Apply rechecks its preconditions against a fresh read and never consumes a preview.

### Requirement: Discoverable contracts
A single operation model SHALL describe request/response schemas, selectable fields, effects, permissions and availability. Compact views SHALL identify omitted detail and full views SHALL be complete.

#### Scenario: Selected response schema
- **WHEN** an agent requests a response schema with selected fields
- **THEN** mandatory envelope fields remain defined and unselected item properties are not falsely required.

#### Scenario: Parser and contract drift
- **WHEN** accepted command paths, flags, typed inputs, response schemas or selectable fields change
- **THEN** deterministic development export reflects the actual parser and operation registry, checks flag/input correspondence and example equivalence without executing resource operations, and the gate rejects differences from the reviewed snapshot until explicit regeneration.

#### Scenario: Incomplete advertised operation
- **WHEN** an exported operation lacks explicit scope/recovery metadata, advertises an unmapped parser argument or gives a mutation read-only retry semantics
- **THEN** the development check fails with bounded semantic paths rather than accepting a regenerated incomplete contract.

### Requirement: Bounded field-selected output
Reads SHALL default to bounded metadata; selected fields SHALL be validated and projected upstream where supported or locally otherwise. Missing, null, redacted and unselected data SHALL not be conflated.

#### Scenario: Small read
- **WHEN** an agent requests title and version only
- **THEN** the result preserves required identity/paging evidence, does not echo full bodies, and declares any actual truncation.

#### Scenario: Heterogeneous rich value
- **WHEN** a selectable field has multiple non-null schema variants
- **THEN** the whole field retains its complete schema and data, while variant-dependent child paths are not advertised and fail as unknown fields.

#### Scenario: Write output preflight contract is violated
- **WHEN** a state-changing handler unexpectedly returns evidence exceeding its preflighted output budget
- **THEN** the encoder retains the actual outcome and recovery data with an explicit budget warning instead of replacing it with a generic error that hides the effect; normal writes still require preflight before mutation.

### Requirement: Lossless pagination
List/search traversal SHALL use opaque bound cursors, preserve fetched remainders, distinguish live traversal from a snapshot and stop on expiry or lack of progress.

#### Scenario: Budget ends mid-page
- **WHEN** a fetched page contains more items than fit
- **THEN** the next cursor resumes at the first unreturned item rather than skipping to the provider's next page; an empty nonterminal page still has continuation.

#### Scenario: Immutable replay and access
- **WHEN** a provider cursor is replayed concurrently or after token refresh under the same grant and actor
- **THEN** its logical position is unchanged, adjustable output budgets remain permitted, and current access is checked before cached records are returned; query, target, grant, actor, descriptor or projection changes fail without a resource fetch.

#### Scenario: Oversized first record
- **WHEN** a complete first record exceeds the selected output budget
- **THEN** a bounded `ITEM_TOO_LARGE` result retains a recovery cursor at that record; if the recovery envelope itself cannot fit, preflight refuses the fetch. No later record is substituted.

#### Scenario: Provider cursor integrity and publication
- **WHEN** continuation is expired, missing, foreign, mismatched, corrupt, cyclic, beyond capacity or cannot be persisted
- **THEN** a specific failure is returned, no successful successor page is published and no live cursor is evicted or silently restarted; a failed successor does not consume its predecessor.

#### Scenario: One native fetch and observation provenance
- **WHEN** buffered records remain or a native page is short or empty but nonterminal
- **THEN** buffered replay performs no resource fetch, each other invocation performs at most one native page fetch, continuation reflects provider metadata rather than item count, and output records identify their original observation and live consistency.

### Requirement: Portable artifact I/O
Nested inputs and large outputs SHALL use bounded file/stdin/artifact paths with digest/provenance and safe OS-aware file handling. Binary output SHALL not corrupt JSON stdout.

#### Scenario: Download boundary
- **WHEN** an attachment redirect or output path targets an untrusted host or escapes an allowed root
- **THEN** the CLI refuses it without forwarding credentials or replacing an unrelated file.

#### Scenario: Bounded authenticated binary stream
- **WHEN** a sealed read operation downloads bytes through a compiled GET route
- **THEN** the host constructs the destination before attaching credentials, disables redirects/retries/compression/proxies, accepts only a complete HTTP 200 entity, rejects partial/encoded/malformed-length responses and unsupported or repeated transfer codings (only one chunked framing header is accepted) and counts streamed bytes against an explicit bound. Remote filenames and URLs grant no path or routing authority.
- **AND** cancellation, deadline, policy/scope revocation, oversize or expected-digest mismatch before publication drops staging without publishing a successful artifact. Completed local publication retains its receipt despite a later failure. Unknown publication acknowledgment is reported separately and never implies no local file exists.
- **AND** raw bytes never enter the JSON envelope. Receipts carry actual size, SHA-256, media type, resource provenance and local artifact identity. Optional export uses an independently selected output root and relative destination, with create-new semantics unless replacement is explicitly selected. No automatic network retry follows storage failure.

#### Scenario: Atomic artifact publication and collisions
- **WHEN** writers compete for the same logical artifact name or a write fails before finalization
- **THEN** only one complete data/receipt pair is published, an existing destination is preserved, and incomplete bytes never become a successful receipt.

#### Scenario: Artifact restart and abandoned staging
- **WHEN** a writer process dies or publication acknowledgment is lost
- **THEN** a fresh session can verify a complete receipt by exact size/digest, and bounded cleanup removes only abandoned generated staging after obtaining its lease while preserving live writers and final artifacts.

#### Scenario: Persisted artifact metadata attempts path authority
- **WHEN** a logical name contains traversal or a stored receipt contains a local path, wrong binding, corrupt digest or linked file
- **THEN** the adapter refuses it without creating a missing input, following the link, trusting a supplied path or returning a complete artifact.

#### Scenario: Explicit workspace input snapshot
- **WHEN** the host selects an existing workspace input root and a relative regular file for an upload snapshot
- **THEN** reading does not create or modify the source, path traversal/device/link aliases are refused, and bounded streaming publishes an immutable artifact only after source identity/change checks and any expected digest match; failure publishes no complete artifact.

#### Scenario: Guarded local CLI input
- **WHEN** structured command input or a local plan/design file is selected within the current project root
- **THEN** relative and contained absolute paths use existing-only checked input handles, linked/escaped/nonregular or changed sources are refused before decoding, and bounded reading still observes invocation cancellation/deadline without creating a missing file.

#### Scenario: Explicit artifact export
- **WHEN** the host exports a verified artifact to a selected output root and relative destination
- **THEN** complete bytes are staged within the checked destination directory, creation refuses an existing entry by default, explicit replacement affects only the selected ordinary file outside protected artifact storage, and digest/size checks precede atomic publication; invocation control is checked after synchronization, attempted publication errors retain uncertainty and confirmed publication survives later cancellation.

#### Scenario: Local artifact inspection and bound export recovery
- **WHEN** an operator inspects an exact artifact name in the closed downloads, requests or reviews collection
- **THEN** the CLI returns a streamed size/digest-verified receipt without binary bytes, credentials, profile selection or cloud access. Missing and unreadable/corrupt artifacts remain distinct, and an explicit byte ceiling is enforced without classifying a valid oversized object as corrupt.
- **AND** local export requires the observed lowercase SHA-256, rechecks it against the same opened artifact before staging, validates complete response capacity before publication and guards the selected output outside the entire private state root. Existing outputs require explicit replacement; uncertain publication is never automatically retried and confirmed receipts survive late cancellation.
- **AND** strict input, collection/name/digest/path constraints and unsupported projection/profile selection fail before opening private state. Local receipts never authorize a cloud replay.

#### Scenario: Claimed bounded multipart upload
- **WHEN** a verified host sends a single-file Jira attachment upload from an immutable named snapshot
- **THEN** snapshot size and expected digest are verified before claim acquisition, the same opened file is streamed again in bounded chunks with exact multipart length, fixed file field and X-Atlassian-Token header, and compiled routing attaches credentials only to the declared Jira content destination.
- **AND** the send claim binds framing, filename, media type and exact snapshot identity. Equivalent preparations preserve framing identity; no successful response from an incomplete or corrupt source becomes qualified upload evidence. Bounded native response evidence is retained independently when source completion is unqualified.
- **AND** cancellation, deadlines, disconnects, redirect responses and response-persistence failures preserve the original claimed uncertainty without retries. Ordinary JSON mutation preparation cannot execute a multipart-only operation.
