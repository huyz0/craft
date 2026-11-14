# Secure authentication

## ADDED Requirements

### Requirement: Explicit profiles and grants
Profiles SHALL resolve trusted provider/tenant/actor configuration explicitly. Multiple profiles sharing a grant SHALL share refresh ownership and locks, without confusing requester, executor and reviewer.

#### Scenario: Persisted create policy
- **WHEN** the candidate create command resolves a profile/project
- **THEN** a separate private-state policy must explicitly enable that exact service profile, issued grant, project and expected actor; absent, disabled, malformed, locally mismatched or changed policy prevents acquisition, and verified actor mismatch prevents resource reads/writes.
- **AND** local policy writes require explicit apply and expected-revision compare-and-swap with preflight before commit; repository hints never define policy. The command rechecks the same policy revision before authentication/resource reads and after the send claim, without claiming cross-machine fencing or an independent human approval from local state alone.

#### Scenario: Explicit local permission administration
- **WHEN** a setup command reads or changes a saved Confluence read/write or Jira create/issue-read/mutation/recovery record
- **THEN** it requires an explicit trusted profile and exact site/project/space target rather than repository-selected administration. Strict kind/operation/actor/apply/revision/output validation precedes state, and the profile supplies its actual grant.
- **AND** changes use existing typed setters, output preflight and revision CAS. Normal output describes local configuration only, with live_verified false. Unknown commits retain the exact proposed revision separately from observed stored configuration and never retry automatically. Reads require no credentials or network access.
- **AND** schemas describe complete configuration/recovery data. Candidate dispatch preserves the original request identity and known effects. Public activation remains subject to independent consequential review; service provisioning retains existing prompt/pipe and actor/site proof requirements.

#### Scenario: Reviewed setup and cloud command registration
- **WHEN** reviewed service provisioning, local permission setup and the initial context/publication/ticket commands enter public parser and discovery
- **THEN** parser paths, typed request/response schemas and provider manifests agree. Cloud capabilities identify service OAuth and keep credential/policy/tenant/live verification unknown until actually established for the selected profile; registration alone proves none of those conditions.
- **AND** profile/grant/actor/scope/policy checks remain in the existing host. Public default builds retain no synthetic endpoint or credential bypass. Unsupported page-update editor-draft behavior and broader mutation/recovery/admin commands remain unavailable until qualified.
- **AND** executable/native acceptance and authorized live credential/site qualification follow registration before a usable live workflow is claimed. Independent review must cover registration changes as well as the underlying implementation.

#### Scenario: Ambiguous site
- **WHEN** a credential can reach multiple sites and no target is configured
- **THEN** the CLI fails with target ambiguity rather than selecting the first resource.

Local `profile list/show/create/use` SHALL store non-secret configuration, without claiming a verified actor or accessible tenant. Create/use SHALL declare local write effects, reject overwrite/conflicting selection, and preflight complete projected success/recovery envelopes before commit. A new grant reference SHALL be generated locally; sharing SHALL require an existing issued reference with matching authority and auth mode.

#### Scenario: Repository hint attempts to change security policy
- **WHEN** `.craft.json` contains state-root, credential-backend, auth-host, broker or reviewer settings
- **THEN** implicit repository selection fails with a bounded local error; explicit profile selection needs no repository hint read and the rejected hints do not redefine trusted configuration.

#### Scenario: Profile selection and target precedence
- **WHEN** explicit selection, repository hints and trusted active/default configuration differ
- **THEN** explicit selection wins, followed by repository hints, then trusted active/default configuration; conflicting explicit aliases and wrong tenant/resource kinds fail without choosing a substitute.

#### Scenario: Local write response cannot fit
- **WHEN** a profile create/use response or its unknown-commit recovery envelope exceeds the selected byte budget
- **THEN** no profile, grant or active-selection change commits; a failed commit acknowledgment instead preserves uncertainty and requires read-back from the same state root.

#### Scenario: Unsupported or corrupt trusted state
- **WHEN** a private state database has a foreign application ID, unsupported schema, malformed settings or incompatible native backend
- **THEN** it is refused without automatic replacement, backend fallback or a repository security-setting merge.

### Requirement: Native credential storage
OAuth credentials SHALL persist only in explicitly selected Windows Credential Manager, macOS Keychain or persistent Linux Secret Service backends, with no plaintext/file fallback.

#### Scenario: Large or locked credential
- **WHEN** a credential exceeds one native item or the vault is locked
- **THEN** bounded generation/chunk recovery succeeds without truncation, or the command fails without prompting or exposing a secret.

#### Scenario: Explicit persistent item backend
- **WHEN** the Windows adapter is selected from trusted installation settings
- **THEN** it uses only Windows Credential Manager with local-machine persistence, installation-bound collision-resistant item identities and a bounded native item size; another configured/global keyring store cannot redirect it.

#### Scenario: Native item health and failure
- **WHEN** native read health is probed or a native item operation fails
- **THEN** the result identifies backend/principal/read accessibility without enumerating unrelated entries, assumes no write permission from a read, never prompts or exports secret data, and preserves uncertainty when a mutation's outcome is not established.

#### Scenario: Interrupted credential generation
- **WHEN** a process stops during chunk writes, activation or obsolete-item cleanup
- **THEN** readers reject incomplete/mixed data, recovery verifies exact length/digest before activation, retired items remain discoverable for cleanup and a confirmed replacement is not hidden by a later cleanup failure.

