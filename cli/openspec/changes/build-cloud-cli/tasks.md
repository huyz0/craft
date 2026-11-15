# Craft implementation tasks

This is the authoritative implementation ledger for the [current OpenSpec change](proposal.md). Every task is initially planned, not implemented. Read one semantic task through `python tools/plan.py show <name>` from `cli/`; use `next` for dependency-ready candidates. Task names and checkboxes are planning metadata only. Put no milestone/task identifiers in implementation artifacts or commit subjects. Follow the [coding standard](../../../docs/coding-standard.md).

A task is complete only when its own acceptance, dependencies and linked evidence hold. Evidence is `none` until an actual record under `cli/docs/evidence/` is supplied. A task marked ready only has completed dependencies; it is not proof of credentials, authorization, platform resources or a safe live environment. Split or revise work when evidence requires it; do not shrink acceptance to obtain a pass.

## Rust foundation

### cli-bootstrap

- [x] Deliver this task.

**Goal:** Create a real, minimal Rust workspace rooted in cli/ with a runnable Craft binary and no product-support claims.

**Spec:** delivery-quality / Lightweight Rust workspace

**Depends on:** none

**Build:** Create workspace, thin craft-cli binary, craft-core library, committed lockfile and semantic smoke test. Pin a tested stable toolchain; document commands in cli/README.md. Add runtime/provider crates only as their tasks need them.

**Verify:** Run cargo metadata, cargo build --locked and cargo test --workspace --locked from cli/; invoke the built executable with --version and --help.

**Done when:** A fresh supported development host can build the exact workspace; version identifies Craft and no milestone/task labels appear in source or comments.

**Evidence:** docs/evidence/workspace-bootstrap.md

### dependency-baseline

- [x] Deliver this task.

**Goal:** Make dependency and platform choices explicit before implementation grows.

**Spec:** delivery-quality / Lightweight Rust workspace

**Depends on:** cli-bootstrap

**Build:** Record exact Rust/MSRV, TLS and JSON Schema choices, explicit keyring backend candidates, SQLite/native prerequisites, features, licenses and dependency rationale. Keep Python confined to documentation tooling.

**Verify:** Resolve and build locked dependencies on the intended development host; inspect normal/build/dev dependency trees and validate license/source inventory.

**Done when:** No insecure/mock vault is a production default; selected versions are reproducible and unverified OS prerequisites remain labeled.

**Evidence:** docs/evidence/dependency-baseline.md

### core-request-model

- [x] Deliver this task.

**Goal:** Define provider-independent, serializable contracts without hidden I/O.

**Spec:** agent-interface / Strict request validation

**Depends on:** cli-bootstrap

**Build:** Add typed resource/provider/tenant/grant references, effect classes, canonical requests, preconditions, errors and separate request/job outcomes in craft-core. Model omitted versus null explicitly.

**Verify:** Pure tests cover equality/canonicalization, invalid types and outcome serialization; dependency inspection proves core does not import runtime/providers/workflows.

**Done when:** Contracts represent unknown, partial and accepted states without boolean shortcuts or provider-specific global assumptions.

**Evidence:** docs/evidence/core-contracts.md

### operation-registry

- [x] Deliver this task.

**Goal:** Give every supported operation one reviewed registration and descriptor.

**Spec:** cloud-providers / Isolated provider modules

**Depends on:** core-request-model, dependency-baseline

**Build:** Define provider/operation traits, immutable descriptor registration, scope alternatives, tenant conditions, schema references and operation-specific retry/reconcile metadata. Reject duplicate command IDs; add a synthetic provider for tests.

**Verify:** Compile a minimal provider against core ports, reject duplicate registrations and validate descriptors without live credentials.

**Done when:** Adding a compiled provider does not require changing core dispatch or exposing arbitrary authenticated HTTP.

**Evidence:** docs/evidence/provider-registry.md

### fixture-testkit

- [x] Deliver this task.

**Goal:** Make negative paths observable without credentials or live mutations.

**Spec:** delivery-quality / Behavioral and platform qualification

**Depends on:** cli-bootstrap

**Build:** Create dev-only HTTP recorder, fake clock/vault, failpoint journal/artifact adapters and subprocess harness. Support request counts, disconnect-after-send, 429, malformed bodies and cancellation.

**Verify:** Run the fixture harness including deliberate unexpected requests and delayed responses; inspect Cargo trees to keep test support dev-only.

**Done when:** Tests can prove zero requests, exact effects and unknown send outcomes rather than trust a mocked success value.

**Evidence:** docs/evidence/test-fixtures.md

### strict-json-input

- [x] Deliver this task.

**Goal:** Reject plausible but wrong agent input before any remote or auth side effect.

**Spec:** agent-interface / Strict request validation

**Depends on:** core-request-model, fixture-testkit

**Build:** Implement bounded UTF-8 parsing with duplicate-key detection, recursive closed schemas, discriminator/type/range checks, file/stdin limits and explicit flag/file conflict handling. Add metadata-constrained custom-field maps.

**Verify:** Subprocess cases cover duplicate/unknown keys, trailing JSON, deep/oversized input, wrong booleans/IDs and valid Unicode/multiline code; fixture request count remains zero on local failures.

**Done when:** No silent coercion, key dropping or target correction; errors identify exact bounded field paths.

**Evidence:** docs/evidence/strict-input.md

### cli-envelopes

- [x] Deliver this task.

**Goal:** Make agent invocation deterministic across success, error and interruption.

**Spec:** agent-interface / Deterministic invocation

**Depends on:** strict-json-input, core-request-model

**Build:** Implement one JSON stdout envelope, stable exits, secret-free stderr, concise help exception, error/write-state mapping and broken-pipe handling. Normal commands cannot prompt, page or open a browser.

**Verify:** Exercise subprocess stdin/stdout/stderr/exits, invalid commands, cancellation and closed pipes; validate every response against its declared schema.

**Done when:** No malformed/partial JSON or raw panic/provider trace reaches normal agent output; confirmed versus unknown effects remain explicit.

**Evidence:** docs/evidence/invocation-control.md

### contract-discovery

- [x] Deliver this task.

**Goal:** Let an agent discover the smallest useful input or output contract.

**Spec:** agent-interface / Discoverable contracts

**Depends on:** operation-registry, cli-envelopes

**Build:** Implement command find, filtered capabilities scaffolding, help --json and schema compact/property/full/example views, including --response and selected-field schemas. Add version/digest caching metadata.

**Verify:** Compare parser-accepted flags and generated schemas; validate full examples, bundled references, pointer errors and mandatory control fields under projection.

**Done when:** Only implemented commands are discoverable and compact schema views explicitly disclose omitted structure.

**Evidence:** docs/evidence/contract-discovery.md

### profile-resolution

- [x] Deliver this task.

**Goal:** Resolve identity and target without accidental cross-site operation.

**Spec:** secure-auth / Explicit profiles and grants

**Depends on:** core-request-model, cli-envelopes

**Build:** Add local profile list/show/create/use, explicit provider/deployment/tenant and shared-grant metadata, trusted config precedence and a single private state root. Separate repository hints from auth/backend/reviewer trust configuration. Live sites list/select/verify is retained in site-discovery because it requires the later auth/transport implementations.

**Verify:** Test multiple sites, wrong tenant/resource kind, malicious repository hints, malformed config, shared-grant profiles and isolated temporary homes.

**Done when:** Ambiguity and invalid security configuration fail explicitly rather than choosing the first site or silently defaulting.

**Evidence:** docs/evidence/profiles.md

### site-discovery

- [ ] Deliver this task.

**Goal:** Discover and verify actual accessible sites without treating configured metadata as live authority.

**Spec:** secure-auth / Explicit profiles and grants

**Depends on:** profile-resolution, service-oauth, delegated-login, trusted-http

**Build:** Implement sites list/select/verify with service/delegated strategies, exact product/cloud-ID binding, bounded discovery snapshots and explicit tenant selection. Revalidate the chosen site under the selected grant; configured intent alone is not verified access.

**Verify:** Fixture both auth modes, multiple/empty accessible sites, revoked access, wrong product/cloud ID, changed grants and stale snapshots; run live checks only with configured sandbox authority.

