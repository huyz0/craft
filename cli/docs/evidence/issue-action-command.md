# Action candidate and policy evidence

Date: **10 November 2026**, Australia/Sydney. Internal candidate/private-policy qualification only. No public command, live tenant mutation, real credential provisioning, publication or deployment was enabled. Baseline: [issue action provider and journaled host](jira-issue-actions.md).

## Contract and implementation

The [command/policy contract](../issue-action-command.md), active provider scenarios and design define required non-null assignee/subtask intent and a versioned explicit policy update before implementation.

`crates/craft-runtime/src/issue_mutation_policy.rs` appends independent assign, unassign, delete and delete-all-subtasks allowances. Newly written records use format two under the existing trusted apply/preflight/revision-CAS setter. Valid format-one edit/transition rows remain readable without rewriting; action strings in legacy payloads stay invalid even though the enum now recognizes them. Unknown formats/operations, duplicate/unsorted and foreign rows still fail without repair. The SQLite schema/table, settings and other policy encodings are unchanged.

`crates/craft-cli/src/issue_mutation/command/actions.rs` supplies three strict candidate argument types. Expected assignee and subtask intent are optional only while flags await structured input; custom serde decoding and non-null required schemas reject missing/null structured intent. Local validation rejects omitted flags before state access. The parent command module selects the exact delete/cascade allowance, reuses profile/site/project resolution and captures the entire policy row through the existing host's dynamic checks. `args.rs` and `dispatch.rs` preserve typed decoding, operation identity and complete recovery results. Public Jira parsing/catalog activation remains skipped.

## Focused qualification

`target/issue-action-command-policy-red.log` records the failure-first format regression: the trusted setter still emitted one rather than the required two. `target/issue-action-command-policy-qualified.log` passes eight policy tests, including two additions for byte-preserving legacy reads, rejection of all four action strings in legacy payloads, no inferred permissions, failed-preflight preservation, explicit upgrade/CAS and the full exact-operation permission matrix. The existing malformed-format/unknown-operation controls now use format three and an unknown operation; the new legacy test retains refusal of action strings in format one.

`target/issue-action-command-candidates-red.log` records the schema test failing because action candidates were not registered. The corrected schema test passes in `target/issue-action-command-schema-first.log`. Four action candidate tests pass in `target/issue-action-command-unit-final.log`: required/non-null/closed schema agreement, flag/JSON parity with no synthesized intent, malformed/duplicate/local input/apply/projection/budget rejection before state, independent operation allowance, exact captured record and wrong project/site refusal. The existing edit/transition tests also passed in `target/issue-action-command-unit-qualified.log`.

`target/issue-action-command-dispatch.log` passes the added four-mode dispatch case. It preserves expected state, destination account or explicit cascade mode, original request identity, unknown-write/cache/prior-record data, warnings and evidence-preserving output.

Owned TLS tests in `crates/craft-cli/tests/jira_issue_mutation_host/actions/candidates.rs` pass in `target/issue-action-command-tls-qualified.log`. Three tests cover **57 scenarios**: four action modes across success/excluded/pre-auth/claimed-revocation/claimed-revision/frozen-clock-revocation/lost/refused outcomes (32); wrong project, state drift, assignee/child drift and native metadata refusal before a claim (16); persisted revocation after each completed native read (9). They use the actual typed candidates, private records, sealed cached/renewed service access and journaled host. Pre-claim failures send no mutation and retain completed renewal evidence; claimed refusals preserve uncertainty without resource sends; terminal/unknown attempts cannot replay. Synthetic secrets are checked absent from state. `target/issue-action-command-tls-first.log` records the initial passing 48-scenario subset before read-boundary cases were added.

Feature-enabled all-target Clippy passes in `target/issue-action-command-clippy-first.log`.

## Final checkpoint

The final `python tools/verify.py` passed in `target/issue-action-command-gate-final.log`: **521 default**, **68 composed-host** and **123 feature-runtime** passing executions. Normalized identities in `target/issue-action-command-test-counts.json` establish **591 distinct passing Rust tests**, zero failures and **712 passing executions**, including 121 repeated runtime cases. Five distinct opt-ins remain ignored (eight occurrences). Both all-target Clippy modes, formatting, public contracts, provider/dependency/layer checks, planning and synthetic isolation pass. All 230 captured source/toolchain/tool digests remained unchanged through the final gate. Existing incremental-cache hard-link fallback warnings did not prevent qualification.

`cargo build --workspace --locked --offline --no-default-features` passed in `target/issue-action-command-default-build.log`, restoring default binary SHA-256 `05dec76e94511679ada91e94ef24e889b1ae333c2c55498689ba32f6635b0342`. Actual execution in `target/issue-action-command-discovery.json` confirms twenty local operations, no cloud provider and unavailable schema/parser lookup for edit, transition, assignment, unassignment and deletion. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`.

`target/issue-action-command-checkpoint-hashes.json` and `target/issue-action-command-checkpoint-validation.json` retain 294 source/document/binary digests and checks over 61 documents. Local links resolve and code fences balance. Manifests, lock, dependency inventory and public schema match the previous checkpoint. SQLite schema eight and settings format one are unchanged. Mutation policy reads support legacy format one and expanded format two; explicit trusted updates write two. Other policy encodings remain unchanged. Native-vault/cache opt-ins were not rerun because their implementation and persistence schema did not change. The ledger remains 16 of 87 complete, with broad issue-write work still unchecked.

## Remaining delivery

Further reconciliation/postcondition tooling, remaining Jira/Confluence resources, independent consequential review and actual public/live qualification remain unfinished. The broad issue-write task remains unchecked. Private policy expresses local host intent, not native permission, human approval or distributed fencing. No independent consequential review or other-platform/live test has been performed for this slice.
