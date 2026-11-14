# Project-context candidate and permission acceptance

Date: **10 November 2026**, Australia/Sydney. Internal candidate qualification. No public provider/command, live tenant operation or real credential provisioning was enabled.

## Changed behavior

The [contract](../project-context-command.md) connects typed get/status arguments, exact project selection, shared site/actor configuration with explicit per-project permissions and complete dispatch to the [qualified host](jira-project-context.md). Source is in `crates/craft-cli/src/project_context/command*`, `args.rs`, `dispatch.rs`, and runtime `project_read_policy.rs` plus its `context.rs` child.

Legacy payload format one remains list-only. Format two adds bounded independent operation permissions; explicit applied CAS/preflight edits perform the upgrade and preserve listing/other target intent. Enabled permissions cannot silently transfer to a new actor. SQLite schema six, settings format, manifests, package graph, lock, dependency inventory and public schema remain unchanged.

## Focused observed checks

Six new permission tests and four existing project-read tests passed in `target/project-context-policy-focused.log`. They cover missing base configuration, apply/preflight refusal without upgrade, explicit upgrade/reopen, independent listing/get/status allowance, exact case-sensitive selectors, unchanged other permissions, whole-record CAS with one competing writer, actor-transfer refusal, explicit disablement before actor changes, capacity refusal without eviction/revision changes and strict corrupt/legacy/unknown/duplicate/foreign payload rejection. Database version remains six. Earlier schema/cursor/journal migration tests retain their original acceptance path.

Four new candidate tests passed in `target/project-context-arguments-focused.log`: flag/JSON parity and defaults; strict authority/type/input/projection/cursor/budget refusal before state creation; exact explicit/default project and operation selection with no list-only authority inheritance; and isolated child-process profile/project precedence. Explicit profile bypasses malformed hints, explicit selector overrides the hinted project, foreign targets are refused and missing project context is an error. Parent environment/working-directory state is unchanged.

One new dispatch test passed alongside the three existing candidate dispatch tests in `target/project-context-dispatch-focused.log`. Project actions retain their specific decoded arguments, original request identity and complete result/error/cache/warning evidence. Statuses retains oversized-group recovery. The existing create, issue-read and project-list paths still pass through the shared seam.

The composed target now contains eight passing tests in `target/project-context-command-composed.log`. The new case exercises parsed get/status arguments, actual private records, service/cache access and owned TLS servers across thirteen scenarios: success, revocation before auth, during auth, after native reads, enabled revision change, operation removal and status buffered replay. Success works while listing remains disabled. Late failures suppress project/page/items, retain confirmed renewal receipts, satisfy generated response schemas and have exact issuer/product request counts.

The first focused permission, candidate, dispatch and composed runs passed. Feature-enabled all-target Clippy passed in `target/project-context-command-clippy.log`, without suppressions.

## Full gate and executable checkpoint

`python tools/verify.py` passed in `target/project-context-command-gate.log`: **389 default passes**, **34 composed passes** (seven create, ten project-list, nine issue-read and eight project-context), and **117 feature-runtime passes**. This is **425 distinct passing Rust tests**, zero failures, or **540 reported passing executions**, including 115 repeated runtime cases. Five distinct opt-ins remain ignored (eight occurrences). Both Clippy modes, formatting, public contract snapshot/negative controls, provider/source inventory, dependency/layer guards, planning and synthetic-feature isolation passed. Counts are independently extracted from result lines and normalized executable/test identities in `target/project-context-command-test-counts.json`. Existing incremental hard-link fallback warnings did not prevent compilation or Clippy.

`cargo build --workspace --locked --offline --no-default-features` passed after fixture testing, recorded in `target/project-context-command-default-build.log`. The default development binary SHA-256 is `63e3ef1969fe81e4cd081cdd68c51726f606cfe7f5ef44fd0b967cd950b462a8`. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Final actual discovery, source/doc hashes and unchanged baseline checks are recorded in `target/project-context-command-checkpoint-validation.json` and `target/project-context-command-checkpoint-hashes.json`. Native opt-ins were not rerun; native vault and database schema implementations did not change.

## Limits

No live tenant, public native credential input/provisioning, independent reviewer, other-platform run, compliance assurance or deployment was exercised. Shared-record revision checks do not provide distributed fencing or immutable audit history. Project status membership still does not prove issue-specific transition availability. Prior [project-context evidence](jira-project-context.md), [issue-read candidate evidence](issue-read-command.md) and [project-list evidence](project-list-command.md) retain their recorded scopes. The broader Jira context/auth/paging tasks and full CLI delivery scope remain unfinished.
