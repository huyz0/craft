# Jira dependency links in reviewed plans

Checked 10 November 2026. This note supports Craft's plan-to-ticket preview and later application; it does not establish live-tenant behavior.

[Jira's link-type endpoint](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-link-types/) provides a nonpaged catalogue of configured type IDs and directional labels. It requires issue linking to be enabled and declares classic `read:jira-work` or granular `read:issue-link-type:jira` scope. Catalogue visibility does not grant permission to create links. Craft uses a sealed authenticated read despite the endpoint's anonymous-access capability.

[Atlassian's linking model](https://developer.atlassian.com/cloud/jira/platform/issue-linking-model/) explains labels from the issue viewer's perspective. A neighboring `inwardIssue` corresponds to the inward label displayed on the current issue. Consequently, when the dependent should display an inward label such as `is blocked by`, the native endpoints are `inwardIssue = prerequisite` and `outwardIssue = dependent`. Choosing the outward label reverses those roles. A type's name alone cannot select the intended relation.

[The official Atlassian MCP issue #112](https://github.com/atlassian/atlassian-mcp-server/issues/112) reports agents creating reversed dependency links. The maintainer said the tool forwarded inputs unchanged and improved its instructions. This supports making role semantics explicit in the CLI, rather than assuming endpoint names will be interpreted correctly. The report is not proof of a Craft live test.

Craft preparation therefore requires both `dependency_link_type_id` and `dependency_link_direction` when included tasks have dependencies. Direction names the label on the dependent toward its prerequisite. The review artifact retains observed native labels; the ticket preview retains prerequisite/dependent identities, native endpoint identities and the dependent's label. Native label changes invalidate the reviewed material digest. No implicit type is chosen from a name such as “Blocks.”

Implementation and limits are recorded in [ticket-preview evidence](../../cli/docs/evidence/ticket-preview.md). Both directions passed synthetic native HTTPS fixtures. A future consented live qualification must create owned test issues, confirm each issue's displayed label and GET representation, and retain cleanup receipts. Ticket/link application remains unfinished.

Follow-up, 11 November 2026: [link-create evidence](../../cli/docs/evidence/jira-link-create.md) records the new journaled write host. The [native write API](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-links/) acknowledges duplicate relations without returning a link ID. Craft therefore retains unknown creation attribution, exact endpoint roles and claim/receipt evidence. Link inspection/deletion/reconciliation and reviewed composite application remain unfinished.
