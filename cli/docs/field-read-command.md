# Field discovery candidates and explicit permissions

Typed list/search adapters connect argument decoding and private field-read permissions to the [qualified host](jira-field-reads.md). They are now public with [policy setup and usage](jira-context.md). The following describes their preparation and argument shapes; live access remains separately unqualified.

Implementation: [arguments/context](../crates/craft-cli/src/field_reads/command.rs), [dispatch](../crates/craft-cli/src/dispatch.rs), [private policy](../crates/craft-runtime/src/field_read_policy.rs). See [acceptance evidence](evidence/field-read-command.md).

## Arguments and context

Both operations accept optional `cloud_id`, `limit` (default 20) and an opaque cursor. Search also accepts typed native filters and `page_size` (default 50). Fields and byte budgets remain common output controls. Filters, projection, cursor syntax and complete recovery-envelope budgets are checked before opening state or acquiring credentials.

| Search argument | Candidate flag | Behavior |
| --- | --- | --- |
| `ids` | Repeat `--id` | Canonical custom-field IDs; no CSV splitting |
| `types` | Repeat `--type` | Exact `custom`/`system` values |
| `project_ids` | Repeat `--project-id` | Positive int64 integers; floating-point JSON values are refused |
| `query` | `--query` | Literal text; commas, Unicode, plus and ampersands remain data |
| `order_by` | `--order-by` | Native closed ordering, including explicit ascending/descending variants |
| `page_size` | `--page-size` | Native page size from 1 to 100 |

Flags and structured input share defaults/types. The development example converter now handles numeric array members as individual scalar flags; structured objects remain JSON-only. Neither arguments nor repository hints can supply actor, grant, continuation origin or permission. Unknown authority fields and raw expansions/offsets are refused.

Existing profile precedence applies: explicit profile bypasses repository hints; otherwise take one current-directory hint snapshot, then the active profile. Exact site selection uses explicit Cloud UUID, repository site, profile site default, then the selected profile tenant. A site cannot cross the selected profile's tenant binding. The selected profile must use Jira Cloud service access.

## Private permission record

SQLite schema seven adds `field_read_policies` without granting field discovery to existing issue/project configurations. A format-one record binds installation, profile, issued grant, exact site, expected actor, canonical field-site origin, closed list/search operations, enabled state and revision. Either operation can be allowed independently.

The trusted setter requires explicit apply, expected-revision CAS and complete receipt preflight before commit. An absent revision only creates. Concurrent writers keep one winner; changed requests must inspect the current record before retrying. An enabled record must be disabled before changing actor, target or continuation origin; profile/tenant restrictions still apply after disablement. Empty/duplicate/unknown operation sets, noncanonical persisted origins, malformed or foreign selected records and duplicate JSON members fail without repair.

Atomic migration preserves settings format one, profiles/grants/preferences, prior permissions, mutation journal and both cursor tables. Exact field-table shape and absence of triggers are checked on open. Conflicting DDL rolls back without repairing or downgrading the store. The Windows native service-cache process path passed after the schema change; other-platform/native-principal qualification remains separate.

## Execution and output

Preparation captures the entire allowed record. Execution checks it before auth, through native/cached work, and after final observation/output/cursor work. Revoked/removed operations return policy denial; another enabled revision returns version conflict. No field/page is released on these failures, while confirmed credential-renewal receipts remain visible.

Dispatch keeps the decoded operation-specific arguments, original request ID, complete page/recovery/cache data, warnings and output policy. Public configuration commands, auth activation, independent consequential review and live qualification remain unfinished. These private records do not establish human approval, immutable audit history, same-principal isolation or distributed fencing.
