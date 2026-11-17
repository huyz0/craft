# confluence-pages skill: live checks and A/B test

Date: 2026-11-17. Site: the project's free Confluence Cloud test site, through the CraftTest service account and the `cf-test` profile.

## Element checks

Three probe pages (storage format, ADF, and layout/code/task extras) were published with `confluence.page.create`, read back with `confluence.page.get` in both representations, and viewed in a browser at 1280px, 1920px and wider windows. Every snippet in `skills/confluence-pages/references` rendered as described. Findings that shaped the skill:

- Storage `note` renders as the yellow warning panel and `warning` as the red error panel; the purple note panel needs the `ac:adf-extension` form.
- A newline inside `<p>` renders as a line break.
- `status` `subtle` is ignored; panel `title` drops inline elements.
- No Mermaid renderer exists without a Marketplace app: the `mermaid` macro shows "Error loading the extension!". An attached SVG shown with `ac:image` renders well.
- An ADF `blockCard` datasource without site IDs fails to load; the `jira` macro with `jqlQuery` renders a live table.
- Page width lives in the `content-appearance-published` and `content-appearance-draft` page properties: `fixed` is a 760px column, `full-width` grows to 1800px, `max` is uncapped. A body update keeps the width.
- Tables wider than the window are scaled down proportionally, with a floor of about 80px per column. Date chips need 140px.

`confluence.page.width.set` was run live: create (no existing property) and update paths both confirmed, for `full_width`, `max` and `fixed`, and the browser showed the expected width each time.

## A/B test

Three realistic tasks (weekly status report, design doc with a diagram, incident postmortem) were each written by a fresh agent with only the craft-cloud skill (baseline) and by a fresh agent with the confluence-pages skill. Agents could not install software, use online renderers or open a browser. Each page was published, screenshotted in the browser at 1280px, and scored blind by a separate reviewer agent against an 8-criterion rubric (first screen, structure, native elements, tables, diagram, faithfulness, scannability, visual defects; 1 to 5 each). Pages were relabelled X/Y per task in mixed order.

| Round | Status report | Design doc | Postmortem | Total |
| --- | --- | --- | --- | --- |
| 1: first skill draft vs baseline | 30 vs 28 | 31 vs 32 | 32 vs 29 | 93 vs 89 |
| 2: revised skill vs the same baseline pages | 32 vs 23 | 33 vs 31 | 34 vs 27 | 99 vs 81 |

Round 1's reviewer found the skill-written pages better on tables and panel meaning but marked one down for hiding required "alternatives considered" content in an expand, which the first draft recommended, and for a small diagram. The skill was revised: expands only for secondary material, panel color by message, no 50/50 key/value tables, measured column widths for dates, mentions, smart links and code identifiers, diagram text size, and no invented facts. Round 2 used new agents with the revised skill against the unchanged baseline pages and a new reviewer.

The two reviewers scored the same baseline pages differently (89 and 81), so per-page differences of a few points are within reviewer noise. The consistent signal across both rounds is table sizing: baseline pages repeatedly split key/value and timeline tables 50/50 and wrapped dates and identifiers, while revised-skill pages did not. Remaining defects seen in round 2: a hand-written SVG label overflowing its box, which the diagram reference now addresses, and date chips wrapping at the end of long panel lines.

Caveats: three tasks and one sample per variant; agents and graders are the same model family; the baseline agents also wrote good pages, so the gains are in layout polish and convention, not in basic capability.
