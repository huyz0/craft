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
| `confluence.folder.create`, `confluence.whiteboard.create` | POST folder or whiteboard (v2) | current space; a named parent page or folder current in that space | HTTP 200 (or 201) with that type, space, title and parent |
| `confluence.folder.trash`, `confluence.whiteboard.trash` | DELETE folder or whiteboard, no purge | the current, empty item at the reviewed version | HTTP 204 |

## Space create

`confluence.space.create` creates a space through the v2 route with an explicit `key`, `name`, `visibility` (`default_roles` or `private`, no default) and optional plain description. It first runs an exact-key lookup and refuses with `VERSION_CONFLICT` if any visible space has that key. Atlassian offers this route on sites with role-based access control; there is no fallback to the legacy v1 route, so an uncertain create is never replayed elsewhere. It needs `write:space:confluence` and the site's Create space permission.

Its authority is a site-level `confluence-admin` policy listing `confluence.space.create`. Per-space write policies and read policies never grant it. Space update, deletion and permission changes are not exposed.

## Folders and whiteboards

`confluence folder get` and `confluence whiteboard get` take a numeric `id` and return its space, title, status, parent ID and type, and version. Each needs its own `confluence-read` allowance and `read:folder:confluence` or `read:whiteboard:confluence`. Author and owner account IDs are not returned, and whiteboard drawing content has no API.

`create` takes `space_id`, a `title` of at most 255 characters without surrounding spaces, and `placement`: `{"kind":"root"}`, `{"kind":"page","id":…}` or `{"kind":"folder","id":…}`. Before sending, Craft reads the space and the named parent and refuses one that is not current in that space or is not of the named type. Root placement sends no `parentId`; on the test site Confluence then files the item under the space homepage. A whiteboard starts blank unless `template` names one of Confluence's built-in templates (for example `flow-chart` or `kanban-board`; the list follows the pinned v2 specification), optionally with a template `locale` such as `en-US`; folders refuse both. Private creation is not exposed. Drawing content has no API: [whiteboard scenes](whiteboard-scenes.md) explains how `whiteboard encode` and `whiteboard decode` move content through the editor's copy and paste. On the test site the CraftTest service account gets HTTP 404 from whiteboard create while folder create, whiteboard get and trash work; an account token against the site URL can create whiteboards. Other OAuth 2.0 callers report the same 404 through the api.atlassian.com gateway, so treat whiteboard create as unavailable to OAuth credentials until Atlassian serves it. A create is confirmed only when the response has the requested type, space, title and, when named, parent; a different type or space keeps no resource and stays unknown.

`trash` takes `id` and `expected` (`{"version":…}` from the matching get) and refuses a newer version, an item that is no longer current or in another space, and an item that still has direct children: native trash would move the whole subtree with it, so trash or move the children first. The evidence read asks for `include-direct-children`. The v2 routes move the item to the trash and offer no purge. Scopes are `write:folder:confluence` and `delete:folder:confluence`, or the whiteboard equivalents, plus `read:space:confluence` and the parent's read scope for create.

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

`confluence attachment get` and `list` return current page attachment metadata: ID (`att…`), page, title, media type, size, file ID, version and comment. Download links are not returned because they are not trusted routing input. Filename upsert and metadata update are not available.

## Attachment download

`confluence attachment download` takes `id` (`att…`), `page_id`, an immutable `artifact_name`, an existing `output_root` outside the private state root and a relative `output`, with optional `byte_limit` and `expected_digest`. It needs a `confluence-read` allowance for `confluence.attachment.download` and `read:attachment:confluence`.

Confluence answers every native download with a redirect to its media service. The owner approved following exactly one redirect, under these rules: the Location must be an absolute HTTPS URL on `https://api.media.atlassian.com` with no user information or fragment, the follow-up GET carries no Craft credential (the URL embeds its own short-lived media token), it is sent once without retry, and a second redirect is refused. Bytes then stream into the immutable artifact and the guarded output with the same size, digest and publication rules as Jira download. Receipts report `followed_media_redirect`. Every other provider read still refuses redirects.

## Scopes

Atlassian lists granular scopes for the v2 routes and classic scopes, with a granular alternative, for the v1 badge routes. A service credential needs, in addition to the read scopes: `write:page:confluence`, `delete:page:confluence`, `write:comment:confluence`, `delete:comment:confluence`, `read:attachment:confluence`, `delete:attachment:confluence` and `write:content:confluence` (with `read:content-details:confluence`). Missing scopes fail before any request.

Test evidence is in [space write evidence](evidence/confluence-space-writes.md).
