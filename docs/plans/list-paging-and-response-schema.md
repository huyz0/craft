# Lists, cursors, field selection, and response schemas

Status: proposed contract, **9 November 2026**. Applies to list/search commands in every provider. Extends the [agent interface requirements](agent-cli-efficiency-and-skills.md) and [JSON contract](atlassian-agent-cli-contract.md). Source IDs refer to the [research register](../researches/agent-cli-and-skills-research.md). No runtime behavior is implemented or tested.

## List behavior

Default to one bounded result slice: at most **20 items and 16 KiB** of serialized output, using the compact command-specific projection. These are initial product defaults to measure. `--limit` means the maximum items returned by this invocation, not the total collection size or a promise to fill a page. Start with a documented valid range of 1–100 and reject invalid values rather than silently changing them. Upstream providers can impose lower limits.

Normal list commands make at most one upstream page fetch per call. Returning buffered items can require none. A short or empty result can still have continuation, especially with provider filtering or local projection/filtering. Never infer exhaustion from `returned < limit`. Return `page.has_more` and `page.next_cursor` explicitly. Do not fetch a count or every page merely to construct a result.

Proposed example, with a placeholder space ID:

```text
craft --profile factory confluence page list --space-id SPACE_ID --limit 20 --fields id,title,version.number
craft --profile factory confluence page list --space-id SPACE_ID --limit 20 --fields id,title,version.number --cursor CURSOR_FROM_RESULT
```

Illustrative envelope with one item because of this example's byte budget, not a real provider response:

```json
{
  "schema_version": "1.0",
  "ok": true,
  "command": "confluence.page.list",
  "request_id": "req-example",
  "data": {
    "items": [{"id": "10001", "title": "Invoice design", "version": {"number": 7}}]
  },
  "page": {
    "returned": 1,
    "has_more": true,
    "next_cursor": "cur_EXAMPLE",
    "truncated": true,
    "stop_reason": "byte_limit",
    "consistency": "live"
  },
  "warnings": []
}
```

`has_more: true` means continuation is available, possibly through more empty provider pages. It does not guarantee another matching item exists. `has_more: false` with `next_cursor: null` means this traversal is exhausted. `truncated` indicates output stopped because of its byte/content budget; ordinary page boundaries alone do not imply truncation. `stop_reason` distinguishes `end`, `provider_page`, `item_limit`, and `byte_limit`. Errors must not masquerade as successful exhaustion.

Total counts are optional and absent by default. If requested and supported, label them exact or estimated, with observation time where available. Do not fabricate page numbers or counts for forward-only collections. A list is the records visible to the selected actor and query, not proof of everything that exists.

## Cursor contract

Use **opaque continuation cursors** as the public interface. The provider adapter translates native token, next-link, offset, or keyset mechanisms. Do not make agents calculate offsets or construct a provider's next URL. Google's pagination guidance, Jira enhanced search, Confluence v2 and Bitbucket each establish relevant but different contracts. [E15–E18]

For token efficiency, prefer short random local handles that refer to bounded continuation state in the protected local cache. The cache holds native continuation, request fingerprint, buffered remainder and expiry. Cursors contain no OAuth credentials and confer no authorization. Cache contents can include sensitive query/results and need restricted permissions, bounded retention and cleanup. A handle is an operational reference, not an independent security boundary against the same OS user.

Bind continuation to command/provider version, deployment/tenant, effective identity/grant, target, filters/query, sort, field projection and expansions. Reject changed bindings with `CURSOR_MISMATCH`. Token refresh under the same grant/identity should not break it. Local output budgets such as `--limit` and `--max-bytes` may change; preserve any provider-bound page-size/query settings internally. A cursor created by another machine/cache is not automatically portable and returns `CURSOR_NOT_FOUND`. Shared workers need an explicitly supported shared continuation store or a fresh traversal.

On resume, use the bound profile/identity and ordinary credential/policy checks. Buffered data retains its fetch provenance and may reflect earlier visibility; a cursor does not re-establish current per-object cloud permissions or a fresh snapshot. Do not advertise offline cached continuation as a live authorized provider read. Distinguish local cached results where relevant to the task.

Expired or invalid native continuation returns `CURSOR_EXPIRED` or `CURSOR_INVALID`, with a restart instruction. Do not silently restart at the first page. Use `expires_at` only when the CLI knows its own/provider expiry; do not invent the provider token lifetime. Never parse, modify, or accept an arbitrary URL as a CLI cursor. Provider next links must remain within validated provider/tenant routes before authenticated transport uses them.

Keep each cursor's logical position immutable. Retrying a read cursor must not advance a shared mutable pointer and skip items. Read retries remain read-only, but live collections can change and an upstream response is not necessarily reproducible. Bound any cached page/buffer and reject a corrupted or incompatible cache entry rather than guessing a position.

### Do not lose records when bounding output

If an upstream page has 50 records and only 20 fit, the continuation must first return the remaining 30. Returning only the provider's next-page token would silently skip them. Persist the fetched remainder and its following native continuation, then advance only past records actually emitted. Store this state before returning its handle; a cache-write failure must not return an unusable success cursor.

The same rule applies when bytes, local filtering, or provider minimum page sizes differ from CLI limits. Never cut serialized JSON. If one required item cannot fit, return `ITEM_TOO_LARGE` with a way to select fewer fields or use an artifact, preserving its position. Do not skip it to appear to make progress. Keep cursor/cache sizes bounded; large exports use an artifact workflow.

