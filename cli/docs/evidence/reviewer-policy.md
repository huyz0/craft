# Native reviewer storage and explicit administration

Implemented on 10 November 2026. `review policy get/set` persists exact per-profile/per-space reviewer intent in schema twelve. Strict format-one rows bind installation, issued grant, profile, space, service actor, revision, enabled state, reviewer/exclusion lists and quorum. Missing rows grant no eligibility. Native-to-workflow conversion rechecks the complete enabled persisted record. [Usage](../reviewer-policy.md) distinguishes configuration from actual human approval; the [internal verifier evidence](review-contract.md) remains separate.

## Observed focused checks

- `target/reviewer-policy-native.log`: five tests pass apply/preflight/CAS refusal, canonical lists, disabled-before-actor-transfer, stale/disabled capture refusal, duplicate/self/overlap/quorum/foreign-target rejection, payload bounds, exact eleven-to-twelve migration, retained existing configuration, conflicting-table rollback, trigger refusal, corrupt/foreign/unknown payload refusal and one-winner concurrent updates.
- `target/reviewer-policy-public-complete.log`: two subprocess tests pass JSON and flags, cross-process persistence, complete response schemas, stale revisions, disablement and absent credential state. Malformed repository hints cannot select or interfere with explicit administration. The native bridge accepts the actual record and refuses forged/disabled/stale captures. Invalid intent, unknown fields, missing explicit profile and insufficient output capacity fail before state initialization.
- `target/reviewer-policy-contracts-final.log`: public contract generation contains 39 operations. Set discovery classifies a local access change, not a provider write or human identity proof.
- The command receipt mapper includes an injected unknown-commit check: proposed intent remains separate from observed policy, configuration uncertainty and both false verification markers survive bounded output. Its actual result belongs to final qualification.

## Parser correction and retained failures

The first public test failed because the common action-flag guard counted Clap's `Cli` argument group as an explicit action flag, rejecting JSON plus global invocation controls. `target/reviewer-policy-input-guard.log` records the isolated failing assertion. The guard now walks declared argument metadata, ignoring groups; its paired regression also requires an actual action flag plus JSON to remain rejected. The corrected public tests pass.

Initial compile/non-exhaustive and subsequent wrong Clap mutable-parser-method errors are retained in `target/reviewer-policy-compile.log` and `target/reviewer-policy-public-final.log`; final compilation passes in `target/reviewer-policy-compile-final.log`. Earlier public/group failures remain in `target/reviewer-policy-public.log`, with successful follow-ups in `target/reviewer-policy-public-qualified.log` and the final complete log. Failed runs are not represented as passes.

Historical migration fixtures now remove the newer reviewer table when constructing older schemas and assert current schema twelve. The profile-settings corruption fixture restores the actual current version after its unsupported-version test, so table-migration conflicts cannot mask settings validation. Production migrations never drop or repair foreign tables.

## Independent review

Authorized read-only reviewer `/root/activation_review` found no concrete blocker in native binding/validation, explicit-profile administration, CAS/disable-transfer ordering, complete receipts and uncertain proposals, native conversion recheck, transactional empty-table migration or parser metadata handling. The reviewer executed no tests and accessed no state, credentials or tenant.

## Limits

Configuration is operator-declared eligibility. Live identity qualification, complete current root/reply inventory, authenticated plan/design/Jira mapping adapters, public review verification and materialization remain unfinished. No real account, credential, permission, page, issue or comment was changed. Settings/credential formats, dependencies and lockfile remain unchanged. Local owner authority is not a multi-principal worker boundary. The owning review-authority task remains unchecked.

## Final local qualification

`python tools/verify.py` passed all fourteen full local checks in `target/reviewer-policy-verification.log`: planning, dependency/layer and provider inventory/selftests/routes, formatting, public contracts and negative controls, workspace tests, all nine composed hosts, feature runtime tests, both Clippy modes and synthetic-feature isolation. The injected unknown-commit mapper and real-argument/input exclusivity tests pass in this run. Native opt-ins and live tenant qualification were not run.

`cargo build --workspace --locked --offline` restored the default executable in `target/reviewer-policy-build.log`. Actual discovery returns 39 commands and includes both reviewer-policy operations; public live review verification remains unknown/unavailable. Binary SHA-256: `4bb3bedcb2dd31dcc76fb6815500fada709b1358d15d61e4ba4e35aac9b7d8fe`. Contract SHA-256: `f350957d6a0a91f9a875f708b30eb7d324e51ca5f0ebf710d1c9e31360665085`. Machine-readable identity and discovery are retained in `target/reviewer-policy-result.json` and `target/reviewer-policy-discovery.json`. Qualified source hashes are captured after final prose validation in `target/reviewer-policy-qualified-source-hashes.json`; the evidence-only check is `target/reviewer-policy-documentation.log`.