**Done when:** Site selection never chooses the first accessible tenant and verification reflects actual mode-specific evidence with unknown/unavailable states preserved.

**Evidence:** none

### descriptor-drift-checks

- [x] Deliver this task.

**Goal:** Keep schemas, parsing, projection and provider registrations in agreement.

**Spec:** agent-interface / Discoverable contracts

**Depends on:** contract-discovery, fixture-testkit

**Build:** Add semantic contract generation/check commands, reviewed schema snapshots and descriptor completeness checks. Detect unsupported advertised flags, duplicate commands, missing scope/recovery metadata and accidental breaking changes.

**Verify:** Deliberately change a parser flag, response type and selectable field without updating the descriptor; each drift control fails and valid regeneration restores agreement.

**Done when:** One source generates consistent public contracts; no schema advertises a no-op option.

**Evidence:** docs/evidence/contract-drift.md

### contributor-quality-guards

- [x] Deliver this task.

**Goal:** Enforce planning/code separation and review comment quality during development.

**Spec:** delivery-quality / Lightweight Rust workspace

**Depends on:** cli-bootstrap

**Build:** Wire coding-standard checks into development verification, including implementation-label scanning, dependency direction and targeted linting. Add a reviewer prompt/checklist for behavior/design comments; do not pretend a regex validates prose accuracy.

**Verify:** Run a temporary artificial-label negative control outside production files, core-layer violation control and review a semantic comment change against its design.

**Done when:** Code/tests/configuration and commit subjects use semantic names; comments explain actual behavior/invariants and remain free of delivery status.

**Evidence:** docs/evidence/local-verification.md

## Authentication and vaults

### tenant-auth-inventory

- [x] Deliver this task.

**Goal:** Establish deployment inputs needed for honest auth and permission support.

**Spec:** secure-auth / Explicit profiles and grants

**Depends on:** profile-resolution

**Build:** Record explicit Cloud IDs, sandbox projects/spaces, supported auth modes, service-account eligibility, requested capability bundles and independent reviewer policy. Keep secrets out of the inventory.

**Verify:** Verify admin-provided eligibility and consented read access using an authorized setup environment; mark unavailable information unknown.

**Done when:** No guessed site, account, scope or reviewer is used to enable a live mutation.

**Evidence:** docs/evidence/tenant-auth-inventory.md

### native-vault-backends

- [x] Deliver this task.

**Goal:** Select and expose real persistent OS credential stores without fallback.

**Spec:** secure-auth / Native credential storage

**Depends on:** dependency-baseline, profile-resolution, fixture-testkit

**Build:** Implement explicit Windows Credential Manager, macOS Keychain and persistent Linux Secret Service adapters with principal/session health reporting. Use typed redacted secret wrappers and a test-only fake backend.

**Verify:** Run adapter contract tests and at least the development host's real write/read/delete test; test locked/unavailable behavior without implicit prompts.

**Done when:** Backend selection is explicit; unsupported hosts fail honestly and neither files nor mock/session-only stores become production fallbacks.

**Evidence:** docs/evidence/native-vault.md

### credential-generation-store

- [x] Deliver this task.

**Goal:** Persist variable-size credentials safely across interruption and backend limits.

**Spec:** secure-auth / Native credential storage

**Depends on:** native-vault-backends

**Build:** Implement bounded keyring-only chunks/generations, length/digest verification, active-pointer activation and obsolete/orphan cleanup. Keep secrets out of journal/config and zeroize where justified.

**Verify:** Inject failures at each chunk/pointer/cleanup boundary and exercise large/Unicode/binary values plus native restart persistence.

**Done when:** Readers reject mixed/incomplete generations and no token is truncated or activated before its complete durable representation.

**Evidence:** docs/evidence/credential-generations.md

### grant-locking

- [x] Deliver this task.

**Goal:** Serialize refresh and logout by actual credential grant across local processes.

**Spec:** secure-auth / Safe credential lifecycle

**Depends on:** credential-generation-store, profile-resolution

**Build:** Add cross-process grant locks, deadlines, locked rereads and shared-profile ownership; define multi-machine ownership restrictions rather than copying rotating refresh chains.

**Verify:** Race two subprocesses refreshing one grant, unrelated grants and logout; kill a lock holder and inspect bounded recovery.

**Done when:** One grant has one local refresh owner and unrelated identities are not blocked or deleted.

**Evidence:** docs/evidence/credential-generations.md

### service-oauth

- [x] Deliver this task.

**Goal:** Acquire unattended Atlassian sessions through eligible service credentials.

**Spec:** secure-auth / Service OAuth sessions

**Depends on:** tenant-auth-inventory, grant-locking, trusted-http

**Build:** Implement secure prompt/pipe provisioning, explicit client-credentials login, per-product access verification, expiry-based reacquisition and scope reporting. Reject secret values in argv. Provide explicit-profile local permission setup adapters for the shared-page and ticket workflow.

**Verify:** Fixture tests inspect exchanges and redaction; candidate flags/schema/dispatch preserve complete results and stdin ownership without public exposure; native service bundles survive fresh processes, competing renewal and logout; authorized sandbox acquisition proves both Jira and Confluence access for the supported mode.

**Done when:** Service OAuth uses its own lifecycle and eligibility evidence; it does not run delegated refresh or expose credentials.

**Evidence:** docs/evidence/service-auth.md

### broker-protocol

- [ ] Deliver this task.

**Goal:** Specify a secure delegated-login trust boundary before deploying a broker.

**Spec:** secure-auth / Delegated OAuth broker

**Depends on:** tenant-auth-inventory, core-request-model

**Build:** Write protocol/threat design for organization/installation identity, transaction proof, registered callback/state, single-use token handoff, cleanup, authenticated refresh and host allowlists. Choose hosting/config independently of provider pages.

**Verify:** Walk replay, swapped users/installations, callback mismatch, stolen browser URL, expiry and broker outage against protocol scenarios and independent security review.

**Done when:** Every token redemption is bound to the intended CLI and actor; no embedded app secret or anonymous exchange path remains.

**Evidence:** none

### broker-service

- [ ] Deliver this task.

**Goal:** Implement the confidential broker separately from the distributed CLI.

**Spec:** secure-auth / Delegated OAuth broker

**Depends on:** broker-protocol, fixture-testkit, dependency-baseline

**Build:** Build a separately configured Rust service with organization auth, transaction TTL/single redemption, server-side app-secret access, callback checks, redacted telemetry and token-handoff cleanup. Add the broker origin to the CLI's trusted host allowlist under the existing typed-route and credential-attachment rules. Deployment remains separately authorized.

**Verify:** Service integration tests cover concurrent redemption, invalid state/proof, expiry, logging/proxy redaction and interrupted handoff.

**Done when:** Broker behavior matches the reviewed protocol and its tests do not require distributing the Atlassian app secret to an agent.

**Evidence:** none

### delegated-login

- [ ] Deliver this task.

**Goal:** Connect explicit human OAuth authorization to local native token persistence.

**Spec:** secure-auth / Delegated OAuth broker

**Depends on:** broker-service, credential-generation-store, trusted-http

**Build:** Implement interactive login/no-browser URL modes through a bound broker transaction, token persistence acknowledgment and product/site validation. Normal resource calls return auth-required rather than launching login.

**Verify:** Test cancelled/denied/swapped login and vault-write failure; perform a consented sandbox handoff with independently configured callback.

**Done when:** Only the initiating installation can redeem the session and failure leaves no reusable browser-side token material.

**Evidence:** none

### refresh-recovery

- [ ] Deliver this task.

**Goal:** Handle rotation and uncertain exchanges without corrupting grants.

**Spec:** secure-auth / Safe credential lifecycle

**Depends on:** delegated-login, grant-locking, retry-deadlines

**Build:** Implement refresh margin, locked reread, one exchange, durable generation activation and bounded invalid-grant/lost-response recovery through the broker. Preserve app/site/scope binding.

**Verify:** Race refreshers and inject loss after provider rotation or before local persistence; verify no unconditional exchange replay or secret-bearing diagnostics.

