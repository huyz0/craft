# Native artifact storage

`craft-runtime::artifacts::NativeArtifacts` implements the core artifact port for non-secret files. It is an internal API, without an artifact or attachment CLI command yet. The launcher explicitly chooses a private root, byte ceiling and invocation control. Provider content may supply a validated logical name, media type and resource provenance; it cannot select filesystem roots or overwrite policy.

## Publication and receipts

`begin(ArtifactSpec)` accepts a positive per-artifact byte ceiling no larger than the store ceiling. The hard ceiling is 1 GiB. Writers stream to private generated staging directories, update SHA-256 incrementally and refuse excess bytes. A failed write poisons finalization. Dropping an unfinished writer attempts cleanup without waiting for a lock; any remaining staging can be reclaimed explicitly.

`finish` flushes data and bounded JSON metadata before moving the complete directory to its final name atomically. The move refuses replacement and cross-volume copy. Windows uses `MoveFileExW` with write-through and without replacement/copy flags. Linux uses `renameat2(RENAME_NOREPLACE)` and macOS uses `renamex_np(RENAME_EXCL)`. Unsupported filesystems fail without substituting a non-atomic operation. Only Windows has compiled and passed native acceptance here; the Unix implementations remain unqualified.

The final directory name hashes the exact logical name. Names such as `CON`, `NUL`, `Foo`, `foo` and `trailing.` remain separate logical artifacts without becoming OS device paths or case aliases. Fixed entries contain the raw `data`, `receipt.json` and an inactive lease file. The receipt records logical name, media type, exact size, SHA-256 and optional resource reference. Its local path identifies the raw data file. Persisted metadata must not supply a path; the host reconstructs it from the checked root.

A confirmed publication returns its receipt even if cancellation arrives afterward. An uncertain acknowledgment returns `OutcomeUnknown`; use a fresh invocation to read and compare the receipt before further action. Do not infer absence or repeat an upstream write/download from that error. Concurrent publication has one winner, and an existing destination is preserved even if it is empty or malformed. No overwrite option is implemented.

## Reads and cleanup

`receipt(name)` verifies the entire data file with bounded streaming memory. `read(name, max_bytes)` also returns the bytes within both caller and store limits. Missing objects return `None` without creating inputs. Linked, insecure, incomplete, malformed or digest-mismatched objects return an error, never a successful complete receipt. The file APIs use checked regular files, reject hard links/reparse points and open reads without write access. Root/path guards retain native handles during relevant operations.

`cleanup_pending` scans at most 4,096 root entries per call. It recognizes only generated staging names, holds a root maintenance lock and tries each staging lease without waiting. Live writers are skipped. A killed writer releases its lease for later cleanup. Initialization can fail before lease creation; the root lock excludes a live creator in that window. Cleanup validates all staged entries before deleting only the fixed owned files. Unknown children, links or insecure paths are refused; final artifacts are never reclaimed. An error can follow earlier reclaimed staging, so cleanup is idempotent rather than transactional.

Cancellation/deadline errors before publication retain distinct storage errors. Streaming checks cancellation between bounded chunks; synchronous filesystem calls themselves are not interruptible. Process restart/crash tests do not establish power-loss durability or protection against malicious code running as the same principal.

## Workspace inputs and remaining integration

The selected private root is the adapter's artifact input/output root. A separate host-selected `InputRoot` now supports read-only workspace inputs and `NativeArtifacts::import_file` snapshots them into this store after size/change/expected-digest checks. Existing CLI file arguments use the same guarded reader. See [workspace input behavior](workspace-inputs.md) and its [partial acceptance](evidence/workspace-inputs.md).

The Windows host API now also supports [explicit export](workspace-outputs.md), with default collision refusal, explicit replacement and complete size/digest verification before handle-bound publication. Public export recovery/cleanup, HTTP binary transport and attachment operation-context integration remain unfinished. The artifact task stays unchecked pending those surfaces, other native platforms and independent consequential review. This store is not a credential fallback, approval authority or a mechanism for automatically executing/unpacking artifacts. See [earlier store evidence](evidence/artifact-store.md) and the [design](../openspec/changes/build-cloud-cli/design.md#native-artifact-storage).
