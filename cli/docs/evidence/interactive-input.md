# Interactive credential input evidence

Date: **10 November 2026**, Australia/Sydney. Partial service provisioning implementation. No public provisioning or cloud path was enabled.

## Source and contract

- `craft-platform/src/console/` adds a single-owner terminal guard and Windows/Unix adapters. Windows FFI remains in the allowed native leaf module with checked alignment, bounded records and owned zeroizing storage.
- `craft-runtime/src/auth/prompt.rs` collects two hidden fields through the native guard, preserves Unicode/editing semantics, enforces byte limits and restores terminal state before returning. `ServiceCredentials::from_secret_parts` shares existing strict credential validation without JSON or argv conversion.
- The pipe-reader subprocess control now proves interactive input refuses a pipe before prompting. Ordinary CLI commands remain noninteractive.
- `tools/console-acceptance.ps1` builds and runs an explicitly owned hidden-console fixture. It never attaches to or changes the user's terminal.
- [Input contract](../interactive-input.md), secure-auth scenarios and design record restoration, cancellation and platform boundaries.

## Dependency change

`craft-platform` now uses the existing pinned `zeroize` 1.9.1 directly. Windows SDK bindings enable `Win32_System_Console`; Unix Nix 0.31.3 enables `fs`, `poll` and `term` for the adapter. No package version was changed or new package added; refreshed inventory remains **278 packages**. Licenses remain Apache-2.0 OR MIT for zeroize, MIT OR Apache-2.0 for windows-sys 0.61.2 and MIT for Nix. The workspace lock updates the platform dependency edge; the locked inventory records exact feature resolution.

## Observed acceptance

The explicit hidden-console acceptance passed after the final alignment expression change. The child proved exact synthetic Japanese/emoji event data, echo/line/processed-input suppression, pending-input discard, saved mode restoration through finish and drop, bounded empty polling and absence of synthetic input in its own screen buffer. Logs are `target/console-native-output.log` and `target/console-native-errors.log`; stderr is empty. Native test executable SHA-256 is `62e2f593fd446e05cc45c734b7b03514aa7de9b316bfff54c2cd8921f9075f9a`.

Four runtime prompt tests cover fixed-label output without field echo, exact Unicode editing and byte limits, cancellation/deadline, malformed encoding, invalid control data, excessive repeat input, read errors, output errors and failed restoration. A failed restoration never returns credentials. The native pipe child also refuses interactive entry when stdin is piped.

The initial full workspace gate passed. A later gate passed tests but caught Clippy's preferred alignment expression; the expression was corrected without suppressing the lint and native acceptance passed again. Logs are retained at `target/interactive-input-initial-verification-output.log` and `target/interactive-input-alignment-clippy-failed.log`.

The final `python tools/verify.py` gate passed with native filesystem/loopback access: **181 named Rust tests passed, zero failed and three opt-in tests were ignored**, plus contract snapshot checking, ten contract controls, formatting, locked inventory/layer checks and all-target Clippy. The ignored tests are two real-vault write cases and the console case; console acceptance was run separately as described above. Final output is retained at `target/interactive-input-verification-output.log`. Quality/planning selftests passed nine/fifteen controls. `cargo build --workspace --locked --offline` passed. Incremental-cache hard-link warnings used copying without failing the gate.

| Checkpoint | SHA-256 |
| --- | --- |
| `target/debug/craft.exe` | `903d7460dfbd24a89311164fb2922008afb353c5ccc2a338fc6b4fe72259cb47` |
| `crates/craft-runtime/src/auth/prompt.rs` | `66a3642c5af9e73cfdfc1a2dc93c684c76db24cd6c15bae6c0d39eed81f69c81` |
| `crates/craft-platform/src/console/windows.rs` | `9ce59774e81abed18e0265f401636f395a46f1a07b9e6812668817149adca869` |
| `Cargo.lock` | `a63dd7a920c3a5d66a4f20ce495fc78c0dc308859977087bd7ba375f24102058` |

Initial compilation also caught the Windows event-tag integer width and an unavailable test cancellation getter; both were corrected before acceptance. These exploratory compiler outputs were returned by tools rather than retained as dedicated files.

## Limits and remaining work

The native acceptance qualifies the Windows guard with synthetic console events. Runtime prompt lifecycle/error behavior uses an injected terminal. This is not a real service credential entry, end-to-end public provisioning or live OAuth acceptance. Only `x86_64-pc-windows-msvc` is installed; Linux/macOS compilation, terminal/PTY behavior and native credential backends remain unqualified.

The guard requires exclusive foreground terminal ownership. Another process can race a Windows console read after a wait; the process-local mutex cannot fence it. A forced kill can bypass restoration. Owned buffers zeroize on drop, without a guarantee for every register, native internal buffer or transient library allocation. Same-principal malicious code is outside this guarantee.

Public auth command integration, eligibility/profile-mode policy, confirmed/unknown activation output, actual native-vault service-bundle/process acceptance, live site proof and independent consequential auth review remain unfinished. No real credentials, cloud resources, commit, push, broker deployment or release were changed.
