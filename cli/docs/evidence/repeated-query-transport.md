# Repeated GET query transport acceptance

Date: **10 November 2026**, Australia/Sydney. Internal transport qualification. No public command/provider or live tenant access was enabled.

## Behavior and scope

The [transport contract](../transport.md) now carries declared, bounded repeated GET parameters. This is a dependency for native Jira field-search filters, not an implementation of field discovery. The [design](../../openspec/changes/build-cloud-cli/design.md) and [operation recovery scenarios](../../openspec/changes/build-cloud-cli/specs/operation-recovery/spec.md) record the intended semantics.

`craft-core/src/execution.rs` validates declarations, vector representation, UTF-8 value bounds, name/pair counts and collisions. Runtime `transport/routing.rs` independently encodes each item, preserves exact bound vectors across continuation, bounds parsed links and reconstructed URLs, and leaves the special Jira project-site parser scalar-only. Existing route/call literals supply empty values. Empty route declarations are omitted from serialized descriptors; existing scalar operations keep their descriptor encoding. Mutation preparation and journal claims still reject GET; mutation fingerprints and persistence formats are unchanged. Earlier scalar/TLS foundation evidence remains [linked](trusted-http.md).

## Focused observed checks

`cargo test -p craft-core --test provider_execution --locked --offline` passed **11 tests**, including two new cases for legacy descriptor round trips, declaration refusal, GET-only support, representation, names, pairs, exact 16-KiB UTF-8 byte bounds and uncoerced values. POST, PUT, PATCH and DELETE repeated declarations are refused.

`cargo test -p craft-runtime --lib transport::tests --locked --offline` passed **19 tests**, including five new cases in `transport/tests/repeated_queries.rs`. These use a registered array provider, sealed prepared operation, checked handler execution and owned TLS fixture. A three-request round trip preserves repeated IDs/types/project IDs, numeric strings, Unicode, commas, plus/ampersand data, duplicate/empty items and bearer destination across omitted and exact next-link filters. Refusals cover partial/reordered/changed/added arrays, duplicate decoded scalar/cursor names, malformed percent/UTF-8, foreign routes, scalar/array collisions, unknown names, vector/value limits, 256-pair boundary and rebuilt URL overflow. Invalid inputs cause zero connections. The special Jira project-site parser refuses repeated declarations.

Logs: `target/repeated-query-focused.log` retains the passing core checks and an initial test compilation failure caused by an incorrect error-enum variant in the new assertion. That assertion was corrected to the actual `InputInvalid` contract. `target/repeated-query-transport-focused.log` records the subsequent **19/19** native-access transport pass. The first restricted run passed three of the initial four cases but returned sanitized `RemoteUnavailable` for the TLS case; the same four cases passed with authorized native certificate access before the fifth boundary case was added. No production routing relaxation was needed.

## Workspace checkpoint

`python tools/verify.py` passed with authorized native access in `target/repeated-query-gate.log`: **396 default passes**, **34 composed-host passes** and **122 feature-runtime passes**. This is **432 distinct passing Rust tests**, zero failures, and **552 reported passing executions**, including 120 repeated runtime executions. Five distinct opt-ins remained ignored (eight occurrences). `target/repeated-query-test-counts.json` records counts derived independently from result lines and normalized suite/test identities. Both all-target Clippy modes, formatting, public contract/negative controls, dependency/layer/provider checks, planning and synthetic-feature isolation passed. Incremental hard-link fallback warnings did not prevent compilation or Clippy.

`cargo build --workspace --locked --offline --no-default-features` passed after fixture testing, in `target/repeated-query-default-build.log`. Default development binary SHA-256: `21938f93720ecf01190513faa43a39e176662b1c8d96b2b151854147b628fee7`. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Actual binary discovery in `target/repeated-query-discovery.json` still shows twenty local operations and no cloud provider; field schema lookup and Jira parser activation are refused. Manifests, lock, dependency inventory, persistence implementation and public schema remain unchanged from the [previous checkpoint](project-context-command.md). Native opt-ins were not rerun because native vault/database behavior did not change.

Source/document hashes and final validation are recorded in `target/repeated-query-checkpoint-hashes.json` and `target/repeated-query-checkpoint-validation.json`. The broader transport/Jira-context tasks remain unchecked; only this dependency has acceptance evidence.

## Limits

No live provider, actual field endpoint, field adapter/host/policy, other-platform native test, public auth activation or independent review was exercised. Existing consequential auth/write qualification remains required. GET transport preserves duplicates and empty items as data; this does not establish that a particular provider filter accepts them. Broad transport and Jira-context tasks remain incomplete.

## Closure

On 15 November 2026 the owning ledger task was closed. The remaining gaps listed above are answered in [foundation closure](foundation-closure.md), with platform runs in [platform qualification](platform-qualification.md) and review in [foundation review](foundation-review.md).
