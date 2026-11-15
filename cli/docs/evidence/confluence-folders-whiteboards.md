# Confluence folder and whiteboard evidence

Recorded 16 November 2026 on Windows. Usage is in [space writes](../confluence-space-writes.md#folders-and-whiteboards).

## Deterministic tests

Provider preparation and interpretation (`craft-provider-confluence/tests/pages.rs`):

- `folder_and_whiteboard_create_bind_space_and_parent_then_confirm_type_title_and_parent`: root placement sends only `spaceId` and `title`; page and folder parents add `parentId` after the parent is proven current in the same space. A trashed folder, a page given as a folder, a foreign page or a missing parent read fails before compile. 200 and 201 confirm only with the requested type, space, title and parent; another type or space keeps no resource; a changed title or parent stays unknown with the known ID; 400 is refused and 500 is unknown. Template keys and other extra fields are rejected.
- `folder_and_whiteboard_trash_recheck_the_reviewed_version_and_send_one_delete`: 204 confirms; a newer version is `VERSION_CONFLICT`; a trashed item, another space or the other content type fails; version 0 is rejected.
- `write_contracts_match_the_pinned_native_inventory` still passes.

Composed host with mock TLS servers (`craft-cli --features synthetic-acceptance --test confluence_context_host`, `content` module):

- `folder_and_whiteboard_get_check_type_and_omit_private_fields`: the v2 route by ID, a wrong native type and a 403 fail, a missing read allowance stops before any request, and author/owner IDs and `_links` never reach output. The envelope matches the published response schema.
- `folder_and_whiteboard_writes_read_space_parent_and_revision_then_send_once`: root and in-folder create read the space (and the parent folder) and send one POST with the exact body; a lost create response is `sent_unknown` with reconciliation required; trash reads the item and sends one DELETE without `purge`; a stale version, a missing write scope and a missing space allowance send nothing.

`tools/providers.py check` covers the six new catalog commands against the pinned v2 specification, and `tools/contracts.py check` and `tools/runtime_skill.py check` pass with the regenerated contract snapshot.

## Live test site

Not run yet. It needs the CraftTest credential to carry `read:folder`, `write:folder`, `delete:folder`, `read:whiteboard`, `write:whiteboard` and `delete:whiteboard` (all `:confluence`), the new operations in the local cf-test policies, and the site owner's approval for the writes. Then `python tools/live_host.py content --space <id>`.
