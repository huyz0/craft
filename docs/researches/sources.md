# Annotated source register

All entries accessed **8 November 2026**. “Living docs” means publication date was not established, not that the capability was newly released. Dates below come from the source page where available. Links point to public evidence; no repository tool installations or runtime benchmarks were performed.

## Evidence method

The review searched for AI software factories, multi-agent coding, SDD tooling, architecture, review, long-running harnesses, empirical productivity, and relevant talks. Recency searches included October and November 2026. Primary documentation and engineering reports were opened, followed by research papers and independent analyses. Media access was recorded separately.

Representative search families:

```text
site:developers.openai.com codex multi agents code review sdk
"AI software factory" agents spec driven 2026
spec driven development github spec kit openspec 2026
site:anthropic.com engineering multi agent coding harness 2026
AI software factory conference presentation video spec driven development 2026
"AI software factory" "October" "2026" architecture review
"AI software factory" "November" "2026"
```

This is a targeted review rather than an exhaustive search with a reproducible inclusion denominator. Search summaries were used for discovery. Consequential findings use opened sources. One media item with inadequate page extraction and one third-party AI summary remain explicitly labeled leads.

Evidence categories used here:

- **Product documentation:** strong for the documented interface; does not prove output quality or account eligibility.
- **First-party case or experiment:** useful mechanisms and observed results in the author's setting; limited external validity.
- **Empirical research:** inspect methods, task setting, and study limitations; preprints are not assumed peer-reviewed.
- **Practitioner analysis or proposal:** design insight; no implied controlled validation.
- **Media transcript or slide text:** inspectable content of a talk; claims still need independent evidence.
- **Listing only:** discovery and navigation, not substantive support.

## Product and execution sources

### S01. OpenAI agent runtime overview

