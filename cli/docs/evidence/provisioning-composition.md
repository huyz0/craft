# Provisioning composition evidence

Date: **10 November 2026**, Australia/Sydney. Partial service OAuth delivery. The operation is deliberately unregistered pending consequential review and public integration.

## Changed artifacts

- `crates/craft-cli/src/provision.rs` defines strict non-secret provisioning arguments, explicit service-profile/shared-grant/actor binding, native input/cache composition and complete projected activation/recovery envelopes.
- `provision/tests.rs` exercises output budgets before/after qualification, replacement/staging gates, actor/target proof matching, cleanup receipts, distinct token/local-write uncertainty, rate advice, schema-valid failure data and actual trusted-profile selection/deduplication.
- Runtime service acquisition rejects reflected client ID, client secret or bearer text in scope data before producing public scope strings. Dedicated synthetic TLS tests verify refusal without secret diagnostics.
- Runtime provisioning now offers a host preflight after identity proof and before keyring replacement, rechecks token freshness afterward, and reports local credential-write state separately from possible token issuance. Existing confirmed cache receipts retain confirmed local state after later session failure.
- Pure service-target validation is shared with the composition; no generic authenticated URL or endpoint override was introduced.
- The public CLI catalog/parser and reviewed snapshot remain at thirteen local operations. No new command, dependency, feature or package version was exposed/changed by this composition.

## Validation

The final `python tools/verify.py` gate passed with native filesystem/loopback access: **189 named Rust tests passed, zero failed, three opt-in tests ignored**, plus contract snapshot comparison, ten contract controls, formatting, dependency/layer checks and all-target Clippy. Log: `target/provisioning-composition-verification-output.log`. The first gate passed tests and failed Clippy's callback type-complexity check; a named preflight type fixed it without suppression. Its log remains at `target/provisioning-composition-clippy-failed.log`.

Six composition tests passed, including a real private SQLite profile fixture. Two additional runtime tests cover reflected secrets in scope metadata and callback refusal/stale time after successful synthetic product proof, with zero keyring writes. Existing cache fault cases now assert local unknown/confirmed write states explicitly. The composition's mutation backend is an injected test fixture; runtime TLS and generation tests separately exercise their actual adapters. This is not a complete native-vault/OAuth end-to-end acceptance.

`cargo build --workspace --locked --offline` passed after final behavior-preserving comment cleanup. Quality/planning checker selftests passed nine/fifteen controls. Dependency inventory remains 278 packages. Incremental-cache hard-link warnings used copying and did not fail the gate.

| Checkpoint | SHA-256 |
| --- | --- |
| `target/debug/craft.exe` | `b4f0d7e06fd662ff42e1b584de24e6669349dbdd530289ab4eb8fcb36c9c6744` |
| `crates/craft-cli/src/provision.rs` | `dde43b26917fff81bb3d73bbe3ff53cd3f8a06a2ea1c0a9d34de6abd4d539dc8` |
| `crates/craft-runtime/src/auth/service.rs` | `8d11d9852271bba1d0c631ef7be5187a070128d1990323ced3dc191b9378692f` |
| `crates/craft-runtime/src/auth/service/cache.rs` | `30a52d0e70b511ebc028497e56c3f4ab95953ca4fda991d923931523b5e5be23` |

## Limits and next action

The [review package](../review-service-provisioning.md) identifies the code, expected invariants and activation boundary. Independent review remains pending and no public provisioning command is enabled. Later [disabled adapter integration](provision-dispatch.md) now preserves complete envelopes and stdin ownership through typed dispatch/output. Explicit public parser/catalog enablement, native/public composition acceptance, real issuer/scopes/site access, platform qualification and the full cloud CLI remain unfinished. Later [Windows service-cache tests](native-service-cache.md) qualify the internal native cache/process path using synthetic credentials; they do not exercise this CLI composition end to end.

No real credential was entered or stored, native credential item written by these new tests, cloud resource changed, broker deployed, commit/push made or release published. Generation fault tests use a memory vault with native locks, and network probes use synthetic TLS fixtures.
