# Confluence page-command evidence

Date: 10 November 2026. Internal integration only. No live tenant, real credential provisioning, release, deployment or independent consequential review occurred.

## Observed implementation

- `crates/craft-cli/src/confluence_pages/command.rs` and `command/tests.rs`: typed create/update flags and JSON, strict complex argument parsers, normal profile/site/space resolution, full host response schemas, pre-state validation and captured write policy. Three focused tests pass, covering schema/flag equivalence, complex duplicate fields, invalid/bypass/no-apply/output refusal without state creation, read/write separation, defaults, foreign space/site refusal and revocation.
- `crates/craft-cli/src/args.rs`, `dispatch.rs` and `dispatch/tests.rs`: nested internal actions, structured decode and complete request/receipt preservation. The added dispatch test passes. Profile selection recognizes Confluence commands. Root parser/catalog activation remains unavailable.
- `crates/craft-provider-confluence/src/models.rs`, `selection.rs`, and `tests/context.rs`: page-get revision selection computes the exact write precondition without returning an unselected body. The focused provider tests pass, including exact digest comparison, derived storage query, missing-body refusal and list/search refusal.
- `crates/craft-runtime/src/confluence_write_policy.rs`, `profiles.rs` and tests: schema eleven adds an empty separate write table keyed by profile/space. Four policy tests pass for apply/preflight/CAS, explicit operations, exact space/actor/grant/installation, no repair, bad/foreign inputs, preserved reads and migration conflict/trigger refusal. Historical migration fixtures were updated to omit the newer table when constructing older states; buffered migration coverage now includes schema ten.
- `crates/craft-cli/tests/confluence/publication.rs`: the existing five successful native POST/PUT scenarios now use actual typed candidates and saved write records. A new five-scenario captured-policy test passes for revocation before auth, after identity/read, an otherwise allowed revision change, and revocation after a durable claim. No refused case sends a native mutation. Existing loss/rate/refusal/lifecycle/freshness scenarios remain covered. Four publication tests pass in `target/confluence-write-command-host.log`.

## Validation and failures

The initial policy test failed because the write-policy API did not exist; four tests passed after implementation. Historical policy/cursor migrations passed in `target/confluence-write-migrations.log` before adding the schema-ten cursor case. Focused command, revision and dispatch logs are `target/confluence-write-command-focused.log`, `target/confluence-revision-focused.log` and `target/confluence-write-command-dispatch.log`.

The first feature-enabled Clippy check found duplicate read-command patterns introduced while adding write context selection. The duplicate patterns were removed; workspace all-target feature-enabled Clippy passes in `target/confluence-write-command-clippy-final.log` without a lint suppression. Existing incremental hard-link fallback warnings remain environmental output.

The Windows native service-cache restart/contention/logout opt-in passed on schema eleven in `target/confluence-write-command-native-cache.log`, exercising owned synthetic credentials, process isolation and cleanup. It does not qualify live service access or other platforms.

The full `python tools/verify.py` gate passed in `target/confluence-write-command-gate.log`: 660 distinct passing tests, zero failures and 781 passing executions (577 default, 81 composed-host, 123 feature-runtime; 121 repeated executions). Five distinct opt-ins remained ignored, eight occurrences. Both Clippy modes, formatting, contract/provider/dependency/layer/planning checks and synthetic isolation passed. The schema-ten buffered migration case is included in the final gate. Counts are retained in `target/confluence-write-command-test-counts.json`. All 267 captured source/toolchain/tool hashes remained unchanged through qualification.

The default offline workspace build passed in `target/confluence-write-command-default-build.log`. Binary SHA-256: `051fc5dff6e247f73eb0e473e5b91743a3ebf3e10591935b109d19dc9cf12744`. Actual discovery in `target/confluence-write-command-discovery.json` still has twenty local commands, no cloud provider and unavailable page-create/update parser/schema lookups. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Manifests, lock and dependency inventory are unchanged. Final validation and hashes are retained in `target/confluence-write-command-final-validation.json` and `target/confluence-write-command-tested-source-hashes.json`. Credential formats and earlier policy payloads remain unchanged; private SQLite schema is eleven and new write records use format one.

The pinned v2 page-get contract includes the current version by default (`include-version=true`), so revision-only retrieval does not depend on an undocumented expansion. This remains pinned contract and fixture evidence, not actual Cloud qualification.

## Remaining delivery boundary

Public auth and policy administration, parser/catalog activation and actual test-site qualification remain unfinished. Page publication from the rendered plan, exact-revision human approval and Jira ticket materialization still need orchestration. Page trash and editor-draft interaction remain unqualified. The full CLI scope remains in the task ledger; broad tasks stay unchecked. No public cloud provider is enabled.
