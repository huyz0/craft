# Craft Factory review quality delivery plan

The disposable CI lane demonstrated red CI, a bounded repair, an exact-revision
human decision, and a green scheduled poll after merge. It used a scripted
worker. This follow-up makes verification part of admission to human review and
tests a real agent across successive changes.

Deliver each item in its own PR to `main`. Keep one review slot and per-change
human approval in the disposable evaluation. Keep the pilot repository private
and label an explicit chat decision as operator approval if the forge cannot
enforce native approvals on the private repository.

| Delivery | Work | Acceptance on `main` |
|---|---|---|
| 1. Pre-draft CI repair gate | Pin a trusted verifier and its harness before repair admission. Verify paired baseline and candidate results, reject protected history, and require the evidence before the pilot publisher opens a draft. Record the exact revisions and receipt digests. | A real assertion fails on the base and passes on the candidate. A preservation check passes on both. Skipped checks, infrastructure errors, stale or forged receipts, and a candidate that edits or reverts protected tests cannot reach the draft write. Exercise the guarded path in a disposable repo. |
| 2. Revision-bound review brief | Render one short brief from the work order, diff, independent verification, and native checks. Surface intent, change, evidence, risk, and required decision. Add a distinct plan-approval path for changes that reach configured sensitive areas. | A routine repair and a sensitive change take their respective review paths. Replacing the head invalidates its brief and any previous decision. The PR links to the exact diff and receipts. |
| 3. Real-agent iterative evaluation | Run five related changes on one disposable codebase. Each starts from the preceding accepted revision. Include a superficially green change that would violate preserved behavior. Record model, effort, human minutes, rework, token or billed cost where available, and basic structural signals. | Publish reproducible inputs and observed outcomes. Every accepted change has a human decision, passing required evidence, and an exact revision. Report unobserved readings and failed runs. Soundness and reviewer time are evaluated alongside CI. |
| 4. Feedback into the harness | Use existing owner-reviewed knowledge proposals. Turn a recurring finding from review into a test, a lint rule, or a scoped skill improvement, with references to the source reviews. | Observe the same class of defect on distinct review events, approve one narrowly scoped change to the harness or guidance, and replay a case to show the difference. Keep one owner responsible for promoting and reverting the rule. |

Delivery 1 merged after the hosted Factory checks passed. In the private
disposable lane, the gate rejected a candidate that changed a protected test
and opened one draft for the independently verified candidate.
[Craft PR #15](https://github.com/huyz0/craft/pull/15)

Start with the [paired dependency verification contract](../../factory/README.md#verify-baseline-and-candidate-behavior)
and the [independent verification handoff](../../factory/shared/README.md#prepare-independent-verification).
Horthy’s [conference talk](https://ai.engineer/code/2026/talks/dexter-horthy)
motivates the focus on verification and reviewer attention. The
[SlopCodeBench paper](https://arxiv.org/abs/2603.24755) informs the successive-change design.
