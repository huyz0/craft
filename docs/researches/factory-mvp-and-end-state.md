# Software factory MVP and end state

**Design decision · 20 November 2026 · proposed**

Build a **supervised, reusable Jira-to-PR loop** first. One human delegates one bounded change. A Codex worker implements it, a fresh verifier and CI test it, and a human reviews the intent, evidence, and critical code. The workflow must send requested changes back to an agent and return a newly verified candidate. Start with the team's existing tools and a small local runner. Add shared dispatch when unattended work creates a demonstrated need for shared claims.

The [requirements and pilot brief](factory-requirements-and-pilot.md) records the problem, sources, and design assumptions. This decision turns that research into a build boundary and a path to the mature system.

See the [architecture and stack decision](factory-architecture-and-stack.md) for the follow-up vote on the launcher, database, runners, and publication boundary.

## Independent votes

Three agents voted from architecture and reuse, review and governance, and operations and efficiency perspectives. Each distributed 10 points, with up to three points per capability, and selected one starting shape. Points express **priority for the first release**. A zero vote can still identify an important end-state feature. Safety obligations and prerequisites apply regardless of score.

| Starting shape | Votes | Rationale |
| --- | ---: | --- |
| **Supervised one-change loop using existing tools** | **2** | Tests the central hypothesis: can agents give a human a better decision and handle the repair loop with less effort? Architecture and governance both favored this. |
| **The same loop plus a shared dispatcher in the MVP** | 0 | Valuable when several unattended runners must claim the same queue on day one. With explicit delegation, it adds coordination infrastructure before the workflow is proven. |
| **CI-native failure to draft PR in one repo** | 1 | The operations reviewer favored a recurring trigger and concrete failure evidence. This is a strong trial of autonomous repair, with narrower coverage of plan approval and team handoff. |

| Capability | Architecture | Governance | Operations | Total |
| --- | ---: | ---: | ---: | ---: |
| Independent checks, short human review, and repair | 3 | 3 | 3 | **9** |
| Explicit delegation, pinned work order, and local or headless execution | 3 | 3 | 2 | **8** |
| Review capacity and deduplicated notifications | 1 | 2 | 1 | **4** |
| Permission-aware durable context and versioned skills | 1 | 2 | 1 | **4** |
| Standing pickup and event-based CI investigation | 0 | 0 | 2 | **2** |
| Shared claims, crash recovery, and idempotency | 1 | 0 | 0 | **1** |
| Pluggable trackers, knowledge stores, and forges | 1 | 0 | 0 | **1** |
| Model and effort routing, cost per accepted task | 0 | 0 | 1 | **1** |
| Cross-repo campaigns and dependencies | 0 | 0 | 0 | **0** |
| Operational inventory and read-only production diagnosis | 0 | 0 | 0 | **0** |
| Dependency scanning and agent-assisted upgrades | 0 | 0 | 0 | **0** |
| Risk-based auto-merge and rollout automation | 0 | 0 | 0 | **0** |
| **Total** | **10** | **10** | **10** | **30** |

**Decision.** Adopt the supervised loop. Keep a single active review slot and pinned access-scoped context in the MVP. Include local run records, usage collection, and a small adapter boundary from day one. Shared failover, adaptive model routing, and provider breadth can follow. Use the operations reviewer's CI-first proposal as the next workflow, starting with a human-selected failed run. This tests a different entry point without requiring a second factory.

## MVP

One team, one Jira project, one representative repository, one forge, and its existing CI. Use Confluence when a decision needs a reviewed plan. Use an existing repo with an owner, passing baseline, and a realistic change. A disposable repo can test Git mechanics separately.