**Done when:** Expired/revoked/uncertain grants return actionable typed outcomes and cannot silently switch identity.

**Evidence:** none

### auth-lifecycle

- [ ] Deliver this task.

**Goal:** Expose secure auth status, diagnosis, refresh, logout and revocation guidance.

**Spec:** secure-auth / Safe credential lifecycle

**Depends on:** service-oauth, refresh-recovery

**Build:** Implement auth status with optional remote verification, doctor without forced unlock, explicit refresh, idempotent logout across all generations and mode-specific revoke-info. Report affected shared profiles.

**Verify:** Capture all outputs with sentinel secrets; test revocation, locked stores, repeated logout, failed cleanup and logout racing with refresh.

**Done when:** Local removal is distinguished from remote revocation and no racing process resurrects logged-out credentials.

**Evidence:** docs/evidence/auth-diagnostics.md

## Transport, artifacts and operation state

### trusted-http

- [x] Deliver this task.

**Goal:** Centralize authenticated routing and prevent provider data redefining trust.

**Spec:** operation-recovery / Bounded transport and async jobs

**Depends on:** operation-registry, profile-resolution, fixture-testkit

**Build:** Implement allowlisted provider host templates, typed route components, TLS/enterprise CA policy and auth attachment through host-owned ports. Normalize next links within provider/tenant context. Broker origins join the same allowlist when `broker-service` defines them; until then no broker origin exists to route to.

**Verify:** Test query/fragment/encoded-ID abuse, cross-host redirects, malicious next links and wrong cloud IDs; inspect credential headers and URL logs.

**Done when:** Credentials reach only intended validated endpoints; provider modules cannot use raw-token or arbitrary-host escape paths.

**Evidence:** docs/evidence/repeated-query-transport.md

### retry-deadlines

- [x] Deliver this task.

**Goal:** Bound retries, rate limits and cancellation by operation semantics.

**Spec:** operation-recovery / Bounded transport and async jobs

**Depends on:** trusted-http

**Build:** Implement total deadlines, cancellable bounded backoff, Retry-After/reason mapping and one appropriate auth refresh/reacquisition path. Distinguish safe reads from uncertain mutations and invalid-grant exchanges.

**Verify:** Use fake clock and HTTP faults for 429/401/403/5xx/timeouts, chatty responses and cancellation; assert exact request counts and no retry storm.

**Done when:** Write retries require declared safe semantics; deadlines bound the whole invocation rather than resetting on each packet.

**Evidence:** docs/evidence/retry-deadlines.md

### artifact-store

- [x] Deliver this task.

**Goal:** Read/write non-secret artifacts without path escape or accidental replacement.

**Spec:** agent-interface / Portable artifact I/O

**Depends on:** profile-resolution, cli-envelopes, fixture-testkit

**Build:** Create OS-aware trusted input/output roots, safe open/create, atomic finalization, overwrite policy, metadata/digest receipts and cleanup. Reject traversal, symlink/junction escape and unbounded input.

**Verify:** Run temporary-root traversal/link/race boundary cases, permissions checks, interrupted writes and existing-destination protection on available platforms.

**Done when:** Artifact paths never come from untrusted content as authority; receipts identify complete versus incomplete local files.

**Evidence:** docs/evidence/artifact-store.md

### binary-transfer

- [x] Deliver this task.

**Goal:** Stream attachments through bounded secure transport and artifact receipts.

**Spec:** agent-interface / Portable artifact I/O

**Depends on:** artifact-store, trusted-http, retry-deadlines

**Build:** Implement bounded multipart upload/file streaming and binary download with media type, size/digest, atomic output and provider-specific authenticated/signed redirect handling.

**Verify:** Exercise oversized/interrupted streams, malicious filenames/hosts, digest mismatch, stdout purity and cancellation; inspect upload retry counts.

**Done when:** No binary bytes mix with JSON, no credential crosses an untrusted redirect and uncertain uploads remain reconcilable.

**Evidence:** docs/evidence/multipart-upload-transport.md

### mutation-journal

- [x] Deliver this task.

**Goal:** Make write intent and observed effects durable before relying on retries.

**Spec:** operation-recovery / Durable mutation identity

**Depends on:** core-request-model, profile-resolution, dependency-baseline, fixture-testkit

**Build:** Implement transactional SQLite schema/migration for operation/payload/target/grant bindings, prepared/sent/confirmed/failed/unknown requests and per-step remote IDs. Require apply and operation ID before resource writes.

**Verify:** Crash/fault tests at pre-send/post-send/receipt boundaries, changed-ID replay, migration and concurrent local attempts; inspect persisted records for secrets.

**Done when:** No send occurs without durable intent; confirmed mappings survive restarts and changed operation payloads are refused.

**Evidence:** docs/evidence/mutation-journal.md

### provider-reconciliation

- [x] Deliver this task.

**Goal:** Define explicit unknown/partial recovery with provider-specific strategies.

**Spec:** operation-recovery / Uncertain and partial outcomes

**Depends on:** mutation-journal, retry-deadlines, operation-registry

**Build:** Implement reconcile/list/get operations, correlation evidence and strategy hooks for known IDs/async handles/unknown creates. Report unresolved ambiguity; do not equate empty search with no object.

**Verify:** Fault double creates then drops the response; reconcile discovers a known object or returns unresolved and never sends a blind duplicate create.

**Done when:** Recovery records confirmed effects and uncertainty, without claiming exactly-once or automatic rollback.

**Evidence:** docs/evidence/ticket-completion-recovery.md

### async-operations

- [ ] Deliver this task.

**Goal:** Track remote jobs separately from submission and caller time budgets.

**Spec:** operation-recovery / Bounded transport and async jobs

**Depends on:** mutation-journal, retry-deadlines, provider-reconciliation

**Build:** Implement operation get/wait adapters, accepted/running/succeeded/failed/unknown job state and durable handle persistence; support bounded pending returns.

**Verify:** Simulate accepted-then-failed/pending jobs, lost submission receipt, polling 429 and cancellation, checking no second job is submitted.

**Done when:** Successful submission is never labeled completed remote work; handles and known outcomes remain usable after restart.

**Evidence:** docs/evidence/async-operations.md

### operation-preview

- [x] Deliver this task.

**Goal:** Validate and describe mutations before apply without accidental writes.

**Spec:** agent-interface / Strict request validation

**Depends on:** strict-json-input, operation-registry, mutation-journal, profile-resolution

**Build:** Implement local input validate and operation preview, digest/effect/precondition receipts and explicit --check-remote reads. Preserve permanent-effect acknowledgments and target policy.

**Verify:** Assert zero network for local modes, no resource writes for remote preview, stale precondition checks at apply and apply-required behavior.

**Done when:** Preview is clearly not a lock/approval and applying a different payload cannot reuse its bound manifest silently.

**Evidence:** docs/evidence/foundation-closure.md

## Lists, projection and pagination

### response-projection

- [x] Deliver this task.

**Goal:** Fetch and emit only declared fields while preserving control evidence.

**Spec:** agent-interface / Bounded field-selected output

**Depends on:** contract-discovery, trusted-http

**Build:** Implement dot-path/array-element projection, always-returned identity/control fields, default metadata selection and server/local/mixed adapter mappings. Retain upstream continuation fields internally.

**Verify:** Test unknown paths, child/sibling selection, nullable/missing/unselected data, redaction uncertainty and output schema under selected fields.

**Done when:** Projection saves agent-visible content without fabricating nulls or losing IDs/version/next metadata.

**Evidence:** docs/evidence/foundation-closure.md

### cursor-store

- [x] Deliver this task.

**Goal:** Persist opaque continuation with identity binding and fetched remainders.

**Spec:** agent-interface / Lossless pagination

**Depends on:** mutation-journal, response-projection, artifact-store

**Build:** Add bounded expiring random local cursor handles with immutable logical position, native continuation, query/grant/schema/projection fingerprint and buffered records/provenance.

**Verify:** Test corrupt/foreign/expired/mismatched cursors, token refresh under same grant, concurrent replay and store-write interruption.

**Done when:** Cursors grant no authority, cannot silently restart and cannot advance past unreturned records.

**Evidence:** docs/evidence/provider-pagination.md

