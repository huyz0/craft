# Inspect, remove and investigate Jira links

The link commands use explicit service-profile policy. `jira-read` has separate `jira.link.get`, `jira.link.list` and `jira.link.types` allowances for the selected site. Deletion requires `jira.link.delete` in `jira-mutation` for both endpoint projects. Investigation requires the original `jira.link.create` or `jira.link.delete` in `jira-recovery` for both projects. An ordinary issue allowance grants none of these operations. [Policy setup](policy-setup.md) describes complete replacement operation sets and revision checks.

```powershell
craft --profile jira-test jira link types --limit 20
craft --profile jira-test jira link list --issue-id 11000 --limit 20
craft --profile jira-test jira link get --link-id 90000
craft schema --command jira.link.list --response --fields id,link_type.name
```

Get retains the exact native identity and directed endpoints. List reads the visible `issuelinks` field on the specified numeric issue. Types returns the native type ID and current labels. Field selection projects both data and response schema; required identity fields remain. These are current observations under Jira visibility, not a complete inventory of hidden relations.

Lists and types use opaque local cursors for the remainder of the observed native response. Pass `page.next_cursor` as `--cursor` with the same site, actor, operation, input and field selection. A cursor survives process restart and can be replayed without discarding its remainder. Each invocation rechecks access and policy. The cursor does not invent a native offset or establish current write authority.

To delete, save the exact expected relation and actual native link ID in `delete-link.json`:

```json
{
  "operation_id": "remove-dependency",
  "apply": true,
  "project_id": "10000",
  "link_id": "90000",
  "expected": {
    "inward_issue_id": "12000",
    "outward_issue_id": "11000",
    "inward_project_id": "20000",
    "link_type": {
      "id": "10000",
      "name": "Blocks",
      "inward": "is blocked by",
      "outward": "blocks"
    }
  }
}
```

```powershell
craft --profile jira-test --max-bytes 8388608 jira link delete --input delete-link.json
```

`project_id` is the outward project. Deletion brackets current link/type/endpoint identities and checks Browse projects on both issues plus Link issues on at least one issue. It claims one permanent, comment-free DELETE for that exact link ID. Documented 200/204 acknowledgment confirms the addressed deletion, not deletion of either issue. A replacement link ID is not interchangeable. Confirmed, refused and unknown attempts cannot replay under the operation ID. Preserve the complete journal and independent credential-cache receipts; output projection is unavailable for deletion.

`jira link reconcile` investigates an original write without sending another write. Use an explicit profile, `--max-bytes 4194304` and a JSON input containing `operation_id` plus the complete original native canonical request in `original`. Obtain that contract through `craft schema --command jira.link.reconcile --view full`; the descriptor example shows the required shape. The original request must match the recorded digest, target, actor and grant before authentication. User-facing create/delete arguments are not a substitute for the canonical request.

Investigation after creation discovers matching candidate IDs from the original outward issue and checks each within a bounded strategy. Investigation after deletion checks the original link ID. Output distinguishes observed, changed, not observed and absent-or-invisible relations, retains initial/latest journal records and marks incomplete observations. A matching relation does not attribute creation; a missing link does not attribute deletion. Investigation never upgrades attribution, edits journal outcomes or authorizes replay.

The commands have local synthetic acceptance and independent source review. Live tenant permissions, provider behavior and other platforms require separate qualification. See [acceptance evidence](evidence/jira-link-lifecycle.md).