| Component | MVP responsibility | Evidence it works |
| --- | --- | --- |
| **Intake and handoff** | A human explicitly delegates an owned Jira issue. A versioned work order records intent, allowed scope, acceptance examples, repo and base SHA, context links and revisions, and applicable checks. | A second developer can understand the task and continue from its records. An unresolved decision names the human needed and the revision they are deciding on. |
| **Local runner and developer choice** | Automate one Codex worker in an isolated checkout. Use the same work order for a developer using Codex Desktop, Claude Code, Cursor, Rovo Dev, or another approved tool. A developer can submit a commit with a short handoff for independent verification. | The worker produces a scoped candidate with a commit and run reference, or a clear blocker. A developer-submitted candidate passes the same CI, review, and publication gates. Runner metadata unavailable for a manual candidate is recorded as unknown. |
| **Shared skill and repo context** | Keep a short repo guide and pilot one reviewed `SKILL.md` package, such as how to run and report the repo's focused tests. Pin the approved guide and skill revision in the work order. Keep a controlled profile for the automated worker. | Two approved tools can load the same skill revision and the same work order. Required checks run through trusted CI regardless of how the candidate was produced. |
| **Independent verification** | Run trusted checks against the candidate SHA. Give a fresh verifier the intent, diff, and independently owned acceptance expectations. Use static analysis already configured for the repo. | The review can distinguish a verified fix, an inconclusive result, and a candidate that passes only because an expectation was weakened. |
| **Human review and repair** | Present one short PR brief with intent, test intent, results, risk, and links to consequential code. The human decides using existing PR controls. An authorized request for changes is handed back to a worker against the same PR. | The repair produces a new candidate and refreshed checks. A previous candidate's approval is ineligible for the revised change when the policy requires current approval. |
| **Evidence and capacity** | Keep small per-attempt records and artifact links in approved storage. Record model, effort, token usage, checks, and outcome. Keep one active review slot and update one current brief. A human or single launcher owns the slot during the pilot. | A failed attempt remains visible. A second delegation waits when the review slot is full. A human can inspect how the candidate and decision were produced. |
| **Publication** | An authorized human or the existing controlled publisher opens or updates the draft PR. Existing repository policy owns approval and merge. | The implementation worker cannot fulfill its own independent approval or alter the rules that evaluate its candidate. A publication with an unknown outcome gets reconciled before anyone repeats it. |

The expected path is `explicit delegation → pinned work order → candidate → verification → human review → merge through existing policy`. Open a draft before verification if CI requires a PR. Request human review once the evidence is ready. Requested changes loop through a new candidate and verification. A material scope change goes back to the responsible human for a decision.

Keep the handoff to a work-order reference, candidate commit, check output, and next step or blocker. A different developer or tool can start from these records without access to the implementer's chat or private skill setup. See the [agent choice and shared setup requirements](factory-requirements-and-pilot.md#agent-choice-shared-skills-and-personal-setup).

### Team rollout acceptance

The Windows MVP delivery uses a disposable Jira-to-Bitbucket integration exercise with simulated reviewer feedback, plus the credential-free suite and technical review. The following conditions apply when a team chooses to trial the workflow on its own work.

1. Complete one representative change with an actual human request for changes and an agent repair. Verify the repaired revision, the final approval where required, and the merge outcome if the owner chooses to merge.
2. Exercise missing or conflicting intent, a candidate that weakens a required check, and a new head after approval. Confirm that replaying the same delegation creates no new attempt and that a different eligible change stays queued while the review slot is full. Include an unknown PR publication outcome in the supervised recovery drill.
3. Compare ordinary Codex work with comparable work using this loop. Measure reviewer active minutes, time to an accepted outcome, rework, material defects, and all-attempt cost. One completed change proves the flow. It takes a broader comparison to judge whether the process reduces review burden.
4. In a representative trial, use a second approved tool to follow the same pinned work order and load the pilot skill. Hand its candidate to the same verifier. Confirm the automated runner loads the approved skill when personal configuration offers a same-name skill. Record which tool versions and launch modes the trial covered.

Shared automatic pickup, hosted takeover, multiple automated runner adapters, production access, and automatic merges are later release capabilities. In the MVP, existing policies govern all publication and release decisions.

## End-state components

The mature version is a **team delivery and operations system**. People can own and operate separate agents. Shared records and execution rules coordinate work across people, repos, and runner locations. Craft remains a CLI integration surface for bounded, auditable tool operations.

