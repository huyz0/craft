# Validation and completion evidence

## Stop repeated verification loops

A failed check requires a diagnosis before another attempt. Identify whether the cause is product behavior, a fixture, the environment or the gate itself, then change the relevant input or select a check that resolves the uncertainty. Do not replay the same failure without new evidence. Repair a faulty gate within the authorized scope while retaining the original failure and its acceptance assertion.

Each verification phase ends when its stated risk is checked and the result is recorded. Reopen it only for a relevant input change or a specific unresolved finding. Freeze executable inputs before broad qualification; close subsequent prose with planning checks. A permission refusal remains pending while independent work continues. It does not justify repeated approval attempts or unrelated matrix runs.

A known operating-system signing-policy refusal must not be retried without an external policy/environment change. Retain its event, executable identity and required check as pending; run unaffected checks independently and label their scope. Do not weaken the policy, move/rename the rejected executable or claim a complete matrix from the remainder. A default/full verifier that would replay that known refusal remains pending until an approved environment is available.

## What can run now

From `cli/`, run `python tools/plan.py check`, `status`, `next`, `show <task-name>`, `coverage`, and `selftest`. This validates the documentation/task graph, spec references, coverage, local links, completed-task evidence presence and prohibited-label guard. `selftest` uses temporary synthetic documents/files to prove the checker catches missing fields, cycles, unknown dependencies, missing coverage, false completion and label leakage. It does not call a provider.

Planning traversal prunes excluded directories before entering them, so build-cache growth does not increase source-validation work. A guarded traversal regression fails if the checker enters an excluded directory; source-label and documentation checks retain their existing scope. See [gate efficiency evidence](evidence/gate-efficiency.md).

The local checker validates the contributor skill's deliberately small, two-plain-scalar-field frontmatter and links, without implementing a general YAML parser. The runtime skill has a separate offline contract/link checker. Its official skill-validator pass uses PyYAML installed only under `target/skill-validation`; this does not change the global Python environment. Neither structural check establishes host discovery or task efficiency. Upstream OpenSpec validation has not run because its CLI is not installed.

## Local executable verification

Each gate must resolve a concrete changed-behavior risk and have a stopping condition. Preserve valid results; after a fixture or implementation correction, rerun the failed case and affected checks. Keep independent scenarios separately runnable so one failure does not replay every earlier scenario. Once required checks pass, continue delivery. Documentation updates need documentation checks. Improve a repeatedly wasteful gate within the authorized scope instead of treating the gate's current design as immutable.

A repeated identical gate failure requires diagnosis before another run: record whether it is a product defect, fixture lifetime error, launcher failure or stale contract, then change that cause or collect a discriminating observation. Do not replay an unchanged matrix as a recovery strategy. Freeze implementation inputs for final qualification, retain its hashes, and close evidence/usage/handoff edits with planning checks only. Reviewers must not run Cargo or executable tests concurrently with the implementing process on Windows, where executable locks can outlive Cargo's build lock. Reopen review only for changed consequential behavior or a specific unresolved finding.

`python tools/verify.py` now selects checks from inputs changed since the last successful verification. It reports its scope and exact commands before running. `--plan` previews the selection without running checks or advancing the baseline. `--full` preserves the complete previous matrix and is required for release/CI qualification or an explicitly broad review. A missing baseline also selects the full matrix once.

Documentation-only changes run planning checks. Unchanged inputs run no checks and do not claim a new full qualification. Provider changes test that provider, dependent CLI code and its composed HTTPS hosts. CLI changes run CLI tests/Clippy and the affected composed flows; standalone cross-cutting dispatch changes select all composed hosts. Shared core/runtime/platform/test-support, toolchain and dependency changes select the full matrix. Unknown impacts widen the scope. Verifier changes run its layer/label/impact-selection selftests rather than rebuilding unrelated Rust.

Planning-review changes select the Confluence context and Jira create hosts. Nested host test modules resolve to their driver whether their directory retains the `_host` suffix or uses the shorter form. Adding a shared dispatch change still widens these selections to all hosts; source/command/environment checkpoint rules remain unchanged.

Multi-invocation fixtures must reopen invocation-bound artifact stores using each current invocation's control, matching public dispatch. Reopening retains on-disk records and conflict behavior; it must not renew a deadline within the same invocation. Do not raise fixture deadlines to hide a stale preparation control. Issue-action activation exposed and corrected this lifetime error; [evidence](evidence/jira-issue-action-activation.md) retains the failures and unchanged fault assertions.

Runtime skill files are tracked inputs. Skill-only changes select `tools/runtime_skill.py check` and `selftest`, without Rust or provider-host checks. Contract changes also check skill compatibility. The checker validates local links, cited command IDs, example command paths/flags/required scalar inputs/constraints/projections and the compatibility snapshot. Input-file examples name operator-provided files whose contents are not checked by this documentation gate; agents must use installed input validation. This is not a general JSON Schema validator. Token lengths are optional evidence: set `PYTHONPATH` to the workspace validation dependencies and `TIKTOKEN_CACHE_DIR` to the workspace tokenizer cache, then run `python tools/runtime_skill.py tokens`. Cached encodings work offline. Text length and roundtrip checks do not establish model-specific token usage or task-efficiency gains.

The baseline lives in `target/verification-state.json`; it records hashes and commands, not independent review, native-platform or live-provider approval. A failed command does not advance it. Inputs changed during checking stay unqualified; stable inputs are retained so a concurrent documentation edit does not force another Rust matrix. A focused pass is labeled as focused. Release/CI must invoke `--full` rather than infer broad qualification from the cache.

