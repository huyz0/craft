# Native credential item adapter

The Windows item adapter is implemented and tested with synthetic entries under the current development principal. It is an internal runtime API; credential provisioning, login and other credential-facing CLI commands remain unimplemented. The complete backend requirement still includes macOS Keychain and persistent Linux Secret Service.

## Windows contract

`NativeVault::open` accepts an explicit backend and a trusted installation ID from `ProfileStore::settings`. It constructs Windows Credential Manager directly. Neither a process-global keyring default nor repository hints select another store. Unsupported backend selections fail with `StoreError::Unavailable`.

The adapter implements the core `CredentialStore` port using binary `SecretBytes`, without password/string conversion. It derives versioned native target names from the installation ID and a digest of the structured service/account identity. Credential names contain no raw service/account values. Native search/enumeration is disabled.

Items have explicit local-machine persistence under the credential principal. The native item limit is 2,560 bytes; larger input fails before a native mutation. Reads and mutations reject existing entries with incompatible persistence. The item adapter's mutex serializes calls through one instance; the generation adapter below adds grant-scoped cross-process ownership.

`health` returns backend, process principal, configured persistence, the result of one read-only namespaced probe and the native item limit. A missing probe entry establishes API read accessibility, without establishing that all entries can be read. Write accessibility stays `unknown`. Health never creates a credential, enumerates entries or initiates an unlock prompt. Thread impersonation is outside the supported execution model.

Errors cross the adapter boundary only as redacted `StoreError` variants. An uncertain native mutation returns `OutcomeUnknown`; callers must reconcile rather than infer absence or retry blindly. Operations use synchronous native APIs. This adapter does not itself make a native call interruptible or qualify outer auth cancellation/recovery behavior.

## Run the native sentinel acceptance

From `cli/` on the qualified Windows development host, with access to Windows Credential Manager:

```text
cargo test -p craft-runtime --lib --locked --offline native_windows_vault_round_trip_restart_limits_and_persistence -- --ignored --exact vault::tests::native_windows_vault_round_trip_restart_limits_and_persistence
```

The test uses a fresh random installation namespace and synthetic binary bytes. It checks read/write/delete, exact item-size boundaries, unchanged content after an oversized write, persistence attributes, fresh-process reading, namespace separation, session-only refusal and resistance to a mock global default. It explicitly deletes its sentinel entries and attempts cleanup on failure. No real OAuth credential or unrelated native entry is read or changed.

The test is ignored by the ordinary workspace gate because it writes owned synthetic entries to the real OS vault. Run it explicitly and record the result. A skipped native test is not a native pass. Missing native access fails the explicit test; it does not select a mock or file fallback. [Evidence](evidence/native-vault.md) records the observed host, result and limitations.

## Locked credential generations

`NativeCredentials::open(&ProfileStore)` uses the validated installation root and native backend. `lock(&GrantRef, &InvocationControl)` retains an exclusive OS file lock for that installation/grant until the returned session drops. Lock acquisition checks cancellation and deadlines; a killed owner releases its kernel lock. Lock files contain no secret bytes and are never deleted to break contention. One principal, machine and state root own a live installation. Copying an installation to another root or machine does not share these locks and is unsupported for rotating credentials.

The session's `read` returns an opaque version and zeroizing secret bytes. `inspect` returns index metadata only; it does not verify active chunks. `replace` requires the current version, stages a bounded index before chunks, verifies exact chunk sizes/length/SHA-256, then activates the generation. Secrets are limited to 1 MiB and split into 2,048-byte keyring items. The index stays within 2,048 bytes and contains at most two distinct active, pending or retired manifests. Secret and index bytes are never stored in SQLite or lock files.

Readers refuse unresolved staging and corrupt/mixed chunks. `recover` activates complete staging or applies its recorded policy: `KeepPrevious` retains a verified earlier credential, while `Reauthorize` clears it when the external protocol cannot permit reuse. An unavailable/locked vault does not establish that chunks are missing. Confirmed activation survives cleanup failure or cancellation through a receipt with `cleanup_pending`; recovery completes deletion without changing its logical version.

`logout` requires a current version and commits an inactive epoch before deleting chunks. It retains the index tombstone so stale snapshots cannot revive credentials. It removes local authority only; provider revocation is separate. These methods do not perform OAuth exchanges. The auth protocol must durably record exchange intent before remote rotation; the byte store cannot recover an exchange response lost before staging.

The explicit Windows acceptance checks oversized Unicode/binary generations across fresh processes, child-process logout, stale-version refusal, incomplete-staging recovery and owned synthetic-entry cleanup:

```text
cargo test -p craft-runtime --lib --locked --offline native_generations_survive_restart_recover_and_logout -- --ignored --exact secrets::tests::native::native_generations_survive_restart_recover_and_logout
```

This test is ignored by the ordinary gate because it writes synthetic entries to the real vault. [Generation evidence](evidence/credential-generations.md) separates deterministic, kernel-lock and native results from remaining auth/platform work.

## macOS and Linux backends

Each OS selects exactly one store and refuses the other two: Windows Credential Manager, the macOS login keychain (generic passwords through `apple-native-keyring-store` 1.0.2) or the Linux Secret Service login collection. Linux items carry one attribute, the hashed per-installation target, and are reached through the `secret-service` 5.2.0 blocking API directly rather than the keyring wrapper above, because that wrapper unlocks items and collections implicitly. Craft never calls an unlock method: a locked collection or item returns `Locked` for read, write, delete and the health probe, so no prompt can appear. Writes also refuse when the default collection is the session-only alias. Both Unix stores use an 8 KiB item limit; generations chunk above it.

The Linux opt-in `vault::tests::native_linux_secret_service_round_trip_limits_and_locked_refusal`, the cross-platform generation test above and the native service-cache test pass against a real gnome-keyring Secret Service; see [platform qualification](evidence/platform-qualification.md). The macOS adapter is compiled only on macOS and has not been built or run, so macOS remains unqualified.