| Component | Mature capability |
| --- | --- |
| **Provider adapters** | Versioned tracker, document, Git/CI, artifact, runner, and telemetry interfaces. Jira, Confluence, GitHub, and Bitbucket can coexist. A project can opt into another provider where its capabilities meet policy. Declare actual enforcement and approval semantics for each adapter. |
| **Deterministic dispatcher and execution store** | Manual pickup, standing user rules, and authorized event triggers use the same admission path. Route by permissions, repo, owner preference, runner capabilities, budget, and workload. Enforce claims, lease generations, cancellation, bounded retries, publication reservations, reconciliation, and review capacity. |
| **Runner fleet and selective AI coordination** | Laptop and hosted runners use a shared work-order and result contract. Adapters declare supported actions, permission boundaries, and available usage data. The dispatcher applies repo and data policy before honoring a developer's preferred runner or an allowed fallback. An AI coordinator helps with ambiguous changes and dependencies. Its proposals go through deterministic admission. |
| **Context, memory, and skills** | Retrieve accepted plans, code and contract revisions, runbooks, service relationships, and prior evidence within project and user access. Publish reviewed skills from a versioned Git catalog through the tools' supported loaders. Governed runs use pinned team skills in a controlled profile. Developers keep their own approved tools and private customization for interactive work. |
| **Review and change control** | Team-sized review queues, concise briefs, independently owned acceptance checks, evidence-backed findings, and optional diagrams for material design changes. Keep human decisions revision-bound. Trusted publication and release services enforce the applicable policy. Auto-merge is scoped to justified and enforceable lanes. |
| **Operations and maintenance** | CI and monitoring create bounded investigations. Connect deployed artifact and config revisions to source, reviewed intent, resource inventory, telemetry, and ownership. Use short-lived scoped identities for production reads. Use Renovate and scanners for detection and queues of upgrades, with agents handling migrations and targeted test work. Verify the affected running versions before closing remediation. |
| **Measurement and model routing** | Capture usage and cost by task and stage, including failed runs, CI, and human rework. Qualify model and effort profiles on comparable tasks, apply fixed budgets, and escalate with evidence. Review model and skill changes against representative cases. |

### Promotion order

| Priority | Add | Decision to advance |
| --- | --- | --- |
| **1. Prove the review loop** | MVP with manual delegation, a single active review slot, independent checks, and a real repair cycle. | A reviewer can make the right decision from the brief and evidence. The pilot catches invalid approval and weak-test cases. |
| **2. Automate repeated work** | Try one failed-CI workflow using existing CI or a small dispatcher. Pin the failing commit, job, and attempt from current provider records. Run candidate code separately from the draft-publishing credential. Add standing pickup for one allowlisted task class. Add a shared execution authority if two unattended runners need the same queue. | The worker returns a verified draft repair or an evidence-backed escalation. Duplicate events produce one active attempt. Offline takeover cannot send a conflicting write while an earlier publication is unresolved. The queue stays within reviewer capacity. |
| **3. Expand to team and cross-repo delivery** | Qualify a second automated runner when there is demand. Add a second project or repo, parent change and dependency mapping, catalog-backed context, reviewed skill updates, and evidence-driven model routing. | A second runner produces a usable result from the same contract and passes the skill-collision, permissions, and missing-capability checks. A partial rollout stays visible, and compatibility holds if a downstream change stalls. |
| **4. Expand operations and controlled autonomy** | Add dependency campaigns, staged rollouts, and scoped operational inventory and production reads. Run auto-merge decisions in shadow mode before enabling a qualified lane. | A merged patch with the old artifact still running remains open as a remediation. A read-only investigator cannot trigger a production mutation. Required human approvals and exception records remain effective. |

Choose the simplest backend that passes the current phase. Human delegation can use a local runner and existing tools. A single authority with a local database can serve several runners when shared claims become necessary. Evaluate an existing CI or orchestration product before replacing it. Add replica coordination, richer interfaces, and more adapters when actual workloads justify their cost. [Symphony specification](https://github.com/openai/symphony/blob/main/SPEC.md) · [GitHub Agentic Workflows architecture](https://github.github.com/gh-aw/introduction/architecture/)

The winning design prioritizes **human effort per accepted change at acceptable risk**. Track queue age, rework, escaped defects, and cost alongside throughput. If the factory creates more drafts while people spend longer reviewing or repairing them, throttle new work and improve context and evidence before adding more workers.
