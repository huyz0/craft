# Document renderer foundation evidence

Date: **10 November 2026**, Australia/Sydney. Partial implementation of document rendering/import. The owning task remains unchecked. No new command/provider or cloud publication is enabled.

Later [document import acceptance](document-import.md) completes the supported library task, including visible storage/ADF import, explicit raw retention and structured section/field extraction. The partial status, dependency roles and remaining gaps below identify the original renderer checkpoint rather than current completion.

## Implemented scope

The new `crates/craft-workflows` member contains `src/documents/{mod.rs,tests.rs}`: a privately constructed bounded Markdown AST, source-offset errors, deterministic Jira ADF and Confluence storage output. [Content contract](../document-content.md), reviewed-plans scenarios and design describe supported/rejected syntax and limits. Visible provider import, plan extraction and public integration remain open.

The renderers escape text/attributes, split embedded CDATA terminators, preserve Unicode/code text, normalize tight list items for ADF and collapse duplicate marks. Recognized table rows are checked against the header width before GFM can pad or discard cells. Unsupported syntax and unsafe links fail without source echo.

## Observed checks

`cargo test -p craft-workflows --offline` passed five tests, zero failures. Output: `target/document-rendering-tests-output.log`. Tests check narrative/mark/link/list/break output, Unicode and multiline code through an XML CDATA round trip, table headings/escaped pipes and missing/excess-cell refusal, unsupported constructs with safe source-offset errors, and input/depth/event bounds. Exact repeat rendering is stable. XML checks use the independent quick-xml parser; ADF checks compare selected documented node shapes, not a full vendor schema.

`cargo clippy -p craft-workflows --all-targets --locked --offline -- -D warnings` passed. The subsequent native-access `python tools/verify.py` gate passed **200 named Rust tests, zero failures and four ignored native opt-ins**, thirteen-operation contract comparison, ten contract controls, formatting, locked inventory/layer checks and all-target Clippy. Output: `target/document-rendering-verification-output.log`. Native opt-ins were not rerun; their earlier evidence remains separate. Quality/planning selftests passed nine/fifteen controls. `cargo build --workspace --locked --offline` passed. Compiler incremental hard-link warnings used copying without failing the gate.

The first offline resolution failed because `unicase` was not cached. Native network access fetched the pinned parser/dependency. A before/after lock tuple comparison found no changed or removed existing package versions/sources; only `craft-workflows` and three external packages were added. `python tools/dependencies.py --write` recorded 282 packages. [Dependency baseline](../dependencies.md) records inspected licenses, features, build behavior and limits. The upstream parser defaults/HTML emitter/CLI are disabled; quick-xml is dev-only. No production XML import dependency is claimed yet.

SHA-256 checkpoints:

| Artifact | SHA-256 |
| --- | --- |
| `Cargo.lock` | `948c044840bcab8f6554317ea8260c3c6ac1aef6f1bc96593b2ea0f8292dc725` |
| `crates/craft-workflows/src/documents/mod.rs` | `aa6df0ebecf0b8b102d6079f2ea3102aa25e374befea03146a7507e3f597f588` |
| `crates/craft-workflows/src/documents/tests.rs` | `b6af66d052cc6b108e4a6a81e0fccba6564f36707eba8a85d699855be90220de` |

The public executable remains `target/debug/craft.exe`, SHA-256 `b9348b297e6727acae0a4c27518ecbcb02ccba16f661cad70df6402272ef6897`. The workflow crate is not yet linked into that binary.

## Remaining acceptance

Storage and ADF import, precise unsupported-macro errors, visible structured plan sections, raw retrieval, full vendor-schema validation, Cloud/editor normalization and actual provider/CLI composition remain unrun or unimplemented. Rendering is not approval and does not supersede human edits. Only the Windows toolchain was exercised. Parser/source inspection is not an exhaustive security audit. No credentials, tenant resources, commit, push, publication or deployment changed.
