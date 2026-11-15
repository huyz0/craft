# Jira operation conformance

Recorded **14 November 2026**, Australia/Sydney; published-schema and outcome qualification added **16 November 2026** (below). This is the operation-by-operation matrix for every public Jira command in `schemas/local-contracts.json` (33 commands plus the shared `operation.reconcile`). Fixture evidence is deterministic and runs in the ordinary gate. Live evidence is separate and opt-in; see [live conformance](live-read-conformance.md). A live pass is one run on one small site, not production support.

Columns: **Fixture** names the composed-host suite under `crates/craft-cli/tests/` (all run with `--features synthetic-acceptance`) that exercises the command through the real host over owned TLS. **Provider live** means the provider's prepare/send/interpret ran against the site with an account API token. **Host live** means the built `craft` executable ran the command with the service account, local policy and journal.

| Command | Effect | Fixture | Provider live | Host live |
| --- | --- | --- | --- | --- |
| `jira.project.list` | read | jira_project_list_host | pass | pass |
| `jira.project.get` | read | jira_project_context_host | pass | pass |
| `jira.project.statuses` | read | jira_project_context_host | pass | pass |
| `jira.field.list` | read | jira_field_reads_host | pass | pass |
| `jira.field.search` | read | jira_field_reads_host | pass after fix | pass |
| `jira.issue.get` | read | jira_issue_reads_host | pass | pass |
| `jira.issue.search` | read | jira_issue_reads_host | pass | pass |
| `jira.metadata.edit` | read | jira_issue_context_host | pass | pass |
| `jira.transition.list` | read | jira_issue_context_host | pass | pass |
| `jira.permission.check` | read | jira_permissions_host | pass | pass |
| `jira.comment.get` | read | jira_issue_reads_host/comments | pass | pass |
| `jira.comment.list` | read | jira_issue_reads_host/comments | pass | pass |
| `jira.link.get` | read | jira_create_host/link_reads | pass | pass |
| `jira.link.list` | read | jira_create_host/link_reads | pass | pass |
| `jira.link.types` | read | jira_create_host/link_reads | pass | pass |
| `jira.attachment.get` | read | jira_issue_reads_host/attachments | pass | pass |
| `jira.attachment.list` | read | jira_issue_reads_host/attachments | pass | pass |
| `jira.attachment.settings` | read | jira_issue_reads_host/attachments | pass | pass |
| `jira.attachment.download` | read | jira_issue_reads_host/downloads | not run | pass |
| `jira.issue.create` | write | jira_create_host | not run | pass after fix |
| `jira.issue.update` | write | jira_issue_mutation_host | confirmed | pass |
| `jira.transition.apply` | write | jira_issue_mutation_host | not run | pass |
| `jira.issue.assign` | write | jira_issue_mutation_host/actions | confirmed | pass |
| `jira.issue.unassign` | write | jira_issue_mutation_host/actions | confirmed | pass |
| `jira.issue.delete` | permanent | jira_issue_mutation_host | confirmed | pass |
| `jira.comment.create` | write | jira_create_host/comment_writes | confirmed | pass |
| `jira.comment.update` | write | jira_create_host/comment_writes | confirmed | pass |
| `jira.comment.delete` | permanent | jira_create_host/comment_writes | confirmed | pass |
| `jira.link.create` | write | jira_create_host/links, link_lifecycle | confirmed | pass |
| `jira.link.delete` | permanent | jira_create_host/link_lifecycle | confirmed | pass |
| `jira.link.reconcile` | read | jira_create_host/link_lifecycle | not run | not run |
| `jira.attachment.upload` | write | jira_issue_reads_host/uploads | not run | pass |
| `jira.attachment.delete` | permanent | jira_issue_reads_host/deletes | not run | pass |
| `operation.reconcile` | read | jira_issue_mutation_host/recovery, jira_create_host/recovery | not run | not run |

Every write's fixture suite covers, at minimum: refusal before any request for missing apply, profile, policy or output budget; policy and scope rechecks around the claim; one journaled send with no automatic retry; a lost or unqualified response recorded as `sent_unknown`; and a rerun of the same operation ID refusing without another send. Response envelopes are validated against the published response schema in the host suites. The latest local results for this source: `cargo test --workspace` passed with no failures; the eight Jira composed hosts passed (77, 9, 7, 15, 30, 6, 8 and 10 cases); runtime synthetic-acceptance passed 152 (3 native opt-ins ignored); both Clippy modes, `cargo fmt --check`, `tools/contracts.py check`, `tools/plan.py check`, `tools/providers.py check` and `tools/runtime_skill.py check` passed. The Confluence composed host was not rerun; Windows Application Control still blocks it (see `STATE.md`).

