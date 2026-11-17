# Verifying a page

A successful create or update receipt proves Confluence accepted the body, not that it renders as intended. Unknown macros, broken mentions and malformed cards are stored without error and only fail when someone views the page.

## Read back

```sh
craft confluence page get --fields id,title,version,revision,body_storage --input page.json
```

`page.json` is `{"id": "<page id>"}`. Check, element by element, that what you sent survived:

- Each macro you used appears as `ac:structured-macro ac:name="…"` (Confluence adds `ac:macro-id` and `ac:schema-version`; that is normal).
- Mentions kept their `ri:account-id`, dates their `datetime`, cards their `data-card-appearance`.
- Tables kept `colgroup` widths (Confluence rewrites `90px` as `90.0px`).
- Nothing you sent was dropped. A dropped element usually means invalid markup.

Request `body_atlas_doc_format` instead to see the ADF form; it is also how you learn the ADF of an element you only know in storage format.

## Look at it

When a browser is available, open `SITE/wiki/spaces/<space key>/pages/<page id>` and check:

- Panels have the intended colors and icons.
- Lozenges sit inline on one line where intended.
- Smart links show issue summaries and status, not raw URLs (a raw URL in a card means the link could not be resolved for the viewer).
- No "Error loading the extension!" or "We ran into an issue" boxes.
- Tables have sensible column widths and no column squeezed to a few characters.
- Expands are collapsed and titled clearly.
- The first screen tells the reader what the page is for.

If no browser is available, say that the page was verified by read-back only.

## Fixing

Update with a fresh `revision` from `page get` and `editor_draft_policy: native_reconciliation_may_overwrite`. Send the whole corrected body; updates replace the body, they don't patch it.
