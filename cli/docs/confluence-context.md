# Confluence context reads

The Confluence provider, host and typed command/profile adapters implement internal space/page reads and page search. They are not registered in public CLI discovery. Public activation and live tenant qualification remain unfinished. These reads supply context for the plan-page workflow; they do not publish a page or establish review approval.

## Native operations

| Internal command | Compiled API route | Input and native bound |
| --- | --- | --- |
| `confluence.space.get` | GET `/wiki/api/v2/spaces/{id}` | Canonical positive numeric space ID |
| `confluence.space.list` | GET `/wiki/api/v2/spaces` | Optional exact key; native page size 1–250, default 25 |
| `confluence.page.get` | GET `/wiki/api/v2/pages/{id}` | Canonical positive page ID; current published content, no draft/history switch |
| `confluence.page.list` | GET `/wiki/api/v2/pages` or `/wiki/api/v2/spaces/{id}/pages` | Optional exact space ID and title filter; explicit lifecycle, default current; native size 1–250 |
| `confluence.page.search` | GET `/wiki/rest/api/search` | Literal bounded CQL; native size 1–100, default 25 |

Paths, query names and scope declarations are checked against the independently pinned [provider inventory](provider-contracts.md). V2 space/page reads use `read:space:confluence` or `read:page:confluence`; search uses the current documented `search:confluence` scope. The current-user probe requires its separate scope. Scopes and native visibility remain independent of trusted host enablement.

Space keys and IDs are distinct output fields. Discovery by key rejects a different returned key and duplicate identities rather than guessing a target. CQL is not rewritten. Callers must restrict searches to pages; unsupported result kinds fail the page rather than disappearing from it.

## Fields and content

Select `body_storage` or `body_atlas_doc_format` to request one primary page-body representation. Metadata defaults request neither. A page always retains `body_state`: `not_requested`, `not_returned` or `returned`. Missing selected fields are listed in `fields_not_returned`; an unavailable body is not reported as an empty complete document. A returned representation label must match the requested representation.

Space descriptions similarly use `description_plain` or `description_view`. Search expansions are derived from selected fields, including space identity, version and the chosen body format. Raw expansion strings are not accepted. Owner/user metadata, excerpts, media tokens, returned URLs and unrelated provider fields are not copied into normalized output. Selected body text remains inert native content, including markup and Unicode.

Page metadata from search does not establish that a plan is complete or reviewed. Publication and approval must separately require the exact page/version/body evidence needed by their own contracts.

## Pagination and host execution

One invocation performs at most one native resource GET. The existing runtime persists normalized remainders and opaque continuation bound to command, effective input/defaults, projection, descriptor/schema, actor, grant and exact site. A reopened buffered page revalidates access without fetching the resource again. Changed input or fields cannot reuse its cursor. Live collections are not snapshots.

The documented `_links.next` supplies continuation data. Known exact product-relative paths are reduced to query data, then validated by the host at the original gateway route. Foreign origins, changed paths/filters, duplicate parameters, nonadvancing offsets and repeated immediate positions fail. Search also accepts a documented advancing offset; it does not switch an established cursor/offset traversal to another strategy. A reported native search-size clamp constrains validation of its link, while future requests retain the original requested size. Empty/short pages may continue, and changing totals do not prove completion. Other site-absolute link forms still need explicit host/site qualification.

`PreparedConfluenceRead` accepts trusted exact target/grant/actor and explicit enabled-command authority. Pure input/field/output preflight precedes credential acquisition. The host uses sealed service access, current scope and actor verification, dynamic policy checks, bounded transport and complete-envelope fitting. Late refusal suppresses content/continuation while retaining independent credential-cache receipts. The provider cannot obtain raw credentials or select an authenticated URL. Reads do not claim absence from 404 or empty visibility-filtered results, retry automatically, publish content or obtain mutation authority.

Observed checks and remaining work are recorded in [the evidence](evidence/confluence-context.md).

## Typed command and profile integration

Internal candidates expose `space get/list` and `page get/list/search` under the Confluence action. Flags and strict JSON share typed arguments. Get requires an exact numeric `id`; list/search distinguish native `page_size` from output `limit` and accept only an opaque Craft cursor. `cloud_id` is optional context selection. Global selected fields choose content representations. Actor, grant, raw expansion and authenticated URLs are not arguments.

For example, the page-list candidate argument object is:

```json
{"space_id":"100","status":"current","page_size":25,"limit":20}
```

The candidate validates input, fields, cursor syntax and recovery-output capacity before opening private state. It uses the normal explicit/repository/active profile resolution, requires a Confluence Cloud service profile, and resolves an exact site from explicit context, hints or the profile's configured tenant. It then requires a separate enabled Confluence read record for that profile and operation. A space key or profile default cannot silently replace an explicit numeric resource ID.

Schema ten introduces an empty `confluence_read_policies` table. Format-one records bind installation, profile, grant, exact site, expected actor, enabled state, revision and the explicit closed set of five read operations. Existing Jira/read/create/recovery configuration grants no Confluence allowance, even when profiles share an Atlassian credential grant. Trusted local administration uses the runtime setter with apply, receipt preflight and exact revision comparison. An enabled identity must be disabled before actor or target transfer. This setter is internal; public administration/auth setup remains qualification work.

Execution captures and compares the whole record before auth, across native and buffered reads, and before releasing output. Revocation or a changed allowed record suppresses data and continuation while preserving newly committed credential-cache effects. Command dispatch returns the host's complete envelope and warnings, retaining the original request identity. Internal registration does not activate the public parser or provider catalog. See the [command integration evidence](evidence/confluence-command.md).
