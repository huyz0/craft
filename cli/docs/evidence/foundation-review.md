# Independent review of the foundation changes

Date: **15 November 2026**. Reviewer: a separate read-only reviewer session, authorized under the project's standing review authorization. It read the source and the pinned dependency sources (secret-service 5.2.0, apple-native-keyring-store 1.0.2, security-framework 3.7.0), edited nothing, and ran `cargo test -p craft-runtime --lib -- retries cache_tests downloads` (33 passed) on Windows.

Scope: bounded safe-read retry and download pre-body retry, the single service-OAuth reacquisition after a 401, the macOS Keychain and Linux Secret Service vault adapters, the Unix guarded output root, and the busy-wait added to store opening.

## Findings and dispositions

| Severity | Finding | Disposition |
| --- | --- | --- |
| High | The macOS Keychain adapter left keychain UI enabled, so a locked login keychain would show the system unlock dialog, and `errSecInteractionNotAllowed` would map to unavailable rather than locked | Fixed. Every keychain read, write and delete runs under `SecKeychain::disable_user_interaction()`, serialized by the vault's gate. `-25308` maps to `Locked`. Not compiled or run here; macOS stays unqualified |
| Medium | Linux create and delete can still run a library prompt with no timeout if the collection locks between the check and the call, or with a provider that confirms every change | Partly mitigated: every operation now checks the collection and each matched item for lock state immediately before acting. The race and confirm-on-change providers remain a documented residual risk in `vault/linux.rs`; gnome-keyring with an unlocked login collection does not prompt |
| Medium | A disconnected prompt during a mutation mapped to `Locked`, a known-unchanged state, although the change may have been applied | Fixed: maps to `OutcomeUnknown` for mutations |
| Low | Linux searches covered every collection, including the session collection | Fixed: reads, writes and deletes search only the persistent login collection |
| Low | A Unix create-new publish refused by a filesystem without hard links was reported as unknown and left staging behind; the parent directory was not synced | Fixed: `linkat` and `renameat` either apply or leave the directory unchanged, so their errors are clean refusals and staging is discarded. The directory is synced after publication, best effort |
| Info | Cloned retry requests keep the first attempt's per-request timer | No change: the 10 ms invocation-control polling bounds every attempt, as the reviewer noted |

No blocking issue was found in transport retry (opt-in only for sealed safe reads, at most three sends, redirects disabled so credentials stay on the original host, deadline and cancellation honored, writes and multipart never repeated), in the reacquisition path (one exchange under the caller's grant lock, every target verified before `KeepPrevious` activation, a second 401 final), in the Unix output root (handle-relative `O_EXCL|O_NOFOLLOW` 0600 staging, single-link recheck, no replacement under create-new, protected roots excluded by device and inode), or in `wait_busy` (idempotent pragmas only, within the invocation budget).

## Checks after the fixes

Windows: workspace Clippy with `-D warnings`, `craft-platform` and `craft-runtime` library tests (160 passed). Linux container with a real gnome-keyring session: workspace Clippy with the synthetic feature, `craft-platform` and `craft-runtime` library tests (151 passed), the Jira download and local-artifact host suites, and the Linux vault and native generation opt-ins all passed.
