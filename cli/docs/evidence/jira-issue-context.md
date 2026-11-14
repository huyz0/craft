# Issue-bound metadata acceptance

Date: **10 November 2026**, Australia/Sydney. Internal read composition only. No public cloud command, live tenant change or real credential provisioning was enabled.

## Scope

The [contract](../jira-issue-context.md) covers edit metadata and transition-list reads, bracketed issue observations, inert selected metadata, private freshness proof, and optional immutable page context. Source modules are `crates/craft-provider-jira/src/issue_context/`, `crates/craft-cli/src/issue_context.rs`, core pagination and runtime pagination/state. Existing native-page literals explicitly use absent context. SQLite schema seven and settings format one remain unchanged.

Earlier Jira-context evidence remains available through [field candidates and permissions](field-read-command.md) and its linked read/metadata checkpoints. The task ledger supports one evidence path per entry, so it points here while the earlier records remain linked.

The CLI manifest adds one synthetic-only composed test target, `jira_issue_context_host`, and the gate runs it explicitly. The isolation checker now checks all six composed targets, including the existing field-read target previously absent from its explicit list. Inventory comparison found only the CLI manifest digest changed: package graph, versions, sources, checksums and lock were unchanged. The dependency inventory was regenerated after inspecting that comparison. No production dependency was added.

## Focused results

- Ten provider tests passed in `target/issue-context-provider-tests.log`: pinned API routes/query declarations, no admin bypasses, three-call numeric routing, alias resolution, issue-state drift, mandatory native fields, inert future schemas, null/missing selection, actual transition IDs/filter/flags/expansion, duplicate refusal, freshness boundaries/backward clocks, principal/operation transfer, warnings/denials/bounds, pure refusals, and schema-valid output for every selectable field.
- Twenty-five pagination tests passed in `target/issue-context-paging-tests.log`, including five new context tests. They verify legacy encoding/digest, context through reopen/replay/empty pages, larger-budget recovery, current access, exact 32-KiB bounds, whole-page bounds and recomputed-checksum corruption.
- Four pure CLI preparation tests passed in `target/issue-context-preparation-tests.log`: exact policy/principal binding, strict inputs and unavailable bypasses, complete recovery/context budget reservation and unsupported projection refusal.
- Six composed TLS tests passed with native Windows access in `target/issue-context-composed-tests.log`. They exercise both operations, three resource reads on numeric routing, empty output with issue observation, reopen/replay without metadata refetch, drift/denials/cancellation, late policy checks, renewal receipts, complete schema/budget/request identity, oversized-record recovery, stalled/disconnected transport and exact connection/request counts. The first five-scenario pass remains in `target/issue-context-composed-tests-first-pass.log`.
- Feature-enabled all-target Clippy passed after correcting one unnecessary clone; `target/issue-context-clippy.log` records the result. The earlier lint failure remains in `target/issue-context-clippy-initial.log`.

## Failures and corrections

The initial port's parse failure and corrected compile are retained in `target/issue-context-port-check.log` and `target/issue-context-port-check-corrected.log`. A mechanical literal edit had also matched function return types; those accidental insertions were removed before behavior testing.

The first pagination behavior run reported 24 passes and one failure: `legacy_cursor_without_context_resumes_and_invalid_rechecksummed_context_is_refused` returned “Expected corrupt context.” Explicit JSON null had decoded as absent optional context. Both normalized-page and persisted-cursor decoding now retain explicit null for object validation. The unchanged regression and all 25 pagination tests passed after correction. This first run was observed directly in tool output rather than captured in a standalone log.

The initial provider test compile failed because the test clock used `Cell` while the actual `Clock` port requires `Send + Sync`. Atomic clock fields corrected the fixture; the failure is retained in `target/issue-context-provider-tests-initial.log`.

Restricted Windows execution could not qualify the composed TLS path: token response reads failed before resource execution. The failed run remains in `target/issue-context-composed-tests-restricted.log`. Native Windows access passed the five original scenarios. An expanded interruption test then expected EOF specifically for an abrupt TLS close but observed deadline expiry. Its failure remains in `target/issue-context-composed-tests-interruption-initial.log`. The corrected test accepts either transport EOF failure or deadline for abrupt close, requires deadline expiry for an actual stall, and requires exact resource connection/request counts with no retries and no partial observation.

A subsequent run exposed a fixture timing issue: the one-second invocation expired after the first issue read, before the intended metadata request (`target/issue-context-composed-tests-interruption-count.log`). The interruption fixture now uses a deterministic invocation clock that becomes overdue only after the server receives the metadata request. Service-token/freshness time remains independent. This preserves the exact four-request assertion and tests the intended transport boundary without depending on setup speed.

## Workspace gate and limits

`python tools/verify.py` passed in `target/issue-context-gate.log`: **437 default passes**, **49 composed-host passes** and **123 feature-runtime passes**. This is **488 distinct passing Rust tests**, zero failures, and **609 reported passing executions**, including 121 repeated runtime cases. Five distinct opt-ins remained ignored (eight occurrences). `target/issue-context-test-counts.json` records normalized package/suite/test identities and independently matched result-line totals. Formatting, both all-target Clippy modes, public contracts and negative controls, provider/dependency/layer checks, planning and explicit synthetic isolation passed. Existing incremental hard-link fallback warnings did not prevent the gate.

`target/issue-context-final-build.log` records a successful `cargo build --workspace --locked --offline --no-default-features`. Default binary SHA-256: `e0f4fb813aadd77fcaed17a77c3bee1bc87337c8f3595ad07c7def23b9ded6b7`. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Actual discovery in `target/issue-context-discovery.json` shows twenty local operations, no cloud provider, and refused edit-metadata/transition schema and cloud-parser lookup. No new cloud authority is advertised.

`target/issue-context-checkpoint-hashes.json` and `target/issue-context-checkpoint-validation.json` retain the source/document identities and final checks. The planning checker initially rejected two comma-separated evidence paths; the ledger now uses its supported single path, linking prior context evidence from this record. The broader tasks remain unchecked and the ledger remains 16 of 87 complete.

No native vault opt-in was rerun for this read/payload addition; the vault implementation and SQLite schema are unchanged. These fixtures use owned loopback TLS, private SQLite state, memory credentials, actual generation/cache logic and native locks. They do not prove live auth, real Jira permission/configuration behavior, other platforms, independent review, public activation or edit/transition writes. The broader Jira-context and transition tasks remain unchecked.
