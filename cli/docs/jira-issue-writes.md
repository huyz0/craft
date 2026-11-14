# Change Jira issues and inspect uncertain outcomes

The development CLI exposes issue update, assignment, explicit unassignment, permanent deletion and native workflow transition execution. Each uses the existing journaled host, verified service identity and separate project-operation policy. Local qualification is recorded in [activation evidence](evidence/jira-issue-action-activation.md); live tenant behavior remains unqualified.

| Command | Exact local mutation allowance | Required additional intent |
| --- | --- | --- |
| `jira issue update` | jira.issue.update | Typed partial changes |
| `jira transition apply` | jira.transition.apply | Native transition ID and expected destination status |
| `jira issue assign` | jira.issue.assign | Expected assignee and exact destination account ID |
| `jira issue unassign` | jira.issue.unassign | Expected assignee |
| `jira issue delete` | jira.issue.delete or jira.issue.delete-all-subtasks | Explicit refuse/cascade subtask mode |

Configure only the required `jira-mutation` allowances using [policy setup](policy-setup.md). Create/read permissions do not grant these actions. Policy is local intent; native permissions, transition conditions, assignment eligibility and tenant configuration remain independent. Server refusal cannot be bypassed with a different operation ID.

Read the current issue with the fields needed for expected state:

```powershell
craft --profile jira-test jira issue get --issue SCRUM-1 --fields id,key,project.id,issue_type.id,status.id,updated
craft schema --command jira.issue.update --view full
```

Use the returned numeric issue ID, key, project/type/status IDs and exact updated value. For assignment, also read `assignee.account_id`. Metadata or transition reads provide native constraints, not write permission. The host independently reads and brackets current state; serialized expectations do not recreate live authority. Jira can still change after the last read, so Craft does not promise native compare-and-swap.

A complete illustrative update input is:

```json
{
  "operation_id": "refresh-payment-summary",
  "apply": true,
  "issue": "10000",
  "expected": {
    "id": "10000", "key": "SCRUM-1", "project_id": "100",
    "issue_type_id": "101", "status_id": "102",
    "updated": "2026-11-10T01:00:00.000+0000"
  },
  "changes": {"summary": "Reviewed payment summary"}
}
```

Replace every example identity/state with observed values and choose the intended summary before applying:

```powershell
craft input validate --command jira.issue.update --file update.json
craft --profile jira-test --max-bytes 1048576 jira issue update --input update.json
```

Validation checks input, not permissions or freshness. These commands require explicit apply rather than offering a complete remote dry run. Keep input files and the original operation ID. Standalone issue commands confirm an immutable canonical source before native acquisition/sending and return a compact `saved_request` receipt. It reports local source publication separately from native effects; a confirmed source does not establish a sent or successful write. Partial updates preserve omitted fields; null and empty values have their declared distinct meanings. Use a transition command for status changes, never a status field in the update map.

Assignment uses `expected_assignee` as either `{"kind":"unassigned"}` or `{"kind":"account","account_id":"observed-account"}`, plus `account_id` for the destination. It does not accept names, emails or the project-default sentinel. Unassignment has its own allowance and explicit command; removing an account field from another request is not unassignment.

Deletion has permanent effect. `subtasks: {"mode":"refuse"}` refuses visible children and keeps Jira's native non-cascade guard. `{"mode":"delete_all","expected_visible_ids":["10001"]}` requires the separate cascade allowance and authorizes Jira to delete all subtasks, including invisible or newly added children. The visible list is a drift guard, not a complete hierarchy claim. No subtask mode is synthesized when omitted. Discover exact schemas rather than copying these fragments as complete requests.

Retain the full receipt, journal revision, persistence/cache dispositions and any prior record. A timeout, lost response or unknown journal acknowledgment does not permit another send. A confirmed 204 means the addressed native operation was processed; it does not prove that concurrent automation left the requested state unchanged.

Inspect local evidence first:

```powershell
craft operation get --operation-id refresh-payment-summary
```

Keep the original state root; this local command rejects `--profile`. For a captured standalone issue write, `operation reconcile --saved` loads its canonical source under the same operation ID, original service profile and independent `jira-recovery` allowance:

```powershell
craft --profile jira-test --max-bytes 16777216 operation reconcile --operation-id refresh-payment-summary --saved
```

The format-one source is a bounded immutable JSON artifact under the selected state's private requests store. It retains target/input/preconditions, native descriptor binding, grant and actor, without credentials. The first source remains unchanged when only output controls differ. A conflicting or corrupt source stops a new attempt. Unknown source publication is reported in `saved_request.write_state`; native receipt state still describes native effects. Storage failure rereads journal evidence and never authorizes another send. Null source metadata in a refusal means unobserved, not absence.

New reviewed-controller issue-create steps also retain their canonical source under the reported native operation ID. Legacy records and controller link/page calls may not have a supported source. They can still use structured reconciliation input containing the exact original **canonical native request**, which differs from the CLI argument object: operation/apply/cloud controls are outside native input. Explicit original and saved selection are mutually exclusive. A digest cannot reconstruct a missing original; report that gap instead of guessing it. Native descriptors and historical journal bindings have not changed. Internal controller contracts retain their separate manifests and recovery behavior.

Recovery can inspect an existing issue or explicit bounded create candidates. Missing local records and unknown creates without candidate identities stay local without opening credentials. Saved data is revalidated and current recovery policy/journal bindings still apply. Successful inspection is not successful mutation: matching fields/assignee/status or an absent-or-invisible issue never upgrades attribution, changes the journal or authorizes replay. Link writes use the separate `jira link reconcile` workflow. Automatic correlation discovery and operator-directed completion remain unfinished. See [reconciliation semantics](jira-reconciliation.md).