## Published-schema and outcome qualification

Run on **16 November 2026** on Windows from the `jira-conformance` branch. Every envelope the eight composed Jira host suites produce is now validated against the response schema that `schemas/local-contracts.json` publishes for its command (`crates/craft-cli/tests/support/conformance.rs`). Explicit field selections are validated against the schema the published projection contract derives for them. Hosts that return `(data, error)` below the command layer (comment writes, link create/delete/reconcile, attachment delete) are wrapped exactly as their public command wraps them, and prepare-time refusals are validated as data-free failure envelopes. Before this change, schema checks existed only in some suites and none recorded which outcomes each command reached.

`python tools/jira_conformance.py run` runs the suites with `CRAFT_CONFORMANCE_RECORD` set and fails unless every Jira command the contract publishes has a validated success, a validated permission refusal (`PERMISSION_DENIED`, `POLICY_DENIED` or `AUTH_REQUIRED`), and either a validated bound or transient failure (reads) or a validated `WRITE_OUTCOME_UNKNOWN` plus `OPERATION_ID_REUSED` or `APPLY_REQUIRED` refusal (writes). Its `selftest` runs in `tools/verify.py`.

The first recorded run validated every envelope but found 40 gaps across 15 commands: seven write/reconcile commands were never checked at envelope level, and several reads had no recorded scope refusal or deadline case. Two fixture families also failed the published default schema because they request explicit fields, which is why the checker is projection-aware. New cases close every gap:

- issue get, attachment get/list/settings, comment get/list and link get/list/types refuse with `PERMISSION_DENIED` without `read:jira-work` after the two identity requests, and end in `DEADLINE_EXCEEDED` after exactly one stalled resource request (no retry). The deadline is a manual clock expired once the stalled request arrives, because the TLS fixture closes a stalled connection after two seconds, which a real-clock deadline would report as `REMOTE_UNAVAILABLE`;
- transition list now runs the late policy-denial, cancellation, disconnect and deadline cases that already covered edit metadata;
- a lost comment delete response is `sent_unknown` and the same operation cannot send again; comment and link-delete resend refusals are recorded;
- attachment delete refusals for any-intent under an own allowance (`POLICY_DENIED`) and missing apply (`APPLY_REQUIRED`) are recorded.

Final run: 175 cases across the eight suites passed (79, 9, 7, 15, 40, 6, 8 and 11), 33 commands, 0 gaps. Recorded outcomes per command:

