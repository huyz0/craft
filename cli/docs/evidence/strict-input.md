# Strict structured command input

Observed on 9 November 2026 on Windows x86_64 MSVC with Rust 1.99.1. Implemented strict argument-source handling in `craft-cli`, path-aware JSON parsing in core, typed registry validation and context-bound field metadata contracts.

`--input FILE` or `--input -` supplies the selected command's entire registered argument object. Explicit action flags conflict before reading that source; parser defaults never override it. Fields, byte budget and timeout are separate invocation controls. Schema field selection present in both JSON and flags fails explicitly. Stdin has one owner: it cannot simultaneously supply an invocation argument object and a nested validation payload. Both flag and JSON forms dispatch through the same closed schemas and Rust types.

The bounded JSON visitor refuses invalid UTF-8, duplicate properties at any depth, non-object roots, trailing JSON and byte/depth/container overflows. It preserves valid international/multiline strings. Errors report escaped JSON pointers without raw values or raw parser error strings; paths longer than the diagnostic limit carry `path_truncated: true`. Typed registrations apply strict deserialization to `input validate` as well as invocation/preparation. This prevents a schema-valid mathematical integer such as `1.0` from passing validation and then failing Rust's integer decoding. The full/compact schema views disclose the `serde-strict` decoder policy and its digest changes with that annotation.

`FieldMapContract` compiles individually bundled schemas from metadata for an exact resource context. Unknown fields, wrong types/ranges and a different tenant/project/type context fail without coercion. External schema references are refused. These are local validation primitives; obtaining fresh live metadata and rechecking permissions belong to provider preflight.

Observed checks:

- Core parser tests verify exact escaped nested paths, truncated-path signaling, duplicate/UTF-8/trailing/depth/size/container rejection, canonical preservation and Unicode/multiline content.
- CLI subprocess tests verify flag/stdin parity, both input sources' size bounds, required arguments, unknown/duplicate/type failures, numeric decoding parity, source conflicts before file access, path-root rejection, and nested stdin ownership. No sentinel value appears in returned errors.
- A real loopback fixture exercises parse, preparation and execution: malformed requests produce zero HTTP calls; a valid international/multiline record then makes exactly one call and arrives unchanged. Existing provider tests reject malformed identities/targets before host access.
- The full workspace gate passed 55 named tests and five subprocess controls before invocation-control additions. Subsequent invocation evidence records the expanded suite. All-target Clippy, formatting and source/license inventory comparison passed. A nested-if lint and a validator type-complexity lint were fixed without suppressions.

`input validate` does not claim live permission, freshness or tenant checks. Initial file canonicalization/root checks remain subject to the OS-safe open/race acceptance in the artifact task; no symlink/junction race qualification is inferred here. Native provider DTO/metadata conformance remains part of each provider's acceptance, not a claim derived from the synthetic fixture.

The resulting development executable is identified by SHA-256 `09bdba78de05943a3e85e06789e7f22ef9a32f9009c6bb8be302c9d8416162a9`; subsequent invocation/discovery tests cover this build's argument adapters. No installed or signed distribution is claimed.
