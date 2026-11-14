# Context, artifacts, and agent communication: source register

Accessed **8 November 2026**. These are primary product documentation, first-party engineering reports, and research-paper abstracts. Documentation is evidence of a described capability, not independent proof of effectiveness or availability on a particular account. IDs support the [study](context-artifacts-and-agent-communication.md).

| ID | Source | Evidence and limits |
| --- | --- | --- |
| N01 | [Atlassian: What is Code Context?](https://support.atlassian.com/rovo/docs/what-is-code-context/) | Opened body. Cross-repo retrieval, indexing freshness, SCM permissions, Teamwork Graph CLI. Eligibility and supported SCMs matter. |
| N02 | [Atlassian: Rovo Dev in Jira webinar Q&A](https://community.atlassian.com/forums/discussion/3215401/webinar-q-a-rovo-dev-in-jira) | Opened body. Selected-repo sandbox and Jira/Confluence context. First-party community answer; do not generalize its restrictions to every Rovo surface. |
| N03 | [Atlassian: Rovo Dev in the IDE](https://support.atlassian.com/rovo/docs/use-rovo-dev-in-the-ide/) | Opened body. File and selection context, planning, sessions, and integration behavior. |
| N04 | [Atlassian: Memory in Rovo Dev CLI](https://support.atlassian.com/rovo/docs/use-memory-in-rovo-dev-cli/) | Opened body. Persistent user/project files and memory commands. |
| N05 | [Atlassian: Subagents in Rovo Dev CLI](https://support.atlassian.com/rovo/docs/use-subagents-in-rovo-dev-cli/) | Opened body. Supplied context, scoped configuration, prompts, tools, and memory selection. |
| N06 | [Claude Code: How Claude remembers your project](https://code.claude.com/docs/en/memory) | Opened body. Persistent guidance, auto memory, scoped rules, and distinction between context and enforcement. Loading behavior is version-dependent. |
| N07 | [OpenAI: Custom instructions with AGENTS.md](https://learn.chatgpt.com/docs/agent-configuration/agents-md) | Opened discovery and layering sections. Explains instruction discovery, overrides, and size limits. Markdown alternate URL failed; HTML body was available. |
| N08 | [Sourcegraph: Cody context](https://sourcegraph.com/docs/cody/core-concepts/context) | Opened body. Search, code relationships, explicit context references, and multi-repo context. Not an agent execution or cross-repo write guarantee. |
| N09 | [Augment: Context Engine MCP](https://www.augmentcode.com/blog/context-engine-mcp-now-live) | Published 6 February 2026, updated 18 June 2026. Opened body. Local versus hosted retrieval. Performance claims are vendor-reported and not reproduced here. |
| N10 | [Anthropic: Effective context engineering](https://www.anthropic.com/research/effective-context-engineering-for-ai-agents) | Published 29 September 2025. Opened body, including retrieval section. Engineering guidance, not a controlled comparison of this factory's options. |
| N11 | [Anthropic: Effective harnesses for long-running agents](https://www.anthropic.com/research/effective-harnesses-for-long-running-agents) | Published 26 November 2025. Opened body. Incremental work and durable progress artifacts from reported experiments. |
| N12 | [Cursor: Scaling long-running autonomous coding](https://cursor.com/blog/scaling-agents) | Published 14 January 2026. Opened body. Coordination failures and planner/worker experiments. Do not equate research experiments with every shipped product. |
| N13 | [Cursor: Introducing Projects](https://cursor.com/blog/projects) | Published 10 September 2026. Opened body. Describes beta rollout, coordinator, shared files, and recurring work. Account access and productivity claims not independently verified. |
| N14 | [Claude Code: Agent teams](https://code.claude.com/docs/en/agent-teams) | Opened body. Direct messages, separate contexts, shared work, and experimental availability. Higher coordination/token cost noted by the vendor. |
| N15 | [OpenAI: Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents) | Opened substantive body. Delegation, summarized outputs, context separation, and coordination overhead. Client and triggering rules vary. |
| N16 | [GitHub Agentic Workflows: Artifacts](https://github.github.io/gh-aw/reference/artifacts/) | Opened artifact overview and cross-run trust section. Run evidence and producer validation. Exact artifact names evolve with gh-aw releases. |
| N17 | [GitHub Agentic Workflows: Security architecture](https://github.github.io/gh-aw/introduction/architecture/) | Opened permission-isolation section. Separate agent and external-write jobs. Detection modes and configured policy affect behavior; this is not universal human review. |
| N18 | [Gloaguen et al.: Evaluating AGENTS.md](https://arxiv.org/abs/2602.11988v3) | Abstract and version metadata reviewed through the unversioned landing page, which identified v3, revised 29 September 2026. Full methods were not reviewed in this follow-up. Findings bounded to reported evaluation. |
| N19 | [Lulla et al.: Impact of AGENTS.md on efficiency](https://arxiv.org/abs/2601.20404v2) | Abstract and version metadata reviewed through the unversioned landing page, which identified v2, revised 30 March 2026. Full methods were not reviewed. Different measures/settings from N18. |
| N20 | [A2A protocol v1.0.1 specification](https://a2a-protocol.org/v1.0.1/specification/) | Opened protocol overview/data-model outline and authentication/authorization section. Provides interoperability concepts; implementation must supply its own authorization policy. This study pins a version rather than inferring latest from conflicting search results. |
| N21 | [Anthropic: Building a C compiler with parallel Claudes](https://www.anthropic.com/research/building-c-compiler) | Published 5 February 2026. Opened body. Separate containers/clones, Git task claims, and durable notes; explicitly no other communication mechanism in that prototype. Does not establish separate physical machines. |
| N22 | [OpenAI Symphony specification](https://github.com/openai/symphony/blob/main/SPEC.md) | Opened problem statement and system overview. Draft specification for tracker-driven execution and reconciliation; not a general distributed scheduler. |

## Reconciliation of apparently conflicting claims

**Rovo multi-repo context versus a selected-repo session:** N01 describes search infrastructure and its access surfaces. N02 describes the Jira coding execution experience. Neither establishes unrestricted multi-repository execution. Verify the selected surface and enabled capabilities.

**Instruction files help versus instruction files do not help:** N18 and N19 report different experiments and measures. They do not establish a universal effect for all instructions. The recommendation is to test concise, non-obvious guidance on representative work.

**Direct agent communication versus hierarchy:** N14 describes a supported experimental team capability. N12 reports failures and improvements in a particular harness. Capability and workload suitability are separate questions.

**A2A release versions:** search results exposed several versioned and development specifications. Claims here are limited to the opened v1.0.1 document; no assertion is made that every coding agent natively implements it.
