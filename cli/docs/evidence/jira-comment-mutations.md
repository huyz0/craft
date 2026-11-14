# Jira comment mutation implementation

Date: 11 November 2026, Windows development workspace. [Comment reads](jira-comment-reads.md) remain separate evidence. The implementation adds public create/update/delete through `crates/craft-cli/src/comment_writes/`, its host, typed ADF and native preparation under `crates/craft-provider-jira/src/comments/`, and five versioned project mutation allowances. Public command count is 72. The task remains unchecked because its prerequisite ledger tasks are incomplete; this implementation is locally qualified.

Private frozen preparation binds exact project/issue/comment, original author, fingerprint, actor/grant and original dual-clock lifetime. Updates preserve omitted fields and refuse null; project visibility compiles to native null. Native own/all checks and separate local own/any allowances remain distinct. Fixed routes/scopes carry no arbitrary HTTP bypass. Native checks are non-atomic; no Jira comment CAS is invented.

The host rechecks captured policy before authentication, through reads and at the claimed-send boundary around freshness. One durable claim authorizes one attempt. Confirmed comment identity survives incomplete optional responses and uncertain local journal acknowledgment. Foreign, extra and unavailable evidence stays visible without becoming attribution or replay authority. Default write output strips ADF after verification while retaining fingerprints/privacy.

## Focused observed checks

- Provider comment tests: fourteen cases passed in `target/comment-provider-focused.log`, covering reads, exact payloads, original-author own/any permission, fingerprint/privacy drift, both clocks, typed input and rich-content limits. The method-specific refusal correction is also included in final qualification below.
- `cargo clippy -p craft-cli -p craft-provider-jira -p craft-runtime --all-targets --features craft-cli/synthetic-acceptance --locked --offline -- -D warnings`: passed before public wiring, log `target/comment-write-clippy.log`.
- `cargo test -p craft-runtime --test issue_mutation_policy --locked --offline`: ten passed. New cases reject every comment allowance in formats one through four, require explicit format-five updates and preserve independent own/any authority and all thirteen allowances. Log `target/comment-mutation-policy.log`.
- `cargo test -p craft-cli --lib comment_writes --locked --offline`: five passed, including lost response, uncertain local acknowledgment, foreign/additional effects and unavailable first/later journal observations. Log `target/comment-host-unit-final.log`.
- `cargo test -p craft-cli --test jira_create_host --features synthetic-acceptance comment_writes --locked --offline`: fourteen passed. Owned TLS fixtures use synthetic credentials and actual journal claims; they cover POST/PUT/DELETE, no replay after success/refusal/loss, incomplete success bodies, original-author denial, native any denial, fingerprint drift, policy revocation before auth/after claim/during final clock observation, and small output-budget refusal. Log `target/comment-composed-host-final.log`.
- `cargo test -p craft-cli --test local_interface public_comment_writes --locked --offline`: passed public schema/response examples and malformed/pre-state refusal cases. Log `target/comment-public-refusal.log`.
- `python tools/contracts.py write`: generated 72 operations. Skill compatibility is bound to the new snapshot.

## Final qualification and installation

`python tools/verify.py > target/comment-write-qualification.log 2>&1` passed the full seventeen-command local matrix, zero reused commands and no source drift. It covered workspace tests, both Clippy modes, all nine composed TLS hosts, provider/contracts/dependency/skill/bundle checks, 125 passing feature-runtime cases (three native opt-ins ignored), production isolation and nondebug synthetic-feature refusal. `target/comment-write-qualified-source-hashes.json` retains the qualified tracked-source checkpoint. No native-vault, console-handoff or live-provider opt-in ran.

After qualification, `cargo build -p craft-cli --locked --offline` restored the normal build. Only prose/evidence edits followed. Binary SHA-256: `5efcb0c0f956aa2c394cbb4042184fb393f61b5c4660258068264016939df6ce`. Contract SHA-256: `cc3ffe655f2214de9b79f2eb38eb78fe8a8aecaf07db9ca898e9a03722071c99`.

`tools/bundle.py prepare`, `verify` and `install` succeeded for `target/craft-0.1.0-windows-comment-writes.zip`, SHA-256 `88670922dd47754daa7486ccf36894515453f2069c97b645377d19b1f5374c8f`. Installation is at `target/craft-comment-writes`. Installed offline `version` and `command find --query jira.comment --limit 10` succeeded and returned exactly get/list/create/update/delete. Results are retained in `target/comment-write-installed-version.json` and `target/comment-write-installed-discovery.json`. No PATH or host skill directory changed. This is an internal development artifact, not a qualified release.

Post-qualification hash comparison found only handoff/documentation Markdown changes, with no implementation, tool, schema or skill drift. Final guidance closure selected planning validation only; `target/comment-write-guidance-check.log` records its result. No Rust or provider matrix was repeated for prose closure.

## Diagnosed failures and review

`target/comment-adf-tests.log` retains the first failing unsupported-attribute test. Serde internally tagged unit variants discarded unknown attributes; empty struct variants now reject them, including rules, hard breaks, marks and project visibility. `target/comment-host-check.log` retains the initial compile errors, fixed at their exact type/borrow boundaries.

`target/comment-composed-host.log` retains the initial fixture failures. The plaintext scan tried to read held credential-lock files; it now runs after releasing the lock/fixture, matching existing host tests. The revocation trigger checked Unknown, while the durable preflight state is SendStarted. Correcting that trigger made the isolated and complete suites pass without relaxing assertions, time limits or product controls. The isolated corrected logs are `target/comment-claim-revocation-fixed.log` and `target/comment-late-revocation.log`.

Authorized independent reviewer `/root/skill_review` inspected provider, policy, host and public wiring read-only. Findings corrected method-specific refusal sets, journal-read uncertainty, output-capacity enforcement and final policy/freshness ordering. The reviewer reported no remaining blocker in these source scopes and ran no Cargo, executable or verification processes.

No real tenant, real credential, native vault or other platform was used. Comment-specific live reconciliation/adoption, host skill loading and full CLI delivery remain unfinished. A fixture pass does not establish live eligibility or compliance.
