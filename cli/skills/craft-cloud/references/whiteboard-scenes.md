# Whiteboard scene format

A scene is a JSON file that `whiteboard.encode` turns into editor clipboard records and `whiteboard.decode` produces from them. Unknown fields, kinds, colors, shapes and sprites are rejected, so a scene that encodes is safe to paste. Visual rules (color meaning, type scale, spacing) come from the atlassian-design skill; this page maps them onto whiteboards.

```json
{
  "format": "craft.whiteboard.scene.v1",
  "elements": [
    {"kind": "section", "id": "flow", "x": 0, "y": 0, "w": 1200, "h": 400, "title": "Release flow", "color": "blue-subtlest"},
    {"kind": "shape", "id": "start", "shape": "start_end", "x": 40, "y": 160, "w": 160, "h": 80, "fill": "green-subtle", "text": "Start"},
    {"kind": "shape", "id": "build", "shape": "rounded_rectangle", "x": 280, "y": 152, "w": 200, "h": 96, "fill": "blue-subtle", "text": "Build\n- compile\n- test"},
    {"kind": "shape", "id": "ok", "shape": "diamond", "x": 560, "y": 120, "w": 160, "h": 160, "fill": "yellow-subtle", "text": "Green?"},
    {"kind": "shape", "id": "ship", "shape": "rounded_rectangle", "x": 840, "y": 64, "w": 200, "h": 96, "fill": "green-subtle", "text": "Ship"},
    {"kind": "shape", "id": "fix", "shape": "rounded_rectangle", "x": 840, "y": 240, "w": 200, "h": 96, "fill": "red-subtle", "text": "Fix and retry"},
    {"kind": "connector", "id": "c1", "from": "start", "to": "build"},
    {"kind": "connector", "id": "c2", "from": "build", "to": "ok"},
    {"kind": "connector", "id": "yes", "from": "ok", "to": "ship", "from_side": {"x": 1, "y": 0.33}},
    {"kind": "label", "on": "yes", "text": "Yes"},
    {"kind": "connector", "id": "no", "from": "ok", "to": "fix", "from_side": {"x": 1, "y": 0.67}, "dash": "dashed"},
    {"kind": "label", "on": "no", "text": "No"},
    {"kind": "stamp", "sprite": "rocket", "on": "ship"}
  ]
}
```

## Workflow

