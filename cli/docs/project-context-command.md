# Project-context candidates and explicit permissions

Typed project get/status adapters connect project selection and stored operation permissions to the [qualified project-context host](jira-project-context.md). They are now public with [policy setup and usage](jira-context.md). This document describes their underlying preparation and permission model; live access remains separately unqualified.

Implementation: [arguments/context](../crates/craft-cli/src/project_context/command.rs), [dispatch](../crates/craft-cli/src/dispatch.rs), [shared record](../crates/craft-runtime/src/project_read_policy.rs), [project permissions](../crates/craft-runtime/src/project_read_policy/context.rs). See [acceptance evidence](evidence/project-context-command.md).

## Arguments and project selection

Get accepts optional `cloud_id` and `project`. Statuses also accepts `limit` (default 20) and an opaque `cursor`. `project` is a numeric ID or case-sensitive key. Fields and byte budgets remain common output controls. Flags and JSON share types/defaults; unknown authority fields, expansions, wrong types, malformed selectors/cursors and insufficient complete recovery output fail before state is opened. When project context is unresolved, pure preflight reserves the maximum selector length.

An explicit profile bypasses repository hints. Otherwise use one repository-hint snapshot and active-profile fallback. Project precedence is explicit selector, repository project and profile project default. Missing project context produces an error and asks for an explicit/configured target. An explicit Cloud UUID must equal the selected profile tenant. It cannot move a hinted/default project to another site. Only exact Jira Cloud service profiles qualify.

Arguments do not supply grant, actor, enabled state, operation allowance or policy revision. The selected target must have an explicit matching private permission.

## Listing intent and context permissions

The existing `project_read_policies` table supports payload formats one and two, introduced without a database migration. The current SQLite schema seven adds separate field-read permissions; settings format stays one, and project-table/row bounds are unchanged. Format one remains list-only and is not automatically converted into context authority.

Format two adds a required `context_reads` array, bounded to 32 exact project targets. Each entry has its own enabled flag and a nonempty explicit list of `jira.project.get` and/or `jira.project.statuses`. Targets and operations are sorted canonically without duplicates. Numeric IDs and keys are separate selectors; resolving one does not create permission for an alias. Shared installation/profile/grant/site/actor bindings remain exact.

The legacy record's `enabled` flag controls listing only. A disabled listing can coexist with an enabled get/status permission, and an enabled listing does not authorize either operation.

The trusted context setter requires an existing valid site/actor record, explicit apply, exact whole-record revision CAS and caller output preflight. It changes one target, preserves listing intent and other targets, and upgrades format one only for that explicit edit. Empty/duplicate/unknown operations, malformed/foreign targets, corrupt rows and capacity overflow fail without repair or eviction. Disabling a target retains its explicit operation list and sets its flag false.

The listing setter preserves format-two permissions. It refuses a shared-actor change while any context permission is enabled. Disable those entries first; an actor change does not silently transfer active rights. Re-enabling for the new actor requires another explicit permission update.

This record is local configuration. It does not establish provider permission, independent human approval, immutable audit history or cross-machine fencing. Its setters are not public administration commands.

## Captured configuration and results

Preparation captures the whole record and verifies the exact project/operation. Execution rechecks it before authentication, throughout native/cached access, and after final timestamp/output/cursor work. Missing, disabled or operation-excluding permissions return `POLICY_DENIED`; other captured-record changes return `VERSION_CONFLICT` and require new preparation. Unrelated permission edits also change the shared revision. A newly prepared invocation may resume an otherwise matching immutable cursor when current authority permits it.

Dispatch preserves the original request identity and complete project/status/error/page/cache envelope. Status remainders and oversized-group recovery retain the host's existing cursor semantics. Confirmed credential renewal remains visible when later policy checks fail. No resource retry or implicit provisioning is added.

## Remaining qualification

Owned-server tests exercise parsed candidates, actual stored permissions, cache renewal, native reads and late permission changes. They use fixed synthetic credentials. Public operational configuration, independent consequential review, actual public/live acceptance, remaining Jira field/workflow metadata and CRUD/recovery, and Confluence integration remain unfinished. The full CLI delivery scope is still active.
