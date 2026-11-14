# Internal project details and status context acceptance

Date: **10 November 2026**, Australia/Sydney. Internal library qualification. Public parser/catalog/provider activation and live tenant operations remain unavailable.

## Changed behavior

The [contract](../jira-project-context.md) describes the provider, projection/normalization and verified host under `crates/craft-provider-jira/src/project_context*` and `crates/craft-cli/src/project_context.rs`. The host binds an exact project selector/grant/actor, composes existing OAuth access, and uses immutable remainder paging for an unpaginated status array. It does not widen project-discovery or issue-read configuration, change SQLite, add authority administration or follow native links.

The CLI manifest and gate add the explicit `jira_project_context_host` synthetic integration target. Dependency inventory comparison found only the CLI manifest digest changed; package graph, versions, features and lock digest were unchanged before refresh. The public schema remains unchanged.

## Focused observed checks

Nine provider tests passed using `cargo test -p craft-provider-jira --test project_context --locked --offline`. They construct actual registry/schema/projection and compare compiled native routes/current scope alternatives with the pinned specification. They verify exact numeric/key identity, local-only projection without expansions/properties, default suppression of bodies/user data/links, explicit bounded description, missing/null/not-returned values, grouped status output, nested category types and partial missing paths, duplicate refusal, supported empty arrays, wrong target/input/get cursor, plan/context binding before invocation, physical mock-response bounds and sanitized single-attempt HTTP errors. Status IDs can repeat across distinct issue-type groups.

One pure host preparation test passed in `target/project-context-preparation.log`, covering wrong target/grant/actor, unknown fields/input, get cursors and insufficient complete recovery budget for both operations.

Seven actual composed tests passed in `target/project-context-composed.log`. They use owned TLS, sealed access, cache generation logic, native grant locks and SQLite cursors with fixed synthetic credentials:

- Project get sends one declared GET without expansions/properties, preserves exact identity/observation/request ID and fresh/renewed credential receipts, and omits unselected private sentinels. Actual projected outputs satisfy generated response schemas.
- The native grouped array is fetched once; immutable remainder resumes after token renewal without a status fetch or flattening. Original observation/digest survive, and the final page is terminal.
- Selector, field or actor changes reject the original cursor before auth.
- Oversized get releases no partial project. Oversized status groups retain same-position recovery and fit at a larger budget from buffered data. Error and success envelopes satisfy selected schemas.
- Scope/actor/HTTP denial, redirects, disconnect, malformed get identity and duplicate groups retain renewal receipts without successful project/page output. Request counts prove refusal boundaries and no retries.
- Revocation before auth, during auth, after native work and during buffered replay prevents output, including the late observation boundary covered by the prior issue-read fix. Confirmed renewal receipts remain visible.
- Cancellation and deadline interrupt stalled resource I/O without retries or page publication.

The first provider and composed runs passed. Initial compile checking found an unused import, removed before Clippy. Focused feature-enabled all-target Clippy passed in `target/project-context-clippy.log` without suppressions.

## Full gate and executable checkpoint

`python tools/verify.py` passed in `target/project-context-gate.log`: **378 default passes**, **33 composed passes** (seven create, ten project-list, nine issue-read and seven project-context), and **117 feature-runtime passes**. This is **413 distinct passing Rust tests**, zero failures, or **528 reported passing executions**, including 115 repeated runtime cases. Five distinct opt-ins remain ignored (eight occurrences). Both Clippy modes, formatting, public contract snapshot/negative controls, provider/source inventory, dependency/layer guards, planning and synthetic-feature isolation passed. Existing incremental hard-link fallback warnings did not prevent compilation or Clippy. Counts are independently extracted from result lines and normalized executable/test identities in `target/project-context-test-counts.json`.

`cargo build --workspace --locked --offline --no-default-features` passed after feature testing, recorded in `target/project-context-default-build.log`. The default development binary SHA-256 is `16d089b7aa427d9ad4f0140c90a31e5f34dee25fcfbf2ad874e6df6c0b147385`. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Final actual public discovery, source/doc hashes and unchanged lock/other-manifest checks are recorded in `target/project-context-checkpoint-validation.json` and `target/project-context-checkpoint-hashes.json`. Native opt-ins were not rerun; native vault and SQLite implementations did not change.

## Limits

No live tenant, real credential, public native input/provisioning, independent reviewer, other-platform run, compliance assurance or deployment was exercised. These tests do not establish currently available transitions for an issue. Key-bound status pages do not establish immutable numeric project identity or fresh item-specific visibility. Public candidate/configuration integration and the remaining Jira field/workflow/context/CRUD and Confluence work remain unfinished; no broad task closes from this addition.

Prior [issue-read candidate evidence](issue-read-command.md) and [project-list evidence](project-list-command.md) retain their recorded scopes.
