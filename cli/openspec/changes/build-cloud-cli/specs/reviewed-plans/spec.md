# Reviewed planning workflow

## ADDED Requirements

### Requirement: Published shared planning
An agent SHALL publish a normal Confluence page containing the proposed design/task graph for human review. Native unpublished drafts and ticket creation SHALL not be implicit side effects.

#### Scenario: Plan publication
- **WHEN** a proposed plan page is created
- **THEN** humans can read/edit the complete visible task graph and no Jira tickets have been created by this workflow.

#### Scenario: Typed document publication
- **WHEN** `plan page-create` or `design page-create` loads a local JSON, storage or ADF document with explicit apply and an operation identity
- **THEN** validation and complete visible rendering precede profile/secret access, document target metadata supplies the exact site/space/parent without override, and the existing Confluence create policy and durable journal govern the only remote write.
- **AND** a plan must have proposed review state; publication returns the complete native page receipt without claiming review or ticket creation. Its local journal retains the native Confluence operation identity, and uncertain writes cannot be replayed implicitly.

#### Scenario: Local plan and design preparation
- **WHEN** scaffold, validate or render prepares a local plan/design
- **THEN** explicit target metadata and caller-supplied semantic work identities are used without profile/auth/network access, incomplete templates are labeled, graphs reject cycles/missing or duplicate identities, and visible review-state labels never become verified approval or ticket authority.

#### Scenario: Explicit scaffold targets
- **WHEN** a caller supplies scaffold targets through flags or JSON
- **THEN** both channels accept the same flat required target fields, the generated document retains nested target metadata, and semantic errors identify the supplied seed field rather than an absent nested input object.

### Requirement: Human-edited plan import
After publication the visible page SHALL be authoritative. Import SHALL retain task identities, human changes and unsupported-content errors without restoring stale hidden/local JSON.

#### Scenario: Human changes acceptance
- **WHEN** a reviewer changes a task's acceptance criteria in the page
- **THEN** the imported plan and future ticket preview reflect the new visible criteria or fail with a precise ambiguity error.

#### Scenario: Remote document retrieval
- **WHEN** `plan page-get` or `design page-get` reads a published page as storage or ADF
- **THEN** only its current visible body supplies the document, the document's Confluence tenant and space must match the observed page, and the result includes the exact page/version/representation fingerprint and semantic document digest. The native Confluence title is authoritative over a duplicated standalone body heading.
- **AND** selected output fields do not skip complete import validation, display approval state grants no review authority, and import/projection/output failures retain credential-renewal evidence without a resource write or stale local fallback.

#### Scenario: Supported document rendering
- **WHEN** bounded Markdown containing supported paragraphs, headings, emphasis, HTTP links, lists, code and rectangular tables is rendered
- **THEN** a shared AST produces deterministic storage/ADF, preserves Unicode and code content, escapes markup and rejects unsupported constructs with a source byte offset rather than silently removing them.

#### Scenario: Explicit raw retrieval and visible import
- **WHEN** storage or ADF is retrieved for inspection or semantic import
- **THEN** explicit raw format retains the bounded original content even if import fails; semantic import rejects duplicate fields, unsupported macros/attributes, merged tables and ambiguous visible field names, preserving supported human changes and identifying the failure by byte offset or JSON path.

### Requirement: Revision-bound human review
Ticket materialization SHALL verify eligible human approval bound to exact page/version/digest, target/field mapping and included design revisions, including edited/revoked approval.

#### Scenario: Native complete footer inventory
- **WHEN** the native review adapter collects comments for an exact page
- **THEN** both root and reply allowances are captured and checked before authentication, all root pages and each observed comment's direct replies are exhausted, and duplicate identities, cursor cycles, unreadable branches, contradictory page/parent metadata or policy drift refuse complete coverage.
- **AND** collection is bounded to 1,024 comments, 65,536 encoded bytes per body, 8 MiB total bodies, 2,048 native collection pages and a 30-second dual-clock freshness interval. Cancellation and the caller's I/O deadline remain effective. Credential-cache effects survive collection failures; failure cannot return a complete inventory.
- **AND** the direct current parent proof for every reply collection must match that comment's inventoried version/author/time. A stale list result cannot override contradictory current-GET evidence, even when repeated list reads return the same stale data.

#### Scenario: Live review preparation
- **WHEN** an operator prepares review for a normal shared plan page and explicit per-task Jira create proposals
- **THEN** current page/space identity and the visible plan are imported, every referenced design is read at its exact native version and semantic document digest, and included task identities must exactly match the proposals. Project keys, issue-type names, fields and optional existing-parent/assignee context are qualified through native create metadata under configured project create policy, without a journal claim or POST.
- **AND** the binding covers canonical task payloads and material field metadata, including reported defaults; omitted fields with declared but unreported defaults cannot be presented as resolved. Observation timestamps and native ordering of field rows do not change the material digest.
- **AND** local immutable review artifacts contain source bodies/documents, exact proposed payloads and mapping details alongside the approval statement. Preparation does not verify approval, publish comments or create tickets. Credential-cache effects and uncertain local artifact publication remain visible on failure.

