# Document conversion and visible import acceptance

Date: **10 November 2026**, Australia/Sydney. This qualifies the supported document-library contract and completes the document-rendering task. It does not deliver the plan model, public provider commands or live editor acceptance. Verification is by the author; no approval authority or consequential auth path was enabled.

## Requirement audit

| Requirement | Implementation and observed evidence |
| --- | --- |
| AST-based Markdown to storage/ADF, escaping links/code/tables | `crates/craft-workflows/src/documents/mod.rs`; deterministic rendering and independent XML code/CDATA round trip in the renderer tests |
| Import visible human content | `documents/import.rs` imports checked XML and strict duplicate-checked ADF JSON; tests edit a rendered title/acceptance and observe those values through imported sections/fields |
| Parse structured visible sections | `documents/visible.rs` retains heading levels, rich section bodies, Field/Value rows and source origins; duplicate names, wrong headers, preamble loss and extra field-table blocks are refused |
| Preserve raw retrieval as an explicit format | `RawDocument` retains its bounded original format/string after semantic import fails; unsupported-macro raw retrieval is tested |
| Round-trip supported Unicode and multiline code | Storage and ADF round trips cover all supported node families, Unicode, code delimiters, escaped pipes and empty content |
| Identify unsupported content rather than discard it | XML macro/attribute/entity/declaration/merged-table failures have safe offsets; ADF node/mark/shape failures have safe paths; no regex HTML patching, macro expansion or hidden plan JSON restoration exists |

Domain task identities, graph cycles, Jira field mapping, approval binding and actual provider retrieval are owned by later tasks. A rich value is retained as a document; plain-text extraction refuses links instead of dropping their targets. The supported subset and format-specific bounds are documented in [content conversion](../document-content.md).

## Observed verification

`cargo test -p craft-workflows --locked --offline` passed **eleven tests**, with the external-schema case ignored. Output: `target/document-import-tests-output.log`. Six tests were added after the [renderer foundation](document-rendering.md). Focused all-target Clippy passed without suppressions.

`python tools/document-schema-acceptance.py` passed one separate opt-in test using the explicitly fetched schema. Atlassian's documented [schema link](https://developer.atlassian.com/cloud/jira/platform/apis/document/structure/) resolved to `@atlaskit/adf-schema` **57.7.39**, whose full schema is 73,655 bytes, SHA-256 `5128562b75278c8a83e7e3619a570205bc80d59696985ec31a7a7883cff66fbe`. Both tool and test require that digest. The fixture is retained only in `target/vendor-adf-schema.json`. Output: `target/document-vendor-schema-acceptance-output.log`.

For a fresh verification workspace, run `python tools/document-schema-acceptance.py --fetch` to fetch the pinned public version, then omit `--fetch` for offline reuse. The tool sets only the child test's non-secret fixture path and performs no tenant access. The normal development gate never downloads a schema.

The first vendor invocation failed because the new dev edge required lock regeneration; its log remains `target/document-vendor-schema-lock-failed.log`. The first actual schema check found block quotes nested inside list items were invalid ADF. The checked common AST now refuses that nesting, headings/tables/rules in those containers and nested quotes. Its failed log remains `target/document-vendor-schema-nesting-failed.log`. The final acceptance validates representative generated documents and asserts the unsupported nesting failures. This is not proof of every possible input or live product support.

The final native-access `python tools/verify.py` gate passed **206 named Rust tests, zero failures and five ignored opt-ins**, thirteen-operation snapshot comparison, ten contract controls, formatting, locked inventory/layer checks and all-target Clippy. Output: `target/document-import-verification-output.log`. The opt-ins are four native tests plus the vendor-schema fixture; only the latter was run separately here. Earlier native evidence remains separate. Quality/planning selftests passed nine/fifteen controls. Inventory remains 282 packages. `cargo build --workspace --locked --offline` also passed. The final planning check passed with twelve completed tasks; this is the supported library task, not full CLI completion.

Quick-xml moves from dev-only to the normal workflow dependency for XML import. Jsonschema remains a dev edge for external-schema acceptance. Lock comparison shows no removed/changed preexisting package version/source tuple; the package set is unchanged from the renderer addition. The refreshed dependency inventory records the actual manifest/edge changes.

SHA-256 checkpoints:

| Artifact | SHA-256 |
| --- | --- |
| `Cargo.lock` | `8c724a1017c1c2462cfb75a312dd7b396729d43e212aa4fc99d344f1efc209ca` |
| `documents/mod.rs` | `bbd8b60f2b3cb4db852496c46f1f5a22cdfcc50f312cd89f4b3d25c6c53d0ce0` |
| `documents/import.rs` | `2c466504633fa7e073a65b50e171ee0f57f5fb6642597559bc01e3272f899983` |
| `documents/visible.rs` | `2cbb3b787e65f90d78d7b5f1cd6ec8e0aea976e9dc87f535a4eafd1dd42ef77e` |

Document paths in this table are under `crates/craft-workflows/src/`.

## Limits

The library is not yet linked into the public CLI, which still advertises thirteen local operations. Full arbitrary Confluence editing, Cloud-specific normalization/macros/attributes, live human-account edits and provider field support are not qualified. The test's human edit is a local content fixture, not a live Confluence edit. Only the Windows toolchain was exercised. Converter output is not approval, and raw inspection is not edit authority. No real credential, native credential write, tenant mutation, commit, push, publication or deployment occurred. The full CLI goal remains active.
