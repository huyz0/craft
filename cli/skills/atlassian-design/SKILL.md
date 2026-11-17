---
name: atlassian-design
description: Apply Atlassian Design System color, type, spacing and contrast rules when Craft creates Confluence or Jira content such as pages, whiteboards, tables and status markers.
---

Use these rules for anything Craft puts in front of Atlassian users, so it looks native and stays readable. Product-specific mappings live with each product's guidance (for whiteboards, [scene format](../craft-cloud/references/whiteboard-scenes.md)).

## Color

Use color to carry one meaning at a time, and always say it in words too (a label, a legend or an icon); never rely on color alone.

| Meaning | Accent hue | Typical use |
| --- | --- | --- |
| Success, done, approved | green | completed steps, good outcomes |
| Danger, blocked, failure | red | risks, defects, stops |
| Warning, in progress, needs attention | yellow (orange when yellow is taken) | open questions, decisions pending |
| Information, default emphasis | blue | the main flow, neutral highlights |
| Discovery, new, ideas | purple | proposals, experiments |
| Neutral, background, out of scope | gray | context, backlog, notes |
| Categories without meaning | teal, lime, magenta, orange | owners, teams, themes |

- Keep a view to about five hues. More than that reads as noise.
- Accent strengths go from subtlest (backgrounds of large areas) through subtler and subtle (fills of cards, stickies and shapes) to bolder (lines, borders and icons) and the text accent (colored text). Large areas get the lightest strength; small marks get the strongest.
- Body text uses the default text color (`color.text`, #172B4D). Colored text uses the matching text accent, never the subtle background strengths.
- Contrast: body text needs 4.5:1 against its background, and large text (24 px or 19 px bold) and graphical marks need 3:1. Dark text on subtlest, subtler or subtle accent backgrounds passes; on bolder accents it fails, so use white text there or a lighter fill.

## Typography

Atlassian Sans with the ADS type scale. Pick sizes by role, not by taste:

| Token | Size / line height | Role |
| --- | --- | --- |
| heading.xxlarge | 32 / 36 | page or board title, once |
| heading.xlarge | 28 / 32 | top-level areas |
| heading.large | 24 / 28 | section titles |
| heading.medium | 20 / 24 | group titles |
| heading.small | 16 / 20 | card or column titles |
| heading.xsmall | 14 / 20 | labels that need weight |
| body | 14 / 20 | default text |
| body.small | 12 / 16 | captions, metadata; never below 12 |

- One title per view, then no more than three heading levels under it.
- Sentence case for titles and labels. Keep labels to a few words.
- Emphasis with bold, not underline (underline means a link) and not all caps.

## Spacing

Lay everything out on the 8 px grid using the ADS spacing scale: 2, 4, 6, 8, 12, 16, 20, 24, 32, 40, 48, 64, 80 px (`space.025` to `space.1000`).

- Related items: 8 to 16 px apart. Items in the same group: 24 px. Separate groups: 40 to 64 px. Separate areas: 80 px or more.
- Use one gap value per row or column; uneven gaps read as meaning.
- Align edges and centers; snap positions and sizes to multiples of 8.
- Padding inside containers: 16 to 24 px for cards, 40 px for large areas.

## Content

- Lead with the outcome; one idea per card, sticky or table cell.
- Prefer plain words to jargon; spell out acronyms once.
- Legends sit near what they explain and use the same words as the colors' meanings above.
