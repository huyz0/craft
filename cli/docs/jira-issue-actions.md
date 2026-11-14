# Issue assignment and deletion preparation

Provider preparation and the journaled host support these actions under separate trusted host allowances. [Typed adapters and persisted policy](issue-action-command.md) connect that path to exact project records. Public activation is now qualified locally after independent review; see [current usage](jira-issue-writes.md) and [activation evidence](evidence/jira-issue-action-activation.md). Earlier [provider/host evidence](evidence/jira-issue-actions.md) and [adapter evidence](evidence/issue-action-command.md) remain historical checkpoints. Live tenant behavior is unqualified.

## Intent and authority

Keep `jira.issue.assign`, `jira.issue.unassign` and `jira.issue.delete` separate from edit and transition. Assignment accepts one exact account ID, never a name, email or the project-default sentinel `-1`. Unassignment has its own command and compiles only `{"accountId":null}`. Neither operation uses edit-screen metadata or claims that Edit Issues permission is required. Native Assign Issues, visibility, user eligibility and unassignment configuration still apply.

Assignment and unassignment have write effect; issue deletion has permanent effect, matching the catalog. Its preparatory observations remain reads. All three actual mutations require explicit apply and a durable journal claim.

Both assignment inputs require the numeric issue ID, complete expected `IssueState` and an explicit expected assignee: `{"kind":"unassigned"}` or `{"kind":"account","account_id":"account-one"}`. Omission is invalid. Assignment additionally supplies the destination `account_id`. No arbitrary native User object is accepted.

Deletion requires the numeric issue ID, complete expected state and explicit `subtasks` intent. `{"mode":"refuse"}` sends `deleteSubtasks=false` and refuses visible children during preparation. `{"mode":"delete_all","expected_visible_ids":["10001"]}` sends `deleteSubtasks=true`. The latter explicitly authorizes the native operation to delete **all actual subtasks**, including hidden children or additions after observation. The lexicographically sorted unique expected visible IDs are a detectable drift check, not a child allowlist, completeness proof or server compare-and-swap. At most 256 observed/expected children are accepted. A team unwilling to accept unobserved children must use refuse mode. Do not replace cascade with a sequence of individual child deletions.

Trusted host policy must separately allow assign, unassign, delete without subtasks and delete-all-subtasks. Existing edit/transition/create policies grant none. The caller's expected project must match trusted project policy and the native issue observation. Jira evaluates permissions independently. A local allowance proves neither native permission nor tenant configuration.

## Private preparation

Internal read operations carry the same validated intent as their corresponding write and declare their own scopes/routes. Assignment preparation needs `read:jira-work` and `read:jira-user`; unassignment and deletion declare only the issue-read scope. The three writes need `write:jira-work`. These are operation scopes, not the entire service grant: the existing sealed service-identity probe additionally requires `read:jira-user` or its qualified granular alternative. All preparation uses exact numeric issue identity and grant/actor binding.

Read the issue with `project,issuetype,status,updated` plus `assignee` or `subtasks`, without history updates. Compare complete expected state and explicit assignee or visible child expectations. For assignment only, query `/rest/api/3/user/assignable/search` with numeric `issueId`, exact `accountId`, `startAt=0` and `maxResults=2`. Exactly one matching active result is positive proof. Empty, inactive, duplicate or mismatched results authorize nothing and do not prove absence. Never substitute the project-bound create-assignment proof.

Repeat the narrow issue read after qualification and require unchanged state and relevant assignee/visible child IDs. Keep evidence in process, bound to both prepared operations, exact target and principal. Freeze the native call and original wall/monotonic observation times. Backward clocks and age of 60 seconds fail. A final checkpoint, read-operation verification and freshness check run before returning; the journaled host must recheck frozen evidence and dynamic policy at its final send boundary.

Frozen evidence records the observed assignee or visible child IDs, the native cascade intent and whether that cascade includes unobserved children. It never claims a complete hierarchy. Public JSON cannot reconstruct live authority.

## Outcomes and recovery

The native assignment endpoint returns 204 on success and documents 400/403/404 refusals. Deletion returns 204 and documents 400/401/403/404 refusals. Other responses remain unqualified or unknown. A 204 confirms the bound native operation, independently of later state or journal acknowledgment. It does not prove that the issue remains assigned to that account or enumerate deleted children. Receipts identify the addressed issue; no invented affected-child evidence is emitted. Unexpected synthetic body content must not erase a 204 confirmation.

A lost response, timeout, cancellation or rate-limit response never authorizes automatic retry. A later matching assignee or absent/invisible issue is not attribution. Preserve the operation journal and independent persistence/cache effects. An existing issue ID is not success evidence for assignment/deletion, and a later 404 does not confirm that this operation deleted it.

## Evidence sources

Checked on 10 November 2026 against the pinned `sources/jira-v3.json` under the active OpenSpec change: PUT issue assignee, DELETE issue and GET assignable user search. The corresponding official references are [issues](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/) and [user search](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-user-search/). Native API descriptions supply no established conditional-write or hierarchy snapshot guarantee.
