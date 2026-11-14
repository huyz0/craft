# Issue metadata candidate acceptance

Date: **10 November 2026**, Australia/Sydney. Internal candidates only. No public cloud command, live tenant change or real credential provisioning was enabled.

## Scope and focused results

The [candidate contract](../issue-metadata-command.md) connects strict edit/transition arguments, existing profile/site selection, explicit issue-read operation allowances, complete dispatch and the [qualified metadata host](jira-issue-context.md). The existing issue policy now accepts two additional opt-in operation strings. SQLite schema seven, settings format one, policy format one and previous get/search encoding are retained. Manifests, package graph, lock and dependency inventory are unchanged.

Five candidate tests passed in `target/issue-metadata-candidate-tests.log`. They cover flag/JSON parity and defaults, strict unavailable inputs, pure input/projection/recovery refusal without state creation, exact escaped-evidence budget boundaries, explicit operation allowances, captured revision changes, exact issue binding and isolated-child repository/explicit/active profile/site precedence.

Nine issue-policy tests passed in `target/issue-metadata-policy-tests.log`, including three additions: enabled actor transfer requires disablement, metadata allowances are independent while legacy record bytes/schema/settings remain stable, and malformed new operation sets/disabled profile-target transfer fail closed. Existing apply/preflight/CAS, concurrent revisions, corruption and migration-preservation cases also pass. No new migration was added.

Six dispatch tests passed in `target/issue-metadata-dispatch-tests.log`. The new case exercises both metadata operations through argument decoding and typed execution, preserving filters, request identity, issue observation, error/warning/cache evidence and oversized-item continuation. Existing create/project/issue/field cases remain qualified.

Seven composed TLS tests passed with native Windows access in `target/issue-metadata-command-composed.log`. The new candidate test covers eighteen scenarios across both operations: success, revocation before/during auth, after the initial issue read, after metadata, after the bracket, enabled revision changes, operation removal and buffered output after renewal. Exact issuer/resource request counts establish where execution stopped. Every denial retains confirmed renewal receipts where applicable and suppresses metadata/observation/cursors. All output matches the generated schema and byte bound. Existing host/reopen/replay/state-drift/oversize/interruption tests also pass.

Feature-enabled workspace all-target Clippy passed in `target/issue-metadata-command-clippy.log`. The all-target feature check passed in `target/issue-metadata-command-check.log`.

## Failures and corrections

The actor-transfer regression first failed against the unguarded setter in `target/issue-metadata-policy-transfer-red.log`; the same regression passed after adding the enabled-record guard.

The first concurrent candidate run reported three passes and a no-state assertion failure (`target/issue-metadata-candidate-tests-initial.log`). The isolated case and a subsequent full focused run passed; the failure's original cause was not conclusively established. The test fixture had used process ID and wall timestamp alone for root names. It now includes an atomic process-local sequence to guarantee different roots for concurrent tests even if timestamps coincide, and the assertion includes the failing mode/error. The observed isolated rerun is retained in `target/issue-metadata-candidate-failure-detail.log`. No production validation was weakened.

Review found that metadata recovery preflight reserved the shorter `exact_id` resolution label and unescaped update text. The opaque native update evidence permits backslashes, requiring additional JSON bytes. A boundary regression failed in `target/issue-metadata-preflight-red.log`. Preflight now reserves the longest resolution and fully escaped maximum update evidence; the same test rejects one byte below the complete recovery envelope and accepts its exact bound. The first full gate completed successfully before this correction and remains in `target/issue-metadata-command-gate.log`; a new full gate qualifies the corrected source separately.

## Workspace checkpoint and limits

The corrected full `python tools/verify.py` gate passed in `target/issue-metadata-command-gate-corrected.log`: **446 default passes**, **50 composed-host passes** and **123 feature-runtime passes**. This is **498 distinct passing Rust tests**, zero failures, and **619 reported passing executions**, including 121 repeated runtime cases. Five distinct opt-ins remained ignored (eight occurrences). `target/issue-metadata-command-test-counts.json` records normalized identities and independently matched result-line totals. Formatting, both all-target Clippy modes, public contract/provider/dependency/layer checks, planning and synthetic isolation passed. Existing incremental hard-link fallback warnings did not prevent the gate. The earlier gate before the budget correction is preserved with its own counts in `target/issue-metadata-command-test-counts-initial.json`.

`target/issue-metadata-command-final-build.log` records a successful `cargo build --workspace --locked --offline --no-default-features`. Default binary SHA-256: `010bed6274aaad58cfb6c24596bbb605432342fd1f817f64d0faf020c0da7d20`. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Actual discovery in `target/issue-metadata-command-discovery.json` retains twenty local operations, no cloud provider, and refused metadata/transition schema and parser lookup.

Source/document hashes and final checks are retained in `target/issue-metadata-command-checkpoint-hashes.json` and `target/issue-metadata-command-checkpoint-validation.json`. The broader tasks remain unchecked; the ledger remains 16 of 87 complete.

The per-exchange host wrapper rechecks existing sealed access and captured policy before invoking its original transport context. It does not introduce an authenticated bypass. This work remains internal pending independent consequential review and public/live qualification. No native vault opt-in was rerun because credential storage and persistence schema are unchanged. Edit/transition mutation authority and reconciliation remain separate work. The broader Jira-context/transition tasks and the full CLI delivery goal remain unfinished.
