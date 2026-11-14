# Bounded retries and invocation deadlines

Date: **14 November 2026**, Australia/Sydney. Windows x86_64 MSVC and Linux x86_64 (Debian bookworm container), pinned Rust 1.99.1. The [design section](../../openspec/changes/build-cloud-cli/design.md#bounded-read-retry-and-invocation-deadlines) is the contract.

## What changed

- `crates/craft-runtime/src/transport/retry.rs` holds the policy. Only a sealed descriptor with `EffectClass::Read` and `RetryClass::SafeRead` opts in; the HTTP method is never consulted, so POST searches qualify and journaled writes never do.
- The JSON read context (`HttpContext::invoke`) sends at most three attempts. It repeats HTTP 429, 502, 503 and 504, and a connection that drops after the request was written. Connect, DNS and certificate failures, other statuses, invalid or oversized bodies, redirects, cancellation and deadline expiry end on the first occurrence. The context rechecks its own effect before repeating, so a context labelled as a write cannot repeat a send even if its descriptor was a read.
- Waits use `Retry-After` (delta seconds, or an HTTP date read against the invocation clock's wall time) and otherwise 250 ms doubling to a 4 s cap. Advice above 30 s, or a wait that would leave under one second before the invocation deadline, is not taken: the final native response goes back to the provider unchanged, so its typed rate or availability error keeps the original advice and request ID. Waits poll cancellation and the deadline every 10 ms. No attempt resets the invocation budget.
- A transport failure after more than one send says how many bounded attempts were already made. Provider read errors now say "no further automatic retry is made" instead of claiming no retry happened.
- Binary downloads repeat only before the artifact writer receives a byte. Once streaming starts, a failure is final and the unfinished writer publishes nothing.
- Journaled mutations and multipart uploads are unchanged: one durable claim, one send, no retry.
- Service OAuth's single reacquisition: a cached token that is not yet due for renewal but whose current-user probe gets HTTP 401 is treated as revoked. The host makes one client-credentials exchange under the same grant lock, verifies every provisioned product with the new token and only then activates a new generation. A second rejection is returned as `AUTH_REQUIRED` with no further exchange and the committed generation untouched. The identity probe marks this case with the issue rule `credential_rejected`; other auth failures keep their existing handling.
- `InvocationControl::unix_millis` exposes the invocation clock's wall time for absolute provider advice. Deadlines remain monotonic.

## Checks

| Check | Result |
| --- | --- |
| `cargo test -p craft-runtime --lib transport::` | 67 passed, including eight new retry cases and the updated body-refusal case |
| `cargo test -p craft-runtime --lib transport::downloads` | 13 passed, including the new pre-body retry case |
| `cargo test -p craft-runtime --lib auth::service` | 29 passed, one native opt-in ignored, including the new reacquisition case |
| `python tools/verify.py` (full scope on the fresh worktree) | Planning, skill, contract, provider and dependency checks, formatting, both Clippy modes and `cargo test --workspace` passed (842 tests passed, nine opt-ins ignored). The composed-host stage then exposed fixtures that assumed one read attempt, listed below |
| All nine composed-host suites with `synthetic-acceptance`, after the fixture updates | 58 + 77 + 9 + 30 + 15 + 7 + 6 + 8 + 10 passed |
| `cargo test -p craft-runtime --lib --features synthetic-acceptance`, `tools/synthetic-acceptance.py`, workspace Clippy with the feature | passed |

New cases and what they pin down:

| Case | Observation |
| --- | --- |
| `safe_reads_repeat_only_retryable_statuses_within_three_sends` | Each of 429/502/503/504 followed by 200 returns the 200 after exactly two requests; three rate-limited replies return the third reply's status, request ID and advice after exactly three requests |
| `non_retryable_statuses_are_sent_once` | 400, 401, 403, 404, 409 and 500 with `Retry-After: 0` each produce one request |
| `advice_beyond_the_cap_or_the_remaining_budget_is_returned_without_waiting` | `Retry-After: 31` under a 60 s budget and `Retry-After: 3` under 3.5 s each return in under 2 s after one request; a past HTTP date retries immediately |
| `connection_failures_before_a_response_are_retried_but_bad_bodies_are_not` | Disconnect then 200 succeeds after two requests; three disconnects fail as `REMOTE_UNAVAILABLE` with `failed` write state and "3 bounded attempts"; an invalid JSON body fails after one request |
| `cancellation_interrupts_a_retry_wait_and_a_short_budget_skips_it` | Cancellation 200 ms into a 2 s wait returns `CANCELLED` within 1.5 s with one request; a 2.9 s budget never starts a 2 s wait |
| `a_context_labelled_as_a_write_never_repeats_its_send` | A 503 to a write-labelled context is sent once |
| `only_sealed_safe_read_descriptors_opt_into_retry` | `LocalRead`, `Never`, `ReconcileBeforeReplay` and a write effect all yield no policy |
| `backoff_doubles_from_the_base_and_never_extends_the_deadline` | 250 ms, 500 ms, then none at the third retry; the 4 s cap; 30 s advice accepted and 31 s refused; a 1.1 s budget refuses any wait |
| `retryable_status_before_streaming_is_repeated_once_and_publishes_one_artifact` | A 503 then a complete entity publishes one confirmed artifact after two requests |
| `rejected_cached_token_is_reacquired_once_and_a_second_rejection_is_final` | 401 on the cached token leads to exactly one more token exchange, five product probes and a new active generation; a second 401 leaves the original generation active after the same single exchange |

Existing cases that already asserted single sends keep passing unchanged: `failed_statuses_return_only_status_advice_and_request_identity_without_retry` (advice of 9 s exceeds its 2 s budget), the stalled-request deadline and cancellation case, every journaled mutation and multipart case, and the download cases for cancelled or expired in-flight streams. The TLS refusal case first failed because certificate failures were being retried; the design was tightened so connect-phase failures are final, and the case passes unchanged.

## Host fixtures updated for the new contract

Ten composed-host cases encoded the old rule that a safe read is never repeated. None was weakened: each now states the bounded behavior.

- Rate-limit fixtures in recovery and policy-revocation reads (`confluence_context_host`, `jira_create_host` recovery and link lifecycle, `jira_issue_mutation_host` recovery) used `Retry-After: 7` under a 30 s budget, which the new policy waits on. They now use 60 s, above the cap, so the case still proves that rate advice is returned without a wait or a second request.
- Disconnect fixtures in project list, project context, field reads and issue search now supply three dropped connections and assert five product requests (two identity probes plus three attempts), still with no page, cursor or successor published.
- `disconnected_project_read_does_not_retry_or_publish_continuation` is renamed `disconnected_project_read_stops_after_bounded_attempts_without_continuation`.

The dependency checker reported inventory drift on this worktree only because Git checked `Cargo.lock` and manifests out with CRLF line endings; regenerating changed only hashes. The committed inventory is left as it was.

## Limits

Live provider rate limiting was not induced; the behavior rests on the loopback TLS fixtures above. macOS was not run. Delegated refresh-token rotation is out of scope here and remains with `refresh-recovery`.
