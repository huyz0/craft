# Dependency qualification evidence

Observed on 9 November 2026, Windows x86_64 MSVC. [Dependency choices](../dependencies.md) distinguish locked current dependencies from uncompiled runtime/native candidates.

- Pinned Rust 1.99.1 compiled the locked workspace; workspace tests and all-target Clippy passed during core/local-interface qualification.
- Inspected `cargo tree --locked --offline -p craft-core --edges normal,build` and `cargo tree --locked --offline --edges dev --depth 2`. Core depends only on schema/serialization/hash/error libraries; CLI has a dev-only response validator.
- `python tools/dependencies.py --write`, then `python tools/dependencies.py`: passed, 110 workspace/resolved packages including 108 external registry packages. Each external entry has a locked registry checksum and declared license metadata. No external path or git dependency was accepted.
- Full offline metadata initially failed because an inactive-platform source, `zerocopy-derive`, was not cached. `cargo fetch --locked` retrieved it; inventory generation and comparison then passed without changing the lockfile.
- Inspected published version/features/license/MSRV metadata with `cargo info` for reqwest 0.13.4, keyring-core 1.0.1, rusqlite 0.41.1, windows-native-keyring-store 1.1.0, apple-native-keyring-store 1.0.2 and zbus-secret-service-keyring-store 1.0.1. These commands retrieved source metadata, not native qualification results.

No production credential backend or mock/file fallback is configured. Linux/macOS toolchain builds, native vault acceptance, TLS/native dependency compilation and final distribution license/notice assembly remain unrun and belong to their owning acceptance work. Baseline acceptance does not imply those platforms or integrations are delivered.
