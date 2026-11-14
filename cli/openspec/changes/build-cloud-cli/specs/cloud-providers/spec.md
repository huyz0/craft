# Cloud provider modules

## ADDED Requirements

### Requirement: Isolated provider modules
The core SHALL not depend on concrete providers or runtime infrastructure. Provider modules SHALL register native operations through a versioned contract and host-mediated auth/transport.

#### Scenario: New provider registration
- **WHEN** a new compiled provider is added
- **THEN** existing providers and core contracts continue working and provider-specific routes/scopes do not become global assumptions.

#### Scenario: Native parameter names
- **WHEN** a declared native route uses provider path/query names such as `comment-id` or `body-format`
- **THEN** safe literal names remain usable without URL interpolation, while credential-bearing query names remain refused in underscore and hyphen spellings.

### Requirement: Scoped capability discovery
Capabilities SHALL distinguish implementation, policy, granted scope, tenant support and live verification, including unknown states. Discovery SHALL not use destructive probes.

#### Scenario: Scope without permission
- **WHEN** a credential has a scope but lacks an actual project permission
- **THEN** preflight reports the distinction or uncertainty and the mutation cannot bypass the provider's denial.

#### Scenario: Pinned provider contract inventory
- **WHEN** the development checker compares the researched command catalog with pinned provider specifications
- **THEN** every catalog command has explicit route variants, native input/output references, documented scope alternatives and permissions, lifecycle/recovery semantics and owning work; missing/deprecated/unverified routes cannot become advertised runtime capabilities.

### Requirement: Native Jira resources
Jira operations SHALL implement catalog Core issue, metadata, project-read, comment, link, transition and attachment behavior using native semantics and explicit field validation.

#### Scenario: Attachment metadata and settings reads
- **WHEN** a separately allowed attachment get, issue-field attachment list or site settings read executes
- **THEN** fixed native GET routes and current actor/grant/site authority apply. Metadata omits returned URLs, author emails/display names and arbitrary properties; normalized IDs retain integer-or-canonical-string native forms. Filename metadata never becomes a local destination or replay identity. Metadata fingerprints do not claim file-content digests.
- **AND** issue lists check the exact source issue and bound complete visible attachment arrays. Local cursors buffer the observed remainder without inventing native offsets, refetching buffered records or proving current native visibility. Missing/null metadata and settings stay unknown; native errors, duplicate IDs, inconsistent source or over-limit arrays fail without truncation.
- **AND** explicit read allowances require a new policy format; older policies inherit no attachment access. Metadata/settings do not authorize downloads, uploads or deletion, and current attachment access can still depend on issue security and private-comment restrictions.

#### Scenario: Issue-bound comment writes
- **WHEN** a comment create, update or delete is prepared
- **THEN** exact numeric issue/project identities and current issue-scoped Browse plus Add, Edit own/all or Delete own/all permissions are required. Ownership compares the original author with the authenticated actor, never the last editor. A local own-only intent refuses another or unknown author even if native all permission exists.
- **AND** updates/deletes compare a required fingerprint of current complete comment evidence before and after preparation. This is a bounded observation, not native compare-and-swap; native permissions and concurrent writes still apply at send. Fresh preparation is bound to operation, grant and actor and expires under either clock.
- **AND** typed bounded ADF supports paragraphs, headings, quotes, lists, code blocks, rules, text, hard breaks and common marks. Unsupported rich content fails locally rather than being dropped. Visibility changes explicitly select project access, stable group identifier or role name; omitted update fields preserve their native values and explicit null is rejected.
- **AND** documented acknowledgments retain addressed comment identity independently of optional returned body/privacy verification. Unrecognized/lost outcomes retain uncertainty without a second send; output never invents a new ID or attributes another comment's returned content to the addressed write.

#### Scenario: Public issue actions retain frozen host intent
- **WHEN** issue update/assign/unassign/delete or transition apply enters public invocation
- **THEN** exact typed intent, expected state and operation identity reach the existing journaled host, local malformed or incomplete inputs fail before private state, and native scope, current issue evidence and separate project-operation policies remain required.
- **AND** deletion remains a permanent effect with explicit refuse/cascade subtask intent; assignment does not imply unassignment, read authority does not imply mutation authority, and no generic handler bypasses the verified host or restores replay permission.

