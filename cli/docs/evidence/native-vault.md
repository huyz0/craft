# Windows native item adapter evidence

Observed on 9 November 2026, Windows x86_64 MSVC, Rust 1.99.1. This is partial evidence for the multi-platform native backend task. Its checkbox remains unchecked: macOS/Linux adapters and qualification, native unavailable-session cases and consequential auth review are incomplete. The full CLI delivery goal remains active.

## Implemented artifacts

`crates/craft-runtime/src/vault.rs` implements explicit Windows Credential Manager selection, versioned installation-bound item identities, binary secret I/O, local-machine persistence checks, native size refusal, redacted error mapping and read-only principal/access health. It constructs the concrete store and never registers or consults the process-global default. The ordinary constructor has no fake or file option. The core secret wrapper remains nonserializable and redacts Debug output; byte-bearing native error payloads are zeroized before returning a typed error.

`crates/craft-platform/src/lib.rs` exposes the existing owned-token principal lookup through a safe API. It returns identity metadata without credential enumeration or a raw handle. No additional unsafe scope was added to runtime/core. Existing native FFI remains in the platform leaf; third-party native keyring FFI is pinned registry source.

The design and secure-auth scenarios record item identity/persistence, no global-store redirection, honest health, uncertainty and the Linux wrapper's implicit unlock problem. `Cargo.toml`, the runtime target dependencies, lockfile and [source/license inventory](dependency-inventory.json) pin the selected Windows crates with optional search and sample features disabled.

## Observed validation

| Command/check | Observed result |
| --- | --- |
| `cargo test -p craft-runtime --lib --locked --offline` | Three deterministic tests passed; the real-vault test was explicitly ignored. Tests cover structured identity collisions and installation separation, unsupported backend refusal and redacted native errors with mutation uncertainty. |
| `cargo test -p craft-runtime --lib --locked --offline native_windows_vault_round_trip_restart_limits_and_persistence -- --ignored --exact vault::tests::native_windows_vault_round_trip_restart_limits_and_persistence` with native access | One real Windows vault test passed. It used a fresh random installation namespace and synthetic binary bytes, read the data from a fresh subprocess, observed `Local` persistence, refused a 2,561-byte mutation while preserving the 2,560-byte entry, rejected a session-only entry on read/write/delete and explicitly removed its entries. A mock global keyring default did not redirect native operations. |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | Passed after integrating the native adapter and tests. |
| `python tools/verify.py` with native filesystem/loopback access | Passed: 88 named Rust tests, five subprocess controls, formatting, locked inventory and all-target Clippy. One opt-in real-vault test was ignored by this ordinary gate; its separate observed pass is above. Output retained at `target/vault-verification-output.log`. The gate's generic native/live `not_run` marker does not replace the explicit native result. |
| Dependency inventory review | Four new registry packages: keyring-core 1.0.1, windows-native-keyring-store 1.1.0, byteorder 1.5.0 and log 0.4.35. No existing package version changed. Windows credential API and zeroize default/alloc feature metadata changed as expected. Inventory now contains 135 packages, five workspace and 130 external. |

Current development executable SHA-256: `e89036c57f61a29af317371835fb3dd880d23948a0f5d0cae5a1de120000f2bf`. The native test ran the runtime unit-test executable and a fresh copy of that test process; it does not claim an auth command exists in this CLI binary.

## Source findings and limits

Offline dependency resolution initially failed because `byteorder` was not cached. Authorized ordinary `cargo fetch` added the four reviewed packages; subsequent checks used locked offline sources. Compiler incremental-cache hard-link warnings fall back to copying; no test/lint gate was relaxed.

Inspected published source: keyring-core 1.0.1's direct-store API and error payloads; windows-native-keyring-store 1.1.0's binary operations, explicit `Local` modifier, native item limit, native-buffer erasure and error mapping; apple-native-keyring-store 1.0.2 and security-framework 3.7.0's Keychain interaction behavior; zbus-secret-service-keyring-store 1.0.1's automatic item/collection unlock calls. These were cached registry-source inspections, not native macOS/Linux executions or claims that all candidate behavior is accepted.

Windows's generic item backend reports a missing credential logon session as unavailable. The current test does not induce that OS condition or claim to test an independent Windows vault-lock state. Redaction/unavailable mapping has deterministic tests; real unavailable-session, other principal/service/container and macOS/Linux locked/unavailable acceptance remain unrun. The health probe reports read accessibility only, with write accessibility unknown.

The current mutex covers one adapter instance. Cross-process grant locks, variable-size generation activation, refresh/logout races, crash/power-loss handling and outer invocation cancellation remain incomplete. No credentials are stored in SQLite, files, logs, argv or artifacts. The native acceptance writes only synthetic, randomly namespaced fixtures and deletes them; real OAuth material, live providers and remote resources are untouched. Required independent review has not occurred, so no credential-facing CLI path is enabled by this work.
