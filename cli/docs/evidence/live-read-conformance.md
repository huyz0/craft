# Live conformance evidence

Run on **14 November 2026**, Australia/Sydney, from the Windows development host against the authorized Atlassian test site (origin kept in local configuration, not recorded here). Authentication was an Atlassian account API token saved by `tools/live-credential.ps1`; see [usage](../live-conformance.md). No tenant data was created, changed or deleted. Identifiers and response values are omitted.

## Result

24 provider read commands passed against live responses, 0 failed, 5 skipped for lack of site data.

| Command | Result | Live exchanges |
| --- | --- | --- |
| `jira.project.list` | pass | project search |
| `jira.project.get` | pass | project by key |
| `jira.project.statuses` | pass | project statuses |
| `jira.field.list` | pass | field list |
| `jira.field.search` | pass after fix | field search, two pages |
| `jira.issue.get` | pass | issue |
| `jira.issue.search` | pass | JQL search (POST) |
| `jira.metadata.edit` | pass | issue, edit metadata, issue |
| `jira.metadata.create` | pass | project, create issue types, issue type fields |
| `jira.metadata.assignee` | pass | assignable-user lookup |
| `jira.transition.list` | pass | issue, transitions, issue |
| `jira.permission.check` | pass | my permissions |
| `jira.operation.inspect` | pass | issue |
| `jira.comment.list` | pass | issue comments (empty) |
| `jira.attachment.list` | pass | issue attachment field (empty) |
| `jira.attachment.settings` | pass | attachment settings |
| `jira.link.list` | pass | issue link field (empty) |
| `jira.link.types` | pass | link types |
| `confluence.space.list` | pass after fix | spaces |
| `confluence.space.get` | pass | space |
| `confluence.page.list` | pass | pages |
| `confluence.page.get` | pass | page |
| Jira reads through the host (project list/get/statuses, field list/search, issue get/search, edit metadata, transitions, permission check, comment/attachment/link lists, attachment settings, link types) | 15 pass |
| Confluence reads through the host (space list/get, page list/get/search, footer-comment list) | 6 pass |
| `confluence.page.create` (root placement in the onboarding space) | confirmed; page removed afterwards |
| `confluence.comment.list` | pass | footer comments (empty) |
| `jira.comment.get` | skipped | no comment on the site |
| `jira.attachment.get` | skipped | no attachment on the site |
| `jira.link.get` | skipped | no issue link on the site |
| `confluence.comment.get` | skipped | no footer comment on the site |
| `confluence.comment.replies` | skipped | no comment reply on the site |

## Divergences between mocks and the live service

1. **Field search continuation.** Live `GET /rest/api/3/field/search` returned `isLast: false` with `startAt`, `maxResults` and `total` but no `nextPage` (and no `self`). Project, workflow, status and screen searches on the same site did include `nextPage`. The provider required a link for every non-final page, so field search failed on any site with more fields than one page. Fixtures had always supplied `nextPage`. The provider now advances by the returned record count when `isLast` is explicitly false and no link is present, refuses an empty non-final page as stalled and keeps validating a link when one exists. The design records this as a field-search exception.
2. **Field search records without a schema.** Three system fields (issue key, parent and thumbnail) came back with no `schema` object. The provider required `schema.type` for every search record. A missing schema is now reported through `fields_not_returned` when selected; a present schema must still have a non-empty type.
3. **Confluence space type.** A new site's default space has `type: "onboarding"`. The provider accepted only `global`, `personal`, `collaboration` and `knowledge_base` and rejected the whole space list. `onboarding` is now accepted. The live list also had null `description` and `icon` and only `_links.base` at the top level; those were already handled.

Each fix has a deterministic fixture that mirrors the observed shape: `live_shaped_field_search_advances_by_returned_records_and_tolerates_schemaless_fields` in `craft-provider-jira/tests/fields.rs` and `live_shaped_space_list_accepts_onboarding_and_personal_spaces` in `craft-provider-confluence/tests/context.rs`. The earlier fixture case that expected `isLast: false` without a link to fail now expects an empty non-final page to fail.

