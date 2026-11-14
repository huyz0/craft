# Provider continuation acceptance

Date: **10 November 2026**, Australia/Sydney. Internal provider-neutral library and private-state migration. No cloud listing command, live tenant operation, credential provisioning, release or publication was enabled.

## Changed behavior

The [library contract](../provider-pagination.md), [design](../../openspec/changes/build-cloud-cli/design.md) and [capability scenarios](../../openspec/changes/build-cloud-cli/specs/agent-interface/spec.md) define bound immutable handles, lossless remainder retention, one native fetch per invocation, byte/item limits, explicit oversized-item recovery, access-check ordering, provenance and live consistency. Core types and error codes are in `crates/craft-core/src/pagination.rs` and `error.rs`. The runtime engine and persistence are in `crates/craft-runtime/src/pagination/`. SQLite schema four migrates the private profile store, preserving previous tables and settings format one. Existing migration fixtures now simulate the actual earlier schemas.

The public schema snapshot intentionally adds eight error variants and changes derived projection fingerprints. Public discovery still has twenty local operations and no cloud provider. Old schema-bound cursors may require a new traversal after upgrading; database-row preservation does not override binding validation. Packages, manifests, dependency lock and provider inventory are unchanged.

## Observed verification

One new core test rejects credential parameter names, control characters and excessive/empty native positions without echoing supplied values, while preserving legitimate opaque Unicode/encoded token data. Twenty new runtime tests exercise actual private SQLite state with normalized in-process sources:

- Item/byte remainders, adjusted budgets, restart, terminal and empty/short nonterminal pages, observation preservation and no fetch while buffered.
- A 1,010-record traversal through byte-limited output and an empty native page, with exact ordered ID equality through exhaustion and four native fetches.
- A 5,000-character first record returning a bounded full error envelope and a recovery cursor; retry retains that record before the following record.
- Input/recovery-budget preflight with no source access; ten semantic binding changes refused before source access.
- Cached access denial and revocation before publication, without exposing buffered output or a successful successor.
- Concurrent independent replays returning identical buffered records and distinct successor handles; live native-position replay explicitly observes different provider data.
- Repeated/cyclic positions, fixed traversal expiry across buffered/native resumes, expiry before publication, payload/format/history/continuation corruption and cross-installation/missing handles, with no implicit restart.
- Native record/byte bounds, 256-handle and sixteen-MiB aggregate capacity refusal without live eviction; expired-state reclamation.
- Callback failure, cancellation and injected SQLite insertion failure without consuming the predecessor.
- Migrations from schemas one, two and three preserving settings, profile/grant, active selection, local-profile cursor and existing journal/policy state where present. Conflicting provider tables roll back intermediate DDL; unexpected triggers fail closed.

Focused runs passed with `cargo test -p craft-runtime --test provider_pages --locked --offline` and the combined `cargo test -p craft-runtime --test provider_pages -p craft-core --test pagination --locked --offline`. The existing profile/policy targets passed as well. The initial test-first run failed because the engine method did not exist; the first compiled fixture needed an explicit JSON-encoding error conversion. These were implementation-stage failures, not accepted results.

The first full gate, retained in `target/provider-pagination-gate.log`, found an outdated legacy journal migration fixture. It simulated schema one while retaining the newly introduced provider table, yielding schema refusal instead of its expected deadline. The fixture now removes every later table and expects schema four. Its focused deadline/rollback test passed after correction. Review also aligned the engine's selected output limit to the shared one-through-100 contract.

The final `python tools/verify.py` passed in `target/provider-pagination-final-gate.log`: **325 default passes**, **seven composed-target passes**, and **116 feature-runtime passes**, zero failures. This is **334 distinct passing Rust tests**, or **448 passing executions**, including 114 repeated runtime tests. Five distinct opt-ins were ignored (eight occurrences). Both Clippy configurations, formatting, contract snapshot and negative controls, provider/source inventory, dependency/layer guards and synthetic-feature isolation passed. Cargo emitted the existing incremental hard-link fallback warnings; compilation and Clippy completed successfully.

The separate Windows native test `auth::service::tests::native_tests::windows_service_cache_restarts_serializes_reacquisition_and_preserves_logout` passed with `cargo test -p craft-runtime --lib <test-name> --locked --offline -- --ignored --exact`, recorded in `target/provider-pagination-native-cache.log`. It used its own synthetic native-vault entries and owned TLS servers, checking restart, reacquisition contention and logout after the schema change. Other native-vault, console, vendor-schema and operating-system opt-ins were not rerun.

`cargo build --workspace --locked --offline --no-default-features` passed after fixture testing, recorded in `target/provider-pagination-default-build.log`. Default binary SHA-256: `460922b2b8efcc80dc53c0cbf1ef133c14b5e92875c11089b37b5835d42e4f2b`. Public schema SHA-256: `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Source/document/binary hashes and subsequent checkpoint checks are recorded in `target/provider-pagination-checkpoint-hashes.json` and `target/provider-pagination-checkpoint-validation.json`.

## Limits and next action

The source port is injected trusted code. These tests do not qualify its eventual sealed auth composition, token renewal, provider permission visibility, routing or a real provider's continuation protocol. Payload checksums are not same-principal tamper protection. A simulated SQL insertion failure is not process-kill or power-loss proof. The generic engine has no public dispatch, optional count, overflow artifact export or provider snapshot mode. Jira short/empty-page offset semantics and site/gateway next-link handling remain for the dependent typed adapter. The inspected official pagination guide supports empty pages and changing totals but does not settle that advancement rule.

No independent reviewer was invoked or claimed. Consequential auth/write activation still requires the existing independent review and live/public qualification. The broad cursor-store and bounded-listing tasks remain unchecked, linked to this partial evidence; the overall CLI delivery goal remains active. Next, qualify and compose Jira project discovery against the actual host and envelope contracts without weakening authenticated routing.
