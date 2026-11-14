# Local operation inspection acceptance

Date: **10 November 2026**, Australia/Sydney. This covers the public read-only `operation get` command and its local journal composition. It does not enable auth, provider mutation, reconciliation, waits or operation listing. The broader journal and reconciliation tasks remain incomplete.

## Contract and implementation

The [recovery spec](../../openspec/changes/build-cloud-cli/specs/operation-recovery/spec.md#scenario-local-operation-inspection), [design](../../openspec/changes/build-cloud-cli/design.md#local-mutation-journal) and [usage](../operations.md) define the boundary. CLI `args.rs`, `operations.rs`, `catalog.rs`, `dispatch.rs` and `contracts.rs` compose existing validated runtime journal reads. No runtime/core persistence or send-claim implementation changed. No dependency was added.

Results expose stored binding/revision and a stable named-step array. Local absence has null reconciliation knowledge. Started/unknown submissions and unknown jobs set reconciliation required. Pending and terminal job states remain distinct. Projection always retains identity, local presence, uncertainty, local scope and false live-verification/replay-authority flags. Invalid argument, semantic identity, profile selection and projection fail before state opens. The existing private state opener can initialize/migrate metadata, but operation records are unchanged.

## Observed verification

`cargo test --locked --offline -p craft-cli --test operations` passed **six tests** in `target/operation-inspection-focused-output.log`. Fresh subprocesses read actual Windows private SQLite state through the executable. The cases verify binding/digests/resource and job preservation, exact state-root selection, explicit absence, full/projected response-schema agreement, invalid/duplicate/unknown input, invalid fields/profile, local input validation, cancellation before state initialization and corrupt-state refusal. Large records fail with a bounded error, remain unchanged in storage, and can be read with explicit projection or a larger byte budget. A table covers prepared, started, confirmed and failed submissions plus accepted/running/succeeded/failed/unknown jobs; an unknown submission with known effects is exercised separately.

The first `python tools/contracts.py check` intentionally failed with only `/operations/operation.get` changed, recorded in `target/operation-inspection-contract-diff-output.log`. This is an additive local operation under contract 1.0; existing operation descriptors are unchanged. Intentional `python tools/contracts.py write` passed with **twenty operations** in `target/operation-inspection-contracts-output.log`.

Final `python tools/verify.py` passed **244 Rust tests, zero failures and five ignored opt-ins**, the twenty-operation snapshot, ten contract controls, provider inventory/controls, dependency/layer checks, formatting and all-target Clippy. Output: `target/operation-inspection-integrated-output.log`. This gate does not run separate native-vault/live-provider opt-ins. `cargo build --workspace --locked --offline` passed in `target/operation-inspection-build-output.log`. Cargo reported incremental-cache hard-link warnings and copied files instead. The unchanged lock digest is `a437de3d34dfb3a5b050f789d9275b56677e9988e4f7024ee59e38e6d5d3792d`; the new public contract digest is `7d2bc7c99092ae40902a3d70013df9d6a6c41814535d29ca076470be7d23322e`.

## Limits

This is author verification on Windows with synthetic operation records and no live provider. It proves neither an authenticated remote actor nor provider reconciliation, execution completion, cross-machine uniqueness, power-loss durability or other native platforms. The command has no credential or transport path and does not alter mutation authority. Consequential auth/write enablement still requires independent review; no independent reviewer was started or approval inferred.

Source, manifest, lock, public contract and executable hashes are recorded in `target/operation-inspection-checkpoint-hashes.json`. The debug executable SHA-256 is `69c4adfac11022cbddad56a52ef8a840adc8831fd2e3ca83c77323db7abc931a`. The ledger retains its incomplete journal/reconciliation tasks; local lookup is not provider recovery.

Final planning checks passed with 85 tasks and 14 complete. Twelve changed documentation files passed local-link/fence checks, and all 118 checkpointed files matched their digests in `target/operation-inspection-checkpoint-validation.json`.
