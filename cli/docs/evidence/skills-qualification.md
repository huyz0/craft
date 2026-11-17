# Skills qualification

Date: 2026-11-17. Tool: `python tools/skills_qualify.py` (tests: `tools/skills_qualify_test.py`).

## What a run checks

Offline, for every skill under `skills/` (`craft-cloud`, `confluence-pages`, `atlassian-design`):

- two-field frontmatter whose name equals the directory, description under 1 KiB, `SKILL.md` under 16 KiB (it is loaded whole; detail belongs in references);
- every relative link resolves to a file inside `skills/`, and every `#anchor` matches a heading;
- every `craft ...` line in a code block parses against the contract (command, flags, choices, required inputs, scalar ranges), and every backticked `craft <command path>` or dotted command name exists in `schemas/local-contracts.json`;
- the `craft-cloud` compatibility pin (contract hash, binary version, providers, unqualified hosts) through `tools/runtime_skill.py`.

With `--binary <craft>` it also runs the built executable: `version` equals `Cargo.toml`, `schema --command` resolves each of the 48 commands the skills cite, and the whiteboard scene example from the scene reference passes `whiteboard check`, `encode` and `decode`. No network is used.

## Evidence freshness

A/B and live runs cannot be repeated by a script, so `docs/evidence/skills-qualification.json` records the content hash of each skill as last qualified by that evidence. `check` lists skills whose content changed under `changed_since_evidence`, and `--strict` fails on any. After a skill edit, rerun the matching evidence, update the evidence document, then run `baseline`.

| Skill | Behavioral evidence | Method to repeat |
| --- | --- | --- |
| `craft-cloud` (whiteboard references) | [whiteboard scenes](whiteboard-scenes.md): live paste and copy-back, A/B of layout recipes | Encode a scene, paste it into a board with a browser tool, copy back and decode |
| `confluence-pages` | [page skill](confluence-pages-skill.md): live rendering of every snippet, two-round blind A/B (99 vs 81 on the second round) | Workflow of agents per task and variant, screenshots at 1280 px, blind reviewer with the 8-criterion rubric |
| `atlassian-design` | Applied through both skills above; no separate run | Re-run either A/B after changing color rules |

## First run

All three skills pass static checks; 48 cited commands resolve in the release build of 0.1.0; the whiteboard example round-trips. The hashes recorded in the baseline are the versions merged in pull requests 6 and 7, which carry the evidence above. Host loading in Codex and Claude Code stays unqualified. Live provider behavior is covered by the live evidence documents, not by this tool.
