# Windows artifact export acceptance

Date: **10 November 2026**, Australia/Sydney. This qualifies the internal Windows output API against owned filesystem fixtures. The broader artifact task remains unchecked. No public export/auth/provider command or consequential runtime capability was enabled, and independent review has not occurred.

## Requirement audit

| Requirement | Implementation and observed evidence |
| --- | --- |
| Explicit output authority | `craft-platform/src/outputs.rs` exposes a separately constructed existing-only `OutputRoot`; input roots and receipt/provider paths cannot implicitly grant it; export excludes its own immutable root using checked directory identity before staging |
| Safe destination and bounded private staging | Checked relative paths and parent guards refuse escape, device/stream aliases, linked parents/targets and nonregular replacement; private create-new staging is bounded to at most 1 GiB |
| Atomic policy distinction | Create-new is default; replacement is explicit. Native tests preserve existing directories/files, publish Unicode/multi-chunk bytes and yield one complete winner in a concurrent create-new race |
| No ambient source-path reopen | New Windows wrappers retain a delete-capable staging handle and publish/discard through `SetFileInformationByHandle`; attempts to rename staging or its parent through another path fail while that guard is held |
| Correct complete receipt | Runtime `export_file` reuses streaming verification and returns the source name/media/size/digest/provenance with the selected exported path only after publication |
| No corrupt output or false absence | Corrupted source data leaves a selected old file intact; injected pre-publication and post-publication acknowledgment failures remain distinct; cancellation after success preserves the receipt |
| Unqualified platforms/activation remain unavailable | Other native platforms return unsupported, and no parser/catalog entry exposes export. Public recovery, staging cleanup and independent consequential review remain open |

The [contract](../workspace-outputs.md) and [review package](../review-artifact-exports.md) explain native FFI safety, replacement metadata, recovery and activation limits. New unsafe code remains scoped to the existing Windows leaf module; no raw pointer/handle is accepted from a caller or serialized. Current source selection and ordinary private store behavior retain their previous tests.

## Observed checks

`cargo test --locked --offline -p craft-platform --test workspace_outputs` with native filesystem access passed **five tests** in `target/workspace-output-platform-native-output.log`. The first restricted run passed four and failed hard-link fixture creation with access denied; its output remains `target/workspace-output-platform-output.log`. No test was skipped or weakened.

`cargo test --locked --offline -p craft-runtime --lib artifacts::tests` with native access passed **twenty tests**, including four new export cases, in `target/workspace-output-artifact-native-output.log`. This includes real source verification/rename plus injected before/after acknowledgment boundaries. `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` passed in `target/workspace-output-clippy-output.log`.

An initial integrated native-access gate passed **236 named Rust tests, zero failures and five ignored opt-ins** in `target/workspace-output-native-verification-output.log`. The bounded readiness/release race fixture passed there. A subsequent audit moved the invocation check after staging synchronization, immediately before native publication. The exact-boundary cancellation test passed in `target/workspace-output-cancellation-output.log`: the selected old file is preserved and owned staging is reclaimed. The follow-up integrated gate passed in `target/workspace-output-final-native-verification-output.log`.

The final immutability audit added protected-root exclusion by native directory identity. `target/workspace-output-storage-protection-output.log` records its passing test: neither replacement nor creation inside the store is allowed, including a root selected inside another captured object's directory. The first integrated run after this guard refused five earlier export fixtures because they placed outputs inside the protected store; `target/workspace-output-protected-native-verification-output.log` retains that failure. The fixtures now use separate owned sibling output directories, while the explicit protected-store refusal case is unchanged. The artifact suite passed **twenty-two tests** in `target/workspace-output-external-destination-native-output.log`.

Final `python tools/verify.py` completed with **238 Rust tests passed, zero failed and five ignored opt-ins**, plus the contract/inventory checks and Clippy, in `target/workspace-output-complete-native-verification-output.log`. This development gate does not run the separate native-vault/live-provider opt-ins. `cargo build --workspace --locked --offline` passed in `target/workspace-output-complete-build-output.log`; Cargo reported incremental-cache hard-link warnings and copied files instead. Source, manifest, lock, public contract and executable hashes are recorded in `target/workspace-output-checkpoint-hashes.json`. The debug executable SHA-256 is `21e7793e5b08777e244bc233ad5553c6d77f98f7b07d3d25b3437a996a914daf`. Planning checks passed with 85 tasks and 14 complete. Task checkboxes remain unchanged.

## Limits

This is author verification on Windows x86_64 MSVC, Rust 1.99.1. ReFS/SMB/mapped-volume behavior, power loss, malicious same-principal/privileged writers and real I/O failures are not established by these fixtures. The buffer/access semantics were checked against cached windows-sys 0.61.2 source and official Microsoft API documentation; no new package or version was introduced.

Default publication and explicit replacement are not a distributed compare-and-swap service. Replacement changes the selected file's metadata to that of private staging. An unknown native failure can leave staging; durable export intent, orphan cleanup and public inspection/reconciliation are not implemented. Output-envelope budgeting and independent review remain activation prerequisites. No real credential, tenant object, commit, publication or release was changed.
