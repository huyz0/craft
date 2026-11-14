# Jira create-host acceptance

Date: **10 November 2026**, Australia/Sydney. Partial internal implementation. The public CLI still advertises twenty local operations and no cloud provider. No live tenant, real credential, release or publication was changed. The broader create/CRUD and authentication tasks remain unchecked.

## Changes

The [host contract](../jira-create-host.md) and [review package](../review-jira-create-host.md) describe the composed path. New `issue_create/host.rs` prepares strict input, exact trusted policy, explicit apply/operation identity and output before authentication. It derives metadata/account requests from sealed input and verified service access, inspects prior operation state, freezes the qualified body and uses the checked journaled send/completion path. Issue type is now required in the logical input and must match metadata.

Runtime `auth/service/access.rs` retains private product/actor/grant, token lifetime and scopes without exposing bearer bytes. `open_service_access` and existing `open_service_session` share cache acquisition; confirmed updates survive later access/session construction failure. Cached host execution holds its selected lock through completion. The journal can now share its handle with current invocation control for bounded host inspection/waits. TLS policy cloning carries the same selected trust roots across the composed contexts, without an origin override.

Projected create output retains local cache-write evidence separately from resource effects. Before-send metadata/auth failures cannot masquerade as issue submission. Matching prior positive journal evidence survives replay refusal. Output preflight reserves the longest request-state/cleanup encodings and bounded grant identity, and post-effect completion retains evidence on persistence/output failure.

## Observed checks

The focused `cargo test -p craft-cli --lib issue_create --locked --offline` run passed **12 tests**, including six new host cases, in `target/jira-create-host-final-focused-output.log`. New cases cover malformed/missing issue type, invalid labels/parent/account/custom fields, oversized input, policy/apply/output refusal, unsupported cursor/preconditions, issue-type digest changes, generic-handler refusal, metadata/account body composition, freshness, prior-operation refusal/retained receipts and separate cache/resource effects under projection.

Provider metadata now has **15 tests**, including a direct registry refusal of missing issue type and rejection of a different explicit type against qualified metadata. Synthetic fixtures seed their known issue type only in test helpers; production has no fallback.

Two new runtime access tests exercise actual loopback OAuth/current-user TLS requests and verify sealed identity, target/descriptor changes, token expiry, scope alternative conjunctions and empty/insufficient scope refusal. Two cache tests exercise the access constructor's reuse/renewal/logout and retention of a confirmed activation when access conversion later expires. Existing session/cache tests also run in the integrated gate.

The first focused host run found an invalid projected descriptor example; the example now contains schema-valid data. Initial service tests, including an unchanged existing test, failed under restricted TLS execution and passed with native loopback access. The restricted integrated gate later stopped on the existing hard-link fixture with Windows Access Denied, retained in `target/jira-create-host-integrated-restricted-output.log`. The first Clippy run reported one collapsible conditional, corrected without suppression; its output is in `target/jira-create-host-clippy-output.log`.

A native-access `python tools/verify.py` checkpoint passed **295 Rust tests, zero failures and five ignored opt-ins**, the unchanged public contract, provider inventory/controls, formatting, dependency/layer checks and all-target Clippy, in `target/jira-create-host-integrated-output.log`. After the input/output-bound tightening, the final full gate passed again in `target/jira-create-host-final-integrated-output.log`. The final log totals were parsed and checked as 295/0/5. Inventory remains 283 packages and seven workspace crates; the public snapshot remains twenty local operations.

`cargo build --workspace --locked --offline` passed in `target/jira-create-host-build-output.log`. The debug executable SHA-256 is `e24a10a150cfa7532b0377f590b3d76e5f3a0dfeb5369f7beb90b7f2d6e33191`. The dependency lock and public schema match the preceding checkpoint. Source/document/binary hashes and comparison results are retained in `target/jira-create-host-checkpoint-hashes.json` and `target/jira-create-host-checkpoint-validation.json`. Planning validation remains 85 tasks and 14 complete; no broad task checkbox was closed. Cargo used its copying fallback for incremental-cache hard-link warnings.

The existing `windows_service_cache_restarts_serializes_reacquisition_and_preserves_logout` opt-in passed separately against the actual Windows vault and owned synthetic TLS servers, retained in `target/jira-create-host-native-cache-output.log`. This rechecks the shared cache refactor across fresh processes, renewal contention and logout; it does not exercise the new CLI host end to end. Other native opt-ins were not rerun.

## Remaining evidence

Subsequent [composed acceptance](composed-host-acceptance.md) closes the whole-host fixture gap described at this checkpoint. The original observations below remain historical; live/public/independent-review limits still apply.

Host metadata composition uses scripted `OperationContext` fixtures. Actual OAuth/access/cache, journal/TLS sends and completion each have separate coverage, but no single test currently exercises the new complete host through an owned server and real sealed cache access. No public parser/dispatch/create command or live provider behavior is qualified. Independent consequential review remains outstanding. No public origin override or test-credential constructor was added to bridge that gap.

Parent/account and metadata facts can change after reading; freshness is not snapshot isolation. Direct-token hosts own their grant/lifecycle binding. General provider reconciliation/listing, remaining field families, complete CRUD, public auth and non-Windows qualification remain required. See earlier [create completion](jira-create-results.md), [metadata](jira-create-metadata.md), [parent/account](jira-parent-assignee.md), [journaled HTTP](journaled-http.md) and [service auth](service-auth.md) evidence.
