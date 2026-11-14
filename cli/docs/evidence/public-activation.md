# Public command activation

Observed on 10 November 2026 on Windows. The development executable exposes 32 operations and two cloud providers. Added public operations: auth provisioning, policy get/set, Confluence space get/list and page get/list/search/create, and Jira issue get/search/create.

## Artifacts and checks

- Executable: `target/debug/craft.exe`, SHA-256 `d3489b97dd0e44ec7b2512e7aebd06183ba856d55a9e0e69a43c20206c4de9d8`.
- Public contract: `schemas/local-contracts.json`, SHA-256 `4a2929ea1843a6ebabd1b3b28516c107f9e7150acc15567311f97f2e8b47ec86`.
- Actual discovery and summary: `target/public-activation-discovery.json`, `target/public-activation-result.json`.
- Selected CLI validation: `target/public-activation-validation.log`. The stale profile assertion expected no providers; it was corrected, and all seven profile tests passed in `target/public-activation-profiles-final.log`. Earlier CLI checks, including 112 unit tests, passed before that failure.
- All nine composed host targets passed in `target/public-activation-hosts.log`. Both Clippy modes passed in `target/public-activation-clippy.log` and `target/public-activation-feature-clippy.log`. The default build was restored in `target/public-activation-build.log`.
- Public interface checks cover discovery, schemas, unknown live capabilities, unsupported leaves and invalid-input refusal before state/secret access. Candidate tests use an isolated local registry.
- `target/public-activation-qualified-source-hashes.json` captures 418 qualified inputs. `target/verification-state.json` records the combined successful checks as `focused-with-failed-target-correction`. This is focused qualification, not a new full-suite pass.

Earlier failed logs remain available: `target/public-activation-cli-tests.log` (old non-exposure assertions), `target/public-activation-cli-tests-second.log` (restricted hard-link fixture), and the stale-provider assertion in the selected validation log. No failed command was reported as a pass.

## Independent review and limits

The authorized read-only reviewer `/root/activation_review` found no concrete blocker in the final public registration changes, including typed provider registration, skipped leaves, startup registry, capability unknowns, object/Boolean examples and page-create-only exposure. Earlier auth/policy and verifier reviews are recorded in their owning evidence. The reviewer did not edit files or access credentials or the tenant.

`live_verified` remains false. Browser inspection found the test site signed in and no listed service account on its service-account onboarding screen. No account, credential, page, ticket or permission was created or changed. Secure service-account setup is needed before live CLI qualification. Page-update editor behavior, Linux/macOS native acceptance, broader CRUD, broker/team coordination and plan-review-ticket orchestration remain incomplete. SQLite schema remains eleven; dependencies and lockfile are unchanged by this activation.
