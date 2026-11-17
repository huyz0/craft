# Whiteboard scenes

Confluence's REST API manages whiteboards as content items (create, get, trash, permissions) but has no route that reads or writes what is drawn on them. The editor keeps drawings in a collaborative document behind a private, token-gated service, which Craft does not call. The one supported channel for drawing content is the editor's own copy and paste, so Craft works with that payload.

`craft whiteboard encode --file scene.json` reads a scene and returns `clipboard_html`: the `<div id="canvas-clipboard" data-canvas-clipboard="…">` element the editor writes when a user copies, holding base64 of a UTF-8 JSON array of element records. Pasting it as `text/html` into an open board creates the elements, selected, at the pointer. `craft whiteboard decode --file copy.html` reverses the process for HTML copied from a board. Both commands are local: no profile, credential or network, and `board_written` is always false.

The scene format and its vocabulary are documented for agents in the runtime skill: [workflow](../skills/craft-cloud/references/whiteboards.md) and [format](../skills/craft-cloud/references/whiteboard-scenes.md).

## Design notes

- The editor positions shapes, stickies and text by the center of a basis box (`basisPosition`, `basisSize`) and sections, stamps and links by `position` (center) and `size`. The fields it does not use for an element still have to be present: a record missing any expected field stops the board from rendering until the page reloads. Encode therefore emits complete records, and decode keeps anything it cannot model exactly as a raw record rather than rebuilding it.
- Colors are Atlassian accent tokens. A color outside the palette renders black, so scenes accept only palette names (52) and stickies only the eight sticky colors.
- Connectors bind by position in the array (`sourceIndex`, `targetIndex`); the editor assigns element IDs on paste. Decode emits one scene element per record in order, so index references inside raw records stay valid across a decode and encode.
- Text is stored as a serialized Atlassian Document Format document. Plain scene text covers headings, lists and paragraphs; anything richer is carried as `adf`. Decode only returns plain text when encoding it again reproduces the same document.
- Decode removes author account IDs (`createdBy`, `authorIds` and similar) and resolved resource identifiers (`ari`) everywhere in a record. On paste the editor records the person pasting as the author.
- Image bytes travel in separate `data-file…` attributes of the copied element; decode does not keep them, so images only survive as raw records that point at the source board's files.
- Whiteboard create accepts the template keys and locales enumerated by the pinned v2 specification. The service account's OAuth gateway answers 404 to whiteboard create (see [space writes](confluence-space-writes.md#folders-and-whiteboards)), so templates are verified with an account session and with mocks.
