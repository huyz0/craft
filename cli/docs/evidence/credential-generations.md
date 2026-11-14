# Locked credential generation evidence

Observed on 9 November 2026, Windows x86_64 MSVC, Rust 1.99.1. This is partial evidence for credential generations and grant locking. Their task checkboxes remain unchecked because multi-platform native backends, external OAuth lifecycle integration and required independent review are incomplete. The full CLI delivery goal remains active.

## Implemented artifacts

`crates/craft-runtime/src/secrets/` implements bounded keyring-only indexes/chunks, exact length/digest verification, indexed staging, activation, recovery policies, inactive logout tombstones, stale-version refusal and receipt-preserving cleanup. Production sessions open only from validated profile state and its concrete native vault. Test-only fault stores are not selectable by a production constructor.

`locks.rs` retains an exclusive OS file lock under the private installation root. Lock ownership follows the process/guard, with bounded cancellation-aware acquisition and independent files per grant. No stale-file deletion or secret file fallback is used. Ownership is restricted to one principal, machine and state root; copied installations do not share refresh ownership.

`tests.rs` uses fault-injected in-memory vaults and actual OS file locks. Its cleanup field drops after root/lock handles, and checks its canonical owned path stays within `target/tmp` before removal. `tests/native.rs` uses only fresh installation/grant namespaces and synthetic bytes in the actual Windows vault. Production logout retains tombstones; test-only final cleanup removes its owned index after all chunks are deleted.

The [design](../../openspec/changes/build-cloud-cli/design.md#locked-credential-generations), [secure-auth scenarios](../../openspec/changes/build-cloud-cli/specs/secure-auth/spec.md) and [adapter contract](../native-vault.md#locked-credential-generations) describe the boundaries. No dependency or lockfile change was needed.

## Observed validation

| Check | Observed result |
| --- | --- |
| `cargo test -p craft-runtime --lib secrets::tests --locked --offline` | 13 tests passed; one real-vault acceptance test ignored. Includes one subprocess helper that does no work outside its explicitly selected child invocation. |
| Fault injection | Every one of seven write and twelve read boundaries was reached, then restart-style recovery retained a complete old or new value. Before/after faults at every retired-chunk deletion verified idempotent cleanup and confirmed receipts. |
| Strict/corrupt state | Missing/null/unknown fields, wrong binding/version/manifest limits, duplicate generations, bad digest and wrong chunk sizes were refused. Locked/unavailable vaults preserved staging intent without writes/deletes; corrupt staging was never activated. |
| Versions and cancellation | Exact Unicode and maximum 1 MiB binary round trips passed. Stale/cross-grant versions and logout revival were refused. Cancellation after confirmed activation preserved its version and deferred cleanup. |
| Native kernel locks | A ready-confirmed child held one grant. The parent hit its same-grant deadline, acquired another grant, killed/waited the child and reacquired the first grant. Its fixture directory was removed after handles closed. |
| Explicit real Windows vault acceptance | `cargo test -p craft-runtime --lib --locked --offline windows_generations_survive_restart_recover_and_logout -- --ignored --exact secrets::tests::native::windows_generations_survive_restart_recover_and_logout` passed with native access. Oversized Unicode/binary payloads survived fresh-process reads; child logout, stale refusal and incomplete-stage recovery passed. All owned native entries and fixture files were removed. |

The integrated `python tools/verify.py` gate passed 116 named Rust tests, five subprocess controls, formatting, locked dependency inventory and all-target Clippy. Two opt-in native-vault tests were ignored by that gate; the generation acceptance above passed separately, and the earlier item acceptance remains recorded in [native item evidence](native-vault.md). Output is retained at `target/generation-verification-output.log`. Quality-checker selftests passed nine controls and planning-checker selftests passed fifteen. The inventory is unchanged at 278 packages, including five workspace members.

Current development executable SHA-256: `9a5fc9404d4c3b745f83db8013f7bcf6e64dd3e869cc0333c5f1fec4d61a3a2c`. Native acceptance ran the runtime unit-test executable and fresh child processes; no auth command is implied in this CLI binary. Incremental hard-link warnings used the compiler's copying fallback; no gate was relaxed.

## Limits and next integration

Native macOS/Linux tests, unavailable Windows logon sessions, other principals/services/containers, actual refresh/logout races, power-loss durability, copied-installation migration and independent consequential review remain unrun. The Windows restart test exercises process restarts, not OS restart or power failure. Fault recovery does not turn transient vault unavailability into proof of missing data.

The generation adapter does not make native calls interruptible and cannot recover a remote rotation response lost before staging. The auth protocol still needs external exchange intent, broker/provider integration and its own recovery decisions. No credential-facing cloud command is enabled. Real OAuth credentials, live tenants, remote resources, commits, pushes and releases were unchanged.
