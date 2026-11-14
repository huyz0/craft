# Workspace input and snapshot acceptance

Date: **10 November 2026**, Australia/Sydney. This records partial artifact integration and safer existing CLI file reads. It does not close the artifact-store task or qualify attachment transport, export, independent consequential review or other native platforms. No public auth/provider/artifact command was added.

## Requirement audit

| Requirement | Implementation and observed evidence |
| --- | --- |
| Explicit existing-only source authority | `craft-platform/src/inputs.rs` adds `InputRoot` and read-only `InputFile`; missing roots/files create nothing and ordinary workspace permissions are accepted |
| No path escape or linked source reads | Native tests refuse lexical traversal, absolute selections in the relative API, Windows device/stream aliases, multiple hard links and junction roots/descendants; checked root/ancestor handles remain pinned |
| OS-aware native selection | Windows source write and root/child rename attempts fail while the file guard is live; Unix source uses safe descriptor-relative no-follow APIs with source identity/change stamps, but remains uncompiled/unrun |
| Immutable, bounded upload preparation | `craft-runtime/src/artifacts/inputs.rs` streams into existing private staging; tests import empty/Unicode/multi-chunk bytes, verify digest/size, then edit the source and reopen unchanged snapshot bytes/receipts |
| No false completion on invalid input | Expected-digest mismatch, malformed digest, oversize, path escape and cancellation produce no complete artifact; collisions preserve the original artifact and missing sources remain absent |
| Public CLI integration before decoding | `craft-runtime/src/inputs.rs` supplies the checked reader used by CLI dispatch for `--input` and document `--file`; a subprocess test refuses an external hard link through both paths without echoing a sentinel |
| Preserve ordinary file/stdin contracts | CLI tests accept relative and contained absolute paths, including Windows normal/verbatim and ASCII case variants; existing byte/type/schema/stdin conflict/deadline cases pass |

The [runtime contract](../workspace-inputs.md) distinguishes source authority, private artifact authority, successful snapshots and remaining export/transport work. New FFI stays inside the existing scoped Windows module: `GetFileInformationByHandle` receives a live borrowed handle and fixed output buffer, and returns only checked identity data. No native pointer crosses the safe API. Unix reads use the existing pinned safe nix API, without new unsafe code or dependencies.

## Observed verification

- `cargo test --locked --offline -p craft-platform --test workspace_inputs` with native filesystem access passed **five tests**. Output: `target/workspace-input-platform-native-output.log`. The final full gate also passed the expanded root/child ancestor-pin assertion.
- `cargo test --locked --offline -p craft-runtime --lib artifacts::tests` with native filesystem access passed **sixteen tests**, including three new import cases and the prior publication/restart/link/cleanup cases. Output: `target/workspace-input-artifact-native-output.log`.
- `cargo test --locked --offline -p craft-cli --test local_interface` with native filesystem access passed **sixteen tests**, including the public hard-link refusal case. Output: `target/workspace-input-cli-native-output.log`.
- An earlier CLI input run passed the prior fifteen tests after the reader integration. Output: `target/workspace-input-cli-output.log`.

The first restricted platform run passed two tests and failed hard-link/junction fixture creation with access denied. A traversal fixture also lost its dot component when Windows normalized a verbatim `PathBuf::join`; the corrected fixture constructs the intended raw path text. These results remain in `target/workspace-input-platform-output.log`. No assertion or link scenario was removed. Initial compilation found a naming collision with `Read::bytes`; the source size accessor was renamed, and the subsequent check passed in `target/workspace-input-check-output.log`.

The final native-access `python tools/verify.py` gate passed **227 named Rust tests, zero failures and five ignored opt-ins**, nineteen-operation public snapshot comparison, ten public contract controls, the 99-command/112-route provider inventory, 24 inventory controls, native route-shape validation, formatting, dependency/layer checks and all-target Clippy. Output: `target/workspace-input-native-verification-output.log`. The five ignored cases are earlier native credential/console and external ADF opt-ins; they were not rerun for this input change. No public provider or provisioning path was enabled.

`cargo build --workspace --locked --offline` passed in `target/workspace-input-build-output.log`. The locked dependency inventory remains 282 packages without a manifest/lock/package-version change. Local links and fences passed for six changed documentation files. Planning checks still report fourteen completed tasks; the artifact task stays unchecked because this is partial integration.

SHA-256 checkpoints, with thirteen selected artifacts retained in `target/workspace-input-checkpoint-hashes.json`:

| Artifact | SHA-256 |
| --- | --- |
| `crates/craft-platform/src/inputs.rs` | `920a4b8c8f9e033b8ceca6f7802a0e0cf7c65aedc2914ce703e177f1296b238c` |
| `crates/craft-runtime/src/inputs.rs` | `9350037cc116cb8ae9de0a2ee83c8d541da78b0c40a0e9b1b5a67b0a7ed2855d` |
| `crates/craft-runtime/src/artifacts/inputs.rs` | `da7ecd2dc3b12739ebf30b996d2a351d86ff7c0fc6555dd13c54eb89a954d6f4` |
| `crates/craft-cli/src/dispatch.rs` | `a2f5fcded1a1e4c533d7c4be682f31981c9d6a6ee7575bdb9d63daf72e704176` |
| `target/debug/craft.exe` | `a2d04cac7cf32baad8c94f89017d01242cba720455e3d801426deb182e1bc11e` |

The existing lock and public schema snapshot hashes are unchanged from the preceding provider inventory checkpoint. This workspace has no Git revision. Compiler incremental-cache hard-link warnings used copying fallback; no test or quality gate was relaxed.

## Qualification limits

Only Windows x86_64 MSVC/Rust 1.99.1 was compiled and exercised. The Unix source API was compared with the pinned nix 0.31.3 signatures, which return owned descriptors; this is source inspection, not platform acceptance. Unix symlink/edit/FIFO behavior, source-change races on every filesystem, network-mounted filesystem stalls, power loss and malicious same-principal writers remain unqualified. The Windows tests establish sharing/pinning and fixture behavior on this host.

Public CLI file reads use the new boundary; upload snapshots remain a host library API. Independent review, external output destinations and overwrite/recovery rules, operation-context integration, real HTTP binary transfer and live attachments remain unfinished. The artifact-store ledger entry remains unchecked. No credential, real tenant resource, commit, release or deployment was created.
