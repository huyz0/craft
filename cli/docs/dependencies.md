# Dependency baseline

Qualified development host: Windows x86_64 MSVC, Rust **1.99.1**, edition 2024. Both compiler pin and declared minimum Rust version are 1.99.1. A lower MSRV has not been tested. Rustfmt and Clippy are pinned by `rust-toolchain.toml`. Python 3.11+ supports documentation tooling only; the executable never launches Python.

## Resolved dependencies

Workspace dependencies use exact versions and `Cargo.lock` pins the full graph. [The generated inventory](evidence/dependency-inventory.json) records 276 external packages and eight workspace packages across resolved target/dev graphs, including registry sources/checksums, manifest hashes, declared licenses, selected features, declared Rust versions and normal/build/dev edges. Jira and Confluence provider modules depend only on core within the workspace; CLI composes them, and runtime imports Jira only for development integration tests. The Confluence addition introduces one workspace member and a CLI composition edge, with no external package/version/source changes. `python tools/dependencies.py` compares locked offline Cargo metadata with that inventory. Refresh it with `--write` only after reviewing dependency changes. A missing cached platform crate requires `cargo fetch --locked` first.

| Package | Version | Rationale and selected features |
| --- | --- | --- |
| clap | 4.6.7 | Typed CLI parser and built-in human help; `derive` plus defaults. Machine errors are sanitized separately. |
| serde | 1.0.229 | Explicit closed wire models; `derive`. |
| serde_json | 1.0.151 | Bounded JSON parsing/encoding. A custom visitor detects duplicate keys. |
| schemars | 1.2.3 | Generates JSON Schema from the same Rust models used by the CLI. |
| jsonschema | 0.61.0 | Local schema validation; all default features disabled, including HTTP/file reference resolution. Registry also rejects external references. |
| sha2 | 0.11.1 | Versioned canonical request and artifact digest primitives. |
| thiserror | 2.0.20 | Typed errors at Rust boundaries. |
| zeroize | 1.9.1 | Redacted, nonserializable vault buffers zeroize on drop; workspace selects `alloc` with defaults disabled. The native Windows dependency also enables its default alloc feature. Apache-2.0 OR MIT, declared MSRV 1.85, compiled with the pinned workspace toolchain. |
| serde_path_to_error | 0.1.20 | Strict typed input errors with field/index paths, without echoing raw rejected values. MIT OR Apache-2.0, declared MSRV 1.61. |
| ctrlc | 3.5.2 | Native Ctrl-C handler in runtime; defaults disabled. Legacy MIT/Apache-2.0 declaration, MSRV 1.69. Native Windows console delivery passed; macOS/Linux signal acceptance remains unrun. |
| rusqlite | 0.41.1 | Transactional non-secret profile/cursor metadata; defaults disabled, bundled SQLite. Persistent rollback journals keep native writable paths pinned. Compiled and exercised with MSVC; MIT, declared MSRV 1.89. |
| getrandom | 0.3.4 | Native randomness for opaque installation, grant and cursor references; no token generation or credential export. MIT OR Apache-2.0. |
| windows-sys | 0.61.2 | Native leaf wrappers for Windows token SID, protected ACLs, handle checks and guarded console input. Selected features include `Win32_System_Console`; unsafe FFI stays in native modules. MIT OR Apache-2.0. |
| nix | 0.31.3 | Unix identity/mode checks and the nonblocking terminal adapter; defaults disabled, `user`, `fs`, `poll` and `term`. Source inventoried; Unix compilation and qualification remain unrun. MIT. |
| keyring-core | 1.0.1 | Direct credential API for the concrete Windows store; no global default is selected/consulted. `sample` feature disabled. Its built-in mock exists upstream but is used only by our explicit test negative control and is never a production selector. MIT OR Apache-2.0, MSRV 1.85. |
| windows-native-keyring-store | 1.1.0 | Windows-only dependency, defaults/search disabled. Binary items use explicit local-machine persistence. Synthetic item and generation restart/logout/recovery tests passed; actual OAuth lifecycle and auth CLI remain separate. MIT OR Apache-2.0, MSRV 1.88. |
| reqwest | 0.13.4 | Defaults disabled; `rustls` and `json` only. Async I/O under the synchronous port supports cancellation without a lingering blocking request worker. No cookies, system-proxy, compression, HTTP/2 or HTTP/3 feature is selected. MIT OR Apache-2.0, MSRV 1.85. |
| tokio | 1.53.1 | Explicit `rt`, `time`, `macros`, `net`; indirect features supply I/O and synchronization. A private current-thread runtime drives the host port. |
| url / percent-encoding | 2.5.8 / 2.3.2 | Typed path/query construction and strict UTF-8 continuation decoding. Invalid encoding fails rather than normalizing an opaque token. |
| httpdate | 1.0.3 | Parses bounded HTTP-date rate advice without retaining arbitrary header text. |
| rustls / rcgen | 0.23.46 / 0.14.10 | Dev TLS server and generated fixture certificates, using AWS-LC. Rustls is also selected transitively by reqwest; rcgen is a runtime dev dependency and absent from the Windows production CLI dependency tree. |

Runtime now includes internal HTTPS read transport, Windows keyring-only credential generations and native artifact storage with OS file locks, without enabled credential-facing cloud commands or a selected fake-vault default. Core normal/build dependencies contain no runtime, provider or workflow crate. Runtime owns clock, input-worker, signals, non-secret SQLite metadata, native credential/artifact access and trusted HTTP; `craft-platform` is a native leaf with no local crate dependency. Test support remains dev-only for the executable/runtime. CLI's JSON Schema dev dependency exercises emitted envelopes against generated contracts. Generation/locking work used existing dependencies and safe standard-library file locks. Artifact streaming adds a direct runtime edge to the existing pinned SHA-256 library; no package, version or feature changes were introduced, and the refreshed inventory remains 278 packages.