| Command | Validated outcomes |
| --- | --- |
| `jira.attachment.delete` | ok, APPLY_REQUIRED, OPERATION_ID_REUSED, POLICY_DENIED, WRITE_OUTCOME_UNKNOWN |
| `jira.attachment.download` | ok, CANCELLED, DEADLINE_EXCEEDED, INPUT_INVALID, ITEM_TOO_LARGE, PERMISSION_DENIED, POLICY_DENIED, REMOTE_UNAVAILABLE, VERSION_CONFLICT |
| `jira.attachment.get` | ok, DEADLINE_EXCEEDED, PERMISSION_DENIED, POLICY_DENIED, REMOTE_UNAVAILABLE |
| `jira.attachment.list` | ok, CURSOR_MISMATCH, DEADLINE_EXCEEDED, PERMISSION_DENIED, POLICY_DENIED |
| `jira.attachment.settings` | ok, DEADLINE_EXCEEDED, PERMISSION_DENIED |
| `jira.attachment.upload` | ok, OPERATION_ID_REUSED, PARTIAL_APPLY, PERMISSION_DENIED, UNSUPPORTED_CAPABILITY, WRITE_OUTCOME_UNKNOWN |
| `jira.comment.create` | ok, OPERATION_ID_REUSED, POLICY_DENIED, WRITE_OUTCOME_UNKNOWN |
| `jira.comment.delete` | ok, OPERATION_ID_REUSED, PERMISSION_DENIED, WRITE_OUTCOME_UNKNOWN |
| `jira.comment.get` | ok, DEADLINE_EXCEEDED, PERMISSION_DENIED |
| `jira.comment.list` | ok, CURSOR_MISMATCH, DEADLINE_EXCEEDED, PERMISSION_DENIED, POLICY_DENIED |
| `jira.comment.update` | ok, INPUT_INVALID, OPERATION_ID_REUSED, PERMISSION_DENIED, VERSION_CONFLICT, WRITE_OUTCOME_UNKNOWN |
| `jira.field.list` | ok, DEADLINE_EXCEEDED, POLICY_DENIED, VERSION_CONFLICT |
| `jira.field.search` | ok, CURSOR_MISMATCH, ITEM_TOO_LARGE, PERMISSION_DENIED, POLICY_DENIED, REMOTE_UNAVAILABLE, VERSION_CONFLICT |
| `jira.issue.assign` | ok, CANCELLED, OPERATION_ID_REUSED, PERMISSION_DENIED, POLICY_DENIED, RATE_LIMITED, VERSION_CONFLICT, WRITE_OUTCOME_UNKNOWN |
| `jira.issue.create` | ok, AUTH_REQUIRED, CANCELLED, DEADLINE_EXCEEDED, INTERNAL, OPERATION_ID_REUSED, PARTIAL_APPLY, PERMISSION_DENIED, POLICY_DENIED, VERSION_CONFLICT, WRITE_OUTCOME_UNKNOWN |
| `jira.issue.delete` | ok, CANCELLED, OPERATION_ID_REUSED, PERMISSION_DENIED, POLICY_DENIED, RATE_LIMITED, VERSION_CONFLICT, WRITE_OUTCOME_UNKNOWN |
| `jira.issue.get` | ok, DEADLINE_EXCEEDED, ITEM_TOO_LARGE, PERMISSION_DENIED, POLICY_DENIED, VERSION_CONFLICT |
| `jira.issue.search` | ok, CANCELLED, CURSOR_MISMATCH, DEADLINE_EXCEEDED, ITEM_TOO_LARGE, PAGINATION_STALLED, PERMISSION_DENIED, POLICY_DENIED, REMOTE_UNAVAILABLE, VERSION_CONFLICT |
| `jira.issue.unassign` | ok, CANCELLED, OPERATION_ID_REUSED, PERMISSION_DENIED, POLICY_DENIED, RATE_LIMITED, VERSION_CONFLICT, WRITE_OUTCOME_UNKNOWN |
| `jira.issue.update` | ok, CANCELLED, OPERATION_ID_REUSED, PERMISSION_DENIED, POLICY_DENIED, RATE_LIMITED, VERSION_CONFLICT, WRITE_OUTCOME_UNKNOWN |
| `jira.link.create` | ok, CANCELLED, INPUT_INVALID, OPERATION_ID_REUSED, PERMISSION_DENIED, POLICY_DENIED, VERSION_CONFLICT, WRITE_OUTCOME_UNKNOWN |
| `jira.link.delete` | ok, OPERATION_ID_REUSED, PERMISSION_DENIED, REMOTE_UNAVAILABLE, VERSION_CONFLICT, WRITE_OUTCOME_UNKNOWN |
| `jira.link.get` | ok, DEADLINE_EXCEEDED, PERMISSION_DENIED |
| `jira.link.list` | ok, CURSOR_MISMATCH, DEADLINE_EXCEEDED, PERMISSION_DENIED, POLICY_DENIED |
| `jira.link.reconcile` | ok, OPERATION_ID_REUSED, POLICY_DENIED, RATE_LIMITED |
| `jira.link.types` | ok, DEADLINE_EXCEEDED, PERMISSION_DENIED |
| `jira.metadata.edit` | ok, CANCELLED, DEADLINE_EXCEEDED, ITEM_TOO_LARGE, PERMISSION_DENIED, POLICY_DENIED, REMOTE_UNAVAILABLE, VERSION_CONFLICT |
| `jira.permission.check` | ok, CURSOR_MISMATCH, DEADLINE_EXCEEDED, ITEM_TOO_LARGE, PERMISSION_DENIED, POLICY_DENIED, REMOTE_UNAVAILABLE, VERSION_CONFLICT |
| `jira.project.get` | ok, ITEM_TOO_LARGE, POLICY_DENIED, REMOTE_UNAVAILABLE, VERSION_CONFLICT |
| `jira.project.list` | ok, CURSOR_MISMATCH, DEADLINE_EXCEEDED, ITEM_TOO_LARGE, PAGINATION_STALLED, PERMISSION_DENIED, POLICY_DENIED, REMOTE_UNAVAILABLE, VERSION_CONFLICT |
| `jira.project.statuses` | ok, CANCELLED, CURSOR_MISMATCH, DEADLINE_EXCEEDED, ITEM_TOO_LARGE, PERMISSION_DENIED, POLICY_DENIED, REMOTE_UNAVAILABLE, VERSION_CONFLICT |
| `jira.transition.apply` | ok, OPERATION_ID_REUSED, POLICY_DENIED, RATE_LIMITED, VERSION_CONFLICT, WRITE_OUTCOME_UNKNOWN |
| `jira.transition.list` | ok, CANCELLED, DEADLINE_EXCEEDED, POLICY_DENIED, REMOTE_UNAVAILABLE, VERSION_CONFLICT |

