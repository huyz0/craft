# Cloud CLI command catalog

Status: proposed interface, researched 9 November 2026. No commands below are implemented. **`craft` is an agent-facing cloud CLI for the AI factory**, initially Jira and Confluence, with Bitbucket, Trello, and other providers added later. The [provider architecture](cloud-cli-provider-architecture.md) owns extension boundaries; the [contract](atlassian-agent-cli-contract.md) owns JSON, auth, errors, and plan review. API source IDs refer to the [source register](../researches/atlassian-agent-cli-sources.md).

## Conventions and release boundaries

Use `craft <provider> <resource> <action>`, with global `--profile`. Resource names are singular. Every remote write requires `--apply` and an operation ID; input comes from typed flags or `--file`. Commands are noninteractive unless an auth/bootstrap command explicitly requests interaction. Tables group multiple proposed commands for readability, not as literal shell syntax.

`operation preview --command <stable-command-id> --file <input.json>` validates a typed request and returns its target, expected effects, preconditions and payload digest without product-resource writes. Default preview is local; unverified live conditions remain explicit. `--check-remote` permits reads/auth maintenance to check metadata and permissions. Apply invokes the named resource command with the same payload and digest and revalidates applicable live conditions. For example, `operation preview --command jira.issue.delete --file delete-issue.json` precedes `jira issue delete --file delete-issue.json --operation-id cleanup-01 --apply`. A preview is not a lock or a server guarantee.

Use `command find` and selected-command `schema` for progressive discovery, `input validate` for local strict checks, and compact read/receipt projections. The [agent interface requirements](agent-cli-efficiency-and-skills.md) define these contracts, output budgets and skill loading. Do not require a full catalog/schema lookup before each known read.

**Core** is the intended first general-resource release. **Extended** follows the core. **Privileged** is a separately enabled administrative capability bundle. **Conditional** requires a tenant/API/auth capability check. These are priorities, not claims of shipped support. Unsupported operations fail before sending a request. Exact schemas, granular scope sets, permissions, API exclusions, and deprecated routes must be pinned from provider specifications and verified during implementation.

The paths below are product-relative API routes. Jira/Confluence OAuth calls use the approved gateway and cloud ID from the profile. Do not copy Basic-auth examples from reference pages into the implementation.

The [pinned provider matrix](../../cli/docs/provider-contracts.md) now maps every command in the Jira/Confluence resource tables to owning work and exact native schemas/routes. It is a development inventory with all runtime availability false. Source gaps and deprecated routes remain explicit; the matrix does not establish implementation, tenant permission or live OAuth eligibility.

## Jira Cloud

### Work items, comments, dependencies, and attachments

| Proposed command suffix after `craft jira` | API mapping | Tier / behavior |
| --- | --- | --- |
| `issue get`, `issue search` | GET `/rest/api/3/issue/{key}`; GET/POST `/rest/api/3/search/jql` | Core; bounded fields and pagination. [A13, A14] |
| `issue create`, `issue update`, `issue delete` | POST `/rest/api/3/issue`; PUT/DELETE `/rest/api/3/issue/{key}` | Core; discover required fields, explicitly handle subtasks on deletion. [A14] |
| `issue assign` | PUT `/rest/api/3/issue/{key}/assignee` | Core; account ID or explicitly permitted unassigned value. [A14] |
| `transition list`, `transition apply` | GET/POST `/rest/api/3/issue/{key}/transitions` | Core; transition ID plus required fields. Do not edit `fields.status`. [A14] |
| `comment list`, `comment get`, `comment create`, `comment update`, `comment delete` | GET/POST `/rest/api/3/issue/{key}/comment`; GET/PUT/DELETE `.../comment/{id}` | Core; ADF and visibility restrictions. [A23] |
| `link list`, `link create`, `link delete` | Read issue links field; POST `/rest/api/3/issueLink`; DELETE `/rest/api/3/issueLink/{id}` | Core; configured link type/direction, linking enabled. [A24] |
| `remote-link list`, `remote-link create`, `remote-link update`, `remote-link delete` | GET/POST `/rest/api/3/issue/{key}/remotelink`; PUT/DELETE `.../remotelink/{id}` | Extended; global-ID upsert is explicit. [A25] |
| `attachment list`, `attachment get`, `attachment download` | Issue attachment field; GET `/rest/api/3/attachment/{id}`; GET `/rest/api/3/attachment/content/{id}` | Core; separate metadata and binary output. [A22] |
| `attachment upload`, `attachment delete`, `attachment settings` | POST `/rest/api/3/issue/{key}/attachments`; DELETE `/rest/api/3/attachment/{id}`; GET `/rest/api/3/attachment/meta` | Core; multipart `file`, `X-Atlassian-Token: no-check`, tenant size limits. [A22] |