[Agents](https://developers.openai.com/api/docs/guides/agents), living official documentation. Opened the runtime overview and managed API overview directly. Distinguishes managed Agents API, application-hosted Agents SDK, and Responses API. Supports separating model calls, harness execution, and application-owned workflow.

### S02. Codex non-interactive execution

[Non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode), living official documentation. Read the invocation, sandbox, JSON output, schema, and resume sections. Establishes the scripting surface, not whether a generated change meets its task.

### S03. Codex SDK

[Codex SDK](https://learn.chatgpt.com/docs/codex-sdk), living official documentation. Read programmatic session start and continuation examples. Relevant for replacing shell wrappers with an application integration.

### S04. Codex platform and managed harness

[Codex as a platform](https://developers.openai.com/blog/codex-as-a-platform), 19 August 2026, and [Agents API overview](https://developers.openai.com/api/docs/guides/agents-api/overview), living docs. These establish integration choices and managed-session responsibilities. Verify current environment and data requirements before adopting a managed runtime.

### S05. Codex custom review rules

[Custom Code Review rules for Codex](https://developers.openai.com/blog/custom-code-review-rules-for-codex), Hari Srikanth, 20 July 2026. Official guidance on scoped repository invariants. Read the article and examples of consequential compatibility rules. Product guidance, not an independent review-accuracy benchmark.

### S06. Claude Code programmatic execution

[Run Claude Code programmatically](https://code.claude.com/docs/en/headless), living official documentation. Read CLI execution, discovery/bare mode, output, lifecycle, and usage-accounting material. Supports the execution adapter design. Provider lifecycle details should remain visible in retained raw events.

### S07. Claude subagents

[Create custom subagents](https://code.claude.com/docs/en/sub-agents), living official documentation. Separate role context and tool access provide a local delegation mechanism. This does not by itself establish durable delivery state.

### S08. Claude agent teams

[Orchestrate teams of Claude Code sessions](https://code.claude.com/docs/en/agent-teams), living official documentation. Team sessions, tasks, and messaging are documented; the feature is marked experimental and disabled by default. Treat operational limitations as part of tool selection.

### S09. Claude hosted Code Review

[Code Review](https://code.claude.com/docs/en/code-review), living official documentation. Read review stages, eligibility, and merge-decision boundaries. Distinguish this hosted product from a local review command or a custom review worker.

## Harnesses, coordination, and measured outcomes

### S10. Long-running application harness

[Harness design for long-running application development](https://www.anthropic.com/research/harness-design-long-running-apps), Prithvi Rajasekaran, 24 March 2026. Read the role design, sprint contracts, behavioral evaluation, and comparison example. First-party experiment; increased quality also incurred substantial extra time and cost.

### S11. Parallel compiler construction

[Building a C compiler with a team of parallel Claudes](https://www.anthropic.com/research/building-c-compiler), Nicholas Carlini, 5 February 2026. First-party prototype illustrating long-running parallel coding. Useful for testing and coordination mechanisms; not a controlled estimate for business software delivery.

### S12. Multi-agent coordination research

[Patterns and problems in emerging multiagent systems](https://www.anthropic.com/research/multiagent-systems), Anthropic Frontier Red Team, 13 August 2026. Read coordination experiments and their qualifications. Vulnerability search and shared construction show different scaling behavior; scope and budget differences limit direct efficiency comparisons.

### S13. Factory for data systems

[Towards an AI Software Factory for Data Systems](https://arxiv.org/html/2609.36323v1), Anna Pavlenko and coauthors, submitted 28 September 2026, v1. Primary preprint from Microsoft-associated authors and collaborators. Read architecture and targeting sections. Reported efficiency gains concern data systems and measurable evolutionary tasks; general architecture remains a different problem.

### S14. SDD practice and SpecMine

[Spec-Driven Development as Prescribed and as Practiced](https://arxiv.org/html/2608.25202v4), Shyam Agarwal and coauthors, first submitted 25 August 2026, v4 revised 15 October 2026. Read abstract and discussion of artifact longevity and maintenance. Empirical preprint, observational rather than causal. Earlier search metadata used the previous SPECMINE title; the current title and version above control this report.

### S15. Source control at agent scale

[Redesigning source control at Anthropic](https://www.anthropic.com/research/redesigning-source-control), 13 October 2026, updated 15 October. First-party infrastructure account. Supports monitoring integration bottlenecks; the organization explicitly describes its workload as unusual. It does not justify replacing Git in this project.

### S16. Test-passing patches versus maintainer acceptance

[Many SWE-bench-Passing PRs Would Not Be Merged into Main](https://metr.org/notes/2026-03-10-many-swe-bench-passing-prs-would-not-be-merged-into-main/), Parker Whitfill and coauthors, 10 March 2026. Read summary and study setup. Less-reviewed research note covering historical agents and limited maintainer samples; agents did not iterate on feedback.

### S17. SDD levels and tools

[Understanding Spec-Driven-Development: Kiro, spec-kit, and Tessl](https://martinfowler.com/articles/exploring-gen-ai/sdd-3-tools.html), Birgitta Böckeler, 15 October 2025. Independent practitioner analysis. Useful conceptual distinction between spec-first, spec-anchored, and spec-as-source; historical tool descriptions should not override current docs.

### S18. Experienced-developer productivity experiment

[Measuring the Impact of Early-2025 AI on Experienced Open-Source Developer Productivity](https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/), METR, 10 July 2025. Opened primary study used as historical context; core result and limitations were also checked through the 2026 update. The 19% slowdown is specific to its developers, repositories, tasks, and early-2025 tools.

### S19. Productivity measurement update

[We are Changing our Developer Productivity Experiment Design](https://metr.org/blog/2026-02-24-uplift-update/), Joel Becker and coauthors, 24 February 2026. Read results and selection/timing caveats. Prevents treating the earlier slowdown as an estimate of today's tools or multi-agent workflow.

## Design methods and factory case reports

### S20. BMAD architecture decisions

[Design UX and Architecture](https://docs.bmad-method.org/plan/design-ux-and-architecture/), living project documentation. Read architecture-spine guidance. Supports identifying decisions that would conflict if made independently. This is a method prescription rather than measured proof of architecture quality.

### S21. LangGraph persistence

[Persistence](https://docs.langchain.com/oss/python/langgraph/persistence), living official project documentation. The durable-execution URL redirected here. Read the checkpoint/store distinction. Useful infrastructure capability; the factory must still define task and evidence semantics.

### S22. StrongDM factory

[Software Factories And The Agentic Moment](https://factory.strongdm.ai/), Justin McCarthy, 6 February 2026. Read the narrative, scenario concept, and evaluation discussion. First-party account of highly autonomous delivery; no controlled comparison establishes that its operating rules transfer to every workload.

### S23. GuildBuild failure report

[Implementation, traps and failures of the so-called AI software factory](https://www.guildbuild.ca/insights/ai-software-factory-traps-and-failures), GuildBuild Team, 10 October 2026. Read workflow and failure descriptions. First-party field report with explicit limitations and no matched productivity trial. Useful concrete evidence about provenance and state reconciliation.

### S24. Disposable-worker architecture proposal

[A blueprint for a dark factory](https://timdepater.com/articles/blueprint-for-a-dark-factory/), Tim de Pater, 28 October 2026. Read state-machine, worker, and role design. Practitioner blueprint, not an independently validated production result. Informative separation of orchestration from agent judgment.

### S25. Spec Kit current workflow

[Spec-Driven Development Quickstart](https://github.github.com/spec-kit/quickstart), living GitHub project documentation. Read process, constitution, skill invocation, and setup guidance. The observed workflow includes convergence and integration-specific invocation. Pin a release before implementation.

### S26. Sogl SDD transcript

[Spec Driven Development: The End of Vibe Coding](https://gitnation.com/contents/spec-driven-development-the-end-of-vibe-coding), Daniel Sogl, recording from 26 February 2026 according to the speaker's [talk page](https://danielsogl.de/en/speaking/spec-driven-development). Transcript reviewed. Historical demo guidance, with transcription errors in some tool names.

### S27. Sogl SDD presentation

[VibeKodeCon Munich deck](https://speakerdeck.com/danielsogl/spec-driven-development-vibekodecon-munich-2026), 24 June 2026 according to the canonical talk page. Slide text reviewed. The report uses workflow examples and limitations, not unverified headline statistics.

### S28. OpenSpec current workflow

[OpenSpec README](https://github.com/Fission-AI/OpenSpec), also inspected as [raw text](https://raw.githubusercontent.com/Fission-AI/OpenSpec/main/README.md), living project documentation. Read change artifacts, lifecycle, and invocation differences. Vendor-authored comparisons are positioning rather than neutral product rankings.

### S29. BMAD change review

[Review a Change](https://docs.bmad-method.org/build/review-a-change/), living project documentation. Read review and triage workflow. Supports role-specific code review as a process option. Performance and economic claims in project prose were not adopted as measurements.

### S30. Kiro requirements-first specs

[Requirements-First](https://kiro.dev/docs/specs/feature-specs/requirements-first/), living official documentation. Supports the requirements-to-design-to-tasks workflow comparison. This review did not benchmark Kiro or establish feature parity across its surfaces.

## Media leads and supplementary material

### S31. Spec Stacking lesson

[Spec Stacking & Software Factories](https://maven.com/p/9db11e/spec-stacking-software-factories), 29 May 2026. Chapter outline and description reviewed; recording not reviewed. Useful navigation for layered specs, validation, and stopping conditions.

### S32. Agent Conf factory talk index

[Stop Making LLMs Deterministic](https://confpad.io/conferences/agent-conf-2026/stop-making-llms-deterministic-a-spec-driven-software-factory), Jakub Kuzimski, event 17–18 September 2026. Third-party chapter listing and AI-written summary only. Quantitative claims are excluded pending primary verification.

### S33. LeadDev panel

[Controlling your AI software factory](https://leaddev.com/technical-direction/controlling-your-ai-software-factory), page dated 24 October 2026. Abstract and metadata inspected; login-gated recording not reviewed. Promoted partner content.

### S34. CERN recording lead

[Lessons from Spec-Driven Development](https://videos.cern.ch/record/3025661). Discovered in search; opened page lacked usable talk text. Retained only as a media lead, with speaker/date not independently established from the opened record.

### S35. Official Codex video directory

[Codex learning collection](https://developers.openai.com/learn/codex), living official listing. Individual recordings were not reviewed. Use for follow-up demos rather than claims about measured effectiveness.

### Supplementary reading

[Structured-Prompt-Driven Development](https://martinfowler.com/articles/structured-prompt-driven/), Wei Zhang and Jessie Jie Xia, 28 April 2026. Opened practitioner article on versioned prompt assets and reviewable changes; useful adjacent practice.

[Moderne AI Software Factory Summit](https://moderne.ai/ai-software-factory/), current event page. Opened to check media discovery; the advertised 2027 program is future material and is excluded from completed-talk evidence.

## Claim-to-evidence map

| Claim or decision | Sources | Qualification |
| --- | --- | --- |
| Coding harnesses can be invoked programmatically | S01–S04, S06 | Documented capability; no live invocation in this research |
| Native agent features differ from durable factory orchestration | S07, S08, S21 | Architectural inference grounded in documented responsibilities |
| Long-running agents benefit from explicit feedback and handoffs | S10, S11 | First-party experiments, task-specific results |
| More agents can create coordination problems | S12 | Model and workload dependent |
| Specs need ongoing reconciliation | S14, S17, S27 | Observational and practitioner evidence |
| Shared architecture decisions help align tasks | S20 | Method recommendation; requires local validation |
| Green tests and AI review do not establish merge acceptance | S05, S09, S16 | Product boundaries and historical maintainer study |
| Review and delivery evidence must identify actual revisions | S23 | First-party field report; adopted as proposed invariant |
| End-to-end productivity must be measured locally | S13, S18, S19 | Heterogeneous settings prevent a universal forecast |
| Small deterministic scheduler with disposable workers | S24 plus our design synthesis | Proposed architecture, not an implemented result |

## Research gaps

No head-to-head benchmark here establishes a winning model, framework, or optimal agent count. No current dollar-cost estimate for this repository is available. Hosted review and managed API access depend on the user's organization and configuration. Media coverage includes substantial transcript and slide reading, but most recordings were not directly reviewed. Sources support a concrete pilot, not a promise of unattended production reliability.
