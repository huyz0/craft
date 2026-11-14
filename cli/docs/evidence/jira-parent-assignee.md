# Parent and assignee qualification acceptance

Date: **10 November 2026**, Australia/Sydney. Internal candidate Jira workflows only. No live provider request, real credential, public parser/catalog operation or cloud provider was enabled. Independent consequential review has not occurred and the broader Jira context/CRUD tasks remain unchecked.

## Implementation and source

The [contract](../jira-parent-assignee.md), [provider scenario](../../openspec/changes/build-cloud-cli/specs/cloud-providers/spec.md#scenario-qualified-parent-and-assignee) and [design](../../openspec/changes/build-cloud-cli/design.md#native-providers-and-factory-workflow) describe this partial integration. Create metadata now carries conditional exact-parent evidence and compiles parent/user field schemas. A separate `create_metadata/accounts.rs` read helper supplies a non-importable assignable-account context under its own scope. The frozen create retains both freshness checks and maps only qualified parent/account IDs into the native body.

The pinned source was inspected for `getIssue` field selection, fail-fast/history options, implicit moved-issue resolution, project/security visibility requirements, `findAssignableUsers` exact account/project query, scope/permission declarations, filtering limits and `User.active`. No source, dependency or public contract was repinned. The account helper is internal prerequisite implementation and does not add an advertised catalog command or user-administration capability.

## Observed verification

`cargo test --locked --offline -p craft-provider-jira --test create_metadata` passed **fourteen tests**, including six new parent/account cases, in `target/jira-parent-account-focused-output.log`. Existing eight metadata cases also passed separately in `target/jira-parent-account-regression-output.log`.

New coverage verifies exact parent identity/project/type, missing visibility and permission errors, parent omission/null/mismatch refusal, exact active account results, empty/wrong/duplicate/inactive/missing-active/name-only results, no alias/broad search, separate declared scopes, account metadata constraints, read-only user fields, unsupported explicit unassignment and proof binding to project/principal/account. The account proof's separate clock can expire a previously frozen body while metadata itself remains fresh. Normalized account evidence excludes email/name/URL sentinels.

`cargo test --locked --offline -p craft-runtime --lib subtask_and_assignee_proofs_guard_the_final_tls_create_send` passed against actual loopback TLS and private Windows SQLite in `target/jira-parent-account-native-output.log`:

- Fresh evidence produces five declared GETs and exactly one POST with the generated project/type/summary/parent/account-ID fields; the known created ID survives journal reopen.
- A cross-project parent produces only four GETs, no write claim and no POST.
- Account evidence expiring at the final claimed-send preflight produces five GETs and no POST, returning version conflict and a durable unknown marker.

The test checks actual wire query fields/history options and exact project/account lookup without a name query. A synthetic email returned by the account endpoint is absent from SQLite metadata. All-target Clippy passed in `target/jira-parent-account-clippy-output.log`.

Final `python tools/verify.py` passed **284 Rust tests, zero failures and five ignored opt-ins**, the unchanged twenty-operation public snapshot, ten contract controls, pinned provider inventory/controls, formatting, locked dependency/layer checks and all-target Clippy. Output: `target/jira-parent-account-integrated-output.log`. Separate native-vault/live-provider opt-ins did not run.

`cargo build --workspace --locked --offline` passed in `target/jira-parent-account-build-output.log`. Cargo's incremental-cache hard-link warnings used copying fallback. Source/manifest/lock/inventory/schema/binary hashes are in `target/jira-parent-account-checkpoint-hashes.json`. Current debug executable SHA-256: `8080b96ab0d78f37af23be911590bad4d2d2f43d71fef6e2ca7403b87cb81acb`. Dependency lock, inventory and public contract remain unchanged. Planning validation passed with 85 tasks and 14 complete; no broad context/CRUD task was checked complete.

Ten changed documentation files passed local-link/fence checks. All 132 checkpointed files matched their hashes; lock, inventory and public contract matched the prior checkpoint in `target/jira-parent-account-checkpoint-validation.json`.

## Limits

All evidence uses synthetic native responses, owned loopback TLS and local Windows state. It does not prove real Jira permissions, service-token scope eligibility, native unassignment, reporter/security/custom-user semantics, arbitrary hierarchy parents, live response shapes, other operating systems or independent review. Parent and account facts can change after a read; bounded freshness is not snapshot isolation. The host must bind its asserted principal to authenticated session/policy and complete public output/recovery composition. General listing, binary transfer, provider reconciliation and complete CRUD remain unfinished.
