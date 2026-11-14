# Runtime skill preparation

Observed 11 November 2026 on Windows x86_64 with Python 3.13. This prepares the repository skill and improves verification selection; it does not qualify a distribution or either agent host.

`skills/craft-cloud/SKILL.md` routes to four conditional references for listing, writes, shared plan review and recovery. It uses installed discovery instead of copying provider schemas. `compatibility.json` identifies development version 0.1.0, contract 1.0 and the reviewed snapshot SHA-256. Both Codex and Claude Code remain explicitly unqualified. The development binary and Rust implementations were unchanged.

`tools/runtime_skill.py` checks the deliberately small frontmatter format, local links, cited command IDs, example paths/flags/required scalar values/constraints/projections and compatibility. It is not a general JSON Schema validator and does not read operator-provided input files from examples. Its fourteen negative cases cover unknown commands/flags or choices, missing required values/IDs, ID patterns, numeric values/bounds, action/input conflicts, invalid projections, escaping/broken references and stale compatibility.

`tools/verify.py` now tracks skill files. `tools/verification_scope.py` selects offline skill checks for skill-only changes and also checks compatibility after contract changes. The focused plan selects no Rust packages or composed provider hosts. Full release verification retains the skill checks along with the existing matrix. Token measurements are optional and do not become a network-dependent development gate.

Observed checks:

- `python tools/runtime_skill.py check`: passed, five Markdown files and fourteen command IDs.
- `python tools/runtime_skill.py selftest`: passed, fourteen negative cases after fixing the review finding.
- Official `skill-creator/scripts/quick_validate.py skills/craft-cloud`: passed using workspace-only PyYAML 6.0.3.
- `python tools/verify.py`: four focused checks passed before review corrections; final focused verification recorded below after corrections.
- `runtime_skill.measure()` with workspace-only tiktoken 0.12.0: both cl100k_base and o200k_base encoded and roundtripped every Markdown file. Entrypoint including frontmatter: 442 and 441 tokens respectively. Reference files range from 227 to 337 tokens. Exact measurements and source hashes are in [runtime-skill-metrics.json](runtime-skill-metrics.json). These encodings do not establish Claude token counts or completed-task efficiency gains.

The first restricted PyPI download failed DNS resolution. The pinned validator/tokenizer packages were subsequently downloaded with authorized network access only into `target/skill-validation`; vocabulary caches live under `target/tokenizer-cache`. No global Python package, PATH, host skill directory, credential or tenant was changed. The normal checker uses only the Python standard library and reviewed local contracts.

Independent reviewer `/root/skill_review` used offline installed help/schema/input validation for a known page-title read, cursor continuation and an unknown ticket send. It found that initial example checks accepted missing or malformed scalar inputs. The checker now rejects those cases. The forward response also incorrectly retained `--profile` on local `operation.get`; the recovery reference now explicitly says to retain the state root and omit that rejected control. Review follow-up is recorded with final validation.

Follow-up independently confirmed all six added malformed examples are refused, local inspection rejects `--profile` and accepts `--state-root`, and the amended recovery guidance resolves the forward-use ambiguity. No additional actionable defect was found in that bounded review. Final qualification initially stopped because the ledger evidence field was authored as a prose link rather than its required repository-relative path; the field was corrected without changing the checker or implementation.

Final `python tools/verify.py`: passed three impact-selected checks (planning, runtime skill check, runtime skill selftest), zero reused commands, zero Rust packages/hosts selected and no source drift. Verifier-selection/checkpoint selftests passed in the earlier four-check run; those implementation files were unchanged afterward. No fresh full qualification is claimed.

Unrun: actual Codex/Claude Code skill discovery, matched cold/warm task evaluations, real page/cursor responses, live permissions, unknown-send reconciliation, installation/upgrade/uninstall and platform distribution. The broader runtime-skill task remains unchecked because required workflow dependencies remain partial. No project license was selected and no package was published.