General `issue create` is authorized by profile/project policy. It is available for ordinary ledger work. `plan tickets-create` additionally requires approval of the source plan revision. Projects that require approved plans for *every* creation need that rule enforced by a trusted mutation service and provider permissions; a local CLI restriction alone cannot prevent another client creating issues.

Jira has no binary attachment-update endpoint in this inspected group. A future `attachment replace` would be a composite upload followed by optional deletion, with distinct IDs and partial-failure results. Upload retries must reconcile an uncertain response. A filename match alone is insufficient evidence of the same upload. [A22]

### Discovery and project administration

| Proposed command suffix | API mapping | Tier / behavior |
| --- | --- | --- |
| `project list`, `project get`, `project statuses` | GET `/rest/api/3/project/search`; GET `/rest/api/3/project/{key}`; GET `.../{key}/statuses` | Core. [A26] |
| `field list`, `field search`, `metadata create`, `metadata edit` | GET `/rest/api/3/field`, `/field/search`; issue create/edit metadata operations | Core; metadata guides valid input. Exact metadata subroutes pinned in implementation. [A14, A29] |
| `permission check` | GET `/rest/api/3/mypermissions`; contextual POST `/rest/api/3/permissions/check` where applicable | Core; contextual preflight, not a future-access guarantee. [A30] |
| `version list`, `version get`, `version create`, `version update`, `version delete` | GET `/rest/api/3/project/{key}/version`; POST `/rest/api/3/version`; GET/PUT `/rest/api/3/version/{id}`; POST `/rest/api/3/version/{id}/removeAndSwap` | Extended; release/archive via update flags, deletion needs explicit replacement mappings. The pinned spec deprecates DELETE `/version/{id}`. [A27] |
| `component list`, `component get`, `component create`, `component update`, `component delete` | GET `/rest/api/3/project/{key}/component`; POST `/rest/api/3/component`; GET/PUT/DELETE `/rest/api/3/component/{id}` | Extended; distinguish project admin permissions. [A28] |
| `project create`, `project update`, `project delete`, `project archive`, `project restore` | POST `/rest/api/3/project`; PUT/DELETE `/project/{key}`; POST `/project/{key}/archive`, `/restore` under v3 | Privileged; asynchronous deletion variant POST `/rest/api/3/project/{key}/delete` needs operation polling. [A26] |

Custom-field administration, user/group administration, workflow editing, and permission-scheme editing are deferred. Read discovery does not imply permission to mutate administrative configuration.

## Confluence Cloud

### Pages and spaces

| Proposed command suffix after `craft confluence` | API mapping | Tier / behavior |
| --- | --- | --- |
| `page get`, `page list`, `page create`, `page update` | GET/POST `/wiki/api/v2/pages`; GET/PUT `/wiki/api/v2/pages/{id}`; GET `/wiki/api/v2/spaces/{id}/pages` | Core; expected version on updates, explicit body representation. [A15] |
| `page search` | GET `/wiki/rest/api/search` with CQL | Core; v1 adapter, permission-filtered results. [A20] |
| `page trash`, `page purge` | DELETE `/wiki/api/v2/pages/{id}`; same route with `purge=true` for trashed content | Core trash; Privileged permanent purge. No ambiguous `page delete` alias initially. [A15] |
| `page version-list`, `page version-get` | GET `/wiki/api/v2/pages/{id}/versions`; GET `/wiki/api/v2/pages/{id}/versions/{number}` | Extended; version metadata differs from historical body retrieval. [A38] |
| `page move`, `page copy`, `page copy-tree` | PUT `/wiki/rest/api/content/{id}/move/{position}/{targetId}`; POST `/content/{id}/copy`, `/content/{id}/pagehierarchy/copy` under v1 | Extended; explicit destination and before/after/append; tree operations can be asynchronous. [A37] |
| `page archive` | POST `/wiki/rest/api/content/archive` | Conditional, experimental, asynchronous. No restore promise without a verified route. [A35] |
| `space list`, `space get` | GET `/wiki/api/v2/spaces`, `/spaces/{id}` | Core; persist space key and ID distinctly. [A31] |
| `space create` | POST `/wiki/api/v2/spaces` on RBAC tenants; POST `/wiki/rest/api/space` as a separately verified legacy adapter | Core, Conditional adapter; global create-space permission. Resolve adapter before writing. [A31, A32] |
| `space update`, `space delete` | PUT/DELETE `/wiki/rest/api/space/{spaceKey}` | Privileged; update metadata, not permissions. Deletion is permanent and asynchronous. [A32] |

