# Service OAuth and native-generation cache evidence

Date: **9 November 2026**, Australia/Sydney. Partial implementation evidence. The service OAuth task remains unchecked; no credential-facing CLI command or live tenant access is enabled.

Later input/diagnostic work is recorded in [local auth diagnostics](auth-diagnostics.md) and [interactive credential input](interactive-input.md). Public provisioning, consequential auth commands and live access remain unfinished; the older hashes/results below identify their original checkpoint.

The **10 November 2026** [Windows native service-cache acceptance](native-service-cache.md) now qualifies a multi-item service bundle across fresh processes, competing renewal and logout using the actual Windows keyring and synthetic TLS servers. It supersedes the native service-bundle gap below for this principal only; live issuer, public CLI and other platform qualification remain open.

The later [disabled provisioning adapters](provision-dispatch.md) preserve complete envelopes and input-source ownership, with strict candidate schema/flag checks and verified public non-exposure. No consequential auth command was enabled.

## Implemented scope

- `crates/craft-runtime/src/auth/{mod.rs,wire.rs,service.rs}`: bounded secret provisioning parse, fixed-origin single token exchange, safe errors/rate advice, dual-clock expiry and opaque token ownership.
- `crates/craft-runtime/src/auth/service/identity.rs`: compiled per-product current-user probes, scope alternatives, consistent actor binding, target-specific proof and failed-revalidation invalidation.
- `crates/craft-runtime/src/auth/service/cache.rs`: strict native-generation bundles, held-lock grant binding, explicit provisioning, fresh-cache verification, stale reacquisition, configured target enforcement and preserved activation receipts.
- Shared bearer/TLS helpers in `transport/mod.rs`; one crate-level synthetic TLS fixture used by transport and auth tests.
- `secrets/generations.rs` supplies the held lock's non-secret grant identity. Existing test-only credential fixtures are reused without adding a production memory-vault fallback.
- [Contract](../service-auth.md), secure-auth scenarios and design document describe the behavior and enablement boundary. No Cargo dependency/feature/version change was needed.

## Observed validation

The initial service exchange/identity subset passed 13 tests with native loopback access. The later service exchange/identity gate passed 158 named Rust tests plus the subprocess controls, inventory, formatting and Clippy, after fixing a duplicate TLS fixture module load. Its passing log is `target/service-oauth-verification-output.log`; its earlier Clippy failure remains in `target/service-oauth-verification-failed.log`.

Five initial cache tests passed in `target/service-cache-tests-output.log`. The first cache subset failed while scanning a still-locked Windows lock file, after all auth assertions completed; the test now drops its lock before file inspection. Its failed log remains in `target/service-cache-tests-failed.log`.

The expanded cache gate passed its tests but caught two follow-up issues: a stale-response fixture depended on clock-read counts, and Clippy rejected the larger auth error layout and an unnecessary test wrapper. The clock fixture now observes actual HTTP request receipt, and the optional activation receipt is boxed without suppressing the lint. Failure logs remain in `target/service-cache-verification-failed.log` and `target/service-cache-clippy-failed.log`. A later planning check identified this evidence file's then-missing link target; that log is retained as `target/service-cache-documentation-failed.log`.

The final `python tools/verify.py` gate passed with native loopback/filesystem access: **166 named Rust tests passed, zero failed, two opt-in real-vault tests ignored**, plus formatting, locked inventory/layer checks and all-target Clippy. Its log is `target/service-cache-verification-output.log`. `cargo build --workspace --locked --offline` also passed. Checker selftests passed nine quality controls and fifteen planning controls. Dependency inventory remains 278 packages. Compiler incremental-cache hard-link warnings used copying and did not fail the gate.

Development executable `target/debug/craft.exe` has SHA-256 `7c30265514b354b722ba5bdef9f8eb3ef7f84270a164ecbcce5c11c98bbcb369`. Source SHA-256 checkpoints:

| File | SHA-256 |
| --- | --- |
| `auth/service.rs` | `bb4e04cc30aeea348858ccb6559d924fea937da0d757f23d44b6e262b9f6a2ff` |
| `auth/service/identity.rs` | `44b5a1d3dd1638ae017566d777548b17d05fa0a437d09dbfc75b615c2284c865` |
| `auth/service/cache.rs` | `33792df179a4f9f5c2d22b8c0bd5b6b20ea32790d7daa065c839576eb5c71a7b` |

## Behavior checked by synthetic tests

Token tests inspect the exact form/path/headers and one-attempt behavior; reject malformed provisioning, duplicate/missing/oversized response fields, invalid bearer/scope/lifetime and already stale responses; preserve unknown omitted scopes; ignore OAuth extensions; redact secrets/error bodies; preserve typed rate advice; and observe cancellation/deadlines. Redirect sinks receive no credential request. Invalid CA/hostname handshakes receive no HTTP credentials.

Identity tests exercise Jira classic/full-granular and Confluence scope alternatives, fixed routes, exact provider/cloud proofs, missing scopes and malformed tenant refusal before network, inactive/anonymous/missing/duplicate/mismatched actors, cross-product consistency and invalidation of a failed target recheck while preserving actor binding.

Cache tests reopen bundles with a different client/monotonic origin, reuse fresh tokens without another exchange, reacquire stale tokens once, verify all provisioned targets on acquisition, retain the previous generation after actor failures and reject unconfigured targets. Missing, corrupt, duplicate, wrong-grant, invalid-lifetime and locked state do not acquire credentials. Logout remains inactive. A frozen or backward wall clock does not extend cache life within a client. Interrupted staging blocks further exchanges until local recovery. Confirmed activation exposes cleanup-pending state; an expiry after activation preserves the receipt in the returned failure. Files in the owned test lock directory are checked for sentinel secrets after releasing the lock.

The cache tests use a fault-injected memory vault behind the real generation adapter and native file locks. They do not establish actual service-bundle persistence in the Windows keyring across processes. Earlier [native generation acceptance](credential-generations.md) qualified that byte-store layer separately, with synthetic data. Reopening a cache in one process is not labeled a fresh-process OAuth test.

## Remaining acceptance and risks

- Secure human prompt/pipe provisioning and normal CLI auth commands.
- Explicit service eligibility, profile auth-mode policy and operation-specific scope/resource checks.
- Authorized live service acquisition and Jira/Confluence actor/access proof on the test site.
- Actual issuer concurrency and other principals/platforms. The later [Windows service-cache acceptance](native-service-cache.md) covers native bundle persistence, fresh-process reuse, concurrent renewal and logout with a synthetic issuer for this user/session.
- macOS/Linux backends, unavailable sessions and service/container principals.
- Delegated confidential broker, refresh/revocation integration and multi-machine authority.
- Independent consequential auth review. None has occurred.

Owned secret buffers zeroize; complete erasure of transport/parser internal allocations is not proven. Cross-restart expiry depends on wall time and cannot detect all malicious clock changes. Native generation failure can require explicit local recovery; it does not authorize OAuth replay. No real credentials were provisioned, no cloud resource was changed and no commit, push, publication or release occurred.


## Setup adapter follow-up

Explicit-profile policy get/set adapters now connect the existing Confluence read/write and Jira create/read/mutation/recovery stores to candidate command dispatch. See [setup evidence](policy-setup.md) for checks, exact source boundaries and pending activation review. This adds no credential export, schema or authentication lifecycle. Public provisioning/setup and live qualification remain unfinished.
