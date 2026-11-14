# Lightweight spec-driven development

Use **OpenSpec's standard spec-driven artifact layout**, initialized manually for this subproject. Its project config supports manual creation; no global installation is required to edit the artifacts. The upstream OpenSpec executable is not installed here and its validator has not been run. Our small local checker validates the planning contract described below. This is not a claim of upstream tool conformance.

## One source of authority per concern

| Concern | Authority |
| --- | --- |
| Active contributor workflow | `AGENTS.md`, this file, `openspec/specs/project-workflow/spec.md` |
| Implementation naming and comments | `docs/coding-standard.md` |
| Intended product behavior in the current change | `openspec/changes/build-cloud-cli/specs/*/spec.md` |
| Technical approach | The change's `design.md` |
| Detailed work and completion status | The change's `tasks.md` only |
| Current handoff | `STATE.md`, linking to the ledger rather than repeating its statuses |
| Observed validation | Evidence records attached to completed task entries; [validation policy](docs/validation.md) |
| Research background | `docs/repository-audit.md` and the root research/planning documents |

The product change has `proposal.md`, `design.md`, capability delta specs and `tasks.md`. The living workflow spec is already active. Product delta specs describe a target, not shipped behavior. Do not archive the product change until its selected scope is implemented and verified; archive then merges accepted behavior into living specs while preserving incomplete/deferred work explicitly.

## Use the smallest change record that carries the decision

1. Read the selected task and capability scenarios. A bug fix can add one regression scenario and edit the existing task/design. Create a separate OpenSpec change only for a distinct feature, substantial contract change or independently reviewed scope.
2. Settle required semantics before coding: effect, input/output, target authority, preconditions, failures and observable checks. Ordinary implementation choices remain autonomous. User approval is required only where actual authority or intent is missing, not merely because this workflow has a document called a proposal.
3. Implement one dependency-ready task or a coherent subset, with focused behavior tests. Use failure-first tests for risky input/auth/write/recovery semantics; documentation-only corrections do not need artificial tests. Keep unrelated discoveries as separate planning changes.
4. Run the checks appropriate to the actual change, inspect results and record failures/limits. The default verifier selects affected checks from changed inputs; inspect `--plan` when needed and use `--full` for release/CI or broad qualification. Do not repeat unchanged suites or rerun Rust matrices for documentation updates. An independent reviewer checks consequential auth, deletion, review evidence and unknown-write paths before they are enabled. Do not call a mock tenant a live integration.
5. Update the ledger and an evidence record together. If interrupted, retain partial changes and a concise state handoff; do not invent a pass, erase a failed check, or commit/push just to satisfy a ritual.

## Task records

Each semantic task section contains exactly one completion checkbox and **Goal**, **Spec**, **Depends on**, **Build**, **Verify**, **Done when**, and **Evidence** fields. `Depends on` contains semantic task names or `none`. A checked task needs a real local evidence record under `docs/evidence/`; that record lists actual artifacts, commands/results, failures and unrun checks. The helper checks graph/fields/links/evidence presence, but cannot establish behavioral correctness from prose alone.

All product tasks initially remain unchecked. Semantic names are references within planning, not prefixes to source symbols. Task decomposition is a hypothesis: split work when implementation shows incompatible responsibilities or an unreviewable diff. Keep dependencies and capability coverage valid after a split. No forced one-task/one-commit rule and no mandatory milestone ladder.

## Planning metadata stays out of the product

The [project coding standard](docs/coding-standard.md) makes this rule and comment quality part of SDD. Comments describe the code's behavior or design, including intent, invariants, failures and non-obvious tradeoffs. They must remain accurate and must not merely restate obvious statements or record delivery progress.

Do not embed milestone/task labels in code, comments, tests, fixture names/content, public schemas, CLI output, configuration, package/module names, or commit subjects. Use semantic test names such as `rejects_unknown_fields` and semantic commit descriptions such as `feat(auth): persist rotated credentials safely`. Map the actual test path/result to the requirement in the ledger or evidence document. Schema versions, provider IDs and runtime request/operation IDs are domain data, not banned planning labels.

`python tools/plan.py check` scans supported implementation text files for conventional artificial tracking-label patterns and verifies graph/spec coverage. This is a useful guard, not proof that every conceivable identifier was detected. Review checks contextual leakage. Do not evade the rule by inventing another label prefix.

## Keep context and tools small

Only the contributor router, state and current task are common context. Read exact specs/design sections and at most the relevant validation/recovery material. The `craft-sdd` skill routes the contributor; future `craft-cloud` skills teach users of the executable and have separate acceptance tasks. No LLM, scheduler, external ticket ledger or generated agent ecosystem is required to use this plan. The local helper uses only Python's standard library and is documentation tooling, not a Craft runtime dependency.

Sources: [OpenSpec README](https://github.com/Fission-AI/OpenSpec), [manual config and customization](https://github.com/Fission-AI/OpenSpec/blob/main/docs/customization.md), and [your repository audit](docs/repository-audit.md). We inspected Spec Kit too; OpenSpec's small artifact model fits this project's current need without adopting a full bundle/integration system.