#### Scenario: Public native approval verification
- **WHEN** an operator verifies a prepared review identity
- **THEN** the immutable review pack supplies constraints only. The enabled native reviewer policy must match its saved policy digest, and every binding observation is rebuilt through authenticated current plan/design/Jira metadata reads. Current root/reply inventories are exhausted twice; cached statements or receipt JSON cannot supply approval authority.
- **AND** execution identity, page/comment scopes, captured policies, source/mapping equality and a 30-second wall/monotonic window are checked across observation and final publication. Failure, missing quorum, unresolved human requests, edited evidence, revocation or expiry cannot return verified approval.
- **AND** successful native evidence and its first complete comment observation are recorded in a new immutable local verification artifact. Apply authorizes that local publication only. Unknown or confirmed publication effects and credential-cache receipts survive later failure; no ticket is created and serialized evidence cannot authorize a future write without new live checks.

#### Scenario: Stale approval
- **WHEN** the approved page or material mapping changes
- **THEN** creation stops and requires review of the new revision; an agent-authored approval-looking comment is insufficient.

#### Scenario: Trusted review evidence contract
- **WHEN** a host verifies a plan for materialization
- **THEN** operator-managed per-space reviewer eligibility, exclusions and distinct-reviewer quorum are combined with observed current comment-version identities, explicit structured decisions and exact plan/design/target/mapping bindings.
- **AND** complete root/reply inventories and current source/policy are read twice within a bounded dual-clock interval; missing coverage, unresolved requests, withdrawal, excluded actors, malformed reviewer content or source/policy/comment drift cannot produce a verified review.
- **AND** a verified result cannot be restored from caller JSON. Revalidation repeats live observation and preserves the original approval revisions; adding a new approval does not substitute for an edited or deleted original approval.

#### Scenario: Trusted reviewer administration
- **WHEN** an operator reads or changes reviewer configuration through `review policy get/set`
- **THEN** an explicit trusted Confluence service profile selects local authority; exact space, installation, grant and expected service actor are bound to the record. Eligible/excluded identities and quorum are explicit, unique and disjoint; a service actor cannot approve its own work.
- **AND** set requires explicit enabled state, apply, complete receipt capacity and revision CAS before commit. Missing policy grants no eligibility, disabled policy is refused before identity/resource reads, and changes invalidate captured review policy. No account, credential, provider permission or human approval is created by configuration.
- **AND** schema migration adds an empty reviewer table while preserving existing settings, grants, policies, cursors and mutation records; malformed, foreign or trigger-altered stores fail without repair.

#### Scenario: Page-bound comment observation
- **WHEN** a footer comment is read for a named page
- **THEN** current comment identity, page association and version-author metadata must be established by the provider response; historical versions, alternate containers and missing identity metadata are refused.
- **AND** optional body/revision projection preserves exact visible content and a fingerprint; version-author identity is not a claim of original authorship, human eligibility or approval.

### Requirement: Recoverable ticket materialization
Approved tasks SHALL become Jira issues/links with persistent task-to-issue mappings, per-batch review rechecks and explicit partial outcomes. General authorized issue CRUD SHALL remain available outside this workflow.

#### Scenario: Complete approved ticket preview
- **WHEN** `plan tickets-preview` observes a prepared review with current eligible human approval
- **THEN** it rebuilds the same native binding and records a complete immutable manifest with exact issue payloads, qualified existing parents, deterministic dependency batches and every included dependency edge. The explicit inward/outward relation selects the native label displayed on the dependent task toward its prerequisite; the manifest retains both task roles and exact native endpoint roles.
- **AND** plans with dependencies require an explicit dependency link-type ID and dependent relation direction during review preparation. Its current native ID/name/inward/outward semantics are bound alongside the exact task payloads; missing types or changed link semantics invalidate review. A plan without dependencies cannot silently consume an unused link mapping.
- **AND** the compact result retains the manifest digest, counts and complete local/cache effects. Apply authorizes local artifact publication only; no issue or link mutation is sent. The manifest is data and cannot restore approval or native send authority for later apply.

#### Scenario: Result publication fails
- **WHEN** issues are confirmed created but the result-page write fails
- **THEN** the mapping remains durable and resume republishes results without creating the issues again.

