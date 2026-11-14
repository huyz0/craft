# Internal Jira create host

`craft-cli::issue_create::host::PreparedCreate` composes service access, metadata qualification, a journaled JSON send and evidence-preserving completion. It has no public parser/catalog registration. This contributor API does not provide a runnable cloud command.

## Preparation and authority

The [candidate command adapter](create-command-policy.md) now resolves actual profile/project context and a revisioned private policy before constructing this host. It uses dynamic policy checks before acquisition/reads and at claimed send; static library callers remain responsible for their own trusted policy lifecycle.

`prepare` accepts a canonical request, explicit operation ID/apply, request ID and trusted `CreatePolicy`. The policy enables one exact Jira Cloud project, grant and expected actor. It must come from the host's policy configuration, not repository instructions or remote content.

Preparation is local. It applies core JSON size/depth/container bounds, strict typed input and basic semantic checks, rejects cursors and unsupported preconditions, requires explicit apply and enabled matching policy, and preflights selected recovery output before credentials or state are opened. Numeric `issue_type_id` is required, participates in the logical mutation digest and must match fetched metadata. There is no default issue type. The private registry declares fixed routes and scopes; its generic create handler refuses execution.

## Service access and execution

`VerifiedServiceAccess` retains the sealed token, exact product identity, grant, actor and reported scopes. It exposes no bearer bytes or serialization. Before each prepared operation, it verifies the unchanged descriptor, exact tenant, a nonempty satisfied scope alternative and dual-clock token freshness. Scopes do not prove actual project permissions; Jira can still refuse a resource request.

The host checks all required scopes before resource reads, including optional account lookup. It derives `MetadataPrincipal` from verified access. Existing metadata and parent/account helpers produce the [frozen body](jira-create-metadata.md). Logical operation identity and exact native-request identity remain separate bindings.

Existing journal state is inspected before resource reads. Changed bindings, foreign steps and previously started/terminal creates are refused. Matching prepared state can proceed to the authoritative atomic native-request claim. This early read does not resolve concurrent claim races. Matching stored positive evidence survives refusal of completed work; a missing record never authorizes replay.

`execute_cached` requires the selected `LockedCredentials` and keeps the grant lock borrowed through acquisition, reads, send and completion. Wrong-grant or already-started work is refused before acquisition. It uses the existing fresh-token identity recheck/stale-token acquisition protocol. `execute` accepts an already verified capability for hosts that manage acquisition separately; those hosts remain responsible for lifecycle and locking policy.

Runtime freezes a fixed-origin request and consumes it only after the durable exact request claim. The final callback rechecks service access and frozen metadata/account freshness after any claim wait, before polling HTTP. It neither refreshes nor retries there. Failure after claiming retains conservative uncertainty through [completion](jira-create-results.md). Journal waits use current invocation control.

## Separate effects in output

Output always retains `credential_cache_update` under projection. It is null when no update evidence was supplied, or carries the affected grant, local write state and nullable cleanup disposition. Confirmed activation has a known cleanup boolean; an uncertain write has unknown cleanup. These are non-secret receipts.

Auth/metadata failures before issue submission report the issue as `not_sent`, separately from any confirmed or uncertain cache update. OAuth issuance uncertainty does not imply issue creation. Claimed sends use their own resource interpretation. Known IDs, journal evidence and cache receipts survive later persistence/output failure. Insufficient ordinary recovery budgets are rejected before acquisition; post-effect output preserves evidence even if unexpectedly oversized.

## Qualification boundary

The [initial acceptance record](evidence/jira-create-host.md) distinguishes the original layer tests. Subsequent [composed acceptance](composed-host-acceptance.md) exercises the actual complete cached host against owned TLS servers, using memory credentials, native locks and SQLite. Public dispatch, reconciliation, independent consequential review, live qualification, broader CRUD and unsupported field families remain open.
