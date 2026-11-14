# Bound Jira create metadata

The Jira library now has a candidate `jira.metadata.create` read handler, a private create-context object and typed frozen-body preparation. The main CLI does not register this provider or advertise cloud commands. These APIs are prerequisites for the full create workflow, not evidence of live Jira access or complete CRUD support.

## Read the exact context

The module declares fixed reads for project by ID, create issue types for that project, create fields for the selected issue type and conditional narrow subtask-parent qualification. A separate account helper has its own read scope. The routes and native shapes come from the [pinned Jira v3 source](../openspec/changes/build-cloud-cli/sources/jira-v3.json): `getProject`, `getCreateIssueMetaIssueTypes`, `getCreateIssueMetaIssueTypeId`, `getIssue`, `findAssignableUsers`, `FieldCreateMetadata` and `JsonTypeBean`. The [provider inventory](provider-contracts.md) retains source hashes, scopes and permission evidence; internal qualification details are in the [parent/assignee contract](jira-parent-assignee.md).

Metadata input selects a numeric issue-type ID and a page size of 1–100, default 50. The selected target is a Jira Cloud project ID. A project response must return that exact ID. The reader consumes all bounded issue-type and field pages before returning a usable context. It rejects changed totals, unexpected offsets, duplicate IDs, conflicting `fields`/`results` aliases, oversized pages/totals and empty nonterminal pages. Returned next, self and autocomplete URLs are never requested. Failure or exhaustion returns an error instead of a partial write context.

Craft's internal scan bounds are 40 pages and 2,000 items per paginated endpoint, 1 MiB per response and 8 MiB of serialized response data across the read. These are Craft limits, not claimed vendor quotas. Native `maxResults` can be smaller than requested; offsets advance by actual returned items. This internal context scan does not implement general agent-facing list cursors or snapshot isolation.

The normalized read result reports target, project key, selected type/name, subtask flag, observation time and field summaries. It always preserves `complete: true` and `mutation_authorized: false`. Default projection omits field schemas; agents can request those explicitly. Remote names and configuration remain data, not local instructions.

## Validate fields without guessing plugin semantics

The non-importable `CreateContext` retains complete normalized metadata, compiled closed field schemas and a host-supplied principal binding. JSON summaries cannot be imported as that context. The host is responsible for verifying the supplied grant and actor against its authenticated session; the provider does not authenticate that assertion or treat a successful read as complete permission proof.

The initial field families are:

| Shape | Qualification and encoding |
| --- | --- |
| Project and issue type | Derived from the exact bound IDs, never accepted as raw input overrides |
| Summary | Known system string field, nonblank and at most 255 characters |
| Description | Known system string field; literal text becomes an ADF document |
| Labels | Known system string array; bounded unique values without whitespace/control characters |
| Custom textfield/textarea | Exact Atlassian built-in plugin identifiers; textarea becomes ADF |
| Custom float | Exact built-in plugin identifier and numeric native type |
| Custom select | Closed `{"id":"…"}` values from the returned allowed IDs |
| Custom multiselect/multicheckboxes | Bounded unique arrays of those closed ID objects |
| Subtask parent | Exact qualified visible issue in the same project; closed ID object |
| Assignee | Exact active assignable-account proof plus a qualified settable user field |

Unknown plugin families, mismatched type/custom IDs, read-only fields, ambiguous allowed values and unsupported configurations remain explicit in the summary. A supported field must declare `set`. Required fields without a reported default must be supplied, including custom fields. Unsupported optional fields can be omitted; a required unsupported field without a reported default blocks preparation. Reported defaults are left to Jira rather than copied from arbitrary metadata objects.

Typed create input contains required numeric `issue_type_id`, summary, optional description/labels, explicit parent/assignee IDs and a custom-field map. Parent/assignee values require their separate positive qualification. Unknown top-level fields and native field names inside the custom map are refused. Custom fields must use actual `customfield_…` IDs and their qualified schemas. Missing, explicit null and supplied values stay distinct: the current non-null field schemas refuse explicit null rather than silently omitting it. Workflow status is not a create field override. Raw arbitrary schemas, field update operations, user objects or URLs are not an escape hatch.

## Freshness and the frozen request

The context uses the same retained clock for a 60-second freshness policy measured from the first metadata read. Both wall and monotonic clocks must stay within the bound and must not move backward. Preparation and frozen-operation verification reject a changed target, principal or prepared-operation digest. This is a bounded-age policy, not a guarantee that Jira metadata cannot change immediately afterward.

Body construction validates raw values through core's metadata-bound field contract, then converts description/known textarea strings to literal ADF. It fixes project/type IDs, refuses a body over 1 MiB and returns a private `FrozenCreate` with read-only call access. The host uses `PreparedHttpMutation::send_checked` to verify frozen operation/principal/freshness after acquiring the journal claim, immediately before polling HTTP. Failure preserves a conservative unknown marker without sending. Invocation control is checked before and after that trusted preflight. The [internal host](jira-create-host.md) now composes service access, policy and output; public activation remains unavailable. A clone of a native call does not carry independent metadata or mutation authority.

Subtask parents and exact active assignees now have [qualified context paths](jira-parent-assignee.md). Other hierarchy relationships, explicit unassignment, reporter/security/date/version fields and other plugin families remain required follow-up. There is no fallback to a raw field map, unverified parent or guessed account. See [create-result completion](jira-create-results.md), [journal-bound sends](journaled-http.md) and [earlier metadata evidence](evidence/jira-create-metadata.md).
