# Local artifact inspection and export recovery

The CLI adapter in `crates/craft-cli/src/local_artifacts.rs` reads exact names from closed downloads/requests/reviews collections and performs digest-bound local export. It does not select a profile or access credentials/cloud services. Runtime `export_file_checked` validates the expected digest against the opened source before output staging, and receipt verification now reports a valid oversized object as TooLarge rather than Corrupt. The Jira download host binds its export to its just-published digest.

Focused evidence before public activation:

- `cargo test -p craft-cli --lib local_artifacts --locked --offline`: five groups passed (`target/local-artifact-tests.log`), covering strict flattened JSON/unknown keys, real binary inspection, absent/corrupt/oversized distinction, exact export digest, collisions/replacement, whole-state protection, output capacity and invalid controls before state access.
- `cargo test -p craft-runtime --lib artifacts::tests --locked --offline`: 24 cases passed (`target/local-artifact-runtime-tests.log`), including new opened-source digest binding and verification-ceiling cases, existing uncertainty/cancellation, link/path protection and persistence faults.
- `cargo clippy -p craft-cli -p craft-runtime --all-targets --locked --offline -- -D warnings`: passed (`target/local-artifact-clippy-fixed.log`). The original log retains two needless-borrow findings in the response-reservation sample, corrected before rerunning. Cargo reports incremental-cache hard-link fallback warnings.

Public parser/catalog wiring, subprocess recovery, independent source review, final qualification and installed-bundle checks remain pending at this checkpoint. Native/live authority is not established by local receipt verification. Other-platform guarded export remains unavailable; uploads and deletion are separate unfinished work.

Public wiring and focused executable acceptance:

- Parser/catalog and dispatch expose artifact.get/export through the dedicated local host; generic dispatch cannot bypass it. State-root selection is supported and profile selection is not.
- `cargo test -p craft-cli --test local_artifacts --locked --offline`: three tests passed (`target/local-artifact-subprocess.log`). A real saved artifact is inspected and exported through stdin JSON and flags without a profile database, collision retains the source receipt, response schemas accept the emitted envelopes, stdout contains no binary bytes, invalid/projection input leaves state absent, and corruption remains unknown presence.
- Authorized source-only reviewer `/root/skill_review` found no blocker in the host, expected-digest adapter, state-root exclusion, response reservation or existing download binding. The reviewer ran no tests/binaries. Public-wiring review and broad qualification are recorded separately below when complete.

The same reviewer inspected public parser/catalog/dispatch wiring and found no blocker. This was source-only review, with no executable or Cargo invocation. Broad qualification remains pending at this source checkpoint.

## Broad attempt, focused correction and environmental refusal

`python tools/verify.py` passed its tooling, both Clippy modes and workspace suite, then failed to launch confluence_context_host before any test in that executable ran (`target/local-artifact-qualification.log`). Windows error 4551 and Code Integrity event 3077 identify an Application Control enterprise-signing refusal, policy 0283ac0f-fff1-49ae-ada1-8a933130cad6. Exact path/hash and events are retained in `target/local-artifact-appcontrol-block.json` and `target/local-artifact-appcontrol-events.json`. No policy was changed and no blocked executable was renamed, moved or retried to bypass the refusal. Approval through the policy or an approved Windows environment is pending.

All eight unaffected Jira composed hosts passed once in `target/local-artifact-unaffected-hosts.log`, using the original selected host arguments minus the blocked host and with no-fail-fast. Their source checkpoint is `target/local-artifact-broad-checked-source-hashes.json`. This is partial broad evidence, not full qualification.

A subsequent audit corrected the local export error envelope: because export itself is the local write, error.details.write_state must retain its export disposition, including SentUnknown and Confirmed after late cancellation. Get remains NotSent. Six focused command cases passed (`target/local-artifact-envelope-tests.log`), including the new pure envelope/encoded-output regression for those two dispositions; that command's name filter selected zero subprocess cases, so they were explicitly rerun without the filter. All three subprocess cases passed in `target/local-artifact-subprocess-final.log`, and all-target CLI Clippy passed in `target/local-artifact-envelope-clippy.log`. Authorized source-only reviewer /root/skill_review confirmed the correction with no blocker and ran no executables.

Feature-enabled runtime passed 139 cases with three native opt-ins ignored (`target/local-artifact-feature-runtime.log`). Production dependency isolation and nondebug feature refusal passed (`target/local-artifact-isolation.json`). The final focused source checkpoint is `target/local-artifact-checked-source-hashes.json`. The earlier workspace and eight composed-host results precede only the focused local-envelope correction; they are not claimed as a fresh full matrix on the final source. The Confluence host remains unrun and required.

## Development bundle

The normal build, bundle preparation/verification/installation and installed offline version, artifact discovery, full export schema and missing-object inspection passed. The installed executable is `target/craft-local-artifacts/bin/craft.exe`, archive `target/craft-0.1.0-windows-local-artifacts.zip`. Installed inspection creates no profile database and reports local-only evidence. Installed discovery still refuses Confluence page update. No PATH, host skill directory, live tenant or real credential changed.

Binary SHA-256: `b0933297b3ee2396a0f766f21c8b46e9919634949b709770ab2ba38b92d29487`. Contract SHA-256: `d34e2befd8f9136772fe17172e5e1865aa603c764689665d01c55c47203bf047`. Archive SHA-256: `bb7674ad7b409a5bc403dedcb0a4fc4e09c3da432eb5120f42d401f42d2b9625`. The bundle exposes 78 commands. Its qualification is partial due to the required Confluence host refusal; it is not a qualified release.