#### Scenario: Public context reads preserve internal authority boundaries
- **WHEN** project list/get/statuses, field list/search, edit metadata, transition list or contextual permission checks enter public discovery and invocation
- **THEN** their typed schemas, flags, projection and host dispatch agree, malformed inputs fail before state/credentials, and each read retains its existing explicit policy, service scope and current-user checks.
- **AND** exposing transition listing does not expose transition execution or issue mutation commands; read results and local capability discovery do not establish write authority or live tenant qualification.
- **AND** public policy administration can configure the existing field/site/per-project read records. Canonical continuation origins are explicit for site policies, per-project updates require the existing whole-record revision and same actor, and complete receipts preserve sibling project allowances and listing enablement.

#### Scenario: Native link reads and bounded local continuation
- **WHEN** an explicitly allowed link get, issue-link list or link-type catalogue read executes
- **THEN** exact current native IDs and endpoint roles are retained, unrelated issue content and native routing URLs are omitted, and requested fields project both data and schema. Link lists validate their source issue association; opaque local continuation preserves the observed native field remainder without inventing provider offsets or completeness over hidden relations.
- **AND** get/list/types have separate explicit read allowances. Captured policy, grant, actor, scope and cursor binding are checked before auth, at native/cache boundaries and before output. Cached observations cannot establish current write authority; failures retain credential effects.

#### Scenario: Exact link deletion and read-only reconciliation
- **WHEN** deletion applies an expected native link ID, type and endpoint pair
- **THEN** both exact endpoint projects require independent deletion allowance, current link/issue identities are bracketed, Browse projects is required on both and Link issues on at least one endpoint, and one permanent comment-free DELETE is durably claimed. Only documented 200/204 acknowledgments confirm the addressed deletion; unknown attempts cannot retry, and no issue deletion is implied.
- **WHEN** an original link write is reconciled
- **THEN** its complete original native logical binding must match the local journal and independent recovery allowances on both projects before authentication. Observed matching link IDs or absent/invisible evidence describe current state only, never upgrade attribution, modify journal outcomes or authorize replay. Initial/latest records, drift, partial observations and cache effects remain explicit.
- **AND** newly recognized read, deletion and recovery operations cannot become valid in older serialized policy formats. Explicit revision-checked configuration is required; unsupported-format fixtures use an out-of-range sentinel independent of the next planned version.

#### Scenario: Exact directed issue-link creation
- **WHEN** `jira link create` applies an explicit numeric inward/outward issue pair and expected native link type under one service profile
- **THEN** independent explicit link allowances for both exact projects must identify the same installation/grant/actor/site. Legacy mutation records grant no new link authority; only an explicit format-three link update can enable it.
- **AND** current link type and both issue/project identities are read and rechecked, with issue-specific Browse projects permissions on both endpoints and Link issues permission on the outward endpoint. Missing scope, feature, visibility, permission, expected label or stable project evidence refuses before a send claim. Native endpoint roles are used exactly as supplied, without inferring direction from labels.
- **AND** one exact comment-free POST is durably claimed and never automatically retried. Only documented 201 confirms the requested relation; no native link ID or proof of newly created work is invented. Documented refusals remain failed, while unqualified/lost outcomes remain unknown. Current/prior journal records and credential effects survive failures, cancellation and unknown local acknowledgment. Claimed sends recheck captured policy and original dual-clock freshness before exchange.

#### Scenario: Issue-specific assignment and explicit unassignment
- **WHEN** an internal assignment or unassignment is prepared against an exact numeric issue and explicit expected state/assignee
- **THEN** narrow issue observations bracket preparation without edit-screen access; assignment requires exactly one active matching issue-specific assignable account under its separate user-read scope, while unassignment compiles only explicit null under separate local authority.
- **AND** omitted expectations, names/emails/default sentinels, ambiguous/inactive accounts, wrong issue/project/principal, observed drift, expired or backward-clock evidence prevent preparation; no serialized result recreates live authority.

#### Scenario: Action candidates and versioned explicit policy
- **WHEN** assignment, explicit unassignment or deletion enters typed candidate dispatch
- **THEN** required expected-assignee/subtask objects agree across flags, JSON and schema, omitted/null intent fails before state, and normal exact profile/site/project resolution selects an independent explicit allowance, including separate delete-all-subtasks authority.
- **AND** format-one records remain readable only for edit/transition with no rewrite; newly recognized action strings in those legacy payloads remain invalid. Trusted format-two updates require apply/preflight/revision CAS without a database schema migration. Preflight/conflict/pre-commit cancellation preserves the original row; unknown commit acknowledgment requires exact-row inspection and does not imply rollback or authorize blind retry.
- **AND** complete captured policy is checked through auth/read/preparation/final claimed-send boundaries, conservative uncertainty and independent credential receipts survive refusal, complete dispatch preserves recovery output, and no public registration or live authority is inferred.

