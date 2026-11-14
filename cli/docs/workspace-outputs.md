# Export a captured artifact to an explicit destination

The Windows host library can export a captured non-secret artifact to a selected existing output directory. This complements [workspace input snapshots](workspace-inputs.md). It is an internal API; the executable has twenty local commands and no artifact export command.

The host constructs `craft-platform::OutputRoot` independently of input authority. `NativeArtifacts::export_file` receives the artifact's logical name, that output root, a relative destination and an `OverwritePolicy`. Neither a provider filename nor a persisted receipt path can select the output root.

## Publication and overwrite

`OverwritePolicy::CreateNew` is the default. Any existing destination is a collision. `Replace` is an explicit choice to create an absent file or replace the selected ordinary file. Before staging, an existing replacement target must pass the native regular-file, reparse and single-link checks. A directory, hard link or junction is refused. Replacement is not a compare-and-swap guarantee against another writer changing that selected pathname.

Parents must already exist. Traversal, absolute destination paths, Windows device/alternate-stream aliases and linked ancestors are refused. Checked parent handles remain live through publication. No missing output root or parent is created automatically.

The artifact export API also excludes its own immutable store before staging. It compares the native volume/file identity of the protected root with the retained destination ancestry, covering alternate path spellings. Explicit replacement cannot mutate another captured artifact or create a loose file inside that store. The host must separately exclude any other private state roots when selecting a future public output scope.

The host generates an unpredictable `export_` staging identity. A private file is created in the destination parent with create-new semantics. Its writer is bounded to the source artifact's exact size, with a 1 GiB platform ceiling. Source metadata, exact size and SHA-256 are verified while streaming in 64 KiB chunks; corrupt or incomplete source bytes never reach final publication.

Windows staging retains one handle with read/write/delete access and denies other write/delete sharing. `SetFileInformationByHandle` renames that same object after flush/sync, without a close-and-reopen source-path gap. The replacement flag is chosen from the explicit policy. A create-new race has one complete winner and preserves that winner against the other attempt. No copy fallback or automatic switch to replacement exists. [Native rename semantics](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info).

The exported file retains the private staging ACL. Replacement does not preserve the previous file's ACL, owner, timestamps or alternate streams. Sharing or metadata-preservation behavior needs its own explicit policy before a public workflow relies on it. The source artifact remains in its immutable store.

## Receipts and recovery

A successful export returns the normal artifact receipt, preserving logical name, media type, byte count, digest and source provenance, with `local_path` reconstructed from the selected destination. Binary bytes go to the file, not JSON stdout. This receipt is a content/location result, not human approval or proof of a remote write.

Failure before publication returns a known local failure and attempts to discard only the retained staging object. Windows abort marks that owned handle for deletion on close. It does not reopen a staging pathname for deletion. [Disposition and required access](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle).

Once rename is attempted, an unclassified native error returns `OutcomeUnknown`. A failed acknowledgment does not prove the destination is absent or still contains old bytes. Inspect the selected destination against the known source size/digest before deciding what happened; matching content is evidence of the desired bytes, not proof of a particular historical write event. No automatic replay or rollback occurs. Uncertain staging is preserved rather than deleted as though publication definitely failed.

The runtime synchronizes staging, then checks invocation control immediately before publication. A cancellation observed at that boundary refuses completion and preserves the old destination. Any further write invalidates staging's synchronized state. Native filesystem calls themselves are not interruptible. Once publication is confirmed, later cancellation preserves the complete receipt. Fault fixtures separately cover cancellation after synchronization, failure immediately before publication, lost acknowledgment afterward and cancellation after success.

## Scope before public exposure

Only the Windows implementation is available. Other platforms return `Unsupported`; no unqualified path-based fallback exists. The native tests use owned local Windows fixtures. Network/mapped filesystems, power-loss durability, permission changes by privileged/malicious writers and every possible driver behavior are unqualified.

Process death can leave a private staging file. Durable export intent, leases/ownership records, bounded orphan cleanup and CLI inspection/reconciliation still need integration before public exposure. The native unknown-error branch is conservative; the acknowledgment-loss test injects that condition after an actual successful rename and does not simulate every OS failure.

The public command will also need exact output-envelope preflight so a small output budget cannot hide a completed filesystem effect. Independent consequential review is required before enabling overwrite or unknown-write handling through the CLI. [Review package](review-artifact-exports.md), [observed evidence](evidence/workspace-outputs.md). The broader artifact task remains unchecked.
