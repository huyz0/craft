# Storage format snippets

Every snippet below rendered correctly on a live Confluence Cloud site. Replace `SITE` with the site origin (for example `https://example.atlassian.net`), `ACCOUNT_ID` with an Atlassian account ID and `PROJ-1` with a real issue key. Keep each paragraph on one source line: a newline inside `<p>` becomes a visible line break.

A storage body is an XHTML fragment with no `<html>` or `<body>` wrapper. Escape `&`, `<` and `>` in text (`&amp;`, `&lt;`, `&gt;`). Code and other plain-text bodies go in `<![CDATA[…]]>`; a literal `]]>` inside code must be split across two CDATA sections.

## Headings and table of contents

```xml
<p>One-paragraph summary of what this page decides or reports.</p>
<ac:structured-macro ac:name="toc"><ac:parameter ac:name="maxLevel">3</ac:parameter></ac:structured-macro>
<h2>Context</h2>
<h3>Constraints</h3>
```

`toc` parameters that help: `maxLevel` (usually 2 or 3), `minLevel`, `exclude` (a regex of headings to skip).

## Panels

```xml
<ac:structured-macro ac:name="info"><ac:rich-text-body><p>Blue info panel.</p></ac:rich-text-body></ac:structured-macro>
<ac:structured-macro ac:name="note"><ac:rich-text-body><p>Yellow warning panel.</p></ac:rich-text-body></ac:structured-macro>
<ac:structured-macro ac:name="warning"><ac:rich-text-body><p>Red error panel.</p></ac:rich-text-body></ac:structured-macro>
<ac:structured-macro ac:name="tip"><ac:rich-text-body><p>Green success panel.</p></ac:rich-text-body></ac:structured-macro>
```

Add `<ac:parameter ac:name="title">Short title</ac:parameter>` before the body for a bold title line inside the panel.

The purple note panel, and any ADF panel type by name, uses the ADF extension form:

```xml
<ac:adf-extension><ac:adf-node type="panel"><ac:adf-attribute key="panel-type">note</ac:adf-attribute><ac:adf-content><p>Purple note panel.</p></ac:adf-content></ac:adf-node></ac:adf-extension>
```

`panel-type` accepts `info`, `note`, `warning`, `error` and `success`. Confluence converts the four classic ones back to the `info`/`note`/`warning`/`tip` macros when it stores the page.

Custom background color, with an optional emoji icon:

```xml
<ac:structured-macro ac:name="panel"><ac:parameter ac:name="panelIcon">:bulb:</ac:parameter><ac:parameter ac:name="panelIconText">💡</ac:parameter><ac:parameter ac:name="bgColor">#EAE6FF</ac:parameter><ac:rich-text-body><p>Custom panel.</p></ac:rich-text-body></ac:structured-macro>
```

Pick `bgColor` from the subtle background colors in the atlassian-design skill so text keeps its contrast.

## Status lozenges

```xml
<p>Status: <ac:structured-macro ac:name="status"><ac:parameter ac:name="colour">Green</ac:parameter><ac:parameter ac:name="title">DONE</ac:parameter></ac:structured-macro></p>
```

`colour` is `Green`, `Yellow`, `Red`, `Blue` or `Purple`; leave it out for grey. Note the British spelling. Conventional meanings: grey for not started or to do, blue for in progress or in review, green for done or approved, yellow for at risk or waiting, red for blocked or failed, purple for new or experimental. Several lozenges in one paragraph need a space between them and no newline.

## Dates

```xml
<p>Due: <time datetime="2026-12-01" /></p>
```

Renders as a date chip in the reader's locale. Use ISO `YYYY-MM-DD` only; times are not supported.

## Mentions

```xml
<p>Owner: <ac:link><ri:user ri:account-id="ACCOUNT_ID" /></ac:link></p>
```

The account ID comes from Jira or Confluence user data (for example an issue's assignee or a page's author), never from a display name. Mentions notify the person.

## Links and smart cards

```xml
<p>Plain link: <a href="SITE/browse/PROJ-1">PROJ-1</a></p>
<p>Inline smart link: <a href="SITE/browse/PROJ-1" data-card-appearance="inline">SITE/browse/PROJ-1</a></p>
<a href="SITE/browse/PROJ-2" data-card-appearance="block">SITE/browse/PROJ-2</a>
<p>Page link: <ac:link><ri:page ri:content-title="Exact page title" /></ac:link></p>
```

- An inline smart link to a Jira issue renders as the issue icon, key, summary and live status.
- A block card renders a preview card with status and an "Open preview panel" button. It works for any URL that has a preview, such as Jira issues, Confluence pages and many public sites.
- `data-card-appearance="embed"` embeds the target's full view. It is tall; use it only for things meant to be viewed in place, such as a design file.
- A page link by title resolves in the same space. Add `ri:space-key="KEY"` on `ri:page` for another space.

