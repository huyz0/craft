# HTTP and subprocess fixtures

Observed on 9 November 2026, Windows x86_64 MSVC, Rust 1.99.1. The shared fixture task now includes the HTTP/subprocess slice and the clock/vault/journal/artifact port adapters described below. Native durability, real credential stores and runtime dispatch integration remain separate acceptance work.

Initially added `crates/craft-test-support/` without dependencies for HTTP/subprocess helpers. It now depends on core contracts for the state adapters. `craft-cli` uses it only as a dev dependency. `cargo tree -p craft-cli --locked --offline --edges normal,build --depth 1` excludes test support, while `--edges dev --depth 1` includes it. The current locked inventory contains 112 packages including 109 external packages. `python tools/dependencies.py` validates it.

The loopback HTTP recorder checks expected method/target/body/header subsets, counts actual accepted requests and retains observed requests. It scripts delayed responses, arbitrary response bodies/status/headers and a disconnect only after reading the complete request. Missing, unexpected and mismatched exchanges fail verification. Stopping during a pending delayed response is a failure, not proof of delivery. It is an HTTP/1.x test fixture with bounded headers and explicit content-length bodies; it is not a production parser, TLS server or chunked streaming fixture.

The subprocess harness captures stdout/stderr concurrently, bounds both streams, closes stdin on completion/early exit and kills/reaps the direct child on deadline. CLI interface tests now use this harness instead of unbounded wait/output collection. A harness-free integration executable invokes itself in controlled child modes; its five assertions exercise stream pressure, stdin/EOF, deadline, capture bounds and early input closure without ignored tests or shell commands. Descendant process-tree control is not implemented or claimed.

Observed verification:

- The first `python tools/verify.py` run passed layer/plan/inventory/format checks and existing Rust suites, then failed the new socket fixtures with Windows error 10013. This sandbox does not allow loopback binding; the failure was retained rather than skipping or weakening tests.
- `cargo test --workspace --locked` with authorized local socket access passed 36 named tests plus all five subprocess controls at that point.
- After adding the delayed-shutdown control, `cargo test -p craft-test-support --locked` with local socket access passed all four HTTP tests and five subprocess controls.
- A subsequent declared-effect correction added one core test: native POST searches retain read outcome semantics. `cargo test -p craft-core --locked` passed all 26 current core tests, including that check and mutation response uncertainty. There are now 38 named workspace cases, with the latest additions covered by these targeted runs.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo fmt --all --check`, inventory comparison, six guard controls, 15 planning controls and planning/label validation passed after the changes.
- `cargo build --locked -p craft-cli` passed; direct version and nested field-selection invocations returned complete JSON. Final development binary SHA-256: `1155449ae093934c51319c845ce5a47b9435714a28867c1d394ed58a346af301`.

Run the combined development gate from a native environment that permits loopback sockets. Native credential stores, actual HTTP transport/provider semantics and cloud writes remain unqualified. These fixtures do not prove remote effects in a live tenant.

## Clock and state port qualification

Added pure core ports in `crates/craft-core/src/storage.rs` and dev-only adapters under `crates/craft-test-support/src/state/`. The reviewed design records wall versus monotonic time, redacted/zeroizing vault bytes, immutable operation bindings, journal CAS/transition rules and staged artifact publication. No runtime provider execution context exposes a credential store.

Seven state-port tests pass: wall adjustments and overflow leave monotonic time valid; vault before/after-commit failures and locked/unavailable modes preserve the correct secret state; journal lost acknowledgments retain intent/send markers; two concurrent CAS attempts cannot lose an update; resource/job receipts and terminal state cannot regress; partial/failed artifact writes remain unpublished; lost artifact acknowledgments reconcile by complete receipts; and competing writers preserve the first complete result. The known `abc` SHA-256 vector validates raw artifact hashing independently of JSON canonicalization.

The integrated HTTP/journal test injects failure before intent and observes zero actual requests. It then commits a send marker, reads a full POST at the loopback server before disconnect, injects a post-send journal failure and reopens shared backing state. The persisted send marker requires reconciliation and the test sends no duplicate. This qualifies the fixture's controls, not production dispatch or a real remote transaction.

`cargo test --workspace --locked` with authorized loopback access passed **46 named tests and five subprocess controls**. Formatting, all-target Clippy with warnings denied, dependency inventory comparison, six guard selftests and dev-only Cargo-tree inspection passed. The new zeroize 1.9.1 dependency was inspected through published source/license/MSRV metadata and built on the current host. Memory adapter clones simulate reopened backing storage; no process or power-loss durability, OS vault access, file path safety or SQLite migration is established by them.
