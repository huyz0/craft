# Inspect and export saved artifacts

Use the same private state root that produced the artifact. These commands use no profile, credentials or cloud requests. Choose a closed collection: downloads (default), requests or reviews. Names come from retained receipts or the original command input, not filesystem paths.

```powershell
craft artifact get --artifact-name design.bin --state-root D:/craft-state
```

Get streams the stored object through SHA-256 and size verification, returning its receipt without content bytes. found=false means the exact local object was not observed. An unreadable or corrupt object returns an error with found=null; it does not establish absence. The verification limit defaults to 16 MiB. Raise byte-limit explicitly, up to 1 GiB, for larger known objects. Item-too-large is distinct from corruption.

To recover a failed download export, reuse the digest already returned by the download or observed with artifact get:

```powershell
craft artifact export --artifact-name design.bin --expected-digest sha256:0000000000000000000000000000000000000000000000000000000000000000 --output-root D:/exports --output design.bin --state-root D:/craft-state
```

The digest above is an illustrative value. Replace it with the observed digest. Export checks it against the same opened source that it copies, verifies bytes again, then publishes to an explicitly selected existing output root and relative destination. Parent directories must exist. Existing files are refused unless overwrite is explicit. Native path checks exclude the entire selected private state root, including destinations reached from an ancestor output root. Guarded output publication is currently Windows-only.

Export is an immediate local write. Before publication the command reserves space for complete success/error receipts. It never downloads again and never retries publication automatically. Keep artifact, exported and export_write_state independently: a confirmed source can remain after failed export, and confirmed output remains real after late cancellation. sent_unknown requires inspection of the exact output before retrying; a missing output is not established by a null receipt.

Profile selection and projection are refused. Stored request/review artifacts remain local data, not current human approval, live-provider verification or cloud replay authority. These commands do not enumerate private directories or delete saved artifacts.

[Local recovery evidence](evidence/local-artifact-recovery.md) records validation and limitations.
