# Atlassian test-site inventory

Recorded **9 November 2026**, Australia/Sydney, and completed on **14 November 2026** after the service account was provisioned. No credential values, account IDs or client identifiers are stored here.

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

## Inventory as of 14 November 2026

The site owner created a non-admin `CraftTest` service account and an OAuth 2.0 client credential, then ran `craft auth provision` themselves; Craft never saw the secret in chat or argv. The provisioning run exchanged client credentials, proved the same service actor through Jira and Confluence current-user reads and stored the grant in Windows Credential Manager under the explicit state root used for live runs. The [live conformance record](live-read-conformance.md#production-host-with-the-service-account) and the [Jira conformance matrix](jira-conformance.md) hold the run results.

| Input | Recorded value |
| --- | --- |
| Cloud ID | The value above, re-proved by both current-user probes under the provisioned grant |
| Supported auth modes | Service OAuth (client credentials) is supported and verified. Delegated OAuth is unavailable: no broker is deployed, so it stays disabled rather than guessed |
| Service-account eligibility | Verified by issuance and by both product probes returning one active service actor |
| Capability bundle | Exactly ten scopes: `read:jira-work`, `write:jira-work`, `read:jira-user`, `read:confluence-user`, `read:space:confluence`, `read:page:confluence`, `write:page:confluence`, `read:comment:confluence`, `read:content-details:confluence`, `search:confluence`. Broader selections hit Atlassian's 50-scope cap and drop the granular Confluence scopes, so this list is the requested bundle, not a minimum guess |
| Effective permissions | Jira browse/create/edit/transition and comment, link and attachment writes confirmed live. Issue deletion needed the project Administrator role, granted by the owner; without it the CLI reported Jira's refusal as a confirmed failure |
| Jira sandbox | The one team-managed project above. Live writes create and remove items labeled `craft-test` |
| Confluence sandbox | The site's default onboarding space, used for page create and removal |
| Independent reviewer policy | Unknown. No independent human reviewer identity is configured, so `review policy` rows stay absent and reviewed workflows remain unqualified live |
| Assets, Projects, Artifacts | Unknown and out of the current command surface; nothing depends on them |

No live mutation used a guessed value. Every write ran with an explicit profile, state root, policy target and the owner's per-batch approval, against the sandbox items above.

## Earlier unresolved inputs (historical)

Read-only recheck on **11 November 2026**: the in-app browser remains signed in to the same Jira test site and organization administration. The service-account screen still shows its empty-state creation offer. Actual native `craft profile list --limit 5` returned an empty list (restricted execution first refused access to the private default state path). Public Atlassian [service OAuth documentation](https://support.atlassian.com/user-management/docs/create-oauth-2-0-credential-for-service-accounts/) confirms client-credentials form exchange, reported scopes, api.atlassian.com routing and the centralized user-management prerequisite. No credential was inspected or issued. Automatic approval review rejected opening service-account creation because persistent account creation lacked explicit authorization; the user was asked specifically about a non-admin CraftTest account with Jira/Confluence access. That authorization is pending, independently of implementation work.

- Supported service-account credential issuance and exact scope grants.
- Successful Jira and Confluence OAuth current-user probes with a consistent service actor.
- Approved capability bundles and effective operation permissions.
- Selected Confluence sandbox space and an independent human reviewer policy.
- Assets workspace, schema, object-type and attribute identifiers and supported auth modes.
- Projects site/container and project ARIs, exact qualified fields and granted access.
- Artifacts availability, API/tool contract, ownership/sharing policy and unattended identity.

The list above was the state on 11 November. The 14 November inventory resolves the service-account, scope, probe, permission and sandbox rows; the reviewer, Assets, Projects and Artifacts rows are recorded as unknown. Related product findings are in the [Projects, Assets and Artifacts research](../../../docs/researches/atlassian-projects-assets-and-artifacts.md).
