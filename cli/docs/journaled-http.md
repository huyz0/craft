# Journal-bound JSON mutations

The runtime has a candidate host-library path for a single declared JSON mutation. It is separate from the ordinary provider-facing read context. No public CLI command or compiled provider handler uses it. Independent consequential review, provider-specific composition and live qualification remain required before public cloud activation.

## Prepare, claim, attempt, interpret

`PreparedHttpMutation::prepare` accepts a registered sealed operation, host session, TLS policy, invocation control and typed native call. It refuses read operations and GET routes. It uses the same descriptor/session checks, fixed Atlassian origin templates, route/path/query/body validation, bearer encoding and TLS client policy as reads. The actual HTTP request and cloned native call remain private. Preparation opens no connection and obtains no journal authority.

`send` consumes that prepared value. Before acquiring a claim it rejects cancellation, a nested Tokio runtime and a grant that differs from the session. Nested-runtime refusal shuts down the owned idle runtime without blocking in the caller's executor. The journal is rebound to the HTTP invocation's control, preserving its shared installation/database and fault state while ensuring lock/commit waits use the same deadline.

The existing `claim_send` path requires explicit apply and binds the operation ID, effective actor, selected grant, descriptor, logical payload/target, stable host-defined step and exact native request digest. It commits the send marker before returning a claim. A refused or uncertain claim never polls the exchange. Reusing a prepared object is prevented by ownership; separately prepared contenders still compete through the journal. Step names must come from trusted operation composition. Changing the step, operation ID or state root is not a retry strategy.

After the claim, `send` rechecks invocation control immediately before polling the one-attempt exchange. Any resulting exchange error retains conservative `sent_unknown` write state and no retry advice. The sanitized error can retain cancellation, deadline or policy codes. The transport never follows redirects or repeats a request. If cancellation occurs between claim acquisition and polling, the marker still requires reconciliation even though no connection was opened in that execution.

The later `send_checked` host variant also runs a trusted operation preflight after claim acquisition and checks invocation control on both sides of that callback. The [Jira metadata context](jira-create-metadata.md) uses it to recheck freshness after a journal wait. A rejected final preflight leaves conservative uncertainty and zero HTTP attempts. The callback is trusted host code, not an agent-supplied expression, authentication proof or generic HTTP interface. Public operation composition must choose the required checks explicitly.

## Keep response and persistence independent

`MutationAttempt` retains two separate results:

| Field | Meaning |
| --- | --- |
| `response` | Bounded native response with status/body/request ID/rate advice, or sanitized exchange error |
| `pending_evidence` | Result of one immediate attempt to record unknown submission state through the acquired claim |

HTTP success does not establish semantic success. A valid 201 or 202 response leaves the step unknown until the trusted provider composition validates its operation-specific response and records the qualified resource/job evidence. Native error statuses remain data too: the error body is omitted, rate metadata is preserved, and no automatic retry or failure classification is inferred.

`record_outcome` on the attempt uses the same claim to append qualified confirmed/failed/unknown evidence. It performs no HTTP request. Existing journal rules preserve confirmed resource IDs, immutable request bindings and monotonic job/submission state. It may make one immediate local persistence attempt after cancellation/deadline without waiting on contention. Failure preserves the caller's supplied evidence and existing durable marker. A missing acknowledgment is resolved by inspecting the journal, not resending.

The host must retain the known response and qualified receipt when persistence or output encoding fails. It must also finish response validation, output-envelope preflight and recovery composition before enabling a public write. This library path does not supply those missing host/provider guarantees. `operation get` can inspect persisted metadata, but it cannot reconstruct a response whose IDs were never recorded.

## Authority and limits

The session constructor validates local encoding/binding. The trusted authentication host must independently verify current actor, access, scope, token freshness, target and operation policy. The journal does not authenticate an actor or implement human review. Explicit apply is necessary but is not a substitute for those checks.

The path supports JSON only. Multipart uploads, binary transfers, provider handlers, operation-specific reconciliation, public recovery envelopes, delegated/broker transport and shared mutation authority remain unfinished. Single-root claims do not prevent writes from another installation, machine, operation identity or bypassing writer. Ordinary `HttpContext::for_operation` still refuses mutations.

See [transport](transport.md), [journal](journal.md), [review package](review-journaled-http.md) and [observed evidence](evidence/journaled-http.md).
