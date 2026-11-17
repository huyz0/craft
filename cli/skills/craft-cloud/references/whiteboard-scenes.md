# Whiteboard scene format

A scene is a JSON file that `whiteboard.encode` turns into editor clipboard records and `whiteboard.decode` produces from them. Unknown fields, kinds, colors, shapes and sprites are rejected, so a scene that encodes is safe to paste.

```json
{
  "format": "craft.whiteboard.scene.v1",
  "elements": [
    {"kind": "section", "id": "flow", "x": -40, "y": -80, "w": 1180, "h": 420, "title": "Release flow", "color": "blue-subtlest"},
    {"kind": "shape", "id": "start", "shape": "start_end", "x": 0, "y": 60, "w": 180, "h": 80, "fill": "green-subtle", "text": "Start"},
    {"kind": "shape", "id": "build", "shape": "rounded_rectangle", "x": 260, "y": 50, "w": 200, "h": 100, "fill": "blue-subtle", "text": "Build\n- compile\n- test"},
    {"kind": "shape", "id": "ok", "shape": "diamond", "x": 540, "y": 20, "w": 160, "h": 160, "fill": "yellow", "text": "Green?"},
    {"kind": "sticky", "id": "ship", "x": 800, "y": -40, "color": "green-subtle", "text": "Ship it"},
    {"kind": "sticky", "id": "fix", "x": 800, "y": 170, "color": "red-subtle", "text": "Fix and retry"},
    {"kind": "connector", "from": "start", "to": "build"},
    {"kind": "connector", "from": "build", "to": "ok"},
    {"kind": "connector", "from": "ok", "to": "ship", "from_side": "top", "color": "green-bold", "stroke": "medium"},
    {"kind": "connector", "from": "ok", "to": "fix", "from_side": "bottom", "color": "red-bold", "dash": "dashed"},
    {"kind": "stamp", "sprite": "rocket", "on": "ship"},
    {"kind": "text", "x": 0, "y": 360, "w": 600, "text": "## Notes\nGreen builds ship; red builds loop back."}
  ]
}
```

## Geometry

- Units are canvas pixels at 100% zoom. `x`, `y` is the top-left corner; the y axis points down.
- Coordinates are relative. The editor drops the pasted group where the user last clicked, so only positions relative to each other matter. Start near (0, 0).
- Array order is paint order: put sections first, then shapes and stickies, then connectors and stamps.
- An element whose box lies fully inside a section belongs to it and moves with it. Keep a 40 px margin inside sections and leave the top 60 px for the title.
- Sections are the only grouping container. Confluence whiteboards have no frame element (the Miro or FigJam kind with presentation or export boundaries); frames imported from Miro become plain rectangles. When a user asks for frames, use titled sections, and use a `shape` with `filled: false` only for a visual border that should not group anything. A section cannot contain another section.
- Sizes that work well: sticky 144 x 144 (default; it grows downward with longer text), process shape 200 x 100, decision diamond 160 x 160, start/end 180 x 80, icon shapes 100 x 100, text width 300 to 600. Leave 60 to 120 px between connected elements so arrows stay readable.

## Element kinds

Every kind takes an optional `id` (letters, digits, `-`, `_`, `.`, `:`; at most 64) that connectors and stamps refer to. `text` and `adf` are exclusive.

| Kind | Required | Optional (default) |
| --- | --- | --- |
| `shape` | `shape`, `x`, `y`, `w`, `h` | `fill` (`gray-subtle`), `filled` (true; false draws the outline only), `text` or `adf`, `align` (`center`), `valign` (`middle`), `font_scale` (1), `rotation` (0) |
| `sticky` | `x`, `y` | `w`, `h` (144), `color` (`yellow-subtle`), `text` or `adf`, `align` (`center`), `valign` (`middle`), `font_scale`, `rotation` |
| `text` | `x`, `y`, `w` | `text` or `adf`, `color` (`default-text`), `align` (`left`), `font_scale`, `rotation` |
| `section` | `x`, `y`, `w`, `h` | `title`, `color` (`white`), `shadow` (false) |
| `connector` | `from`, `to` (element ids) | `from_side`, `to_side` (`auto`), `route` (`elbow`), `start_cap` (`none`), `end_cap` (`arrow`), `color` (`gray-bold`), `stroke` (`thin`), `dash` (`solid`) |
| `line` | `x1`, `y1`, `x2`, `y2` | `route` (`straight`), caps, `color`, `stroke`, `dash` as for connectors |
| `stamp` | `sprite` and either `on` (a shape or sticky id) or `x`, `y` | `size` (60) |
| `link` | `url` (http or https), `x`, `y` | `w` (280), `h` (306) |
| `raw` | `element` (an editor record) | `id` |

Connectors bind to their elements and follow them when the user moves things; use `line` only for free-standing arrows. A connector cannot start or end on a connector, line or stamp. Sides are `auto` (facing sides from the element centers), `top`, `right`, `bottom`, `left`, or a point `{"x": 0.25, "y": 1}` in the element's unit box. Routes are `straight`, `elbow` (right angles, the editor's "dynamic") and `curved`. Strokes are `thin`, `medium`, `thick`; dashes are `solid`, `dashed`, `dotted`.

Caps: `none`, `arrow`, `filled_arrow`, `open_arrow`, `filled_diamond`, `open_diamond`, `open_circle`, `slash`, `triple_bar`, `open_circle_cross`, `cross`, `cross_crows_foot`, `crows_foot`, `circle_crows_foot`. The crow's-foot family suits entity diagrams; diamonds suit UML aggregation.

