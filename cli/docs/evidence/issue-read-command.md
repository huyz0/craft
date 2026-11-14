# Issue-read candidate and configuration acceptance

Date: **10 November 2026**, Australia/Sydney. Internal candidates. No public cloud provider, live tenant operation or real credential provisioning was enabled.

## Changed behavior

The [contract](../issue-read-command.md) connects typed get/search arguments, existing profile/site precedence, independent issue-read configuration and complete dispatch to the [qualified host](jira-issue-reads.md). Source lives in `crates/craft-cli/src/issue_reads/command.rs`, `args.rs`, `dispatch.rs`, runtime `issue_read_policy.rs` and `profiles.rs`. SQLite schema six adds only the private issue-read table; settings format and earlier row/payload contracts are unchanged. Project discovery supplies no issue authority.

Candidate arguments cannot claim actor, grant or operation authority. Captured configuration is rechecked before auth, throughout resource/cached access and after final observation/output/cursor work. Public parser/catalog registration remains unavailable. No dependency, manifest or lock changes are required for this addition.

## Focused observed checks

Six new runtime tests passed alongside existing create/project policy, profile and provider-cursor tests in `target/issue-read-policy-focused.log`. They cover apply/output refusal, sorted explicit operation allowance, reopen, revision CAS, operation removal/revocation, competing writers with one winner, wrong target/actor/profile/mode, duplicate/empty operations, corrupt identity/revision, unknown/duplicate fields and unknown/duplicate/noncanonical operations. Migration from schema five retains actual settings, profile/grant, active selection, create/project policies, journal evidence and usable local/provider cursor continuation. An early-schema conflict rolls back intermediate DDL and retains the old version/profiles; unexpected issue-policy triggers are refused.

Older migration fixtures now remove the added table when simulating earlier schemas. Their expected final version is six. The first focused run stopped on a stale schema-five assertion in the create-policy test; the corrected focused run passed. The tests still exercise their intended older-table conflicts rather than failing incidentally on an unrelated later table.

Four new candidate tests passed in `target/issue-read-arguments-focused.log`: flag/JSON scalar and array parity, defaults and strict schema refusal; malformed input/projection/budget without state creation; missing/disabled/get-only configuration and refusal to inherit project authority; and isolated child-process profile/site precedence with explicit profile bypass of malformed hints. Parent working-directory and environment state are unchanged.

One new dispatch test passed with the existing create/project tests in `target/issue-read-dispatch-focused.log`. Both get and search retain their operation-specific decoded arguments, original request identity and complete result/error/cache evidence; search also preserves its oversized-item recovery cursor. The original candidate dispatch cases continue to pass.

The composed issue target now has nine passing tests in `target/issue-read-command-composed.log`. Its new case exercises parsed get/search arguments, actual profile/private policy and cached host execution across thirteen scenarios: success; revocation before auth; during auth; after the native read; an enabled revision change; operation removal; and search buffered replay. Late failures return no issue/page/items, preserve renewal receipts and have exact issuer/product request counts.

The initial composed regression failed for get after a configuration change during final timestamp work, retained in `target/issue-read-command-composed-initial.log`. A final policy/control check now runs after get output qualification and after the search engine returns. The corrected complete composed run passed. A late search refusal may leave an unused successor row; the predecessor remains immutable and normal TTL bounds that row. This is not distributed fencing or a claim of atomic cloud permission revocation.

Focused all-target Clippy with the synthetic feature passed in `target/issue-read-command-clippy.log`, without suppressions.

## Full gate and executable checkpoint

`python tools/verify.py` passed in `target/issue-read-command-gate.log`: **368 default passes**, **26 composed passes** (seven create, ten project-list and nine issue-read) and **117 feature-runtime passes**. This is **396 distinct passing Rust tests**, zero failures, or **511 reported passing executions**, including 115 repeated runtime cases. Five distinct opt-ins remain ignored (eight occurrences). Both Clippy modes, formatting, public contract snapshot/negative controls, provider/source inventory, dependency/layer guards, planning and synthetic-feature isolation passed. Test counts are independently extracted from result lines and normalized executable/test identities in `target/issue-read-command-test-counts.json`. Existing incremental-cache hard-link warnings used copying without failing the gate.

The Windows native cache restart/contention/logout opt-in passed separately after migration, recorded in `target/issue-read-command-native-cache.log`:

```text
cargo test -p craft-runtime --lib --locked --offline -- --ignored --exact auth::service::tests::native_tests::windows_service_cache_restarts_serializes_reacquisition_and_preserves_logout --nocapture
```

That one parent test checks six fresh children, native synthetic-credential persistence, shared renewal, logout refusal, sentinel redaction and owned cleanup. It does not exercise a live tenant or public provisioning. Other native opt-ins were not rerun.

`cargo build --workspace --locked --offline --no-default-features` passed after fixture testing, recorded in `target/issue-read-command-default-build.log`. The default development binary SHA-256 is `bb017e8eceab913b7c68a4de04cb23c0612de7d41d4de5f9dc61467249ef2431`. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Manifests, package graph, lock and dependency inventory are unchanged. Final source/doc hashes and actual public discovery are recorded in `target/issue-read-command-checkpoint-hashes.json` and `target/issue-read-command-checkpoint-validation.json`.

## Limits

Prior [issue-read evidence](jira-issue-reads.md) and [project candidate evidence](project-list-command.md) retain their historical scopes. This addition does not qualify live tenant eligibility, independent review, public native credential input/provisioning, other platforms or compliance. Broader Jira context/paging/auth tasks stay unchecked; remaining metadata/CRUD, operational public configuration, recovery, Confluence workflows and final delivery qualification remain required.
