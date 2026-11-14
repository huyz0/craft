# Sources: agent-facing Atlassian CLI

Primary sources opened during this research, initially 8 November and extended 9 November 2026. Living documentation and package APIs may change. Source IDs are used by the [research](atlassian-agent-cli-research.md), [command catalog](../plans/cloud-cli-command-catalog.md), and linked planning documents. Findings are documentation-level evidence, not a live tenant test. No long quotations or vendor performance claims are used. The register now contains 48 references.

| ID | Source | Material reviewed / limit |
| --- | --- | --- |
| A01 | [Atlassian Jira OAuth 2.0 3LO](https://developer.atlassian.com/cloud/jira/platform/oauth-2-3lo-apps/) | App distribution guidance, grant types, refresh defaults, and property-search limitation; updated 6 November 2026. No PKCE support found in inspected text; absence is not proof of universal non-support. |
| A02 | [Authorization code flow](https://developer.atlassian.com/cloud/oauth/getting-started/implementing-oauth-3lo/) | Browser authorization, state, registered redirect, required client secret. |
| A03 | [Refresh token flow](https://developer.atlassian.com/cloud/oauth/getting-started/refresh-tokens/) | Offline access, rotation, secret requirement, invalid-grant handling. |
| A04 | [OAuth credentials for service accounts](https://support.atlassian.com/user-management/docs/create-oauth-2-0-credential-for-service-accounts/) | Centralized-management prerequisite, client credentials, product scopes, administrative revocation. Actual organizational eligibility unverified. |
| A05 | [RFC 8252](https://www.rfc-editor.org/info/rfc8252/) | Native-app browser authorization and embedded-secret boundary, especially section 8.5. A standards recommendation does not establish Atlassian support for a particular flow. |
| A06 | [Making calls to Atlassian APIs](https://developer.atlassian.com/cloud/oauth/getting-started/making-calls-to-api/) | Gateway routing, cloud IDs, 3LO resource discovery and ordering. One introductory sentence misleadingly says code in the Authorization header; examples correctly show ACCESS_TOKEN. Plan uses access tokens. |
| A07 | [Rust keyring 4.2.0](https://docs.rs/keyring/4.2.0/keyring/) | Current library design and recommendation for explicit backend control. Opened through latest URL, which identified this version. |
| A08 | [Rust keyring-core 1.0.1](https://docs.rs/keyring-core/1.0.1/keyring_core/) | Core abstraction, insecure example/mock stores, credential concurrency caveat. Opened latest URL, which identified this version. Specific backend versions remain an implementation spike. |
| A09 | [Microsoft CREDENTIALW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw) | CredentialBlobSize limit and native credential structure. Body was accessible despite generic sign-in banner. |
| A10 | [Freedesktop Secret Service specification](https://specifications.freedesktop.org/secret-service/latest/) | D-Bus service, collections, sessions, locking and prompting outline. Latest URL identified a draft specification; deployed compatibility must be tested. |
| A11 | [Zalando go-keyring](https://github.com/zalando/go-keyring) | Platform dependencies and macOS command-backed implementation. Evaluated as an alternative, not selected. |
| A12 | [Jira entity properties](https://developer.atlassian.com/cloud/jira/platform/jira-entity-properties/) | Shared editability, last-write-wins behavior, sensitivity and size limits. |
| A13 | [Jira issue search](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-search/) | Enhanced search, pagination, delayed visibility, reconcileIssues. |
| A14 | [Jira issues](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/) | Create/edit/metadata/transitions, scope declarations and ADF use. Exact per-operation payloads must be pinned from OpenAPI during implementation. |
| A15 | [Confluence v2 pages](https://developer.atlassian.com/cloud/confluence/rest/v2/api-group-page/) | Published default, draft options, representations, versioned updates and draft-reconciliation warning. No live draft test performed. |
| A16 | [Official ACLI command tree](https://developer.atlassian.com/cloud/acli/reference/commands/) | Opened tree listed Jira/admin/Rovo Dev; absence of Confluence in that tree is not proof that no binary build supports it. |
| A17 | [ACLI Jira login](https://developer.atlassian.com/cloud/acli/reference/commands/jira-auth-login/) | Browser OAuth and auth command family. Does not establish secret storage details. |
| A18 | [Build an app respecting Jira rate limits](https://developer.atlassian.com/cloud/jira/platform/build-an-app-that-respects-rate-limits/) | Shared-limit scopes, RateLimit-Reason and Retry-After behavior. |
| A19 | [Confluence rate limiting](https://developer.atlassian.com/cloud/confluence/rate-limiting/) | 429 handling and current rate-limit model. No stress testing performed or proposed on production tenants. |
| A20 | [Confluence content search](https://developer.atlassian.com/cloud/confluence/rest/v1/api-group-search/) | CQL, permission filtering, cursor links, classic/granular scopes. |
| A21 | [Confluence v2 comments](https://developer.atlassian.com/cloud/confluence/rest/v2/api-group-comment/) | Page footer comments, comment lookup, page association, version/author metadata, and comment scopes. The CLI's approval semantics are proposed policy, not a native Confluence approval guarantee. |
| A22 | [Jira attachments](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-attachments/) | Multipart upload, metadata/content/delete, settings, special header; no binary-update operation in this group. |
| A23 | [Jira comments](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-comments/) | CRUD, ADF, visibility and own/all comment permissions. |
| A24 | [Jira issue links](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-links/) | Link create/get/delete and issue-linking prerequisites. |
| A25 | [Jira remote links](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-remote-links/) | CRUD and global-ID create/update semantics. |
| A26 | [Jira projects](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-projects/) | Project metadata, administrative lifecycle, synchronous/asynchronous delete alternatives. |
| A27 | [Jira project versions](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-project-versions/) | CRUD, release/archive flags, replacement/move semantics, project-management scope. |
| A28 | [Jira components](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-project-components/) | Project component discovery and CRUD. |
| A29 | [Jira fields](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-fields/) | Read discovery and administrative operations; custom-field administration deferred. |
| A30 | [Jira permissions](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-permissions/) | Contextual permission checks; not an authorization reservation. |
| A31 | [Confluence spaces v2](https://developer.atlassian.com/cloud/confluence/rest/v2/api-group-space/) | List/get/create, RBAC-only creation availability and global create-space permission. |
| A32 | [Confluence spaces v1](https://developer.atlassian.com/cloud/confluence/rest/v1/api-group-space/) | Legacy create, metadata update, permanent asynchronous deletion and scopes. |
| A33 | [Confluence attachment writes](https://developer.atlassian.com/cloud/confluence/rest/v1/api-group-content---attachments/) | Multipart create/upsert, binary replacement, metadata update, download redirect and special header. |
| A34 | [Confluence attachments v2](https://developer.atlassian.com/cloud/confluence/rest/v2/api-group-attachment/) | List/get, trash versus purge, scopes and stronger purge permission. |
| A35 | [Confluence content lifecycle](https://developer.atlassian.com/cloud/confluence/rest/v1/api-group-content/) | Experimental asynchronous archive. Restore route not established by this review. |
| A36 | [Confluence content states](https://developer.atlassian.com/cloud/confluence/rest/v1/api-group-content-states/) | Badge get/set/clear, available states, lifecycle-status query and space settings. Not approval. |
| A37 | [Confluence move and copy](https://developer.atlassian.com/cloud/confluence/rest/v1/api-group-content---children-and-descendants/) | Move positions, single-page/tree copy. Do not use deprecated descendant reads without checking replacements. |
| A38 | [Confluence versions v2](https://developer.atlassian.com/cloud/confluence/rest/v2/api-group-version/) | Page/attachment/comment version metadata routes. |
| A39 | [Confluence labels v2](https://developer.atlassian.com/cloud/confluence/rest/v2/api-group-label/) | Page and space label retrieval. |
| A40 | [Confluence content labels v1](https://developer.atlassian.com/cloud/confluence/rest/v1/api-group-content-labels/) | Add/remove operations and scope declarations. |
| A41 | [Confluence restrictions](https://developer.atlassian.com/cloud/confluence/rest/v1/api-group-content-restrictions/) | Read/add/replace/remove restrictions; broadening-access implications. |
| A42 | [Confluence properties v2](https://developer.atlassian.com/cloud/confluence/rest/v2/api-group-content-properties/) | Page property CRUD with versions. |
| A43 | [Confluence long tasks](https://developer.atlassian.com/cloud/confluence/rest/v1/api-group-long-running-task/) | Task status polling after asynchronous submission. |
| A44 | [Bitbucket REST and authentication](https://developer.atlassian.com/cloud/bitbucket/rest/intro/) | Distinct API base, OAuth endpoint/scopes, token types and refresh behavior. Future-provider evidence, not verified integration. |
| A45 | [Bitbucket pull requests](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-pullrequests/) | Operation outline: PRs, diffs, comments/tasks, approval, request changes, merge and status. Full payload/scope audit deferred. |
| A46 | [Trello confidential OAuth2](https://developer.atlassian.com/cloud/trello/guides/rest-api/oauth-2-confidential-client-usage/) | PKCE S256, secret, bearer usage, rotation. Prose token endpoint conflicts with endpoint list/code; sandbox verification required. Updated 1 October 2026. |
| A47 | [Trello cards](https://developer.atlassian.com/cloud/trello/rest/api-group-cards/) | Card CRUD, list/archive fields, attachment/comment/checklist operations. Auth compatibility per endpoint not live-tested. |
| A48 | [Trello OAuth2 client configuration](https://developer.atlassian.com/cloud/trello/guides/rest-api/oauth-2-client-configuration/) | Public versus confidential clients, callback constraints and Power-Up guidance. Public client support does not establish a tested CLI flow. Updated 1 October 2026. |

## Important distinctions

- **Documented:** authorization-code exchange requires a secret. **Not established:** a secretless Jira/Confluence 3LO public-client flow. **Proposed:** confidential broker for delegated login.
- **Documented:** service-account OAuth exists. **Not established:** the user's organization is eligible or grants access to both products.
- **Documented:** keyring libraries expose several stores. **Not established:** a selected backend works unattended on all supported OS/session combinations.
- **User-defined:** plan drafts are normal shared Confluence review pages, then approved page revisions become Jira tickets. Native unpublished drafts are not required.
- **Documented:** Jira properties do not resolve concurrent edits. **Proposed:** external execution leases or a single mutation authority for multi-machine workers.
- **Proposed command examples:** all `craft` commands in the plan are a new interface, not existing Atlassian commands.
