# Internal issue get and enhanced search acceptance

Date: **10 November 2026**, Australia/Sydney. Internal library qualification. Public parser/catalog/provider activation and live tenant access remain unavailable.

## Changed behavior

The [contract](../jira-issue-reads.md) describes the new typed provider module, field plan and verified host. Source is under `crates/craft-provider-jira/src/issue_reads/` and `crates/craft-cli/src/issue_reads.rs`. The host uses the existing OAuth access and immutable cursor libraries; it does not widen private project-read policy records or change SQLite. The public schema snapshot remains unchanged.

Earlier [project-list candidate/configuration evidence](project-list-command.md) remains applicable within its recorded scope. This addition does not supersede its native migration tests or complete the broader Jira-context requirement.

The CLI manifest adds the explicit feature-gated `jira_issue_reads_host` integration target. The development gate and fixture-isolation checker include it. Dependency inventory comparison found only the CLI manifest digest changed: package graph, versions, features and Cargo lock digest were unchanged before the inventory refresh.

## Focused observed checks

Fourteen provider tests passed using `cargo test -p craft-provider-jira --test issue_reads --locked --offline`. They construct the actual registry and schema/projection, compare compiled routes/current scope alternatives and search fields with the pinned specification, verify default masks and normalization, refuse mismatched field plans before invocation, preserve missing/null values, ignore malformed unselected members, qualify bounded Unicode/ADF/plaintext descriptions, check exact ID/key aliases and reject unsupported local input. Search cases cover unchanged reconciliation/JQL, opaque URL-like tokens, empty/short/terminal pages, warning refusal, contradictory metadata, malformed positions, duplicate IDs, oversized pages and sanitized single-attempt errors. Mock responses also obey the real transport's JSON byte/depth/container bounds.

The initial twelve-test provider run had seven passes and five failures. Those failures established the missing command/field-plan binding and unselected/nested-field normalization regressions. Corrections preceded the passing run; two source/bounds tests were subsequently added and passed. Compile-only evidence from before these tests is not treated as provider qualification.

Two pure host tests passed with `cargo test -p craft-cli --lib issue_reads --locked --offline`. They cover wrong policy/site/grant/actor, unknown fields/input, get cursors, insufficient complete recovery budgets, effective input-default binding and equivalent field ordering.

Eight composed tests passed with `cargo test -p craft-cli --test jira_issue_reads_host --features synthetic-acceptance --locked --offline` in the authorized native environment. They exercise real TLS, declared HTTP contexts, sealed access, service/cache generation logic, native grant locks and SQLite cursor state with memory-only synthetic credentials:

- Single live get, actual native field mask/history flags, key alias resolution, request identity and fresh/renewed cache receipts.
- POST search, exact JQL/IDs/mask/native size, buffered remainder, same-grant token renewal within cursor lifetime, retained original observation and opaque token continuation without URL following.
- Changed JQL, reconciliation IDs, fields or native size rejected before auth using the original cursor.
- Provider warnings, permission denial, disconnect, redirects and malformed data release no page/cursor while retaining renewal receipts.
- Scope/actor refusal before resource HTTP and dynamic policy revocation before auth or after native get/search prevent unqualified output.
- Oversized get releases no partial body; oversized search retains same-position recovery and succeeds from buffered data at a larger budget.
- Repeated native token fails with no successful successor.
- Invocation deadline interrupts cached identity I/O; cancellation interrupts search I/O, without a retry.

The first composed attempt failed to compile because the test used a nonexistent error variant. After correcting that assertion, six of eight cases passed; the two failures were fixture mistakes: advancing beyond the cursor TTL and scanning an actively locked grant file. The corrected fixture uses a short-lived token for renewal within the cursor TTL and releases credential handles before scanning. All eight then passed. Focused Clippy initially found test-module ordering and a needless struct update; these were corrected without lint suppressions.

## Full gate and executable checkpoint

`python tools/verify.py` passed in `target/jira-issue-reads-gate.log`: **357 default passes**, **25 composed passes** (seven create, ten project-list and eight issue-read) and **117 feature-runtime passes**. This is **384 distinct passing Rust tests**, zero failures, or **499 reported passing executions**, including 115 repeated runtime cases. Five distinct opt-ins remain ignored (eight occurrences). Both Clippy modes, formatting, public contract snapshot/negative controls, provider/source inventory, dependency/layer guards, planning checks and synthetic-feature isolation passed. Existing incremental hard-link fallback warnings did not prevent compilation or Clippy. Test counts are independently extracted from test result lines and normalized executable/test identities in `target/jira-issue-reads-test-counts.json`.

`cargo build --workspace --locked --offline --no-default-features` passed after feature testing, recorded in `target/jira-issue-reads-default-build.log`. The default development binary SHA-256 is `9889f7b5d585308c91ab7428bb28d54955a619cc8f0d102e218c709ea8106399`. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Public discovery and source/doc hash checks are recorded separately in `target/jira-issue-reads-checkpoint-validation.json` and `target/jira-issue-reads-checkpoint-hashes.json` after the default rebuild. Native opt-ins were not rerun for this addition; no native vault or SQLite implementation changed.

## Limits

No actual tenant, real credential, independent reviewer, public command, remote write or provider activation was exercised. The fixtures are debug-only, opt-in and isolated from normal dependency/credential paths. No native vault or other-platform qualification is claimed by this slice. No broad task closes: remaining Jira metadata/context/CRUD, candidate arguments/configuration, recovery, Confluence workflows, consequential auth review and live acceptance remain required.
