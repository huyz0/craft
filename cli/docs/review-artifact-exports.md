# Artifact export review package

Status: **Windows host implementation ready for independent source review, 10 November 2026**. No reviewer was started. No artifact export command is registered. Consequential public activation remains subject to the [SDD review rule](../SDD.md) and later CLI/recovery integration.

## Review these boundaries

| Boundary | Source and question |
| --- | --- |
| Output authority | `crates/craft-platform/src/outputs.rs`: is the existing output root separate from input/provider metadata, is its checked parent retained, and does protected-root identity comparison refuse writes inside immutable storage before staging? |
| Native private staging | `crates/craft-platform/src/windows.rs`, `create_output_file`: do creation flags, owner/ACL, sharing and delete access bind a new owned staging file without replacing an entry? |
| Rename buffer safety | `publish_output_file`: verify native alignment, checked UTF-16 length, variable-tail bounds, terminating storage, union field/class selection and borrowed-handle lifetime against pinned windows-sys 0.61.2 and Microsoft documentation |
| Abort identity | `discard_output_file` and `OutputFile::drop`: does cleanup refer only to the owned handle, and are uncertain/final objects preserved? |
| Complete bytes before finalization | `crates/craft-runtime/src/artifacts/mod.rs`, `verify`; `artifacts/outputs.rs`: do exact source length/digest checks precede staging synchronization, and does invocation control run afterward immediately before commit? Does a failed sink/preparation prevent publication? |
| Effects and replay | Are default collision, explicit replacement, known failure, uncertainty and confirmed success distinct? Can cancellation or diagnostics erase a confirmed receipt? |
| Tests and scope | Native output tests and artifact export fixtures cover owned local paths, not live providers, cross-machine authority, other OSes or power loss |

The [export contract](workspace-outputs.md) records overwrite/ACL semantics, platform limitations and expected recovery. The [evidence](evidence/workspace-outputs.md) records actual checks and checkpoint hashes.

## Activation work still required

Before CLI registration, integrate trusted output selection, durable intent/receipts, staging ownership/leases and bounded orphan cleanup, inspect/reconcile operations, exact output-budget preflight and cancellation-safe envelope encoding. Decide how metadata/ACL sharing should be exposed. Keep source/destination content checks distinct from proof of the exact write event. Native fault coverage must be extended where integration changes the effect boundary.

Public registration and later consequential changes require review disposition. This source package does not authorize another agent, a tenant mutation, distribution or release. A human reviewer or an explicitly authorized separate reviewer can provide the required review.
