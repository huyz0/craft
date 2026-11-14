# How teams operate an AI software factory

Research checked **8 November 2026**. This follow-up covers work tracking, outer loops, human coordination, agent identity, billing, and execution. Source IDs refer to [the operating-model source register](operating-models-sources.md).

**The strongest match to the requested system is OpenAI Symphony: a tracker-driven service around Codex.** Linear provides native agent delegation and human interaction primitives. Jira has coding-agent integrations. For a custom application, Microsoft Agent Framework and Vercel's agent stack provide workflow infrastructure. These occupy different layers; none automatically reconciles every spec, design decision, ticket, and deployed artifact.

This changes the earlier build-first recommendation: evaluate existing tracker-driven orchestration before building a scheduler from scratch. Keep the [earlier reference architecture](reference-architecture.md) as the set of responsibilities to evaluate.

## 1. Where spec, design, tasks, and execution state live

The reviewed implementations use several stores, each for a different purpose. They do not establish that one tracker dominates the industry. The evidence supports the existence of these patterns, not a market-share ranking.

| Store | What it holds | Concrete examples | Important boundary |
| --- | --- | --- | --- |
| Human work tracker | Requested outcome, priority, ownership, dependencies, acceptance, review state | Linear, Jira, GitHub Issues, Trello | A ticket status does not prove a worker is alive or a deployment succeeded |
| Versioned repository artifacts | Living specs, design decisions, contracts, workflow instructions | SDD tools from the earlier study; Symphony `WORKFLOW.md` | Changes need review and a defined maintenance rule |
| Agent-oriented task ledger | Dependencies, ready work, claims, durable agent memory | Beads, Gas Town, Taskmaster | Must reconcile with any human tracker used alongside it |
| Runtime records | Run IDs, events, attempts, checkpoints, cancellation, usage | Durable workflows, managed sessions, application DB | A runtime session is not a product specification |
| Evidence store | Candidate commits, CI logs, PR review, test traces, artifacts | GitHub PRs/CI, worker artifacts | Bind evidence to the candidate actually evaluated |

A practical arrangement is a human tracker plus versioned design artifacts plus a small execution ledger. It is also valid to use a local agent ledger without Linear or Jira. What matters is explicitly choosing which system owns each field.

### Concrete tracking choices

| System | What the inspected material actually shows | Fit and limitation |
| --- | --- | --- |
| Linear | Agent sessions, activities, app identities, delegation, comments, project/document interaction. [O05–O08] | Strong shared human interface; actual coding runs elsewhere |
| Jira | Agent collaboration in work items, workflow/board triggers, and coding integrations. [O10–O12] | Preserves existing enterprise process; integration-specific identity and billing |
| GitHub | Issues and PRs connect task dispatch to implementation and review; current cloud sessions add broader workflow continuity. [O11, O45] | Fewer cross-system links when work already lives in GitHub |
| Trello | Official MCP access exists; Sergio demonstrates card polling, planning, questions, and draft PRs. [O13, O14] | Useful lightweight board; MCP alone does not schedule coding work |
| Beads | Dependency-aware ready/claim/close workflow in a Dolt-backed task graph. [O15] | Agent-oriented local/distributed ledger; not a ready-made PM collaboration portal |
| Gas Town | Multi-agent workspaces, persistent work tracking, coordination, recovery, and merge handling. [O16] | A more elaborate orchestration approach; evaluate its operational complexity |
| Taskmaster | Task management integrated through CLI/MCP, with planning and dependency tooling. [O17] | Work-management component; inspect the selected release's automation and collaboration behavior separately |

The Beads storage description matters: its current README says Dolt. Historical articles calling it merely a Git/JSONL task list should not define a new implementation.

## 2. What the named companies provide

| Company/system | Category and demonstrated mechanism | Existing coding-agent connection | Maturity/evidence boundary |
| --- | --- | --- | --- |
| OpenAI Symphony | Polling scheduler, tracker adapters, per-issue workspaces, repository workflow. [O01–O04] | Reference implementation launches Codex app-server | Engineering preview; reference implementation says evaluation only |
| OpenAI Codex for Linear | Managed delegation from issue assignment or mention, progress and result links. [O09] | Native Codex cloud integration; local MCP is a separate path | Product integration, not a general multi-agent factory engine |
| Microsoft Agent Framework | Multi-agent graph workflows, checkpoints, events, human request/response. [O18, O20] | Provider integrations and extensible execution; no reviewed evidence of a universal Claude/Codex tracker dispatcher | Framework; your application supplies task semantics and connectors |
| Microsoft Magentic orchestration | Manager with task facts, plan, and progress checks. [O19] | Coordinates participants in its agent workflow | Its reasoning ledger is distinct from a business issue tracker |
| Google Jules | Hosted coding sessions, activities, API polling, plan approval and messages. [O21–O23] | Uses Jules; not a Codex/Claude execution wrapper | Jules API currently labels itself alpha |
| Google Agents CLI | Scaffolding, evals, deployment, and observability around ADK, LangChain, or Claude Agent SDK. [O24] | Can build with a coding assistant and scaffold a Claude SDK application | Developer framework tooling, not a prebuilt issue queue |
| Vercel Open Agents | Web application, durable agent workflow, GitHub integration, sandbox. [O25] | Its own tool-driven runtime; inspected README does not establish a drop-in Codex/Claude adapter | Open-source reference application |
| Vercel eve | Durable agent framework with identity, channels, subagents, approval; factory template described. [O27] | Coding execution needs validation for the chosen template/backend | Landing page showed v1.0.0-rc.6; demo is not production evidence |
| Anthropic Claude Agent SDK | Claude Code's execution capabilities exposed programmatically. [O30] | Direct harness integration for a custom dispatcher | Framework, with application-owned ledger and policy |
| Anthropic cloud scheduled Code tasks | Schedule, API, and GitHub triggers with repositories/connectors. [O29] | Runs Claude Code cloud sessions | Research preview; individual-account ownership |
| Anthropic Managed Agents / Claude Tag | Managed session/harness/sandbox; shared-channel agent identity. [O31–O34] | Jira integration uses Managed Agents; Tag is a separate collaboration surface | Distinct products with distinct authorization models |
| Meta thin orchestrator | Internal pipeline routing and reliability gates around specialists. [O40] | The article does not establish a public Codex/Claude integration | Engineering account, not a released universal factory framework |
| Stripe Minions | Internal Slack/web/task entry points and unattended coding with human review. [O41, O42] | Custom harness derived from Goose, rather than a claim to run Claude Code | First-party production account; internal infrastructure |

