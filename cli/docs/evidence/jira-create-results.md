# Jira create-result acceptance

Date: **10 November 2026**, Australia/Sydney. Partial provider/host integration only. No cloud command, provider registration or live Jira mutation is enabled. The Jira CRUD task remains unchecked. Independent consequential review has not occurred.

## Changed surfaces

- New `craft-provider-jira` crate: pure create-result interpretation with a core-only workspace dependency.
- `craft-cli/src/issue_create.rs`: candidate output preflight, response interpretation, claim-based receipt recording, projected schema and evidence-preserving envelope composition.
- Runtime journal/transport: read-only accessors expose the claim's immutable binding/step and the original command/route on an attempt. They provide no new send or retry authority.
- A runtime development dependency on the Jira module supports the actual TLS response-to-decoder-to-journal test. Production runtime remains independent of provider modules.

The [contract](../jira-create-results.md), [provider recovery scenario](../../openspec/changes/build-cloud-cli/specs/cloud-providers/spec.md#scenario-created-issue-with-incomplete-or-secondary-failure-evidence) and [design](../../openspec/changes/build-cloud-cli/design.md#native-providers-and-factory-workflow) describe the intended boundary. The parser was checked against pinned source `/paths/~1rest~1api~13~1issue/post/responses`, `CreatedIssue`, `NestedResponse`, `ErrorCollection` and `WarningCollection`. Native optional string schemas do not establish Craft's stricter ID/key bounds.

## Observed verification

The first focused run passed the then-current **five completion tests**, six provider parser tests and existing CLI library cases in `target/jira-create-completion-focused-output.log`. The provider cases cover the native response example, retained IDs under malformed optional metadata, missing/wrong/oversized IDs, unknown statuses, secondary failure/uncertainty, documented refusal distinctions, wrong project authority and omission of free-text/provider URLs from output.

CLI completion tests exercise actual Windows private SQLite claim recording with a synthetic native response after cancellation, projected/full schema agreement, output preflight, persistence errors, mismatched completion/record bindings, native rate advice and the preserve-evidence encoder. Follow-up coverage preserves preexisting resource/job evidence and rejects contradictory refusal or multiple-effect success claims. The completion fixture does not send HTTP and is not labeled a live integration.

`cargo test --locked --offline -p craft-runtime --lib native_create_response_flows_through_jira_interpretation_and_durable_partial_receipt` passed in `target/jira-create-native-composition-output.log`. The fixture performs one real loopback TLS POST to the create route, uses the actual Jira decoder on a 201 response with a failed watcher update, and records the known issue ID in an actual journal reopened afterward. Wire route/method, immutable attempt metadata, partial classification and absence of raw error text in SQLite are checked. Only the fixture can override the fixed origin.

All-target Clippy passed at the earlier focused stage in `target/jira-create-clippy-output.log`. The workspace inventory review found **one added workspace package**, CLI and runtime dependency/manifest changes, no removed package, and **no changed registry package, version, checksum or feature set**. The inventory was intentionally regenerated at 283 packages: 276 external and seven workspace packages.

Final `python tools/verify.py` passed **268 Rust tests, zero failures and five ignored opt-ins**, the unchanged twenty-operation public contract, ten contract controls, pinned provider inventory/controls, formatting, dependency/layer checks and all-target Clippy. Output: `target/jira-create-integrated-output.log`. This includes six completion tests, six provider decoder tests and the native TLS composition case. Separate native-vault/live-provider opt-ins did not run.

`cargo build --workspace --locked --offline` passed in `target/jira-create-build-output.log`. Cargo's incremental-cache hard-link warnings used copying fallback. Source, manifest, lock, inventory, public schema and executable hashes are in `target/jira-create-checkpoint-hashes.json`. Debug executable SHA-256: `79529e8c983734e2e45fcf5488775f9458738371b79643ec23dcd47810b454b4`. The lock changed only for the reviewed workspace addition/edges; its digest is `6a458282ff4e0c28775244e35102487f0974aff31a3312e98380868493fa7d1d`. Public contract digest remains `7d2bc7c99092ae40902a3d70013df9d6a6c41814535d29ca076470be7d23322e`.

Planning validation passed with 85 tasks and 14 complete. The partial response adapter does not close the Jira CRUD or host activation tasks.

Nine changed documentation files passed local-link/fence checks. All 127 checkpointed source/manifest/lock/inventory/schema/binary files matched their digests in `target/jira-create-checkpoint-validation.json`.

## Remaining acceptance

Source parsing, loopback TLS and native local persistence do not establish successful creation in Jira. The full public input/metadata/auth/policy/send/output path is not composed. An end-to-end CLI invocation creating a sandbox issue has not run. Provider reconciliation, live permissions/custom fields, other native platforms, process death before receipt capture, agent efficiency evaluation and independent consequential review remain unfinished. Public discovery still has twenty local operations and no cloud provider.
