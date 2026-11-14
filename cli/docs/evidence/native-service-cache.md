# Windows service-cache process acceptance

Date: **10 November 2026**, Australia/Sydney. This qualifies the internal service cache on this Windows user/session with synthetic credentials. The service OAuth task remains unchecked. No public auth command or provider was enabled.

## Scope and observations

`crates/craft-runtime/src/auth/service/tests/native_tests.rs` exercises production native credentials, Windows Credential Manager, generation storage, profiles, native grant locks and the service cache. It starts the current test executable in six fresh child processes. Test-only local TLS servers issue a synthetic token and current-user responses. Public CA bytes, ports and fixture choices cross stdin; credential values are neither passed in argv/environment nor written into fixture files.

- Initial provisioning stores a Unicode client ID and large synthetic secret in a bundle exceeding the native item limit. A fresh child preserves the generation and issuance time, reuses the token and makes a new Jira identity probe without exchange.
- Two new children announce readiness while the parent owns the grant lock. After release, one reacquires the stale token and verifies both products. The other reads the committed generation and verifies its product. Both report the same version/issuance; exactly one reports an update.
- A logout child owns the lock while another child prepares a stale-token request. After logout, the waiter returns `AuthRequired`; a later fresh child sees the same tombstone. Neither acquires a token or probes a product. A parent writer using the prior version receives `VersionConflict`.
- Exactly two exchanges occur in total, including provisioning, and six current-user reads. Renewed native bytes preserve credentials, token and both targets; both exchange forms retain the original credentials. Child streams and local fixture files contain no credential/token sentinels.
- Logout cleans owned chunks, and the test deletes/verifies absence of its owned index. The fixture root is absent at success. Runner guards join children before cleanup; if explicit native cleanup cannot be confirmed, local metadata is retained.

The test reuses the test-only `NativeCleanup` in `secrets/tests/native.rs`. The secure-auth spec now states the process acceptance scenario. No production endpoint selector, credential fallback, registration or dependency changed.

## Commands and results

From `cli/`, with native Windows vault/loopback access:

```powershell
cargo test -p craft-runtime --lib --locked --offline -- --ignored --exact auth::service::tests::native_tests::windows_service_cache_restarts_serializes_reacquisition_and_preserves_logout --nocapture
python tools/verify.py
python tools/verify.py --selftest
python tools/plan.py selftest
```

The opt-in passed one parent test, which checks six child results, with zero failures. Output: `target/native-service-cache-acceptance-output.log`. The initial restricted attempt failed at credential-state access before acquisition; cleanup also could not read native state. That path precedes native credential writes. Its log remains `target/native-service-cache-restricted-failed.log`. The later native pass used a new owned namespace and confirmed cleanup.

The full development gate passed **189 named Rust tests, zero failures, four ignored opt-ins**, thirteen-operation contract comparison, ten contract controls, formatting, locked inventory/layer checks and all-target Clippy. Output: `target/native-service-cache-verification-output.log`. Item, generation, service-cache and owned-console tests are opt-ins; only the new service-cache test was repeated here. Other native passes retain their own evidence. Selftests passed nine quality and fifteen planning controls. Inventory remains 278 packages. Incremental-cache hard-link warnings used copying without failing the gate.

SHA-256 checkpoints:

| Artifact | SHA-256 |
| --- | --- |
| `auth/service/tests/native_tests.rs` | `2d14f10dcf541930bf43259c968a9fc0921db00ddbf83e11cd25b709444013e1` |
| `secrets/tests/native.rs` | `b65b73ef5464313c78baf819fba1240bd1a933107bc740edd6ef76f1b9a8674d` |
| `target/debug/deps/craft_runtime-2dec3e9846e14ca8.exe` | `d2b5f86d0f87e95aab64b2a746cbf88c322e9cc2fa66228965da972eef28febb` |

## Limits and next work

Token/product servers and wall clocks are synthetic. This does not prove live Atlassian eligibility, scopes, site access or operation permissions. It bypasses public parser/dispatch, human input and the provisioning envelope. Other users, service/container principals, unavailable sessions, macOS/Linux, multi-machine coordination and every process-kill boundary remain unqualified. Earlier generation fault/kill checks are separate evidence. Independent auth review remains pending; no reviewer was started. Public integration and authorized live qualification remain required. No real credential, tenant resource, commit, push, publication or deployment changed.
