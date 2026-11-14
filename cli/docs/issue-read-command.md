# Issue-read candidates and configuration

Typed issue get/search candidates now connect profile/site selection and separate private configuration to the [issue-read host](jira-issue-reads.md). They remain absent from the public parser, command catalog and provider discovery. This document describes an internal integration API, not commands available in the distributed CLI.

Implementation: [arguments/context](../crates/craft-cli/src/issue_reads/command.rs), [dispatch](../crates/craft-cli/src/dispatch.rs), [private configuration](../crates/craft-runtime/src/issue_read_policy.rs). See [observed evidence](evidence/issue-read-command.md).

## Arguments and selection

Get accepts an issue identifier and optional `cloud_id`. Search accepts literal `jql`, optional `cloud_id`, `page_size` (default 50), `limit` (default 20), an opaque `cursor` and `reconcile_issue_ids` (default empty array). Flags and JSON share scalar/array types and defaults. Reconciliation IDs can be supplied with the candidate `--reconcile-issue-ids 1,2` flag or as a JSON string array. They remain strings until the provider emits exact native int64 numbers. Wrong types, duplicate/noncanonical IDs and unsupported get cursors fail locally.

Fields and byte budget remain common output controls. Logical inputs, physical JSON bounds, projection and a conservative complete error/cache/paging budget are checked before state is opened. Required values can come from flags or structured input. Empty parser placeholders are never accepted as valid native input. No argument supplies grant, actor, enabled state, operation allowance or policy revision.

Context follows the existing profile resolver. An explicit profile bypasses repository hints entirely. Otherwise resolve one repository hint snapshot and then the active profile. Site precedence is explicit cloud, repository site, profile site default and finally the configured profile tenant. Every selected target must remain in the profile's exact tenant. Only Jira Cloud service profiles qualify.

## Separate private configuration

SQLite schema six adds `issue_read_policies` while settings format remains one. Migration creates an empty table and preserves existing settings, profiles, grants, preferences, journal, create/project policies and both cursor kinds. It does not convert project discovery into issue authority.

The current schema seven adds [separate field permissions](field-read-command.md) and preserves these issue-read records.

Each strict record binds installation, profile, issued grant, exact site, expected actor, enabled state, revision and explicit allowed operations. Get/search allowances remain independent. The record also supports opt-in [metadata candidates](issue-metadata-command.md) and [contextual permission observations](jira-permission-reads.md). Existing get/search records grant none of these additions. Operations are sorted canonically without duplicates; a disabled record grants none.

The trusted setter requires explicit apply, exact expected-revision CAS and caller output preflight before commit. No expected revision means create-only. Enabled records must first be disabled before transferring actor or target; disabled records still obey profile target binding. Unknown fields/operations, duplicate JSON properties, foreign identities, malformed rows, conflicting tables or unexpected triggers fail without repair. A possibly committed local write must be inspected under the original state root before retry. These APIs have no public policy-administration command yet.

This configuration expresses local intent. It does not establish provider permission, independent human approval, immutable audit history or distributed fencing. Jira still controls current native visibility; buffered content retains the original observation and the host's documented item-visibility limits.

## Execution and policy changes

Preparation captures the exact enabled record and operation. Execution rechecks it before auth, before native/cached access and before releasing a result. Disabled, missing or operation-excluding configuration returns `POLICY_DENIED`; another captured-record change returns `VERSION_CONFLICT` and requires new preparation.

The final policy check occurs after get timestamp/complete-envelope work and after search cursor work. This closes the observed late-change gap. If search has already persisted an unreleased successor before a late refusal, the predecessor remains unchanged and the unused row expires under normal cursor retention. Neither a local configuration check nor a returned cursor supplies cross-machine fencing against concurrent writers outside this boundary.

Dispatch preserves the original request ID and full envelope, including get resolution, page/oversized-item recovery, errors, warnings and credential-cache receipts. Renewal is independently reported even when later policy or resource checks fail. Native issue requests are not retried automatically.

## Qualification boundary

Owned TLS tests exercise parsed candidate arguments, actual profile/configuration selection, native access/cache libraries and policy changes during auth, after resource reads and before buffered output. They use fixed synthetic credentials. Public command activation still requires its own contract/dispatch qualification, consequential auth review and actual tenant acceptance. Remaining Jira context/CRUD, Confluence review-page workflows and the overall CLI delivery scope are not complete.
