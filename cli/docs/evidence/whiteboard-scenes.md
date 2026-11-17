# Whiteboard scene evidence

Recorded 17 November 2026 on Windows. Usage is in [whiteboard scenes](../whiteboard-scenes.md).

## Format research on the test site

With the site owner's approval and signed-in browser session, in the test space:

- Public routes: the current v2 specification (`openapi-v2.v3.json`, 2.0.0) has no whiteboard content route; whiteboard create accepts `templateKey` (53 keys) and `locale`. A create with `templateKey: flow-chart` and `locale: en-US` returned 200 and the board opened with the flowchart template.
- Storage: the editor syncs a collaborative document over a websocket; its REST endpoints under the site gateway answer 401 `missing_token` to the session cookie alone. Not used.
- Clipboard: copying in the editor writes only `text/html` containing `<div id="canvas-clipboard" data-canvas-clipboard="BASE64">`; the value decodes to a UTF-8 JSON array of element records. Records were captured for sections, text (template and mind-map), smart links, images, shapes and connectors (flowchart template), stickies, drawings, free lines, attached stamps and mind maps.
- Paste: dispatching a `paste` ClipboardEvent carrying such HTML into the editor frame created the elements, selected, at the pointer. Placement follows `basisPosition` (center) for shapes, stickies and text, and `position` (center) for sections; connectors given only `sourceIndex` and `targetIndex` were bound by the editor. Copying a section returned the section with a pasted sticky inside its bounds and without one outside, so membership follows geometry.
- Pasted and rendered: shape codes 0 to 88 except the compartment shapes 82 and 83 (pasted in two batches without errors), all three routes, dashed and dotted dashes, thick stroke, filled-arrow, filled-diamond and crow's-foot caps, stamps `rocket`, `gold-star` and `thumbs-up` (free and attached), headings, bold, italic, underline, strike, links and ordered lists in ADF.
- Failure modes: a record missing fields (`position`, `size`, `source` and others) stopped the board from rendering ("Something went wrong") until reload; colors outside the token palette rendered black; an ADF emoji node did not render.
- Not verified live: Smart Link paste and the full stamp sprite list beyond the three above. The sprite identifiers come from the editor's stamp message keys.

Test boards were created with the owner's account; see the live run below for cleanup.

## Deterministic tests

`craft-cli` unit tests (`whiteboard_scene::tests`):

- `base64_matches_the_standard_alphabet_and_padding`
- `plain_text_maps_to_headings_lists_and_paragraphs_and_back`: ambiguous or marked text stays ADF.
- `encode_places_elements_by_center_and_binds_connectors_by_index`: every kind, auto sides, attached stamps, HTML round trip.
- `encode_rejects_invalid_scenes_before_producing_a_payload`: 15 rule cases plus unknown shape, field, color, format and kind.
- `decode_maps_editor_records_removes_identities_and_keeps_unknown_records_raw`: editor-shaped records, three private fields removed, no account or ARI text in output, raw records unchanged, decode-encode-decode stable.
- `decode_rejects_payloads_that_are_not_clipboard_arrays`
- `every_named_vocabulary_value_round_trips_through_its_code`

`craft-provider-confluence` `folder_and_whiteboard_create_bind_space_and_parent_then_confirm_type_title_and_parent` now also checks that a whiteboard template and locale reach the body as `templateKey` and `locale`, that folders refuse them, and that unknown keys, unknown locales and a locale without a template are rejected.

`tools/contracts.py write` regenerated the snapshot for the two new local commands and the create flags; `tools/runtime_skill.py check` passes with the rebound skill.