### bounded-listing

- [x] Deliver this task.

**Goal:** Make default paging small, lossless and honest about live collections.

**Spec:** agent-interface / Lossless pagination

**Depends on:** cursor-store, retry-deadlines, cli-envelopes

**Build:** Implement per-call item/byte budgets, at most one native page fetch, buffered replay, explicit next/has_more/stop reason and optional totals only when requested/supported.

**Verify:** Test empty/short nonterminal pages, terminal pages, many small records and one oversized item; compare every emitted ID through exhaustion.

**Done when:** No item is skipped to fit bytes; normal page boundaries and actual truncation remain distinct.

**Evidence:** docs/evidence/provider-pagination.md

### scan-artifacts

- [ ] Deliver this task.

**Goal:** Offer bounded complete scans without dumping a collection into model context.

**Spec:** agent-interface / Portable artifact I/O

**Depends on:** bounded-listing, artifact-store, async-operations

**Build:** Implement explicit scan/export with total item/page/byte/time caps, checkpoint/artifact digest and complete/budget/rate/cancelled/error outcomes. Detect repeated/no-progress native continuation.

**Verify:** Resume after 429/cancellation and verify retained IDs/digest/checkpoint; trigger a repeating cursor and stop with pagination-stalled.

**Done when:** A partial export cannot claim completeness and no unbounded --all stdout path is introduced.

**Evidence:** docs/evidence/scan-artifacts.md

### pagination-conformance

- [ ] Deliver this task.

**Goal:** Prove cursor/projection behavior independently of one provider fixture.

**Spec:** agent-interface / Lossless pagination

**Depends on:** bounded-listing, scan-artifacts, descriptor-drift-checks

**Build:** Create shared adapter conformance cases for native token, next-link and offset providers, field masks and local filtering. Include live insertion/deletion duplicates/gaps as declared limitations.

**Verify:** Run the same observable-ID suite against synthetic provider styles, replay, changed binding and byte truncation, validating all response schemas.

**Done when:** All provider adapters can reuse a losslessness/metadata test contract and none falsely claims snapshot isolation.

**Evidence:** docs/evidence/pagination-conformance.md

## Provider contract inventory

### provider-operation-matrix

- [x] Deliver this task.

**Goal:** Pin the exact endpoint/auth/permission contract for each researched command.

**Spec:** cloud-providers / Scoped capability discovery

**Depends on:** operation-registry, contract-discovery

**Build:** Create a reviewed machine-readable matrix from the existing API catalog and pinned provider specs: method/route, input/output, scope alternatives, permissions, tenant conditions, lifecycle effect and recovery class per operation. Mark unverified routes unavailable.

**Verify:** Compare every Core/Extended/Privileged/Conditional catalog row with owning operations/tasks; validate route/scopes/descriptors and explicit unsupported/deferred entries.

**Done when:** No catalog command is silently lost or advertised only because a generated client method exists.

**Evidence:** docs/evidence/provider-contracts.md

## Jira core client

### jira-context

- [x] Deliver this task.

**Goal:** Read Jira context and live field/workflow constraints with bounded output.

**Spec:** cloud-providers / Native Jira resources

**Depends on:** provider-operation-matrix, bounded-listing, service-oauth

**Build:** Implement issue get/search via enhanced JQL, project list/get/statuses, field list/search, create/edit metadata and contextual permission checks. Use explicit site/project/resource identity and metadata freshness.

**Verify:** Fixtures exercise search cursors/stale visibility, required custom fields, account resolution, issue security, permission denial and selected-field projection.

**Done when:** Read operations expose native limits and sufficient typed metadata for writes without guessing schemas or returning full issue blobs by default.

**Evidence:** docs/evidence/jira-context-activation.md

### jira-issue-writes

- [x] Deliver this task.

**Goal:** Create, edit, assign and delete authorized issues with recoverable effects.

**Spec:** cloud-providers / Native Jira resources

**Depends on:** jira-context, operation-preview, provider-reconciliation

**Build:** Implement issue create/update/assign/delete with live required fields, constrained custom fields, expected project/issue type, explicit unassignment and subtask-delete semantics. Use targeted updates preserving unknown/human fields.

**Verify:** Test required fields, wrong project/account, stale detectable fingerprints, partial subtask restrictions, lost create response and replayed operation IDs.

**Done when:** General CRUD works under its own policy, never bypasses mandatory project authority and never invents Jira compare-and-swap.

**Evidence:** docs/evidence/jira-issue-action-activation.md

### composed-host-acceptance

- [x] Deliver this task.

**Goal:** Qualify the actual create host across sealed cache access, bounded HTTP, send claims and completion without a production origin override.

**Spec:** delivery-quality / Synthetic composed acceptance

**Depends on:** fixture-testkit, provider-operation-matrix

**Build:** Add a non-default debug-only synthetic fixture feature with fixed sentinel credentials/bearer, loopback-only ports, cache/token provenance checks and isolated memory credentials/native grant locks. Execute the unchanged host/library APIs through an owned TLS server; keep default builds and public dispatch unaffected.

**Verify:** Observe complete success, metadata/account qualification, scope/policy denial before resource reads, token/metadata expiry or cancellation after a committed claim, lost/partial responses, replay refusal and receipt retention. Refuse non-synthetic credentials/bearers/cache before fixture network access. Prove the feature cannot compile without debug assertions and normal dependency/default discovery remain unchanged.

**Done when:** The integration tests exercise actual composed code, negative fixture controls and build/discovery separation pass, and limitations distinguish memory-vault integration from native-vault/live/public qualification.

**Evidence:** docs/evidence/composed-host-acceptance.md

### create-command-policy

- [x] Deliver this task.

**Goal:** Connect candidate create arguments to exact persisted private policy and the tested host without public activation.

**Spec:** secure-auth / Explicit profiles and grants

**Depends on:** profile-resolution, composed-host-acceptance

**Build:** Add strict typed flags/structured arguments, skipped parser/candidate registration and complete envelope dispatch. Resolve profile/project context separately from revisioned installation/profile/grant/project/actor policy. Add atomic private-state migration, apply/CAS/preflight policy administration APIs and dynamic policy checks before acquisition/reads and at claimed send.

**Verify:** Check flags/JSON/null/omission parity, invalid/apply/output refusal before state creation, missing/disabled/mismatched policy, concurrent revision writers, corrupt rows and migration rollback/preservation. Exercise actual cached candidate success and policy changes before auth/after claim; prove public discovery stays unchanged and dispatch retains partial receipts/request identity.

**Done when:** These internal adapter/policy behaviors have observed acceptance evidence and remain unregistered publicly, with local policy explicitly distinguished from live eligibility, human-review evidence and distributed fencing. Broader public auth/CRUD/reconciliation tasks remain open.

**Evidence:** docs/evidence/create-command-policy.md

### jira-transitions

- [x] Deliver this task.

**Goal:** Use actual workflow transitions instead of arbitrary status edits.

**Spec:** cloud-providers / Native Jira resources

**Depends on:** jira-context, jira-issue-writes

**Build:** Implement transition list/apply with validated IDs, required transition fields, target state evidence and operation-specific reconciliation.

**Verify:** Exercise nonstandard workflows, unavailable transitions, denied permissions, wrong field values and an uncertain transition response.

**Done when:** A status-looking field cannot silently replace a valid transition and duplicate retry behavior is explicitly safe or refused.

**Evidence:** docs/evidence/jira-issue-action-activation.md

### jira-comments

- [x] Deliver this task.

**Goal:** Manage comments without losing visibility or edit/delete authority.

**Spec:** cloud-providers / Native Jira resources

**Depends on:** jira-context, jira-issue-writes

**Build:** Implement comment list/get/create/update/delete, ADF validation, visibility restrictions, own-versus-all permission handling and bounded bodies/receipts.

**Verify:** Fixtures cover own/all denial, private visibility, malformed rich text, local parsing failures and lost create response.

**Done when:** Comments preserve native access controls and do not turn human review text into trusted CLI instructions.

**Evidence:** docs/evidence/jira-comment-mutations.md

### jira-links

- [x] Deliver this task.

