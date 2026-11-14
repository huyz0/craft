# Pilot: does this factory improve accepted outcomes?

Run a bounded comparison before increasing agent count or delivery autonomy. The proposed experiment separates the value of written intent from the value of additional agent roles. This plan has not been executed.

## Three comparison paths

| Path | Workflow | Question |
| --- | --- | --- |
| A: coding-agent baseline | Existing task description, one coding harness, ordinary CI and review | What can the team already achieve? |
| B: SDD | Same harness and acceptance policy, with a reviewed spec and explicit constraints | Does clearer intent repay its planning cost? |
| C: factory | Path B plus persistent task state, independent evaluator, and separate review worker | Do workflow and role separation improve accepted delivery? |

Keep the model and harness version fixed across a comparison wherever possible. If testing Codex against Claude Code, treat that as a separate factor; changing provider, model, spec quality, and worker count simultaneously prevents a useful attribution.

Use representative tasks from the actual target repository. Proposed initial set: 12 tasks across local repairs, ordinary features, and changes involving shared contracts. This is an operational pilot, not a sample size justified for a statistical claim. Report uncertainty and task-level outcomes.

Independent executions of the same task need isolated starting revisions and should not see other paths' solutions. Alternatively, randomize comparable task blocks while recording differences in complexity. Avoid a workflow in which a person solves a task in A and then uses that knowledge to guide C without recording the contamination.

## Acceptance before execution

For each task, settle expected behavior, protected invariants, required checks, and material human decisions before comparing paths. Keep acceptance standards comparable. Use an independently owned evaluation set for every path; do not give the factory easier success criteria.

Potential task classes:

- A local regression with a reproducible failing example.
- A feature with happy-path, negative, and persistence requirements.
- An API change with an existing-client compatibility constraint.
- A schema change with migration and rollback implications.
- A concurrency bug with a repeatable interleaving or stress test.
- A UI change with an explicit interaction and accessibility requirement.

Do not fabricate test commands or architecture constraints for an empty project. Select tasks once the target implementation and its normal verification environment are known.

## Measurements

| Metric | Definition | Interpretation |
| --- | --- | --- |
| Accepted-change lead time | Time from an agreed start boundary to accepted outcome | Include waiting, review, repair, and integration |
| Human active time | Time spent specifying, steering, reviewing, and repairing | Include planning cost, not just coding assistance |
| All-attempt cost per accepted change | All run and infrastructure cost divided by accepted changes | Include failed and abandoned attempts; report unresolved work separately |
| First-candidate acceptance | Tasks accepted without repair divided by completed first candidates | Reveals whether output needs hidden cleanup |
| Rework | Number and effort of repair cycles after initial candidate | Separate changed intent from implementation defects |
| Escaped defects | Confirmed attributable failures after acceptance | Requires an observation period and attribution rule |
| Review precision | Confirmed actionable findings divided by adjudicated findings | High comment count is not a quality measure |
| Known-defect detection | Seeded or historical relevant defects found | A bounded calibration measure, not total recall on unknown bugs |
| Coordination cost | Integration conflicts, stale evidence, duplicate tasks, blocked time | Shows whether more writers are useful |
| Spec maintenance | Time and revisions needed to reconcile intended behavior | Captures continuing SDD cost |

Record both active compute time and wall-clock elapsed time. For example, a fast worker may merely move waiting time into a slow review queue. Count session-resume usage carefully to avoid counting cumulative usage twice.

Illustrative arithmetic only: if the pilot spends $300 across all attempts and accepts six changes, run cost is $50 per accepted change before human and infrastructure costs. If none are accepted, report total spend and zero accepted changes rather than presenting a misleading per-change figure.

## Reliability probes

Before increasing autonomy, run a few failure exercises that can invalidate the workflow design:

1. Kill a worker after it produces a patch but before it reports completion. Confirm that recovery reconciles the patch and does not duplicate work.
2. Deliver a duplicate completion event. Confirm that only one next-stage job starts.
3. Change the candidate after review. Confirm that the old review no longer satisfies the acceptance gate.
4. Let main change underneath a candidate. Confirm integration checks target the combined revision.
5. Attempt to edit a protected evaluator or policy file. Confirm the execution boundary or final-diff check rejects the result.
6. Introduce a known behavioral defect that unit tests miss. Confirm an independent scenario or reviewer catches it.
7. Exhaust the budget. Confirm the scheduler stops dispatching new work and preserves actionable diagnostics.

These probes test the proposed factory's behavior. They are not tests run as part of this documentation task.

## Staged rollout

### Stage 1: one complete vertical slice

Use a compact spec, one coding worker, independent checks, a fresh reviewer, and ordinary human integration. Persist versions and results. Completion means a real accepted change with traceable evidence and a demonstrated recovery path.

### Stage 2: separate evaluation

Add a behavioral evaluator and protected acceptance cases. Compare B and C on defect detection, human effort, and cost. A useful evaluator should catch failures that the implementer's own checks miss without generating excessive false alarms.

### Stage 3: limited parallel work

Test two implementation workers only on tasks with settled interfaces and limited shared writes. Compare integration delay, conflicts, and cost against one writer. Preserve a serialized integration queue.

### Stage 4: selective automation

Automate acceptance or integration only for task classes with demonstrated reliability and an explicit repository policy. Consequential migrations, unclear requirements, and unresolved architecture decisions can retain a decision gate. Deployment automation is a separate scope from generating a candidate patch.

## Decision rule

Before running the pilot, the team should choose an acceptable cost, defect rate, and human-time improvement for its workload. There is no evidence-backed universal percentage threshold. Adopt C where it improves accepted delivery or defect detection enough to justify its cost; retain A or B for tasks where the extra roles do not pay off.

Record decisions by task class. “Factory useful for compatibility-sensitive features, compact agent workflow better for local repairs” is a more actionable outcome than a single blended speedup number.

## Remaining questions

The implementation language, target repository, current CI, hosting constraints, and spending limit remain unspecified. They affect implementation choices but do not prevent this research or its architecture proposal. A production design also needs the actual ownership and release policies. Do not treat the proposed examples as existing project policy.
