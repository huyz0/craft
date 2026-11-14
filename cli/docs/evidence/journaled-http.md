# Journal-bound HTTP acceptance

Date: **10 November 2026**, Australia/Sydney. Candidate host-library integration only. No public command, auth path, provider handler or live mutation was enabled. The broader transport/journal tasks remain unchecked and independent consequential review has not occurred.

## Observed behavior

The [contract](../journaled-http.md) and [review package](../review-journaled-http.md) describe production changes and activation gaps. The code prepares a private fixed-origin JSON request, consumes it after exact journal claim acquisition and retains native transport evidence separately from local unknown-recording disposition. Ordinary read contexts still refuse mutations. Journal waits use the same invocation control as HTTP. No dependency or public CLI schema changed.

`cargo test --locked --offline -p craft-runtime --lib transport::mutations::tests` passed **eleven tests** with native filesystem/loopback TLS access, recorded in `target/journaled-http-race-output.log`. Earlier focused stages passed seven and nine tests in `target/journaled-http-focused-output.log` and `target/journaled-http-boundaries-output.log`.

| Boundary | Observed result |
| --- | --- |
| Fixed-origin public preparation | The constructed private request targets the expected Atlassian tenant/route with a sensitive bearer header; read effect, GET mutation and wrong-tenant sessions are refused before claims |
| Immutable native request | Changing the caller's original body after preparation does not change wire bytes or the persisted exact native-request digest |
| Preflight/apply/grant | Bad route/query/path/body, missing apply and mismatched session/authority grants yield no journal claim or TLS connection |
| Concurrent contenders | Two ready/released contenders with different native bodies produce one committed claim and one complete HTTP request; the stored digest matches the winning wire body |
| Lost marker acknowledgment | Injected failure after the durable send-marker commit returns uncertainty, sends zero connections and leaves a started marker that refuses another claim |
| HTTP response | A 201 response remains unknown until explicit provider-qualified recording; IDs and an accepted job then survive a reopened journal |
| Untrusted/invalid responses | Disconnect, duplicate JSON, oversized body, wrong media type and foreign redirect each retain unknown state; only one request is observed and no retry is allowed |
| Native status/rate advice | 429 preserves status and typed Retry-After, omits its provider error body and leaves unknown state without retry |
| Cancel/deadline before send | Cancelled preflight and journal contention send no connection; the HTTP deadline bounds a separately opened long-lived journal |
| Cancel after claim | An exact-boundary hook cancels after claim acquisition; zero connections occur, and unknown evidence persists without reopening the claim |
| Cancel/deadline after send | A real stalled TLS request receives one HTTP request; invocation deadline or ready-confirmed cancellation returns durable unknown state |
| Receipt persistence | Injected before/after unknown-record commit failures preserve the known native response and durable started/unknown marker; qualified evidence can still be recorded after cancellation |
| Nested executor | Consuming send inside an existing Tokio runtime returns refusal without panic, claim or connection |
| Non-secret journal | Raw native-body and bearer sentinels are absent from SQLite metadata |

`python tools/verify.py` passed **255 Rust tests, zero failures and five ignored opt-ins**, the twenty-operation public contract, ten contract controls, pinned provider inventory/controls, dependency/layer checks, formatting and all-target Clippy. Output: `target/journaled-http-integrated-output.log`. This gate does not run the separate native-vault/live-provider opt-ins. The earlier read-transport and journal suites also ran here, preserving the ordinary mutation-construction refusal and existing claim/receipt behavior.

`cargo build --workspace --locked --offline` passed in `target/journaled-http-build-output.log`. Cargo reported incremental-cache hard-link warnings and copied files instead. Source, manifest, lock, public contract and executable hashes are in `target/journaled-http-checkpoint-hashes.json`. Current debug executable SHA-256: `8612006921ede712a153f7b69bcb9550e2ed3f30d5e5ce75e9743e9c98075fce`. The dependency lock and public contract snapshot retain their previous hashes; no provider was advertised.

Final planning validation passed with 85 tasks and 14 complete. Eleven changed documentation files passed local-link/fence checks, and all 120 checkpointed files matched their hashes in `target/journaled-http-checkpoint-validation.json`. No broad task was checked complete from this partial integration.

## Limits

Author tests use Windows private SQLite and local TLS with synthetic bearer/actor data. They do not establish real OAuth access, provider semantics, a deployment approval, live recovery, another native OS, power-loss durability, cross-machine fencing or independence of review. TLS overrides exist only in fixtures. Known responses still require provider-specific parsing and explicit receipt/output preservation. A lost process before those IDs are recorded can leave only the durable uncertainty marker. No automatic replay, rollback or exactly-once guarantee is supplied.
