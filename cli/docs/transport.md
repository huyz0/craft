# Trusted JSON transport

The runtime implements fixed-origin Atlassian Cloud JSON reads and journal-bound mutations. Qualified public service commands include selected Jira issue/link and Confluence page operations; see [current availability](cloud-quickstart.md). Native TLS fixtures establish local behavior on Windows. Actual tenant qualification, broker routing, binary transfer and other-platform acceptance remain incomplete.

[Journal-bound mutation transport](journaled-http.md) consumes exact durable send claims and retains response/persistence evidence. Generic HTTP contexts stay private; each public write uses a separately reviewed operation-specific host.

## Construct and bind the context

The trusted host constructs `HostSession` from a tenant, grant and redacted `SecretBytes`. This validates local binding and bearer encoding; the auth/site layers must separately establish actual access and identity. Provider handlers receive only `OperationContext` with typed route calls, checkpoints and continuation normalization. They receive no session, raw URL, HTTP client or auth header.

`HttpContext::for_operation` checks the current registry descriptor against the sealed prepared operation, matches the host session to its tenant and selected grant, then creates a private client/runtime. Registry execution verifies the context's operation digest before invoking a handler, preventing reuse with a different prepared request. This port is for synchronous host execution; construction/invocation inside an existing Tokio runtime is refused.

The initial endpoint templates are fixed:

| Provider/deployment | Base |
| --- | --- |
| Jira Cloud | `https://api.atlassian.com/ex/jira/{cloud_id}/` |
| Confluence Cloud | `https://api.atlassian.com/ex/confluence/{cloud_id}/` |

Cloud IDs must have UUID syntax. Declared literal paths are appended under the selected base. IDs are encoded once as individual components; dot traversal, separators and already encoded identities are refused. Query names must be declared and values are encoded as data. Credential attachment occurs after URL validation. No public raw-origin override exists. The fixture's localhost override is confined to test code.

Registry registration refuses PUT/PATCH/DELETE routes on read descriptors. POST-based searches remain possible, so compiled endpoint declarations still require operation-specific semantic qualification. The ordinary provider-facing context refuses mutations. The separate candidate host path requires an exact committed journal claim before polling its prepared exchange; public provider/CLI write integration and recovery remain unfinished. Synthetic mutation-disconnect tests do not enable a cloud write command.

## TLS, bounds and failure behavior

The async reqwest client uses Rustls/AWS-LC and platform certificate verification, with TLS 1.2 or later. `TlsPolicy::AdditionalCaBundle` can supplement native trust from trusted host configuration. A bundle is bounded to 1 MiB and 32 certificates. Certificate and hostname checks remain mandatory; no verification bypass or repository-selected trust policy is exposed.

Redirects, automatic protocol retries, proxies, referers, cookies, compression, TLS key logging and verbose connection diagnostics are disabled. Requests explicitly select identity encoding. The fixture suite tests inherited proxy variables in a fresh subprocess and observes zero connections to the proxy sink. Each ordinary transport call attempts one request; operation-specific retry policy remains separate work.

| Boundary | Limit or behavior |
| --- | --- |
| Bearer token | Nonempty RFC-compatible ASCII bearer grammar with trailing padding only, at most 16,384 bytes; Debug is redacted and the header is marked sensitive. |
| Constructed URL | At most 32,768 bytes. |
| Query | Declared keys only, at most 32 distinct supplied names and 256 total pairs; each value at most 16,384 UTF-8 bytes. Declared GET arrays contain 1 to 100 values per name. |
| JSON request | At most 1 MiB serialized, validated before sending. |
| JSON response | Streamed to the route's declared 512-byte to 16-MiB budget, including chunked responses. Strict UTF-8, duplicate-key, depth and container checks apply. |
| Response types | Valid JSON values, including arrays; empty 201/204/205 becomes null, including a Created response without Content-Type; nonempty entities retain JSON checks. Request argument objects still require object roots. |
| Native error status | Body is omitted. Only status, a bounded whitelisted request ID and typed Retry-After advice reach the provider port. |
| Cancellation/deadline | Remaining invocation budget bounds the request; a periodic control check can cancel pending I/O. A completed result takes precedence over a late cancellation. |

Transport failures use static errors without a raw URL, token or provider body. Read failures report failed state. Fixture mutation failures retain possible send uncertainty, without replay. Header values and HTTP library buffers are not claimed to be universally zeroized; the original secret wrapper and temporary header assembly buffer are zeroized, and Debug/logging paths are restricted.

## Normalize continuation as data

`NativeRoute.continuation_parameters` must be a unique subset of its declared query names. GET next links must retain the same origin and exact constructed route path. Credentials, fragments, duplicate decoded scalar/paging keys, undeclared parameters and invalid percent/UTF-8 encoding fail. Existing semantic filters are preserved; a next link cannot change them. Only declared paging parameters may change.

`NativeRoute.repeated_query_parameters` explicitly declares GET array names as a unique subset of query names, disjoint from continuation names. `NativeCall.repeated_query` carries a nonempty bounded vector for each supplied array, including single-item arrays. Scalar/array collisions and scalar representation of declared arrays fail before connection. Each item is independently URL-encoded; order, duplicates, commas, empty string items and numeric strings remain data. Adapters must separately enforce native types and semantic uniqueness. Empty declarations are omitted during serialization, preserving existing scalar route descriptors and their operation digests.

An advertised next link may omit a bound array, in which case the original is inherited. If it includes that name, its decoded values and order must exactly match the original vector. Partial, reordered, changed or newly added arrays fail. Both parsed pair counts and the final reconstructed URL are bounded. The Jira project-site parser continues to accept only its original scalar route contract. Repeated declarations are GET-only, while mutation preparation and journal claims reject GET; this addition does not change mutation request digests or recovery.

A separate `JiraFieldSite` constraint accepts only the configured site's exact field-search path and compiled scalar/array declarations. The host enables it only for a sealed read with that single route. It normalizes site links without changing the authenticated endpoint or project-site authority. [Field-read qualification](jira-field-reads.md) covers advertised page-size clamping, bound filters and actual gateway requests.

Normalization returns a typed `NativeCall`; it never fetches the supplied raw URL. The core checked context validates the normalized call again. Cursor persistence, no-progress detection and provider page semantics retain their separate implementation tasks. Delta-seconds and HTTP-date Retry-After values are retained as typed metadata, without choosing a retry automatically.

## Verify and complete the remaining scope

From `cli/`, with native loopback access:

```text
cargo test -p craft-runtime --lib transport::tests --locked --offline
python tools/verify.py
```

The suite uses generated fixture CAs and synthetic tokens. It does not install trust roots or call a real provider. [Observed evidence](evidence/trusted-http.md) records exact results and fixture corrections. [Repeated-query acceptance](evidence/repeated-query-transport.md) covers array encoding, filter binding and bounds through registered operations and owned TLS fixtures.

Broker origins/auth exchanges, live tenant/session qualification, binary transport, operation journals, safe retries/async jobs and other OS builds remain required. Windows platform-verifier behavior is qualified only by the exercised certificate/hostname cases. Pathological native certificate-chain stalls, other platform trust stores and cancellation inside synchronous platform-verifier work remain unqualified. Required independent auth/write review has not occurred; no credential-facing cloud CLI path is enabled by this foundation.