1. Write the elements with ids and rough sizes; positions can be zero when a layout will place them.
2. Lay out: `whiteboard.layout` with `algorithm` `layered` (flows, pipelines, dependencies), `radial` (mind maps: the root in the middle, branches around it), `tree` (org charts, breakdowns, decision trees) or `grid` (sticky boards; one grid per section). It is deterministic, snaps to the 8 px grid, refits sections around their members, keeps chains straight, and sets every connector's sides (facing the other end, along the flow) and spreads ends that share a side so lines do not stack. Set `route` per connector; layout leaves it alone. With `layered`, each section is a tier: its members share one column (or row with `direction` `down`), which suits architecture tiers and swimlanes. Layout moves shapes, stickies, links and sections only; add or move titles, legends and free text afterwards, then check again.
3. Check: `whiteboard.check` (also part of the layout result) reports overlaps, elements straddling a section edge, connectors crossing or running through elements, shared anchor points, text that will not fit its box, low text contrast, too many colors, uneven or off-grid spacing, text sitting on a connector instead of a label, and unlabelled decision exits. Fix warnings; consider notes. The score is 100 minus 10 per warning and 2 per note.
4. Look: `whiteboard.preview` returns an approximate SVG of the scene (fonts, icons and the editor's line routing differ). Render or view it when you can, and judge balance, crowding and readability before pasting.
5. Encode and paste ([whiteboards](whiteboards.md)).

```sh
craft whiteboard layout --file scene.json --algorithm layered --direction right --max-bytes 1048576
craft whiteboard check --file scene.json --max-bytes 1048576
craft whiteboard preview --file scene.json --fields svg --max-bytes 4000000
```

## Geometry and order

- Units are canvas pixels at 100% zoom. `x`, `y` is the top-left corner; the y axis points down. Positions are relative: the pasted group lands where the user last clicked.
- Rotation is in degrees, clockwise, from -360 to 360 (the editor stores radians; Craft converts). Rotate only for deliberate emphasis, such as a tilted sticky; rotated shapes do not connect cleanly.
- Stacking order is array order: later elements are drawn above earlier ones, except that the editor always draws connectors above shapes. Put sections first, then shapes and stickies, then connectors, labels and stamps. Users change it later with Bring to front and Send to back; copying returns elements in stacking order.
- Sections are the only containers. An element whose box lies fully inside a section belongs to it and moves, copies and deletes with it; one that straddles the edge belongs to neither. Sections cannot nest. Confluence whiteboards have no frames (frames imported from Miro become rectangles); use titled sections, or an unfilled shape for a border that groups nothing.
- Groups (`group`, the editor's Group elements) make two or more elements select and move together without a visible container. Use them for an icon with its caption, or a card with its badge; use sections for anything with a title or background.

## Element kinds

Every kind takes an optional `id` (letters, digits, `-`, `_`, `.`, `:`; at most 64) that connectors, labels, stamps and groups refer to. `text` and `adf` are exclusive.

| Kind | Required | Optional (default) |
| --- | --- | --- |
| `shape` | `shape`, `x`, `y`, `w`, `h` | `fill` (`gray-subtle`), `filled` (true; false draws the outline only), `text` or `adf`, `align` (`center`), `valign` (`middle`), `font_scale` (1), `rotation` (0) |
| `sticky` | `x`, `y` | `w`, `h` (144), `color` (`yellow-subtle`), `text` or `adf`, `align` (`center`), `valign` (`middle`), `font_scale`, `rotation` |
| `text` | `x`, `y`, `w` | `text` or `adf`, `color` (`default-text`), `align` (`left`), `font_scale`, `rotation` |
| `section` | `x`, `y`, `w`, `h` | `title`, `color` (`white`), `shadow` (false) |
| `connector` | `from`, `to` (element ids) | `from_side`, `to_side` (`auto`), `route` (`elbow`), `start_cap` (`none`), `end_cap` (`arrow`), `color` (`gray-bold`), `stroke` (`thin`), `dash` (`solid`) |
| `label` | `on` (a connector or line id) | `text` or `adf`, `at` (0.5), `color` (`default-text`), `font_scale` |
| `line` | `x1`, `y1`, `x2`, `y2` | `route` (`straight`), caps, `color`, `stroke`, `dash` as for connectors |
| `stamp` | `sprite` and either `on` (a shape or sticky id) or `x`, `y` | `size` (60) |
| `group` | `members` (two or more box ids) | |
| `link` | `url` (http or https), `x`, `y` | `w` (280), `h` (306) |
| `raw` | `element` (an editor record) | `id` |

## Text belongs on things

Put words in the element they describe instead of floating `text` next to it:

- Shapes and stickies carry their own text, centered by default; the box grows with long sticky text but a shape does not, so size shapes for their text (check reports overflow).
- A connector carries its words in a `label` (`at` 0 is the start, 1 the end); the label moves with the line. Label every exit of a decision.
- Section titles carry the heading of an area.
- Use a `text` element only for board titles, legends, notes and paragraphs.

## Text is Atlassian Document Format

Every text-bearing element (shape, sticky, text, label) stores its content as an Atlassian Document Format (ADF) document, the same format as Confluence pages and Jira descriptions. Give either `text`, which Craft converts, or `adf` directly.

- `text`: one block per line. `# ` to `###### ` headings, `- ` or `* ` bullet items, `1. ` numbered items, an empty line for an empty paragraph, anything else a paragraph. No inline formatting.
- `adf`: `{"version":1,"type":"doc","content":[...]}`. Supported on boards: `paragraph`, `heading` (level 1 to 6 in `attrs`), `bulletList`, `orderedList`, `listItem`, `text`, and the marks `strong`, `em`, `underline`, `strike` and `link` (with an `href` attribute). The `emoji` node does not render on boards; type emoji characters into text instead. Tables, panels, media, mentions and macros are page features, not board text.
- Sizes: board title heading 1, area titles heading 2 or 3, everything else normal text at `font_scale` 1. Avoid scales under 1 for anything people must read, and keep to one or two text sizes per area.

## Connectors and lines

Connectors bind to their elements and follow them; use `line` only for a free arrow (a timeline axis, a callout). A connector cannot start or end on a connector, line, label, stamp or group.

- `from_side` and `to_side`: `auto` (the facing sides), `top`, `right`, `bottom`, `left`, or a point `{"x": 0.25, "y": 1}` in the element's unit box (0 is left or top, 1 is right or bottom). When several connectors leave or enter the same side, give each its own point (0.25, 0.5, 0.75) so they do not draw on top of each other; `whiteboard.layout` does this automatically, ordered by where the other ends are, which also avoids crossings.
- `route`: `elbow` (right angles, the editor's "dynamic"; best for flows and architecture), `straight` (short links, radial maps), `curved` (mind maps, loops back).
- `start_cap`, `end_cap`: `none`, `arrow`, `filled_arrow`, `open_arrow`, `filled_diamond`, `open_diamond`, `open_circle`, `slash`, `triple_bar`, `open_circle_cross`, `cross`, `cross_crows_foot`, `crows_foot`, `circle_crows_foot`. Arrows show direction; omit them (`none`) for associations; diamonds suit UML aggregation and composition; the crow's-foot family shows cardinality in entity diagrams; `slash` and `cross` mark cuts or exclusions.
- `stroke` `thin`, `medium`, `thick` and `dash` `solid`, `dashed`, `dotted`: thick for the main path, dashed for asynchronous, optional or planned, dotted for weak or inferred links. Say what styles mean in a legend.
- Avoid overlaps: keep 64 to 120 px between connected elements, flow in one main direction, connect facing sides, route long back-links around the outside (bottom to bottom, `curved`), and run `whiteboard.check` to find crossings and lines through elements.

## Colors

Only these names exist; the editor draws any other color as black.

| Scene name | ADS token | Use on boards |
| --- | --- | --- |
| `<hue>-subtlest` | color.background.accent.<hue>.subtlest | section backgrounds |
| `<hue>-subtle` | color.background.accent.<hue>.subtler | stickies, shape fills |
| `<hue>` (plain) | color.background.accent.<hue>.subtle | emphasised fills |
| `<hue>-bold` | color.border.accent.<hue> | lines, outlines, small marks |
| `<hue>-boldest` | color.text.accent.<hue> | colored text, labels |
| `default-text` | color.text | body text |
| `white` | elevation.surface | plain sections and fills |

Hues: gray (no `-subtlest`; use `white`), blue, teal, green, lime, yellow, orange, red, magenta, purple. Stickies take only `yellow-subtle`, `orange-subtle`, `red-subtle`, `magenta-subtle`, `purple-subtle`, `blue-subtle`, `teal-subtle`, `green-subtle`.

- Meaning follows the atlassian-design skill: green done, red blocked or risk, yellow open or in progress, blue information or the main path, purple ideas or new, gray neutral.
- Text on shapes and stickies is dark, so fills must be `-subtle` or plain; `-bold` and `-boldest` fills fail contrast (check reports it).
- Keep a board to about five hues and add a legend when color carries meaning.

## Sizes and spacing

Use the 8 px grid and the ADS spacing scale for gaps (8, 16, 24, 32, 40, 48, 64, 80).

| Element | Size |
| --- | --- |
| Sticky | 144 x 144 (default) |
| Process or step | 200 x 96 |
| Start or end | 160 x 80 |
| Decision | 160 x 160 |
| Icon shapes | 96 x 96 |
| Text block | 320 to 640 wide |
| Gap between stickies in a group | 24 |
| Gap between connected shapes | 80 to 120 |
| Section padding / title band | 40 / 64 |
| Gap between sections | 80 |

## Shapes

Basic and flowchart: `rectangle`, `sharp_rectangle`, `rounded_rectangle`, `ellipse`, `diamond`, `up_triangle`, `down_triangle`, `left_parallelogram`, `right_parallelogram`, `start_end`, `process`, `alternate_process`, `decision`, `predefined_process`, `document`, `multiple_documents`, `input_output`, `manual_input`, `manual_operation`, `preparation`, `delay`, `display`, `database`, `stored_data`, `internal_storage`, `hard_disk`, `off_page`, `connector`, `merge`, `summing_junction`, `or`, `comment_left`, `comment_right`.

Architecture icons (the label sits outside the icon): `cloud`, `server`, `compute`, `computer`, `mobile`, `browser`, `frontend`, `internet`, `database_advanced`, `archive`, `file`, `folder`, `firewall`, `lock`, `key`, `shield`, `mail`, `settings`, `switch`, `user`, `users`.

UML: `actor`, `use_case`, `classifier`, `component`, `node`, `container`, `note`, `interface2`, `provided_interface`, `required_interface`, `assembly`, `activation`, `activity`, `start`, `end`, `flow_final`, `gateway`, `horizontal_fork`, `vertical_fork`, `history_pseudostate`, `deletion`, `send_signal`, `receive_signal`, `pin`, `pin_left`, `pin_right`, `pin_filled_left`, `pin_filled_right`, `off_page_link`, `template`, `boundary_object`, `entity_object`, `control_object`. UML class and interface boxes with compartments are not supported; decode keeps them raw.

## Stamps

`thumbs-up`, `thumbs-down`, `thanks`, `clapping-hands`, `heart-eyes`, `smile`, `joy`, `crazy`, `party-hat`, `shocked`, `cool`, `star-eyes`, `ok-face`, `peace`, `up-point`, `down-point`, `left-point`, `right-point`, `fire`, `100`, `spicy`, `party`, `heart`, `question-mark`, `gold-star`, `eyes`, `plus-one`, `win`, `tool`, `hourglass`, `magnifying-glass`, `megaphone`, `bell`, `candy`, `calendar`, `idea`, `artist`, `rocket`, `success`, `note`, `info`, `question`, `decision`, `error`, `warning`. A stamp with `on` sits on the top-right corner of its shape or sticky, clear of the text, and moves with it; use stamps for votes, status and attention, not decoration. Several votes read better as one stamp plus a count in the text (for example "Flaky tests (5 votes)").

A `link` becomes a Smart Link card that the editor resolves per viewer; a Jira work item URL shows its key, summary and status.

## Recipes

- Flowchart: `layered`, direction right (or down when tall); `start_end` at both ends, `rounded_rectangle` steps, `diamond` decisions with labelled exits, `elbow` connectors.
- Architecture: icon shapes or rounded rectangles in sections per tier or trust zone (`-subtlest` backgrounds), `layered` direction right; `medium` stroke for the main request path, `dashed` for asynchronous messages, labels for protocols.
- Retrospective or brainstorm: one section per column, titled; stickies inside; `grid` with the same `columns` for every section (1 when columns hold four or fewer stickies) so sections end level; one sticky color per column; votes as a stamp plus a count in the text.
- Kanban: one section per status; `grid` with `columns` 1; color by type or owner with a legend.
- Mind map: central `ellipse` (larger, `-subtle` of the main hue), branches as `rounded_rectangle` with heading text, leaves smaller with normal text; `radial`; `curved` connectors with `end_cap: "none"`, `medium` stroke from the root and `thin` to leaves; one hue per branch, leaves in the same hue's `-subtlest` or `-subtle`.
- Org chart or breakdown: `tree`, direction down, `elbow` connectors without arrows.
- Prioritisation: four sections in a 2 x 2 grid with axis titles; place stickies by score; no connectors.
- Timeline: a `line` along the axis with `start_cap: "none"`, `diamond` milestones on it and stickies above or below; `grid` is not suitable.
- Jira dependency map: a `link` per work item, connectors for blocks and depends-on with labels, `layered` direction right.
