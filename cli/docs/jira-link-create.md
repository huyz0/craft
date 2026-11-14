# Create a directed Jira issue link

`jira link create` links two exact existing issue IDs through one comment-free native POST. Use an explicit Jira service profile with an enabled `jira.link.create` allowance in its `jira-mutation` policy for **each endpoint project**. Both records must identify the same service actor, grant, installation and site. A same-project link uses that project's record for both endpoints. [Policy setup](policy-setup.md) explains apply and revision CAS; `operations` is the complete desired allowance set.

The host needs classic `read:jira-work` and `write:jira-work` scopes plus the native current-user probe scope. It reads current type labels, brackets both issue/project identities, and checks issue-specific Browse projects on both endpoints and Link issues on the outward endpoint. Local allowance does not grant Jira permissions.

Save a non-secret `link.json` using the site's actual numeric identities and current labels:

```json
{
  "operation_id": "dependency-link",
  "apply": true,
  "project_id": "10000",
  "inward_project_id": "20000",
  "inward_issue_id": "12000",
  "outward_issue_id": "11000",
  "link_type": {
    "id": "10000",
    "name": "Blocks",
    "inward": "is blocked by",
    "outward": "blocks"
  }
}
```

```powershell
craft --profile jira-test --max-bytes 8388608 jira link create --input link.json
craft schema --command jira.link.create --view full
```

`project_id` is the expected outward project. Native endpoint roles are used exactly as supplied. For dependency labels and the corresponding endpoint roles, follow [ticket preview](ticket-preview.md). The expected type's ID/name/inward/outward values must still match Jira. Aliases, self-links, unexpected comments, incomplete type data and stale membership/labels refuse. No comment field is supported.

The output allowance reserves room for complete exceptional journal/cache evidence; ordinary output remains small. Projection is unavailable because recovery evidence must remain complete. A documented empty `201` confirms the requested relation. Jira uses the same success for a duplicate relation and supplies no link ID, so `new_link_created` and `link_id` remain null. `relation_confirmed` describes this invocation's native acknowledgment, while the operation receipt and retained records can include earlier evidence.

Confirmed, refused and uncertain attempts cannot obtain another send claim under the same operation ID. Lost transport/claim/receipt acknowledgments remain visible; no automatic POST retry occurs. Keep the receipt, persistence disposition, initial/pending/latest records and independent cache effects. Unexpected resources or other journal effects require reconciliation rather than success. Inspect `operation get --operation-id dependency-link` and follow [link investigation](jira-link-lifecycle.md) before deciding recovery.

General link creation is public after local qualification and independent review. Reviewed plan application must additionally compare frozen payloads and revalidate original human approvals before each batch. Live-tenant behavior and other platforms remain unqualified. See [evidence](evidence/jira-link-create.md).
