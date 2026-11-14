# Tools and coding execution

Recommendation: select one SDD convention, build a thin scheduler, and use existing coding harnesses. Start with CLI dispatch to establish the workflow, then adopt an SDK when event handling, cancellation, or session control justifies it. The factory should normalize outcomes while retaining provider-specific evidence.

Sources and access dates are in the [source register](sources.md). Capabilities below were checked against current documentation on 8 November 2026. This is a capability comparison, not a performance ranking.

## Specification and design tooling

| Option | Evidence-backed capability | Recommended fit | Trade-off to test |
| --- | --- | --- | --- |
| GitHub Spec Kit | Constitution, specification, planning, tasks, implementation, and convergence workflow; optional analysis gates. [S25] | New features needing a shared structured process | More artifacts and phase handling than a compact repair needs |
| OpenSpec | Change proposals with specs, design, and tasks; apply and archive flow. [S28] | Incremental changes in an existing repository | The team must still define acceptance and operational enforcement |
| BMAD Method | Architecture decisions and change-review workflows. [S20, S29] | Work spanning several epics or teams that need shared design | Role and workflow breadth may exceed a small project's needs |
| Kiro | Requirements-first design and task planning in its product workflow. [S30] | Teams choosing an integrated spec experience | Evaluate its execution and deployment surfaces against the desired factory integration |
| Small repository-native templates | Our proposed spec, ADR, task, and result contracts | Initial pilot or a team with established conventions | You own lifecycle rules and validation |

For this repository's research stage, use the reference contracts before choosing an installer. A pilot can compare a compact native spec against one selected framework. Avoid maintaining several competing live-spec systems for the same feature.

Do not copy old tutorial commands without checking the current integration. The inspected Spec Kit quickstart uses `/speckit-*` skills and includes convergence; older materials often show dotted forms. OpenSpec's canonical `/opsx:propose` spelling varies by assistant, including its Codex skill form. Pin the chosen tool release and store the generated instruction versions. [S25, S28]

## Execution surfaces

| Surface | What it supplies | What the factory still owns |
| --- | --- | --- |
| Codex CLI `codex exec` | Non-interactive coding execution, events, final structured output options | Work scheduling, acceptance, integration, operational outcome |
| Codex SDK | Programmatic local Codex sessions and continuation | Same delivery responsibilities plus application integration |
| Codex app-server | Richer application integration around conversations and events | Product workflow and policy |
| OpenAI Agents API | OpenAI-managed Codex harness and session infrastructure | Task definition, tools, accepted outcomes, integration |
| OpenAI Agents SDK | Application-hosted agent orchestration | Deployment, durable business state, tools and execution environment |
| Claude Code `claude -p` | Non-interactive Claude Code execution | Scheduling, acceptance, integration, operational outcome |
| Claude Agent SDK | Programmatic tools and agent loop used by Claude Code | Application lifecycle, workflow state, evaluation and delivery |

The OpenAI platform article describes `codex exec`, SDK, and app-server integration choices; the runtime overview distinguishes managed Agents API from Agents SDK. They are not interchangeable names for one API. [S01, S04]

Choose managed infrastructure if it removes a concrete operations burden and meets required data and environment constraints. Choose self-hosted workers if existing CI, private dependencies, or execution control make that simpler. No hosting choice establishes better output quality by itself.

## Minimal CLI examples

These are documentation-based invocation examples, **not commands executed during this research**. Run them in a prepared repository with the task inputs present and configured authentication. Confirm flags against the pinned installed release.

Codex implementation worker:

```powershell
codex exec --sandbox workspace-write --json 'Read work-order.md and implement only its assigned task.'
```

Codex documentation supports JSONL events and schema-constrained final output. Keep the event stream separate from the final report. Its current documentation says `--full-auto` is no longer supported; the example uses the documented sandbox setting. [S02]

Claude implementation worker:

```powershell
claude -p 'Read work-order.md and implement only its assigned task.' --output-format json --allowedTools 'Read,Edit,Bash'
```

The tool list is an example, not adequate isolation for arbitrary code. Configure the runtime's permission mode and execution environment deliberately. Claude's documentation describes JSON output and programmatic use through the Agent SDK. Avoid assuming identical event or resume semantics across providers. [S06]

For repeatable automation, explicitly control loaded instructions, hooks, skills, plugins, and MCP servers. Claude's documented `--bare` mode skips much automatic discovery, including `CLAUDE.md`; a factory using it must supply needed context explicitly. Choose a documented configuration for the pinned Codex release rather than assuming the same flags exist. [S06]

## Normalized worker adapter

Proposed adapter responsibilities:

```text
prepare(work_order) -> workspace + resolved inputs
start(workspace, role, inputs) -> run handle
stream(handle) -> normalized events + retained raw events
cancel(handle) -> observed cancellation outcome
collect(handle) -> patch + checks + session reference + usage
```

An adapter should distinguish execution failure, permission denial, timeout, invalid output, and a valid candidate. Do not reduce every non-success to an automatic retry. Validate result schemas and changed paths outside the agent, then run independent validation.

Use explicit provider session IDs when resuming work. A global “last session” selector can attach a repair to the wrong task when jobs run concurrently. Preserve the spec version and base revision in the factory ledger even when a provider retains its own conversation.

## Built-in multi-agent features

Claude subagents provide separate contexts for focused tasks; agent teams coordinate multiple sessions with shared tasks and messaging. The current team documentation marks teams experimental and disabled by default, with limitations around resumption and coordination. [S07, S08]

Use built-in delegation for local exploration or specialist analysis. Keep durable task ownership and delivery state outside an ephemeral conversation. A native team feature can execute work inside the proposed factory, but it does not replace the factory's acceptance contract.

## AI review integration

Codex repository review rules use scoped `AGENTS.md` guidance. Claude hosted Code Review documents GitHub review and verified findings, with plan and data-configuration eligibility constraints. Validate actual organization eligibility before choosing a hosted review service. [S05, S09]

If hosted review is unavailable, use a separate read-only worker invocation and consume its findings as artifacts. In either case, CI and repository rules should decide whether a candidate is mergeable. Calibrate review on known faulty changes and safe counterexamples before letting findings drive automatic repairs.

## Orchestration choices

A small explicit state machine is sufficient for the pilot. Add a graph runtime when checkpointing and branching become difficult to maintain. LangGraph documents persistence, checkpointers, stores, and interruption-oriented workflows. Persist production state with a suitable durable backend; an in-memory example does not survive process loss. [S21]

At greater scale, evaluate a durable job system against actual needs: leases, retries, idempotent external actions, long human waits, observability, and artifact retention. Avoid replacing an existing reliable CI scheduler merely to obtain an agent-themed abstraction.
