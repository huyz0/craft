# Native artifact store evidence

Observed on 9 November 2026, Windows x86_64 MSVC, Rust 1.99.1. The artifact task remains unchecked: root-managed storage is implemented, while external input/export selection, operation-context/CLI integration, Unix native qualification and independent consequential review remain incomplete. The full CLI delivery goal remains active.

Follow-up on 10 November: [workspace input snapshots and guarded CLI file reads](workspace-inputs.md) now have separate partial acceptance. The external-input limitation below describes the original checkpoint; explicit export, binary transport, full attachment composition, multi-platform acceptance and consequential review remain unfinished.

## Artifacts and boundaries

`crates/craft-runtime/src/artifacts/` implements the artifact port with private staging, incremental size/digest accounting, bounded strict metadata, atomic no-replacement directory publication, verified reads, root maintenance locks and live-writer leases. Final paths derive from the selected trusted root and hashed logical names. Metadata paths are refused. The core storage error contract now distinguishes pre-publication cancellation and deadline expiry.

`crates/craft-platform/src/entries.rs` adds existing-only directory/file access, read-only regular files, create-new private files/directories and exclusive publication. `windows.rs` adds one scoped native move wrapper. `unix.rs` contains minimal exclusive-rename calls for Linux/macOS; their NUL-terminated path buffers remain live through synchronous calls and no raw pointer crosses the safe API. Other crates continue to forbid unsafe code. Existing platform tests cover create/read/replace behavior and the Windows native wrapper.

These changes implement the [native artifact design](../../openspec/changes/build-cloud-cli/design.md#native-artifact-storage), [portable artifact scenarios](../../openspec/changes/build-cloud-cli/specs/agent-interface/spec.md) and [adapter contract](../artifacts.md). There are no enabled external writes or new production test-store selectors.

## Observed checks

| Check | Result |
| --- | --- |
| `cargo test -p craft-runtime --lib artifacts::tests --locked --offline` with native filesystem access | 13 tests passed. One is an explicitly selected child helper and otherwise does no work. |
| Artifact content and persistence | Empty, Unicode and multi-chunk binary round trips; exact digest/size and raw local path; fresh-process reads; missing reads create no entries. |
| Publication and recovery | Competing native writers produce one winner; empty/existing destinations survive; before/after publication faults distinguish absent from confirmed/unknown state; cancellation before publication refuses completion and cancellation after publication preserves the receipt. |
| Cleanup | A ready-confirmed child holds a live staging lease, cleanup skips it, the parent kills/waits the child, and cleanup then reclaims exactly its abandoned staging. Unknown children are refused before any deletion and no lease is manufactured for missing input. |
| Path/content refusal | Traversal/alternate-stream names, oversize/failed writes, forged paths/names/digests/lengths/media/unknown JSON, duplicate fields, overlarge metadata, corrupted data, hard links and junctions are refused. DOS device/case/trailing-dot logical aliases remain distinct. Junction targets remain untouched. |
| `cargo test -p craft-platform --locked --offline` with native filesystem access | Seven tests passed, including existing-only read/create-new refusal and atomic publication preserving an empty destination. |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | Passed for the integrated artifact implementation and tests. |
| Dependency comparison | No package/version/feature addition or removal. Runtime adds a direct dependency on the existing pinned SHA-256 library. Inventory remains 278 packages; only its workspace runtime manifest hash/dependency edges changed. |

The initial focused restricted-runner attempt passed seven tests and failed the hard-link case with Windows access denied. The exact hard-link test and subsequent full artifact suite passed with native filesystem access; no test was removed or weakened. Early compilation identified SHA-256's array type lacking lower-hex formatting, which was corrected with bounded hexadecimal encoding. The lockfile was updated offline only for the new existing-library dependency edge.

The integrated `python tools/verify.py` gate passed 132 named Rust tests, five subprocess controls, formatting, locked inventory and all-target Clippy. Two opt-in native-vault tests were ignored; their earlier explicit acceptance remains recorded in native-vault/generation evidence and was not repeated here. Output is retained at `target/artifact-verification-output.log`. Quality-checker selftests passed nine controls; planning-checker selftests passed fifteen. The planning guard still reports 85 tasks, ten checked complete; this artifact task remains unchecked.

The first integrated gate failed in the existing delayed HTTP recorder test with Windows connection reset. Accepted sockets could inherit the nonblocking listener mode, causing fragmented requests to be refused before their remaining bytes arrived. The fixture now explicitly selects blocking mode before its read deadline. A regression waits for acceptance, then sends fragmented headers and body. All seven HTTP recorder tests and the complete gate passed afterward. The original failure remains at `target/artifact-verification-failed.log`; no test/retry assertion was weakened.

Current development executable SHA-256: `fd8b828ad145d607f51685962ebfcbdf5e5cf2715c3b8f2edf170fb400d46c98`. Compiler incremental hard-link warnings used copying fallback; no quality gate was relaxed.

## Remaining qualification

Linux/macOS compilation and native filesystem cases remain unrun. Their API signatures/constants were inspected in pinned cached libc 0.2.189 source, not executed. Real disk-full/I/O failures, OS restart/power loss, malicious same-principal modification, external workspace-path import/export, binary provider transport, operation-context composition and independent review remain unqualified. Unit faults and process-kill recovery do not prove those conditions.

The root limit applies per artifact, not as a total disk quota. Cleanup is bounded and may report an error after earlier staging was reclaimed. No final artifact deletion or overwrite API is enabled. No real OAuth credential, live provider, repository commit, push, publication or release was changed.

Local receipt inspection and digest-bound export recovery are described in [local recovery evidence](local-artifact-recovery.md). That adapter does not establish live-provider or cross-platform qualification.

## Closure

On 15 November 2026 the owning ledger task was closed. The remaining gaps listed above are answered in [foundation closure](foundation-closure.md), with platform runs in [platform qualification](platform-qualification.md) and review in [foundation review](foundation-review.md).
