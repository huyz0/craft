# Local field projection

Observed on 9 November 2026, Windows x86_64 MSVC, Rust 1.99.1. This is partial acceptance evidence for projection/discovery, not completion of provider-side projection or pagination work.

`crates/craft-core/src/projection.rs` compiles declared dot paths against response schemas, follows local references and nullable objects, projects array elements, retains mandatory identity fields and preserves missing/null/unselected distinctions. It derives the selected response schema from the same selection tree, retains envelope/error/control fields and removes unselected required item properties. Contracts declare default/mandatory fields, item container and local/server/mixed mode. Unsupported dynamic/indexed/wildcard/ambiguous paths fail closed; duplicates and syntax/depth/count violations are rejected.

Local CLI reads accept global `--fields`. `schema --response --fields ...` returns the matching selected schema/example, digest, projection metadata and all supported selectable paths. Compact schema views remain explicitly incomplete. Input-schema selection with response fields is refused rather than ignored. Provider listing uses registered manifests, though no actual cloud modules are registered in the development binary.

Checks passed in the 33-test workspace/local verification run:

- Four pure projection tests cover nested sibling exclusion, array elements, real null/missing values, identity/control preservation, default body exclusion, parent selection, invalid/duplicate paths, selected schema validity and error variants/digest changes.
- Two additional subprocess tests cover actual nested `capabilities` selection against its generated schema, identity retention in command search, discoverability of nondefault selectable fields, and bad field/input-schema requests without secret echo.
- Direct invocations of `craft.exe capabilities --command version --fields availability.implemented` and the corresponding selected compact response schema returned complete valid JSON.

No provider HTTP field mask, upstream continuation retention, protected cursor store, redaction attribution, server bandwidth measurement or token-cost evaluation is established. The owning tasks remain unchecked until those dependencies and full acceptance hold.

## Closure

On 15 November 2026 the owning ledger task was closed. The remaining gaps listed above are answered in [foundation closure](foundation-closure.md), with platform runs in [platform qualification](platform-qualification.md) and review in [foundation review](foundation-review.md).
