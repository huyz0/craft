# Current-user permission read acceptance

Date: **10 November 2026**, Australia/Sydney. Internal GET view and candidates only. No public cloud command, live tenant change or real credential provisioning was enabled. Earlier context evidence remains linked from [metadata candidates](issue-metadata-command.md).

## Scope and contract evidence

The [contract](../jira-permission-reads.md) describes `crates/craft-provider-jira/src/permissions/`, `crates/craft-cli/src/permission_reads*`, candidate dispatch and the appended issue-read allowance. Official permission documentation was reopened and compared with the pinned Jira source. Context-dependent project permissions, comment restrictions, missing flags and exemption from app access rules are retained rather than interpreted as general write approval.

The CLI manifest adds one synthetic-only `jira_permissions_host` target. The gate and isolation checker include all seven composed targets. Inventory comparison found only the CLI manifest digest changed; no packages, locked graph, versions, sources or checksums changed. Inventory was regenerated after inspecting that comparison. SQLite schema seven and settings/policy format one remain unchanged. No production dependency was added.

## Focused checks

Eight provider tests passed in `target/permission-provider-tests.log`. They verify pinned GET route/parameters/current scope, omitted impersonation/bulk controls, six selector forms across four contexts, ordered missing/false/unknown results, strict key/context limits, warnings/denials/bounds, principal/operation/dual-clock checks including time spent reading, and schema-valid selected/missing output for every selectable field. Returned URLs/instructions remain inert or omitted.

Six CLI tests passed in `target/permission-cli-tests.log`: typed flag/JSON parity, pure refusals without state, new operation allowance/captured revisions, isolated-child profile/site precedence, pure authority/projection/recovery preflight and cached record type/order/unknown-state checks. Ten issue-policy tests passed in `target/permission-policy-tests.log`, including the new fifth-operation independence/canonical order/reopen case. Existing migration, apply/CAS, transfer, corruption and concurrency cases remain qualified.

Seven dispatch tests passed in `target/permission-dispatch-tests.log`. The new permission case preserves requested context, unknown decisions, request identity, cache/error/warning data and recovery continuation through decoding and execution. All prior candidate paths remain tested.

Feature-enabled all-target checks and Clippy passed in `target/permission-read-port-check.log`, `target/permission-candidate-check.log`, `target/permission-host-check.log` and `target/permission-clippy.log`. Final composed and workspace-gate results are recorded after inspection below.

Six composed TLS tests passed with native Windows access in `target/permission-composed-tests.log`. They exercise six selector forms, one declared GET, request headers/encoding, renewal/reopen/replay with false and unknown decisions, cursor context/key-order/projection mismatch before auth, oversized escaped metadata recovery, HTTP/warning/duplicate-JSON/disconnect/deadline failures, and an eight-scenario parsed candidate matrix covering success, missing scope, early/late revocation, enabled revision change, operation removal and cached output. Exact resource/issuer request counts, complete output schema/byte bounds, mandatory non-authorizing flags and independent renewal receipts are checked.

## Failures and corrections

The initial provider run reported seven passes and one invalid-input fixture failure (`target/permission-provider-tests-initial.log`). The test had assumed the shared numeric-string validator rejected leading zeroes. The documented design preserves numeric strings, so the invalid fixture now uses a path separator. No production validator was weakened; all eight tests then passed.

The first composed run reported five passes and one expected-error mismatch in the candidate matrix (`target/permission-composed-tests-initial.log`). The fixture expected `POLICY_DENIED` for missing token scope; the actual access contract correctly returns `PERMISSION_DENIED`. The expectation now distinguishes scope refusal from local policy refusal.

Review found that the candidate's serde empty-list default made its schema accept an omitted permission list even though execution rejected it. The new schema-validation assertion failed in `target/permission-required-keys-red.log`. Removing that default makes the list required in candidate JSON and aligns validation with execution; flag placeholders are still checked before state/auth. The full run already in progress is retained separately from the final corrected-source gate.

## Workspace checkpoint and limits

The corrected full `python tools/verify.py` gate passed in `target/permission-read-gate-corrected.log`: **462 default passes**, **56 composed-host passes** and **123 feature-runtime passes**. This is **520 distinct passing Rust tests**, zero failures, and **641 reported passing executions**, including 121 repeated runtime cases. Five distinct opt-ins remained ignored (eight occurrences). `target/permission-read-test-counts.json` records normalized identities and independently matched result-line totals. Formatting, both all-target Clippy modes, public contract/provider/dependency/layer checks, planning and synthetic isolation passed. Existing incremental hard-link fallback warnings did not prevent the gate. The first full run is retained in `target/permission-read-gate.log`, with its count record in `target/permission-read-test-counts-initial.json`; it did not execute the later required-list assertion in its earlier default test phase.

`target/permission-read-final-build.log` records a successful `cargo build --workspace --locked --offline --no-default-features`. Default binary SHA-256: `80de074753103ebdbb868137ee4233ad87c29848a6a43a646b9976cd261f9230`. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Actual discovery in `target/permission-read-discovery.json` retains twenty local operations, no cloud provider, and refused permission schema/parser and metadata-parser lookup.

Source/document hashes and final checks are retained in `target/permission-read-checkpoint-hashes.json` and `target/permission-read-checkpoint-validation.json`. The broader tasks remain unchecked; the ledger remains 16 of 87 complete.

Fixtures use owned loopback TLS, fixed synthetic credentials, private SQLite state, actual cache-generation algorithms and native locks. They do not establish live permission behavior, other platforms or independent review. Native vault opt-ins were not rerun because vault implementation and persistence schema are unchanged. Cached observations cannot grant mutation authority, and the GET view does not implement bulk/other-user variants. Public activation, edit/transition write validation/recovery, remaining Jira/Confluence resources and the full delivery goal remain unfinished.
