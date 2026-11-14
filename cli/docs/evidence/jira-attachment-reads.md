# Jira attachment read implementation

Date: 11 November 2026, Windows development workspace. The source registry exposes 75 commands, including attachment get/list/settings. Implementation paths are `crates/craft-provider-jira/src/attachments.rs`, `crates/craft-cli/src/attachment_reads.rs` and its command adapter, public dispatch/registration, and appended site-read policy allowances. Binary transfer and attachment mutations remain unfinished; the aggregate task stays unchecked.

The native adapters use fixed GET routes, normalize bounded inert metadata/settings, omit returned URLs and author presentation, retain unknown values, validate exact source issue and refuse duplicate/malformed/over-limit arrays. Local buffered continuation has no fabricated native offset. Actor/grant/site/scope/captured policy checks bracket native/cache work; final policy checks follow access/freshness observations. Format-four records explicitly add three allowances, with a thirteen-operation bound. Older formats never inherit them.

Observed focused checks:

- Six provider groups passed in `target/attachment-provider-tests.log`, covering scalar normalization, integer/string IDs, URL/presentation omission, unknown settings, exact routes, no native paging, duplicates/foreign/missing evidence and bounds. Final qualification includes the aligned 1,000-entry boundary.
- `cargo test -p craft-runtime --test issue_read_policy --locked --offline`: thirteen tests passed, including legacy-format rejection and all thirteen allowances. Log: `target/attachment-read-policy-tests.log`.
- `cargo test -p craft-cli --test jira_issue_reads_host --features synthetic-acceptance attachments --locked --offline`: four groups passed with actual TLS, synthetic credentials and SQLite buffering. They cover get/list/settings, projections, one issue GET across buffered remainders, changed cursor projection, explicit policy/revocation before auth, missing scope/denial/404/bad evidence with renewal, and final clock-triggered revocation suppressing content while preserving renewal. Log: `target/attachment-composed-final.log`.
- `cargo test -p craft-cli --test local_interface public_attachment --locked --offline`: passed schemas, typed flags and malformed/ID/cloud/cursor/limit/projection/budget refusal before state. Log: `target/attachment-public-tests-fixed.log`.
- Feature-enabled Clippy for changed crates passed in `target/attachment-clippy-fixed.log` before the final public-input test addition. Final matrix evidence will be recorded below.
- `python tools/contracts.py write`: exported 75 commands. Companion-skill compatibility binds the new snapshot.

Diagnosed failures remain in `target/attachment-clippy.log` (Windows default encoding corrupted Unicode fixtures; duplicate same-limit branch) and `target/attachment-public-tests.log` (test called a private dispatch helper). The encoding rewrite was reversed with explicit UTF-8 reads, the branch combined without suppression, and the test now serializes the public typed action directly. No product control or assertion was removed.

Authorized reviewer `/root/skill_review` inspected the provider, host, policy, tests and public seam read-only and found no blocker. The shared pager's tighter limit was made explicit. The reviewer ran no tests or binaries. No tenant or real credential was used. Native-vault/live-provider qualification and binary transfer remain separate.

## Final qualification and installed build

`python tools/verify.py > target/attachment-read-qualification.log 2>&1` passed the full seventeen-command local matrix, zero reused commands and no source drift. It covered workspace tests, both Clippy modes, all nine composed hosts, 125 passing feature-runtime cases (three native opt-ins ignored), provider/contracts/dependency/skill/bundle checks and production isolation/nondebug refusal. The independent impact-selector selftest passed in `target/attachment-impact-selftest.log`; attachment-host-only edits now select the issue-read host. The qualified source checkpoint is `target/attachment-read-qualified-source-hashes.json`. No native-vault, secure-console or live-provider opt-in ran.

`cargo build -p craft-cli --locked --offline` restored the normal build. Binary SHA-256: `9a409756b290ab94806a69baaa1ede2ee656b22ded706c8d1bda62395344ba0d`; contract SHA-256: `7ddf5295f7025b2f0e9f8be935f65aae17ec68d129cf9bff51eaa999ea46e427`.

Bundle preparation, verification and installation passed for `target/craft-0.1.0-windows-attachment-reads.zip`, SHA-256 `8042aa76bd25bba469a11da01326bed27a12580bb210f57e427feaaa3f98e551`. The installed executable is `target/craft-attachment-reads/bin/craft.exe`. Offline version and attachment discovery returned exactly get/list/settings; results are in `target/attachment-read-installed-version.json` and `target/attachment-read-installed-discovery.json`. `target/attachment-read-unexposed-page-update.json` confirms Confluence page update still returns COMMAND_UNKNOWN. No PATH or host skill installation changed. This is an internal development bundle, not a qualified release.

Confluence update activation was separately rejected by automatic approval review because its native behavior may overwrite another person's divergent unpublished editor draft. No enabling patch executed. The specific asynchronous authorization question remains pending; this read-only delivery neither grants nor assumes that authority.
