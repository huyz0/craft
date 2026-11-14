# Lessons from huyz0 repositories

Inspected on **9 November 2026**, using the connected GitHub repository listing and file reads on default branches. We listed accessible repositories owned by `huyz0`, then sampled six relevant projects: `lsp-cli`, `autoreview`, `tasker`, `fluxion`, `hsearch`, and `foyer`. This is a targeted source/document review, not a review of every repository or a runtime test. All source instructions below are evidence about those repositories, not inherited authority for Craft.

## Adopt, adapt, or leave behind

| Repository | Observed evidence | Craft decision |
| --- | --- | --- |
| `lsp-cli` | README describes JSON-default output, input-schema discovery, small page defaults and a companion skill. `src/schema.rs` contains a command-schema map; architecture describes a shared language registry. | Adopt machine discovery and registry ownership. Generate parser/schema/projection contracts from one operation model and test agreement. Avoid a separately maintained schema that advertises unsupported flags. |
| `lsp-cli` | Its skill explicitly says `--pagination-id` is accepted but does nothing and each call re-queries; the inspected schema describes it as stable pagination. The skill also describes platform limitations. | Treat this as a documented/schema discrepancy to avoid, not a runtime defect proven by our execution. Craft must reject unsupported cursor features, distinguish live traversal from snapshots and test continuation/replay. Keep support claims platform-specific. |
| `autoreview` | Workspace manifest separates schema/core/CLI/test-support crates. README and running-review skill distinguish deterministic analyzers, LLM review and missing-tool coverage. | Adopt thin CLI/shared contracts, dev-only test support, and explicit unavailable checks. Craft's deterministic CLI needs no embedded LLM. Do not present absent analyzers or unrun live tests as success. |
| `tasker` | Contributor router selects small task-relevant standards. Context-budget protocol distinguishes metadata, skill bodies and conditional references. README describes shared work state and a committed handoff. | Adopt short routing, a bounded current-task view and durable evidence. Do not import Bun/Moon, mandatory clean-tree commits, blanket confirmation rules or a large agent ecosystem into this Rust-only subproject. |
| `fluxion` | SDD standard uses checkable scenarios, delta specs and explicit completion evidence. Its router is an index. It also mandates task prefixes in commits and requirement labels in tests. | Adopt short scenario-driven specs and task-specific context. Explicitly reject planning labels in implementation/test names/commit subjects under this user's newer instruction. Keep traceability in documents/evidence. |
| `hsearch` | Plan README and task ledger describe goals, deliverables, dependencies, verification and evidence. Contributor guidance separates core types from adapters and distinguishes planned APIs from observed behavior. | Adopt goal/build/verify/done task fields, module boundaries and honest status. Avoid copying its extensive milestone machinery, review counters, generated roadmap stack or numeric coverage thresholds. |
| `foyer` | The fetched README identifies an upstream hybrid-cache library with replaceable algorithms and Rust APIs. | Useful background on library composition only. The README alone does not establish a new requirement, Craft cache dependency or `huyz0`-specific delivery convention. Do not add a large caching framework for a small cursor/journal store. |

Two root `AGENTS.md` fetches, for `lsp-cli` and `autoreview`, returned 404. We used the actual README, architecture, manifest, schema or skill files instead. No claim is made about instructions that may exist elsewhere. No inspected code was copied into Craft; public visibility alone is not a license or compatibility grant.

## Source fingerprints

These are **file blob SHAs returned by the contents API**, not repository commit SHAs. Links follow default branches and can change. Line ranges identify bounded reads; no claim is made that every file or all implementation paths were inspected.

