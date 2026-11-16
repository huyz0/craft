# Confluence folder and whiteboard evidence

Recorded 16 November 2026 on Windows. Usage is in [space writes](../confluence-space-writes.md#folders-and-whiteboards).

## Deterministic tests

Provider preparation and interpretation (`craft-provider-confluence/tests/pages.rs`):

- `folder_and_whiteboard_create_bind_space_and_parent_then_confirm_type_title_and_parent`: root placement sends only `spaceId` and `title`; page and folder parents add `parentId` after the parent is proven current in the same space. A trashed folder, a page given as a folder, a foreign page or a missing parent read fails before compile. 200 and 201 confirm only with the requested type, space, title and parent; another type or space keeps no resource; a changed title or parent stays unknown with the known ID; 400 is refused and 500 is unknown. Template keys and other extra fields are rejected.
- `folder_and_whiteboard_trash_recheck_the_reviewed_version_and_send_one_delete`: 204 confirms; a newer version or listed direct children is `VERSION_CONFLICT`; a missing or truncated children listing, a trashed item, another space or the other content type fails; version 0 is rejected. Titles with surrounding spaces or more than 255 characters are rejected.
- `write_contracts_match_the_pinned_native_inventory` still passes.

Composed host with mock TLS servers (`craft-cli --features synthetic-acceptance --test confluence_context_host`, `content` module):

- `folder_and_whiteboard_get_check_type_and_omit_private_fields`: the v2 route by ID, a wrong native type and a 403 fail, a missing read allowance stops before any request, and author/owner IDs and `_links` never reach output. The envelope matches the published response schema.
- `folder_and_whiteboard_writes_read_space_parent_and_revision_then_send_once`: root and in-folder create read the space (and the parent folder) and send one POST with the exact body; a lost create response is `sent_unknown` with reconciliation required; trash reads the item with `include-direct-children=true` and sends one DELETE without `purge`; a stale version, a non-empty item, a missing write scope and a missing space allowance send nothing.

`tools/providers.py check` covers the six new catalog commands against the pinned v2 specification, and `tools/contracts.py check` and `tools/runtime_skill.py check` pass with the regenerated contract snapshot.

## Live test site

Run on 16 November 2026 with the site owner's written approval, through the production host with the CraftTest profile and new credential `craft-granular-5` (25 scopes: the 18 of `craft-granular-4`, the six folder and whiteboard scopes and `read:content.metadata:confluence`), in the onboarding space. `python tools/live_host.py content --space <id>`:

| Command | Result |
| --- | --- |
| `confluence.folder.create` at root | first run: HTTP 200 but `sent_unknown`, because the live response returns `createdAt` as epoch milliseconds where the reference documents a string. Craft no longer reads `createdAt`, and the mocks now mirror the live shape. After the fix: confirmed; Confluence filed the folder under the space homepage |
| `confluence.folder.create` inside a folder | confirmed with the parent folder |
| `confluence.folder.get` | pass, current and trashed items |
| `confluence.folder.trash` with a stale version | `VERSION_CONFLICT`, nothing sent |
| `confluence.folder.trash` (nested, then root) | confirmed (204); a later get shows `trashed` |
| `confluence.whiteboard.create` (root and inside a folder) | refused: the service account gets HTTP 404 from the create route. Craft recorded `failed`, and no whiteboard was created. The site owner's account token creates whiteboards with the same body (response shape matches folders), so the gap is specific to the service account or OAuth path; the cause is not established |
| `confluence.folder.trash` on a folder with a child folder (after review) | `VERSION_CONFLICT`, nothing sent; the live response lists children under `directChildren.results` with `meta.hasMore`. After trashing the child, the same trash confirmed |
| `confluence.whiteboard.get`, `confluence.whiteboard.trash` | pass on a whiteboard created with the owner's account token; trash confirmed (204) |

The unknown folder from the first run was read with `folder get` and trashed. All `craft-test` folders and whiteboards from these runs are in the space trash; the v2 routes have no purge.
