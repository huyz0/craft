# Trusted HTTPS foundation evidence

Observed on 9 November 2026, Windows x86_64 MSVC, pinned Rust 1.99.1. This is partial evidence for trusted HTTP. The task remains unchecked: the implemented context covers Atlassian Cloud read routing and local TLS qualification; broker transport, live auth/provider qualification, other OS/native-verifier cases and consequential review remain incomplete. The full CLI goal remains active.

## Artifacts and contract

`crates/craft-runtime/src/transport/` constructs host-owned tenant/grant/operation-bound HTTPS execution from sealed core preparation. Atlassian origins are compiled templates. Route IDs, components, query names and paging parameters are validated before credential attachment. Only test code can substitute localhost origins. Host sessions use redacted secrets; providers receive the core operation port, not a token, header or raw HTTP client.

The client uses async reqwest on a private runtime to support cancellation behind the synchronous core port. HTTPS/certificate/hostname checks remain enabled, with bounded explicit extra CA roots. Redirects, automatic retries, proxies, compression, cookies, referers and TLS key logging are disabled. JSON is streamed to the route's declared byte bound. Error bodies are omitted, raw library errors are discarded, request IDs are bounded and Retry-After values are typed. Production mutating contexts remain refused pending durable send/recovery integration.

Core additions include strict bounded response-value parsing while preserving object-only requests; typed rate advice; a continuation port; immutable context/operation verification; and explicit paging query declarations. Registry registration refuses read descriptors with PUT/PATCH/DELETE while accepting POST searches. Existing test adapters were updated for the extended native response/route metadata. All source names/comments remain semantic and free of planning identifiers.

## Observed checks

- `cargo test -p craft-runtime --lib transport::tests --locked --offline` with native loopback access passed 13 tests. They cover encoded components and fixed templates; unsafe/foreign/filter-changing continuation; session/grant/descriptor mismatches; mutation preconditions; actual HTTPS headers and bounded JSON arrays; zero-connection preflight/cancellation; wrong CA/hostname refusal with an observed connection and no HTTP credentials; redirect refusal with zero sink connections; malformed, oversized, chunked and disconnected bodies; error-status scrubbing and single-attempt behavior; request-observed cancellation/deadline; fixture mutation uncertainty; and inherited proxy isolation in a fresh subprocess.
- `cargo test -p craft-core --test provider_execution --locked --offline` passed eight cases after adding the read-method fence. The fence rejects PUT/PATCH/DELETE atomically and preserves POST-based search registration. Existing tests retain effect-based POST response semantics and prevent not-sent claims after mutation response failures.
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` passed, including the final full gate after the method fence, strict bearer padding and case-insensitive JSON media-type checks.
- `python tools/verify.py` with native filesystem/loopback access passed 103 named Rust tests, five subprocess controls, planning/inventory checks, formatting and all-target Clippy. The ordinary gate ignored one opt-in native-vault test, whose historical separate pass remains linked below. Output is retained at `target/transport-verification-output.log`. Its generic native/live `not_run` marker does not replace the specific TLS and earlier vault evidence.
- Dependency review/inventory compares 278 packages: five workspace and 273 external across target/dev resolution. All 143 additions used cached registry sources, and no existing version was changed or removed. Production reqwest features are `rustls` and `json` without default features; the Windows CLI's full normal/build dependency tree contains no rcgen fixture dependency. MSVC/CMake compiled AWS-LC successfully. Existing feature changes and new license obligations are recorded in [dependency choices](../dependencies.md).

Current development executable SHA-256: `1b54c5ab6a72ee2d669f66c733cba5735320d0517a7db2a636d4a0dfdb3af2d1`. Native Ctrl-C was not repeated for this binary; prior signal evidence remains historical. No cloud command is inferred from internal transport tests.

Fixtures create temporary CA/leaf keys in memory and use synthetic bearer bytes on loopback. No real provider token, OS credential entry, trusted root store or remote resource is read or changed by these transport tests. The normal gate still leaves the separately opt-in Windows native-vault test ignored; its earlier result is [separate evidence](native-vault.md).

## Failures and corrections

The first fixture input model used a runtime enum without JsonSchema and a non-object request root. It was replaced by a closed typed object fixture, matching the actual request contract. Adding the dev schema dependency required an offline lock refresh before locked checks could run.

Initial TLS fixtures failed before receiving HTTP. Native-access retries established that sandbox access alone did not explain the failures. The accepted Windows socket inherited the listener's nonblocking mode; the fixture now explicitly restores blocking mode. A subsequent Windows certificate error exposed identical default subject names on the CA and leaf. Distinct CA/leaf identities plus an authority key identifier fixed actual validation. Certificate/hostname verification was never disabled. Temporary diagnostics used only synthetic URLs and were removed from final source.

Negative TLS tests now require an observed connection, so an unrelated connection refusal cannot count as certificate validation. Cancellation triggers after an HTTP request is observed. The real-time stalled-response case uses a one-second budget and an under-two-second assertion, rather than assuming a TLS handshake completes within the earlier 150-millisecond fixture budget.

Continuation review found that the URL library's ordinary query decoder could replace invalid UTF-8. Strict percent/UTF-8 decoding now refuses malformed escapes instead of changing an opaque cursor. Review also added the descriptor method fence above. No test was removed or converted into a pass to accommodate these failures. Independent review remains outstanding.

Final review tightened bearer padding to the trailing position required by bearer grammar and tested invalid leading/internal padding. Mixed-case vendor `+JSON` fixture responses verify media-type handling. Requests explicitly select identity encoding, which the wire test observes, while automatic decompression remains disabled. The fixture's changed media type does not weaken invalid-content/body bounds. The final 103-test gate includes these changes.

## Remaining acceptance

The current context is synchronous-host only and rejects nesting inside an existing Tokio runtime. It does not provide a broker-origin/auth flow, binary transfer, durable send marker, production mutation, retry/job policy, provider cursor store or live scope/permission proof. Platform-native verifier work can be synchronous; pathological chain/network stalls and cancellation inside that work are not qualified by the local fixtures. Linux/macOS compilation/native TLS and independent consequential auth/write review remain unrun. The full task and goal remain incomplete.

The later [journal-bound JSON mutation checkpoint](journaled-http.md) composes existing durable claims with a separate consuming host request and retained response/persistence evidence. This supersedes the absence of a host send-marker composition, while the ordinary provider-facing context still refuses mutations. Public provider/CLI writes, reviewed operation-specific recovery, live qualification and the other remaining acceptance above are unfinished.

## Closure

On 15 November 2026 the owning ledger task was closed. The remaining gaps listed above are answered in [foundation closure](foundation-closure.md), with platform runs in [platform qualification](platform-qualification.md) and review in [foundation review](foundation-review.md).
