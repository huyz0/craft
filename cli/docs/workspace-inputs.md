# Read selected workspace inputs

Craft's existing `--input FILE` and local document `--file FILE` reads now use checked native input handles. The current project directory supplies the trusted root. Relative paths and contained absolute paths are accepted; traversal, links and nonregular inputs are refused before JSON/document decoding. Missing inputs are never created. Stdin remains the explicit `-` path and keeps its existing bounded worker and cancellation behavior.

The same input boundary supports an internal upload-preparation API that captures an immutable artifact before a future network send. No artifact upload/export command or cloud capability is enabled by this work.

## Existing CLI behavior

```text
craft plan validate --file plan.json
craft command find --input request.json
```

Inputs remain bounded by the CLI's 1 MiB ceiling and invocation timeout. Hard links are rejected even when the link name is inside the project, so a linked external file cannot become an authorized input. Symlink/junction targets are not canonicalized and followed. Device/alternate-stream aliases and trailing dot/space names are refused on Windows. ASCII case variants and normal/verbatim local-disk absolute spellings inside the Windows project root are supported. Unicode content and ordinary Unicode filenames remain supported.

The root itself must be an existing checked directory with no link components. Ordinary workspace permissions are accepted; they are not changed to the private permissions used for Craft state. An inaccessible, linked, nonregular or changed file returns a sanitized input error. A lexically outside absolute path returns `POLICY_DENIED`. These checks occur before profile/auth/network execution.

## Native host API

`craft-platform::InputRoot::open` selects an absolute existing directory. `read_file` accepts a relative path and returns an `InputFile` implementing `Read`, without a write method or exposed raw file handle. Selection is bounded to 64 components and 4,096 UTF-8 path bytes per root/relative argument. The input root is host authority, never a value extracted from a provider filename, page, attachment or receipt.

Windows retains checked directory handles without delete sharing, and opens the source file without write/delete sharing. The file guard retains ancestor handles even after the original root guard is dropped. Reparse points and files with multiple hard links are refused. Native file identity, size and modification time are retained and rechecked.

Unix source uses safe `nix` descriptor-relative `openat` calls with no-follow flags and directory descriptors. Final files are opened nonblocking to avoid blocking on a FIFO before type inspection. Regular files require one hard link. Device/inode, length, modification time and change time form the retained source stamp. This source and its Unix-only edit/symlink test have not been compiled or executed here; only the Windows target is installed.

`InputFile::verify_unchanged` checks the stamp. Reading to EOF also performs that check, allowing the existing bounded worker to refuse changed inputs before returning bytes. Ordinary source edits are detected or, on Windows, excluded by sharing rules. This is not an atomic filesystem snapshot against malicious same-principal writers, writable mappings or every filesystem's timestamp behavior. Explicit expected digests provide a separate content check.

`craft-runtime::inputs::read_project_file` combines root containment, native selection and the bounded worker. Source bytes and native errors are not echoed in CLI diagnostics. A deadline can leave a worker finishing synchronous filesystem I/O until process exit, as with the previous bounded reader; synchronous root/file opening is not itself interruptible. Mounted/offline storage stalls remain unqualified.

## Prepare an immutable upload input

`NativeArtifacts::import_file` receives a selected `InputRoot`, relative path, `ArtifactSpec` and optional expected `sha256:` digest. It validates the spec/digest and size ceiling before staging. It streams in 64 KiB chunks, checks invocation control between chunks, and refuses excess bytes, changed source length/stamp or a digest mismatch before finalization. Zero-length, Unicode and multi-chunk binary inputs are supported.

Successful finalization returns the normal size/digest/provenance receipt for the private immutable copy. The external source path is not persisted or reopened through that receipt. Later source edits do not change the prepared bytes. An existing logical artifact name is preserved; an import cannot overwrite it. An import failure publishes no complete artifact. Generated staging that cannot be reclaimed immediately retains the store's existing explicit cleanup behavior.

Cancellation before publication refuses completion. Confirmed publication uses the store's existing receipt semantics, including preserving a confirmed receipt if cancellation arrives afterward. An uncertain acknowledgment still needs receipt reconciliation. The import's optional resource provenance is supplied by the host; a local file snapshot does not establish remote identity or reviewer approval.

Windows host [export and overwrite handling](workspace-outputs.md) now has separate partial acceptance. Public export recovery/cleanup, binary HTTP transport, operation-context attachment composition, independent consequential review and Linux/macOS native qualification remain unfinished. The broader artifact task stays unchecked. See [input acceptance evidence](evidence/workspace-inputs.md) and [native artifact storage](artifacts.md).