Two early failures were harness errors, not product divergences: production read handlers refuse the generic registry path (the harness now calls each provider's read entry point, as the hosts do), and `jira.metadata.assignee` requires a numeric project target.

## Checks

- Live harness rerun after the fixes: 24 passed, 0 failed, 5 skipped.
- `cargo test -p craft-provider-jira -p craft-provider-confluence --offline`: all passed.
- `cargo test -p craft-cli --test jira_field_reads_host --features synthetic-acceptance --locked --offline`: 9 passed.
- `cargo clippy` with `-D warnings` on the two providers and the harness, and `cargo fmt --all --check`: clean.
- `python tools/providers.py check`: ok. `python tools/contracts.py check` reports drift in the `artifact.get`/`artifact.export` collection enumerations; the same drift is present without these changes, so it predates this work and is left for its owner.

## Write lifecycle

Run on **14 November 2026** after the site owner approved it in writing. `tests/live_writes.rs` created two Jira issues and one Confluence page titled `craft-test live <timestamp>`, exercised provider writes on them, ran the five previously skipped reads against that data, and removed everything. Each provider write ran the host sequence: the provider's bracketing reads and freeze through a context bound to the read descriptor, one send of the frozen call through a context bound to the write descriptor, then the provider's interpretation of the real response.

| Step | Result |
| --- | --- |
| `jira.comment.create`, `jira.comment.update`, `jira.comment.delete` | confirmed (201, 200, 204) |
| `jira.link.create`, `jira.link.delete` | confirmed (201, 204) |
| `jira.issue.update` (summary) | confirmed (204) |
| `jira.issue.assign`, `jira.issue.unassign` | confirmed (204) |
| `jira.issue.delete` (both issues, subtasks refused) | confirmed (204) |
| `confluence.page.create` (root placement) | confirmed |
| `jira.comment.get`, `jira.link.get`, `jira.attachment.get` | pass against the created data |
| `confluence.comment.get`, `confluence.comment.replies` | pass against the created data |

Fixture data no public command manages was created and removed with raw requests: the two issues, one text attachment, and a Confluence footer comment and reply. Cleanup trashed and purged the page; the attachment went with its issue. A follow-up JQL and CQL search for `craft-test` returned no issues and no pages.

No provider divergence was found in the write paths. The only failures in the first attempt were harness inputs: comment and link writes bind to a numeric project target, not the site.

With this run, every read listed above has passed against live data at least once (29 reads), and 11 provider writes have confirmed live outcomes.

## Production host with the service account

Run on **14 November 2026** with the non-interactive production path: the site owner created the `CraftTest` service account and its OAuth client credential, then ran `craft auth provision` to verify the actor against both products and store the grant in Windows Credential Manager. `tools/live_host.py` drives the built `craft` executable with an explicit state root, local policies (`policy set`) and the provisioned profiles; it never handles secrets.

| Area | Result |
| --- | --- |
| `auth provision` (OAuth client-credentials exchange, Jira and Confluence current-user proofs, native vault) | confirmed by the site owner's run; `auth status` shows the stored grant |
| `policy set` for jira-read, jira-project-read, jira-project-context, jira-field-read, jira-create, jira-mutation, confluence-read | all confirmed |
| `jira.issue.create` -> `jira.transition.apply` -> `jira.issue.update` -> `jira.issue.delete` on a `craft-test host` issue | all confirmed after granting the service account the project Administrator role |

Findings:

4. **Create metadata issue type.** Live create metadata reports the system `issuetype` field with an empty `operations` list, because the type is fixed by the per-type route. The provider required a `set` operation, so every `jira.issue.create` failed validation. The issue type is now settable to its const value; fixture `live_shaped_issue_type_without_operations_still_compiles_create` mirrors the shape.
5. **Project-context targets.** Project-context policies bind the exact target used at setup; invoking with the project key when the policy names the numeric ID is refused. This is documented behavior, recorded here because it is easy to trip over.
6. **Delete permission.** In a team-managed project with open access, a service account without a project role can create, edit and transition but not delete. The CLI reported Jira's refusal as a confirmed failure with no retry, as designed. Leftover `craft-test` issues from refused deletes were removed with `tests/live_cleanup.rs`.

7. **Service-account scope selection.** Atlassian caps a service-account OAuth credential at 50 scopes and lists them alphabetically. Selecting broadly fills the cap with `delete:` and `manage:` scopes, leaving only classic Confluence scopes, and the v2 space/page operations then fail the CLI's scope check (Atlassian's v2 reference lists only granular scopes for them). A credential with exactly these ten scopes passes every implemented command: `read:jira-work`, `write:jira-work`, `read:jira-user`, `read:confluence-user`, `read:space:confluence`, `read:page:confluence`, `write:page:confluence`, `read:comment:confluence`, `read:content-details:confluence`, `search:confluence`. The CLI's refusal happened before any request, as designed.

## Limits

- Not exercised live: `confluence.page.update` (still withheld pending authorization), attachment upload/download/delete through the CLI, ticket application and review flows, reconciliation of an unknown outcome.
- The production host, service-account OAuth, policy, journal and `api.atlassian.com` gateway routing were not exercised. Gateway-rewritten continuation links remain unobserved. Service-account eligibility is still pending in the [tenant inventory](tenant-auth-inventory.md).
- One small site: one Jira project, two spaces, a handful of pages. Large-page, custom-field and permission-restricted behavior is not represented.
