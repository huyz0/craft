# Jira attachment upload and delete evidence

Recorded **14 November 2026**, Australia/Sydney, on the Windows development host. Usage is in [Jira attachments](../jira-attachments.md); the transport underneath upload is in [multipart upload transport](multipart-upload-transport.md).

## What changed

- `jira.attachment.upload` is now a public command. The CLI host (`crates/craft-cli/src/attachment_upload.rs`) and provider context (`crates/craft-provider-jira/src/attachment_upload.rs`) already existed and were source-reviewed with the transport; activation registers them in discovery, parsing and dispatch without changing them.
- `jira.attachment.delete` is new. The provider command and its internal context are in `crates/craft-provider-jira/src/attachment_delete.rs`, the journaled host in `crates/craft-cli/src/attachment_delete.rs`.
- The project mutation policy gains `jira.attachment.delete-own` and `jira.attachment.delete-any` under record format seven (`crates/craft-runtime/src/issue_mutation_policy.rs`); the mutation bound rises from fourteen to sixteen operations. Earlier formats reject the new values.
- `tools/live_host.py attachments` runs upload, list, get, download and delete through the built executable on a fresh `craft-test` issue and deletes the issue afterwards.

## Fixture checks

Provider, `cargo test -p craft-provider-jira --lib attachment_delete` (7 passed):

- preparation reads issue (`fields=project,attachment`), metadata, permissions, metadata, issue in that order and freezes only `DELETE /rest/api/3/attachment/{id}` with no body;
- the delete route is the documented permanent attachment path;
- own intent refuses another uploader; any intent requires `DELETE_ALL_ATTACHMENTS`; own intent without either delete permission refuses;
- a stale metadata revision, a foreign project or an issue that does not list the attachment refuses, the last two after one read;
- preparation older than 30 seconds refuses to verify;
- 204 confirms the attachment resource; 403 and 404 are failed; 200, 429, 500 are unknown;
- non-canonical identities, malformed revisions and unknown fields are rejected.

CLI completion, `cargo test -p craft-cli --lib attachment_delete` (5 passed): confirmed identity is recorded; a confirmed deletion survives an unknown local acknowledgment; a lost response is unknown while 403/404 are failed; foreign-binding or wrong-route evidence cannot confirm; extra recorded effects require inspection.

Composed host over owned TLS, `cargo test -p craft-cli --test jira_issue_reads_host --features synthetic-acceptance -- deletes:: uploads::` (7 passed):

- delete sends exactly one `DELETE` to `/ex/jira/<cloud>/rest/api/3/attachment/301` after the five context reads, the response validates against the published schema, private author and URL values do not reach output, and rerunning the confirmed operation ID refuses without another request;
- an own-only allowance refuses any intent, and missing apply refuses, both before any request;
- a lost delete response is `sent_unknown` with reconciliation required, and the rerun refuses;
- the four existing upload host cases (exact snapshot bytes and `X-Atlassian-Token: no-check`, no second POST on repeat, missing scope or live permission refusal before POST, pre-claim refusal resuming only the frozen snapshot, lost or sparse acknowledgment blocking replay) pass unchanged.

`local_interface` lists both commands in discovery and `schema`.

## Live checks

Run on **14 November 2026** after the site owner's written approval, through the built executable with the `CraftTest` service account (`tools/live_host.py lifecycle`). On a fresh `craft-test` issue: upload of a 36-byte text file was acknowledged with filename and size verified; list showed exactly the uploaded ID; get reported 36 bytes and `text/plain`; download produced bytes whose SHA-256 equalled the uploaded snapshot digest; delete with own intent and the observed `metadata_revision` returned 204 and confirmed; a follow-up list was empty; the issue was then deleted. Two runs passed identically. No mock divergence was found: the live context reads (issue with `project,attachment`, attachment metadata, my-permissions) and the 204 response matched the fixtures' shapes.

## Limits

- Jira returns no content digest, so `remote_digest_verified` stays false; the live download compares the bytes Jira serves with the uploaded digest instead.
- The delete fingerprint check is not atomic. Jira has no conditional delete.
- Uncertain uploads and deletes have no automatic reconciliation command yet; the receipts and journal are retained for manual inspection.
