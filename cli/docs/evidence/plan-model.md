# Local plan and design model acceptance

Date: **10 November 2026**, Australia/Sydney. This record covers local plan/design models, visible page conversion and six public authoring commands. It does not qualify page publication, eligible human review or Jira ticket materialization. Verification is by the author. No consequential auth, deletion, approval or unknown-write path was enabled, and no independent reviewer was started.

## Requirement audit

| Requirement | Implementation and observed evidence |
| --- | --- |
| Complete versioned plan/design/task models | `crates/craft-workflows/src/plans/mod.rs`: explicit destination, semantic identity, context, rich bodies, acceptance, repositories, dependencies, inclusion, estimates and per-task Jira mapping; design references pin cloud/page/version/digest |
| Strict local validation | Duplicate-checked JSON and closed typed models reject unknown fields; semantic validation bounds graph/body data and rejects duplicate identities, missing references, cycles and included tasks depending on excluded work |
| Visible plans with authoritative human edits | `plans/page.rs` and `documents/compose.rs` render/import metadata, task sections and rich bodies without hidden JSON; tests observe changed title, criteria, estimate and exclusion values |
| Storage and ADF round trips | Plan fixtures preserve canonical content/digests through both formats; design fixtures cover compact/full templates and authored section import through both formats |
| Refuse ambiguous or unsupported content | Document subset tests cover unsafe/unsupported structures; model tests reject unknown fields/sections and reserved headings rather than silently dropping visible content |
| Public scaffold/validate/render | `crates/craft-cli/src/planning.rs`, typed args, dispatch and catalog expose six read-effect operations without profile/auth/network access; subprocess tests cover extraction, projection, invalid input and stdin conflicts |
| Truthful scaffold input contract | Flat required targets are available through flags and JSON; subprocess tests compare their returned documents, check required request-schema fields, reject missing/unknown input and identify malformed cloud IDs by the supplied flat field path |
| No inferred review authority | Projected responses retain false review/live flags and zero tickets even for an `approved` display label; scaffolds retain `ready: false` |
| Accurate schemas and bounded discovery | `craft-core/src/projection.rs` treats heterogeneous values as whole selectable leaves, preserving complete union schemas while refusing variant-dependent child paths; nullable-object traversal remains covered |
| Semantic runtime identities | Examples use caller-supplied work names; planning-label scans and source/comment review keep development labels out of implementation artifacts |

The owning task's local model/command acceptance is separate from the broader published-planning requirement. Publication and reviewed materialization remain dependent work. The [usage document](../planning.md) records actual arguments, document shapes, visible protocol, bounds and limitations.

## Observed verification

The scaffold correction passed five subprocess tests in `target/planning-scaffold-tests-output.log`. The reviewed contract snapshot was regenerated intentionally with `python tools/contracts.py write`; `target/planning-contracts-output.log` records nineteen operations. No public cloud provider or provisioning descriptor was added.

The first complete restricted run failed the preexisting auth diagnostic test with `KEYRING_UNAVAILABLE`; the planning tests had passed. Its output remains `target/planning-scaffold-verification-output.log`. The subsequent native-access `python tools/verify.py` run passed **217 named Rust tests, zero failures and five ignored opt-ins**, snapshot comparison, ten contract controls, formatting, inventory/layer checks and all-target Clippy. Output remains `target/planning-scaffold-native-verification-output.log`. This native-access gate permits Windows keyring reads and loopback fixtures; it does not execute the ignored native acceptance cases or live OAuth.

`python tools/document-schema-acceptance.py` separately passed the ignored vendor-schema case, including complete plan/design pages. It used the existing checksum-pinned Atlassian ADF schema **57.7.39**, 73,655 bytes, SHA-256 `5128562b75278c8a83e7e3619a570205bc80d59696985ec31a7a7883cff66fbe`. Final output: `target/planning-model-vendor-schema-output.log`. No schema download or tenant request was made in this run.

`cargo build --workspace --locked --offline` passed in `target/planning-final-build-output.log`. Fifteen documented command calls ran successfully against the final binary, with all three JSON examples parsed and the structured Markdown example also validated as plan context. Fixture extraction supplied raw storage and an authored design for dependent examples; all planning responses retained their authority flags. Reproduce with the retained `python target/planning-usage-check.py`; output is `target/planning-final-usage-output.log`. This is a source-informed command check, not a fresh-human usability test. Local links and fences passed for seven changed documentation files.

An earlier exporter attempt found heterogeneous rich-text projection was unsupported; its failure remains `target/plan-contract-projection-failed.log`. Earlier Clippy found needless format borrows, which were fixed without suppression. Those failures predate the passing logs above. Cargo reports that incremental-cache hard links are unavailable and copies files instead; builds/checks completed despite that host warning.

The final native-access `python tools/verify.py` run after error-path translation and the expanded design ADF round-trip assertion passed the same **217 tests, zero failures and five ignored opt-ins**, all-target Clippy and the other gate checks. Output: `target/planning-final-native-verification-output.log`. Public provisioning remains skipped/unregistered. The four ignored native opt-ins were not rerun; earlier native evidence remains separate. The local-model task can close against this evidence without closing the publication/review/provider tasks.

SHA-256 checkpoints, with the full eleven-file set retained in `target/planning-final-checkpoint-hashes.json`:

| Artifact | SHA-256 |
| --- | --- |
| `Cargo.lock` | `a437de3d34dfb3a5b050f789d9275b56677e9988e4f7024ee59e38e6d5d3792d` |
| `schemas/local-contracts.json` | `d0e1ca9ce490227d60715bb8ba885b81bf6d122ea32f7bac2a13620a84298d8b` |
| `crates/craft-cli/src/planning.rs` | `63c5933d0c0bc74e057545f0d44f150f33b06e5cf304caf965608677043e9a7a` |
| `crates/craft-workflows/src/plans/mod.rs` | `98734a202ddc40008dbf86a2b406ab85cddc1b8af12342a89390ea15a11072d6` |
| `target/debug/craft.exe` | `930e9298b60eb12a7a74ae6be0ab0ccdcf44d4742c4fce5e6ef2a9c941aba207` |

## Dependency and scope limits

The CLI now links the workflow library. Existing pinned serde/schemars become direct workflow dependencies; no package version/source changes were required. The inventory still contains 282 packages. Core remains independent of runtime/providers/workflows; workflow code uses core contracts without a runtime dependency.

Only the Windows development toolchain was exercised. All human edits here are local fixtures. Arbitrary Confluence content, live editor normalization, current target permissions, remote design revision checks, native credential lifecycle, cross-machine coordination, agent evaluations and distribution remain unqualified. Input/output limits are real and can reject a larger rendered representation even when the original authoring form fit. Digests do not prove reviewer eligibility, immutability or approval. No credential, tenant resource, commit, release or deployment was created for this work. The full CLI goal remains active.
