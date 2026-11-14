# Internal Confluence page publication

Craft now has native page-create and page-update preparation plus a journaled service host. Both are publicly registered commands; update requires the explicit editor-draft policy described below.

`craft-provider-confluence::pages::ConfluencePages` defines `confluence.page.create` and `confluence.page.update`. `craft-cli::confluence_pages::PreparedPagePublication` composes them with sealed service access, an explicitly trusted site/space/grant/actor policy, credential locking, fresh native evidence and the existing durable send journal. Separate create/update allowances are required; read policies grant neither. Typed internal commands now use persisted per-space write records; public administration and activation remain unfinished. See [page commands](confluence-page-commands.md).

## Create a normal shared plan page

The structured provider input explicitly chooses placement:

```json
{
  "space_id": "100",
  "title": "Plan draft",
  "placement": {"kind": "root"},
  "body": {"representation": "storage", "value": "<p>Review this plan together.</p>"}
}
```

Placement can be `root`, `homepage`, or `parent` with a numeric `id`. The host reads the exact current space and, where needed, its selected current parent. A parent must belong to the same space. The native POST sets `root-level` explicitly, pins `status=current` and omits private/embedded/live-document settings. Here, draft describes the plan's review state, not an unpublished Confluence editor draft.

Bodies explicitly select `storage` or `atlas_doc_format`. ADF is a JSON document encoded as the native string value; duplicate JSON fields and a missing document/version/content shape are rejected. Rich authoring and import use the existing document renderer. These low-level inputs do not automatically rewrite or merge content.

## Preserve a reviewed revision

A current page read supplies the exact selected body representation. `observe_page(...).revision()` fingerprints page ID, space ID, title, version and representation/value. Update input supplies `id`, `space_id`, this `expected` revision, and at least one of `title` or `body`. Both the published version and digest must match fresh evidence. Omitted fields preserve the observed values; null does not mean omission. A PUT sends current version plus one and never supplies space/parent/owner changes.

This prevents stale published-content overwrite. It does not establish safety around another user's unpublished editor draft: Confluence documents that publication reconciles into the draft and can overwrite divergent content. Updates therefore require the explicit `native_reconciliation_may_overwrite` editor-draft policy, which the site owner authorized on 14 November 2026, and every receipt reports `editor_draft_preservation_verified: false`. Neither a digest nor successful publication proves human approval.

## Outcomes and recovery

The host checks every needed scope before resource reads, rechecks policy around reads and after the durable send claim, and retains the original observation age with both wall and monotonic clocks (30 seconds). A clock moving backward also refuses preparation. The borrowed grant lock remains held through authentication, reads, send and completion.

Only documented HTTP 200 with qualified identity/space/current status/title/version confirms a write. Update 409 is a conflict; there is no force-overwrite retry. Lost responses, undocumented success codes, rate limits and incomplete success bodies remain unknown. A known returned page identity is retained even if the rest of success evidence is incomplete. No result grants automatic replay.

`PublicationData` retains the resource receipt, space, returned version, HTTP status/request ID/retry delay, full matching journal evidence, local persistence disposition and separate credential-cache receipt. Output capacity is checked before authentication; exceptional existing journal evidence survives through the preserve-evidence encoder. Native error text and returned URLs are not returned as diagnostics or routing authority.

Tests and delivery limits are recorded in [publication evidence](evidence/confluence-publication.md).
