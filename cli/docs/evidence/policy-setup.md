# Local permission setup evidence

Date: 10 November 2026. No real tenant, credential provisioning, release or deployment occurred. The user authorized reviews, and an independent read-only activation review has completed without a code blocker in its stated scope.

## Implemented behavior

`crates/craft-cli/src/policies.rs` and `policies/records.rs` implement typed get/set adapters over six existing permission stores. Administration requires an explicit trusted profile and exact kind-specific resource target. Strict operations, actor, enabled intent, apply, revision and output checks precede state. The selected profile supplies the actual grant. No vault or provider request is used. Existing revision-CAS setters freeze complete output before commit. Responses distinguish stored policy from a proposal retained after uncertain commit and always report live_verified false.

`args.rs` and `dispatch.rs` now provide skipped nested actions, structured decoding and complete handler-envelope preservation. Candidate schemas describe the complete normalized configuration/recovery receipt. The root is not registered publicly pending consequential review. No database schema, native credential format or dependency changed.

## Observed checks

Four setup tests pass in `target/policy-setup-focused-final.log`: flag/schema agreement and closed operation kinds; actual set/get/disable/reopen across six stores with issued profile grants, independent cascade intent and stale CAS refusal; invalid/implicit administration without state creation; and proposed-versus-stored uncertainty receipts. The dispatch test passes in `target/policy-setup-dispatch.log`, retaining exact arguments, request identity and complete configuration outcomes.

The initial expanded fixture failed to compile because it moved invocation options across loop iterations; the fixture now constructs independent options per iteration. Its failed output remains `target/policy-setup-focused.log`. The first Clippy run found an unnecessary borrow in output preflight, corrected without suppression; final workspace all-target feature-enabled Clippy passed in `target/policy-setup-clippy-final.log`.

The full gate passed in `target/policy-setup-gate.log`: 665 distinct tests, zero failures (786 executions, 121 repeats). Both Clippy modes and planning/contract/provider/dependency/layer/isolation checks passed. The default offline build was restored in `target/policy-setup-default-build.log`. Its initial 270-file source/toolchain/tool baseline is `target/policy-setup-tested-source-hashes.json`. One later descriptor-example correction makes the set success example show a confirmed stored record instead of a read-style absent record. All four setup tests pass on that corrected source in `target/policy-setup-final-source-tests.log`. The final source delta and binary/schema hashes are retained in `target/policy-setup-final-validation.json`; this is not an unchanged-source whole-gate claim.

## Delivery boundary

The existing secure provisioning composition and these setup adapters are concrete review material. The project requires independent consequential review before enabling auth/permission changes. The user answered “authorize any review”. The independent reviewer found no activation code blocker in provisioning/setup/authority boundaries, with actual public registration and live/native acceptance still outside that review. Public activation and native/public/live qualification remain unfinished. Service credentials and the actual supported test-site actor still need secure provisioning/verification. Broader field/project-context setup, plan publication orchestration, human review and ticket materialization remain in the full CLI goal.
