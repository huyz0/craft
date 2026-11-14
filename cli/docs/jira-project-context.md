# Internal Jira project details and status context

The project-context library reads selected project metadata and status groups through sealed service access and complete output budgets. Status groups use the [immutable cursor engine](provider-pagination.md) to split an unpaginated native response into bounded CLI pages. Both commands are now public with [policy setup and usage](jira-context.md); live qualification remains separate.

Implementation: [provider](../crates/craft-provider-jira/src/project_context.rs), [field normalization](../crates/craft-provider-jira/src/project_context/fields.rs), [host](../crates/craft-cli/src/project_context.rs). See [acceptance evidence](evidence/jira-project-context.md).

## Target and authority

Both operations use empty typed input and a Jira Cloud `project` target. The tenant is a Cloud UUID; the project selector is a numeric ID or case-sensitive key. The trusted caller supplies the exact target, issued grant and expected actor. Existing list-only and issue-read configuration does not authorize these operations. The [candidate integration](project-context-command.md) now supplies explicit per-project permissions through the existing project-read record. No additional private table is added.

Preparation validates target/input, fields and sufficient complete error/cache/paging output before state or auth access. Cached execution checks status cursor integrity, expiry and binding before credential acquisition. Current service access must match the tenant/grant/actor and `read:jira-work` alternative. Dynamic policy checks run before auth, native/cached access and after final observation/output/cursor work. The same invocation deadline and cancellation govern OAuth, identity and resource HTTP.

Jira still controls native project visibility. Buffered results retain their original observation, not a fresh project-specific permission proof. These reads do not grant mutation, deployment or human-review authority.

## Project get

`jira.project.get` uses `GET /rest/api/3/project/{projectIdOrKey}` without expansions or properties. The API does not provide a field mask and includes some unselected data by default. Craft's projection is local: raw bytes remain bounded, and only selected normalized metadata enters output. Lead/email, issue types, components, versions and returned links are ignored by this adapter.

Default output is project ID/key/name/type. Explicit fields include style, simplified, archived, deleted and a bounded plaintext description up to 64 KiB. Omission and explicit null remain distinct; `fields_not_returned` reports selected missing paths. Actual numeric ID and a valid key are required to qualify identity. Numeric targets must match ID exactly; key targets must match the key exactly, including case. Issue-get alias rules do not apply here.

The response retains requested target, actual project ID, `exact_id`/`exact_key` resolution, observation time and any independent credential-cache receipt. A selected project that cannot fit the complete output envelope returns `ITEM_TOO_LARGE` without a partial project or cursor. A larger budget performs a new live get.

## Grouped statuses

`jira.project.statuses` uses `GET /rest/api/3/project/{projectIdOrKey}/statuses` without query parameters or a body. The native API returns an unpaginated array grouped by issue type. Craft performs one resource read, qualifies the bounded array and stores unreturned whole groups. It does not flatten groups, invent native offsets or request a separate count.

Each output item has mandatory issue-type ID. Default metadata adds type name/subtask and status ID/name. Status description and category ID/key/name require explicit selection. Nested omitted/null values survive normalization. `fields_not_returned` can report a path missing in one or more nested entries; empty arrays and explicit null ancestors are not missing values. Status IDs must be unique within a selected group's status array, and group IDs must be unique in the native array. The same status can belong to several issue types.

Native limits are 1 MiB and at most 1,000 groups or statuses per group. The existing engine additionally limits normalized page/store capacity. Larger responses fail qualification rather than being silently truncated. Whole-group output measurement includes newline, errors, page controls and cache receipts. An oversized first group returns zero items with same-position recovery; increasing the budget resumes without refetching statuses. Changing fields requires a new traversal.

Continuation binds the exact requested selector, descriptor/schema/projection, tenant, grant and actor. Buffered replay retains observation time and page digest and can renew credentials without fetching statuses again. Consistency remains `live`. A key-bound result is historical context for that literal request. The statuses endpoint does not report its resolved numeric project ID. A consumer preparing writes must resolve current numeric identity through project get and obtain applicable fresh metadata under that target.

A status being valid for a project/type does not prove a particular issue currently offers a transition to that status. Workflow transitions require their own issue-specific read and qualification.

## Failures and remaining work

HTTP denial, malformed selected values, duplicates, unsupported continuation and physical response violations release no qualified project/page. A 404 means absent or invisible, not proof of absence. No returned URL is followed and no resource request is retried automatically. Credential-cache changes remain visible independently when a later resource, policy or budget check fails.

The [official Projects API](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-projects/) and [pinned source](../openspec/changes/build-cloud-cli/sources/jira-v3.json) supply protocol evidence. Tests use owned TLS servers and fixed synthetic credentials. [Typed candidates and explicit permissions](project-context-command.md) now connect the host to internal dispatch. Public operational configuration, independent consequential review, public activation and actual tenant behavior remain unqualified. Remaining field/workflow metadata, CRUD/recovery and Confluence workflows are still required for CLI delivery.
