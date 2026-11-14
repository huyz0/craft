# Live read conformance evidence

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
| `confluence.page.search` | pass | CQL search |
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

## Limits

- Read commands only. Writes (issue, comment, link, transition, page create) were not exercised; they need explicit authorization to change the test site.
- Five reads were skipped because the site has no comments, attachments, links or Confluence footer comments. Covering them needs seeded data, which is itself a tenant change.
- The production host, service-account OAuth, policy, journal and `api.atlassian.com` gateway routing were not exercised. Gateway-rewritten continuation links remain unobserved. Service-account eligibility is still pending in the [tenant inventory](tenant-auth-inventory.md).
- One small site: one Jira project, two spaces, a handful of pages. Large-page, custom-field and permission-restricted behavior is not represented.
