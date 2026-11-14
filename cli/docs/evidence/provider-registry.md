# Provider registration and execution boundary

Observed on 9 November 2026, native Windows x86_64 MSVC with Rust 1.99.1. Implemented `crates/craft-core/src/execution.rs` and extended `registry.rs`; the design records the synchronous boundary before its implementation.

Provider manifests and descriptors carry explicit contract/API versions, native route declarations, OR-of-AND scope alternatives, tenant conditions, permissions and typed retry/reconciliation classifications. Registration checks schemas/examples/projection and rejects duplicate commands/routes/modules or mismatched manifests before publishing a provider. Handlers register atomically with their module. Core does not import a provider or runtime crate.

Preparation validates the canonical request, command schema, provider/deployment binding, selected response schema and pure provider rules, then seals request/effect/descriptor digests in a private-field prepared value. The checked execution context exposes checkpoint and declared-route invocation, with no token, arbitrary URL, host, auth header or shell-command field. Undeclared routes, wrong path parameters and undeclared queries fail before the host call. The eventual HTTP host still must enforce origin, grant, policy, encoding, deadlines, wire-size and journal rules.

`crates/craft-core/tests/provider_execution.rs` compiles a synthetic provider solely against core contracts and proves:

- A registered handler executes without a core dispatch branch and records exactly one expected host call.
- Wrong provider/deployment/input identities and undeclared routes/extra parameters are refused locally.
- Changed descriptors and cancellation stop before host invocation.
- Invalid returned data and over-budget native JSON are refused after the recorded call.
- Invalid mutation responses return `WRITE_OUTCOME_UNKNOWN` with `sent_unknown`, never `not_sent` or automatic retry.
- Duplicate module/invalid route contracts leave the live registry unchanged, and undeclared reconciliation adds no call.

`python tools/verify.py` passed: plan/labels, dependency inventory/layers, formatting, 33 Rust tests and all-target Clippy. The synthetic host performs no real HTTP, credentials, journal or cloud mutation. No independent auth/write review or live provider qualification is claimed. The descriptor transport boundary alone is not a sandbox for malicious code linked into the executable.

Observed development binary SHA-256: `84719c76010785dbbb8d159f9f06faf02af71cc6c439fe89da7e21758d74de79`.

Subsequent core verification added a POST-search case: read versus mutation outcome semantics use the declared effect, never the HTTP method alone. Resource-kind mismatches are refused by the synthetic provider's pure validation. All 26 current core tests and all-target Clippy passed after this change; updated build evidence is recorded with the [test fixtures](test-fixtures.md).