#### Scenario: Explicit issue hierarchy deletion intent
- **WHEN** an internal delete selects refuse or delete-all-subtasks behavior
- **THEN** narrow bracketed issue reads compare expected state and bounded unique visible child IDs, refuse mode retains native deleteSubtasks=false protection, and delete-all mode explicitly includes unobserved or concurrently added children without claiming complete enumeration or server compare-and-swap.
- **AND** assignment, unassignment, delete and cascade authority remain independent from existing edit/transition/create authority; preparation sends no mutation, and only the documented operation-specific acknowledgment confirms the addressed issue operation, without inventing affected-child receipts or replay authority from later matching/absent state.

#### Scenario: Project details and grouped status context
- **WHEN** the internal project-get/status host prepares an exact numeric-ID or case-sensitive-key target
- **THEN** closed fields and complete recovery output are checked before auth, the declared native read supplies locally projected bounded metadata, and numeric/key identity qualification does not inherit issue alias semantics or mutation authority.
- **AND** the unpaginated status array is buffered as whole issue-type groups with immutable continuation and original observation, duplicate group/within-group status IDs fail qualification, and buffered resumes preserve the exact selector/actor/grant/fields without refetching statuses.
- **AND** unselected user/body/link data is ignored subject to physical response bounds, missing/null values remain distinct, dynamic policy changes before final output suppress the project/page while retaining independent cache receipts, and public/configured authority is not widened.

#### Scenario: Project-context candidates with explicit per-project permission
- **WHEN** typed project-get/status flags or structured arguments select an explicit, repository or default project
- **THEN** invalid local input/projection/cursor/recovery output fails before state access, exact profile/site rules apply, and only an explicit enabled permission for that literal project and operation authorizes the prepared read.
- **AND** older list-only records do not grant get/status access; format-two permission updates require apply/preflight/revision CAS, preserve other intent and forbid transferring enabled permissions to a different actor.
- **AND** listing enablement stays independent, corrupt/duplicate/foreign/unsupported permissions fail without repair, captured changes suppress native/cached results while retaining cache receipts, and public registration remains unavailable pending qualification.

#### Scenario: Field-selected issue reads
- **WHEN** the internal issue-get or enhanced-search host prepares a read
- **THEN** effective selected fields are validated before auth and mapped to explicit native field masks, default bodies/unrelated user metadata are not fetched, and omission/null/not-returned semantics survive normalization and projection.
- **AND** numeric get requests match the exact returned ID, key aliases retain requested and actual identities, history updates are disabled, and absent/invisible results are not treated as mutation or replay authority.
- **AND** a mismatched compiled field plan fails before native invocation; unselected reference members and unneeded field wrappers are ignored, selected missing/null members remain distinct, and an explicitly selected description is bounded inert ADF or legacy plaintext with no variant-specific child projection.

#### Scenario: Enhanced search token and reconciliation binding
- **WHEN** a JQL search resumes through short/empty pages or after same-grant token renewal
- **THEN** each invocation performs at most one native search, preserves buffered remainder and opaque continuation, and retains the exact JQL, field mask, native size, reconciliation IDs and actor/site/grant binding.
- **AND** contradictory terminal metadata, repeated tokens, duplicate issue IDs, malformed selected data or nonempty/malformed experimental provider warnings prevent a successful successor page. Search remains live; reconciliation is not a global snapshot or proof of absence.

#### Scenario: Issue-read candidates and separate configured authority
- **WHEN** typed issue-get/search flags or structured arguments enter candidate dispatch
- **THEN** invalid local input/projection/recovery budgets fail before opening state, the existing profile/site precedence resolves context once, and a separate private enabled record supplies exact grant/actor and allowed read operations.
- **AND** project-discovery records do not authorize issue access; absent/disabled/changed or operation-excluding issue configuration refuses auth or native/cached output, and complete result/recovery receipts retain the original request identity.
- **AND** a configuration change during final observation/output work or cursor persistence is rechecked before releasing a successful result; no partial issue/page is released on that refusal, while independent credential-cache effects remain visible.

#### Scenario: Issue-read configuration migration and revision control
- **WHEN** a trusted host creates/updates issue-read configuration or opens an older private schema
- **THEN** apply/preflight and expected-revision CAS protect changes, the record has exact installation/profile/grant/site/actor binding and explicit closed operations, and atomic schema-six migration preserves existing profiles/settings/policies/journal/cursors without inferring issue authority.
- **AND** malformed/foreign/duplicate rows, unknown operations, conflicting tables or triggers fail without automatic repair or partial migration.

