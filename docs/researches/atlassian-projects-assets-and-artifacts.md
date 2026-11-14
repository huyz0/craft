# Atlassian Projects, Assets and Artifacts for Craft

Research date: **9 November 2026**, Australia/Sydney. Projects follow-up verified against official documentation on **10 November 2026**. The product overview, Projects API/auth/search, availability, Artifacts overview, Assets schemas and Teamwork Graph CLI overview were rechecked for the latest follow-up. These are evaluated extension candidates. No new Craft cloud capability is implemented or advertised by this report.

## Recommendation

Use **Projects** for shared human ownership and initiative updates. Evaluate **Artifacts** first for publishing AI-generated files. Use **Assets** when the factory needs a structured catalog of runs, repositories, environments and artifact relationships. Keep executable work in Jira and the reviewed plan/design in Confluence.

Also evaluate Atlassian's **Teamwork Graph CLI (`twg`)** before expanding custom adapters. Its documented coverage overlaps much of Craft's proposed provider scope. Craft can still supply reviewed-plan materialization, strict contracts, policy and recovery where the vendor interface does not meet the requirements. Reuse must follow an evidence-based compatibility check; this report does not select a subprocess backend or change the current implementation contract.

| System | Proposed responsibility in the factory | Qualification still needed |
| --- | --- | --- |
| Atlassian Projects | Purpose, accountable human/team, health, risks, decisions and stakeholder updates | Discover actual project identifiers; prove service access and exact API fields |
| Jira | Bounded execution tasks, dependencies, assignees and transitions | Existing provider/auth acceptance work |
| Confluence | Plan draft, architecture/design, human discussion and revision-bound review | Existing reviewed-plan acceptance work |
| Atlassian Artifacts | Shareable reports, diagrams and other generated files | API/tool contract, team ownership, retention, export and service identity |
| Assets | Typed catalog of runs, artifacts, repositories and environments | Workspace/schema access, auth mode, quotas and mutation semantics |
| Git and CI/object storage | Versioned code, machine evidence and retained payloads | Retention, integrity and access policy for each store |

The responsibility assignments in this table are our design recommendations, not vendor guarantees.

### Adoption decision for Craft

**Decision: yes, use Projects as an optional shared initiative view for Craft.** The documentation supports the fit; live integration remains unqualified. The checks below distinguish those conclusions.

| Question | Documentation checked on 10 November 2026 | Remaining evidence |
| --- | --- | --- |
| Can several humans coordinate the initiative? | One owner, individual/team contributors and followers; contributors can edit details and publish updates. | Confirm the intended team's access on the test site. |
| Can an unattended integration access it? | The product-specific API guide documents service-account client credentials and Projects scopes. | Prove token issuance, actor identity, target access and operation-specific scopes. |
| Can Craft avoid building the entire adapter? | TWG CLI explicitly lists Projects lifecycle management and the other proposed Atlassian providers. | Check exact commands, credential storage, output bounds and recovery before choosing reuse. |
| Can it coordinate coding workers on separate machines? | The checked guides establish initiative communication, links and updates. | They do not establish atomic task claims, leases, fencing or durable worker delivery. Keep these in the factory coordinator. |

