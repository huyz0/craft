# AI software factory research

Research date: **8 November 2026** (Australia/Sydney). SDD means **spec-driven development** throughout this collection.

An AI software factory is a repeatable delivery system in which agents turn agreed requirements into architecture decisions, bounded implementation tasks, code, tests, review findings, and release evidence. Codex or Claude Code supplies the coding execution loop. The factory supplies durable work state, coordination, validation, and delivery controls.

**Recommendation:** build a small workflow around versioned specifications, isolated coding workers, a separate evaluator, and CI. Start with one implementation worker and add parallel workers only for independent tasks. Keep product and consequential architecture decisions reviewable by people. Measure accepted changes and maintenance cost before increasing autonomy.

## Read the research

The follow-up on tracking, outer loops, human coordination, identity, and billing is available below. It refines the initial build-first advice: evaluate Symphony and existing tracker integrations before creating a custom scheduler.

| Document | What it answers |
| --- | --- |
| [Findings and synthesis](ai-software-factory-study.md) | What the evidence supports, what has changed recently, and the limits of current approaches |
| [Reference architecture](reference-architecture.md) | How spec, architecture, coding, evaluation, review, and delivery agents could work together |
| [Tools and execution](tools-and-execution.md) | Spec Kit, OpenSpec, BMAD, Kiro, Codex, Claude Code, and orchestration choices |
| [Pilot and evaluation](pilot-and-evaluation.md) | How to test whether a factory beats a plain coding-agent workflow |
| [Presentations and videos](presentations-and-videos.md) | Prioritized media, chapter pointers, slide findings, and access limitations |
| [Annotated sources](sources.md) | Dated source register, evidence strength, and claim-to-source mapping |
| [Operating models and company comparison](operating-models-study.md) | Linear, Jira, GitHub, Trello, dedicated ledgers, and frameworks from the named companies |
| [Outer loop and human control](outer-loop-and-human-control.md) | Polling, webhooks, claims, status updates, decision requests, review, and recovery |
| [Identity, billing, and team models](identity-billing-and-team-models.md) | Personal versus shared agents, authentication/authorization, service identities, cloud costs, and ownership |
| [Operating-model sources](operating-models-sources.md) | 47 additional numbered source documents and retrieval limits |
| [Code review, SOC 2, and SOX](code-review-soc2-sox.md) | Independent approval, agent attribution, repository enforcement, financial change controls, and audit evidence |
| [Context, artifacts, and agent communication](context-artifacts-and-agent-communication.md) | Cross-repo context, Rovo capabilities, scope drift, verbosity, durable outputs, and agent messaging |
| [Context and handoff templates](context-and-handoff-templates.md) | Proposed task context, artifact manifest, agent request, and human result records |
| [Context and artifact sources](context-and-artifact-sources.md) | 22 primary references, evidence limits, and reconciliation of conflicting claims |
| [A factory operated by several humans](team-owned-agent-factory.md) | Shared agent services, team project knowledge, human authority, and the distinction from personal projects |
| [Human-governed delivery and operations](human-governed-delivery-and-operations.md) | Humans maintain workflow, skills, MCP/CLI and review policy; agents plan, design, implement, deploy to nonproduction, and investigate incidents |
| [Agent-facing Jira/Confluence CLI research](atlassian-agent-cli-research.md) | OAuth choices, native keyring constraints, Jira ledger limits, and Confluence-first plan review |
| [CLI source register](atlassian-agent-cli-sources.md) | 48 primary references and access/verification limits |
| [Atlassian Projects, Assets and Artifacts](atlassian-projects-assets-and-artifacts.md) | Team initiative coordination, plan-draft-to-ticket workflow, Projects API/paging/access limits, AI files versus CMDB catalogs and the existing Teamwork Graph CLI |
| [Jira dependency-link direction](jira-dependency-link-direction.md) | Native labels, endpoint-role ambiguity, agent reversal failures and explicit mapping in Craft previews |
| [CLI implementation plan](../plans/atlassian-agent-cli-plan.md) | Architecture, auth/storage, review-page-to-ticket workflow, recovery, milestones, and acceptance gates |
| [CLI command and data contract](../plans/atlassian-agent-cli-contract.md) | Proposed agent commands, plan page format, JSON results, errors, and walkthrough |
| [Cloud CLI command catalog](../plans/cloud-cli-command-catalog.md) | Jira/Confluence CRUD, files, comments, spaces, status, API routes, scope/permission boundaries, and future providers |
| [Cloud CLI provider architecture](../plans/cloud-cli-provider-architecture.md) | Shared core, native provider semantics, capability discovery, auth strategies, Bitbucket/Trello extensions and plugin boundaries |
| [Agent CLI and skill research](agent-cli-and-skills-research.md) | 18 primary source references on token-efficient output, malformed-input defenses, progressive skills, pagination and evaluation |
| [Agent interface requirements](../plans/agent-cli-efficiency-and-skills.md) | Concrete discovery/validation commands, output budgets, short skill draft, packaging and acceptance cases |
| [Lists, cursors, fields, and response schemas](../plans/list-paging-and-response-schema.md) | Bounded pages, opaque continuation, lossless output limits, selected fields, output schema and full scans |
| [Craft implementation project](../../cli/README.md) | Applied lightweight SDD, huyz0 repository audit, extensible Rust design, coding/comment standard and detailed task ledger |

## Five conclusions

1. **The harness is a major engineering component.** Task boundaries, feedback, tool access, recoverable state, and evaluation determine whether coding agents can sustain useful work.
2. **Specs need an owner and a maintenance rule.** Preserve current intended behavior separately from historical plans. Link requirements to tests and delivered commits.
3. **Architecture work should settle decisions that independent workers could make incompatibly.** Record interfaces, state ownership, failure behavior, and consequential trade-offs before parallel implementation.
4. **AI review needs evidence and separate enforcement.** A finding should identify a concrete failure, affected code, and reproduction. CI and merge rules enforce acceptance.
5. **More agents increase both capacity and coordination cost.** Parallel reading and independent review are easier starting points than concurrent writes to coupled code.

## Coverage and limits

This is a targeted web research review, with primary documentation, engineering case reports, research papers, and practitioner presentations. It is not an exhaustive systematic review or a benchmark of installed tools. Sources were accessed on 8 November 2026; living documentation can change. Publication dates are recorded where the page establishes them.

Media analysis uses available slide text, a published talk transcript, and published chapter descriptions. No claim is made that every linked recording was watched. Gated and metadata-only sources are explicitly marked. Research observations and the proposed architecture are distinguished throughout. No factory runtime, paid agent run, deployment, or external integration was created by this research task.

## Validation

The software-factory research comprises 17 original/follow-up Markdown documents. The CLI follow-up adds three research files with 48 API/auth/storage references and 18 agent-interface references, plus seven planning files including the planning index. Its API/extension, efficiency/skill and pagination research was expanded on 9 November 2026. References include sources reused across studies. Local-link and code-fence checks cover the combined collection, and JSON examples are syntax-checked. Reviewed material claims against opened evidence and marked media/retrieval limits. External URLs were inspected during research, without a separate exhaustive link crawl. The workspace has no Git repository, so these files are saved locally without a commit. CLI behavior, native keyrings, live tenant access, token savings and agent/skill behavior remain implementation acceptance work, not verified results of this research.