**Goal:** Represent issue dependencies with explicit direction and remote identities.

**Spec:** cloud-providers / Native Jira resources

**Depends on:** jira-context, jira-issue-writes

**Build:** Implement issue-link list/create/delete with enabled-feature checks, actual link-type metadata and source/target direction. Reconcile duplicate/uncertain link application using observed issue/link IDs.

**Verify:** Test disabled linking, reversed direction, cross-project policy, existing links and response loss after creation.

**Done when:** Dependency links match intended semantics and partial link failure does not erase confirmed issues.

**Evidence:** docs/evidence/jira-link-lifecycle.md

### jira-attachments

- [x] Deliver this task.

**Goal:** Upload, inspect, download and remove files using Jira's real API.

**Spec:** cloud-providers / Native Jira resources

**Depends on:** jira-context, binary-transfer, provider-reconciliation

**Build:** Implement issue-field attachment listing, metadata/settings, multipart file upload with documented Jira header, content download and delete. Check size/permissions; exclude a fabricated binary-update route.

**Verify:** Verify exact multipart/header/routes, filename collisions, settings limits, delete denial, redirect security, digest receipts and uncertain upload recovery.

**Done when:** Each file operation has native semantics and filename equality alone cannot prove replay identity.

**Evidence:** docs/evidence/jira-attachment-download.md

### jira-conformance

- [ ] Deliver this task.

**Goal:** Qualify the entire Core Jira operation family against its descriptors.

**Spec:** delivery-quality / Behavioral and platform qualification

**Depends on:** jira-issue-writes, jira-transitions, jira-comments, jira-links, jira-attachments, pagination-conformance

**Build:** Run an operation-by-operation fixture matrix plus full create/read/edit/assign/comment/link/transition/file/delete sandbox lifecycle when authorized. Record effective actor/scopes/permissions.

**Verify:** Validate every Core Jira result schema and negative permission/target/retry case; keep live tests explicitly separate from fixtures.

**Done when:** Every advertised Core Jira operation has route/behavior/recovery evidence; absent live access does not become a production-support claim.

**Evidence:** docs/evidence/jira-conformance.md

## Confluence core client

### confluence-context

- [x] Deliver this task.

**Goal:** Read spaces/pages/search with representation and continuation provenance.

**Spec:** cloud-providers / Native Confluence resources

**Depends on:** provider-operation-matrix, bounded-listing, service-oauth

**Build:** Implement v2 space/page get/list and v1 CQL search adapters, explicit space ID/key mapping, body representation selection and selected-field output.

**Verify:** Test v1/v2 pagination, inaccessible content, ambiguous space, wrong tenant, CQL errors and optional body expansion.

**Done when:** Discovery never guesses a space key from an ID or conflates a metadata-only result with complete page content.

**Evidence:** docs/evidence/confluence-command.md

### document-rendering

- [x] Deliver this task.

**Goal:** Render and import the supported rich-content subset deterministically.

**Spec:** reviewed-plans / Human-edited plan import

**Depends on:** core-request-model, strict-json-input, fixture-testkit

**Build:** Implement AST-based Markdown subset conversion to Confluence storage and Jira ADF, escaping links/code/tables and a parser for visible structured plan sections. Preserve raw structured retrieval as an explicit format.

**Verify:** Round-trip supported content including Unicode/multiline code; reject unsupported macros/ambiguous human structures without regex-based HTML patching.

**Done when:** No promise of lossless arbitrary Confluence Markdown editing; unsupported constructs are identified precisely rather than dropped.

**Evidence:** docs/evidence/document-import.md

### confluence-pages

- [ ] Deliver this task.

**Goal:** Publish and update pages while preserving revision and human content.

**Spec:** cloud-providers / Native Confluence resources

**Depends on:** confluence-context, document-rendering, operation-preview, provider-reconciliation

**Build:** Implement page create/get/update/title behavior and trash using explicit representation, current-page lifecycle and expected version/digest checks. No force-overwrite retry or implicit unpublished-draft discard.

**Verify:** Test version conflict, concurrent human change, rich-content payload, lost create result, trash effect and editor-draft interaction in a sandbox.

**Done when:** Normal review pages are published and updates preserve supported human edits or stop with a conflict/unsupported-content error.

**Evidence:** docs/evidence/confluence-write-command.md

### confluence-spaces

- [ ] Deliver this task.

**Goal:** Create spaces with explicit tenant adapter choice and global permission.

**Spec:** cloud-providers / Native Confluence resources

**Depends on:** confluence-context, operation-preview, provider-reconciliation

**Build:** Implement list/get/create with v2 RBAC availability and a separately qualified v1 legacy route. Resolve capabilities before send and retain space key/ID.

**Verify:** Test RBAC and legacy fixtures, global permission denial, private/default settings and unknown create response; sandbox-check actual supported path.

**Done when:** No uncertain v2 create is replayed through v1 and metadata update/deletion remain outside the worker bundle.

**Evidence:** docs/evidence/confluence-space-writes.md

### confluence-comments

- [ ] Deliver this task.

**Goal:** Read and manage versioned footer comments and reviewer provenance.

**Spec:** cloud-providers / Native Confluence resources

**Depends on:** confluence-context, confluence-pages

**Build:** Implement footer-comment list/get/create/update/delete/replies with page association, version and author/edit metadata. Preserve permanent-deletion semantics and bounded output.

**Verify:** Test wrong-page comment, version conflicts, own/admin permission denial, deleted/edited content and uncertain creation.

**Done when:** Returned metadata is sufficient for the later approval adapter to validate or explicitly refuse reviewer evidence.

**Evidence:** docs/evidence/comment-observation.md

### confluence-attachments

- [ ] Deliver this task.

**Goal:** Manage Confluence files without silent filename upsert or binary replacement confusion.

**Spec:** cloud-providers / Native Confluence resources

**Depends on:** confluence-context, binary-transfer, provider-reconciliation

**Build:** Implement v2 list/get/trash, v1 upload, ID-based binary replacement, metadata update and download; add explicit filename-upsert mode as a declared operation with distinct collision semantics and documented header.

**Verify:** Check POST/PUT/data routes, documented multipart fields/header, permissions, collision/version changes, redirect/digest behavior and trash versus permanent purge.

**Done when:** Upload, filename upsert and ID replacement cannot silently substitute for one another; uncertain writes preserve IDs/state.

**Evidence:** docs/evidence/confluence-space-writes.md

### confluence-content-state

- [ ] Deliver this task.

**Goal:** Expose page content badges separately from lifecycle and human review.

**Spec:** cloud-providers / Native Confluence resources

**Depends on:** confluence-context, operation-preview

**Build:** Implement content-state get/available/set/clear, required native lifecycle-status query and allowed/custom-state checks from space settings.

**Verify:** Test disabled custom states, invalid badge/space policy, clear operation and attempts to use a badge as approval.

**Done when:** Badge changes never authorize plan tickets or act as Jira workflow transitions.

**Evidence:** docs/evidence/confluence-space-writes.md

### confluence-conformance

- [ ] Deliver this task.

**Goal:** Qualify Core pages/spaces/comments/files/badges as one native provider.

**Spec:** delivery-quality / Behavioral and platform qualification

**Depends on:** confluence-pages, confluence-spaces, confluence-comments, confluence-attachments, confluence-content-state, pagination-conformance

**Build:** Run per-operation descriptor/HTTP/schema fixtures and a consented two-person sandbox page lifecycle with edits/files/comments/space behavior.

**Verify:** Verify every Core route, permission condition, partial/unknown outcome and body round trip; explicitly record editor-draft and RBAC live evidence.

**Done when:** The provider advertises only implemented capabilities and distinguishes fixture qualification from actual tenant/platform qualification.

**Evidence:** docs/evidence/live-read-conformance.md

## Reviewed plan workflow

### plan-model

- [x] Deliver this task.

**Goal:** Define complete visible plans and task graphs without tying runtime to development labels.

**Spec:** reviewed-plans / Published shared planning

**Depends on:** document-rendering, core-request-model

**Build:** Define plan/design/task schemas with stable domain task identities, context, acceptance, repositories, dependencies, inclusion and explicit Jira mappings. Add scaffold/validate/render commands and cycle checks.

