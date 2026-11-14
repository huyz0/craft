# Planning gate efficiency and recovery compile correction

On 11 November 2026, the planning helper was traversing the entire tree with `rglob` and excluding build output only after traversal. Every `next` and `show` also runs that validation. The two observed navigation calls each exceeded the tool's ten-second initial wait.

`tools/plan.py` now uses top-down directory pruning with the existing exclusion sets, preserving deterministic file order, source-label checks, Markdown links and JSON examples. Traversal errors remain failures. The acceptance scenario is that an excluded build directory is never entered, while a planning label in included source is still rejected. A guarded `os.scandir` regression raises on entry into an excluded directory, including nested invalid-UTF-8 fixtures that must never be read.

Observed checks:

- `python tools/plan.py selftest`: 17 controls passed, including guarded traversal and existing source-label rejection.
- `python tools/plan.py check`: passed, 87 tasks, 16 complete, six capabilities, 31 requirements, 15 JSON examples. Measured wall time was 0.340 seconds using a PowerShell stopwatch. The earlier navigation timings are observations, not a controlled benchmark.
- `python tools/verify.py --selftest`: ten controls passed, including impact selection and unchanged-command resumption. Output: `target/gate-efficiency-selftest.log`.
- `cargo check -p craft-cli --all-targets --locked --offline`: passed after correcting two swapped closure result annotations in `crates/craft-cli/src/issue_links/recovery.rs`. Output: `target/link-lifecycle-recovery-check-fixed.log`. This is compile evidence, not link recovery acceptance.
- `cargo test -p craft-cli --lib issue_links::tests --locked --offline`: three focused receipt regressions passed, covering unexpected same-step resources, foreign saved records and unknown receipt acknowledgment. Output: `target/link-lifecycle-focused-regression.log`. These existing cases do not test the new recovery host.

No completed Rust workspace qualification was repeated. No baseline was advanced, command activated, task closed, tenant contacted or credential accessed. Link inspection/deletion/recovery remains in progress and requires its own behavioral acceptance before activation. This tooling correction does not qualify those operations or the full CLI.

## Checkpoint reuse after prose edits

On 11 November 2026, inspection found another avoidable restart: the command checkpoint compared the entire tracked-file snapshot. A handoff or evidence edit after a later failure discarded completed Cargo checks, even though no Rust execution input changed. `tools/verify.py` now retains only Cargo format, Clippy and test passes when changes are confined to `STATE.md`, `HISTORY.md` or Markdown under `docs/`. Tooling checks rerun because they can consume documentation. Specifications, skills, fixtures, code, tools, configuration, unknown paths and environment changes still invalidate the checkpoint. Exact command matching, in-run drift refusal and explicit `--full` bypass remain in place.

The workflow scenario in `openspec/specs/project-workflow/spec.md` defines this boundary. Validation guidance and the handoff describe it. No product contract or native write authority changes.

Observed validation:

- `python tools/verify.py --selftest`: passed eleven control groups. Log: `target/prose-checkpoint-selftest.log`. New isolated scheduler cases replace subprocess execution with a recorder: after a prose-only edit three Cargo commands are reused, a tooling failure remains a failure, its retry runs only tooling, checkpoint rollover retains all valid passes, and explicit full mode executes all four commands. These are verifier tests, not Cargo or product qualification.
- Negative controls reject reuse after changed code, tool, lockfile, skill, specification, dependency inventory, crate Markdown, unknown paths, environment or malformed checkpoint data. Existing actual-subprocess checkpoint/failure and in-run drift controls also pass.
- `python tools/verify.py --plan`: selected planning validation and verifier selftest only, with no Rust packages or composed hosts.
- `python tools/verify.py`: passed both selected checks, zero reused top-level commands, no drift. Planning validation reported 87 tasks, 16 complete and no errors. Log: `target/prose-checkpoint-verification.log`. The command's measured wall time was 1.94 seconds; this is an observation, not a comparative benchmark.
- Authorized independent reviewer `/root/skill_review` inspected the changed source and boundaries read-only and found no blocker. The reviewer ran no verification or build commands.

The correction preserves prior product evidence. It does not establish live-provider behavior, finish comment writes or qualify the full CLI.

## Failure diagnosis and stopping rule

The user clarified that gates are project work products and must be improved when they cause repeated work. `docs/validation.md` now requires a diagnosis and changed evidence before retrying a failed check, an explicit stopping condition, preserved valid results and no repeated permission attempts while authorization is pending.

The binary transport review provided a concrete application: unsupported transfer codings could publish encoded bytes. The independently runnable regression failed against the original implementation (`target/binary-transfer-coding-before.log`), then passed after restricting transfer coding to absent or one chunked header. `cargo test -p craft-runtime --lib transport::downloads --locked --offline` passed all twelve focused cases (`target/binary-download-reviewed-tests.log`). `cargo clippy -p craft-runtime --all-targets --locked --offline -- -D warnings` passed (`target/binary-download-reviewed-clippy.log`); Cargo reported incremental-cache hard-link fallback warnings. Authorized source-only reviewer `/root/skill_review` confirmed the finding resolved with no remaining review finding. No full matrix was repeated.

This is focused foundation evidence. Binary download remains unregistered, filesystem/export composition is untested for this API, and native/live checks remain unrun. The installed 75-command bundle and broader task completion status are unchanged.
