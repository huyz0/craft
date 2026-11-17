---
name: confluence-pages
description: Write well-formatted Confluence Cloud pages in storage format or ADF and publish them with the Craft CLI. Use when creating or rewriting a Confluence page, plan, design doc, status report, runbook or meeting notes, or when a page needs panels, status lozenges, Jira issue links, @mentions, dates, expands, sized tables, diagrams or a page width.
---

# Confluence pages

This skill is about what goes inside a page body. Publishing mechanics (profiles, policies, `apply`, receipts, revisions) are in the `craft-cloud` skill's [writes](../craft-cloud/references/writes.md) reference. Colors, spacing, typography and design tokens belong to the [atlassian-design](../atlassian-design/SKILL.md) skill; read its Color section before choosing a custom panel, highlight or text color instead of inventing values.

Everything here was checked on a live Confluence Cloud site: published through Craft, read back, and viewed in a browser. Where something did not work, the reference says so.

## Workflow

1. **Plan the reader's path.** Decide the one thing a reader must learn in the first screen. Put the outcome, decision or status there, usually as a short summary paragraph or an info panel. Put supporting detail below it, and rarely-needed detail in expands.
2. **Pick the format.** Default to storage format (XHTML with `ac:` macros): it is compact, easy to template and every element below has a verified snippet. Use ADF (`atlas_doc_format`) when you already hold ADF (for example from Jira), or need a node that only ADF expresses cleanly. Don't mix: one body has one representation.
3. **Write the body** using the snippets in [storage format](references/storage-format.md) or [ADF](references/adf.md). Size tables with [tables](references/tables.md). For diagrams, follow [diagrams](references/diagrams.md).
4. **Validate and publish** with `confluence.page.create` or `confluence.page.update`. The body is `{"representation": "storage" | "atlas_doc_format", "value": "<string>"}`. For ADF the value is the JSON document serialized as a string.
5. **Set the page width** when the content needs it (wide tables, side-by-side layouts) with `confluence.page.width.set`. See [page width](#page-width).
6. **Verify.** Read the page back and check it as described in [verification](references/verification.md). Fix and update rather than leaving a broken element on a shared page.

## Structure rules

- The title is the page's only H1-sized text. Start body sections at `<h2>`, use `<h3>` for subsections, and avoid `<h4>` and below. Never skip a level.
- Write headings that say what the section concludes or contains ("Rollout plan", "Why we chose Postgres"), not generic labels ("Details", "Misc").
- Add a table of contents (`toc` macro) when the page has four or more H2 sections. Put it after the summary, not above it.
- Keep paragraphs short. Prefer a bulleted list for three or more parallel items and a table when items share the same attributes.
- One panel per message. Panels lose their force when stacked; three panels in a row is a sign the content should be a list or a table.
- Use status lozenges for states (DONE, BLOCKED, AT RISK), not for decoration. Keep lozenge text to one to three words; Confluence uppercases it.
- Link Jira issues as smart links, not bare keys, so readers see live status.
- Mention people with real @mentions (account IDs), never by typing "@name". A mention notifies that person when the page is published, so only mention people who should be notified.
- Use date elements for deadlines and dates people act on, so they render in each reader's locale.
- Hide long logs, raw data, alternatives considered and appendix material in expands. Never hide the decision, the risk or the ask.

## Element chooser

| Need | Storage element | ADF node |
| --- | --- | --- |
| Neutral context, summary | `info` macro (blue) | `panel` `panelType: info` |
| Caution, something to watch | `note` macro (yellow) | `panel` `panelType: warning` |
| Error, blocker, do-not | `warning` macro (red) | `panel` `panelType: error` |
| Success, recommendation, tip | `tip` macro (green) | `panel` `panelType: success` |
| Side note (purple) | `ac:adf-extension` panel `note` | `panel` `panelType: note` |
| Custom color or emoji panel | `panel` macro with `bgColor`, `panelIcon` | `panel` `panelType: custom` |
| State label | `status` macro | `status` node |
| Calendar date | `<time datetime="YYYY-MM-DD" />` | `date` node (UTC midnight epoch ms, as a string) |
| Person | `<ac:link><ri:user ri:account-id="…" /></ac:link>` | `mention` node |
| One Jira issue, inline | `jira` macro with `key`, or `<a … data-card-appearance="inline">` | `inlineCard` with the issue URL |
| Jira issue list | `jira` macro with `jqlQuery`, `columns`, `maximumIssues` | `extension` `jira` (same parameters) |
| Link preview card | `<a href=… data-card-appearance="block">` | `blockCard` |
| Collapsible section | `expand` macro | `expand` (and `nestedExpand` inside it) |
| Code | `code` macro with `language` | `codeBlock` with `language` |
| Columns | `ac:layout` sections | `layoutSection` / `layoutColumn` |
| Checklist | `ac:task-list` | `taskList` / `taskItem` |
| Decision log | `ac:adf-extension` decision list | `decisionList` / `decisionItem` |
| Diagram | attached image with `ac:image` (see [diagrams](references/diagrams.md)) | `mediaSingle` needs a media ID, so prefer storage |

The panel names are a trap. In storage format the `note` macro is the **yellow warning** panel and the `warning` macro is the **red error** panel. Choose by meaning from this table, not by macro name.

## Page width

Confluence stores a page's width in two page properties, `content-appearance-published` (what readers see) and `content-appearance-draft` (what the editor opens with). `confluence.page.width.set` writes both. Values:

| Value | Result |
| --- | --- |
| `fixed` | Narrow 760px reading column. What a page created through the API gets by default. |
| `full_width` | Content grows with the window up to 1800px. |
| `max` | Content spans the whole window with no cap. |

Use `fixed` for prose: long lines are hard to read. Use `full_width` for pages built around wide tables, roadmaps or two- and three-column layouts. Reserve `max` for dashboards and very wide tables. A body update keeps the width; you don't need to set it again after every edit.

Table width is separate from page width (see [tables](references/tables.md)). A wide table on a fixed page can break out of the column, but a page full of wide tables reads better at `full_width`.

## Pitfalls seen on the live site

- **Line breaks inside a paragraph are kept.** A newline between two inline elements inside `<p>` renders as a line break, so lozenges written one per source line stack vertically. Keep each paragraph on one source line and separate inline elements with a space.
- `status` `subtle=true` is ignored by the current editor; every lozenge renders filled.
- Confluence's `code` macro has no Mermaid language and there is no built-in Mermaid renderer. A `mermaid` macro without a Mermaid app installed shows "Error loading the extension!". See [diagrams](references/diagrams.md).
- An `embedCard` (or `data-card-appearance="embed"`) of a Jira issue embeds the whole issue view, which is tall and distracting. Use `block` or `inline`.
- An ADF `blockCard` with a `datasource` (Jira list view) needs the site's cloud ID and a valid datasource ID; with placeholders it renders "We ran into an issue trying to fetch results". Use the `jira` macro with `jqlQuery` for issue tables instead.
- Read-back is normalized: Confluence adds `ac:macro-id`, `ac:schema-version`, `containerLevel` parameters and may wrap the body in `ac:layout`. Don't diff read-back against what you sent; check that each element survived.
- ADF `date` timestamps are milliseconds since the epoch at UTC midnight. A local-midnight timestamp can show the previous day.
- An ADF Jira `inlineCard` is stored as a `jira` macro; both render the same live issue chip.

## Learn from existing pages

When a site has a macro or app you don't know (a Mermaid app, a roadmap, a page-properties report), find a page that already uses it and read its body with `craft confluence page get --fields id,title,body_storage`. Copy the markup shape, not the content. This is also the fastest way to discover which Marketplace apps a site has.
