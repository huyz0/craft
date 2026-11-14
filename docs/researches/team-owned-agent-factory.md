# A factory operated by several humans

Research date: **8 November 2026**. This answers whether a team can share agents, work state, knowledge, and outputs, rather than each person managing an isolated agent project. It complements the [context study](context-artifacts-and-agent-communication.md) and [identity study](identity-billing-and-team-models.md).

## Answer

Yes. A documented setup is an organization-managed agent service connected to the team's tracker and communication channels. Several humans delegate and review work through shared records. Execution identity, task ownership, memory scope, and authority to approve remain explicit.

Three capabilities are easily confused:

1. A team subscription with separate personal agents and central billing.
2. Shared project knowledge that people use in separate conversations.
3. Shared agent execution where several people can direct, inspect, and review the same work.

The third is closest to the requested factory. Buying team seats does not, by itself, establish it.

## Concrete product examples

### Factory: an organization-managed execution setup

Factory Delegations documents shared requests from Slack, Linear, Jira, and Microsoft Teams. Administrators configure a common agent profile with instructions, plugins, a service-account identity, and remote compute. Channel and issue delegations use that shared identity; personal DMs use the sender's identity. This directly supports several humans delegating into an organization-managed service. The documentation does not establish that every session is jointly editable by all team members. [T1]

This is the clearest documented match in this follow-up for the requested shared execution model. Evaluate visibility, access, knowledge ownership, and review behavior separately before adoption.

### Devin: collaboration on runs, with an important memory boundary

Devin supports team organizations. Its Slack integration lets a session synchronize bidirectionally with a thread, and dedicated code channels expose session work and accept messages. This provides a shared interaction surface for team members rather than requiring one person to relay everything. [T2, T3]

However, Devin's automatic Memory is explicitly personal within each organization, not shared with teammates. Its documentation separates that from deliberately maintained skills, whose scopes can include repositories and organizations. Shared access to an agent service therefore does not imply a common learned memory. [T4]

### Claude: distinguish shared chat projects from the coding coordinator

Claude's Team/Enterprise project-sharing documentation allows shared knowledge and instructions with view/edit permissions. Conversations remain private unless shared, and chat sharing uses snapshots. This supports shared reference context, but does not establish a jointly operated live coding coordinator. [T5]

The redesigned Claude Code Projects announcement describes a coordinator, parallel cloud threads, shared memory, and an artifact library. Its 17 September 2026 announcement placed Team/Enterprise availability later in the rollout. That announcement does not establish current multi-human project control. The specific coding-project documentation could not be retrieved in this follow-up, so this study does not assert its present sharing limits from secondary descriptions. [T6]

Do not apply the ordinary chat-project sharing documentation to the new coding coordinator without verifying that exact surface. Similarly, the earlier study's description of Cursor shared context established sharing among its agents, not proof that multiple humans could jointly control the same coordinator.

## Recommended team arrangement

Use an organization-owned project space, task ledger, and artifact store. Humans retain their own accounts. Agents run under either an explicitly delegated personal identity or an appropriately restricted service identity. Preserve both the requester and execution identity.

| Shared resource | Proposed owner | How humans participate |
| --- | --- | --- |
| Product intent and acceptance criteria | Product owner | Propose work, settle scope, accept behavior |
| Architecture and contracts | Technical owners | Record decisions and approve consequential interface changes |
| Task ledger and dependencies | Delivery owner/coordinator | Prioritize, assign, unblock, cancel, and inspect status |
| Project knowledge | Named maintainers | Review useful lessons and retire stale guidance |
| Execution runs | Agent service with accountable task owner | Inspect evidence, steer within authority, request repairs |
| Code approval | Eligible independent reviewers | Review the candidate revision under repository controls |
| Release | Controlled deployment service | Release approved artifacts and record outcomes |

These are roles, not necessarily seven different people. Define incompatible duties using the [regulated-review controls](code-review-soc2-sox.md).

For example, a product manager submits a task, an architect records an interface decision, and an agent implements it on a cloud worker. Another engineer requests a repair. An eligible reviewer approves the final candidate, then the pipeline releases it. Everyone refers to the same task, decision, artifact, and evidence records. The work survives the original requester being unavailable.

### Handle competing human instructions

A shared chat needs authority rules. A comment from another teammate should not silently override an approved requirement or expand a worker's permissions. Record changes to scope and decisions in the task or decision log. Attach the person's identity, reason, and applicable revision. Define who can reprioritize, cancel, accept, or approve work.

Use one accountable owner for each task at a time, while allowing several people to contribute. Ownership transfer should be explicit. Notify people about blockers and required decisions through the team's chosen channels, with links to durable records.

### Separate personal memory from team knowledge

Keep communication preferences and exploratory personal notes separate from shared decisions. Promote a useful agent lesson into team knowledge only after checking its evidence and scope. Give shared entries an owner and source, and invalidate them when relevant contracts or implementations change.

The team knowledge base can live in Git, Confluence, or another controlled project space. The tracker records work state; the artifact store retains concrete outputs; the agent retrieves the relevant slices. A single large memory file is not required, and a shared document is not a substitute for reliable task claiming.

## Selection guidance

- Evaluate **Factory Delegations** when shared service identity and organization-configured execution are the priority.
- Evaluate **Devin with shared Slack threads/channels** when several people need to collaborate around active coding runs; design shared knowledge separately from personal automatic memory.
- For a **Codex/Claude execution layer behind Jira or Linear**, reuse the team's shared specs, decisions, and review controls. Add an orchestrator that claims tasks and publishes evidence. This is a proposed integration design, not a claim that installing the CLI creates a complete team factory.
- Use **shared Claude chat projects** for team reference knowledge where appropriate. Verify new coding-project membership and control capabilities separately.

## Sources and limits

All opened sources are primary documentation. No product was installed or tested, and no claim is made about industry adoption frequency.

| ID | Source | Use |
| --- | --- | --- |
| T1 | [Factory Delegations](https://docs.factory.com/delegations) | Shared versus personal execution, configuration, and identity |
| T2 | [Devin: Invite your team](https://docs.devin.ai/product-guides/invite-team) | Organization membership |
| T3 | [Devin: Slack](https://docs.devin.ai/integrations/slack) | Shared interaction with a coding run |
| T4 | [Devin Memory and Dreaming](https://docs.devin.ai/product-guides/memory) | Personal memory versus reusable team procedures |
| T5 | [Claude project visibility and sharing](https://support.claude.com/en/articles/9519189-manage-project-visibility-and-sharing) | Shared reference projects and private conversations |
| T6 | [Claude Projects redesigned](https://claude.com/resources/articles/projects-redesigned) | Dated coding-project announcement; not current sharing verification |