**Verify:** Round-trip render/import and reject cycles, missing references, duplicate domain task identities, unsupported structure and ambiguous fields.

**Done when:** Runtime plan task identities are business data; no development milestone/task label is injected into code or generated schemas.

**Evidence:** docs/evidence/plan-model.md

### plan-page-publish

- [ ] Deliver this task.

**Goal:** Publish the complete proposal for humans before creating plan-derived issues.

**Spec:** reviewed-plans / Published shared planning

**Depends on:** plan-model, confluence-pages

**Build:** Implement design scaffold/validate/page-create/page-update and plan page-create/page-get/page-update with readable task sections, proposed review state and design references.

**Verify:** Record HTTP effects for a full plan publication and verify zero Jira creates; two humans can view/edit the sandbox page.

**Done when:** Publication is normal shared content and cannot be mistaken for completed review or materialized tickets.

**Evidence:** docs/evidence/document-publication.md

### plan-human-import

- [ ] Deliver this task.

**Goal:** Make reviewed visible page content authoritative after publication.

**Spec:** reviewed-plans / Human-edited plan import

**Depends on:** plan-page-publish, plan-model

**Build:** Implement page import/validation, source snapshots and stable task identity preservation. Accept supported human edits to titles/acceptance/dependencies/exclusions and fail precisely on unsupported edits.

**Verify:** Edit a page as a second actor, then preview/import; compare parsed fields to visible content while deliberately changing stale local JSON.

**Done when:** Human changes drive future ticket input and hidden/original files cannot silently override them.

**Evidence:** docs/evidence/remote-document-import.md

### review-authority

- [ ] Deliver this task.

**Goal:** Establish eligible human approval and reject impersonation/self-approval.

**Spec:** reviewed-plans / Revision-bound human review

**Depends on:** tenant-auth-inventory, confluence-comments, plan-human-import

**Build:** Implement trusted reviewer configuration and approval-comment/trusted-adapter contract, including page association, actual API identity/edit metadata and unresolved-request policy. Fail if metadata cannot establish eligibility.

**Verify:** Test agent-authored approval-looking text, wrong actor/page, edited/deleted/revoked comments and unavailable identity evidence; independently review the policy.

**Done when:** Eligibility is derived from trusted policy and observed identity, not a supplied comment ID, sentiment, badge or self-written state.

**Evidence:** docs/evidence/reviewer-policy.md

### review-revision-validation

- [ ] Deliver this task.

**Goal:** Freeze the exact reviewed content and material targets before mutation.

**Spec:** reviewed-plans / Revision-bound human review

**Depends on:** review-authority, plan-human-import, artifact-store

**Build:** Implement versioned canonical plan digest, source/design snapshots, expected page revision and target/field-mapping binding. Add review-status checks and defined approval convention.

**Verify:** Change one title, design revision, included task, Jira mapping, target or approval metadata and verify stale review; distinguish semantic digest from raw-body snapshot.

**Done when:** Every material reviewed input is retained and a new revision requires renewed approval for further creation.

**Evidence:** docs/evidence/review-verification.md

### ticket-manifest

- [ ] Deliver this task.

**Goal:** Build a complete no-write ticket preview from the approved source.

**Spec:** reviewed-plans / Recoverable ticket materialization

**Depends on:** review-revision-validation, jira-context, jira-links, mutation-journal

**Build:** Implement plan tickets-preview reading current page, validating task DAG/hierarchy and live Jira metadata, then producing bounded preview plus complete artifact manifest/digest.

**Verify:** Compare preview fields to human-edited page, discover required project fields and assert no issue/link mutations; reject unapproved target changes.

**Done when:** Preview is sufficient for human review and later apply uses the same frozen interpreted input.

**Evidence:** docs/evidence/ticket-preview.md

### ticket-application

- [ ] Deliver this task.

**Goal:** Create approved issues and dependencies with between-batch review checks.

**Spec:** reviewed-plans / Recoverable ticket materialization

**Depends on:** ticket-manifest, jira-issue-writes, jira-links, provider-reconciliation

**Build:** Implement plan tickets-create with source/approval rechecks, dependency-order batches, explicit consistency mode, durable domain-task-to-issue mappings and result artifact/page publication.

**Verify:** Change approval/page mid-run; inject partial create/link and result-write failure; verify subsequent batches stop and confirmed mappings persist.

**Done when:** No plan-derived ticket precedes approval and a result-publication error cannot turn confirmed issues into failed/unknown creates.

**Evidence:** docs/evidence/ticket-application.md

### ticket-recovery

- [ ] Deliver this task.

**Goal:** Resume interrupted plan application without duplicating confirmed work.

**Spec:** reviewed-plans / Recoverable ticket materialization

**Depends on:** ticket-application, async-operations

**Build:** Implement tickets-resume/reconciliation, per-step mappings, linked-source provenance and separate result-publication retry. Require fresh eligibility/revision handling for unfinished work after source changes.

**Verify:** Kill between create/receipt/link/result steps and resume the same operation; verify actual issue counts, preserved IDs and explicit unresolved states.

**Done when:** Confirmed effects are never blindly replayed or automatically deleted; changed inputs require explicit reconciliation/review.

**Evidence:** docs/evidence/reviewed-issue-reuse.md

## Extended and privileged resources

### jira-remote-links

- [ ] Deliver this task.

**Goal:** Connect tasks to pages/artifacts through explicit remote-link semantics.

**Spec:** cloud-providers / Privileged and conditional operations

**Depends on:** jira-context, provider-reconciliation

**Build:** Implement remote-link list/create/update/delete using known link IDs and explicit global-ID create/update mode. Preserve native permission and target policy.

**Verify:** Exercise create versus upsert, existing global ID, wrong issue and lost response; inspect exact request routes.

**Done when:** No generic create command silently changes an existing remote link under an ambiguous identity.

**Evidence:** none

### jira-release-metadata

- [ ] Deliver this task.

**Goal:** Manage project versions/components under project-management authority.

**Spec:** cloud-providers / Privileged and conditional operations

**Depends on:** jira-context, operation-preview

**Build:** Implement version/component list/get/create/update/delete; version release/archive flags and delete replacement/move mappings are explicit. Separate manage-project scopes from worker writes.

**Verify:** Fixtures cover released/archive updates, component reassignment/deletion mappings, insufficient project permissions and partial/unknown outcomes.

**Done when:** Metadata changes preserve issue relationships and ordinary issue-write scope is not assumed sufficient.

**Evidence:** none

### jira-project-admin

- [ ] Deliver this task.

**Goal:** Provide isolated project lifecycle operations without granting every worker admin.

**Spec:** cloud-providers / Privileged and conditional operations

**Depends on:** jira-context, async-operations, operation-preview

**Build:** Implement project create/update/archive/restore/delete through a separately enabled bundle; pin synchronous/asynchronous route choices and native job-status adapter before enabling deletion.

**Verify:** Test disabled bundle, exact destructive acknowledgment, archive/restore constraints, async failure and unknown deletion submission.

**Done when:** No unsupported admin route is advertised and permanent/async consequences are visible before apply.

**Evidence:** none

### confluence-versions-labels-properties

- [ ] Deliver this task.

**Goal:** Expose useful page metadata without conflating historical content or secret storage.

**Spec:** cloud-providers / Privileged and conditional operations

**Depends on:** confluence-context, confluence-pages

**Build:** Implement page version-list/version-get, label list/add/remove and page property list/get/create/update/delete with revision checks. Document metadata versus historical body retrieval and additive labels.

**Verify:** Test duplicate labels, explicit removal, property version conflicts, null/absence projection and attempts to treat properties as credential storage.

**Done when:** These operations preserve native version/additive behavior and metadata is not advertised as a lock or vault.

**Evidence:** none

### confluence-inline-comments

- [ ] Deliver this task.

**Goal:** Add inline-comment operations only after their schemas and anchors are verified.

**Spec:** cloud-providers / Privileged and conditional operations

**Depends on:** confluence-comments, provider-operation-matrix

**Build:** Implement inline list/get/create/update/delete and valid anchors/replies after current API schema audit. Keep resolution operations unsupported until a concrete contract is verified.

