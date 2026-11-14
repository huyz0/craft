# Operating-model sources and evidence notes

Accessed **8 November 2026**. This is the source register for [the follow-up study](operating-models-study.md), [outer-loop design](outer-loop-and-human-control.md), and [identity/billing analysis](identity-billing-and-team-models.md). It supplements the original [35-source register](sources.md).

The numbered entries are source documents, not a count of unique companies or independent validations. Living documentation and main-branch code are mutable snapshots. A capability in documentation is not an observed successful deployment. Framework maturity labels below reflect the inspected source, not a fresh release audit.

## Dispatch and tracker integrations

| ID | Opened primary source | Evidence and limit |
| --- | --- | --- |
| O01 | [Symphony README](https://github.com/openai/symphony/blob/main/README.md) | Engineering-preview positioning and task-run model; README inspected as raw text |
| O02 | [Symphony specification](https://github.com/openai/symphony/blob/main/SPEC.md) | Draft v1; inspected scheduler, claims, reconciliation, retries, and recovery sections |
| O03 | [Symphony reference workflow](https://github.com/openai/symphony/blob/main/elixir/WORKFLOW.md) | Concrete status and human-review policy; sample configuration, not universal defaults |
| O04 | [Symphony Elixir README](https://github.com/openai/symphony/blob/main/elixir/README.md) | Current adapters, Codex launch, tracker credential handling, and blocked-state limits; evaluation prototype |
| O05 | [Linear agent setup](https://linear.app/developers/agents) | App OAuth and delegate semantics; living product documentation |
| O06 | [Linear agent interaction](https://linear.app/developers/agent-interaction) | Webhook timing, session activities and lifecycle, progress links |
| O07 | [Linear signals](https://linear.app/developers/agent-signals) | Stop, authentication, and selection interactions |
| O08 | [Linear agent platform](https://linear.app/docs/agents-in-linear) | Workspace/team access, human interface, agent-seat billing distinction |
| O09 | [Codex in Linear](https://learn.chatgpt.com/docs/third-party/linear) | Native cloud delegation versus local MCP access; official OpenAI documentation |
| O10 | [Jira agent collaboration](https://support.atlassian.com/jira-software-cloud/docs/collaborate-on-work-items-with-ai-agents/) | Workflow/board triggers and current Rovo credit transition; body and billing sections inspected |
| O11 | [Copilot coding agent for Jira](https://github.blog/changelog/2026-03-05-github-copilot-coding-agent-for-jira-is-now-in-public-preview/) | 5 March 2026 announcement; progress and clarification inside Jira; historical release status |
| O12 | [Claude Agent for Jira setup](https://support.atlassian.com/jira-software-cloud/docs/set-up-claude-agent-for-jira/) | Managed Agents API key, machine-account GitHub token, Forge storage, session and PR flow |
| O13 | [Official Trello MCP server](https://github.com/atlassian/trello-mcp-server/blob/main/README.md) | OAuth-backed tool access; not a coding scheduler |
| O14 | [Sergio](https://github.com/Belfio/sergio/blob/main/README.md) | Community Trello poller using Claude CLI; planning/development lanes and setup inspected, not executed |

## Ledgers and orchestration frameworks

| ID | Opened primary source | Evidence and limit |
| --- | --- | --- |
| O15 | [Beads README](https://github.com/gastownhall/beads/blob/main/README.md) | Current Dolt-backed graph, ready/claim/close commands, coding-agent setup |
| O16 | [Gas Town README](https://github.com/gastownhall/gastown/blob/main/README.md) | Persistent worker identity, work tracking, coordination and recovery; author claims not benchmarked here |
| O17 | [Taskmaster README](https://github.com/eyaltoledano/claude-task-master/blob/main/README.md) | CLI/MCP task-management entry points and provider configuration; current release behavior not exercised |
| O18 | [Microsoft Agent Framework README](https://github.com/microsoft/agent-framework/blob/main/README.md) | Graph orchestration, checkpoints, hosting, telemetry; framework rather than tracker product |
| O19 | [Magentic implementation](https://github.com/microsoft/agent-framework/blob/main/python/packages/orchestrations/agent_framework_orchestrations/_magentic.py) | Read task/progress-ledger prompts and structures; limited code inspection, not full audit |
| O20 | [Microsoft workflow HITL](https://learn.microsoft.com/en-us/agent-framework/workflows/human-in-the-loop) | Request/response mechanism; page displayed an access notice but exposed the relevant body |
| O21 | [Jules API quickstart](https://jules.google/docs/api/reference/) | Source/session/activity model, polling, key authentication, plan approval endpoints; alpha label |
| O22 | [Jules limits and plans](https://jules.google/docs/usage-limits) | Account eligibility and task/concurrency allowances; living plan page, not a team-contract quote |
| O23 | [Jules plan review](https://jules.google/docs/review-plan/) | Plan-first user interaction; page opened, later viewport fetch timed out; detailed API support also in O21 |
| O24 | [Google Agents CLI](https://google.github.io/agents-cli/guide/getting-started/) | Framework templates, evaluation and cloud deployment; does not establish tracker orchestration |
| O25 | [Vercel Open Agents README](https://github.com/vercel-labs/open-agents/blob/main/README.md) | Reference application, workflow/sandbox split, session UI, Vercel/GitHub authentication configuration |
| O26 | [Durable coding agent tutorial](https://vercel.com/kb/guide/how-to-build-a-durable-ai-code-agent-on-vercel) | Ben Akehurst, 13 May 2026; workflow, sandbox, gateway and observation example |
| O27 | [eve framework](https://eve.dev/) and [Vercel overview](https://vercel.com/eve) | Framework and factory-template claims; inspected page showed RC version; animated demo is illustrative |
| O28 | [Vercel Sandbox pricing](https://vercel.com/docs/sandbox/pricing) | Resource meters, plan/regional distinctions, CPU versus memory; no workload estimate computed |

## Identity, access, and billing

| ID | Opened primary source | Evidence and limit |
| --- | --- | --- |
| O29 | [Claude cloud scheduled tasks](https://code.claude.com/docs/en/routines) | Formerly routines; research preview; triggers, personal ownership, connector identity and usage |
| O30 | [Claude Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview) | Programmatic harness and authentication/commercial boundary; separate from binary embedding |
| O31 | [Scaling Managed Agents](https://www.anthropic.com/research/managed-agents) | 8 April 2026 engineering account; durable session/harness/sandbox separation |
| O32 | [Claude Tag agent identity](https://claude.com/resources/articles/agent-identity-access-model) | 24 June 2026 account of shared-channel authority; read together with O33 |
| O33 | [Tag personal connectors](https://claude.com/resources/articles/claude-tag-now-supports-personal-connectors-in-channels) | 24 September 2026 update; personal-request and unattended shared-connector distinction |
| O34 | [Claude Enterprise consumption](https://support.claude.com/en/articles/14782391-claude-enterprise-consumption-guide) | Current channel versus DM billing and applicable limits |
| O35 | [Codex workspace service accounts](https://learn.chatgpt.com/docs/enterprise/service-accounts) | Non-human workspace identity, managers and eligibility; distinct from Platform accounts |
| O36 | [Codex authentication](https://learn.chatgpt.com/docs/auth) | API, ChatGPT and enterprise automation identity distinctions; official docs body inspected |
| O37 | [OpenAI workload identity federation](https://developers.openai.com/api/docs/guides/workload-identity-federation) | Short-lived authentication for API/Codex workloads; overview body and identity comparison inspected |
| O38 | [GitHub organization/enterprise billing](https://docs.github.com/en/copilot/concepts/billing-and-usage/organizations-and-enterprises/billing) | Current AI credits, pools, app-installation charging; supersedes older preview billing assumptions |
| O39 | [Claude Code legal/commercial conditions](https://code.claude.com/docs/en/legal-and-compliance) | Conditions for offering the binary in another product/service; do not conflate with SDK terms |
| O43 | [Entra agent identity source](https://github.com/MicrosoftDocs/entra-docs/blob/main/docs/agent-id/agent-identities.md) | Source dated 30 March 2026; fetched after public page exposed access notice only |
| O44 | [Google Agent Identity overview](https://docs.cloud.google.com/gemini-enterprise-agent-platform/govern/agent-identity-overview) | User, agent and delegated identity model; enterprise runtime, distinct from Jules |
| O46 | [Rovo Dev billing](https://support.atlassian.com/subscriptions-and-billing/docs/how-billing-works-for-rovo-dev-standard/) | Separate Rovo Dev allowances and paid usage; not the generic Rovo meter |
| O47 | [Claude API overview](https://platform.claude.com/docs/en/api/overview) | API credentials, workspaces and hosted-session surfaces; no negotiated commercial pricing inferred |

## Company implementation accounts and current product context

| ID | Opened primary source | Evidence and limit |
| --- | --- | --- |
| O40 | [Meta thin orchestrator](https://engineering.fb.com/2026/10/19/ml-applications/thin-orchestrator-ai-agent-architecture/) | 19 October 2026 internal multi-agent pipeline account; no released universal ledger established |
| O41 | [Stripe Minions, part 1](https://stripe.dev/blog/minions-stripes-one-shot-end-to-end-coding-agents) | Alistair Gray, 9 February 2026; human interface and review ownership |
| O42 | [Stripe Minions, part 2](https://stripe.dev/blog/minions-stripes-one-shot-end-to-end-coding-agents-part-2) | Alistair Gray, 19 February 2026; cloud devboxes, custom harness and blueprints |
| O45 | [Copilot on GitHub](https://docs.github.com/en/copilot/concepts/copilot-surfaces/copilot-on-github) | Living docs distinguish new cloud experience available from 28 October 2026 and older flows |

## Search and extraction notes

Representative query families included Symphony/Linear polling, Microsoft task ledgers, Vercel background agents, Jira coding integrations, Trello coding pollers, Google Jules API, shared agent identity, and organization billing. Popularity was not measured, and inclusion should not be read as a recommendation based on stars or brand recognition.

The study used primary sources for technical claims. Community READMEs describe their own implementation and are primary evidence for those descriptions, but not independent evidence of reliability. No installations, logins, external task creation, or paid runs were performed.

Some retrieval limitations affected coverage:

- Ramp's [Inspect article](https://engineering.ramp.com/post/why-we-built-our-background-agent) and [Laminar article](https://engineering.ramp.com/post/building-laminar) returned site text without their bodies. They were excluded from substantive findings.
- The ADK multi-agent documentation redirected to a page that exposed no content, and a guessed source path returned 404. The Google framework comparison therefore relies on the opened official Agents CLI documentation, not an asserted detailed ADK audit.
- Microsoft's Entra page exposed an access notice. The official repository source supplied the identity explanation.
- A Jira third-party-automation page could not be retrieved. Claims about triggers use the opened generic collaboration documentation and specific integration pages instead.
- Main-branch GitHub files were not pinned to commit SHAs. Adoption requires pinning an actual release and verifying its code, especially for previews and rapidly changing agent tooling.

## Consequential distinctions checked

| Question | Controlling evidence |
| --- | --- |
| Does Symphony run Codex rather than merely call a model? | O04 launches Codex app-server |
| Does it require a durable scheduler DB? | O02 describes tracker/filesystem recovery |
| Is human review part of every Symphony deployment? | O03 is a sample workflow, with O02 leaving policy configurable |
| Are agent delegate and human owner separate? | O05 |
| Does an MCP server create a scheduler? | O13 supplies tools; O14 supplies an actual poller |
| Is a Magentic ledger equivalent to Jira? | O19 is internal facts/plan/progress state |
| Are Claude scheduled Code tasks shared team jobs? | O29 says personal ownership |
| Can shared and user-specific credentials coexist? | O32 and O33 |
| Can Codex use non-human organization identities? | O35 and O37, subject to eligibility |
| Is preview-era Copilot per-session billing current? | O38 establishes current AI-credit billing |
| Does generic Jira orchestration pay every external provider bill? | O10 and O12 describe different layers; no such guarantee established |
| Does every named company offer a complete factory framework? | No. O40–O42 are internal case accounts, while frameworks and products occupy different layers |
