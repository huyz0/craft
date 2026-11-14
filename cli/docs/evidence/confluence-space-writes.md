# Confluence space write evidence

Recorded 14 November 2026 on Windows. Usage is in [space writes](../confluence-space-writes.md).

## Deterministic tests

Provider preparation and interpretation (`craft-provider-confluence/tests/pages.rs`):

- `update_requires_explicit_editor_draft_policy_bound_into_digest_but_never_sent`: missing, null and unknown policy values fail; the PUT body never contains the policy.
- `trash_rechecks_revision_and_sends_one_delete_without_purge`: 204 confirms; 400/404 refuse; 200/429/500 stay unknown; stale version or digest fails before compile.
- `comment_writes_bind_page_space_parent_and_reviewed_revision`: top-level create sends only `pageId`, a reply only `parentCommentId` after its parent is proven on the same page; a response for another page is unknown without a resource; update sends the next version; delete is `Permanent` and confirms on 204; stale revisions fail.
- `attachment_trash_compiles_one_delete_bound_to_version_and_file`: a moved page or replaced file ID fails; malformed `att` IDs are refused.
- `write_contracts_match_the_pinned_native_inventory` still passes.

Composed host with mock TLS servers (`craft-cli --features synthetic-acceptance --test confluence_context_host`):

- `page_trash_rechecks_revision_sends_one_delete_and_refuses_stale_or_unscoped`
- `footer_comment_writes_read_page_and_comment_evidence_then_send_once` (create, reply, update, delete, stale revision, missing write scope)
- `content_state_reads_pin_current_status_and_never_grant_approval` (badge, no badge, available list, malformed list, 403)
- `content_state_writes_require_an_offered_state_and_pin_current_status` (space state, custom state, state not offered, clear, mismatched response stays unknown)
- `attachment_reads_bind_the_page_and_follow_only_cursor_continuations` (get, paged list with matching Link header and `_links.next`, foreign page, trashed attachment; no download link in output)
- `attachment_trash_rechecks_version_and_file_then_deletes_without_purge` (trash, replaced file, newer version)

The full Confluence host binary passed 58 cases before these additions with `--test-threads=4`; the earlier Application Control block on that executable did not recur. Under default parallelism some long review-verification cases hit their invocation deadline; with four threads all pass. Workspace Clippy (`-D warnings`, with `synthetic-acceptance`) and `local_interface` passed after each change. `tools/contracts.py write` regenerated `schemas/local-contracts.json`, which also absorbs the earlier `artifact.get`/`artifact.export` collection-enum drift.

## Merged-tree verification

After merging main (Jira attachment upload/delete) into this work, `tools/verify.py` selected the full scope. Tooling, contracts, provider routes, skill checks, format and both Clippy modes passed. Under machine load, three `craft-runtime` profile-listing tests and 25 runtime library tests hit their 5-second invocation deadlines, and one long ticket-application host case did the same. Rerun with lower parallelism once the load dropped, all of them passed: runtime library 152 passed with `--test-threads=2`, profiles 11 passed in 1.9 s, and the host case passed on its own. The rest of the workspace tests passed, the Confluence host passed 63 of 64 with `--test-threads=4` (the remaining case passed on its own), the eight Jira hosts passed all 162 cases, and synthetic-acceptance isolation passed.

## Live test site

Through the production host with the CraftTest service profile, in the onboarding space, on `craft-test` pages that cleanup trashed and purged:

| Command | Result |
| --- | --- |
| `confluence.page.update` (title and storage body, explicit policy) | confirmed version 1 to 2; receipt shows the policy and `editor_draft_preservation_verified: false` |
| `confluence.page.update` with the stale revision | `VERSION_CONFLICT`, nothing sent |
| `confluence.content-state.get` | pass (`read:content-details:confluence`) |

Not yet live: page trash, comment writes, badge set/clear/available and attachment reads/trash. They need a credential with `delete:page`, `write:comment`, `delete:comment`, `write:content`, `read:attachment` and `delete:attachment` (all `:confluence`). `tools/live_host.py page --space <id> --comments --badges --attachments --trash` runs them once that credential is provisioned. Editor-draft interaction with another person's open draft is not live-tested.
