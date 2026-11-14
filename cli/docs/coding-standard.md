# Craft coding standard

This standard applies to Rust, supporting tools, tests, fixtures, configuration and generated implementation artifacts in `cli/`. It is part of the [SDD workflow](../SDD.md), not an optional style note.

## Semantic names and planning metadata

Use domain/behavior names for crates, modules, functions, types, tests, fixture files, error codes and configuration. Do not include milestone or task IDs in code, code comments/doc comments, test names/content, fixture names/content, build/runtime configuration, logs, CLI output, public schemas, package names or commit subjects. Do not substitute a different tracking prefix to evade this rule.

Keep delivery traceability in OpenSpec tasks and evidence records. Those records link to semantic source/test paths and actual check results. Runtime request IDs, operation IDs, schema versions and provider object IDs remain legitimate domain data; they are not planning labels.

## Comments explain behavior or design

Write comments when they clarify the code's behavior, contract, design intent, invariant, failure handling or non-obvious tradeoff. Explain why a constraint exists or what a caller must know. Keep the explanation accurate when implementation changes.

- Document public APIs' inputs, outputs, side effects, preconditions and meaningful failure behavior.
- Explain credential-generation ordering, uncertain remote writes, cursor remainder ownership, trust boundaries and other invariants where readers could otherwise break them.
- Reference a semantic design section when useful, with a short explanation in the comment itself. Avoid a bare planning link or a copied design document.
- Do not use comments as progress reports, session handoffs, task-status notes or planning labels.
- Do not add a comment that merely translates the next obvious statement into English. A comment is not required on every function or line.
- Do not put secrets, tenant credentials or private customer data in examples or doc comments.

Illustrative comments that carry design information:

```rust
// Switch the active credential pointer only after every new chunk is durable.
// Keep the unreturned items before advancing the provider cursor to avoid gaps.
// A timeout after send leaves the remote effect unknown; reconcile before replay.
```

These examples describe intended invariants; they are not evidence that Craft implements them yet.

## Rust implementation rules

Keep the binary thin and use the dependency direction in the design. Core types do not read configuration, clocks, keyrings, files or HTTP directly. Inject those capabilities through narrow ports. Test helpers are dev dependencies only.

Model target identities, effect classes, credential modes, preconditions and operation outcomes explicitly. Use typed errors at module boundaries and sanitize them when constructing CLI output. Avoid string matching for provider/auth decisions and avoid `unwrap`/`expect` on untrusted input or recoverable I/O.

Accept bounded inputs, perform local validation before network/auth effects and encode raw route components at the transport boundary. Keep secrets out of debug derives, generic serialization, logs and subprocess arguments. Do not silently coerce input, force-overwrite human edits, broaden credentials or retry uncertain writes.

Use `cargo fmt`, compiler checks, Clippy and behavior-focused tests once the Rust workspace exists. Explain any targeted lint exception locally by the actual design reason; a delivery deadline is not a correctness reason. Do not add blanket lint suppressions or unsafe code as a shortcut. A genuinely necessary unsafe boundary requires its safety contract and focused verification before use.

## Verification

The planning checker detects conventional artificial tracking-label patterns in implementation text. Review must additionally establish semantic naming and useful comments; no regex establishes comment accuracy. For changed behavior, inspect affected comments/doc comments and update or remove stale explanations. Record source/test/design links and observed results in planning evidence, keeping the product free of delivery metadata.
