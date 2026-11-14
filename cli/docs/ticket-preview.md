# Preview reviewed tickets

After [preparing review context](review-preparation.md) and human approval on the plan page, run:

```powershell
craft --profile cf-test --max-bytes 1048576 plan tickets-preview --review-id payment-review --apply
```

The same explicit Confluence profile selects the prepared context and current reviewer/read policies. The command rebuilds native plan/design/Jira metadata, verifies current human approval, and publishes a complete local manifest. `--apply` authorizes local artifact publication. It sends no issue/link mutation and creates no tickets.

For a plan containing dependencies, include both fields in the preparation input:

```json
{
  "dependency_link_type_id": "10000",
  "dependency_link_direction": "inward"
}
```

Use the site's actual native type ID. The direction selects the label shown on the **dependent task toward its prerequisite**. For a type whose inward label is `is blocked by`, choose `inward` when that expresses the intended relationship. For a type whose outward label is `depends on`, choose `outward`. IDs and labels are read from Jira and included in the review binding. Both fields are required together; plans without included dependency edges refuse unused link mappings. Existing prepared IDs are immutable; prepare a new review ID and obtain new human approval when the mapping needs to change.

Jira's endpoint names are easy to misread. With dependent direction `inward`, the prerequisite becomes `inwardIssue` and the dependent becomes `outwardIssue`; direction `outward` reverses those endpoint roles. The artifact exposes prerequisite/dependent task identities, both native endpoint roles and the dependent's actual label so humans can review the mapping. Native link names, labels or directions changing after preparation invalidate approval. These conventions follow [Atlassian's linking model](https://developer.atlassian.com/cloud/jira/platform/issue-linking-model/).

The compact response contains `ticket_preview.manifest_digest`, task/batch/dependency counts, current approval counts, the preparation receipt, `verification_artifact` and credential-cache effects. Read `verification_artifact.local_path` for the complete manifest and review evidence. The `tickets-` artifact contains the current source/design context, exact issue inputs/payloads/metadata, qualified existing parents, deterministic dependency batches and all symbolic link endpoints. Fields such as descriptions/custom values remain the explicit proposals reviewed with the source plan.

A successful preview has `review_verified: true`, a confirmed artifact and zero tickets created. Unknown publication or late refusal preserves effects and clears the success/preview claims. Reusing the same invocation request ID returns the existing artifact and conflict before auth. A manifest digest identifies interpreted material; it is not send authority or an atomic lock across cloud products.

Apply the immutable preview through [ticket application and resume](ticket-application.md). The controller revalidates original approvals for each unfinished batch, compares fresh frozen native payloads/material metadata, and retains confirmed task-to-issue mappings without blind replay. For changed plans, use [newly reviewed existing-issue reuse](reviewed-issue-reuse.md) and a new run, preserving old preview bindings. Live tenant qualification remains unfinished. See [preview evidence](evidence/ticket-preview.md) and [application evidence](evidence/ticket-application.md).
