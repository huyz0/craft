# Change-aware verification

Date: 10 November 2026. Implemented in response to the user's instruction to fix the inefficient repeated gate loop.

`tools/verify.py` now compares tracked input hashes with `target/verification-state.json`, prints its selected scope/commands, and runs changed-input checks. `tools/verification_scope.py` selects affected crates, dependents, feature-gated hosts and artifact-owner checks. Missing history or explicit `--full` retains the complete fourteen-command matrix. Shared foundation, manifests and toolchain changes also widen to full. Documentation runs planning; unchanged inputs do no work. `--plan` cannot advance verification. Failed commands do not advance history; inputs changed during execution remain pending while stable checked inputs are retained.

The initial baseline reused the just-passed full gate in `target/policy-setup-gate.log` and its final recorded source hashes/focused follow-up in `target/policy-setup-final-validation.json`. No Rust result was invented and the same unchanged Rust matrix was not rerun to bootstrap a performance cache. This cache is not independent review, native-platform or live qualification.

## Observed results

- Verifier layer/label and impact-selection selftests pass. They cover docs-only and unchanged paths, provider dependents, mixed shared CLI changes, feature-gated fixtures, inventory ownership, unknown/shared foundation widening, deleted/new files and stable-input retention after a documentation edit during checking.
- The real default invocation selected two checks, planning and verifier selftests, with no Cargo command. It passed in `target/impact-verification-focused.log`.
- The unchanged rerun selected zero checks and completed in 0.297 seconds. See `target/impact-verification-unchanged.log` and `target/impact-verification-results.json`.
- `--full --plan` retained fourteen commands, including workspace tests and all nine composed host targets. A before/after digest proved the preview did not change the baseline.
- The final documentation-only smoke run is recorded in `target/impact-verification-documentation.log`; its required scope is planning with no Rust commands. The result is captured separately in the machine-readable report.

The first reporting harness incorrectly tried to parse multiline planning JSON as individual complete JSON records, then expected fifteen rather than fourteen legacy commands. These were reporting assertions, not verifier failures. They were corrected; no passing Rust suite was rerun as a consequence.

## Independent review

The user authorized any necessary review. `/root/activation_review` performed read-only inspection and found three genuine coverage gaps in the first selector: mixed shared/flow CLI changes suppressed other hosts, descriptor changes omitted owner contract checks, and dependency inventory edits were treated as ordinary documentation. All were fixed with regression controls. A second read-only review confirmed those findings resolved and found no remaining blocker in the bounded selector review. The reviewer executed no tests and changed no files.

Workflow instructions in `AGENTS.md`, `SDD.md`, `docs/validation.md` and the living workflow spec now require proportional checks and reserve `--full` for broad/release/CI qualification. A focused/cached pass cannot be represented as full qualification. Native/live acceptance and task completion still require their own evidence.

Workflow model changes now select public contract comparison/selftests as well as the workflow and CLI packages. Those models contribute request/response schemas; the bounded selector correction and its regression assertion were independently reviewed with the internal review contract. This adds owner checks without selecting unrelated native hosts.
