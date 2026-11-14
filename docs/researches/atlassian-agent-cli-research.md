# Research: cloud CLI for coding agents, starting with Jira and Confluence

The recommended product is a deterministic cloud CLI that coding agents invoke to read context, manage resources, maintain a work ledger, and prepare or apply design documents and task plans. Start with Jira and Confluence, then extend to Bitbucket, Trello, and other systems. It should not embed another planning LLM. The calling agent writes the content; the CLI validates, renders, authenticates, and applies it.

The deliverable for this goal is research and a detailed implementation plan. No CLI, credentials, OAuth application, or Atlassian content was created. See the [implementation plan](../plans/atlassian-agent-cli-plan.md), [proposed command contract](../plans/atlassian-agent-cli-contract.md), and [source register](atlassian-agent-cli-sources.md).

**User-defined workflow:** an agent creates a normal Confluence page containing the proposed plan and all tasks. Humans review and edit that page. Only after review is complete does the CLI create Jira tickets from the approved page revision. “Draft” describes the plan's review state, not an unpublished Confluence page.

## Scope and assumptions

Assume Jira Cloud and Confluence Cloud. Data Center uses different deployment/authentication contracts and requires a separately researched adapter. The CLI is for agents on Windows, Linux, and macOS. A human or administrator performs initial authorization; normal task commands run without prompts and without exposing tokens to the model.

Several people can review the shared Jira/Confluence records. Authentication can represent a delegated person or a service account; record the effective actor separately from the task requester. The CLI is an integration tool, not a complete scheduler, production deployment controller, or compliance system.

## Findings that determine the design

### 1. Browser OAuth is not automatically a safe standalone CLI flow

Atlassian's documented authorization-code exchange and refresh exchange require `client_secret`. The inspected guides do not establish a public-client, secretless PKCE or device-authorization flow for Jira/Confluence 3LO. Treat those flows as unverified rather than inventing them. [A02, A03]

RFC 8252 explains why a shared secret embedded in a distributed native application is not confidential and favors an external browser for authorization. [A05]

**Recommendation:** do not compile a shared Atlassian client secret into the binary or retrieve it from another installed CLI. For delegated-user login, use an organization-operated confidential OAuth broker. It keeps the app secret server-side while returning provider tokens through an authenticated, single-use handoff to the CLI for local keyring storage. The broker is an additional component and trust boundary, not an Atlassian feature supplied automatically.

Atlassian recommends one distributable 3LO integration and warns against apps that collect customer API tokens or require customers to create individual 3LO applications. It also documents account-level and resource-level grants. Prefer resource-limited grants where available and validated. Do not propose a per-person developer-console registration as the distribution strategy. [A01]

### 2. Unattended agents have a service-account OAuth option

Atlassian documents `client_credentials` OAuth for service accounts, including Jira and Confluence scopes and gateway calls. Its stated prerequisite is centralized user management. Tokens are documented as valid for 60 minutes, and credentials can be revoked through administration. [A04]

**Recommendation:** make this the preferred unattended mode when available. An administrator provisions the service credential once; the CLI stores its secret and issued token in the OS keyring, and obtains another token when needed. Do not apply the 3LO refresh-token algorithm to this mode. Organization eligibility, product access, scopes, and administrator provisioning must be checked before promising it works on the user's sites.

### 3. Sites, scopes, and product permissions are distinct

OAuth API requests use `api.atlassian.com` gateways with a cloud ID. For 3LO, `accessible-resources` identifies authorized sites and associated scopes; its ordering is not a way to choose the latest site. [A06]

**Recommendation:** persist explicit Jira and Confluence site selections, keyed by cloud ID and product. Never select the first returned site. Service-account profiles use admin-provided cloud IDs and validate access directly; do not assume the 3LO discovery endpoint is available for every auth mode. OAuth scope, product/project/space permissions, app-access rules, and local policy must all permit the operation.

### 4. Keyring support needs platform-specific verification

The Rust keyring 4.2 documentation advises applications needing explicit store control to use `keyring-core` with specific credential-store implementations. `keyring-core` itself contains mock/sample stores that are not secure production storage and warns about concurrent access to credentials. [A07, A08]

Windows Credential Manager has a documented credential blob limit of 2,560 bytes. Tokens must therefore be treated as variable-length data, not assumed to fit in one combined JSON credential. [A09]

Linux Secret Service uses D-Bus and includes locked collections and prompting. An installed library is not evidence of an unlocked, available vault in a headless runner. [A10]

The Go `zalando/go-keyring` alternative supports the target systems but documents invoking `/usr/bin/security` on macOS. That is an implementation trade-off for a design that prohibits token-bearing subprocess arguments. [A11]

**Recommendation:** Rust with explicit Windows Credential Manager, macOS Keychain, and persistent Linux Secret Service adapters. Choose and pin exact backend crates during the first implementation spike. No automatic plaintext, file, environment-only, mock, or session-only fallback. A locked/unavailable store returns a structured error and requires provisioning or unlock outside normal agent execution.

### 5. Jira is a business ledger, not a distributed lock service

Jira entity properties are editable by actors with entity-edit access, exist in a shared namespace, and use the most recently saved value without a concurrency mechanism. They must not contain secrets. [A12]

