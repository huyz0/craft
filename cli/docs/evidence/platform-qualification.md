# Windows and Linux qualification of the foundation

Date: **15 November 2026**, Australia/Sydney. Source: branch `foundation` at the commit that adds this file, which includes the Confluence work merged from local `main`. Pinned Rust 1.99.1 on both hosts. macOS was not run.

## Hosts

| Host | How it ran |
| --- | --- |
| Windows 11 x86_64 MSVC | `python tools/verify.py` (full scope) on the development machine |
| Linux x86_64 | Official `rust:1.99.1-bookworm` container. The worktree is copied into the container with normal Unix modes (`go-w`), then every suite runs inside `dbus-run-session` with `gnome-keyring-daemon` unlocked for the secrets component, so the Linux Secret Service is real, not a mock |

## Results

| Check | Windows | Linux |
| --- | --- | --- |
| Planning, skill, contract, provider and dependency checks; formatting | pass | not repeated (host independent) |
| `cargo clippy --workspace --all-targets -D warnings`, with and without `synthetic-acceptance` | pass | pass |
| `cargo test --workspace` | pass | pass |
| Nine composed-host suites and the `craft-cli` feature suite | pass | pass with `--test-threads=4` (see below) |
| `cargo test -p craft-runtime --lib --features synthetic-acceptance` | pass | pass |
| `tools/synthetic-acceptance.py` | pass | not repeated |
| Native opt-in `secrets::tests::native::native_generations_survive_restart_recover_and_logout` | passed earlier on this principal ([generations](credential-generations.md)); not repeated | pass against the real Secret Service |
| Native opt-in `auth::service::tests::native_tests::native_service_cache_restarts_serializes_reacquisition_and_preserves_logout` | passed earlier ([native cache](native-service-cache.md)); not repeated | pass against the real Secret Service |
| Native opt-in `vault::tests::native_linux_secret_service_round_trip_limits_and_locked_refusal` | not applicable | pass |

Windows: 1,178 tests passed, none failed, twelve opt-ins ignored. Linux: every suite passed. The `craft-cli` feature suite run with default parallelism on Linux had seven profile-selection cases hit their 5 s invocation deadline under container I/O load (SQLite runs with `synchronous=FULL` on the Docker overlay filesystem); the same cases pass in the non-feature workspace run and all pass with four test threads. No assertion was relaxed for this.

## Defects the Linux run found, now fixed

1. **Guarded output roots were Windows-only.** `OutputRoot` returned "unsupported" on every other OS, so `jira.attachment.download` export and `artifact export` refused on Linux and macOS. The Unix implementation resolves every name inside the pinned no-follow parent directory: staging is created with `O_CREAT|O_EXCL|O_NOFOLLOW` and mode 0600, verified as the same single-link regular file before publication, published for create-new with `linkat` (which refuses an existing destination atomically) and for replacement with `renameat`, and unlinked on drop if never published. Protected private roots are excluded by `(dev, ino)` identity. The seven download host cases that exercise export, collision, revocation and interruption pass on Linux.
2. **Concurrent store opening could fail on Linux.** `ProfileStore::open` ran its initial pragmas with a zero busy timeout, so a second process opening the store while another held a write lock got `Cannot read or update trusted local state`. The pragmas now wait on `SQLITE_BUSY` within the invocation budget, like transactions already did. `journal::tests::process::competing_processes_cannot_claim_the_same_native_request_twice` passes on Linux.
3. **Test-only deadlock.** Three policy race tests opened a store before a two-thread barrier; if one open failed, the other thread waited forever. They now reach the barrier before propagating the error.
4. **Locked-collection health.** The Linux vault's health probe read an absent item and reported `available` even with the login collection locked. It now checks the collection's lock state first.

Two Linux-only setup mistakes in the first container attempt were not product defects and are recorded so they are not mistaken for passes: a read-only source mount (tests write fixtures under `cli/target`), and a copy that kept the Windows mount's 0777 directory modes, which the private-state checks correctly refused.

## Not covered

macOS: the Keychain adapter compiles only on macOS and was neither built nor run here. Windows unavailable logon sessions, OS restart and power loss, and other Linux Secret Service providers (KWallet, KeePassXC) are not exercised.
