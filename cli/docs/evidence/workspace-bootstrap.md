# Rust workspace bootstrap

## Scope

Created `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `crates/craft-core/` and `crates/craft-cli/`. The executable identifies itself as Craft AI Factory CLI and supports version/help. Cloud resources, JSON command contracts and auth remain unimplemented.

The compiler is pinned to Rust 1.99.1 with the minimal profile, rustfmt and Clippy. Existing older versioned toolchain entries lacked a compiler; the exact 1.99.1 toolchain was installed through the authorized tool escalation. Rustup's pending self-update cleanup also required elevation. The first build attempt failed on that cleanup; after it completed and the toolchain file's table was corrected, the full checks below passed.

## Checks

From `cli/` on native Windows x86_64 MSVC:

- `cargo generate-lockfile`: passed; initial workspace has no third-party dependencies.
- `cargo metadata --no-deps --format-version 1`: passed; two packages, both unpublished, core independent of CLI.
- `cargo fmt --all`: completed.
- `cargo build --locked`: passed.
- `cargo test --workspace --locked`: passed, including the subprocess distribution/version identity check.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `target/debug/craft.exe --version` and `--help`: passed; help explicitly says cloud operations are not implemented.

## Remaining

This qualifies bootstrap on the current Windows development host only. No Linux/macOS build, provider/schema/auth behavior, independent review or release package qualification is claimed. Product task labels are absent from source/comments and the task ledger retains traceability here.