#### Scenario: Contextual current-user permissions
- **WHEN** the internal permission GET view checks explicit bounded unique keys in global, project, issue or comment context
- **THEN** native queries contain only the validated requested context and keys, output retains the endpoint's context-dependent project-permission semantics and requested selector without claiming resolved resource identity, and comment context permits only BROWSE_PROJECTS.
- **AND** one ordered result per requested key preserves missing entries/flags as unknown, native false stays false, unexpected/mismatched/malformed permission entries and warnings/errors fail without partial results, and selected labels remain bounded inert metadata.
- **AND** private operation/principal/dual-clock observation cannot be rebuilt from cached output or authorize writes; permission flags do not replace issue visibility, workflow/screen/value, review or app-access checks.

#### Scenario: Permission candidate authority and recovery
- **WHEN** typed permission flags/JSON enter candidate dispatch
- **THEN** strict selector/key/projection/recovery-budget validation precedes state/auth, ordinary profile/site precedence and a separate explicit operation allowance apply, and immutable buffered/replayed/oversized results preserve context, missing decisions and full cache/page recovery.
- **AND** the candidate JSON schema requires an explicit nonempty permission list; input validation cannot accept omission and defer its rejection to execution.
- **AND** existing four-operation records grant no permission-check allowance, schema seven/settings/policy format one remain unchanged, and captured-policy changes suppress native/cached output while retaining renewal receipts and original request identity. Public parser/catalog/provider activation remains unavailable pending qualification.
#### Scenario: Issue-bound edit and transition metadata
- **WHEN** the internal host reads issue edit fields or transitions
- **THEN** narrow issue reads resolve and bracket the metadata call on the exact numeric issue ID, changed issue context is refused, and selected bounded native metadata preserves required/operation/schema facts without executing returned schema, configuration or links.
- **AND** transitions use actual IDs, field expansion is selection-derived, administrative bypass parameters are unavailable, empty results do not establish absence/permission, and private live context refuses changed operation/principal, expired age or backward clocks. Read output remains explicitly non-atomic and non-authorizing.

#### Scenario: Typed issue metadata candidates and explicit operation allowances
- **WHEN** strict edit-metadata or transition-list flags/JSON enter internal candidate dispatch
- **THEN** input/filter/projection/recovery-budget validation precedes state/auth, normal profile/site precedence resolves one target, and the site-bound issue-read record must explicitly enable that operation before an exact issue-bound host executes.
- **AND** budget preflight reserves the longest resolution label and serialized escaping of bounded opaque update evidence, refusing one byte below the complete recovery envelope before state/auth.
- **AND** existing get/search records grant neither metadata operation, other resource records supply no allowance, full captured-record changes suppress native/cached metadata and continuation while preserving renewal receipts and request identity, and public discovery remains unchanged pending qualification.

#### Scenario: Frozen partial edit and transition preparation
- **WHEN** internal write preparation receives a numeric issue target, exact expected issue state and separate typed edit or transition changes
- **THEN** private fresh metadata must match the read operation, write target and principal; the expected ID/key/project/type/status/update state must match, unrelated fields remain absent from the body, and unsupported preconditions or arbitrary status/project/type/parent/account/history/property maps are refused.
- **AND** edit omissions preserve existing values, explicit clears remain distinct and cannot clear required fields, only qualified field families and actual returned option IDs are accepted, and label replacement cannot overlap typed add/remove operations.
- **AND** transitions use an actual available transition ID and expected destination status, require expanded screen metadata, and required omissions need a qualified native default or a fresh bounded current-value read under unchanged issue state. Missing/null/empty or unsupported current-value evidence cannot silently satisfy a required field.
- **AND** the frozen native call binds the write digest, principal and original dual-clock freshness, routes only to the resolved numeric ID, exposes no admin bypass or notification suppression, and neither preparation nor later matching postconditions authorize replay of an uncertain send.

#### Scenario: Journaled issue mutation host and completion
- **WHEN** an internal edit/transition host executes under explicit project/operation/grant/actor policy
- **THEN** pure input/apply/output validation precedes state/auth, sealed service access supplies exact actor/grant/site and read/write scope proof, fresh native metadata qualifies the frozen request, and a durable single send claim precedes the mutation exchange.
- **AND** access, policy and original freshness are rechecked after the claim; an expired/revoked/cancelled claim remains uncertain without a send or replay. Started/terminal/foreign operation records refuse before authentication or metadata reads and retain prior evidence.
- **AND** documented 204 confirms the operation without claiming a fresh final issue state; operation-specific documented refusals remain failures, other/lost responses remain uncertain, and no observed postcondition grants attribution or replay.
- **AND** completion retains remote evidence, local persistence disposition, request/rate metadata and independent credential-cache receipts through projection, cancellation and persistence failure. No public parser, policy allowance or cloud capability is enabled by this host.