Adding TLS/HTTP resolved 143 packages without changing any preexisting package version. MSVC/CMake successfully compiled AWS-LC. The source/license inventory also covers platform-only, optional and development resolution; it is not a list of libraries linked into the Windows CLI. Additional obligations include AWS-LC's combined ISC/Apache/MIT/BSD declarations, trust-root CDLA-Permissive-2.0 data and existing Unicode/LLVM-exception alternatives. Source metadata remains evidence rather than distribution clearance. See [transport evidence](evidence/trusted-http.md).

Adding the native handler resolved platform-only `block2`, `dispatch2`, `objc2`, `objc2-encode`, `nix` and `cfg_aliases` sources. Their license/source metadata is inventoried; only the Windows target has compiled and run. Initial online resolution also advanced unrelated existing bitflags, libc and wasm/js packages. Those version changes were restored with precise offline updates before refreshing the inventory; the final addition preserved every preexisting package version.

External metadata declares MIT/Apache alternatives, MIT, MIT-0, Zlib, Unicode-3.0 obligations, Unlicense/MIT alternatives and BSD/Apache/MIT alternatives. One package offers an LGPL alternative alongside MIT/Apache; that does not require choosing LGPL. Some registry manifests use legacy slash license syntax. The inventory preserves exact declarations rather than silently normalizing them. Declared metadata is source evidence, not legal clearance or a complete distribution notice bundle. Craft has no assigned redistribution license and remains `publish = false`; packaging must retain required notices and resolve project licensing before publication.

## Document parser additions

The new pure `craft-workflows` member pins `pulldown-cmark` 0.13.6 with default features disabled. Its optional HTML emitter, command parser, generated tests and SIMD features are not selected; its build script is a no-op without generated tests. The cached/fetched source declares MIT and Rust 1.71.1. New transitive `unicase` 2.10.0 declares MIT OR Apache-2.0, no build script and no explicit MSRV. `quick-xml` 0.41.0 now supplies checked storage XML import as a normal workflow dependency with defaults disabled; it declares MIT and Rust 1.79. The existing pinned jsonschema library is also a workflow dev dependency for opt-in acceptance against the separately fetched, checksum-pinned Atlassian schema. Source manifests, feature selection and lock checksums are inventoried. The parser sources were inspected locally; these facts are not a complete security or redistribution clearance.

Lock comparison against `target/document-rendering-before.lock` found no removed or changed preexisting package/version/source tuple. Only the workflow member and three external packages were added, making 282 total. The workflow library is now linked into the public CLI for local plan/design authoring. It adds direct normal edges to the already pinned serde and schemars libraries; no package versions were added or changed for that integration. The refreshed inventory records these edges and the CLI-to-workflow dependency. Renderer behavior and remaining Cloud qualification are recorded in [document content](document-content.md) and [planning usage](planning.md).

## Runtime selections for owning implementations

Composed-host acceptance adds CLI dev edges to the already pinned `rustls` and `rcgen` packages and explicit non-default fixture features in CLI/runtime manifests. Only the CLI dependency-edge record changed in `Cargo.lock`; no package/version/source/checksum was added, removed or changed. Inventory remains 283 packages. `rcgen` and `craft-test-support` remain absent from the normal CLI dependency tree. Runtime's memory/loopback fixtures use existing dependencies, are omitted by default and are rejected without debug assertions. See [acceptance isolation](composed-host-acceptance.md). These explicit development fixtures do not select a production credential fallback.

The remaining macOS/Linux vault candidates below have published metadata/source inspected on 9 November 2026. They are not selected credential adapters. Security-framework source also appears transitively through native TLS verification; that does not implement a macOS vault. Windows item access and HTTP are in the locked graph; the SQLite row distinguishes its implemented local scope from remaining runtime work.

| Boundary | Candidate | Features and prerequisites to qualify |
| --- | --- | --- |
| macOS vault | apple-native-keyring-store 1.0.2, MIT OR Apache-2.0, MSRV 1.85 | Explicit `keychain` feature, not an implicit backend. Native Security framework, persistent Keychain and unlock/access behavior require a real macOS session. |
| Linux vault | Replacement needed for the inspected zbus-secret-service-keyring-store 1.0.1 wrapper | Its source automatically unlocks items/collections and can prompt, so it is not adopted unchanged. Require non-prompting operations against an existing persistent collection, reachable session bus, encrypted transport and no kernel/session-only/file fallback. |
| Operation journal/provider cursors | The locked rusqlite 0.41.1 baseline | Local profiles and their cursors now exercise bundled SQLite, transactions, version refusal, bounded busy waits and private paths. Generic operation journals, provider cursors, power-loss acceptance and cross-platform native qualification remain separate work. Credential grant locks use OS file locks rather than SQLite transactions. |

Candidate versions are not a support claim. Update this baseline and the exact dependency pins when qualification establishes a required change. Do not invent a new OAuth flow or weaken native-store requirements to fit a crate.

The attachment-download adapter adds a CLI composition edge to the existing `craft-platform` workspace leaf for guarded output authority. Offline lockfile/inventory refresh changed no external package version or feature. The inventory still contains 284 packages (276 external and eight workspace packages).

The multipart transport adds direct runtime edges to the existing bytes 1.12.1 and http-body 1.1.0 packages, using reqwest Body::wrap. An offline inventory comparison confirmed unchanged external package versions, sources and selected features; the resolved graph remains 284 packages. No stream/multipart client feature or new background worker was enabled. Native snapshots and file frames remain bounded.
