# Agent identity, billing, and human team models

Checked **8 November 2026** against current primary sources. Prices, eligibility, and preview features can change. This report compares documented billing mechanisms rather than estimating the cost of a particular deployment. Sources are in [the operating-model register](operating-models-sources.md).

## 1. An agent does not have to belong to one employee

Three supported arrangements appear in the reviewed products:

| Arrangement | Human relationship | Execution and access model | Concrete evidence |
| --- | --- | --- | --- |
| Personal automation | A person owns the saved task and connected accounts | Uses their account's access and usage | Claude Code scheduled cloud tasks. [O29] |
| Shared team agent | Several people invoke or steer a common agent | Admin-managed identity and tool access | Claude Tag channel work. [O32] |
| Queue of temporary workers | Human owner is attached to the issue/change | Scheduler starts sessions as work arrives | Symphony and Stripe's task-run approach. [O01, O41] |

These are not mutually exclusive. A team can use personal assistants during exploration and a shared service for admitted implementation work. The human owner of a change need not be the owner of every credential used to implement it.

Linear explicitly keeps the human assignee while an agent is the delegate. In permitted teams, multiple users can work with an installed agent. This is evidence for shared task delegation, not a requirement to purchase a separate agent identity for each human. [O05, O08]

OpenAI documents non-human workspace service accounts for headless Codex workflows, with management delegated to people/groups. They are currently available only on pay-as-you-go plans, and their plugin configuration is separate from the creator's. [O35]

Do not confuse an agent role, an agent identity, and an agent run. “Security reviewer” is a role definition; `review-service` can be a principal; attempt 18 is a run. None requires a new employee seat by definition. The particular product's licensing still governs actual charges.

## 2. Authentication, authorization, and attribution

For this research, `auth*` includes all three:

- **Authentication:** which principal is making the request?
- **Authorization:** what resources and operations may that principal use?
- **Attribution:** which human request, agent run, and cost center caused the action?

Audit a proposed factory across five credential boundaries:

| Boundary | Typical mechanism | What must remain identifiable |
| --- | --- | --- |
| Human to factory | SSO/session, team membership, role checks | Requestor, owner, decision maker |
| Factory to tracker | OAuth app, service principal, scoped API token | Application/workspace and permitted teams/projects |
| Factory/worker to model | API project identity, supported workspace account, provider IAM | Billing project, model, run and attempt |
| Worker to repository | GitHub App installation or scoped machine-user token | Repositories, branch/PR permissions, actor |
| Runtime to infrastructure/tools | Workload identity, short-lived token, brokered credential | Workload, destination, granted operation |

An `AGENTS.md` rule does not enforce these boundaries. An agent name in a log does not establish a distinct principal. A Git commit author string does not establish who authorized the push.

### Documented implementations

**Linear:** app-mode OAuth installs an agent identity rather than impersonating the installer. Scopes govern capabilities, and admin-controlled team access limits where it participates. Account-linking signals can request a separate downstream connection. [O05, O07]

**Native Codex for Linear:** current official documentation also describes automatic triage delegation. For that path, Codex runs using the issue creator's account. A shared tracker and app identity therefore do not imply organization-service-account inference. This differs from building a dispatcher with a separately configured Codex service identity. [O09]

**Symphony:** its reference implementation executes tracker tools with host-side authentication and removes declared tracker-token variables from the Codex child environment. That separation avoids a duplicate tracker login; model and repository authentication remain separate concerns. It is not a complete credential-isolation audit. [O04]

**Claude Agent for Jira:** setup uses a Managed Agents API key plus a GitHub token for a machine account. The documented installation stores credentials in encrypted Forge storage and creates hosted agent sessions, with draft PRs from the configured service account. This is a concrete organization-controlled integration, not a personal Claude Code login flow. [O12]

**Vercel Open Agents:** the inspected application configures Vercel sign-in separately from GitHub App access. Its sandbox and controlling workflow are separate components. The reference application's identity and sharing design needs evaluation before treating it as a multi-user factory. [O25]

**Microsoft Entra Agent ID:** the source documentation defines agent identities using specialized service principals, with blueprints obtaining tokens and a human/group sponsor for accountability. This supplies an identity foundation, not an issue scheduler. [O43]

**Google Agent Identity:** current documentation separates initiating user, deployed agent, and delegated authority, with credential-brokering mechanisms. It is distinct from Jules account authentication and does not imply that every Jules session uses the enterprise platform's identity model. [O44]

**OpenAI workload identity federation:** documented API/Codex flows exchange an existing trusted workload identity for short-lived access. API project service accounts and managed ChatGPT workspace principals are different targets. Eligibility and administrator setup must be checked for the chosen deployment. [O37]

## 3. Shared agent versus acting as the user

Claude Tag's June identity article explains a shared-channel model with admin-connected tools and channel/workspace scope. This addresses the ambiguity of several people steering one agent. [O32]

Its September update adds an important qualification: a person can invoke their own connectors in a channel, with controls over publishing the result. Those personal connectors do not run unattended; scheduled work uses admin-attached shared connectors. Therefore “Claude Tag uses an agent identity” is incomplete without specifying which connector and trigger are in use. [O33]

Claude Code scheduled tasks are different. The current documentation says they belong to an individual account, are not shared with teammates, and use that person's connected identities. Do not model them as an organization-owned worker pool merely because they run in the cloud. [O29]

Our recommendation is to persist `requestor`, `accountable_owner`, `executing_principal`, and `approver` separately. Check who may steer a shared run: being able to read a ticket should not automatically grant permission to approve a deployment or expand repository access.