Sources for this decision: [project roles and purpose](https://support.atlassian.com/platform-experiences/docs/what-is-a-project/), [Projects authentication and scopes](https://developer.atlassian.com/platform/projects/projects-graphql-api/using-projects-graphql-api/), and [TWG CLI coverage](https://support.atlassian.com/organization-administration/docs/what-is-twg-cli/). No tenant request or mutation was made in this source check.

Projects is useful in two places: coordinating the humans building Craft, and providing an optional initiative view for teams using Craft. For the first, a linked project and manually reviewed updates are enough to start. For the second, a qualified provider can read initiative context and publish reviewed summaries. Neither use requires moving the Confluence plan draft or duplicating Jira tasks.

Keep Projects optional while the core CLI is delivered. Add the integration when a team needs cross-team visibility, an accountable owner and a regular update audience. Continue using the factory coordinator for worker claims, machine-to-machine handoffs, retries and approval binding. The checked Projects documentation establishes communication and lifecycle features, but does not establish those execution guarantees.

Before choosing a direct adapter, compare the documented Projects support in TWG CLI with Craft's required credential storage, bounded responses and write recovery. If a direct integration is needed, qualify project discovery and reads first, then one human-reviewed update. Keep Atlassian Projects identities and commands distinct from the existing Jira project-context candidates. This is a research recommendation; no Projects commands have been implemented by this follow-up.

## What Projects is

### Follow-up source check, 10 November 2026

The current official product, API, search, availability and TWG CLI guides were reopened for this follow-up. They support the recommendation above: Projects can be Craft's optional shared initiative view, with a human owner and team contributors, linked to the reviewed plan and execution records. This check did not exercise the tenant or implement a provider.

Older search results need care. A January 2026 API announcement describes API-token-only authentication, and an unresolved public suggestion still says no APIs exist. The current [product-specific API guide](https://developer.atlassian.com/platform/projects/projects-graphql-api/using-projects-graphql-api/) explicitly documents service-account client credentials and per-operation scopes. Use that guide for the proposed integration, then verify the configured tenant and principal. Neither an older announcement nor an app appearing in navigation establishes the access available to Craft.

The [TWG CLI coverage guide](https://support.atlassian.com/organization-administration/docs/what-is-twg-cli/) also explicitly lists Projects. Evaluate its actual read and update contracts before adding a custom adapter. Coverage alone does not establish compatibility with Craft's credential, output-budget or write-recovery requirements.

Atlassian Projects is separate from the Jira collection of work items, now called a space in the referenced documentation. It communicates what an initiative is, why it matters, who is responsible and how it is progressing. Projects can connect to higher-level Jira work. It is intended for regular updates rather than detailed task execution. [Product explanation](https://support.atlassian.com/platform-experiences/docs/what-is-a-project/).

This suits a team of humans coordinating agents: humans retain initiative ownership, while agents prepare evidence-linked updates from Jira, CI and reviewed artifacts. A project should correspond to a meaningful delivery initiative rather than every individual agent run. Keep approval requests and decisions linked to their authoritative records. A green project status should never authorize deployment or substitute for code review.

### Availability and built-in AI assistance

The availability guide says Projects and Goals are platform apps included at no additional cost, with organization administrators controlling user access. The earlier test-site inventory observed Projects Free, but that does not establish service-account access or every optional feature. [Availability and access](https://support.atlassian.com/platform-experiences/docs/goals-and-projects-app-availability/).

Rovo can already prepare project and goal status updates from connected-app activity. The documentation labels this a beta with gradual rollout from October and requires AI activation for the relevant apps. A person reviews the generated content before saving a draft or publishing. Project templates include delivery progress, risks and blockers, or a this-week/next-week structure. This is a useful existing human-review flow to evaluate before building another update composer. It does not establish a programmable Rovo drafting endpoint or unattended execution support. Actual eligibility on the test site remains unverified. [Rovo status drafts](https://support.atlassian.com/platform-experiences/docs/create-project-updates-with-rovo/).

For adoption, keep Projects optional: a team should be able to review a Confluence plan, create Jira work and run coding workers without first creating an Atlassian Project. When enabled, link the authoritative records and publish a compact initiative summary rather than maintaining another copy of every task. This is a proposed Craft boundary, not implemented behavior.

### API and authentication

The product-specific GraphQL guide specifies `https://{subdomain}.atlassian.net/gateway/api/graphql` and explicitly supports service-account OAuth client credentials. It documents read/access, project-write, update-write and deletion scopes. Both token scope and principal permission apply. IDs use Townsquare ARIs. Queries can resolve some fields while returning permission errors for others; missing permission must not become “no data.” Its example qualifies the `updates` field with experimental `@optIn(to: "Townsquare")`. [Projects API guide](https://developer.atlassian.com/platform/projects/projects-graphql-api/using-projects-graphql-api/).

The API introduction describes project metadata, contributors/watchers, updates and goal links. Consult the schema for each exact operation rather than inferring CRUD parity from the UI. [API introduction](https://developer.atlassian.com/platform/projects/projects-graphql-api/introduction/), [schema](https://developer.atlassian.com/platform/projects/apis/schema/).

Proposed Craft integration:

- Separate the Projects provider namespace from Jira project/space commands.
- Start with identity/access discovery, project reads and a local status-update preview.
- Compile typed GraphQL operations and variables with bounded server field selection. Do not add an arbitrary authenticated-query escape hatch.
- Treat HTTP status, top-level GraphQL errors, mutation payload errors and expected result identity as separate checks. HTTP 200 alone is insufficient.
- Preserve discovered ARIs. Do not construct a Projects site identifier from the Jira Cloud ID without evidence.
- Qualify experimental fields explicitly. Bind continuation to the target, query, variables and projection using Craft's existing cursor contract.
- For writes, require the normal apply/journal controls and operation-specific reconciliation. Do not replay an unknown update merely because a subsequent list omits it.

Generic Atlassian GraphQL references can describe a different gateway/auth context. Follow the product-specific route for the qualified mode; never try alternate credential destinations automatically after authentication fails.

### Team ownership and the plan-draft workflow

Projects supports one accountable owner, individual or team contributors, and followers. Contributors can edit details and publish updates. This supports several humans coordinating shared agent workers without assigning every worker permanently to a human. That worker model is our proposed architecture, not a Projects execution feature. [Project roles](https://support.atlassian.com/platform-experiences/docs/what-is-a-project/).

For Craft itself, use an initiative such as “Deliver Craft cloud workflows” with a human owner. For a factory using Craft, create a project per delivery initiative when shared visibility is useful. A small team can defer Projects until it needs that additional coordination view.

### Suggested first project for Craft

Use one optional initiative, **Craft cloud workflows**, with the accountable human as owner and collaborating humans/team as contributors. Its description should state the outcome: agents can read cloud context and carry out reviewed, recoverable work through a bounded CLI. Link the Confluence plan page, implementation repository when available, Jira execution work and evidence records. These are proposed contents, not a project created by this research.

Keep the update short: delivered outcomes with evidence links, current blockers, decisions needed from humans, and the next intended outcome. Prepare the update from authoritative records and review it before publication. Do not copy credentials, raw agent transcripts or every task into the initiative. Projects supplies the shared communication view; Craft's coordinator still needs execution claims, recovery state and revision-bound approval.

Adopt Projects after a read-only probe establishes the intended service actor, project identity and access. Then qualify one reviewed update on an owned test project. Continue the existing Confluence-first planning flow independently while that integration is evaluated.

The product guide also documents syncing an initiative with a Jira epic for name, start date, target date and tags/labels. Prefer that supported synchronization where it fits, rather than maintaining those fields twice through Craft. This does not establish synchronization of task completion, approval, or worker state. [Project and epic relationship](https://support.atlassian.com/platform-experiences/docs/what-is-a-project/).

The current Jira linking guide distinguishes an **Atlassian Project field** from an **Atlassian Projects section**. Inspect the site's actual experience before prescribing setup. It permits linking any work type, but synchronization applies to the parent-level type, and the documented section-based flow allows only one linked parent-level work item per Atlassian project. For a Craft initiative spanning several epics or repositories, keep additional references as related links and prepare an evidence-linked summary; do not assume automatic aggregation across them. [Jira linking and synchronization limits](https://support.atlassian.com/platform-experiences/docs/link-goals-and-projects-to-work-in-jira/) (checked 10 November 2026).

Recommended sequence:

1. Link the initiative to its Confluence plan page, repositories and intended outcome. Projects supports links to goals, related projects, task trackers and documents. [Related work](https://support.atlassian.com/platform-experiences/docs/link-projects-to-related-work-and-goals/).
2. Agents write the proposed breakdown, design, assumptions and unresolved questions into the Confluence page. This is a normal page containing a plan draft, matching the user's intended workflow.
3. Humans review the page. Craft records approval against its exact revision and planned ticket content. Later edits require renewed review of the changed content.
4. After approval, Craft materializes Jira tickets and records the mapping back to the reviewed plan. Recovery resumes from confirmed receipts rather than creating duplicate tickets.
5. Coding workers on separate machines claim bounded tasks through the factory coordinator. They exchange durable task notes, context references, artifacts and results. Projects presents the initiative summary; it does not provide the claim/lease protocol.
6. Agents prepare concise status updates with completed outcomes, evidence links, blockers and decisions needed. A human reviews stakeholder communication according to policy. Blocking decisions also enter the factory's immediate escalation path rather than waiting for a weekly digest.

This leaves one authority for each concern: the reviewed plan revision, Jira task state, the coordinator's execution claims, repository/CI review evidence and Projects initiative communication. Project comments or health changes do not themselves approve code or production access.

### Bounded provider surface

The following are capability candidates, not implemented Craft commands or a settled command syntax.

| Candidate | Evidence and design boundary |
| --- | --- |
| Search/list and get | Official examples use `projects_search`, `projects_byId` and `projects_byIds`. |
| Create and edit/archive | Official examples use `projects_create` and `projects_edit`. |
| Publish an update | `projects_createUpdate` accepts status and structured content; its payload includes success, errors and update identity. |
| Membership, links, comments and deletion | Documented capabilities/scopes exist, but qualify each exact schema and recovery contract before advertising it. |

[API examples](https://developer.atlassian.com/platform/projects/projects-graphql-api/using-projects-graphql-api/), [capability introduction](https://developer.atlassian.com/platform/projects/projects-graphql-api/introduction/).

Search supports filters such as owner, team, status and phase, sorting, and Relay pagination through `first`, `after`, `endCursor` and `hasNextPage`. The documented search-clause budget rejects cost **35 or above**; this is distinct from an output-token budget. [Search and pagination](https://developer.atlassian.com/platform/projects/projects-graphql-api/search-queries/).

Craft should compile validated filters and requested fields, preserve any unreturned page remainder, and wrap provider cursors with its normal binding. Nested connections need their own bounds. The checked documentation does not establish snapshot isolation or worker leases. Start initiative refresh with explicit reads or bounded polling; qualify an event subscription separately before relying on it. GraphQL POST queries remain semantically read operations, while mutations require write controls. Craft's [transport contract](../../cli/docs/transport.md) already allows declared POST searches; Projects still needs its own qualified operation declarations and target/auth handling.

### Access and audit limits

Open projects are visible to people with Projects access. Restricted projects can grant view/edit access to selected people, teams and groups. Progress can roll up to open goals; updates connected to a private Slack channel become visible to that channel's members. Check the audience before copying sensitive context. [Project access](https://support.atlassian.com/platform-experiences/docs/restrict-who-can-view-and-edit-projects/).

The platform compliance page lists SOC 2 and ISO 27001 but uses early-access wording and says viewer-only roles are unsupported, whereas the current project-access guide describes Can view access. Treat that as a documentation scope/version discrepancy requiring tenant qualification. Neither page establishes Craft's independent code approval, segregation of duties, SOX change control or immutable evidence retention. Preserve those in the factory/repository controls and qualify vendor assurance coverage separately. [Compliance support](https://support.atlassian.com/platform-experiences/docs/platform-experiences-compliance-support/), [review research](code-review-soc2-sox.md).

## Assets and Artifacts serve different needs

### Assets: structured catalog

Assets schemas organize object types, attributes, objects and references. This makes Assets a candidate for modeling relationships rather than just storing files. [Schemas](https://support.atlassian.com/assets/docs/what-are-object-schemas/), [attributes](https://support.atlassian.com/assets/docs/what-are-attributes/).

The current schema guide says Assets has moved to the platform and is accessible through the Jira app switcher. That explains its presence beside other apps; it does not establish this tenant's workspace access, quotas or suitability as a binary artifact store. Each schema still has its own permissions. [Platform placement and schema access](https://support.atlassian.com/assets/docs/what-are-object-schemas/).

The [Assets overview](https://support.atlassian.com/assets/docs/what-is-assets-in-jira-service-management-cloud/), rechecked on 10 November 2026, confirms the platform move and describes user-defined objects, attributes and references. It also announces usage-based billing starting **3 December 2026**. Verify the site's included allowance and forecast before adopting a run catalog. For Craft, index retained work products and useful run summaries; keep high-volume worker events in the execution journal or telemetry store. That storage split is our recommendation, not a vendor restriction.

The REST workflow discovers a workspace through `/rest/servicedeskapi/assets/workspace` and uses a workspace-specific gateway under `https://api.atlassian.com/ex/jira/{cloudId}/jsm/assets/workspace/{workspaceId}/v1/`. The workflow page describes basic authentication; the separate scope guide documents OAuth 3LO and Forge. Those references do not establish service-account 2LO support for Craft. [Workflow](https://developer.atlassian.com/cloud/assets/assets-rest-api-guide/workflow/), [OAuth scopes](https://developer.atlassian.com/cloud/assets/assets-rest-api-guide/scopes-for-oauth-2-3LO-and-forge-apps/).

The object reference provides create, get, update, delete, history and AQL search. AQL search has `startAt`, `maxResults` and `includeAttributes`; requesting a separate total count can add latency. Object payloads depend on actual object-type and attribute IDs. Read and write object scopes are distinct. [Object API](https://developer.atlassian.com/cloud/assets/rest/api-group-object/).

Our proposed catalog would contain `AgentRun`, `WorkProduct`, immutable-by-policy `ArtifactVersion`, `Repository`, `Environment` and `ReviewEvidence`, with owner, source revision, URI, digest and relationships. “Immutable-by-policy” is a factory rule requiring enforcement, not a claimed Assets storage guarantee. Keep payloads in the appropriate versioned store and use Assets to index them. Mutable statuses do not provide approval authority, distributed leases or fencing.

Actual workspace/schema permissions, service OAuth, attachment behavior, binary limits and quotas remain unverified. Do not transfer a Jira or Marketplace attachment limit to Assets.

### Artifacts: generated files

Atlassian also has a distinct **Artifacts** product. Its documentation describes a beta available on paid plans for files produced by Rovo and connected agents including Claude and Codex. Files can be shared and embedded using Smart Links. Sharing makes them available to Search and Teamwork Graph; they are private by default. [Artifacts overview](https://support.atlassian.com/platform-experiences/docs/what-is-atlassian-artifacts/).

The creation guide directs third-party agents through Atlassian MCP or Teamwork Graph CLI and recommends explicitly requesting a save to Artifacts. It also supports manual uploads. Interactive artifacts must fit into a single file, and browser features such as local storage or some JavaScript functionality are restricted. Check third-party-created file visibility rather than assuming the desired sharing policy. [Creation guide](https://support.atlassian.com/platform-experiences/docs/add-an-artifact/).

The management guide describes version history, download, embedding and permanent deletion without restoration. These are product capabilities, not proof of an immutable audit archive. [Management guide](https://support.atlassian.com/platform-experiences/docs/manage-an-artifact/).

Artifacts is therefore a closer candidate for the user's “AI work assets” than CMDB Assets when the deliverable is a report, diagram or interactive file. Before adoption, establish actual tool names/schemas, upload/download bounds, revision identity, digest preservation, ownership transfer, team sharing, retention/export and supported unattended identities. This research did not establish a supported direct REST upload route. Migration and Marketplace APIs named “artifacts” refer to different products.

## Existing vendor CLI changes the build-versus-reuse decision

Atlassian documents **Teamwork Graph CLI (`twg`)** for coding agents, including an agent-skill installation step. Listed coverage includes Projects, Goals, Assets, Jira, Confluence, Bitbucket and Trello, alongside permission-aware connected context. This substantially overlaps Craft's proposed cloud resource adapters. Product coverage alone does not establish every endpoint or recovery behavior. [TWG CLI overview](https://support.atlassian.com/organization-administration/docs/what-is-twg-cli/).

The setup guide provides Windows, macOS and Linux installation, diagnostics and companion skills. We did not install it, execute the installer, authorize OAuth, install skills or test commands. [Setup guide](https://support.atlassian.com/organization-administration/docs/get-started-with-twg-cli/).

The auth guide documents interactive OAuth 2.1, refresh, separate Bitbucket credentials and an upkeep helper. Existing installations use a local credential file; opt-in experimental encrypted storage uses a database with a separate key source. That differs from Craft's native-keyring-only secret storage requirement. TWG's supported OAuth flow is not evidence that Craft can use the same registration or flow for its direct Jira/Confluence client. [Authentication and storage](https://support.atlassian.com/organization-administration/docs/how-does-teamwork-graph-cli-authentication-work/).

Admins can configure view, write/manage and delete categories with server-side enforcement and session revocation. The guide lists “allow all” defaults. Existing account permissions also matter. These controls do not establish independent human approval of a specific reviewed plan revision. [Permission controls](https://support.atlassian.com/organization-administration/docs/configure-teamwork-graph-cli-permissions/).

Evaluate vendor reuse against actual fixtures and sandbox results:

1. Read-only Projects discovery, exact identity/target binding and partial-error reporting.
2. Artifact upload/download round trip with content digest, sharing and revision checks in an owned test resource.
3. Strict input, schema discovery, selected fields, bounded output and lossless pagination.
4. Native credential policy, unattended service identity, refresh locking and diagnostic redaction.
5. Write preview, immutable request binding, unknown-outcome recovery and independent review enforcement.

Potential outcome: use vendor tools for qualified context/artifact capabilities and retain Craft's factory workflow. Another outcome is a narrow typed adapter where the vendor tool fails a required contract. Do not route raw agent text or arbitrary command arguments through a generic credential-bearing subprocess.

## Sandbox evidence and limits

Earlier read-only inspection of the authorized `craft-test.atlassian.net` test site found Jira and Confluence Standard entries, Projects Free and Goals Free, and a reachable service-account administration screen. No Projects API, Assets workspace or Artifacts upload was exercised. See the [non-secret inventory](../../cli/docs/evidence/tenant-auth-inventory.md).

The recommended next experiment is read-only vendor/direct API qualification, followed by an explicitly scoped owned-resource artifact test. No new project, credential, catalog schema, page, ticket or artifact was created for this research. The existing Rust auth work remains partial and does not establish live Projects or Artifacts support.
