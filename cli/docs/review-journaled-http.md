# Review journal-bound HTTP execution

Status: **candidate host integration for independent consequential review, 10 November 2026**. No public command/provider mutation is enabled. The implementing agent performed source and fixture verification; that is not independent approval.

## Review scope

Read the [recovery scenarios](../openspec/changes/build-cloud-cli/specs/operation-recovery/spec.md), [local mutation design](../openspec/changes/build-cloud-cli/design.md#local-mutation-journal), [host contract](journaled-http.md) and [evidence](evidence/journaled-http.md). The changed production paths are:

- `crates/craft-runtime/src/transport/mutations.rs`: frozen request, consuming send, claim acquisition, unknown recording and retained response/persistence results.
- `crates/craft-runtime/src/transport/mod.rs`: common private session/client construction and shared one-attempt exchange, preserving read-context refusal of mutations.
- `crates/craft-runtime/src/journal/mod.rs`: a private shared-state clone using the HTTP invocation control. A test-only fault helper exposes the existing injected commit boundaries to integration fixtures.

No core contract, public registry/schema, dependency or native platform implementation changed. Unit fixtures use private localhost routing unavailable through the library API. Tokens, request body sentinels and provider error bodies are synthetic.

## Questions to resolve before activation

1. Does the private request remain exactly the call bound by the committed claim, with no alternate authenticated send path exposed to providers?
2. Can refusal, cancellation, deadline, a nested runtime or uncertain marker acknowledgment produce a network attempt without confirmed claim acquisition? Inspect the same-invocation journal control and the last control check before polling.
3. Do every response, disconnect, redirect, parse/size failure and persistence failure retain uncertainty without silently classifying success/failure or repeating HTTP?
4. Can a known native response or qualified ID/job receipt disappear when local recording fails, the provider parser fails, or the CLI envelope is too small? The library retains evidence, but provider/CLI composition remains required.
5. Are operation/step identities stable host code and actor/grant/session/policy verification enforced by the eventual host, rather than supplied as agent-controlled authority?
6. Does the integration preserve existing journal monotonicity and known effects under concurrent outcomes? Confirm the limits of single-installation coordination and the absence of remote idempotency or cross-machine fencing.

## Activation prerequisites

The later [Jira create-result composition](jira-create-results.md) adds a pure provider response crate, internal CLI completion/preflight and non-secret attempt metadata accessors. Its [separate evidence](evidence/jira-create-results.md) covers partial effects and receipt-preserving output. Include those provider/host paths in a consequential review before enabling a public create; the earlier generic transport checkpoint alone does not qualify them.

The subsequent [metadata/frozen-body integration](jira-create-metadata.md) adds a `send_checked` host callback after the durable claim and before HTTP polling. Review that the actual host wires frozen-context and operation-policy checks there, with invocation control checked before and after the callback; the generic `send` method does not invent those operation-specific checks.

The [parent/assignee extension](jira-parent-assignee.md) adds exact visible parent qualification and a separately scoped positive active-account proof. Review target/principal/account/parent binding, required/default/null handling, lack of alias/unassignment fallback and both retained freshness clocks at the actual send boundary. It does not qualify reporter/security fields or replace authenticated host policy.

Independent review disposition and any fixes are required. Then qualify provider-specific parsing/status/reconciliation, current authenticated actor/access/freshness and scope policy, output preflight/recovery envelopes and live owned sandbox operations. Binary uploads and shared authority need their own integration evidence. Neither this review package nor a passing synthetic gate authorizes a live mutation, deployment or public cloud activation.
