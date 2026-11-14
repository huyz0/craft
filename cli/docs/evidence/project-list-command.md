# Project-list candidate and configuration acceptance

Date: **10 November 2026**, Australia/Sydney. Internal candidate integration. No public cloud provider/command, live tenant change, real credential provisioning, release or publication was enabled.

## Changed behavior

The [command contract](../project-list-command.md) connects typed scalar arguments and structured input to exact profile/site selection, private read configuration, the [qualified host](jira-project-list.md) and complete-result dispatch. Source is in `crates/craft-cli/src/project_list/command.rs`, the skipped Jira action in `args.rs`, `dispatch.rs`, runtime `project_read_policy.rs` and the schema-five migration in `profiles.rs`. Host callbacks recheck the captured record before acquisition and throughout native/cached source checks, including final publication.

Command arguments cannot supply grant, actor, site origin, enabled state or revision. Private records bind those values to the actual installation/profile/issued grant and exact site. The setter requires apply, expected-revision CAS and caller output preflight; it has no public administration command. This is local configuration, not independent approval or immutable audit history.

SQLite migrates schemas one through four atomically to five. Settings format, journal rows and existing cursor payload formats are unchanged. Previous migration fixtures now remove all later tables when simulating an earlier schema so an unrelated table conflict cannot mask the intended negative control. No package version, manifest, dependency inventory, lock or public response schema changed.

## Observed validation

Four new command tests passed in `target/project-list-arguments-focused.log`:

- Flags and JSON share scalar/default semantics; unknown authority fields and wrong types fail. Public parsing/catalog still exclude the candidate.
- Invalid cloud/query/cursor/projection and insufficient recovery output produce failure with original request identity without creating the selected state root.
- Missing/disabled configuration fails; an exact enabled service-profile record supplies its actual issued grant, and a different explicit cloud cannot widen that profile.
- A bounded subprocess with an isolated working directory proves explicit profile bypass of malformed repository hints, repository profile precedence over the active profile, foreign hinted-site refusal and explicit cloud precedence over that hinted site. The parent process's working directory is unchanged.

Four new runtime configuration tests passed in `target/project-read-policy-focused.log`, alongside the existing profile/create-policy/provider-cursor targets (forty passes in that focused run). They cover apply/output refusal, canonical site storage, reopen, CAS/revocation, competing connection writers with one revision winner, wrong site/actor/tenant/type/ID, delegated/other-provider refusal, corrupt installation/profile/grant/revision/target/site records, unknown fields and duplicate JSON properties. The later delegated/provider controls also ran in the full gate.

The schema-four migration case preserves settings, profile/grant, active selection, a local-profile cursor, provider cursor remainder/observation, existing create policy and journal record. A foreign project-read table during schema-one upgrade rolls back intermediate operation/create-policy/provider-cursor DDL and preserves the old version and profiles. Unexpected triggers are refused. Earlier schema-one/two/three tests and controlled deadline rollback also passed in the full gate.

One new dispatch test decodes the correct Jira action and preserves an oversized-item error, recovery cursor, cache receipt, warning, output policy and original request identity through encoding. The existing create-dispatch test still passes under the shared candidate-handler seam.

The composed target now contains ten tests. Its new test uses actual parsed candidate arguments, configured profile, private record, cached service access, provider host and owned TLS servers. Six scenarios run in that case: success; revocation before auth; revocation during auth; revocation after the resource fetch but before publication; an enabled revision change after the fetch; and revocation during identity verification before buffered output. Request counts prove no extra acquisition/resource fetch where refused. Late failures expose no page/items and preserve confirmed renewal receipts when present. Changed enabled configuration returns `VERSION_CONFLICT`; disabled configuration returns `POLICY_DENIED`. The initial five-scenario focused pass is retained in `target/project-list-command-composed.log`; the complete six-scenario case ran in the final gate.

`target/project-list-command-initial.log` is an early three-command-test pass. Its shared name filter selected zero runtime policy tests, so it is not evidence for those tests. The separate focused runtime log and final gate supply that evidence. Focused Clippy passed in `target/project-list-command-clippy.log` without suppressions. Temporary documentation link checks found the not-yet-written evidence page; final checks run after this record was created.

Full `python tools/verify.py` passed in `target/project-list-command-gate.log`: **341 default passes**, **seventeen composed-target passes** (seven create and ten project-list), and **117 feature-runtime passes**. This is **360 distinct passing Rust tests**, zero failures, or **475 reported passing executions**, including 115 repeated runtime cases. Five distinct opt-ins remain ignored (eight occurrences). Both Clippy modes, formatting, public contract snapshot and controls, provider/source inventory, dependency/layer guards, planning checks and synthetic-feature isolation passed. The existing incremental hard-link fallback warnings did not prevent successful compilation or Clippy.

The separate Windows native service-cache restart/contention/logout test passed after migration in `target/project-list-command-native-cache.log`, using its own synthetic vault entries and owned TLS servers. Other native-vault, console, vendor-schema and operating-system opt-ins were not rerun. This does not turn the memory-vault composed command test into native/public CLI acceptance.

`cargo build --workspace --locked --offline --no-default-features` passed after feature testing in `target/project-list-command-default-build.log`. Default executable SHA-256: `2158a8eb75f9684f0ece7ac6a23107a3577d38703c509d70fc3b2329d02e0acc`. The rebuilt binary returns an empty `provider list`; discovery still has twenty local operations. Public schema SHA-256 remains `16a4ccb1d0f054a6d862d4d55a9bb66334db42abb4d7015012d1bf0de8b2dc54`. Final source/document/default-binary hashes and checks are in `target/project-list-command-checkpoint-hashes.json` and `target/project-list-command-checkpoint-validation.json`.

## Limits and next work

No independent reviewer was invoked or claimed. Public consequential auth activation still requires existing independent review and actual sandbox/public qualification. Local configuration does not supply same-principal isolation, immutable history, independent human authority or distributed fencing. There remains a race after the final check; configuration changes cannot retract already returned data or an in-flight request. SQL fault/deadline tests are not power-loss guarantees.

The broader Jira-context, cursor/listing and service-auth tasks remain unchecked for their full scopes and dependencies. The [preceding project-list host evidence](jira-project-list.md) and its linked create-metadata evidence retain their respective scope. Next, finish remaining Jira context reads and resource CRUD/recovery, operational configuration surfaces and Confluence workflow integrations, with concrete public/live review preparation before activation. Keep the full CLI delivery goal active.