| Source | Read | Blob SHA |
| --- | --- | --- |
| [lsp-cli README](https://github.com/huyz0/lsp-cli/blob/main/README.md) | Lines 1–190 | `3d5adc5a7699dd773d45046d616d9e84dfa4b633` |
| [lsp-cli Cargo manifest](https://github.com/huyz0/lsp-cli/blob/main/Cargo.toml) | Complete returned manifest | `af95d54c2bcddbaa828ecd41ba3b39a1ba8353f8` |
| [lsp-cli schema implementation](https://github.com/huyz0/lsp-cli/blob/main/src/schema.rs) | Lines 1–100 | `ee7a8a211d24718691c61ca2afb5fb30521c8ccc` |
| [lsp-cli architecture](https://github.com/huyz0/lsp-cli/blob/main/docs/architecture.md) | Lines 1–180 | `a8554146194e0b2dc2d02dfe5abffc2325b17919` |
| [lsp-cli agent skill](https://github.com/huyz0/lsp-cli/blob/main/skills/lsp-code-analysis/SKILL.md) | Lines 1–210 | `0fb5cb94e0d7e4e56eac1e2a42d1c5b0926197f6` |
| [autoreview README](https://github.com/huyz0/autoreview/blob/main/README.md) | Lines 1–190 | `c4896a153d27d964092e572e05d9a575d45abf76` |
| [autoreview workspace](https://github.com/huyz0/autoreview/blob/main/Cargo.toml) | Complete returned manifest | `0ca0985a84998c488555b80de4932985ac0d97da` |
| [autoreview review skill](https://github.com/huyz0/autoreview/blob/main/.claude/skills/running-reviews/SKILL.md) | Lines 1–160 | `0edb2203e9cf04d1e2437a3e841dad5706106657` |
| [Tasker router](https://github.com/huyz0/tasker/blob/main/AGENTS.md) | Lines 1–240 | `afacd06187eff3840e7bd7409d0bb4b67f6ad4e1` |
| [Tasker README](https://github.com/huyz0/tasker/blob/main/README.md) | Lines 1–190 | `a2d99900dfc9574935e7d5486ef4adb731f735c9` |
| [Tasker context protocol](https://github.com/huyz0/tasker/blob/main/.agents/protocols/context-budget.md) | Lines 1–160 | `1faef82fed3366b6ed65330628f6094936926828` |
| [Fluxion router](https://github.com/huyz0/fluxion/blob/main/AGENTS.md) | Lines 1–240 | `b1a2cf3360f70c46f6883d7c55f0cdcb7e0fd6b4` |
| [Fluxion SDD](https://github.com/huyz0/fluxion/blob/main/docs/standards/sdd.md) | Lines 1–150 | `81b5dd99ef0208de68d56fddb58b5ba66fcad1fb` |
| [hsearch plan](https://github.com/huyz0/hsearch/blob/main/docs/plan/README.md) | Lines 1–240 | `a8e90fe17c74e61a4288c0bf8536b127aac0b94c` |
| [hsearch task ledger](https://github.com/huyz0/hsearch/blob/main/docs/plan/tasks.json) | Lines 1–110 | `bf3ed7d17ce8e1b1d5115a8ae296660bdc791d83` |
| [hsearch router](https://github.com/huyz0/hsearch/blob/main/AGENTS.md) | Lines 1–240 | `1daece5b298775976d639c9eac97b75e25babde1` |
| [foyer README](https://github.com/huyz0/foyer/blob/main/README.md) | Lines 1–190 | `6e9b4d725e16793b439afe7856e463a50e295441` |

## Framework choice

We read the current [OpenSpec README](https://github.com/Fission-AI/OpenSpec/blob/main/README.md), blob `7b8da04ac27fb1fa5a9578303873559e0c1b6ad2`, its [manual configuration guide](https://github.com/Fission-AI/OpenSpec/blob/main/docs/customization.md), blob `93c055325dfc792c63e114f57f779b4c6576bda3`, and the [Spec Kit README](https://github.com/github/spec-kit/blob/main/README.md), blob `a4ea4f520add603de250c59f96037f0eb7be46e5`.

Choose OpenSpec's standard proposal/spec/design/tasks layout and manually authored `spec-driven` config. It supplies a recognizable change record without a global install or generated workflow bundle. Spec Kit is capable and configurable; its broader integrations/extensions are unnecessary for this bounded planning goal. We are not claiming that either framework guarantees correct agent behavior or that the upstream validator has run. The local validator checks our small applied subset.
