# Atlassian test-site inventory

Recorded **9 November 2026**, Australia/Sydney. Partial evidence only. No credential values are stored here.

## Observed deployment

| Item | Evidence and value |
| --- | --- |
| Authorized test site | User supplied `https://craft-test.atlassian.net` |
| Cloud ID | `8e67a304-0cda-4235-9745-07472c26f592`, returned by read-only GET `/_edge/tenant_info` without credentials or redirects |
| Organization | (organization name and ID redacted) |
| Browser user | User-supplied account email (redacted); signed-in administration UI observed |
| Browser account ID | Admin user link contains (account ID redacted); this does not establish a service OAuth actor |
| Existing Jira resource | Key `SCRUM`, displayed name (redacted) |
| App entries | Jira Standard, Confluence Standard, Projects Free and Goals Free observed in administration |
| Other app entry | `cmdb` shown as Free; this label alone does not qualify an Assets workspace or license |
| Service accounts | Directory screen reachable, offering creation; no existing accounts listed in the observed screen |

The browser inspection established signed-in UI access and app entries. It did not establish API eligibility, billing entitlements for each operation, server scope grants or effective mutation permissions. The service-account screen is evidence of UI availability, not successful credential provisioning.

## Unresolved deployment inputs

Read-only recheck on **11 November 2026**: the in-app browser remains signed in to the same Jira test site and organization administration. The service-account screen still shows its empty-state creation offer. Actual native `craft profile list --limit 5` returned an empty list (restricted execution first refused access to the private default state path). Public Atlassian [service OAuth documentation](https://support.atlassian.com/user-management/docs/create-oauth-2-0-credential-for-service-accounts/) confirms client-credentials form exchange, reported scopes, api.atlassian.com routing and the centralized user-management prerequisite. No credential was inspected or issued. Automatic approval review rejected opening service-account creation because persistent account creation lacked explicit authorization; the user was asked specifically about a non-admin CraftTest account with Jira/Confluence access. That authorization is pending, independently of implementation work.

- Supported service-account credential issuance and exact scope grants.
- Successful Jira and Confluence OAuth current-user probes with a consistent service actor.
- Approved capability bundles and effective operation permissions.
- Selected Confluence sandbox space and an independent human reviewer policy.
- Assets workspace, schema, object-type and attribute identifiers and supported auth modes.
- Projects site/container and project ARIs, exact qualified fields and granted access.
- Artifacts availability, API/tool contract, ownership/sharing policy and unattended identity.

No service account, credential, project, issue, page, schema, object or artifact was created. Browser login evidence is separate from CLI acceptance. The inventory task remains unchecked pending these inputs and live qualification. Related product findings are in the [Projects, Assets and Artifacts research](../../../docs/researches/atlassian-projects-assets-and-artifacts.md).