#### Scenario: Mutation candidates and per-project private authority
- **WHEN** strict edit/transition flags or structured input enter candidate dispatch
- **THEN** exact issue/expected-state/changes, apply/operation identity, projection and complete recovery budget are validated before private state or auth; normal profile/site resolution supplies context and a separate enabled exact project record must allow the selected operation.
- **AND** create/read policies grant no edit/transition access; captured changes or revocation stop reads/send while retaining independent cache and mutation receipts, and dispatch preserves the original request identity and complete recovery result.
- **AND** schema-eight migration creates an empty private policy table atomically without changing existing profiles, policies, grants, journal or cursors; malformed/conflicting schema and triggers fail without repair or partial migration. Settings and policy payload format one remain unchanged.
- **AND** policy administration requires apply/preflight/revision CAS, closed canonical operations, exact installation/profile/grant/project/actor binding and disablement before actor transfer. Independent projects under one profile retain separate revisions and allowances. Public activation remains unavailable pending qualification.

#### Scenario: Additive issue-read allowances preserve existing state
- **WHEN** metadata allowances are configured through the existing trusted issue-read setter
- **THEN** explicit apply, receipt preflight and revision CAS protect a closed canonical operation list without changing schema seven, settings format one or existing get/search encodings and cursors.
- **AND** an enabled record must be disabled before actor/target transfer, disabled records still obey profile target binding, and malformed/foreign/unknown/duplicate/unsorted records fail closed without repair.

#### Scenario: Buffered page context
- **WHEN** native metadata has an associated issue observation
- **THEN** bounded inert context is included in normalized-page integrity and preserved with each buffered/replayed/empty or oversized page across reopen, while existing absent-context pages/cursors retain their encoding and semantics.
- **AND** wrong-shaped/oversized context is refused, only declared typed host output exposes it, and it cannot construct private mutation authority or replace current access checks.

#### Scenario: Bounded field discovery through verified access
- **WHEN** the internal field-list/search host executes or resumes an exact site/filter/projection binding
- **THEN** verified scope/grant/actor and current policy protect native and buffered output; malformed local input, unsupported fields and insufficient complete recovery budgets fail before auth, and one invocation fetches at most one native resource page.
- **AND** list coverage remains visibility/configuration-dependent, search coverage remains Classic-project fields, and completion does not establish global completeness or create/edit applicability. Normalized complete records preserve selected missing/null/not-returned metadata while schema/plugin/configuration values remain inert data.

#### Scenario: Candidate field discovery and configured authority
- **WHEN** typed field-list/search flags or structured arguments enter candidate dispatch
- **THEN** local filter/projection/recovery-budget validation precedes state/auth, existing profile and exact-site precedence applies, and only separate private configuration supplies grant/actor/continuation-origin/operation authority.
- **AND** missing, disabled, changed or mismatched records suppress native and buffered output while retaining independent credential-cache receipts, the complete envelope and original request identity. Public parser/catalog/provider discovery remains unchanged pending qualification.

#### Scenario: Field-read configuration persistence and migration
- **WHEN** a trusted host creates/updates a field-read policy or opens a previous private schema
- **THEN** explicit apply, receipt preflight and expected-revision CAS protect the exact installation/profile/grant/site/actor binding and closed operations; an enabled record must be disabled before transferring actor, target or continuation origin.
- **AND** atomic schema-seven migration preserves settings format one, all earlier records and cursors without granting field access. Malformed/foreign/unknown/duplicate rows, conflicting tables and unexpected triggers fail without repair or partial migration.

#### Scenario: Native field-search arrays and continuation
- **WHEN** field search uses typed ID/type/project-ID filters or advertises a next link
- **THEN** filters use declared repeated GET parameters, selected fields derive only documented expansions, and exact bound arrays/scalars survive opaque local continuation. Changed/reordered/added filters, duplicate IDs, unsupported inputs, malformed pages and ambiguous terminal metadata are refused without guessing offsets from totals.
- **AND** an explicit field-search site-link parser accepts only its exact configured origin/path/declarations, preserves advertised clamped page size solely for validation, and rebuilds authenticated requests at the original Cloud-ID gateway. Existing project-site parsing and operation authority remain unchanged.

#### Scenario: Bounded project discovery through verified access
- **WHEN** the internal project-list host executes or resumes an exact site/query/projection binding
- **THEN** it uses the ordinary verified actor/grant/scope access, serves buffered records without a project fetch, fetches at most one native project page otherwise, and preserves complete page/error/cache-update evidence under byte limits.
- **AND** unknown fields, wrong target, invalid native page size, unsupported projection and insufficient recovery output fail in preparation; project visibility is not reported as mutation permission. Public parser/catalog discovery remains unchanged until qualification permits activation.

