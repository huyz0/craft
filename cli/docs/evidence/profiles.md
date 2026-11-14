# Local profile state evidence

Observed on 9 November 2026, Windows x86_64 MSVC, pinned Rust 1.99.1. This accepts local profile resolution/persistence, not credentials, verified actors, live accessible sites or remote mutations. The development executable now exposes `profile list/show/create/use`; no cloud provider is advertised.

## Artifacts and behavior

- `crates/craft-core/src/profiles.rs` defines closed profile/grant/hint models, semantic aliases, typed default targets and explicit/repository/active precedence. Jira/Confluence cloud templates share authority metadata; missing tenant/default ambiguity and cross-tenant/kind mismatches fail.
- `crates/craft-platform/` owns private native path guards. Existing roots/files must pass owner/ACL/link checks. Windows entries are created with protected private ACLs; checked ancestor handles and database/journal files remain pinned. The private Windows FFI exception and limits are recorded in [native safety](../native-safety.md).
- `crates/craft-runtime/src/profiles.rs` uses bundled SQLite, immediate transactions, full synchronization, persistent rollback journals, foreign keys, disabled trusted schema, bounded lock waits, strict typed payloads and application/schema markers. Foreign/future/corrupt state is refused. No arbitrary security-setting writer or credential fallback exists.
- `crates/craft-runtime/src/profile_pages.rs` preserves immutable cached remainders, installation/descriptor/projection bindings, observation/expiry metadata and bounded cursor capacity. Each invocation fetches at most one native page. Budget changes retain the same logical traversal; replay does not mutate the input cursor. This is local profile traversal, not generic provider pagination qualification.
- `crates/craft-cli/src/profiles.rs`, args, catalog and dispatch expose flags/strict JSON, field-selected schemas/results, selection conflicts and exact byte preflight before local writes. Writes declare `write`, `reconcile_before_replay` and `known_resource`; configuration templates do not add cloud capabilities. Local semantic target checks also run through `input validate`.
- The subprocess harness now supports per-process environment and working directory. Executable tests use isolated homes/state roots and bounded process execution, without changing the parent environment or native user state.

## Observed checks

| Check | Result and coverage |
| --- | --- |
| `cargo test -p craft-core --test profiles --locked --offline` | Five tests: selection/target precedence, repository security-field refusal, tenant/kind ambiguity, shared template authority and strict aliases/auth modes. |
| `cargo test -p craft-platform --test private_paths --locked --offline` with native file access | Five tests: private creation/reopen, relative/traversal/insecure-root refusal, pinned-root rename refusal, hard-linked-file refusal and junction root/ancestor refusal without writing into its target. |
| `cargo test -p craft-runtime --test profiles --locked --offline` | Eleven tests: restart, issued/shared grants, preflight/cancellation rollback, competing creates, deadline while locked, foreign/future/corrupt settings, ordered native pages, immutable/restartable/budget-adjustable cursors, cross-page traversal and bounded current-directory-only hints. |
| `cargo test -p craft-cli --test profiles --locked --offline` | Five executable cases: flags/JSON/shared grants, discovered write metadata and response-schema conformance, explicit/repository/active/ambiguous selection, malformed input before state, budget-refused writes, projected opaque pagination, isolated homes and explicit root precedence. |
| `python tools/verify.py` with loopback/native file access | Passed: plan/inventory, formatting, 85 named Rust tests, five subprocess controls and all-target Clippy. Detailed output retained at `target/verification-output.log`. Its generic native/live `not_run` marker excludes native vault/live provider acceptance; Windows path tests above did run. |
| `python tools/verify.py --selftest` | Nine quality controls passed, including allowed runtime/native dependency direction and forbidden native-leaf/provider-bypass controls. |
| `python tools/dependencies.py` | Inventory compares 131 packages: five workspace, 126 external. Nine new registry packages plus the native workspace leaf; no existing resolved version changed. Existing runtime/native feature metadata changed as intended. |
| Profile guide validation | Six documented command invocations passed against the development executable in an isolated temporary state/home, including the positive typed-default JSON example. Both JSON examples parsed and changed documentation links resolved. No fresh-reader or human usability test was performed. |
| Final `python tools/plan.py check` and `selftest` | Passed after evidence/ledger updates: ten completed entries out of 85, six capabilities, 30 requirements and fifteen checker controls. The full CLI delivery goal remains active. |

Current development executable SHA-256: `30ac97db39e7c972b4fe9ff08b64500f877c91e4dd4f3ac4ea26c19bff7f1a0c`. Native Ctrl-C evidence for the prior executable remains historical; it was not rerun against this hash.

## Failures and limits

Initial compilation caught an incorrect SQLite flag spelling, a closure lifetime annotation, SQLite's signed timestamp requirement and a Clippy newline-budget expression. They were fixed before the final gate. Two executable assertions incorrectly assumed all 512-byte calls must fail; small results legitimately fit. Revised cases use large mandatory tenant metadata for refused writes and verify that a small list fits one complete item plus lossless continuation.

The restricted runner denied hard-link creation. The first junction fixture also used a canonical verbatim path unsupported by Windows PowerShell's junction cmdlet. The fixture now passes ordinary local disk paths through environment arguments; native-access tests and the complete gate passed. Earlier traversal/collision fixture corrections are reflected in the current platform tests. These are test setup corrections, not relaxed native path checks. Compiler incremental hard-link warnings use its copying fallback; no lint or test gate was weakened.

Linux/macOS source is uncompiled/unqualified. Cross-process initialization/crash/power-loss recovery, malicious same-principal execution and administrators are not qualified isolation claims. Tests exercise concurrent SQLite connections/threads and fresh CLI processes; they do not simulate sudden power loss or a live auth principal. Native keyrings, credential refresh locks, HTTP, service/delegated OAuth, live site discovery, provider cursors, operation journals, cloud writes/reviewed-plan workflows and distribution remain separate incomplete work. Consequential paths still need their required independent review before enablement. No credentials, live tenant, Git history, publication or release were changed.
