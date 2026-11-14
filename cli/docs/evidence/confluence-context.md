# Confluence context provider and host evidence

Date: **10 November 2026**, Australia/Sydney. Baseline: [typed Jira recovery qualification](issue-recovery-command.md). This implements Confluence reads needed by the [delivery path](../delivery-path.md). No live tenant, real credential provisioning, public cloud registration, publication or deployment was changed.

## Implementation

The native-resource scenario and design were settled before implementation. The new `craft-provider-confluence` workspace member implements five typed commands and six GET routes: v2 space get/list, page get/list including space-scoped listing, and v1 CQL page search. Selection-derived representations/expansions, explicit body availability, exact identity/key handling and qualified continuation are described in the [contract](../confluence-context.md).

`crates/craft-cli/src/confluence_reads.rs` composes the provider with existing sealed service access, native grant locking, bounded HTTPS and the runtime's persisted buffered pagination. It follows the existing issue-read host's proven control sequence, with Confluence schemas and effective defaults. Dynamic policy checks also wrap native invocation and link normalization. Get output rechecks current access after timestamp/envelope work. The host does not register public commands or derive authority from provider content.

`confluence_context_host` is an explicit synthetic-acceptance target. The normal build has no fixture selector; the isolation checker now includes this target. The full gate includes its tests. No existing runtime persistence schema or native credential format changed.

## Focused checks and retained failures

- `target/confluence-context-red.log`: missing implementation modules before the provider was added.
- `target/confluence-context-first.log`: initial six provider tests pass. `target/confluence-context-clamp-red.log` retains the failing CQL limit-clamp case. Validation now uses the provider-reported bounded size for its continuation link while preserving the original size for future requests. Eight provider tests pass in `target/confluence-context-provider-qualified.log`, including independent route/query/scope comparison with the pinned inventory.
- `target/confluence-context-host-first.log`: the new host compiles. Two pure host tests pass in `target/confluence-context-host-unit.log`, covering all command authority/input/field/budget refusals and equivalent default/selection cursor bindings.
- `target/confluence-context-host-compile.log`: the restricted runner failed synthetic token exchange; this is not a live auth result. The same owned TLS composition passes with native loopback access in `target/confluence-context-host-native-first.log`.
- `target/confluence-context-host-qualified.log`: four owned TLS tests pass, covering eighteen scenarios. Six exact native route cases include selected body content and credential renewal. Two v2/CQL traversal cases reopen buffered remainder, reject changed projection before auth and resume the bound cursor. Six refusal cases cover early/late policy, insufficient scope, foreign identity, rate response and oversized get output while retaining renewal receipts. Four cases cover foreign/changed-filter continuation, disconnect and bounded stalled I/O. All resource traffic is GET; selected output and private state exclude sentinel credentials and unrelated user metadata.
- Initial Clippy logs retain a collapsible condition, test-module placement and unnecessary result wrapper. Those were corrected without lint suppression. `target/confluence-context-clippy-final-pass.log` records successful all-target feature-enabled Clippy for both changed crates.

## Dependencies and availability

The new provider uses existing pinned core/serde/JSON/schema dependencies. Comparing the lock to the previous inventory finds one new workspace member and no added, removed or changed external package/version/source tuple. The inventory now has 284 packages: 276 external and eight workspace members. CLI has a new provider composition edge and a ninth explicit synthetic target. Public descriptors and parser activation are unchanged.

The broad Confluence-context task remains unchecked pending typed commands/profile integration, remaining link-form qualification and actual public/live access. Page creation/update, shared plan publication, review and ticket materialization remain unfinished. No independent consequential review or other-platform qualification was performed. The full CLI goal remains active.

## Final qualification

The single full `python tools/verify.py` run passes in `target/confluence-context-gate.log`: 549 default, 76 composed-host and 123 feature-runtime passes. Normalized identities in `target/confluence-context-test-counts.json` establish **627 distinct passing tests**, zero failures and 748 passing executions, including 121 repeats. Five distinct opt-ins remained ignored (eight occurrences). Both Clippy modes, formatting, public contracts, pinned provider checks, dependency/layer checks, planning and synthetic isolation passed. All 249 captured source/toolchain/tool digests remained unchanged through qualification. No unchanged full gate was repeated.

The default offline build passes in `target/confluence-context-default-build.log`. Binary SHA-256 is `de5069bef0f48f3eb549845d27c1b5ed145d903ffbd2d726aac3520cc16a34ac`. Actual execution in `target/confluence-context-discovery.json` confirms twenty local operations, no cloud providers and unavailable Confluence parser/schema lookups, alongside the previously unavailable mutation/recovery lookups. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`.

Selected source/document/binary hashes and local documentation checks are retained in `target/confluence-context-checkpoint-hashes.json` and `target/confluence-context-checkpoint-validation.json`. SQLite schema nine, journal/settings and existing policy/credential formats are unchanged. Native-vault/cache opt-ins were not rerun because their implementation and schema are unchanged; the earlier schema-nine Windows cache qualification remains in the baseline. No live or other-platform support is inferred. Ledger status remains sixteen of eighty-seven complete, and the full CLI delivery goal remains active.
