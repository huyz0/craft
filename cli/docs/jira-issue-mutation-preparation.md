# Jira issue mutation preparation

This contract covers provider preparation for partial issue edits and workflow transitions. The [host](jira-issue-mutation-host.md) composes the frozen call with sealed access, journaled sending and completion. Commands and policy administration are now public with [local activation evidence](evidence/jira-issue-action-activation.md); live acceptance remains separate. Preparation itself performs reads only and never sends the returned call.

## Identity and freshness

The write input contains a numeric issue ID and expected `IssueState`: ID, key, project ID, issue-type ID, status ID and updated value. Fresh bracketed issue metadata must match all six values and the exact issue resource, tenant, grant and actor. Alias resolution happens during reading; mutation input and route use the resolved numeric ID. Reject generic version/digest/parent preconditions rather than imply a Jira-native compare-and-swap.

Keep unprojected native metadata and the typed observation private inside `LiveIssueContext`. Neither public selected items nor a deserialized observation can reconstruct live proof. The frozen call retains the write digest, principal and original wall/monotonic start times. Check both clocks at preparation and immediately before send; backward movement or age of 60 seconds fails. A successful preflight detects observed drift and age only. Jira can still change after the last read.

## Partial fields

Use closed typed system inputs for summary, description, environment, labels, priority and resolution, plus custom field IDs compiled from known native families. Omission, explicit null and empty values remain distinct. Text is bounded; multiline description/environment/known textarea strings become ADF. Summary cannot be empty or cleared. Required fields cannot be explicitly cleared or replaced with an empty value. Unknown optional fields can remain untouched; arbitrary system fields, schema/config execution, accounts and hierarchy changes are unavailable through the field map.

Qualify known built-in custom textfield, textarea, float, select, multiselect and multicheckboxes. Option IDs come from bounded native allowed values. Native metadata keys select field IDs; native `key` is descriptive and need not equal that ID. Compile closed local schemas; never execute native configuration or follow autocomplete links. Optional unsupported fields can be omitted, but supplying one fails.

Labels support replacement or typed disjoint add/remove sets. Replacement and deltas cannot coexist. Each delta needs its native metadata operation. Omit absent `fields` or `update` sections and never put the same field in both. Partial edits do not resubmit omitted required fields and preserve unknown/human-owned values.

For a required-label delta, read the current array under unchanged issue state and validate the resulting set. Adding a label can fill an empty current array; removing the last label cannot satisfy the requirement. Preserve untouched labels by sending the native delta rather than a replacement computed from that read. This is still subject to concurrent changes after observation.

## Transition requirements

Transitions are separate POST intents, never status field edits or data attached to an edit PUT. Require the exact returned transition ID, expected destination status and expanded `transitions.fields`. Reject an explicitly unavailable transition. A transition returned by a normal list excluding unavailable transitions is eligible when its optional availability flag is absent; a list including unavailable transitions requires explicit true. Server permissions and conditions are still evaluated at application.

For each required screen field omitted from the change, accept only a qualified nonempty native default explicitly indicated by metadata or fresh nonempty current-value evidence of a qualified shape. Read the omitted required fields and base identity/state through the declared narrow GET route, compare the state with the bracketed observation, and retain the original freshness bound. Never use caller-supplied or cached current values. Do not copy preserved current values into the mutation body. Null, empty, missing, malformed and unqualified value shapes fail with no mutation. A metadata default flag alone is insufficient evidence. Unknown required shapes require further qualification rather than an invented generic value.

## Native calls and recovery boundary

Edit uses PUT `/rest/api/3/issue/{issueIdOrKey}` with `notifyUsers=true` and `returnIssue=false`; transition uses POST `/rest/api/3/issue/{issueIdOrKey}/transitions`. Bodies contain only qualified `fields`, typed label `update`, and the actual transition ID for transitions. Do not expose screen/editability/remote-condition bypasses, arbitrary update operators, history metadata or properties.

Both normal successes are documented as HTTP 204. Preparation is not a mutation outcome. The host durably binds intent, claims once, runs final policy/access/freshness checks and preserves observed response and persistence evidence. Unknown edit/transition outcomes must not automatically replay. Later matching field/status observations can establish a postcondition, not attribution to a particular request. Provider preparation cannot activate public writes or mark full issue CRUD/transition delivery complete.
