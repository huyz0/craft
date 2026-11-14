# Issue metadata candidates and allowances

The public `jira metadata edit` and `jira transition list` adapters compose profile/site selection and explicit operation allowances with the [issue-bound metadata host](jira-issue-context.md). See [current usage](jira-context.md) and the earlier [adapter evidence](evidence/issue-metadata-command.md). Listing transitions does not enable transition execution.

## Arguments and context

Both candidates require `issue`, accept an optional `cloud_id`, and expose `limit` (default 20) and an opaque `cursor`. Transition-list additionally accepts `transition_id`, `include_unavailable` and `sort_by_status` (both booleans default false). Common selected fields and byte budget remain output controls. Flags and JSON preserve exact typed values; no argument can supply grant, actor or operation allowance. Administrative override flags and arbitrary expansions are unavailable.

Pure validation checks native selectors/filters, physical JSON bounds, projection, cursor shape and the complete metadata/cache/paging recovery budget before opening state or loading credentials. The reservation includes the longest resolution label and serialized escaping of bounded update evidence. Explicit profile selection bypasses repository hints; otherwise repository then active profile selection applies. Site precedence is explicit cloud, repository site, profile site default, then configured profile tenant. The selected site must match the profile and policy. Preparation binds the requested issue exactly under that site, including its original key/ID selector.

## Explicit allowances

The existing `issue_read_policies` record accepts five closed operations in canonical order:

1. `jira.issue.get`
2. `jira.issue.search`
3. `jira.metadata.edit`
4. `jira.transition.list`
5. `jira.permission.check` ([current-user contextual observations](jira-permission-reads.md))

Records contain only explicitly selected operations. Existing get/search records grant neither metadata operation; a metadata-only record grants neither ordinary issue read nor the other metadata operation. Project, field and create records do not supply these allowances.

The record retains exact installation/profile/grant/site/actor binding, enabled state and revision. The setter requires explicit apply, receipt preflight and expected-revision CAS. An enabled record must first be disabled without transferring actor/target; a later edit can transfer only where the profile still permits that target. Duplicate, unsorted, unknown or malformed persisted allowances fail closed.

No table migration is needed. SQLite schema remains seven, settings format one and policy format one. Existing get/search operation order and serialized records remain compatible. Older binaries that do not recognize the additional operation strings cannot use such a record; this does not establish downgrade qualification.

## Execution and recovery

Preparation captures the complete enabled record. Execution rechecks it before authentication, before each of the three native resource reads, for buffered access, and after final context/output/cursor work. Access freshness is checked before policy in the per-exchange wrapper; all requests still pass through the original sealed transport context. No separate route or credential path is introduced.

Missing/disabled/operation-excluding records return `POLICY_DENIED`. Another captured-record change returns `VERSION_CONFLICT`. Failures suppress metadata, issue observation and continuation while retaining independent credential-cache receipts. Checks do not establish an atomic lock against external writers or a mutation approval. Existing immutable cursors preserve their predecessor if an unreleased successor was persisted before a final refusal.

Dispatch preserves complete errors, warnings, observations, recovery pages and original request identity. Metadata remains inert and non-authorizing. Contextual permission reads and public read activation are now implemented; live qualification and public edit/transition execution remain unfinished.