`operation.reconcile` is recorded too (ok, OPERATION_ID_REUSED, POLICY_DENIED, RATE_LIMITED, VERSION_CONFLICT) but is not a Jira-prefixed command, so the gap check does not require it. Target failures are not one error code (Jira reports a missing or invisible target as 404, mapped to `PERMISSION_DENIED` for issue mutations and `REMOTE_UNAVAILABLE` for comment and attachment deletes), so the check does not enforce them; the suites' named target-drift, fingerprint, cursor-mismatch and project-mismatch cases cover them. `cargo fmt --all --check` and `cargo clippy -p craft-cli --all-targets --features synthetic-acceptance -- -D warnings` passed on the final source.

Provider live, same day, read-only: `cargo test -p craft-live-conformance --test live_reads -- --ignored` with the stored account API token passed all 18 Jira provider reads it reached against the test site (and the six Confluence reads). Comment get, attachment get and link get were skipped because the earlier lifecycle cleanup left no comments, attachments or links on recent issues. No mock divergence was found. Host live was not rerun: the service-account state root under `target/live-state` was deleted outside this work on 16 November 2026 and must be provisioned again by the site owner.

## Host lifecycle run

Run on **14 November 2026** after the site owner approved it in writing in the project thread. `python tools/live_host.py lifecycle --project <id> --issue-type <id>` created two `craft-test lifecycle` issues and one `craft-test attachments` issue through the built executable, then:

- comment create, get, update (own) and delete (own) on the first issue: all confirmed;
- link types, link create (Blocks, first to second), link list, link get and link delete: all confirmed. Jira's create response carries no link ID, so the host reports `relation_confirmed` and the ID comes from the following list;
- assign the second issue to the service account and unassign it: both confirmed;
- upload a 36-byte text file, list, get, download and delete it: upload acknowledged with filename and size verified, the snapshot digest matched the source, get reported the same size and media type, the downloaded bytes had the uploaded SHA-256, delete confirmed and the follow-up list was empty;
- all three issues deleted. A JQL search for `craft-test` in the project afterwards returned nothing.

The first attempt's link step stopped after create because the script expected a link ID in the create response; the fix was in the script, not the CLI. No provider divergence was found.

## Effective actor and permissions

Host live runs use the `CraftTest` service account with the ten OAuth scopes listed in the live evidence. It holds the Administrator role in the single team-managed test project; without a project role it could create, edit and transition but not delete issues. Native permissions checked by the commands (Browse projects, Create/Delete attachments, Add/Edit/Delete comments, Link issues, Assign, Delete issues) were all granted through that role. Permission-denied paths are covered by fixtures only.

## Ledger status

Every dependency of `jira-conformance` closed on 15 November 2026 ([foundation closure](foundation-closure.md), [pagination](pagination-conformance.md)). With the published-schema and outcome qualification above, every advertised Core Jira operation has route, behavior and recovery evidence, so `jira-conformance` is checked. That is fixture qualification on Windows plus one small live site; it is not a production-support claim, and other platforms were not rerun for this change.

## Not covered

- `jira.link.reconcile` and `operation.reconcile` have not met a real uncertain outcome live.
- The full sandbox write lifecycle last ran live on 14 November 2026 (above). It was not rerun for the outcome qualification: it needs the service-account state provisioned again and the site owner's written approval.
- Large sites, custom-field-heavy create screens, issue security levels and restricted visibility are not represented on the test site.
- Gateway-rewritten continuation links are exercised only where the site returned more than one page (field search).