**Verify:** Test anchor/body validity, version/permission conflicts and permanent deletion; unsupported resolve returns a capability error before HTTP.

**Done when:** Inline capabilities are accurate and do not invent a separate resolve route or manufacture human review completion.

**Evidence:** none

### confluence-move-copy

- [ ] Deliver this task.

**Goal:** Move/copy individual pages or trees with explicit destinations and async recovery.

**Spec:** cloud-providers / Privileged and conditional operations

**Depends on:** confluence-pages, async-operations, provider-reconciliation

**Build:** Implement move/copy/copy-tree with validated before/after/append semantics, destination scope and provider-specific job handles. Preserve copies' observed new IDs rather than assuming identity.

**Verify:** Test cross-space policy, invalid destination, sibling/top-level caveats, partial tree behavior and lost submission receipt.

**Done when:** Page topology changes and copied resources are observable/recoverable without an atomic-tree or automatic rollback claim.

**Evidence:** none

### confluence-archive

- [ ] Deliver this task.

**Goal:** Keep experimental archival conditional and asynchronous.

**Spec:** cloud-providers / Privileged and conditional operations

**Depends on:** confluence-pages, async-operations, provider-operation-matrix

**Build:** Implement archive only behind an explicit experimental tenant-supported capability, using the documented bulk content API and returned task. Keep restore unavailable until researched.

**Verify:** Test disabled feature, accepted/pending/failed jobs, exact target manifests and unknown submission; record current API version/evidence.

**Done when:** Archive cannot be advertised as stable trash or reversible deletion, and no unverified restore command appears.

**Evidence:** none

### confluence-restrictions-purge

- [ ] Deliver this task.

**Goal:** Protect access-broadening and irreversible page/file changes.

**Spec:** cloud-providers / Privileged and conditional operations

**Depends on:** confluence-pages, confluence-attachments, operation-preview, provider-operation-matrix

**Build:** Implement restrictions get/add/replace/clear and page/attachment purge with distinct permissions/effects, explicit acknowledgment and expected current lifecycle. Keep space-permission APIs deferred.

**Verify:** Test replacement/clear access broadening, insufficient manage-content permission, purge-before-trash, wrong target and uncertain deletion.

**Done when:** Privileged operations cannot masquerade as worker metadata edits, trash or human approval.

**Evidence:** none

### confluence-space-admin

- [ ] Deliver this task.

**Goal:** Update space metadata and delete spaces with explicit permanent async effects.

**Spec:** cloud-providers / Privileged and conditional operations

**Depends on:** confluence-spaces, async-operations, operation-preview

**Build:** Implement v1 space metadata update and permanent deletion with correct key/ID mapping, privileged scope bundle, operation handle and deletion acknowledgment. Do not expose permission updates as metadata fields.

**Verify:** Test wrong-key refusal, denied admin permission, metadata-only scope, asynchronous failure/pending and lost deletion receipt.

**Done when:** The exact space and irreversible effect are represented honestly; no blanket cross-version fallback or rollback is implied.

**Evidence:** none

## Team and multi-machine execution

### shared-authority-contract

- [ ] Deliver this task.

**Goal:** Define where shared leases and duplicate prevention are actually enforced.

**Spec:** operation-recovery / Shared mutation authority

**Depends on:** mutation-journal, ticket-manifest, broker-protocol

**Build:** Specify a narrowly scoped mutation-authority protocol with operation binding, lease identity/expiry/fencing, ownership recovery, actor attribution and rate budgets. Compare coordinator-owned sends with cooperative client-only leases.

**Verify:** Model two workers, expired/reassigned leases, coordinator crash, partition and a bypassing writer; independently review the enforcement boundary.

**Done when:** The design says what exclusivity can be proven and does not treat Jira properties, assignees or local SQLite as distributed locks.

**Evidence:** none

### shared-authority-integration

- [ ] Deliver this task.

**Goal:** Connect the runtime to one explicitly configured trusted mutation authority.

**Spec:** operation-recovery / Shared mutation authority

**Depends on:** shared-authority-contract, ticket-application, trusted-http

**Build:** Implement the chosen adapter and enforcement path, server/client authentication, narrow operation capabilities, durable grants and explicit unavailable/advisory states. Do not deploy it implicitly.

**Verify:** Integration tests race contenders, deny stale leases at the actual send boundary, interrupt coordination and verify preserved operation mappings.

**Done when:** A protected plan cannot bypass authority through another local command path; standalone mode remains clearly weaker.

**Evidence:** none

### multi-machine-qualification

- [ ] Deliver this task.

**Goal:** Prove team mutation behavior with separate machines and identities.

**Spec:** operation-recovery / Shared mutation authority

**Depends on:** shared-authority-integration, ticket-recovery, auth-lifecycle

**Build:** Run a consented two-machine test with separately provisioned grants, shared authority, live sandbox targets and requester/executor/reviewer attribution. Retain crash/reassignment and rate-budget evidence.

**Verify:** Observe actual lease/send decisions and provider issue counts under concurrent duplicate attempts, partition and worker death.

**Done when:** Advertise multi-machine protection only for the tested enforcement/configuration; no copied rotating credential chain or exactly-once claim.

**Evidence:** none

## Skills and qualification

### runtime-skill-package

- [ ] Deliver this task.

**Goal:** Ship concise user instructions that load only workflow-specific detail.

**Spec:** delivery-quality / Progressive runtime skills

**Depends on:** contract-discovery, bounded-listing, operation-preview, ticket-recovery

**Build:** Create skills/craft-cloud/SKILL.md with short precise description/common path and directly linked lists/writes/plan-review/recovery references. Keep command schemas/enums in installed discovery, not copied manuals; add compatibility metadata.

**Verify:** Validate frontmatter/links, measure actual token lengths with evaluated tokenizers and check every cited command against the registry.

**Done when:** The short entrypoint covers authority, narrow reads, exact inputs and safe recovery without loading every provider or repeated confirmation loops.

**Evidence:** docs/evidence/runtime-skill.md

### skill-host-qualification

- [ ] Deliver this task.

**Goal:** Confirm the same skill package behaves correctly in supported coding-agent hosts.

**Spec:** delivery-quality / Progressive runtime skills

**Depends on:** runtime-skill-package, distribution-packaging

**Build:** Document explicit project/user installation for Codex and Claude Code, selective providers, version compatibility, update/uninstall and unchanged unrelated configuration. Test actual discovery/loading behavior.

**Verify:** Use isolated host profiles for a simple read, unfamiliar file operation and plan-review workflow; inspect which references loaded and clean removal.

**Done when:** Portable format is not mistaken for identical host behavior; no global automatic install or credential-bearing skill content.

**Evidence:** none

### adversarial-interface-corpus

- [ ] Deliver this task.

**Goal:** Measure input rejection and recovery against realistic agent mistakes.

**Spec:** delivery-quality / Behavioral and platform qualification

**Depends on:** jira-conformance, confluence-conformance, ticket-recovery, descriptor-drift-checks

**Build:** Build a held-out corpus of invented fields, wrong target/kind/user, stale approval, malformed/duplicate JSON, injection in cloud content, malicious paths/cursors and valid multilingual content. Define observable outcomes and effect verifiers.

**Verify:** Run deterministic fixture/subprocess cases; inspect remote request counts, final state, artifacts and false rejections rather than expected wording.

**Done when:** Malformed local cases make zero requests; prohibited semantic cases make no mutation; valid content is preserved and unknown outcomes do not duplicate effects.

**Evidence:** none

### agent-efficiency-evaluation

- [ ] Deliver this task.

**Goal:** Prove token efficiency on correctly completed tasks, not only small responses.

**Spec:** delivery-quality / Measured agent efficiency

**Depends on:** skill-host-qualification, adversarial-interface-corpus

**Build:** Run paired Codex/Claude Code tasks against full-output/long-skill and no-skill baselines, cold/warm schema discovery, lists/files/review/recovery. Record model/settings/tokenizer, cached/uncached usage, costs/calls/latency, failures and artifact fidelity.