Successful commands also checkpoint to `target/verification-progress.json` before the next command starts. After a failure or interruption, a normal run reuses a pass for identical tracked source hashes, command arguments and execution environment (interpreter, platform and environment-variable digest). Failed commands never enter this checkpoint. If changes are confined to `STATE.md`, `HISTORY.md` or Markdown under `docs/`, completed Cargo format, lint and test passes survive; documentation-consuming tooling reruns. Code, fixture, tool, configuration, skill, specification, unknown-path or environment changes invalidate reuse. Missing or malformed checkpoints rerun checks. `--full` ignores prior command passes; `--plan` writes neither checkpoint. The result reports executed and reused counts separately. This local optimization does not replace native/live checks or account for changes outside the tracked inputs and environment; use `--full` when those change.

If sources change during execution, the selected commands finish but unstable passes are not checkpointed. Persistent documentation-only edits retain the existing stable-input baseline behavior and leave those edits pending. Observed drift followed by reversion, or persistent implementation drift, cannot advance qualification.

The full matrix includes [composed-host acceptance](composed-host-acceptance.md) under its explicit non-default debug feature, feature-enabled runtime regressions/Clippy and the non-debug compiler refusal. This uses synthetic credentials with real generation/cache logic and native locks; it is not native-vault or live-provider evidence. Native platform and live tests remain separate opt-ins. Restore the normal build after fixture testing when preparing a runnable binary, not after every documentation checkpoint.

The [review checklist](review.md) covers comment accuracy and semantic commit subjects. Exact dependency versions/features and platform assumptions are recorded in the [dependency baseline](dependencies.md). Tests should support the changed behavior and delivery outcome; passing more tests is not itself a product milestone.

The HTTP recorder tests require ephemeral loopback socket access. The current restricted sandbox refuses binding with Windows error 10013; run the gate in an authorized native environment with loopback access. Do not disable the socket tests or substitute a mocked pass. [Fixture evidence](evidence/test-fixtures.md) records the sandbox failure and the native test results separately.

| Layer | Required proof | Cannot prove |
| --- | --- | --- |
| Pure contract/validation | Duplicate/unknown/type/size failures, canonical digests, schema/projection agreement, legitimate Unicode preserved | Live permission or native keyring behavior |
| Subprocess | One JSON envelope, exits, no normal prompts/browser, stdin/file conflicts, bounded bytes, broken pipes/cancellation | Provider-side effect from a simulated response |
| HTTP and provider fixtures | Exact route/method/body/scope metadata, retries, redirects, pagination, async and unknown outcomes | Real tenant feature availability or distribution safety |
| Local persistence/faults | Journal atomicity/migration, chunk commit ordering, cursor remainders/replay, cancellation/crash recovery | Cross-machine fencing from a local file lock |
| Native platforms | Real principal/session vault write/read/restart/rotation/logout, locked/missing stores and large credentials on Windows/macOS/Linux | A different user's service/container session or an untested OS variant |
| Live sandbox | Actual auth/scopes/permissions, second-person page edits, files/collisions/trash/purge, async status, tenant adapter conditions | Production authority or compliance certification |
| Team authority | Real two-machine lease/fencing and duplicate attempt/crash controls | Exclusivity against a bypassing writer outside the enforcement boundary |
| Agent evaluations | Matched task outcomes, error/recovery behavior, cold/warm discovery, token/call costs, complete artifacts in supported hosts | Universal immunity to bad intent or unseen prompt injection |
| Distribution | Binary identity/integrity, install/upgrade/uninstall, skill-contract compatibility and qualified platform matrix | Permission to publish/deploy |

Use sentinel credentials and capture argv/stdout/stderr/diagnostics/journal/artifacts across errors and redirects. No real credential may appear in a fixture. Make absent test tools/credentials visible. No pass by removing tests, broadening credentials, shrinking the required platform matrix or relabeling unrun tests. Tests should check meaningful outcomes, not mirror implementation structure or inflate coverage.

## Evidence records and task closure

For completed implementation work, add a semantic Markdown record under `docs/evidence/`. It must contain:

1. Scope and paths changed, plus source/build digest or Git revision when available.
2. Exact commands/environment and observed results, with full failure evidence kept where useful.
3. Behavior/negative scenarios verified, including relevant remote effects or preserved IDs/artifacts.
4. Unrun platform/tenant/agent cases and remaining risks.
5. Independent review disposition for consequential auth, destructive effects, approval and unknown-write behavior, identifying whether review was human or another authorized reviewer.

Link that record in the task's `Evidence` field, then check the task only when its own `Done when` and dependencies hold. A file named evidence is not itself proof. Keep status and evidence changes together; partial work stays unchecked with a state note. No requirement to print task IDs in code or commit subjects. Review comments for behavior/design accuracy and remove delivery chatter.

## Planning completion audit

The package must prove: actual `huyz0` source inspection; applied lightweight SDD; explicit coding/comment standard; Rust placement and extension boundaries; detailed goals/build/verify/done/dependencies for all tasks; coverage of every capability requirement and the researched command surface; valid dependency graph and local links; useful bounded next/show output; deliberate negative controls for the helper; and an honest state saying runtime implementation is unstarted. The [planning evidence](evidence/planning-validation.md) records actual checks after they run. Product acceptance remains separate.

The verifier schedules both default and synthetic-feature Clippy checks before behavioral Rust suites. A selftest asserts both lint commands precede either test command. This ordering catches static failures before expensive workflow acceptance, retaining every previously selected command. [Completion-import evidence](evidence/ticket-completion-recovery.md) records the failure that prompted this correction.
