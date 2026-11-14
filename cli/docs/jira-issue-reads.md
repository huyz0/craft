# Internal Jira issue reads

Craft's internal issue-read library connects typed field selection to verified service access, declared HTTP routes and the [immutable cursor engine](provider-pagination.md). It is not registered in the public parser/catalog and does not enable cloud access in the executable.

Implementation: [provider](../crates/craft-provider-jira/src/issue_reads/mod.rs), [field planning](../crates/craft-provider-jira/src/issue_reads/fields.rs), [host](../crates/craft-cli/src/issue_reads.rs). See [acceptance evidence](evidence/jira-issue-reads.md) for observed checks and limits.

## Input and authority

Both reads target an exact Jira Cloud `site`, with its Cloud UUID as resource ID. A trusted caller supplies `IssueReadPolicy`, binding that site to an issued grant and expected actor. Existing project-discovery configuration does not authorize issue reads. The [candidate integration](issue-read-command.md) supplies separate persisted configuration; no public administration command is exposed.

Preparation rejects invalid targets, inputs, fields and insufficient complete error/cache-recovery output without accessing state, credentials or the network. Cached search execution also checks cursor integrity, expiry and semantic binding before credential acquisition. The host carries the invocation's cancellation/deadline through OAuth, identity and resource HTTP. Direct access and cached execution both require sealed verified access for the exact site, grant and actor, with the current `read:jira-work` scope alternative. A trusted policy callback can revoke execution before authentication, fetch or publication, including buffered output.

The host does not independently prove item-level visibility on buffered replay. Jira enforced visibility at the original fetch. A current site/actor check does not turn that historical observation into a fresh item-specific permission check.

## Selected fields

Default issue metadata contains ID/key, summary, project ID/key and status ID/name. Additional selectable values are issue type ID/name/subtask, assignee account ID/display name, updated, labels and description. The server receives explicit native field IDs derived from effective projection roots. No arbitrary native field masks, properties, expansions or plugin/custom-field escape hatch is accepted. `issue_type` maps to native `issuetype`.

Top-level issue ID is mandatory. The response preserves omitted versus explicit null values, including nested reference members. `fields_not_returned` lists selected paths the provider omitted; it does not diagnose redaction or prove absence. Only selected members are normalized. Unselected user data, malformed unselected members and returned URLs are ignored, subject to physical JSON and byte bounds on the whole response. The compiled field plan must match the prepared command and effective selection before invocation.

Description is fetched only when explicitly selected. It accepts bounded ADF document/version/content data or legacy plaintext, consistent with the pinned native examples. The rich value is opaque to child projection, limited to 256 KiB and retained as inert data. It is not rendered, executed, semantically imported or followed. ADF node validity beyond the declared document envelope is not established by these reads.

## Issue get

`jira.issue.get` uses `GET /rest/api/3/issue/{issueIdOrKey}` with explicit `fields`, `fieldsByKeys=false`, `updateHistory=false` and `failFast=true`. ID input must equal the returned ID exactly. Jira can resolve a case-insensitive or moved key without a redirect; output retains the requested identifier and reports `exact_id`, `matching_key` or `resolved_key` with the returned issue identity. Key is requested for resolution even when it is excluded from displayed fields.

Get returns one `issue`, observation time, target and any credential-cache receipt. A 404 means absent or invisible. It is not proof of absence or permission to retry a mutation. A selected issue that cannot fit the complete output budget returns `ITEM_TOO_LARGE`, no partial issue and no cursor. A larger budget performs a new live get. The host sends no automatic retry.

## Enhanced search

`jira.issue.search` uses read-effect `POST /rest/api/3/search/jql`. Input contains bounded nonblank literal JQL, native `page_size` from one through 100 (default 50), and up to fifty unique canonical positive int64 reconciliation IDs represented as input strings. The native body sends those IDs as JSON numbers, the field mask, `fieldsByKeys=false` and `includeArchivedProjects=false`. Jira validates JQL grammar and required search restrictions. Craft does not rewrite it or parse a replacement grammar.

Search continuation binds effective input defaults, native field mask, response/projection/descriptor identity and exact target/grant/actor. Output limit and byte budget may change; JQL, fields, native size and reconciliation IDs may not. The engine stores opaque native `nextPageToken` values without interpreting URLs and preserves unreturned normalized items. Each invocation fetches at most one native page. Buffered replay still performs ordinary verified-access checks and can renew credentials, while retaining the original observation and page digest.

Empty or short pages can continue. Missing/null next token is terminal unless `isLast:false` contradicts it; `isLast:true` cannot coexist with a token. Malformed tokens, duplicate IDs within a page and repeated/cyclic positions fail without a successful successor. Nonempty or malformed experimental provider warnings also fail qualification because warnings can describe truncated results. Provider warning text is not echoed. These errors do not prove the query has no matches.

Consistency is `live`. Reconciliation applies only to supplied IDs, which remain fixed across pages. There is no global snapshot or global read-after-write guarantee. The [official search consistency guide](https://developer.atlassian.com/cloud/jira/platform/search-and-reconcile/) explains this distinction. The [pinned Jira specification](../openspec/changes/build-cloud-cli/sources/jira-v3.json) supplies route, request and current scope evidence.

## Output and remaining work

Complete projected-envelope measurement includes newline, page control, errors and independent credential-cache receipts. Oversized search items return zero items and a same-position recovery cursor, allowing a larger byte budget without another issue search. Projection changes require a new traversal. A confirmed or uncertain credential-cache effect survives a later resource, scope, policy or output failure; it is not an issue mutation.

Owned-server tests exercise the actual host/runtime composition with fixed synthetic credentials. They do not establish live Atlassian eligibility, public command dispatch, tenant permissions, native credential input/provisioning, other-platform support, compliance or independent review. The [typed candidates and separate private configuration](issue-read-command.md) now connect this host to internal argument dispatch. Public operational configuration, required review and live acceptance remain unfinished. The broader Jira context and CLI delivery work remain incomplete.
