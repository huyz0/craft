# Internal service authentication

Craft's runtime implements an internal Atlassian service-token exchange and native-generation cache. Public local status/doctor diagnostics are described [separately](auth-diagnostics.md). Provision/login/refresh/logout commands and live eligibility checks remain unfinished. This is an implementation contract for contributors, not a runnable login procedure.

## Exchange and identity

`ServiceCredentials::from_provisioning` consumes bounded zeroizing JSON containing `client_id` and `client_secret`. Unknown/duplicate fields, empty/control-containing values and excessive sizes fail without echoing input. It accepts no argv, environment or file fallback. `read_service_credentials_from_stdin` supplies a bounded native-pipe-only worker with cancellation/deadline handling and secret-aware buffers. The explicit interactive entry point now collects both fields without echo through a native restoration guard; see [contract and qualification](interactive-input.md). Neither entry point is wired to a public provisioning command yet.

`ServiceOAuthClient::acquire` makes exactly one form POST to compiled `https://auth.atlassian.com/oauth/token`, with `grant_type=client_credentials` and the two credentials. It shares the HTTPS adapter's verified TLS policy. Redirects, retries, proxies, cookies, compression and TLS key logging are disabled. Production endpoints cannot be supplied by provider content or configuration.

Token responses are bounded to 64 KiB. Known secret fields deserialize directly into zeroizing storage rather than a generic JSON value. Duplicate top-level fields, invalid bearer text, unsafe scope syntax and missing/invalid expiry fail. Extensions are ignored; a refresh-token extension never selects delegated renewal. Omitted scopes remain unknown. Returned lifetime must be between one second and one day. Client IDs, secrets and bearers are bounded to 1 KiB, 64 KiB and 16 KiB respectively. Owned secret buffers zeroize; complete erasure of third-party transport/parser allocations is not claimed.

Expiry starts conservatively at request start. Wall and monotonic time both constrain reuse, with an early reacquisition margin of the smaller of 60 seconds and one tenth of the lifetime.

`verify_product` requires reported probe scopes before sending a fixed-route current-user read for the explicit product and Cloud ID. Jira accepts its classic user-read scope or the full documented granular alternative; Confluence accepts its classic user-read scope or content-details read scope. Jira additionally requires an active user. Anonymous, missing, malformed or changed actors fail. The first successful actor binds the token across products/sites. A failed recheck removes that target's proof without clearing the actor binding. At most 32 product proofs are retained.

`ServiceToken::into_host_session` consumes the token without exporting its bytes and requires freshness plus proof for that exact provider/Cloud ID. Identity proof establishes current-user access, not project/space access, operation scopes or mutation permission.

## Locked keyring cache

`ServiceToken::into_verified_access` retains sealed product/actor/grant, scope and lifetime evidence across prepared operations without exposing bearer bytes. `open_service_access` supplies the actual locked grant and any activation receipt through the same cache protocol as `open_service_session`. Each operation check rejects stale tokens, missing/insufficient scope evidence, changed descriptors and different tenants. The [internal create host](jira-create-host.md) adds resource policy and the final claimed-send check. These APIs do not enable public commands.

The trusted host obtains `LockedCredentials` through `NativeCredentials::lock`. It retains the lock across the complete auth operation. The lock supplies the grant identity; provider arguments cannot select a different grant inside the bundle.

`provision_grant` is explicit reprovisioning. It validates one to 32 unique product targets, reads the current generation, acquires a token, verifies every target and the expected actor when provided, then activates one new native generation. A failure before activation leaves the previous generation selected, or preserves an unresolved staging state if a local write became uncertain.

The strict, versioned bundle contains configured targets, actor, credentials, cached token, expiry and reported scopes. Secret serialization uses a private zeroizing buffer. The production cache has no SQLite, plaintext-file, environment or process-global fallback. Native chunk/index behavior and cleanup receipts follow the [generation contract](native-vault.md).

`open_service_session` refuses missing, inactive, corrupt or unresolved state before acquisition. Unconfigured targets require explicit reprovisioning. A fresh cached token receives a new identity probe for the requested target. A stale token is acquired once from stored client credentials, verified against the original actor for every configured target, and activated before returning a session. Same-root contenders read the committed cache after acquiring the grant lock.

Persisted product proofs are never loaded as current authority. After reopening, wall expiry receives a fresh monotonic anchor; the client's original wall/monotonic anchor also prevents repeated cache loads from extending a frozen clock's lifetime. A wall time earlier than issuance forces reacquisition. Cross-restart clock manipulation that remains after issuance is outside this local clock guarantee.

Cleanup-pending activation remains a successful, visible receipt. If session construction fails after confirmed activation, `AuthFailure::credential_update` retains that receipt. Local storage uncertainty never triggers another OAuth exchange. Callers must resolve pending generation state through local recovery, then issue a new invocation if needed. Client credentials do not rotate when tokens are acquired; this flow does not use delegated-refresh intent/recovery.

## Failure and enablement boundaries

Failures contain safe codes, bounded messages and typed rate advice. They never include raw response bodies or free-text headers. A cancelled/failed token exchange can preserve possible token issuance. Failed identity reads do not claim token issuance or a possible product-resource mutation. There is no automatic retry.

These runtime methods do not enable public provisioning, login, refresh, logout or cloud commands. Before enabling those consequential paths, complete secure provisioning input, eligibility and profile-mode policy, operation-specific scope/permission gates, confirmed/unknown receipt output, live sandbox qualification and independent consequential review. Local status/doctor do not exercise that path. macOS/Linux vault qualification, service/container principals and real OAuth concurrency are still separate acceptance work. See [service auth evidence](evidence/service-auth.md) and [local diagnostic/input evidence](evidence/auth-diagnostics.md).

An unregistered [provisioning composition](review-service-provisioning.md) now connects explicit profile/actor selection, both input sources, grant locking, current-user proof and preflighted output. It remains a review candidate. Its [evidence](evidence/provisioning-composition.md) includes distinct token/local-write uncertainty and reflected-scope refusal.

[Windows process acceptance](evidence/native-service-cache.md) now exercises actual native service bundles across fresh processes, competing renewal and logout with synthetic token/product servers. This qualifies the runtime composition for the tested user/session; it does not establish live service eligibility or the public provisioning path.

The [disabled CLI adapters](evidence/provision-dispatch.md) now retain complete provisioning envelopes and structured-input stdin ownership through dispatch/output. The typed action is skipped by the public parser and its descriptor builder is absent from public registration. Independent review, explicit enablement and actual public/live qualification remain required.