**Verify:** Repeat held-out cases with outcome verification and report distribution/uncertainty; evaluate the proposed median token target without hiding incomplete runs.

**Done when:** Claim savings only from observed paired results with acceptable completion/correctness; otherwise revise budgets/specs transparently.

**Evidence:** none

### continuous-integration

- [ ] Deliver this task.

**Goal:** Make relevant contract/security/platform checks reproducible without a heavyweight harness.

**Spec:** delivery-quality / Behavioral and platform qualification

**Depends on:** descriptor-drift-checks, contributor-quality-guards, jira-conformance, confluence-conformance

**Build:** Add Cargo fmt/Clippy/tests/build, schema/descriptor/projection drift, dependency/license/secret/label checks and bounded platform jobs. Keep optional live tests separate and impact selection conservative for shared contracts/auth.

**Verify:** Run clean checkout CI and deliberate schema/layer/secret/label failure controls; ensure missing credentials/tools produce unrun/failed status, not green coverage.

**Done when:** CI verifies implemented surfaces and records native/provider gaps honestly without milestone-specific gate scripts or inherited numeric rituals.

**Evidence:** none

### native-platform-qualification

- [ ] Deliver this task.

**Goal:** Establish Windows, macOS and Linux support using real native sessions.

**Spec:** delivery-quality / Behavioral and platform qualification

**Depends on:** auth-lifecycle, binary-transfer, cursor-store, continuous-integration

**Build:** Qualify intended interactive/unattended principals on Windows Credential Manager, macOS Keychain and Linux persistent Secret Service, including restarts, locks, large chunks, binary paths and ACLs.

**Verify:** Run real vault write/read/rotation/logout/restart, locked/missing-store and interrupted generation cases per platform; preserve failed/unrun rows.

**Done when:** Each claimed OS/session mode has actual evidence; a mock Linux backend or WSL test cannot qualify native Windows/macOS.

**Evidence:** none

### live-provider-qualification

- [ ] Deliver this task.

**Goal:** Qualify actual Jira/Confluence permissions, conditional routes and human workflow.

**Spec:** delivery-quality / Behavioral and platform qualification

**Depends on:** jira-conformance, confluence-conformance, ticket-recovery, auth-lifecycle, jira-remote-links, jira-release-metadata, jira-project-admin, confluence-versions-labels-properties, confluence-inline-comments, confluence-move-copy, confluence-archive, confluence-restrictions-purge, confluence-space-admin

**Build:** Run a consented sandbox matrix by operation/bundle/auth mode with independent reviewer identity, custom fields/workflow, space RBAC/legacy, files/collisions/trash/purge and async jobs. Destructive resources are explicitly scoped.

**Verify:** Verify actual final provider state, exact scopes/permissions and recovery; conditionally unavailable/experimental operations stay disabled rather than get a fabricated pass.

**Done when:** Every advertised operation/bundle has appropriate live evidence; unsupported features remain explicit and initial Core release can be qualified separately.

**Evidence:** none

### distribution-packaging

- [ ] Deliver this task.

**Goal:** Prepare identifiable Rust binaries and skills without overwriting another Craft.

**Spec:** delivery-quality / Honest distribution and support

**Depends on:** cli-envelopes, runtime-skill-package, continuous-integration

**Build:** Build versioned platform archives, checksums/signatures where supported, license/notices, provider/contract/skill compatibility manifest and explicit install paths. Detect unrelated craft executables and require an explicit destination choice instead of replacement.

**Verify:** Install into isolated directories, verify integrity and --version/build identity, exercise PATH collision and tampered archive controls.

**Done when:** Packages are locally reviewable and identify this project; no publication, remote install or release is implied by preparation.

**Evidence:** docs/evidence/local-bundle.md

### upgrade-qualification

- [ ] Deliver this task.

**Goal:** Verify installation and state migration preserve credentials and operation recovery.

**Spec:** delivery-quality / Honest distribution and support

**Depends on:** distribution-packaging, native-platform-qualification, mutation-journal

**Build:** Test upgrade/downgrade compatibility decisions, state/schema migrations, skill compatibility, uninstall selection and shared-grant credential retention/removal policy. Write recovery/support docs based on actual results.

**Verify:** Upgrade isolated old state with pending operations/cursors, simulate failed migration and remove one version without deleting unrelated profiles/binaries.

**Done when:** Supported transitions preserve recoverable state or refuse safely; release support matrix names only qualified paths.

**Evidence:** none

## Future cloud clients

### bitbucket-auth-spike

- [ ] Deliver this task.

**Goal:** Research and qualify Bitbucket-specific OAuth/routing before resource implementation.

**Spec:** cloud-providers / Additional cloud clients

**Depends on:** provider-operation-matrix, auth-lifecycle

**Build:** Pin Bitbucket Cloud auth/token/refresh/identity/scopes and repository/workspace addressing, distinguishing OAuth and any separately approved static-token mode. Audit actual PR endpoint compatibility.

**Verify:** Use current official API schema and authorized sandbox read/login to verify routes and scope/identity behavior; keep unrelated Atlassian cloud-ID assumptions out.

**Done when:** A reviewed provider contract and fixtures establish a viable auth path without borrowing Jira token discovery or grant semantics.

**Evidence:** none

### bitbucket-pr-client

- [ ] Deliver this task.

**Goal:** Add one complete native code-review client through the existing extension points.

**Spec:** cloud-providers / Additional cloud clients

**Depends on:** bitbucket-auth-spike, pagination-conformance, provider-reconciliation, binary-transfer

**Build:** Add compiled Bitbucket provider with repository/PR reads, paginated diffs/comments/tasks, comment/task changes and declared review actions. Separate agent feedback from human approval/merge policy; enable merge only with independent policy qualification.

**Verify:** Run provider conformance and sandbox read/comment/review lifecycle, permission denial, pagination and lost-write recovery; inspect crate dependencies.

**Done when:** A new cloud client works without changing core contracts or assuming AI approval satisfies repository review controls.

**Evidence:** none

### trello-auth-spike

- [ ] Deliver this task.

**Goal:** Resolve Trello's current OAuth/callback and endpoint compatibility uncertainties.

**Spec:** cloud-providers / Additional cloud clients

**Depends on:** provider-operation-matrix, auth-lifecycle

**Build:** Verify public versus confidential PKCE registration/callback suitability, token endpoint inconsistency, scopes, refresh ownership and supported resource operations from current official evidence. Choose native/broker path explicitly.

**Verify:** Perform authorized sandbox exchange/read against the verified endpoint and test state/PKCE/refresh failure controls; do not probe alternate token hosts with secrets.

**Done when:** Trello has a documented tested auth route independent of Jira cloud IDs and unsupported OAuth operations remain marked unavailable.

**Evidence:** none

### trello-card-client

- [ ] Deliver this task.

**Goal:** Add board/list/card work through native Trello semantics.

**Spec:** cloud-providers / Additional cloud clients

**Depends on:** trello-auth-spike, pagination-conformance, binary-transfer, provider-reconciliation

**Build:** Implement a bounded initial slice for workspace/board/list discovery and card get/list/create/update/move/archive/delete with comments/checklists/attachments as separately declared capabilities. Pin each native method before advertising it.

**Verify:** Run card lifecycle/permission/field/projection/paging and uncertain-write conformance, including invalid cross-board list mapping and file behavior.

**Done when:** Moves use validated list IDs and archive uses Trello semantics; no universal status mapping or implicit Jira transition translation.

**Evidence:** none

### provider-extension-conformance

- [ ] Deliver this task.

**Goal:** Prove future clients remain extensions rather than exceptions in shared code.

**Spec:** cloud-providers / Isolated provider modules

**Depends on:** bitbucket-pr-client, trello-card-client, descriptor-drift-checks

**Build:** Run common contract/scope/tenant/rate/cursor/binary/recovery cases across all qualified providers; document registration recipe and capability differences. Revisit narrow workflow ports only when shared semantics are actually demonstrated.

**Verify:** Inspect core dependencies and diff required for each new provider; add a synthetic further provider using the published module contract.

**Done when:** No cloud-client branch logic leaks into core/auth defaults; unsupported native features stay visible and the extension recipe is executable.

**Evidence:** none
