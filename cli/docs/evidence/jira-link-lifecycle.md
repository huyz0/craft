# Jira link lifecycle qualification

Scope: provider link get/list/types, exact native-ID deletion and journal-bound read-only investigation; CLI typed inputs, policy formats and public dispatch. Source changes are in `crates/craft-provider-jira/src/issue_links/`, `crates/craft-cli/src/issue_links/`, public args/dispatch/registration and the three runtime issue-policy modules. The existing capability scenarios are in `openspec/changes/build-cloud-cli/specs/cloud-providers/spec.md` under native link resources. [Usage](../jira-link-lifecycle.md) describes the contract.

Observed focused evidence before final qualification:

- `cargo check -p craft-cli --all-targets --locked --offline` passed after correcting two swapped closure result annotations in recovery. Public wiring also passed all-target check.
- Fifteen deletion/creation-recovery native TLS cases passed through `cargo test -p craft-cli --test jira_create_host --features synthetic-acceptance link_lifecycle:: --locked --offline`. Output: `target/link-lifecycle-native-acceptance.log`.
- Two opaque cursor cases passed through the same target with `link_reads::`: projection, saved remainders across store reopen, replay, changed-source refusal and revocation before identity access. Output: `target/link-reads-native-acceptance.log`.
- Feature-enabled all-target Clippy for CLI/runtime/Jira provider passed. Output: `target/link-lifecycle-clippy-final.log`.
- Public contracts regenerated successfully with 50 commands. Output: `target/link-lifecycle-contract-write.log`.

The initial test harness used an unavailable fixture method and was corrected to inspect finished captured requests. Restricted execution then failed synthetic token exchange, including an unchanged previously qualified link-create case. Native loopback execution passed. Those failures remain in `target/link-lifecycle-acceptance.log`, `target/link-lifecycle-acceptance-fixed.log` and `target/link-lifecycle-environment-probe.log`. Lint findings in a single-element permission loop, byte-budget comparison and nested recovery check were corrected without suppressions; failed logs remain.

Authorized independent reviewer `/root/activation_review` reviewed lifecycle and public wiring read-only. It found a missing lost-deletion recovery case; source now separately covers visible/absent/replacement-ID/excluded-deletion-allowance recovery. Follow-up found no remaining activation blocker. The reviewer ran no tests and accessed no tenant, credentials or private state.

## Final qualification and executable

`python tools/verify.py` passed all fourteen selected checks in one uninterrupted full-scope run, with zero reused commands and no source drift. Output: `target/link-lifecycle-verification.log`. This includes default workspace tests/doctests, all nine composed TLS targets, feature-runtime tests, both all-target Clippy modes, formatting, contracts, provider/dependency/layer checks and synthetic-feature isolation. The lifecycle target contains 23 separately runnable added cases: eight deletion, eleven creation/deletion recovery and four reads/cursors. All passed. The two added legacy read/recovery policy regressions and public malformed-input subprocess case also passed. Retained source identities are in `target/link-lifecycle-qualified-source-hashes.json`.

`cargo build -p craft-cli --locked --offline` restored the normal executable; output: `target/link-lifecycle-build.log`. Running that executable's `help --json` reports 50 commands, including all six `jira.link.*` operations. Binary SHA-256: `14a7470f2b3c7c21350b2f2a53e7f5467e105cc1173af04d3c21d437d5945a85`. Public-contract SHA-256: `78f47f6291dc758e807ff19f95673f91dd49abc8b4c73f781719c5e02c97e1ab`. Summary: `target/link-lifecycle-result.json`.

Creation's earlier qualification remains in [link creation evidence](jira-link-create.md). Subsequent documentation updates run the planning checks only. Historical handoffs are retained in `HISTORY.md` at the subproject root; `STATE.md` now contains only current status and the next delivery action.

No live tenant, real credential, other-platform or native-vault qualification is inferred. Ignored opt-ins retain their separate qualification requirements. No ledger task is closed by this partial provider slice; reviewed ticket application and the full CLI remain incomplete.
