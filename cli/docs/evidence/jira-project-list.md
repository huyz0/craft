# Jira project-list host acceptance

Date: **10 November 2026**, Australia/Sydney. Internal integration only. No public cloud registration, live tenant call, real credential, release or publication was enabled.

## Behavior and artifacts

The [host contract](../jira-project-list.md) composes `crates/craft-provider-jira/src/projects.rs`, `crates/craft-cli/src/project_list.rs`, verified service access, immutable runtime cursors and full projected-envelope measurement. Site-link normalization is constrained in runtime transport to the exact configured site and declared project-search route. Requests keep the bound Cloud-ID gateway; site URLs never become authenticated destinations. Explicit advertised offsets handle empty/short pages without arithmetic guesses. Returned server page-size clamps constrain link validation without changing the originally requested size.

Preparation validates input, target, projection, policy and recovery output. Runtime cursor checks precede cached authentication. Source access checks bind the actual verified actor/grant/tenant and declared scopes before native or cached records and before output. Observation, exact site, paging and independent credential-cache effects survive success/recovery/failure envelopes. OAuth identity/token HTTP uses an invocation-bounded client view retaining the same issuer, TLS client, freshness origin and private fixture provenance.

Core now owns the shared `CredentialCacheUpdate` type; the existing create API re-exports it with the same wire shape. `PageObservation` gains schema derivation. The paging engine exposes a shared conservative recovery probe and read-only pre-acquisition cursor checks. Public schema/discovery remain unchanged. The CLI manifest adds only an explicit feature-gated integration target; no dependency/version/feature edge or lock change was introduced. The 283-package inventory was refreshed for the CLI manifest hash. Verification and fixture-isolation tooling now include the new target.

## Observed checks

Five provider tests in `crates/craft-provider-jira/tests/projects.rs` passed. They verify native route/filter/scope declarations, compact normalization, extension omission, explicit continuation through empty/short pages and changing totals, malformed flags/counts/fields, duplicate IDs/keys, forged normalized continuation, HTTP denial and pure input/target refusal. These use a normalized context double and do not prove transport behavior alone.

One new runtime test verifies exact site-link validation, fixed gateway reconstruction and refusal of foreign origin/path, changed filters/page size, duplicate parameters, invalid encoding, credentials, fragments and malformed site constraints. One new default CLI test verifies pure host preparation refusal for target, query, page size, projection, site and insufficient recovery budget.

Nine composed tests in `crates/craft-cli/tests/jira_project_list_host.rs` passed in the final full gate. They execute the actual host, provider, service cache, sealed HTTPS context and private SQLite state through debug-only owned TLS fixtures:

1. First native page, buffered resume with no project fetch, empty nonterminal page and terminal page. Original observation survives buffering. All project GETs retain the bound gateway path, fixed filters and fixture host.
2. Foreign/duplicate/changed-filter/fragment links, non-progressing offsets, ambiguous metadata and a 403 refusal. Each stops after the qualified attempt, retains the independently confirmed renewal receipt and never follows the supplied link.
3. Changed-query cursor refusal before cached identity or acquisition; issuer/product request counts remain unchanged.
4. Missing project-read scope and wrong configured actor prevent every project fetch.
5. A 4,000-character first project name returns `ITEM_TOO_LARGE` within a 3,072-byte encoded envelope, retaining its cursor. A larger-budget buffered resume returns that first record before the next and retains request identity.
6. A stalled project read observes the invocation deadline without retry or successful continuation.
7. A disconnected project GET fails without retry or continuation.
8. A stalled cached-identity read observes the same invocation deadline before any project request.
9. Server-clamped native pages advertise a next offset and smaller maximum; following requests keep requested `maxResults=50`, use the advertised offset and return the remaining ID.

Secret scans run after releasing their owned grant locks and find no synthetic credential or rejected-content sentinel in private state. They are not live credential or native-vault qualification. The fixtures remain absent by default and reject non-debug compilation under the existing feature guard.

Development failures are retained rather than relabeled as acceptance. The first compilation exposed a missing client clock accessor. The initial composed log `target/jira-project-list-focused.log` found Windows fixture scans attempted while grant locks were held and a deadline starting before schema preparation. Cleanup order and a request-observed manual-clock deadline corrected them. `target/jira-project-list-focused-corrected.log` records the seven-test intermediate pass. Adding identity-deadline coverage exposed a physical TLS startup race in a 100-ms test budget, retained in `target/jira-project-list-composed-final.log`; both deadline tests now start with a ten-second physical allowance and advance their manual deadline only after the intended request is observed. No assertion was removed. The initial Clippy log surfaced unused fixture helpers/imports; the import was removed and disconnect/request-count controls are now exercised by behavior tests. Corrected Clippy passed without suppressions.

Final `python tools/verify.py` passed in `target/jira-project-list-gate.log`: **332 default Rust passes**, **sixteen composed-target passes** (seven create plus nine project-list), and **117 feature-runtime passes**. This is **350 distinct passing tests**, zero failures, or **465 passing executions** with 115 repeated runtime cases. Five distinct opt-ins remain ignored (eight occurrences). Both Clippy configurations, formatting, unchanged public snapshot and its controls, provider/source inventory, dependency/layer guards, planning checks and synthetic-feature isolation passed. Existing Cargo incremental hard-link fallback warnings did not prevent successful compilation or Clippy.

`cargo build --workspace --locked --offline --no-default-features` passed after fixture testing in `target/jira-project-list-default-build.log`. Default binary SHA-256: `d49c26b054dff3fd908922930b661aa5a959834db8492730c25cc18a0585c000`. The rebuilt executable returns an empty `provider list`; public discovery still contains twenty local operations. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Source/document/binary hashes and final checks are in `target/jira-project-list-checkpoint-hashes.json` and `target/jira-project-list-checkpoint-validation.json`.

## Limits and next work

No independent reviewer was invoked or claimed. Public consequential auth activation still requires the existing independent review and real sandbox/public qualification. The host's manually configured site/Cloud-ID association is a validation constraint, not discovered access evidence. Live page shapes, custom domains, current per-record permission changes, process-kill/power-loss recovery and other operating systems remain unverified. Native-vault, console and vendor-schema opt-ins were not rerun; earlier schema-four native-cache evidence remains historical and separate.

The [earlier Jira metadata acceptance](jira-create-metadata.md) remains applicable to its own create-context slice; this record extends the read surface and does not replace that evidence. Jira-context, cursor-store and bounded-listing remain unchecked for their full scopes and dependencies. Next, connect typed project-list arguments/profile selection and complete candidate dispatch while retaining the current activation boundary, then continue remaining Jira context/CRUD, recovery and Confluence workflows. The overall CLI delivery goal remains active.
