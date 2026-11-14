# Provisioning adapters and complete result output

Date: **10 November 2026**, Australia/Sydney. Partial integration and author verification. Public startup still registers thirteen local operations and no cloud providers. Provisioning is deliberately skipped by Clap and absent from the public catalog. No independent reviewer was started or approved.

## Changes

- `crates/craft-cli/src/args.rs` contains a skipped typed provisioning variant. `provision.rs` supplies its explicitly callable candidate descriptor builder, shared projection contract and a schema requiring profiles and exactly one secret source. This is a library review candidate, not a public command or feature/environment enablement switch.
- `dispatch.rs` can route that typed candidate when a trusted caller supplies its registry entry. It returns the complete provisioning envelope, retaining the request ID, error and recovery data. Other commands now acquire their final envelope at this shared boundary. Public command behavior remains under the normal compiled catalog.
- `main.rs` captures whether `--input -` consumed stdin before moving the input selector, forwards that fact and writes the final envelope without wrapping its data into a new success. The internal two-stream refusal is checked before state creation.
- `output.rs` owns serialization, one trailing newline and exit selection. Reads retain the budget-error fallback. State-changing results must preflight before mutation; if a handler nevertheless violates its budget contract, encoding retains the actual outcome/recovery data with an explicit warning. An existing failure keeps its category; an oversized success gets the budget exit category while retaining success/effect evidence.
- `contracts.rs` supports string-list flag examples and reduces examples only while the remaining object still validates. Alternative-required source selection is preserved. The public contract snapshot remains unchanged at thirteen operations.

The secure-auth/agent-interface specs and design record these boundaries. No dependency, native adapter, issuer endpoint or credential-storage policy changed.

## Observed verification

Six additional tests verify candidate multi-profile flags versus strict JSON, incompatible/missing secret sources and unknown secret fields; a double-stdin dispatch refusal before local state creation; minimal examples with alternative-required fields; complete failure/rate/warning/page/exit preservation; write-evidence retention versus bounded reads; and public subprocess refusal/discovery absence. Candidate flags use the same derived `ProvisionArgs` and example flag encoder, but do not constitute an enabled public parser path. Existing provisioning fault tests remain separate composition evidence.

The initial CLI test compile caught an attempted call to a private runtime fixture helper and a missing local control variable. Both were corrected without widening the runtime API. Failure log: `target/provision-dispatch-compile-failed.log`. The focused CLI library run then passed fifteen tests. A broader restricted run passed the new checks but failed an existing native keyring-read diagnostic; its output remains `target/provision-dispatch-cli-restricted-failed.log`.

The first native-access full gate passed all behavior tests but failed Clippy's large-enum check after the result became a complete envelope. Main now boxes that variant without suppressing the lint. Failure log: `target/provision-dispatch-clippy-failed.log`.

The final `python tools/verify.py` native-access gate passed **195 named Rust tests, zero failures and four ignored native opt-ins**, thirteen-operation snapshot comparison, ten contract controls, formatting, locked inventory/layer checks and all-target Clippy. Output: `target/provision-dispatch-verification-output.log`. The item/generation/service-cache/owned-console opt-ins were not rerun; their earlier evidence remains separate. Selftests passed nine quality and fifteen planning controls. `cargo build --workspace --locked --offline` passed. Inventory remains 278 packages. Incremental-cache hard-link warnings used copying and did not fail the gate.

SHA-256 checkpoints:

| Artifact | SHA-256 |
| --- | --- |
| `target/debug/craft.exe` | `b9348b297e6727acae0a4c27518ecbcb02ccba16f661cad70df6402272ef6897` |
| `src/provision.rs` | `06b015e23a7b34cd1c6f44f849838edbd24cc425b22775b9578f3538d6b189f4` |
| `src/dispatch.rs` | `b22ded55de149f9b599be9c105b608719dabd8f92926b2aa1d44495d691a6f62` |
| `src/main.rs` | `876aedf565c30c63d06047d5fc99889186c6a240b02cb4e543fe90f0670b4ff0` |
| `src/output.rs` | `8237f75e815b89daa2c5fcff72b282db89833ba325351aaf5dcc9ed24ac9d1bb` |

Source paths in this table are under `crates/craft-cli/`.

## Remaining acceptance

Independent consequential review remains pending under [SDD](../../SDD.md). Before public enablement, review/fix the combined input, native auth, dispatch and output flow; explicitly register the reviewed parser/catalog entry and update its public snapshots; qualify the actual public secure-input/native-envelope path and authorized live service identity/site access. The candidate tests do not perform native activation through CLI dispatch, public stdin/interactive provisioning, real issuer access or other-platform qualification. The full provider/CRUD, broker, workflow, skills and distribution scope remains unfinished. No real credential, native credential write, tenant mutation, commit, push, publication or deployment occurred in this checkpoint.
