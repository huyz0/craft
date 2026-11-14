# Core contracts and local interface

Observed on 9 November 2026 on Windows x86_64 MSVC with pinned Rust 1.99.1.

Implemented provider/deployment, tenant, resource and opaque grant references in `crates/craft-core/src/contracts.rs`. Local validation preserves raw identities, checks field/size bounds and validates request preconditions. `presence.rs` provides omitted/null/value patch fields; containing models opt into omission rather than serializing absence as null. Remote job state remains separate from request submission state. Failure envelopes can retain receipts with confirmed resources for partial or unknown outcomes.

Core has no configuration, filesystem, HTTP, keyring or clock calls. `cargo tree --locked --offline -p craft-core --edges normal,build` confirmed no runtime/provider/workflow dependency. JSON Schema's default resolver features are disabled.

`crates/craft-core/tests/contracts.rs` covers identity preservation and separation, wrong wire types, unknown keys, canonical target/digest changes, mismatched parent tenants, omitted/null/value schema and serialization, distinct accepted jobs and confirmed submissions, and partial/unknown failure envelopes retaining confirmed effects. Parser tests exercise duplicate keys, Unicode, trailing input, root-object requirements, byte/depth/container bounds. Registry tests exercise strict schemas, external reference refusal, registration collisions and response consistency.

`crates/craft-cli/` implements local version, command search, input validation, schema views, provider listing, capabilities and JSON help. These are development operations, not cloud capability claims. Subprocess tests validate actual envelopes against their response schemas and check secret-free errors and complete bounded output.

Checks observed:

- `cargo fmt --all`: passed.
- `cargo test --workspace --locked`: passed, 21 tests.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed. The earlier nested-if test lint was corrected with a let-chain that still closes child stdin. A manual generic default also triggered Clippy; restoring the derived enum default corrected it without a lint exception.
- Native compiler emitted incremental-cache hardlink warnings and copied files instead. No lint gate was relaxed.

Subsequent [provider registry](provider-registry.md) and [local projection](field-projection.md) records describe the added handler boundary and selected output. Full provider projection/paging, process-signal cancellation, native vaults, cloud clients and live qualification remain incomplete. No tests above establish live permissions or remote write recovery.
