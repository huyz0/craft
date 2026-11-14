# AI software factory: findings and synthesis

As of 8 November 2026, the strongest practical design is a durable engineering workflow that delegates bounded work to coding agents and checks the resulting software independently. A useful factory must preserve intent, recover across sessions, and demonstrate that a delivered change satisfies its requirements.

Source IDs refer to the [annotated source register](sources.md). Recommendations below are design judgments for this project, rather than established product guarantees.

## 1. Working definition and scope

For this project, an AI software factory includes six connected capabilities:

1. Clarify a requested outcome and capture a testable specification.
2. Read the existing system and record necessary architecture decisions.
3. Decompose the change into tasks with dependencies and acceptance conditions.
4. Execute tasks using a coding harness such as Codex or Claude Code.
5. Evaluate behavior, review the code, and repair specific failures.
6. Produce delivery evidence and feed operational observations into later work.

This definition permits several agents or several role-specific invocations of the same harness. It does not require every role to be a continuously running service. A factory should have a short path for a local repair and a fuller path for changes with substantial coordination risk.

Distinguish the **model**, which reasons and generates tool calls, from the **coding harness**, which manages tools and the working context, and the **factory workflow**, which manages work across tasks, agents, CI, and delivery. These layers have different state and responsibilities. Current OpenAI documentation distinguishes a managed Agents API from an application-hosted Agents SDK; the Codex SDK is another integration surface for Codex execution. [S01–S04]

## 2. Recent evidence worth acting on

| Evidence | Finding | Consequence for this project |
| --- | --- | --- |
| SpecMine study, revised 15 October 2026 | Practitioners report clearer intent alongside planning, review, and maintenance costs. The study combines interviews and repository analysis. [S14] | Test SDD selectively; make spec maintenance part of delivery. |
| Anthropic source-control case, 13 October 2026 | Agent-heavy development strains source control, builds, and CI at that organization's unusual scale. [S15] | Track integration and CI queue time before expanding worker count. |
| Microsoft-associated factory paper, 28 September 2026 | Describes targeting, coding, review, and operations for measurable evolutionary changes in data systems. [S13] | Include task selection and operational feedback in the design; do not infer universal productivity gains. |
| Anthropic coordination research, 13 August 2026 | Independent discovery and interdependent construction have different coordination properties. [S12] | Use task dependency graphs and explicit ownership; avoid assuming a larger swarm improves results. |
| Codex review rules, 20 July 2026 | Repository-specific review guidance can carry non-obvious invariants. [S05] | Write a few scoped review rules and evaluate whether they catch real defects. |
| Anthropic long-running app harness, 24 March 2026 | Planner, generator, and evaluator roles use structured handoffs and feedback. [S10] | Separate implementation from behavioral evaluation and persist handoffs. |

October practitioner reports add operational detail: GuildBuild describes review tied to exact bytes and failures involving mismatched tested and running versions; Tim de Pater proposes deterministic orchestration with disposable workers. These are useful case and design accounts, with less general evidential weight than controlled comparisons. [S23, S24]

## 3. Spec-driven development is a family of practices

Birgitta Böckeler distinguishes **spec-first**, **spec-anchored**, and **spec-as-source** approaches. The distinction matters: writing a document before implementation does not establish how it stays useful afterward. [S17]

For this factory, choose spec-anchored development:

- Maintain intended behavior for the current system.
- Store a change proposal and its rationale as historical records.
- Let code remain inspectable and editable through ordinary engineering work.
- Update affected specifications when approved behavior changes.

Keep three artifact types distinct. The behavioral spec describes the outcome, boundaries, and examples. The architecture record describes decisions and trade-offs. The execution plan describes the current implementation route. Function names and temporary file paths usually belong in the execution plan, because they age faster than the behavioral intent.

The October SpecMine revision discusses this maintenance tension and distinguishes living specs from accumulated change records. It does not establish a causal productivity uplift from SDD. [S14]

### What a useful specification contains

The proposed minimum is a goal, non-goals, observable acceptance conditions, edge cases, compatibility constraints, and unresolved questions that affect behavior. Add measurable non-functional requirements where they matter. “Fast” is insufficient; a latency target needs a workload and measurement method. “Secure” needs concrete boundaries such as tenant isolation or credential handling.

Example proposed requirement:

> REQ-EXPORT-03: When a user requests an export belonging to another tenant, the system returns the agreed unavailable-resource response and exposes neither the file nor its metadata. Validate through the public API using two tenants, including a guessed export identifier.

This example creates work for the architect, developer, evaluator, and reviewer without prescribing an arbitrary class hierarchy. A separate design decision can settle the response policy and its compatibility implications.

Scale the process to uncertainty and consequence. A small repair with a clear reproduction can use a compact change record. A feature crossing APIs, schemas, and services merits explicit contracts and dependency analysis. Daniel Sogl's talk and slides also discuss the overhead of full SDD on small tasks. [S26, S27]

## 4. Architecture is the shared decision surface

Multiple workers can independently choose reasonable designs that do not fit together. Typical conflicts concern API styles, naming, authentication, transaction boundaries, event ordering, and data ownership.

The proposed architecture role should first inspect the repository, existing contracts, and operational constraints. It should then identify decisions that require consistency across task boundaries. BMAD's architecture documentation offers a particularly useful formulation: capture non-obvious choices that separate units could otherwise make incompatibly. [S20]