Native unpublished-draft discard is a separate destructive lifecycle operation and is deferred. The factory's plan draft remains a normal published review page. A page being published, archived, trashed, or carrying a badge does not establish human approval.

### Comments, attachments, and status badges

| Proposed command suffix | API mapping | Tier / behavior |
| --- | --- | --- |
| `comment list` | GET `/wiki/api/v2/pages/{id}/footer-comments` or `/inline-comments` | Core footer; Extended inline; explicit comment kind. [A21] |
| `comment replies` | GET `/wiki/api/v2/footer-comments/{id}/children` with current parent GET for page provenance | Core footer; paged direct replies, separate from roots. [A21] |
| `comment get`, `comment create`, `comment update`, `comment delete` | GET/PUT/DELETE `/wiki/api/v2/footer-comments/{id}`; POST `/wiki/api/v2/footer-comments`; equivalent inline routes | Core footer; Extended inline. Delete is permanent; updates versioned. Inline resolution stays deferred until schema validation. [A21] |
| `attachment list`, `attachment get` | GET `/wiki/api/v2/pages/{id}/attachments`; GET `/wiki/api/v2/attachments/{id}` | Core. [A34] |
| `attachment upload`, `attachment upsert` | POST or PUT `/wiki/rest/api/content/{pageId}/child/attachment` respectively | Core upload; Extended explicit filename-based upsert. [A33] |
| `attachment replace`, `attachment update` | POST `/wiki/rest/api/content/{pageId}/child/attachment/{id}/data`; PUT same path without `/data` | Core; replacement binary by ID versus metadata edit. [A33] |
| `attachment download` | GET `/wiki/rest/api/content/{pageId}/child/attachment/{id}/download` | Core; redirect handling through binary transport. [A33] |
| `attachment trash`, `attachment purge` | DELETE `/wiki/api/v2/attachments/{id}`, optional `purge=true` on trashed attachment | Core trash; Privileged purge. [A34] |
| `content-state get`, `content-state available`, `content-state set`, `content-state clear` | GET/PUT/DELETE `/wiki/rest/api/content/{id}/state`; GET `.../state/available` | Core; content badge, required lifecycle-status query on set; space settings constrain choices. [A36] |

Confluence attachment writes use v1 multipart, with documented `X-Atlassian-Token: nocheck`. Filename upsert can add a new version to an existing attachment, so the ordinary upload command must not silently select PUT. Exact collision behavior and multipart fields need sandbox verification. [A33]

### Labels, properties, and restrictions

| Proposed command suffix | API mapping | Tier / behavior |
| --- | --- | --- |
| `label list`, `label add`, `label remove` | GET `/wiki/api/v2/pages/{id}/labels`; POST `/wiki/rest/api/content/{id}/label`; DELETE `.../label/{label}` | Extended; additive add, no implicit replace-all. [A39, A40] |
| `property list`, `property get`, `property create`, `property update`, `property delete` | GET/POST `/wiki/api/v2/pages/{id}/properties`; GET/PUT/DELETE `.../properties/{propertyId}` | Extended; versioned non-secret metadata. [A42] |
| `restriction get`, `restriction add`, `restriction replace`, `restriction clear` | GET/POST/PUT/DELETE `/wiki/rest/api/content/{id}/restriction` | Extended read; Privileged writes; replacement/clear can broaden access. [A41] |
| Global `operation get`, `operation wait` | Bound provider handle translated to GET `/wiki/rest/api/longtask/{id}` or Jira GET `/rest/api/3/task/{taskId}` when returned | Core; accepted is not completed. These commands are outside the provider namespace. [A43, A26] |