Framework availability does not establish an existing company's internal task system. For example, Microsoft's framework is available to build with; that does not imply Microsoft's engineering teams track all their work in its Magentic ledger. Likewise, Meta's specialist-routing account does not establish its organization-wide spec or billing process.

## 3. Four outer-loop patterns in the evidence

**Tracker polling:** Symphony repeatedly reconciles issue state and dispatches eligible work. It controls scheduling while the repository workflow determines the work policy. Its current reference adapters include Linear, GitHub Issues, Jira Cloud, Asana, and GitLab. Trello is not in that inspected list. [O02, O04]

**Event-driven delegation:** Linear sends agent-session events for assignment/mentions and follow-up prompts. A receiver queues execution and publishes activities. Jira integrations expose similar human entry points, but their backend contracts differ. [O06, O11, O12]

**Durable application workflow:** Vercel Open Agents separates the web UI, workflow, and sandbox. A user message starts durable execution, and the sandbox can have a lifecycle independent from the controlling agent. This is a useful custom-application pattern rather than proof of a ready-made tracker poller. [O25, O26]

**Agent-ledger coordination:** Gas Town uses a dedicated work-tracking and coordination model; Beads exposes ready work and atomic claiming. These can organize workers without a traditional PM board. A synchronization layer is needed if a team also wants Jira or Linear to be authoritative. [O15, O16]

See [outer loop and human control](outer-loop-and-human-control.md) for mechanisms and an implementation proposal.

## 4. How humans remain involved

The evidence supports human ownership without a permanent human-to-agent pairing. Linear explicitly retains a human assignee and gives the agent the delegate role. An installed agent can serve people in its permitted teams. [O05, O08]

Symphony's reference workflow uses a persistent progress comment and separates `Human Review`, `Rework`, and `Merging`. Human acceptance makes work eligible for its landing step. This is a configurable sample workflow, not a universal behavior of every Symphony installation. [O03]

Stripe engineers can initiate several independent runs, inspect decisions in a web UI, and request human review on the resulting PR. This is a pool of task executions serving engineers, rather than evidence of one permanent personal agent for each engineer. Its detailed cost allocation is not disclosed in the reviewed articles. [O41]

Keep three human decisions distinct in the design: approving intended behavior, resolving consequential design trade-offs, and accepting a particular implementation. A “human involved” label does not reveal which of these actually occurs.

## 5. Identity and billing are separate dimensions

A person can own the outcome while a service account executes the work and a project pays for it. Conversely, a shared channel UI can invoke a task with one user's credentials. Do not infer identity from the avatar or the words “team agent.”

Two particularly direct answers to the ownership question are OpenAI's workspace service accounts for eligible pay-as-you-go plans, and Claude Tag's shared agent identity model. Both establish alternatives to tying all automation to an employee login. [O32, O35]

The detailed [identity, billing, and team-model study](identity-billing-and-team-models.md) distinguishes model credentials, tracker identity, GitHub identity, runtime identity, and human decision authority. It also records current billing changes and product-specific constraints.

## 6. Recommended shortlist for this project

These are research recommendations, not selected vendors or installed software:

1. **Evaluate Symphony first** for a tracker-driven Codex factory. Use its spec and reference code to test lifecycle behavior before creating a new scheduler. Add durable blocked-decision and cost records where the evaluation identifies a gap.
2. **Use Linear or the team's existing Jira/GitHub workflow as the human interface.** Changing a working tracker is not required. Linear has especially explicit agent-delegation primitives; Jira already offers concrete coding integrations.
3. **Keep specs and consequential design decisions versioned and linked to tickets.** Decide whether repository files or a document system owns living intent. Do not duplicate authoritative task status across several systems.
4. **Use role-based workers shared by the team**, with a named human owner per change, designated reviewers, and separately identifiable service principals where supported.
5. **Evaluate Vercel's reference stack for a custom shared UI** or Microsoft/Google frameworks where their platform integration fits existing infrastructure. A framework choice should follow a concrete missing capability.
6. **Treat Trello plus a dispatcher as a viable lighter path.** Use Sergio as inspectable community evidence, not an enterprise assurance. Evaluate auth, retry, and concurrency behavior before adoption.

There is no demonstrated need to operate an always-running “architect person,” “developer person,” and “reviewer person” agent for every employee. Define durable roles and task ownership; allocate sessions as work arrives.

## Research limits

This review inspected primary docs, GitHub READMEs, selected source code, and company engineering reports. It did not run the frameworks, audit complete repositories, or validate vendor reliability. Mutable main-branch files are access-date snapshots, not pinned release contracts. Private internal systems, staffing practices, exact budgets, and company-wide tracker prevalence remain unknown where not disclosed.

Ramp's Inspect and Laminar articles were discovered, but opening their pages yielded only site text. Their search excerpts were not used as substantive implementation evidence. Several redirects and product renames are documented in the source register.
