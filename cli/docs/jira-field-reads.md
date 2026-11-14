# Internal Jira field discovery

Craft composes field list/search through exact site/grant/actor policy, verified service access and immutable buffered paging. [Typed adapters and private permission records](field-read-command.md) connect this host to stored configuration. Commands and policy setup are now public; see [current usage](jira-context.md). Live qualification remains separate.

## Native views and input

| Operation | Native read | Coverage and bounds |
| --- | --- | --- |
| `jira.field.list` | GET `/rest/api/3/field` | Visibility/configuration-dependent fields. Empty argument object; one unpaginated response becomes a terminal native page, with local continuation for unreturned records. |
| `jira.field.search` | GET `/rest/api/3/field/search` | Classic-project fields. Typed filters, native offset pagination and at most one resource page fetched per invocation. |

The [official field reference](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-fields/) and pinned `openspec/changes/build-cloud-cli/sources/jira-v3.json` control native interpretation. List's anonymous availability does not bypass Craft's verified service access. Both descriptors require the documented current `read:jira-work` scope. Native access denial remains a provider result; search does not require an invented blanket Administer Jira permission.

Search arguments are closed: `ids` (up to 100 unique canonical `customfield_` IDs with positive int64 suffixes), `types` (unique `custom`/`system` values), `project_ids` (up to 100 unique positive int64 integers), optional literal `query` (1 to 256 characters, at most 1,024 UTF-8 bytes), `order_by` (the native name/count/last-used variants) and `page_size` (1 to 100, default 50). Filter order is preserved and bound; it is not silently sorted. An omitted array uses no filter, while duplicates, unknown input, floating-point IDs and unsupported ordering fail before auth. Arrays use independent repeated query pairs. Raw expansion and paging-offset arguments are unavailable.

## Selection and metadata

Separate closed response models expose each view's supported fields. List defaults to ID, name, custom flag and schema type; search defaults to ID, name and schema type. Identity and `fields_not_returned` remain available. Explicit null differs from omission; selected absent paths are reported. Unselected metadata is not copied into output or cursor payloads.

Search derives documented key/stable-ID/last-used/count/lock/searcher expansions from field selection. It does not invent an expansion for every output property. A search record must establish native name and schema/type even when they are unselected. IDs are mandatory and unique within a native page; supplied ID filters also constrain returned identities.

Schema labels, plugin URIs, last-used labels and scope remain descriptive data. Explicit configuration selection returns a bounded JSON object (64 members and 32 KiB maximum), without resolving references or executing instructions. Arbitrary configuration descendants are not independently selectable. Discovery does not supply the context-specific validation needed to create or edit an issue.

## Continuation, authority and recovery

Search requires a valid returned offset/maximum and explicit terminal or next-link evidence. Totals are advisory. Short/empty pages can continue; ambiguous metadata does not cause a guessed offset or terminal claim. An increasing advertised offset is extracted only after route/filter validation. A provider-clamped maximum constrains link validation; subsequent requests keep the original requested size.

`JiraFieldSite` is a host-configured parser for the exact field-search site path and declarations. It preserves bound scalar/array filters while keeping authenticated requests on the original Cloud-ID gateway. Project-site parser authority is unchanged. Links supply no credential destination.

The runtime retains complete normalized records and original observation in immutable cursors. Buffered output requires current access and policy. Changed filters, projection, actor, target or site cannot resume the traversal. Native/cached failures retain independent credential-renewal receipts. Final access checks follow observation, budget and cursor persistence work before releasing output.

Existing transport limits are 1 MiB per response; cursor capacity remains 1,000 normalized records and 512 KiB per native page. Oversized complete items retain same-projection recovery cursors. Capacity failures return no partial catalog. Envelope preflight reserves paging/error/cache evidence before auth. The `coverage` value distinguishes the two native views, including on errors; traversal completion never establishes a tenant-wide catalog or write applicability.

See [acceptance evidence](evidence/jira-field-reads.md). Live provider eligibility, public candidates/configuration, consequential auth review and other-platform qualification remain unverified.
