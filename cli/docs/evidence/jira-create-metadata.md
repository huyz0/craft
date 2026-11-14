# Jira create-metadata acceptance

Date: **10 November 2026**, Australia/Sydney. Candidate library/read-provider implementation only. Public CLI discovery still advertises twenty local operations and no cloud provider. The Jira context and CRUD tasks remain incomplete; no live Jira metadata request or issue creation was performed.

## Scope

`craft-provider-jira/src/create_metadata/` owns declared read routes, bounded page consumption, native field normalization, selected response projection, private principal/freshness-bound context and typed frozen-body preparation. It reuses core's `FieldMapContract` and omission/null model. The [contract](../jira-create-metadata.md), [scenario](../../openspec/changes/build-cloud-cli/specs/cloud-providers/spec.md#scenario-complete-bound-create-metadata) and [design](../../openspec/changes/build-cloud-cli/design.md#native-providers-and-factory-workflow) describe behavior and remaining limits.

The existing pinned Jira source was inspected for project retrieval, both paginated create-metadata operations, page aliases, field/default/operation flags and native schema labels. No source was repinned and no dependency changed. Native `JsonTypeBean` is not interpreted as arbitrary JSON Schema; only explicit qualified shapes compile into closed core field schemas.

## Observed checks

`cargo test --locked --offline -p craft-provider-jira --test create_metadata` passed **eight tests** in `target/jira-metadata-boundaries-output.log`. Earlier six-test stages are retained in `target/jira-metadata-focused-output.log` and `target/jira-metadata-input-output.log`. Fixtures cover complete paginated metadata, exact project/type/principal/operation binding, dual-clock expiry and rollback, unsupported subtask creation, required/default/read-only/custom field handling, option IDs, literal ADF, unknown fields, explicit null, duplicate labels, foreign schema keys, offset/total/alias/duplicate/no-progress/page-limit failures, status errors and cancellation. Registry execution tests actual nested projection while retaining complete/non-authority flags.

The sequential TLS fixture now supports a finite reply sequence and fails unexpected extra requests. `target/jira-metadata-native-qualified-output.log` records the passing native integration test: actual read context sends project/type/field GETs, an invented custom field fails with no POST or journal intent, a qualified body is frozen and verified, and one journal-bound POST returns a decoded issue ID retained in a reopened Windows journal. Wire methods/routes and exact generated project/type/summary payload are checked.

The first native integration attempt failed descriptor registration because the borrowed synthetic create descriptor still had an empty example after its input became typed. `target/jira-metadata-native-composition-output.log` retains that failure. The example was corrected to provide the required summary; no validation or request check was weakened.

The final send-boundary audit added `PreparedHttpMutation::send_checked`, running a trusted frozen-context preflight after claim acquisition with invocation-control checks before and after it. `target/jira-metadata-send-freshness-output.log` records the native test with both fresh and expired metadata. The expired case advances the same context clock at the final boundary: it produces only the three metadata GETs, returns a version conflict with conservative write uncertainty, and durably records unknown state instead of issuing the POST.

The first integrated gate passed its tests but failed Clippy on two unnecessary borrowed field paths; the calls were corrected without suppressing the lint. Its output remains `target/jira-metadata-integrated-output.log`.

Final `python tools/verify.py` passed **277 Rust tests, zero failures and five ignored opt-ins**, the unchanged twenty-operation public snapshot, ten contract controls, pinned provider inventory/controls, formatting, locked dependency/layer checks and all-target Clippy. Output: `target/jira-metadata-final-integrated-output.log`. Separate native-vault/live-provider opt-ins did not run. `cargo build --workspace --locked --offline` passed in `target/jira-metadata-build-output.log`. Cargo reported incremental-cache hard-link warnings and copied files instead.

Source, manifest, lock, inventory, public schema and executable hashes are in `target/jira-metadata-checkpoint-hashes.json`. Current debug executable SHA-256: `4f6ad76165d43b026fd2c8422bf23f3d1a4abda3a6f93f941d9d7ef2f282c456`. Lock and public contract digests remain unchanged. Planning checks passed with 85 tasks and 14 complete; no broad task was checked complete from this partial metadata implementation.

Nine changed documentation files passed local-link/fence checks. All 131 checkpointed files matched their digests, and lock, inventory and public contract matched the prior checkpoint in `target/jira-metadata-checkpoint-validation.json`.

The later [parent/assignee checkpoint](jira-parent-assignee.md) adds positive subtask-parent and active-account qualification, extending the earlier explicit unsupported boundary. Broader hierarchy, explicit unassignment, reporter/security/custom-user semantics and public/live integration remain incomplete.

## Limits

This is author verification with synthetic native responses, loopback TLS and private Windows SQLite. It does not prove actual scope/permission eligibility, tenant field semantics, stable snapshots, a public CLI create, all custom-field families, subtask parent/account/security qualification, other native platforms or independent consequential review. The host must verify its supplied principal and enforce frozen-context freshness/policy at the actual send boundary. General bounded listing/cursor, provider reconciliation, auth activation and complete CRUD acceptance remain unfinished.