#### Scenario: Candidate project-list arguments and configured context
- **WHEN** typed project-list flags or structured arguments enter candidate dispatch
- **THEN** malformed local input, unknown projection and insufficient recovery output fail before opening state; existing profile precedence and one-time repository hints select an exact site, while private configuration supplies grant, actor and continuation-site constraints.
- **AND** missing, disabled, changed or mismatched configuration refuses execution; explicit profile selection bypasses repository hints. Complete page/error/cache-update output and the original request identity survive dispatch and encoding. Public parser/catalog/provider registration remains unavailable pending qualification.

#### Scenario: Project-read configuration persistence
- **WHEN** a trusted host creates or updates a private project-read record
- **THEN** explicit apply, bounded output preflight and expected-revision CAS protect the update, identity fields are checked against the selected service profile, and concurrent writers retain one winner.
- **AND** migration from previous private schemas preserves existing state atomically; corrupt/foreign/duplicate/unknown records, conflicting tables and unexpected triggers are refused without automatic repair or implied remote authority.

#### Scenario: Project-search site links remain data
- **WHEN** a project page returns an explicit next link at the configured Jira site's exact origin and fixed project-search path
- **THEN** only a validated increasing offset enters continuation, fixed filters and page size cannot change, and the actual authenticated request is rebuilt at the originally bound Cloud-ID gateway. Foreign origin/path, credentials, fragment, duplicate parameters, malformed encoding or inconsistent terminal metadata fail without a request to the link.
- **AND** short/empty pages may continue, changing totals do not imply completion and ambiguous missing continuation metadata is refused rather than guessed.

#### Scenario: Candidate create argument dispatch
- **WHEN** typed create flags or a structured argument object enter candidate dispatch
- **THEN** local validation/projection/output preflight precede state/auth access; profile and project selection retain explicit/repository/default precedence, the trusted create policy supplies actor/grant authority, and the original request identity and complete handler envelope survive dispatch/output.
- **AND** omitted/null/text values are not silently conflated; custom field maps and label arrays use explicit structured input. The candidate remains absent from public parser/catalog/provider discovery until review and live qualification permit enablement.

#### Scenario: Created issue with incomplete or secondary failure evidence
- **WHEN** a create-issue response contains a valid issue ID but malformed display metadata or failed/unknown nested transition/watcher results
- **THEN** the provider retains the qualified created issue reference separately from response interpretation errors, reports partial/uncertain secondary outcomes, ignores returned URLs as routing authority and never retries the create automatically.
- **AND** completion preserves that reference when local journal recording or output budgeting fails; documented creation refusals remain distinct from unqualified statuses and unknown transport outcomes.

#### Scenario: Qualified parent and assignee
- **WHEN** a subtask create selects a parent or a create supplies an assignee account ID
- **THEN** parent evidence must identify the exact visible issue in the selected project with a non-subtask issue type, and assignee evidence must positively return that exact active account from the project's assignable-users endpoint under its separately declared scope.
- **AND** missing/ambiguous/moved/cross-project/inactive/stale or mismatched principal evidence prevents the create; account names/emails, empty search results and explicit unassignment do not become guessed account or policy authority. Both metadata and account evidence are rechecked at the final claimed-send boundary.

#### Scenario: Workflow transition
- **WHEN** an agent supplies a status name as an editable issue field
- **THEN** the request is refused and a supported transition must be selected from actual metadata.

#### Scenario: Complete, bound create metadata
- **WHEN** the provider prepares a create using project, issue-type and field metadata
- **THEN** it validates the exact project, reads bounded complete pages without following returned URLs, rejects duplicate/inconsistent/no-progress pages and binds the resulting non-importable context to target, issue type, supplied verified principal and bounded dual-clock freshness.
- **AND** required fields without reported defaults, unsupported plugin schemas, read-only fields, unknown input, invalid option IDs and wrong/stale context fail before mutation. Project/type fields are derived from the bound context, description text is encoded as ADF, and native metadata type labels cannot become arbitrary executable JSON Schemas or network authority.

#### Scenario: Explicit issue type and composed host
- **WHEN** the internal host prepares a create invocation
- **THEN** the typed input requires an explicit numeric issue-type ID, bound into its logical mutation identity and matched exactly to fetched metadata; malformed basic input, unsupported preconditions/cursors, missing apply, disabled policy and insufficient recovery output fail before authentication.
- **AND** exact trusted project/grant/actor policy and all required operation scopes precede resource reads, the grant lock remains held through cached execution, and every later failure retains any confirmed or uncertain credential-cache update separately from the resource outcome. A reused or already-started operation is refused before resource reads and is never automatically replayed.

