# Jira operation conformance

Recorded **14 November 2026**, Australia/Sydney. This is the operation-by-operation matrix for every public Jira command in `schemas/local-contracts.json` (33 commands plus the shared `operation.reconcile`). Fixture evidence is deterministic and runs in the ordinary gate. Live evidence is separate and opt-in; see [live conformance](live-read-conformance.md). A live pass is one run on one small site, not production support.

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
| `jira.comment.get` | read | jira_issue_reads_host/comments | pass | see lifecycle |
| `jira.comment.list` | read | jira_issue_reads_host/comments | pass | pass |
| `jira.link.get` | read | jira_create_host/link_reads | pass | see lifecycle |
| `jira.link.list` | read | jira_create_host/link_reads | pass | pass |
| `jira.link.types` | read | jira_create_host/link_reads | pass | pass |
| `jira.attachment.get` | read | jira_issue_reads_host/attachments | pass | see lifecycle |
| `jira.attachment.list` | read | jira_issue_reads_host/attachments | pass | pass |
| `jira.attachment.settings` | read | jira_issue_reads_host/attachments | pass | pass |
| `jira.attachment.download` | read | jira_issue_reads_host/downloads | not run | see lifecycle |
| `jira.issue.create` | write | jira_create_host | not run | pass after fix |
| `jira.issue.update` | write | jira_issue_mutation_host | confirmed | pass |
| `jira.transition.apply` | write | jira_issue_mutation_host | not run | pass |
| `jira.issue.assign` | write | jira_issue_mutation_host/actions | confirmed | see lifecycle |
| `jira.issue.unassign` | write | jira_issue_mutation_host/actions | confirmed | see lifecycle |
| `jira.issue.delete` | permanent | jira_issue_mutation_host | confirmed | pass |
| `jira.comment.create` | write | jira_create_host/comment_writes | confirmed | see lifecycle |
| `jira.comment.update` | write | jira_create_host/comment_writes | confirmed | see lifecycle |
| `jira.comment.delete` | permanent | jira_create_host/comment_writes | confirmed | see lifecycle |
| `jira.link.create` | write | jira_create_host/links, link_lifecycle | confirmed | see lifecycle |
| `jira.link.delete` | permanent | jira_create_host/link_lifecycle | confirmed | see lifecycle |
| `jira.link.reconcile` | read | jira_create_host/link_lifecycle | not run | not run |
| `jira.attachment.upload` | write | jira_issue_reads_host/uploads | not run | see lifecycle |
| `jira.attachment.delete` | permanent | jira_issue_reads_host/deletes | not run | see lifecycle |
| `operation.reconcile` | read | jira_issue_mutation_host/recovery, jira_create_host/recovery | not run | not run |

Every write's fixture suite covers, at minimum: refusal before any request for missing apply, profile, policy or output budget; policy and scope rechecks around the claim; one journaled send with no automatic retry; a lost or unqualified response recorded as `sent_unknown`; and a rerun of the same operation ID refusing without another send. Response envelopes are validated against the published response schema in the host suites. The latest local results for this source: `cargo test --workspace` passed with no failures; the eight Jira composed hosts passed (77, 9, 7, 15, 30, 6, 8 and 10 cases); runtime synthetic-acceptance passed 152 (3 native opt-ins ignored); both Clippy modes, `cargo fmt --check`, `tools/contracts.py check`, `tools/plan.py check`, `tools/providers.py check` and `tools/runtime_skill.py check` passed. The Confluence composed host was not rerun; Windows Application Control still blocks it (see `STATE.md`).

## Effective actor and permissions

Host live runs use the `CraftTest` service account with the ten OAuth scopes listed in the live evidence. It holds the Administrator role in the single team-managed test project; without a project role it could create, edit and transition but not delete issues. Native permissions checked by the commands (Browse projects, Create/Delete attachments, Add/Edit/Delete comments, Link issues, Assign, Delete issues) were all granted through that role. Permission-denied paths are covered by fixtures only.

## Ledger status

`jira-context`, `jira-issue-writes`, `jira-transitions`, `jira-comments`, `jira-links`, `jira-attachments` and `jira-conformance` stay unchecked. Their own behavior now has route, behavior and recovery evidence here, but `tools/plan.py check` refuses a completed task with incomplete dependencies, and every one of them depends, directly or transitively, on unfinished foundation tasks (`service-oauth`, `bounded-listing`, `operation-preview`, `provider-reconciliation`, `binary-transfer`, `pagination-conformance`, `trusted-http`, `artifact-store`, `mutation-journal` and others; 24 incomplete tasks in the `jira-conformance` closure). Those tasks carry their own platform and qualification bars, which this work did not assess.

## Not covered

- `jira.link.reconcile` and `operation.reconcile` have not met a real uncertain outcome live.
- Large sites, custom-field-heavy create screens, issue security levels and restricted visibility are not represented on the test site.
- Gateway-rewritten continuation links are exercised only where the site returned more than one page (field search).
