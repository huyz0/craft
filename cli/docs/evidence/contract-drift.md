# Contract export and drift evidence

Date: **10 November 2026**, Australia/Sydney. This record covers local contract generation and the development drift gate, not cloud functionality or independent auth review.

Later [disabled provisioning adapter work](provision-dispatch.md) adds string-list flag examples and validation-preserving example reduction for alternative-required fields. Its gate still verifies thirteen public operations, with no snapshot change or public provisioning registration. The original eleven-operation checkpoint below remains historical evidence.

## Changed artifacts

- `crates/craft-cli/src/lib.rs` shares the existing composition modules between the binary and development exporter. `main.rs` uses those same definitions.
- `args.rs` declares shared invocation controls and source annotations for intentionally JSON-only/output-control fields. `dispatch.rs` shares argument decoding and serialization with the exporter.
- `contracts.rs` enumerates the actual command tree, checks registered paths/arguments, compares flag and JSON inputs with defaults, and compiles/validates each selected response example through core registration.
- `examples/contracts.rs` emits the deterministic development snapshot without executing resource operations.
- `tools/contracts.py` validates metadata completeness and conservative effect/recovery consistency, compares bounded semantic paths, atomically regenerates reviewed snapshots and executes negative controls.
- `schemas/local-contracts.json` is the initial source-reviewed checkpoint for eleven local operations. It contains full descriptors/request/response schemas, parser flags/aliases/defaults/choices/arity, control applicability, provider manifests and projection digests.
- `tools/verify.py` includes snapshot checking and contract-checker selftests in the normal gate. [Usage and source ownership](../contracts.md) describe intentional regeneration.

No dependency, feature or Cargo version change was required. The package's new library/example targets reuse its existing dependencies.

## Observed checks

`python tools/contracts.py write` generated the initial snapshot. Subsequent `check` passed without rewriting it. `selftest` passed ten controls: a valid snapshot; flag, response and projection drift with restoration; three missing authority/recovery metadata fields; two invalid mutation-recovery declarations; and duplicate property refusal. Invalid regenerated descriptors fail before replacing the snapshot.

`cargo test -p craft-cli --lib --locked --offline` passed four Rust tests. They exercise unrepresented action flags and unknown shared controls, a Clap default changed while the typed JSON default stays fixed, response/projection changes inconsistent with registered examples, and the actual source annotations/flags/selectable fields. These tests parse arguments and validate examples without resource I/O.

The first exporter attempt failed on an empty argument schema with no `properties` member. Empty object schemas are now accepted while commands with missing property/flag mappings still fail. That exploratory scratch output was overwritten by later development output; no persistent failure log is claimed for it.

`python tools/verify.py` passed with native loopback/filesystem access: **170 named Rust tests passed, zero failed and two opt-in real-vault tests were ignored**, plus snapshot checking, ten contract controls, formatting, dependency/layer validation and all-target Clippy. The full gate log is `target/contract-drift-verification-output.log`. Separate quality/planning selftests passed nine/fifteen controls. Locked inventory remains 278 packages. Incremental-cache hard-link warnings used copying and did not fail the gate.

`cargo build --workspace --locked --offline` passed, and direct `craft version --fields version` returned a valid projected version envelope preserving mandatory identity fields. Development binary SHA-256 is `b54ea2146d87ffd6ff0ff022c7cbd9748ba3bc6af430667e1c5b714ab2a23652`.

| Checkpoint | SHA-256 |
| --- | --- |
| `schemas/local-contracts.json` | `ddac632380fb47341898aca4d14ad71344c210cf96282760247440284c1d8653` |
| `crates/craft-cli/src/contracts.rs` | `0d4165a1446b8366b8faf0dffe39349dbdae5f0c6c693a4e02b0fcb625e85fff` |
| `tools/contracts.py` | `446620273f7443a16ab4b9bcdede9c9d383fe5f0bcb1aa01cf7d192f2ef09d56` |

## Review and limits

The initial snapshot was inspected against source ownership, the eleven registered local operations, actual parser flags, source-only JSON exceptions and effect/recovery declarations. This is author review, not independent review of cloud authorization or a complete compatibility proof.

The checker reports exact changes instead of claiming full JSON Schema subsumption. Scope metadata must be explicit; empty scope alternatives do not prove remote access. Regenerating a snapshot does not verify runtime command effects, live scopes/permissions, platform behavior or agent token efficiency. Existing CLI behavior tests remain necessary, and new cloud operations must add their own route/auth/recovery acceptance evidence.

No credential-facing command was enabled, credential accessed or cloud resource changed. No commit, push, deployment or release occurred.
