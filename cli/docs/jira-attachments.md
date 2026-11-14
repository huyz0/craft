# Jira attachments

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

No file is written, downloaded, unpacked or executed by these three metadata commands. See [acceptance evidence](evidence/jira-attachment-reads.md).

## Download bytes to an explicit output

`jira.attachment.download` requires its own enabled Jira read allowance. Metadata read allowances do not imply it; records containing download use format five. Read-policy operation capacity is fourteen, while mutation policies remain separate.

```powershell
craft jira attachment download --id 10001 --artifact-name design.bin --output-root D:/exports --output design.bin --byte-limit 16777216 --profile factory
```

The output root and any destination parent must already exist. The relative output path is supplied by the operator, never by native filename metadata. A 16 MiB default can be raised to at most 1 GiB. Supply `--expected-digest sha256:<64-lowercase-hex>` when an independently expected digest is available. The route fixes redirect=false; partial responses, redirects, encoded bytes and unsupported transfer codings fail without publication or automatic retry.

Complete bytes first become an immutable artifact in the private downloads collection, bound to the attachment identity with a byte count and SHA-256. Export then uses guarded atomic publication. Existing files are refused unless `--overwrite` is explicit. Destination ancestry cannot include the selected Craft private state root, including paths reached from an ancestor output root. Guarded export is currently Windows-only.

Read artifact_write_state and export_write_state independently, retaining artifact and exported receipts. Export failure or a late refusal can leave a confirmed artifact. A name collision prevents the binary GET; it does not silently select another name. Uncertain publication requires local inspection of that same identity before another transfer. Use [local artifact recovery](local-artifacts.md) to inspect the same artifact and export its verified bytes without another GET. Binary bytes never mix with the JSON response. [Download evidence](evidence/jira-attachment-download.md) separates local fixture acceptance from unrun native/live qualification.

## Upload a file

`jira.attachment.upload` attaches one local file to one issue. It needs a project `jira-mutation` allowance containing `jira.attachment.upload` (policy format six), the `write:jira-work` scope and native Create attachments permission.

```powershell
craft --profile jira-test --max-bytes 8388608 jira attachment upload --operation-id upload-design --project-id 10000 --issue-id 10001 --artifact-name design-upload.bin --file-root D:/workspace --file design.bin --filename design.bin --media-type application/octet-stream --apply
```

The source is either a relative `--file` under an existing absolute `--file-root`, or an existing private snapshot selected with `--from-artifact --expected-digest sha256:<hex>`. A workspace file is first copied into the private `uploads` collection as an immutable, digest-bound snapshot named by `--artifact-name`; the bytes sent are streamed from that snapshot and verified again while streaming. `--filename` is the name Jira shows and `--media-type` is the part's content type; neither is taken from the file system. `--byte-limit` (default 16 MiB, at most 1 GiB) bounds the snapshot. `--expected-digest` with a workspace source refuses a file whose bytes differ.

Before the send, the host reads the issue's project, attachment settings and the actor's Browse projects and Create attachments permissions, then rechecks the issue. A disabled attachment feature, a file larger than the site upload limit, a foreign project or a missing permission refuses before any POST. The upload is one multipart POST to `/rest/api/3/issue/{issueIdOrKey}/attachments` with `X-Atlassian-Token: no-check`, journaled under the operation ID before it is sent and never retried.

The receipt keeps the snapshot, the saved request, the journal records and the returned attachment metadata independently. `desired_fields_verified` means Jira's response named the expected filename and size; `remote_digest_verified` is always false because Jira returns no content digest. A filename match alone never proves that an uncertain upload succeeded. A lost or sparse acknowledgment leaves `sent_unknown` and `reconciliation_required`; rerunning the same operation ID refuses rather than uploading a second copy. To retry after a refusal that happened before the claim, reuse the same operation ID and source; the saved request binds the original snapshot digest.

## Delete an attachment

`jira.attachment.delete` permanently removes one attachment. It needs a project `jira-mutation` allowance containing `jira.attachment.delete-own` or `jira.attachment.delete-any` (policy format seven), the `write:jira-work` scope and native Delete own or Delete all attachments permission.

```powershell
craft --profile jira-test --max-bytes 8388608 jira attachment delete --operation-id remove-design --project-id 10000 --issue-id 10001 --attachment-id 10002 --expected-revision sha256:<metadata_revision> --ownership own --apply
```

`--expected-revision` is the `metadata_revision` from `jira attachment get` or `list`. Jira's attachment metadata does not name its issue, so the host confirms that the issue is in the target project and that its attachment field lists the attachment, reads the attachment metadata and compares the fingerprint, checks the permissions, then repeats the metadata and membership reads. `--ownership own` (the default) also requires the authenticated actor to be the uploader; `any` requires the delete-any allowance and native Delete all attachments. The checks narrow but cannot close the window for a concurrent change, because Jira has no conditional delete; `atomic_revision_check` is always false.

The deletion is one `DELETE /rest/api/3/attachment/{id}`, journaled before it is sent and never retried. 204 confirms it. 403 and 404 are recorded as refused, with nothing to reconcile. Any other status or a lost response is `sent_unknown`; inspect the attachment before doing anything else, and do not rerun the operation ID, which refuses. `before` records the metadata that was deleted. There is no binary update route: replacing a file is an upload followed by a delete.

[Upload and delete evidence](evidence/jira-attachment-writes.md) lists the fixture and live checks.