A `link` becomes a Smart Link card; the editor resolves it for each viewer, so a Jira work item URL shows that item to people who can see it.

## Text

`text` is plain text, one block per line: `# ` to `###### ` headings, `- ` or `* ` bullets, `1. ` numbered items, an empty line for spacing, anything else a paragraph. Use literal emoji characters (the ADF emoji node does not render on boards). For bold, italic, underline, strike, links or colored text, give `adf` instead: an Atlassian Document Format document (`{"version":1,"type":"doc","content":[...]}`) with headings, paragraphs, bullet or ordered lists and the marks `strong`, `em`, `underline`, `strike`, `link`. Keep sticky text short; long text is better in a `text` element.

## Colors

Only these names exist. The editor draws any color outside its palette as black, which is why free RGB values are not accepted.

| Hue | `-subtlest` | `-subtle` | plain | `-bold` | `-boldest` |
| --- | --- | --- | --- | --- | --- |
| gray | | yes | yes | yes | yes |
| blue, teal, green, lime, yellow, orange, red, magenta, purple | yes | yes | yes | yes | yes |

Plus `white` and `default-text` (the dark navy body text color). For example `blue-subtle`, `yellow`, `red-bold`, `purple-boldest`, `gray-subtle`.

- Stickies take only the eight sticky colors: `yellow-subtle`, `orange-subtle`, `red-subtle`, `magenta-subtle`, `purple-subtle`, `blue-subtle`, `teal-subtle`, `green-subtle`.
- Shape fills read best at `-subtle` or plain; use `-bold` fills sparingly for emphasis.
- Section backgrounds read best at `white` or `-subtlest`.
- Text and lines read best at `default-text`, `-bold` or `-boldest`. `gray-bold` is the editor's default line color.

## Shapes

Basic and flowchart: `rectangle`, `sharp_rectangle`, `rounded_rectangle`, `ellipse`, `diamond`, `up_triangle`, `down_triangle`, `left_parallelogram`, `right_parallelogram`, `start_end`, `process`, `alternate_process`, `decision`, `predefined_process`, `document`, `multiple_documents`, `input_output`, `manual_input`, `manual_operation`, `preparation`, `delay`, `display`, `database`, `stored_data`, `internal_storage`, `hard_disk`, `off_page`, `connector`, `merge`, `summing_junction`, `or`, `comment_left`, `comment_right`.

Architecture icons (the label sits outside the icon): `cloud`, `server`, `compute`, `computer`, `mobile`, `browser`, `frontend`, `internet`, `database_advanced`, `archive`, `file`, `folder`, `firewall`, `lock`, `key`, `shield`, `mail`, `settings`, `switch`, `user`, `users`.

UML: `actor`, `use_case`, `classifier`, `component`, `node`, `container`, `note`, `interface2`, `provided_interface`, `required_interface`, `assembly`, `activation`, `activity`, `start`, `end`, `flow_final`, `gateway`, `horizontal_fork`, `vertical_fork`, `history_pseudostate`, `deletion`, `send_signal`, `receive_signal`, `pin`, `pin_left`, `pin_right`, `pin_filled_left`, `pin_filled_right`, `off_page_link`, `template`, `boundary_object`, `entity_object`, `control_object`. UML class and interface boxes with compartments are not supported; decode keeps them raw.

## Stamps

`thumbs-up`, `thumbs-down`, `thanks`, `clapping-hands`, `heart-eyes`, `smile`, `joy`, `crazy`, `party-hat`, `shocked`, `cool`, `star-eyes`, `ok-face`, `peace`, `up-point`, `down-point`, `left-point`, `right-point`, `fire`, `100`, `spicy`, `party`, `heart`, `question-mark`, `gold-star`, `eyes`, `plus-one`, `win`, `tool`, `hourglass`, `magnifying-glass`, `megaphone`, `bell`, `candy`, `calendar`, `idea`, `artist`, `rocket`, `success`, `note`, `info`, `question`, `decision`, `error`, `warning`. A stamp with `on` sits on that shape or sticky and moves with it; use them for votes, status and attention marks.

## Layout recipes

- Retrospective: three or four sections side by side (for example 420 x 900, 40 px apart) titled What went well, What didn't, Ideas, Actions; stickies in a two-column grid inside each (144 px plus 24 px gaps); one sticky color per column.
- Kanban: one section per status left to right; stickies stacked 24 px apart; color by type or owner, explained in a legend text.
- Flowchart: left to right or top to bottom on a 260 px pitch; `start_end` at both ends, `process` for steps, `decision` for branches with labeled exits (put the label in a small `text` element next to the arrow), `elbow` connectors.
- Architecture: icon shapes for actors, clients, services and stores; group tiers or trust zones in sections; `straight` or `elbow` connectors with `dashed` for asynchronous calls and `medium` for the main path.
- Mind map: central `ellipse`, branches as `rounded_rectangle` on a ring, `curved` connectors without arrow heads (`end_cap: "none"`), one hue per branch.
- Prioritization: four sections in a 2 x 2 grid with axis labels as `text` elements; stickies placed by score.
- Timeline: a long `line` with `start_cap: "none"`; milestones as `diamond` shapes on it and descriptions as stickies above or below.
- Dependency map from Jira: a `link` per work item laid out in columns by team or sprint, connectors for blocks or depends-on, and a legend.