## Ordering, changing collections, and complete scans

Use a deterministic provider-supported sort with a unique tie-breaker where available. Declare unsupported sorting instead of sorting each page independently and presenting that as a globally sorted collection. Prefer immutable order keys for scans. Cursor support alone does not guarantee snapshot isolation: edits, inserts, deletions and access changes can cause duplicates or gaps. Expose `consistency: live` unless an actual provider snapshot is established. Deduplicating IDs can remove repeats but cannot recover missing records.

Export/scan mode is explicit and writes to `--out`; do not add unbounded `--all` output into the model context. It must specify total item/page/byte/time budgets, return counts/checkpoint/artifact digest, and distinguish `complete` from `budget_reached`, `rate_limited`, `cancelled`, and failed/unknown state. On 429, preserve the checkpoint and report retry timing. Detect repeated/no-progress cursors and stop with `PAGINATION_STALLED` instead of looping. Do not invent offsets after cursor expiry.

For destructive bulk work, first materialize and review a bounded immutable target manifest, then revalidate each target/version/permission at apply. Do not mutate a live list while using its changing membership as the authoritative deletion/transition set. Full traversal still does not establish a transaction or a globally consistent snapshot.

## Fetching only required fields

`--fields` is a validated projection of the command's **response item schema**, separate from mutation input fields and provider query syntax. Use documented dot paths such as `id,title,version.number`. No arbitrary jq, template, shell expression, implicit relationship expansion, or recursive wildcard in the initial interface. Reject unknown or unsupported paths with `FIELD_UNKNOWN` and a selected response-schema hint.

The command schema marks fields as `always_returned`, `selectable`, and `default_selected`. List identity and mandatory envelope/paging information remain available even if the caller does not request them. Selecting a child does not implicitly select all siblings. Cap projection depth/count and explicitly define arrays as element-wise projection in the schema. Raw/native provider fields are selectable only where declared, not by arbitrary pass-through strings.

Push projection to the provider when supported. Jira enhanced search accepts `fields`; Bitbucket supports partial responses. [E17, E18] Translate CLI paths through the adapter rather than assuming every provider uses the same names. For endpoints without partial responses, fetch the bounded provider result and project locally. This still saves model-context tokens, but not necessarily provider bandwidth or rate cost. Command descriptors expose server/local/mixed projection behavior.

Always include provider fields needed internally for IDs, ordering, versions and continuation, even if they are not emitted as item data. In particular, a server-side field mask must not discard `next` or other pagination evidence. Internal required fields do not authorize retrieving unrelated bodies/history. Report unsupported field combinations, rather than ignoring a selection or silently expanding to a complete object.

**Omitted is not null.** An unselected property is absent. A selected property with a real null value is `null` only if allowed by its schema. A genuinely missing optional value can remain absent. Permission-redacted or unavailable selected data must be identified through a structured warning/availability indication where the provider establishes the reason. Never fabricate `null` or an empty array to mean “not fetched.” Do not claim a cause the provider does not reveal.

## Response-schema discovery

Proposed forms:

```text
craft schema --command confluence.page.list --response
craft schema --command confluence.page.list --response --path /properties/data/properties/items/items/properties/version
craft schema --command confluence.page.list --response --fields id,title,version.number --view full
craft schema --command confluence.page.list --response --example minimal
```

The default `schema --command ...` describes inputs. `--response` selects outputs. Output schema includes the envelope, projected item types, optional/nullable distinctions, pagination, warnings, error variants and guaranteed mutation receipts. Compact view exposes field names/types/default selection and its incompleteness. Full view returns valid self-contained JSON Schema, including resolved or bundled definitions, or a complete artifact reference if too large. Schema pointers refer to the actual full document, not an invented parallel path syntax.

`--response --fields ... --view full` describes the selected response, retaining mandatory fields and error/control variants. It must not require every selectable field. Include command/contract/provider version and schema digest so agents cache it. Schema discovery is local and does not fetch real resource data. Live custom-field schemas are a separately declared metadata operation, and selected instances can still contain absent or permission-limited values.

Generate schemas, projection validation and serializers from one maintained contract. Preserve schema version/digest in a stable way; don't inject entire schemas or default object examples into each list result. Use the normal request ID for diagnostics. The agent reads the selected schema once and actual values afterward.

## Companion skill instructions and acceptance checks

Add a conditional `references/lists.md` to the proposed skill, directly linked from its entrypoint. It teaches: filter first; select fields needed for the next decision; reuse response schemas; follow returned cursors unchanged; continue only as far as needed; do not interpret an empty page as end; use an export artifact for full scans; retain complete evidence for approvals. Load this reference for multi-page/large-result work, not every single-item get.

Required fixtures: empty page with continuation; short nonterminal page; exact terminal page; byte-limited mid-page continuation without loss; a single oversized item; changed target/query/fields; invalid/expired/foreign-machine cursor; replay; repeated upstream cursor; concurrent inserts/deletes; count unavailable; field masks retaining native continuation; missing versus null; invalid nested/array projection; server versus local projection; cached result provenance; resumed 429/cancelled export. Verify remote calls, emitted IDs, final exhaustion, valid response schemas and byte limits. Measure tokens/calls for several field selections and page sizes. None of these runtime fixtures has been executed in this research.
