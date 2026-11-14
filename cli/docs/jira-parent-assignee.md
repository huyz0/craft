# Qualified subtask parents and assignees

The Jira library can now qualify a subtask parent and one exact active assignee before freezing a create body. These are internal candidate workflows. The main CLI still registers no cloud provider or create command; authenticated host policy, public composition, live qualification and independent consequential review remain unfinished.

## Subtask parent evidence

Create metadata accepts an optional `parent_issue_id` containing a numeric issue ID. After resolving the exact project and selected issue type, it reads that parent through the declared issue route with only `project,issuetype` fields, `failFast=true` and `updateHistory=false`. It requires the response's issue ID to match the request, the returned project ID to match the selected project and the parent issue type to be explicitly non-subtask with a numeric type ID.

The check matters because Jira's issue GET can resolve moved issues without an HTTP redirect. A returned key or URL cannot override the requested numeric identity or project. Missing/hidden fields, denied visibility, a different returned ID/project or a subtask parent return an error. No create claim is acquired by these reads.

The private create context retains the qualified parent in its summary and field-contract identity. A subtask input must provide that same parent ID; omission, null and another ID fail preparation. The metadata must also expose a settable `parent` field with the qualified native `issuelink` shape. The generated body contains only `parent: {"id":"…"}`. Other hierarchy-parent semantics remain unavailable; this does not claim support for arbitrary epic or cross-project hierarchy relationships.

Parent evidence shares the metadata context's target, principal and 60-second dual-clock freshness policy. It is not a lock or provider snapshot; Jira still enforces the actual create's current rules.

## Separate account lookup and scope

`jira.metadata.assignee` is a candidate prerequisite read in an explicitly selected library registry. It is separate from the catalog's public resource surface and main CLI registry. General metadata retains `read:jira-work`; the account helper declares `read:jira-user` and the native permission condition *Browse users and groups OR Assign issues*. No scope grant or permission is inferred from registration.

The helper calls `GET /rest/api/3/user/assignable/search` with the exact `accountId`, selected project ID, `startAt=0` and `maxResults=2`. It never searches display names or email addresses and does not follow returned URLs. The pinned API describes an exact account match and warns that filtered search pages may be shorter than requested. Therefore only a positive result qualifies: the bounded response must contain exactly one entry with that exact account ID and `active: true`.

An empty, ambiguous, inactive, mismatched or malformed result produces no proof. Empty results do not establish account absence, lack of permission or safe unassignment. There is no fallback to broader user enumeration, name matching, automatic pagination or another account. Auth/status errors remain bounded and cause no retry.

The normalized summary retains only project target, account ID, observation time, active/assignable facts and `mutation_authorized: false`. Display names, email addresses and user URLs are not copied to the summary, error or journal. The private `AssigneeContext` is not deserializable from JSON and binds the target, supplied host-verified principal, account ID and its own clock.

## Freeze and recheck both proofs

Typed create input adds `parent_issue_id` and `assignee_account_id`. The host supplies an assignee context through `prepare_create_with_assignee`. A supplied account ID requires matching target/principal/account evidence, fresh clocks and a metadata-qualified settable `user` field. Any returned metadata `allowedValues` further constrains the account IDs. The body contains `assignee: {"accountId":"…"}`; raw user objects are not accepted as input.

Omitted assignee input preserves Jira's default selection. Explicit null is refused with an unsupported-policy result because unassignment eligibility and native clear semantics have not been qualified. Automatic/unassigned sentinels are not account IDs. Reporter, issue security, custom user pickers and other account semantics remain separate work rather than reusing assignability as universal user permission.

The frozen body retains metadata and assignee freshness independently. The actual host uses `send_checked` to call `FrozenCreate::verify` after durable claim acquisition and immediately before HTTP polling. Expiry of either proof prevents the POST. An already acquired claim remains conservatively unknown and cannot be reopened as an implicit retry.

The host must still verify that its asserted principal matches the authenticated HTTP session, scope and operation policy. These private objects provide local binding and positive provider evidence, not authentication, human approval or cross-machine fencing.

Source: the [pinned Jira v3 specification](../openspec/changes/build-cloud-cli/sources/jira-v3.json), `getIssue`, `findAssignableUsers`, `User.active`, create-field `JsonTypeBean` and metadata operation scopes. See [metadata preparation](jira-create-metadata.md), [send boundary](journaled-http.md) and [observed evidence](evidence/jira-parent-assignee.md).