### Requirement: Native Confluence resources
Confluence operations SHALL implement catalog Core pages/search/spaces/comments/files/content badges using appropriate v1/v2 adapters and expected revisions.

#### Scenario: Bounded Confluence context reads
- **WHEN** internal space/page get/list or page CQL search adapters execute
- **THEN** exact Confluence Cloud site authority and canonical numeric IDs select compiled v2 space/page or v1 search routes. Space keys and numeric IDs remain separate; key-filtered discovery cannot guess or silently select a different space.
- **AND** selected fields derive body/description representations and search expansions. Metadata-only reads do not claim complete bodies, unavailable selected fields remain explicit, mismatched representations/identities fail, and content and returned links remain inert data.
- **AND** every invocation performs at most one bounded GET. Continuation is normalized by the host, preserves the original route and fixed filters, and stores only a bounded opaque cursor (or documented search position). Empty/short pages can continue; duplicates, malformed continuation or non-page search results cannot be silently dropped. Visibility-filtered results and 404 never prove absence or mutation authority.

#### Scenario: Typed Confluence read commands and profile authority
- **WHEN** internal Confluence space/page get/list/search flags or structured input enter dispatch
- **THEN** strict typed input, selected fields, cursor syntax and complete recovery-output preflight are validated before private state or credentials. Normal explicit/repository/active profile and site precedence resolve one Confluence Cloud service target without deriving authority from arguments.
- **AND** a separate persisted read record binds installation/profile/grant/site/actor and explicitly enables the requested operation. Jira/create/recovery policies confer no Confluence read permission; the complete captured record is rechecked through authentication, native reads, buffered replay and final output. Changed or revoked authority suppresses content/continuation while retaining credential-cache receipts.
- **AND** trusted administration requires apply, receipt preflight, revision CAS and disabling enabled identity before actor/target transfer. Schema-ten migration creates an empty table atomically, preserves earlier state and payload formats, and refuses malformed/foreign rows, conflicting tables and triggers without repair. Public activation remains separately qualified.

#### Scenario: Human edits a page
- **WHEN** a page has changed since the agent's expected version
- **THEN** the update fails as a conflict instead of force-overwriting the human edit; publication and badges do not mean approval.

#### Scenario: Normal plan-page publication and revision-bound preparation
- **WHEN** an internal page create or update is prepared
- **THEN** strict input selects one explicit storage or atlas_doc_format body and a numeric space. Create placement explicitly selects the space root, its observed homepage, or an observed current parent in that space. Creation pins status current and omits live-document subtype, private and embedded modes.
- **AND** updates require the expected published version and canonical snapshot digest covering page/space identity, title, version and exact selected body representation/value. Fresh current-page evidence must match both; omitted title/body preserve the observed value, explicit null and empty changes fail, and the emitted PUT increments the int32 version without supplying owner, space or parent changes.
- **AND** generic provider execution cannot send writes. A verified host must bind fresh evidence to the site/grant/actor and hold existing journal/send controls. Update requires explicit `editor_draft_policy: native_reconciliation_may_overwrite` intent because native publication can merge or overwrite an unpublished draft. Missing/null/unknown intent fails before state. The intent is digest-bound but is not a native API parameter or a force-retry switch.
- **AND** update receipts retain the selected draft policy and `editor_draft_preservation_verified: false` on success, refusal and uncertain outcomes. Published version/digest checks never imply editor-draft preservation, absence or lock ownership. Live editor testing remains separately unrun until observed; public support describes these native semantics without promising draft protection.
- **AND** only documented HTTP 200 with matching page/space/current status, title and version confirms publication. Known returned identities survive incomplete success evidence; undocumented successes, lost responses and rate limits remain unknown without automatic retry. Documented version conflict is a refusal. Provider URLs and error text cannot become routing or diagnostics.

#### Scenario: Typed page-write commands and explicit space authority
- **WHEN** page create/update flags or structured input enter internal dispatch
- **THEN** typed body, placement, expected revision, operation identity, apply and complete receipt capacity are checked before private state or authentication. Normal profile/site/space selection resolves the target; arguments cannot supply grant or actor authority.
- **AND** a separate per-profile/per-space write record explicitly grants create and/or update to the exact installation/grant/actor/space. Reads and other providers grant no page-write authority. Capture and recheck the complete record before authentication, around native reads and after the committed send claim.
- **AND** administration requires apply, output preflight and revision CAS, with disablement before actor/target transfer. Schema-eleven migration creates an empty write table without changing existing policy payloads, credentials, journals or buffered cursors. Conflicting tables/triggers and malformed or foreign rows fail without repair.
- **AND** page get can return an explicitly selected revision fingerprint without returning the complete body. This performs one current-page GET with one body representation; missing body/metadata cannot produce a fingerprint. Page lists/search cannot supply this write-precondition evidence. Publication responses advertise complete resource/cache/persistence/recovery receipts and never grant replay. Public activation requires independent review and composed-host checks; live editor-draft qualification remains separately identified.