Do not add a generic space-permission command until RBAC versus legacy permission APIs, applicable scopes, and tenant behavior are researched. Page restrictions and space permissions are separate resources.

## Scopes and permission bundles

This is a routing guide, not a copy-ready exhaustive consent manifest. Read each selected operation's current scope list and permission requirements before release. A granular scope list can contain several scopes; a product-wide wildcard is not a valid substitute.

| Bundle | Documented scope examples | Additional checks |
| --- | --- | --- |
| Jira reader / worker | Classic `read:jira-work`, `write:jira-work` for many issue operations | Browse/create/edit/delete/transition and comment/attachment permissions, issue security, app access rules. [A14, A22, A23] |
| Jira project admin | Classic `manage:jira-project` for project-version writes | Project administration, operation-specific granular scopes; not covered by worker write scope. [A27] |
| Confluence pages | `read:page:confluence`, `write:page:confluence`, `delete:page:confluence` | View/add/edit/delete and stronger purge permission; version preconditions. [A15] |
| Confluence comments | `read:comment:confluence`, `write:comment:confluence`, `delete:comment:confluence` | Comment ownership/admin permission where applicable. [A21] |
| Confluence attachments | Read/delete attachment scopes in v2; classic `write:confluence-file` or the documented multi-scope granular combination for v1 upload | Space/content permission, size, collision and version rules. [A33, A34] |
| Confluence spaces | `read:space:confluence`, `write:space:confluence`; v1 update/delete have their own multi-scope requirements | Global create permission, tenant RBAC, space administration; verify chosen route. [A31, A32] |
| Confluence content badge | Classic `write:confluence-content` or documented granular combination | Content edit access and permitted/custom states; not review approval. [A36] |

Profiles should request only enabled bundles. `capabilities` reports missing scope separately from denied provider permission or local policy. A preflight result is advisory until the provider accepts the actual request.

## Binary and destructive operation contract

Uploads accept explicit local paths or a bounded input stream. Record size, digest, media type, target, and operation ID. Downloads require an explicit output path, stream into an atomic temporary file, bound total bytes, and report digest and final path in JSON. Existing files need an explicit overwrite option. Never mix binary bytes into the JSON stdout envelope.

Do not forward provider credentials to an attachment redirect host. Validate destinations and follow only the provider's documented signed-download flow without exposing signed URLs in logs. If the endpoint needs a different authenticated download strategy, implement that explicitly. Sanitize suggested filenames; never unpack or execute downloads automatically.

Destructive previews identify exact target, effect (`trash`, `permanent`, or provider-specific), dependencies, and whether the API returns an async job. Permanent operations require a payload field explicitly acknowledging their effect, in addition to `--apply`. This is machine-verifiable input, not a new conversational confirmation on each command. No automatic child deletion, purge, permission broadening, or compensation after a partial workflow.

An asynchronous submission returns `state: accepted` and a durable operation handle. `operation wait` is bounded by `--timeout`; it returns pending or completed explicitly. A zero exit for successful submission does not mean the remote job completed. Unknown submission outcomes use the existing unknown-write error and reconciliation rules.

## Later providers

| Provider | Candidate command families | Evidence and remaining work |
| --- | --- | --- |
| Bitbucket Cloud | `repository`, `branch`, `pr`, `pr-comment`, `pr-task`, `pipeline`, `commit-status`, `artifact` | API base `/2.0`; PR review, approval, diff, merge and task resources documented. Auth, scopes, protected branches, pipeline secrets and endpoint coverage need their own catalog. [A44, A45] |
| Trello | `workspace`, `board`, `list`, `card`, `comment`, `attachment`, `checklist`, `label`, `member` | Card CRUD, move through `idList`, archive through `closed`; do not translate Jira transitions automatically. Verify OAuth2 support and permission requirements per operation. [A47] |
| Other cloud systems | Provider-native nouns plus shared auth/output/operations | Require a researched API/auth contract and provider conformance evidence before advertising support. No unnamed universal API adapter. |

Factory workflows can later target a configured `WorkLedger` or `DocumentStore` interface, but only for proven common operations. Provider commands remain available for native features. The CLI is an integration plane for coding agents and schedulers; it does not itself schedule autonomous work or grant production deployment authority.