For each consequential decision, ask the architect to produce alternatives, a recommendation, the relevant evidence, and a way to test the consequences. Assign a separate critique pass to look for missing failure paths and contradictions. A person should settle unresolved business trade-offs; the agent can carry out routine implementation decisions within the agreed direction.

Architecture validation should include executable checks where possible: dependency rules, contract compatibility, migration rehearsal, and latency or load experiments. Diagrams alone cannot establish these properties. For a new dependency, a small working spike can reveal uncertainty more cheaply than a long design narrative.

## 5. Coding agents need a feedback-rich environment

Both Codex and Claude Code have programmatic execution paths. This makes it feasible for a workflow engine to dispatch a work order, capture events and artifacts, and decide the next transition. [S02–S04, S06]

A coding task should arrive with a base revision, spec revision, relevant architecture decisions, writable scope, test commands, and a completion contract. The worker should return a candidate commit or patch, observed verification results, and remaining issues.

Anthropic's compiler experiment demonstrates substantial autonomous construction under a purpose-built harness. It used 16 agents and reported nearly 2,000 sessions and about $20,000 in API costs. It is a research prototype and a case study, not a forecast for an ordinary product team. [S11]

Long tasks need recoverable state. Persist what has changed, which checks actually ran, what failed, and what the next worker must do. Treat a conversational summary as useful context rather than the authoritative work ledger. A new context window should not erase a failed check or expand a task's authority.

## 6. AI code review and independent evaluation serve different purposes

Behavioral evaluation asks whether the running software meets the spec. Code review asks whether the change introduces defects, violates an invariant, or creates a maintenance problem. Both are needed because they inspect different evidence.

Claude's current hosted Code Review documentation describes parallel review, verification of findings, and inline comments. It also states that the service does not approve or block merges. Codex's review guidance treats repository rules as additional context and leaves hard enforcement to tests and repository controls. [S05, S09]

Proposed review design:

1. Run deterministic checks appropriate to the change.
2. Give a fresh reviewer the diff, relevant surrounding code, spec, and architecture records.
3. Require concrete findings with an affected location, failure condition, impact, and supporting evidence.
4. Reproduce or challenge consequential findings before creating repair tasks.
5. Review the repaired candidate and rerun affected validation.

A second model may add useful diversity, but model diversity does not establish independence. Different evidence and a different objective matter more than changing the reviewer name. Evaluate cross-provider review as an experiment, not a default guarantee.

METR's March 2026 research note found that roughly half of test-passing patches from the studied mid-2024 to 2025 agents would not be merged by maintainers. Its no-iteration setting limits generalization to a feedback-driven factory. The important distinction is between test success and maintainable, acceptable work. [S16]

## 7. Parallelize where dependencies permit

Good early candidates are repository exploration, alternative design analysis, bounded security review, independent test design, and implementation in modules with settled contracts.

Sequential work is often preferable when two tasks touch the same schema, change a shared interface, or repeatedly depend on each other's discoveries. A task marked independent can become coupled during implementation; the scheduler needs a way to revoke parallel execution and return the dependency for review.

Anthropic's August research found meaningful differences by model and task structure. Its vulnerability-search comparisons also differ in search scope and token budgets, so raw counts should not be treated as a clean efficiency comparison. [S12]

Start with one writer, one evaluator, and optional read-only specialists. Increase writers only when measurements show available independent work and acceptable integration cost. Separate worktrees avoid direct filesystem collisions, but they do not prevent semantic conflicts.

## 8. Fully autonomous factories are an operating hypothesis

StrongDM explicitly describes a factory in which specifications and scenarios drive implementation without human code review. Its account introduces holdout-style scenarios and digital service replicas. This is a valuable example of a more autonomous operating model. It remains a first-party account with workload-specific validation assumptions. [S22]

For this project, adopt its useful evaluation idea: protect some independently owned acceptance scenarios from implementation edits. Keep deterministic checks for precise contracts and use rubric-based evaluation only for properties that require judgment. Protecting scenario files is not sufficient if expected outcomes leak into worker context or if the evaluator itself is easy to satisfy.

Do not turn a vendor's operating style or token-spend heuristic into a prerequisite. Autonomy should follow measured reliability in a particular task class. Product judgment, difficult migration choices, and ambiguous user experience remain appropriate human decision points.

## 9. Measure the whole delivery system

The 2025 METR study found slower completion in its particular experienced-developer setting. Its February 2026 follow-up explains why newer productivity measurements were affected by selection and timing problems. Neither establishes the net benefit of this proposed factory. [S18, S19]

Measure accepted-change lead time, human active time, all-attempt cost, rework, escaped defects, and operational acceptance. Include failures and abandoned runs. Coding speed alone can hide a growing review queue.

The Microsoft-associated paper reports promising gains for a specific data-systems workflow with measurable optimization objectives. Its result supports studying end-to-end workflow design, while leaving open whether those gains transfer to greenfield architecture or ambiguous product work. [S13]

## Recommended direction

Use versioned specs and architecture records, a durable state machine, interchangeable Codex and Claude Code workers, and an independently owned evaluation path. Put a single integration queue between parallel candidates and the main branch. Keep merge, deployment, and observed acceptance as distinct recorded events.

The next useful investment is the [bounded pilot](pilot-and-evaluation.md): compare a plain coding-agent baseline, the same harness with SDD, and the factory workflow on representative tasks. Adopt additional roles and infrastructure when that experiment shows where they improve accepted outcomes.
