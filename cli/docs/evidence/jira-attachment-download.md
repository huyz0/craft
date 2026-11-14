# Jira attachment download

Earlier metadata-read acceptance is in [attachment read evidence](jira-attachment-reads.md). This record adds binary download and preserves that earlier scope.

The internal binary transport and public Jira attachment adapter stream one compiled content GET into bounded immutable artifact staging, then guarded output publication. Source paths are `crates/craft-runtime/src/transport/downloads.rs`, `crates/craft-provider-jira/src/attachment_download.rs`, `crates/craft-cli/src/attachment_download.rs` and its command adapter. `craft-platform` output authority now supports an additional protected private root, and read-policy format five isolates the appended download operation from older formats.

Observed focused evidence before final qualification:

- Twelve runtime transport tests passed in `target/binary-download-reviewed-tests.log`, with runtime all-target Clippy in `target/binary-download-reviewed-clippy.log`. A raw-wire unsupported-transfer-coding regression failed before the fix (`target/binary-transfer-coding-before.log`).
- Six initial composed groups passed in `target/attachment-download-composed-protected.log`: exact binary bytes and digest, real artifact/export receipts, create-new collisions and explicit replacement, no download redirects, oversize/digest/partial refusal, separate operation/scope authority, credential renewal, captured-policy revocation and artifact retention after late refusal. More composed interruption cases were subsequently added for final qualification.
- Provider route regression passed in `target/attachment-download-provider-tests.log`; fourteen runtime read-policy cases passed in `target/attachment-download-policy-tests.log`, including legacy download rejection and all fourteen current allowances together. Pure command validation passed in `target/attachment-download-local-tests.log`.
- Feature-enabled all-target CLI Clippy passed in `target/attachment-download-composed-clippy-fixed.log`. The original log retains a needless-borrow lint failure corrected before rerunning.
- Authorized source-only reviewer `/root/skill_review` identified transfer-coding and whole-state output-protection gaps. Both were fixed; the reviewer confirmed each resolved. The full-state regression selects an ancestor output root, enables overwrite and targets state.sqlite3, checking the database/profile remain intact while the artifact receipt survives. The reviewer ran no binaries or tests.

No real tenant or credential was used. Native vault/console/live-provider opt-ins, other-platform guarded export, public local artifact recovery, multipart upload and deletion remain unfinished. The broader binary-transfer and Jira attachment tasks stay unchecked. Final qualification and bundle identity will be recorded after they run.

## Final local qualification and installed bundle

`python tools/verify.py` passed the full seventeen-command matrix, zero reused commands and no source drift (`target/attachment-download-qualification-fixed.log`). It included workspace tests, both Clippy modes, all nine composed hosts, 137 passing feature-runtime cases (three native opt-ins ignored), contracts/dependencies/provider/skill/bundle checks and production isolation/nondebug refusal. The Jira issue-read host passed all 23 cases, including seven download groups. Source hashes are retained in `target/attachment-download-qualified-source-hashes.json`.

The first qualification stopped at feature Clippy because the new interruption fixture did not map StoreError into CraftError (`target/attachment-download-qualification.log`). That test-only conversion was corrected. Its deterministic cancellation/deadline cases then passed directly in `target/attachment-download-interruption.log` before the final qualification. No behavioral suite failure was hidden. The final authorized public-wiring review found no blocker and ran no binaries.

`cargo build -p craft-cli --locked --offline` restored the normal executable. Bundle preparation, verification and installation passed. The installed path is `target/craft-attachment-download/bin/craft.exe`, archive `target/craft-0.1.0-windows-attachment-download.zip`. Installed offline version, four-command attachment discovery and full download schema checks passed; Confluence page update still returned COMMAND_UNKNOWN. Results are under `target/attachment-download-installed-*.json`. No PATH, host skills, real credentials or live tenant changed.

Binary SHA-256: `83cfa9b78e50c9a3fac86d03cf15f826d6459887acdabe79f969dc3afe7ec374`. Contract SHA-256: `45863c3a48eb8595f8b7e5ab18774adf5b1d4962ec1f14679de54907e76b7df9`. Archive SHA-256: `5ef6d9c5e4bb09103fe5f3efdc0ea39a1d7481648c0e2b320c891a2dee725fa4`. The bundle exposes 76 commands across two providers. It remains an internal Windows development bundle, not a qualified release.