Jira issue macro, inline:

```xml
<p>Tracked in <ac:structured-macro ac:name="jira"><ac:parameter ac:name="key">PROJ-1</ac:parameter></ac:structured-macro></p>
```

Jira issue table from JQL:

```xml
<ac:structured-macro ac:name="jira"><ac:parameter ac:name="jqlQuery">project = PROJ AND fixVersion = "1.4" ORDER BY priority DESC</ac:parameter><ac:parameter ac:name="columns">key,summary,status,assignee</ac:parameter><ac:parameter ac:name="maximumIssues">20</ac:parameter></ac:structured-macro>
```

The table is live: it shows current status and a count with a Refresh link. `serverId` is optional on a site's own Jira; Confluence fills it in. Prefer it over a hand-written table of issues that goes stale.

## Expand

```xml
<ac:structured-macro ac:name="expand"><ac:parameter ac:name="title">Alternatives considered</ac:parameter><ac:rich-text-body><p>…</p></ac:rich-text-body></ac:structured-macro>
```

Expands can nest. The title is what readers scan, so make it say what is inside.

## Code

```xml
<ac:structured-macro ac:name="code"><ac:parameter ac:name="language">rust</ac:parameter><ac:parameter ac:name="title">main.rs</ac:parameter><ac:plain-text-body><![CDATA[fn main() { println!("hi"); }]]></ac:plain-text-body></ac:structured-macro>
```

Optional parameters: `title`, `linenumbers` (`true`), `collapse` (`true`). Unknown languages, including `mermaid`, render as plain text. For short inline code use `<code>`.

## Tables

```xml
<table data-layout="default" data-table-width="760"><colgroup><col style="width: 90px;" /><col style="width: 470px;" /><col style="width: 200px;" /></colgroup><tbody>
<tr><th><p>ID</p></th><th><p>Description</p></th><th><p>Status</p></th></tr>
<tr><td><p>1</p></td><td><p>Longest text goes in the widest column.</p></td><td><p>…</p></td></tr>
</tbody></table>
```

See [tables](tables.md) for choosing `data-layout`, `data-table-width` and column widths. Cell content should be wrapped in `<p>`. Status lozenges, dates, mentions and smart links all work inside cells.

## Layouts

```xml
<ac:layout><ac:layout-section ac:type="two_equal"><ac:layout-cell><p>Left</p></ac:layout-cell><ac:layout-cell><p>Right</p></ac:layout-cell></ac:layout-section></ac:layout>
```

Section types: `fixed-width` (one column), `two_equal`, `two_left_sidebar`, `two_right_sidebar`, `three_equal`, `three_with_sidebars`. When a body uses `ac:layout`, put all content inside layout sections; Confluence does this itself on read-back. Columns squeeze on a fixed-width page, so pair multi-column layouts with `full_width` page width.

## Tasks and decisions

```xml
<ac:task-list><ac:task><ac:task-id>1</ac:task-id><ac:task-status>incomplete</ac:task-status><ac:task-body>Draft the rollout plan <ac:link><ri:user ri:account-id="ACCOUNT_ID" /></ac:link> <time datetime="2026-12-01" /></ac:task-body></ac:task></ac:task-list>
```

`ac:task-status` is `incomplete` or `complete`. A mention and date inside a task assign it and give it a due date in Confluence's task views. Task IDs must be unique within the page.

Decision list:

```xml
<ac:adf-extension><ac:adf-node type="decision-list"><ac:adf-attribute key="local-id">dl1</ac:adf-attribute><ac:adf-node type="decision-item"><ac:adf-attribute key="local-id">d1</ac:adf-attribute><ac:adf-attribute key="state">DECIDED</ac:adf-attribute><ac:adf-content>We use Postgres for the event store.</ac:adf-content></ac:adf-node></ac:adf-node></ac:adf-extension>
```

## Images

Attached image (upload first with `confluence.attachment.upload` to the same page):

```xml
<ac:image ac:align="center" ac:layout="center" ac:width="600" ac:alt="What the image shows"><ri:attachment ri:filename="diagram.svg" /></ac:image>
```

External image: `<ac:image><ri:url ri:value="https://…/image.png" /></ac:image>`. Always set `ac:alt`.

## Text formatting

`<strong>`, `<em>`, `<u>`, `<s>`, `<code>`, `<sub>`, `<sup>`, `<blockquote>`, `<hr />`, `<ul>`/`<ol>`/`<li>`. Colored text and highlight are spans: `<span style="color: rgb(191,38,0);">` and `<span style="background-color: rgb(254,222,200);">`. Use color sparingly and only with meaning, never as the only signal.
