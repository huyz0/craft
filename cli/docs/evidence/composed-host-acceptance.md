# Composed host acceptance evidence

Date: **10 November 2026**, Australia/Sydney. Actual internal CLI/runtime library composition on Windows with owned TLS servers, isolated memory credentials, native grant locks and SQLite. No live provider, real credential, public cloud command or native vault was used in this addition.

## Implemented boundary

The [contract](../composed-host-acceptance.md), [design](../../openspec/changes/build-cloud-cli/design.md#native-providers-and-factory-workflow) and [scenario](../../openspec/changes/build-cloud-cli/specs/delivery-quality/spec.md#requirement-synthetic-composed-acceptance) define the explicit debug-only fixture feature. The actual host is unchanged; the test invokes `PreparedCreate::execute_cached` from `craft-cli` against normal linked runtime code. Only the TLS test server source is reused through a path module.

The runtime fixture uses compiled synthetic credentials and bearer, localhost ports, marker-preserving tokens/sessions, exact client/token provenance and pre-probe cache-material checks. Real/ordinary tokens cannot obtain fixture routing. Default builds omit fixture code, no CLI/profile/environment selector exists, and compilation without debug assertions refuses the feature. The bounded memory vault is an explicit isolated fixture, not a production backend or fallback. Existing generation/cache and native grant-lock implementations supply credential lifecycle behavior.

## Behavior observed

Six integration tests exercise multiple cases each:

- Fresh and renewed cached sessions send seven qualified GETs and exactly one issue POST, with exact project/type/summary/ADF/parent/account fields. Renewed access retains a confirmed cache update under selected output. Stored operation rows retain the created issue across completion and refuse another attempt before authentication.
- A failed secondary transition retains the confirmed created ID and partial result. A disconnect after the observed POST retains unknown journal state and does not retry. Repeating unknown or terminal work sends no identity, resource or token request.
- Missing write, metadata or account scopes and a different trusted actor stop after identity verification, before any metadata/resource read or claim. The missing-account case uses the granular identity-probe alternative to distinguish it from the classic account-read scope.
- Wrong project, cross-project parent and a different returned account stop qualification with no claim. A token renewed just before these failures still returns its confirmed cache receipt separately from the unsent issue.
- A clock observing the actual committed send marker then expires metadata, expires the token or cancels the invocation. Each case retains unknown journal state with no POST or reacquisition; replay is refused before auth.
- A request-sensitive deadline expires only after the stalled project GET is observed, returning a not-sent issue receipt and no claim/retry. Foreign direct credentials, cached credentials, cached bearer and issuer bearer are refused before their inappropriate next network step.

Successful cases verify request counts, method/path/auth headers and request bodies, inspect actual journal rows and cache versions, scan non-secret fixture files for synthetic credential/token/private-account sentinels, and assert the owned roots are removed after handles close. The fixture vault stores `SecretBytes`, which zeroize on drop; this does not claim erasure of every parser/test allocation. Native vault behavior remains separately qualified.

Two feature-only runtime tests additionally refuse unmarked tokens in a fixture client, fixture tokens in an ordinary/different fixture client, nonfixture bearers receiving a routing marker and zero ports. Existing runtime tests also run with the feature enabled to check ordinary unmarked behavior.

## Commands and results

The initial focused five-test run had two fixture failures: scanning a still-held lock file caused Windows error 33, and a short deadline expired before the intended stalled request arrived. Explicit lock/fixture drops precede scanning; the deadline now depends on the observed request. The corrected five-test run passed in `target/create-host-composition-corrected-output.log`. A sixth qualification-failure case and explicit successful cleanup checks were added before the integrated gate.

`python tools/verify.py` passed in `target/create-host-composition-integrated-output.log`:

| Check | Observed result |
| --- | --- |
| Default workspace tests | 295 passed, zero failed, five ignored opt-ins |
| Explicit composed-host target | Six passed, zero failed |
| Feature-enabled runtime regressions | 116 passed, zero failed, three ignored opt-ins |
| Default and feature all-target Clippy | Passed |
| Public contract and controls | Twenty unchanged operations; ten drift controls |
| Provider inventory | 99 commands, 112 variants/routes and 24 controls passed |
| Dependency/layer, planning and formatting | Passed |
| Fixture isolation checker | Explicit feature, normal-tree separation and non-debug refusal passed |

The feature runtime group repeats 114 default tests and adds two controls. There are **303 distinct passing Rust tests**, or **417 passing executions** across the gate's groups; ignored totals repeat three runtime opt-ins. No ignored native-vault/console/vendor-schema test was rerun here. The expected compiler refusal is in `target/synthetic-acceptance-nondebug-output.log`, with an earlier direct refusal in `target/create-host-composition-nondebug-refusal-output.log`.

`cargo build --workspace --locked --offline --no-default-features` passed in `target/create-host-composition-default-build-output.log`, restoring a default-feature development binary after feature testing. Its SHA-256 is `a7983aadfa3fd89e491aa535417ae738b6c1c54cdf6aceb74655da7281bfc996`. Lock comparison found only the CLI package record's new dev dependency edges to already pinned rustls/rcgen. No package/version/source/checksum changed; inventory remains 283 packages. The normal CLI tree excludes rcgen and test support. Public schema is unchanged.

Source/manifest/lock/inventory/schema/default-binary hashes and validation are retained in `target/create-host-composition-checkpoint-hashes.json` and `target/create-host-composition-checkpoint-validation.json`. Local documentation links/fences and planning status are checked after recording evidence.

## Limits and next work

This closes the actual host-library composition gap from the [preceding checkpoint](jira-create-host.md). It does not establish a public CLI create command, live eligibility/scopes/permissions, native vault behavior in this composed test, other operating systems, provider reconciliation, complete CRUD or release/distribution readiness. Independent consequential review remains outstanding before public auth/write enablement. Keep the whole CLI delivery goal active.