#### Scenario: Cancellation after confirmed local activation
- **WHEN** cancellation arrives after the new active index has been read back but before retired chunks are removed
- **THEN** the caller receives the confirmed version with cleanup pending and a later locked session can finish cleanup without changing that version.

### Requirement: Service OAuth sessions
Supported unattended Atlassian profiles SHALL provision service credentials securely and acquire/renew client-credentials sessions with verified site access. Eligibility SHALL be established before enabling the mode.

A verified service access object SHALL retain sealed token lifetime, exact product/actor/grant identity and reported scope evidence across a composed workflow, without exposing credentials to provider modules or serialized output.

#### Scenario: Composed create access
- **WHEN** a host composes metadata/account reads and a resource mutation through one verified service access object
- **THEN** each prepared operation must retain the selected tenant and unchanged descriptor, satisfy its declared scope alternative and pass dual-clock token freshness; the mutation rechecks those facts at the final claimed-send boundary and never refreshes or retries inside that boundary.

#### Scenario: Expired service token
- **WHEN** a supported service session needs renewal
- **THEN** the CLI obtains a new token under the proper lock rather than treating the flow as rotating delegated refresh.

#### Scenario: Service token exchange contract
- **WHEN** service credentials are exchanged
- **THEN** exactly one form-encoded request uses the compiled token endpoint and service grant type, redirects/retries/proxies do not replay or redirect credentials, and errors contain no credential or raw response text.

#### Scenario: Untrusted token response
- **WHEN** token metadata is oversized, duplicated, malformed, missing expiry, scope-unknown or already stale
- **THEN** bounded parsing refuses unsafe values, omitted scope remains unknown, stale tokens cannot form a provider session and no delegated refresh is inferred from response extensions.

#### Scenario: Product identity proof
- **WHEN** a service token becomes a usable product session
- **THEN** the exact provider and Cloud ID have a successful current-user probe under reported scopes, every proof identifies the same active actor and a failed revalidation removes that target's proof without permitting an actor switch.

#### Scenario: Persisted service grant
- **WHEN** provisioning or reacquisition changes a service credential bundle
- **THEN** the native generation store alone retains credentials/tokens, the grant lock spans committed reread through activation, configured targets and actor are verified before replacement, and confirmed activation receipts are preserved even if later session creation fails.

#### Scenario: Provisioning result preflight
- **WHEN** explicit provisioning has verified the configured products but its activation/recovery output cannot fit
- **THEN** the host preflight refuses before a keyring replacement, token issuance uncertainty remains distinct from local activation uncertainty and no secret reflected into scope metadata becomes public output.

#### Scenario: Cache reuse after restart or logout
- **WHEN** a service session is requested from persisted state
- **THEN** a fresh token is reverified for the configured product with a new monotonic expiry anchor, a stale token is reacquired once under the lock, and missing/corrupt/unresolved state or an unconfigured target does not initiate login or exchange.

#### Scenario: Native service cache across competing processes
- **WHEN** separate processes reopen one native service grant, compete to renew its stale token, or wait behind a logout holder
- **THEN** they preserve the complete configured bundle, only the first successful renewal acquires a token, later contenders reread that generation and reverify their product, and a process following logout refuses acquisition without resurrecting the removed bundle.

### Requirement: Delegated OAuth broker
Delegated Jira/Confluence login SHALL use a confidential broker with bound single-use handoff, state/callback checks and no app secret in the distributed binary.

#### Scenario: Replayed handoff
- **WHEN** a login redemption or callback binding is replayed or mismatched
- **THEN** the broker refuses it and returns no token material; ordinary CLI operations do not initiate login.

### Requirement: Safe credential lifecycle
Status/doctor/refresh/logout SHALL redact secrets, coordinate rotating grants and report local removal separately from documented remote revocation.

#### Scenario: Local auth diagnostics
- **WHEN** status or doctor inspects an explicitly resolved local profile
- **THEN** status reads only the locked generation index, reports stored data as unverified and identifies shared profiles, while doctor probes native read accessibility without credential writes, token exchange, product requests or unlock prompts.

#### Scenario: Secret provisioning pipe
- **WHEN** explicit service provisioning consumes a secret stream
- **THEN** the secret reader requires a native pipe rather than a terminal or regular file, enforces a bounded zeroizing buffer and invocation cancellation/deadline, and parses strict secret credentials without raw values in diagnostics or generic JSON output.

#### Scenario: Explicit interactive credentials
- **WHEN** a human explicitly selects interactive service credential entry on a terminal
- **THEN** both fields are read without echo, bounded input and Unicode editing are preserved, cancellation/deadline/read errors restore the original terminal mode before returning, restoration failure refuses credential use and no prompt is started by ordinary agent operations.

#### Scenario: Logout races with refresh
- **WHEN** one process logs out while another attempts token refresh
- **THEN** shared credential state does not resurrect removed tokens and affected profiles are reported without token export.

#### Scenario: Stale local writer after logout
- **WHEN** a writer reuses a version captured before a later replacement or logout
- **THEN** the locked epoch/generation check rejects it, the inactive tombstone remains authoritative and unrelated grants remain usable.

#### Scenario: A local lock holder is killed
- **WHEN** another process owns the same installation/grant lock and then exits without releasing it explicitly
- **THEN** acquisition remains bounded by deadline/cancellation while the holder lives, unrelated grants remain available and the OS releases ownership after process death without deleting the lock file.
