# Generate and review CLI contracts

From `cli/`, check the compiled public surface against its reviewed snapshot:

```text
python tools/contracts.py check
python tools/contracts.py selftest
```

The checker compiles and runs the development example exporter with locked, offline Cargo resolution. It reads the actual Clap command tree, operation registry, typed argument adapters and projection implementation. It parses examples but never executes resource commands, opens profile state, accesses credentials or calls a provider. It is separate from Craft's bounded agent-facing schema commands.

## Make an intentional change

1. Update the capability/design contract and the owning argument or response type. Change the command implementation with it.
2. Run `python tools/contracts.py check`. Changed contracts fail with at most twelve JSON paths and a marker when more differences exist. Values and full schema dumps are not printed.
3. Inspect the old snapshot and generated definitions at those paths. To inspect the complete generated candidate without changing the reviewed snapshot, run `cargo run --quiet --locked --offline -p craft-cli --example contracts`. Review request defaults/required fields, accepted flags, response/selection changes, scope declarations, effects and recovery semantics. Decide whether the public contract version must change.
4. Regenerate intentionally with `python tools/contracts.py write`, then run `python tools/verify.py`. Regeneration validates completeness before atomically replacing the snapshot. It cannot accept an incomplete operation merely to make a diff disappear.

The snapshot is [`schemas/local-contracts.json`](../schemas/local-contracts.json). It records twenty current local operations, including six local [plan/design authoring commands](planning.md) and [local operation inspection](operations.md). Adding a provider must extend the same exporter and checks rather than introduce a parallel schema inventory. Scope alternatives may be explicitly empty for local operations; they are not evidence of permission to access a remote service.

## Source ownership

| Contract | Owner |
| --- | --- |
| Command paths, flags, aliases, choices, arity and defaults | Clap declarations in `crates/craft-cli/src/args.rs` |
| Typed JSON inputs and response models | Argument structs and `catalog.rs`, registered through core schema generation |
| Argument decoding/serialization | `dispatch.rs`, shared between execution and development export |
| Selectable paths, projected schemas and mandatory fields | Core projection contract |
| Effects, scopes, permissions and retry/reconciliation | Operation descriptor |
| Compatibility checkpoint | Reviewed generated snapshot, not a second editable schema source |

Every action flag must map to an input property. A skipped structured property must explicitly declare `x-craft-input-channel: json_only` in its source schema annotation. Shared output selection uses `output_control`. An unannotated skipped field, undeclared flag or unknown shared control fails export. Both the documented example and a reduced valid input must produce matching flag and typed JSON arguments, so differing defaults cannot hide behind an explicit example value. Reduction removes optional fields only while the schema and typed/semantic validation still accept the input; conditional requirements can retain optional fields.

The export also compiles every selectable path and validates its selected response example through core registration. Full schemas and per-selection digests make response and projection changes visible even when the minimal example omits the changed field.

Heterogeneous union values, including plan rich text, are selectable as whole fields with their complete schemas. Variant-dependent child paths are not advertised and return `FIELD_UNKNOWN`. Traversal through a nullable object remains supported.

## What the checks establish

The development gate compares exact semantic JSON, including parser defaults and response schemas. It rejects missing authority/recovery fields, local-read retry semantics on a mutation and mutations without reconciliation metadata. Deliberate negative controls exercise parser/default mismatches, response/projection inconsistencies, snapshot drift and invalid regeneration.

The gate does not prove every command's business behavior or implement complete JSON Schema compatibility/subsumption analysis. A changed path requires review; it is not automatically labeled compatible or breaking. The initial checkpoint was reviewed by its author against source and fixtures. Independent consequential auth/write review and live-provider acceptance remain separate gates. See [observed evidence](evidence/contract-drift.md).
