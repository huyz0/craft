# Read Jira attachment metadata

Craft exposes `jira attachment get`, `jira attachment list` and `jira attachment settings`. They use the configured Jira Cloud service profile and explicit site `jira-read` allowances. Settings do not establish project upload permission; metadata does not include file bytes.

Configure `jira.attachment.get`, `jira.attachment.list` and/or `jira.attachment.settings`. These require policy format four; formats one through three grant none of these operations. Up to thirteen Jira read allowances can coexist. Updates replace the complete operation list and require the observed revision; retain earlier reads you still need. See [policy setup](policy-setup.md).

```powershell
craft --profile jira-test jira attachment settings
craft --profile jira-test jira attachment list --issue-id 10000 --limit 10 --fields id,filename,size_bytes
craft --profile jira-test jira attachment get --id 10001 --fields id,filename,mime_type
```

The native classic scope is `read:jira-work`, plus the identity-probe scope. Metadata still depends on native Browse projects, issue security and private-comment restrictions. Craft uses authenticated service access even where Jira permits anonymous reads. The [official attachment reference](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-attachments/) documents these distinctions; settings have no native permission requirement but still require Craft's local allowance.

Metadata defaults include ID, filename, size in bytes, MIME type, creation time and a mandatory metadata fingerprint. Author account ID is explicitly selected. Missing/null fields stay null. Emails, display names, content/self/thumbnail URLs and arbitrary properties are omitted. No returned URL supplies a request destination. Filenames are inert metadata, never a filesystem path or proof that an uncertain upload already succeeded. `metadata_revision` fingerprints normalized metadata; it is not a digest of file bytes or a native atomic version.

List fetches only the exact issue's attachment field. There is no native attachment-list paging endpoint. `--limit` is an output limit (1–100, default 20); returned opaque cursors consume the locally buffered remainder with unchanged issue, projection, profile/site/grant/actor binding. Resume does not refetch buffered attachments. The native response is bounded to 4 MiB and the array to 1,000 entries; the shared pager also bounds normalized buffered data to 512 KiB. Larger responses fail explicitly rather than being truncated. A cursor's observation is historical and cannot prove current issue/comment visibility or complete access to hidden attachments.

Get requires the exact positive numeric attachment ID. Native integer and canonical-string IDs normalize to a string. Settings always retain `enabled` and `upload_limit_bytes`; absent evidence stays null rather than implying disabled, unlimited or permitted. Reads do not retry automatically, and errors preserve independent credential-cache receipts. Neither denial nor 404 proves that the attachment is absent.

Binary download, multipart upload and deletion remain unfinished. No file is written, downloaded, unpacked or executed by these three commands. See [acceptance evidence](evidence/jira-attachment-reads.md).

## Download bytes to an explicit output

`jira.attachment.download` requires its own enabled Jira read allowance. Metadata read allowances do not imply it; records containing download use format five. Read-policy operation capacity is fourteen, while mutation policies remain separate.

```powershell
craft jira attachment download --id 10001 --artifact-name design.bin --output-root D:/exports --output design.bin --byte-limit 16777216 --profile factory
```

The output root and any destination parent must already exist. The relative output path is supplied by the operator, never by native filename metadata. A 16 MiB default can be raised to at most 1 GiB. Supply `--expected-digest sha256:<64-lowercase-hex>` when an independently expected digest is available. The route fixes redirect=false; partial responses, redirects, encoded bytes and unsupported transfer codings fail without publication or automatic retry.

Complete bytes first become an immutable artifact in the private downloads collection, bound to the attachment identity with a byte count and SHA-256. Export then uses guarded atomic publication. Existing files are refused unless `--overwrite` is explicit. Destination ancestry cannot include the selected Craft private state root, including paths reached from an ancestor output root. Guarded export is currently Windows-only.

Read artifact_write_state and export_write_state independently, retaining artifact and exported receipts. Export failure or a late refusal can leave a confirmed artifact. A name collision prevents the binary GET; it does not silently select another name. Uncertain publication requires local inspection of that same identity before another transfer. Use [local artifact recovery](local-artifacts.md) to inspect the same artifact and export its verified bytes without another GET. Binary bytes never mix with the JSON response. [Download evidence](evidence/jira-attachment-download.md) separates local fixture acceptance from unrun native/live qualification.
