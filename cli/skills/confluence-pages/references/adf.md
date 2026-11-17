# ADF snippets

Atlassian Document Format is JSON. The page body is `{"representation": "atlas_doc_format", "value": "<the document serialized as a JSON string>"}`. Craft rejects a value that is not an object with `"type": "doc"`, `"version": 1` and a `content` array.

Build ADF with a script or a JSON library, never by hand-concatenating strings: every text node is `{"type": "text", "text": "…"}` and escaping mistakes break the whole body. Every snippet below rendered on a live site.

## Skeleton

```json
{"type": "doc", "version": 1, "content": [
  {"type": "paragraph", "content": [{"type": "text", "text": "Summary."}]},
  {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "Context"}]}
]}
```

An empty paragraph is `{"type": "paragraph"}` with no `content`. Text nodes must not be empty strings.

## Marks

```json
{"type": "text", "text": "bold", "marks": [{"type": "strong"}]}
{"type": "text", "text": "link", "marks": [{"type": "link", "attrs": {"href": "https://…"}}]}
{"type": "text", "text": "red", "marks": [{"type": "textColor", "attrs": {"color": "#bf2600"}}]}
{"type": "text", "text": "highlight", "marks": [{"type": "backgroundColor", "attrs": {"color": "#fedec8"}}]}
```

Others: `em`, `underline`, `strike`, `code`, `subsup` (`{"type": "sub"}` / `sup`). `code` can't combine with other marks.

## Panels

```json
{"type": "panel", "attrs": {"panelType": "info"}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "…"}]}]}
```

`panelType`: `info` (blue), `note` (purple), `warning` (yellow), `error` (red), `success` (green), or `custom` with `panelColor` (hex), `panelIcon` (`:shortname:`) and `panelIconText` (the emoji character).

## Inline nodes

```json
{"type": "status", "attrs": {"text": "IN PROGRESS", "color": "blue"}}
{"type": "date", "attrs": {"timestamp": "1796083200000"}}
{"type": "mention", "attrs": {"id": "ACCOUNT_ID"}}
{"type": "emoji", "attrs": {"shortName": ":white_check_mark:"}}
{"type": "inlineCard", "attrs": {"url": "SITE/browse/PROJ-1"}}
{"type": "hardBreak"}
```

Status `color`: `neutral`, `blue`, `green`, `yellow`, `red`, `purple`. Date `timestamp` is a string of milliseconds at UTC midnight of the date (compute it, for example `calendar.timegm(date.timetuple()) * 1000`).

## Cards

```json
{"type": "blockCard", "attrs": {"url": "SITE/browse/PROJ-2"}}
{"type": "embedCard", "attrs": {"url": "https://…", "layout": "center", "width": 100}}
```

For a Jira issue table use the `jira` macro as an extension rather than a `blockCard` with `datasource`, which needs site-specific IDs:

```json
{"type": "extension", "attrs": {"extensionType": "com.atlassian.confluence.macro.core", "extensionKey": "jira",
  "parameters": {"macroParams": {"jqlQuery": {"value": "project = PROJ ORDER BY created DESC"}, "columns": {"value": "key,summary,status,assignee"}, "maximumIssues": {"value": "20"}}}}}
```

## Macros as extensions

Any storage macro maps to an `extension` (block) or `inlineExtension` (inline) with `extensionType` `com.atlassian.confluence.macro.core`, `extensionKey` the macro name and each parameter as `{"value": "…"}` in `macroParams`. Table of contents:

```json
{"type": "extension", "attrs": {"extensionType": "com.atlassian.confluence.macro.core", "extensionKey": "toc", "parameters": {"macroParams": {"maxLevel": {"value": "3"}}}}}
```

## Expand

```json
{"type": "expand", "attrs": {"title": "Alternatives considered"}, "content": [ … ]}
```

Inside an `expand`, a nested collapsible must be `nestedExpand`, not `expand`.

## Code

```json
{"type": "codeBlock", "attrs": {"language": "python"}, "content": [{"type": "text", "text": "print('hi')"}]}
```

## Tables

```json
{"type": "table", "attrs": {"layout": "default", "width": 760, "isNumberColumnEnabled": false}, "content": [
  {"type": "tableRow", "content": [
    {"type": "tableHeader", "attrs": {"colwidth": [90]}, "content": [{"type": "paragraph", "content": [{"type": "text", "text": "ID"}]}]},
    {"type": "tableHeader", "attrs": {"colwidth": [470]}, "content": [ … ]},
    {"type": "tableHeader", "attrs": {"colwidth": [200]}, "content": [ … ]}
  ]}
]}
```

Every cell repeats its column's width in `colwidth` (one number, or one per spanned column with `colspan`). Table `layout`: `default`, `center` (use with `width`), `wide`, `full-width`, `align-start`. See [tables](tables.md).

## Layouts, tasks, decisions

```json
{"type": "layoutSection", "content": [
  {"type": "layoutColumn", "attrs": {"width": 50}, "content": [ … ]},
  {"type": "layoutColumn", "attrs": {"width": 50}, "content": [ … ]}
]}
{"type": "taskList", "attrs": {"localId": "tasks-1"}, "content": [
  {"type": "taskItem", "attrs": {"localId": "task-1", "state": "TODO"}, "content": [{"type": "text", "text": "Open task"}]}
]}
{"type": "decisionList", "attrs": {"localId": "dec-1"}, "content": [
  {"type": "decisionItem", "attrs": {"localId": "dec-1-1", "state": "DECIDED"}, "content": [{"type": "text", "text": "We use Rust."}]}
]}
```

Layout column widths add up to 100. Task `state` is `TODO` or `DONE`. Task and decision items hold inline content directly, not paragraphs. `localId` values must be unique in the document.

## Images

ADF `mediaSingle`/`media` needs the attachment's media file ID, which comes from the upload response and the media service. For diagrams and images, publish the body in storage format with `ac:image` instead (see [storage format](storage-format.md#images)).