**Recommendation:** keep workflow state and human visibility in Jira. For multiple workers/machines, obtain exclusive execution leases from a single mutation coordinator or another transactional authority. A local file lock protects local processes only. Setting an assignee, adding a comment, or writing a lease-looking issue property does not prove exclusive ownership.

The modern Jira search resource is `/rest/api/3/search/jql`, with cursor-like `nextPageToken` pagination and documented delayed visibility after updates. `reconcileIssues` provides stronger read-after-write assurances for known issues. [A13]

Issue creation/editing/transitions have separate contracts; required fields and available transitions depend on project configuration. Jira rich-text descriptions and relevant fields use Atlassian Document Format. [A14]

**Recommendation:** discover metadata and transition IDs, validate every task before application, and reconcile known object IDs directly. Do not turn an empty search result immediately after a write into permission to create a duplicate.

### 6. The plan draft is a shared review page

Confluence v2 supports normal published pages and versioned updates. The update documentation warns that updating current content can affect diverged editor drafts. [A15]

**Required design:** create a normal shared Confluence page, visibly marked Proposed or In review, containing the full design/plan and structured task sections. Page publication makes it reviewable; it does not authorize ticket creation. Import the human-edited page and bind approval to its version and content digest. Reject changed, ambiguous, or unapproved content rather than creating tickets from an old local file. Native unpublished-page support is outside the requested workflow.

Jira issue creation creates a real issue. For plan materialization, defer those writes until the shared plan is approved. General authorized issue creation remains available outside that workflow. A Jira status named Draft is not a substitute for the requested Confluence-first review. [A14]

Confluence provides page footer-comment retrieval. An implementation can verify an explicitly referenced human review comment against its page association and identity metadata, subject to configured reviewer policy and sandbox validation. [A21]

### 7. A partial existing CLI is not a proven replacement

The opened official ACLI command tree establishes Jira commands, while its Jira auth documentation includes browser OAuth, logout/status/switch, and token input. The opened tree did not establish Confluence commands, and the guessed Confluence reference URL was inaccessible. [A16, A17]

**Recommendation:** evaluate the current installed ACLI as an optional compatibility spike, not the core transport or an undocumented token store. A unified agent contract, strict keyring behavior, local design/task drafts, replay recovery, and Jira/Confluence linking still need explicit implementation and verification.

### 8. Rate limiting and uncertain writes are core behavior

Jira documents shared rate limits across processes/instances, scoped limit reasons, and delay headers. Confluence also documents 429 responses and retry behavior. [A18, A19]

**Recommendation:** bound page sizes and output bytes, respect server delays, coordinate rate budgets in a shared runner when necessary, and retry only where operation semantics allow. A timeout after a create may mean the object exists. Return an unknown-write outcome and reconcile before retrying; do not promise exactly-once remote effects.

## API strategy

Use Jira REST v3 and Confluence REST v2 where the needed operation exists. Keep dedicated v1 adapters for CQL, attachment writes, content badges, move/copy, and selected space operations. The CQL endpoint provides permission-filtered search with cursor links and its own scopes. [A20, A32, A33, A36, A37]

Use typed, deliberately selected operations rather than an unrestricted authenticated HTTP command. The expanded [API-backed command catalog](../plans/cloud-cli-command-catalog.md) covers general CRUD, attachments, spaces, comments, transitions, badges, labels and privileged operations. Map every operation to required scopes and permissions before shipping it. This replaces the earlier narrow resource subset as the scope reference.

Do not interpret Markdown as an Atlassian API body format. Convert a documented Markdown subset into validated Jira ADF or Confluence storage format; reject unsupported constructs or preserve existing structured content through an explicit raw format path. Native document formats and code blocks need round-trip tests.

## Decisions and remaining evidence

The 9 November extension adopts [provider modules above a shared core](../plans/cloud-cli-provider-architecture.md). Share keyring/auth primitives, JSON, bounded transport, policy, journals and async-job reporting. Keep each provider's resource names and semantics. Build compiled first-party providers before considering third-party plugins. General CRUD and approval-bound factory workflows are separate layers.

The APIs require visible distinctions: Confluence v2 space creation is RBAC-tenant dependent; legacy space deletion is permanent and asynchronous; attachment writes use v1; content badges are not review approval. [A31, A32, A33, A36] Bitbucket and Trello need their own auth strategies and scope mapping. Trello documents public clients and confidential PKCE, but native CLI callbacks and the inconsistent token-endpoint prose require a spike. [A44, A46, A48]

Recommended defaults are Rust, strict JSON, native OS stores, Cloud adapters, local-first proposals, service OAuth for unattended work, and a confidential broker for delegated browser OAuth. These are design decisions, not results of a built prototype.

Implementation starts by verifying the target organization's auth eligibility, backend vault behavior, selected APIs/scopes, shared plan-page round trips, and verified human review before ticket creation. If any assumption fails, record the observed failure and revise the adapter or auth path. The plan contains concrete stop conditions and alternatives without requiring an answer now.

This session reviewed public primary documentation only. It did not authenticate against a tenant, validate a live token, benchmark dependencies, inspect private site configuration, or demonstrate cross-platform keyring operation. Those checks are acceptance gates in the implementation plan.