Confluence SHALL expose separately authorized paged root and reply reads, with bounded metadata/body projection and opaque host cursors. A single collection page SHALL NOT imply complete review coverage or human approval.

#### Scenario: Reply models omit page identity
- **WHEN** reading replies for an exact parent and page
- **THEN** a current authenticated parent read establishes page association before fetching children, and every child must name that parent. Conflicting native page/container identity is refused.

#### Scenario: Native continuation evidence
- **WHEN** a comment collection supplies Link-header or body continuation
- **THEN** the fixed-route normalizer validates it; both locations must agree when present. Unknown header observation, malformed or conflicting metadata, repeated cursors and changed filters refuse terminal coverage. Observed absence in both locations ends only that collection.

### Requirement: Privileged and conditional operations
Extended/admin/purge/archive/restriction operations SHALL have separate capability bundles and explicit lifecycle/async/tenant semantics. Unsupported or unverified operations SHALL remain unavailable.

#### Scenario: Permanent space deletion
- **WHEN** a caller requests permanent space deletion
- **THEN** the exact target/effect/acknowledgment is checked and an accepted remote job is not reported as completed.

### Requirement: Additional cloud clients
Bitbucket and Trello SHALL use their own researched routes, auth strategies, scopes and resource semantics. Support SHALL be advertised per qualified operation, not inferred from another Atlassian product.

#### Scenario: Trello card move
- **WHEN** a Trello adapter changes a card's list
- **THEN** it uses validated Trello list/card semantics and does not reuse Jira transition IDs or cloud-ID routing.

#### Scenario: Bounded Jira comment reads
- **WHEN** explicit `jira.comment.get` or `jira.comment.list` reads a numeric issue/comment under a separately enabled site read allowance
- **THEN** one native comment GET preserves route-bound issue identity, native author account IDs, visibility restrictions and service-desk privacy flags without copying emails, display names or returned URLs. Body output is explicitly selected; missing and unrequested bodies are distinguished. A revision fingerprint binds available native body and relevant privacy/author/time evidence.
- **AND** list performs at most one resource page per invocation with bounded page size, native created ordering and validated offset/size/total metadata. Opaque remainder/cursor binds input/projection/site/actor/grant, rechecks current authority on buffered reads, and does not imply a collection snapshot or absence of invisible comments. Invalid local inputs/projection/cursors/budgets fail before credentials or network.

#### Scenario: Explicit Jira attachment download and independent export
- **WHEN** a verified Jira service actor downloads an explicit numeric attachment ID with a fresh operation-specific site allowance
- **THEN** a compiled GET content route fixes redirect=false, accepts only complete bounded unencoded bytes and publishes an immutable named artifact after any expected SHA-256 matches. Metadata allowances and older policy formats grant no download authority.
- **AND** an explicit existing output root and relative destination govern guarded atomic export, create-new by default with replacement only on explicit request. Provider filenames and content URLs supply neither path nor routing authority. The complete selected private state root is excluded from output ancestry, including when output begins at an ancestor directory.
- **AND** artifact publication and export dispositions are separate, confirmed artifact receipts survive export failure or late policy/cancellation refusal, and uncertain effects never cause an automatic refetch or export retry. JSON stdout contains receipts, never binary bytes.

#### Scenario: Prepared Jira attachment upload context
- **WHEN** a host prepares one immutable snapshot for an exact numeric issue and project under a verified principal
- **THEN** issue-project reads bracket stable enabled attachment settings and issue-scoped BROWSE_PROJECTS/CREATE_ATTACHMENTS decisions; missing/changed/denied evidence or a size above the native limit refuses preparation before a send claim.
- **AND** a sealed preparation binds the full snapshot input, operation and principal with dual-clock freshness. The multipart encoder independently matches its actual snapshot, issue route and framing metadata to that operation before claiming.
- **AND** only the documented HTTP 200 attachment array supplies acknowledgment evidence. Known bounded IDs survive malformed or unexpected optional metadata, unexpected multiple records remain visible, and no response claims a remotely verified byte digest or permission to replay an uncertain upload.
