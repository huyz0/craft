# Tables

Confluence keeps explicit column widths when a table has a `colgroup` (storage) or `colwidth` on every cell (ADF). Without them, every column gets an equal share, so a one-digit ID column ends up as wide as a paragraph of description. Always size columns.

## Table width

| Page width | Default table width | Notes |
| --- | --- | --- |
| `fixed` | 760 | Matches the reading column. |
| `full_width` | 760 to 1800 | Use 1200 to 1800 for tables with many columns. |
| `max` | up to 1800 | Same table widths as `full_width`; the page just has more room around it. |

Storage: `data-layout="default" data-table-width="760"`. Larger tables: `data-layout="center" data-table-width="1200"`. `data-layout="wide"` and `"full-width"` without a width also work and size to the page. On a fixed page, a wide table extends past the text column on both sides, which is fine for one table but noisy for several; switch the page to `full_width` instead.

What readers actually get depends on their window, measured live:

- When the window has room, a table renders at exactly its width (a 1200px table was 1200px on a 1920px window, on both fixed and full-width pages).
- When it doesn't, Confluence scales every column down by the same factor. On a 1280px laptop window with the sidebar open, the content area is about 865px, so a 1200px table shrank to about 730px: the 160px person column became 96px and mentions and dates wrapped onto two lines.
- Columns have a floor of about 80px whatever you specify, because of cell padding.

So: keep tables at 760 unless the content truly needs more, and when you go wider, give fixed-size columns enough slack to survive being scaled to about 60%.

ADF: table `attrs` `{"layout": "default", "width": 760}` or `{"layout": "center", "width": 1200}`.

## Sizing columns to content

Make the column widths add up to the table width. A good allocation:

1. **Fixed-size columns first.** Give columns whose content has a known short shape a fixed width:

   | Content | Width (px) |
   | --- | --- |
   | Row number, short ID, checkbox, emoji | 80 (the rendered minimum) |
   | Issue key (`PROJ-1234`) or inline Jira chip without summary | 110 to 130 |
   | Status lozenge | 120 to 150 (longest lozenge text × ~8 + 40) |
   | Date | 120 to 130 |
   | Person mention | 150 to 180 |
   | Number, money, percentage | 90 to 110 |

2. **Share the rest by text length.** For each remaining column, measure the longest cell in characters, capped at about 60 (long text wraps anyway), with a floor of the header's length. Split the remaining width in proportion to those measures, with a minimum of 120px each.
3. **Round to whole pixels** and put any rounding remainder on the widest text column so the total matches exactly.

Example on a fixed page (760px): columns ID, Summary, Owner, Status, Due. Fixed: ID 80, Owner 160, Status 130, Due 120 = 490. Summary gets 760 − 490 = 270.

If the fixed columns alone take more than about 60% of the table, the table is too dense for a fixed page: widen the table (and the page), drop a column, or move detail to a second table.

## Readability rules

- Put a header row (`<th>`) first. Keep header text short; it sets the minimum column width.
- One fact per cell. Use lozenges for state, date elements for dates, mentions for people and smart links for issues, so the table stays live and scannable.
- Left-align text and right-align numbers (`<td style="text-align: right;">`, or the `alignment` mark on paragraphs in ADF).
- Order rows by what the reader needs: by priority, by date or by status, not by creation order.
- Over about 15 rows or 6 columns, consider whether a Jira macro, an expand or a child page reads better.
- Never use a table for page layout; use layout sections.
