# Service provisioning review package

Status: **independent implementation review completed, 10 November 2026**. The composition is implemented and tested but is absent from the public parser/catalog. The executable advertises twenty local commands, including local plan/design authoring and journal inspection. Live service provisioning has not run.

## Intended behavior

The candidate takes explicit profile aliases, an expected service-account actor and exactly one secret source: interactive terminal or native pipe. Selected profiles must use service auth, share an issued grant and have valid Jira/Confluence Cloud IDs. Multiple aliases for the same product/cloud target produce one probe target. There is no guessed site, implicit repository selection or `eligible` boolean.

Under the native grant lock it refuses unresolved staging and requires explicit replacement of active credentials before reading new secrets. It acquires a service token once and verifies every selected target against the expected actor. Actual issuer acceptance and current-user proofs qualify that credential/target combination, without claiming project/space write permission.

The host freezes complete projected success and recovery envelopes before native activation. Mandatory grant, profile, local-write state, cleanup and rate evidence survive projection. Cleanup-pending success remains confirmed. Token-request uncertainty and local keyring uncertainty have distinct fields. A known confirmed effect is retained even if an implementation contract is violated after activation. No token bytes are exported or automatically retried.

## Review path

| Concern | Source |
| --- | --- |
| Argument validation, explicit profile/actor binding, native composition and output preflight | `crates/craft-cli/src/provision.rs` |
| Composition, schema, budget, uncertain/confirmed result and trusted-profile controls | `crates/craft-cli/src/provision/tests.rs` |
| Skipped parser action, input-source tracking, complete-envelope dispatch and output evidence policy | `crates/craft-cli/src/args.rs`, `dispatch.rs`, `main.rs`, `output.rs` |
| List flag examples, alternative-required inputs and public non-exposure checks | `crates/craft-cli/src/contracts.rs`, `tests/local_interface.rs` |
| Fixed token exchange, scope reflection refusal, private credentials and typed failure fields | `crates/craft-runtime/src/auth/service.rs`, `auth/wire.rs` |
| Product/current-user proof and actor consistency | `crates/craft-runtime/src/auth/service/identity.rs` |
| Preflight-before-write, persisted cache, local-write state and stale-token refusal | `crates/craft-runtime/src/auth/service/cache.rs` and its tests |
| Secret pipe and interactive input | `crates/craft-runtime/src/auth/input.rs`, `prompt.rs`, `crates/craft-platform/src/console/` |
| Native generation/CAS/cleanup/logout and grant lock boundary | `crates/craft-runtime/src/secrets/`, `vault.rs` |
| Fixed-origin transport/TLS boundary | `crates/craft-runtime/src/transport/` |

Read the [secure-auth scenarios](../openspec/changes/build-cloud-cli/specs/secure-auth/spec.md), [design](../openspec/changes/build-cloud-cli/design.md), [review checklist](review.md), [service auth evidence](evidence/service-auth.md), [interactive evidence](evidence/interactive-input.md) and [composition evidence](evidence/provisioning-composition.md) as needed. Start from the changed flow rather than loading every historical research document.

Check especially that malformed local input, profile mismatch, bad target, missing replacement intent and inadequate recovery output fail before secret input or acquisition as applicable; issuer/token material never reaches diagnostics; target/actor proofs precede activation; output budgets cannot hide known writes; rate advice survives failure; and lock/cancellation/recovery ordering remains correct.

## Current activation status

The following sections retain the pre-activation review scope. Public provisioning/policy registration and its independent follow-up review are now complete. [Public activation evidence](evidence/public-activation.md) records actual executable qualification. Live service-account/site qualification remains outstanding; earlier references to skipped public roots describe the prior checkpoint.

## Activation boundary

Required before public use: independent review and fixes; explicit parser/catalog enablement and registered schema/flag/snapshot checks; native/public composition acceptance; and authorized live service credential/site qualification. [Disabled adapter and output evidence](evidence/provision-dispatch.md) now covers typed dispatch, input-source tracking, strict candidate schemas and complete-envelope encoding. The typed action remains skipped and public startup never calls the candidate descriptor builder. [Windows service-cache process acceptance](evidence/native-service-cache.md) covers the internal native runtime path with a synthetic issuer. Interactive and pipe input exist, but no user should paste credentials into chat or normal argument JSON.

The existing SDD rule requires independent review before enabling consequential auth. This package records author verification only. The user authorized any necessary review. An independent read-only reviewer completed this implementation review and found no concrete code blocker in its stated scope. Public registration changes and actual executable/native/live acceptance still require follow-up; see the disposition below.

Shared-profile aliases are a bounded observed metadata list rather than an atomic snapshot against other profile editors. The supported native coordination scope remains one principal, machine and state root. Linux/macOS native backends/input qualification, delegated broker, remote resource mutations and the rest of the CLI remain separate work.


## Permission setup adapters included in activation review

The new [policy setup adapters](policy-setup.md) make existing Confluence read/write and Jira create/read/mutation/recovery configuration available through typed get/set candidate dispatch. Review `crates/craft-cli/src/policies.rs`, `policies/records.rs`, `policies/tests.rs`, and the policy branches in `args.rs` and `dispatch.rs`. Check explicit-profile administration without repository hints, exact target/kind and operation validation, profile-owned grants, revision CAS, output preflight, and proposed-versus-observed state after an uncertain commit. The adapters perform no vault or provider access, and do not turn recovery permission into write/replay permission.

The candidate root remains skipped and its descriptors are absent from public startup. Existing runtime policy setters, schema eleven and credential formats are unchanged by this adapter. [Observed setup evidence](evidence/policy-setup.md) records actual checks and the remaining activation boundary. Public registration changes must be reviewed against the exact final source, then qualified through the actual executable; an author test pass does not grant independent approval or live eligibility.


## Independent implementation review disposition

Authorized reviewer: `/root/activation_review`. No source edits, test execution, network, tenant or keyring access occurred in the review. The reviewer inspected provisioning/policy adapters, dispatch/output, service exchange/identity/cache/access, secret pipe/prompt/Windows console, native grant generations/locking/vault, fixed-origin transport, Confluence read/publication and Jira-create policy/claim enforcement. No concrete activation code blocker was found. The reviewer independently compared all 261 captured crate/manifest files with `target/policy-setup-final-validation.json` and found zero mismatches.

This does not approve future public parser/catalog changes, establish live credential eligibility, qualify Linux/macOS native behavior, or complete broader CRUD/team/broker work. Those remain explicit follow-ups, with reviews already authorized by the user. The same reviewer also reviewed and rechecked the impact-based verifier correction; [gate evidence](evidence/impact-verification.md) records its findings and fixes.