## 4. Billing models actually documented

| Surface | Billing mechanism verified | Implication |
| --- | --- | --- |
| Linear third-party agent | Installed agents are not billable Linear seats; provider charges can apply. [O08] | Task UI pricing and execution cost are separate |
| Codex local API-key execution | API pricing under the OpenAI Platform account, rather than included ChatGPT usage. [O36] | Use the appropriate project/cost attribution |
| Codex workspace service accounts | Eligible pay-as-you-go workspace model; distinct from Platform service accounts. [O35] | A non-human principal need not depend on an employee account |
| GitHub Copilot organization/enterprise usage | Token-priced AI credits; included pools depend on plan; app-installation requests can bill the installing organization. [O38] | Do not estimate current cost from historical per-session premium-request articles |
| Jira Rovo orchestration | Organization Rovo credit model; current page says allowance enforcement begins 3 December 2026. [O10] | This future date is after the research cutoff |
| Rovo Dev | Separate per-user Rovo Dev allowances and extra-usage policy. [O46] | Rovo and Rovo Dev credits are not interchangeable |
| Claude Tag Enterprise | Channel work uses organization consumption with Tag/channel limits; DMs use the sender's seat. [O34] | Shared-channel cost controls differ from individual limits |
| Claude SDK / Managed Agents | API credentials and provider billing; chosen API has its own usage surfaces. [O30, O47] | Persist actual usage and billing project |
| Jules | Task/concurrency quotas by plan; inspected upgrade page limits paid plans to individual Google accounts. [O22] | Enterprise pooling cannot be assumed from API availability |
| Vercel Sandbox | CPU, memory, creation, network, and storage meters, with regional/plan distinctions. [O28] | Model tokens are only one cost component |
| Stripe / Meta internal systems | Public engineering articles do not disclose enough for a complete internal allocation model. [O40–O42] | Do not invent cost per agent or employee |

The Jira row describes the generic orchestration surface. A specific third-party coding integration can add its own provider charges. Verify the full chain for that integration rather than assuming Rovo credits pay for all external model and compute work.

Vercel's pricing documentation distinguishes active CPU from provisioned memory: waiting can avoid CPU charges while memory still accrues. Releasing or hibernating unused compute matters even in a workflow that pauses durably. [O28]

### Claude Code embedding versus the SDK

Current Claude SDK documentation directs third-party developers toward supported API authentication rather than offering claude.ai login/rate limits without prior approval. Its commercial framework is different from simply running a personal subscription in a shared service. [O30]

The Claude Code binary's separate embedding conditions say end users authenticate with their own permitted credentials and restrict intermediating their usage unless otherwise agreed. Distinguish an internal automation project from a product offered to external customers, and distinguish the binary from the SDK. Those distinctions can change the feasible commercial model. [O39]

## 5. Proposed ownership model

For this project, use an organization-owned worker service with per-change human accountability. This is a recommendation, not a claim that every studied company operates identically.

| Role | Responsibility |
| --- | --- |
| Requestor | States the outcome and supplies context |
| Change owner | Owns scope, priority, and acceptance; resolves who decides |
| Architecture/domain reviewer | Settles consequential shared design decisions |
| Implementation workers | Execute bounded work orders with scoped access |
| Evaluation/review workers | Inspect behavior and code independently |
| Integration/release owner | Owns merge/deployment policy and operational acceptance |
| Platform/security owner | Owns identity, permissions, environments, and auditability |
| Budget owner | Sets allocation, reservations, and escalation policy |

Several people can coordinate a shared pool, but conflicting instructions need a rule. Persist material decisions, their authorized actor, and their effective spec version. Workers should not resolve a disagreement between product owners by following whichever comment arrived last.

Separate reviewer independence from provider identity. A different model with the same incomplete acceptance criteria may reproduce the same error. Use different evidence and objectives as well as separate sessions where useful.

## 6. Proposed cost ledger and controls

Persist at least:

```text
organization_id, project_id, change_id, task_id, attempt_id
requestor_id, accountable_owner_id, executing_principal_id
provider, model, runtime_session_id, billing_account_id
input_tokens, cached_tokens, output_tokens, reported_cost
compute_meter_refs, CI_meter_refs, artifact_storage_refs
reserved_budget, actual_cost, currency, pricing_version
outcome, accepted_revision, abandonment_reason
```

Store estimates and reconciled charges separately. Provider sessions can report cumulative usage across resumes; bill each increment once. Include failed attempts, review passes, test infrastructure, and idle resources. Billing APIs can lag, so reserve spending capacity before dispatching concurrent workers and reconcile later.

Define limits for a run, change, project, and organization. Distinguish a warning threshold from an enforced cap. Allow a small bounded finalization path where needed to save diagnostics, but do not let “cleanup” become an unlimited extra run.

Report cost per accepted outcome and human active time alongside raw token totals. Purchasing more seats or adding more agents does not itself measure engineering value.

## 7. Deployment and cloud environment choices

Cloud execution is well represented in the reviewed examples: Stripe devboxes, Vercel sandboxes, Jules, Managed Agents, and Claude scheduled sessions. It is not mandatory for a small factory; self-hosted workers can use the same work-order contract.

Keep durable state and decisions independent from disposable compute. Isolate each writing task, broker only required credentials, identify the resulting artifact, and retain enough evidence to reproduce a failed run. Worktrees isolate edits; they are not a substitute for OS, network, or identity boundaries.

No cloud account, credential, paid session, or subscription was created or modified in this research. Actual deployment must use the chosen organization's available identity and billing configuration.