#### Scenario: Durable reviewed batch execution and resume
- **WHEN** `plan tickets-create` or `plan tickets-resume` applies an immutable preview under an explicit Confluence profile
- **THEN** a durable run intent binds the run ID, exact preview receipt/digest, original review and result-page title before resource writes. Step intents bind deterministic operation IDs, original logical inputs and exact native journal bindings. The same run cannot select a replacement preview or title.
- **AND** each dependency batch that has unfinished work obtains fresh original-preview approval validation. Issue creation imposes the approved native payload and metadata digest; directed links use confirmed issue mappings and the outward task's explicitly allowed profile. The original batch window and current captured policy are checked at native boundaries. A local or review failure stops subsequent work without undoing confirmed resources.
- **AND** complete terminal step outcomes are stored immutably. Resume reads prior outcomes and exact journal bindings before sending; confirmed mappings are preserved, failed/unknown claims never replay, and a confirmed journal without its full completion record remains explicit reconciliation data rather than silently successful workflow completion. Never-claimed steps may run after fresh revalidation.
- **AND** results publish as a separate child page of the reviewed source only after resource steps complete. Its stable content and separate operation identity allow confirmed page/result-artifact recovery without replaying issues or links. Concurrent runs for the same intent rely on exact one-winner native claims and immutable artifact publication; losers cannot overwrite outcomes. No multi-machine lock is claimed.

#### Scenario: Reviewed material constrains the actual create send
- **WHEN** ticket application passes approved issue material to the journaled create host
- **THEN** the newly frozen native body and complete material metadata fingerprint must equal the approved payload and fingerprint before any send claim and at the final claimed boundary. Current metadata includes the exact target, project/type labels, hierarchy, compiled field schemas, defaults, configuration and supported choice IDs/labels/disabled state; observation timestamps and provider routing URLs do not affect the fingerprint.
- **AND** these equality constraints grant no human approval, provider permission or send authority. Existing policy, verified actor/grant, original frozen freshness and durable claim checks remain required. A mismatch returns not-sent with cache effects retained and no resource journal claim.
- **AND** preview and creation use the same material capture/normalization, and dynamic policy checks bracket each metadata/account exchange rather than only context construction.

#### Scenario: Original preview approvals govern each batch
- **WHEN** application revalidates a stored ticket preview before a batch
- **THEN** the stored pack constrains a complete fresh live review and manifest compilation. The original approval identities, versions, body digests and policy must match; a newer otherwise valid approval does not substitute for the preview's original set. Source changes, withdrawal, material drift or inconsistent artifact provenance cannot return a validated batch.
- **AND** the returned live result cannot be deserialized from the stored pack. Captured policies and the original dual-clock observation window are checked before and after local policy reads. Later checks do not renew the window, and each subsequent batch requires another complete native observation. Revalidation creates no artifact, issue, link or send authority and retains independent credential effects on failure.

#### Scenario: Saved original for a reviewed issue step
- **WHEN** the ticket controller prepares a native issue-create step
- **THEN** it retains the exact canonical original under the deterministic native operation ID before locking credentials for that native step or executing its create; publication failure stops that step and preserves journal/source uncertainty independently.
- **AND** a dropped create response can be inspected with `operation reconcile --saved` under the original Jira profile and current recovery policy, without reconstructing hidden controller input or replaying the create. The controller does not turn that read-only investigation into run completion. Existing link/page recovery and legacy source gaps remain explicit.

#### Scenario: Restore a retained confirmed issue completion
- **WHEN** `plan tickets-record-completion` imports the original full native issue-create envelope after local outcome publication failed
- **THEN** explicit apply and the unchanged existing run/preview/title are required; the task-derived step intent, full native binding, confirmed no-job journal, exact issue identity and complete successful native receipt must agree before immutable publication. Unknown/refused/foreign/incomplete evidence and conflicting existing outcomes are rejected.
- **AND** the operation accesses no credentials or provider, changes no native journal and grants no replay or human approval. It restores the supplied completion as local evidence, not proof of provider authenticity. The outcome name and publication state remain visible on failure; a post-publication journal change is a conflict with retained artifact evidence. Resume still revalidates original approval for unfinished work. Link/page adoption, absent envelopes and changed previews are not silently resolved.

#### Scenario: Newly reviewed reuse after an uncertain prior create
- **WHEN** a replacement plan explicitly names an existing numeric issue and the earlier create operation for an included task
- **THEN** those decisions are visible in normal plan rendering/import, included in review bindings and manifests, and qualified against the native issue identity/project/type before approval and each unfinished batch. A new reviewed run records reuse rather than issuing another create for that task.
- **AND** reuse preserves the complete original journal without upgrading attribution or replay rights. The original operation must exist in the same project and represent an uncertain or matching confirmed create; missing/foreign/refused or contradictory resource evidence is rejected. The new result labels reused identities and preserves original uncertainty independently of completion of the newly approved workflow. Old plans without reuse fields retain their previous rendering, serialization and behavior.
- **AND** old runs remain bound to their original previews; replacement review uses a new run, with explicit original-operation provenance rather than rebinding or resetting old state.
