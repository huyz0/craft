# Confluence page, comment, badge and attachment writes

These commands extend [page publication](confluence-page-commands.md) inside one space. Each one runs through the same journaled service host: it reads fresh native evidence, rechecks the saved space write policy around every read and after the durable send claim, sends once and never replays. Uncertain outcomes keep known resource IDs and require reconciliation.

Every write except space create needs an enabled `confluence-write` policy for the exact space with that operation listed; space create needs a site-level `confluence-admin` policy. Read allowances never grant writes.

| Command | Native effect | Evidence read first | Confirmation |
| --- | --- | --- | --- |
| `confluence.page.update` | PUT page with next version | current page in the reviewed representation | HTTP 200 with next version |
| `confluence.page.trash` | DELETE page, no `purge` | current page matching the reviewed revision | HTTP 204 |
| `confluence.comment.create` | POST footer comment or reply | current page in the space; a reply's parent comment on that page | HTTP 201, current, version 1, same page or parent |
| `confluence.comment.update` | PUT comment with next version | page, then the comment matching the reviewed revision | HTTP 200 with next version |
| `confluence.comment.delete` | DELETE comment (permanent) | page, then the comment matching the reviewed revision | HTTP 204 |
| `confluence.content-state.set` | PUT badge by ID, `status=current` | page, then the states the space offers now | HTTP 200 showing that badge |
| `confluence.content-state.clear` | DELETE badge, `status=current` | page | HTTP 200 with no badge |
| `confluence.attachment.trash` | DELETE attachment, no `purge` | page, then the attachment matching the reviewed version and file ID | HTTP 204 |
| `confluence.space.create` | POST space (v2) | exact-key space lookup with no results | HTTP 201 with numeric ID, same key and name |

## Space create

`confluence.space.create` creates a space through the v2 route with an explicit `key`, `name`, `visibility` (`default_roles` or `private`, no default) and optional plain description. It first runs an exact-key lookup and refuses with `VERSION_CONFLICT` if any visible space has that key. Atlassian offers this route on sites with role-based access control; there is no fallback to the legacy v1 route, so an uncertain create is never replayed elsewhere. It needs `write:space:confluence` and the site's Create space permission.

Its authority is a site-level `confluence-admin` policy listing `confluence.space.create`. Per-space write policies and read policies never grant it. Space update, deletion and permission changes are not exposed.

## Page update and editor drafts

Confluence documents that publishing reconciles into any open editor draft and can overwrite divergent unpublished edits. An update therefore requires `"editor_draft_policy": "native_reconciliation_may_overwrite"` (flag `--editor-draft-policy`). The value is part of the operation digest and is never sent to Confluence. Every update receipt repeats it with `editor_draft_preservation_verified: false`. There is no draft-preserving mode; coordinate editing outside Craft when drafts must survive.

## Revisions

Take `expected` values from reads, never from memory:

- page revision: `confluence page get --fields revision`
- comment revision: `confluence comment get --fields revision`
- attachment version and file ID: `confluence attachment get`

A mismatch stops with `VERSION_CONFLICT` before any write.

## Content-state badges

`confluence content-state get` and `available` are reads that need a `confluence-read` allowance. `set` accepts only a state ID that `available` reports for the page at send time; there is no free-form name or color, so Craft never creates new custom states. IDs 0 to 2 are space defaults, so `state_id` has no default. Badge responses carry `grants_approval: false`. A badge never authorizes plan tickets, approves a review or acts as a Jira workflow transition.

## Attachment upload and replacement

`confluence attachment upload` sends one immutable snapshot of a workspace file (`file_root` plus relative `file`) or of an existing upload artifact selected by `expected_digest`, with an explicit `filename` and `media_type`. Before sending it confirms the page is current in the space and that no current attachment on the page has that filename; an existing name is refused with `VERSION_CONFLICT` rather than turned into a new version. `confluence attachment replace` takes `attachment_id` and `expected` (version and file ID from `confluence attachment get`) and replaces only that attachment's bytes; the filename must equal its current title.

Both use the v1 multipart routes with `X-Atlassian-Token: no-check` and `minorEdit=true`, send at most once, and need `--max-bytes 8388608` for complete snapshot, saved-request and journal evidence. An upload is confirmed by its returned attachment ID; filename, size and (for replacement) the next version are reported in `desired_fields_verified`. Remote byte digests are not verifiable. Filename upsert (v1 PUT) is not exposed.

## Attachment reads

`confluence attachment get` and `list` return current page attachment metadata: ID (`att…`), page, title, media type, size, file ID, version and comment. Download links are not returned because they are not trusted routing input. Download, filename upsert and metadata update are not yet available.

## Scopes

Atlassian lists granular scopes for the v2 routes and classic scopes, with a granular alternative, for the v1 badge routes. A service credential needs, in addition to the read scopes: `write:page:confluence`, `delete:page:confluence`, `write:comment:confluence`, `delete:comment:confluence`, `read:attachment:confluence`, `delete:attachment:confluence` and `write:content:confluence` (with `read:content-details:confluence`). Missing scopes fail before any request.

Test evidence is in [space write evidence](evidence/confluence-space-writes.md).
