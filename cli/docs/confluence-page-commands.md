# Page-write commands and saved permissions

`confluence.page.create` and `confluence.page.update` are public commands that connect strict flags or JSON input to the journaled publication host. Both have passed through the full host against the live test site (see [live evidence](evidence/live-read-conformance.md)); editor-draft interaction with another person's open draft is not live-qualified.

## Select context and provide intent

Normal explicit/repository/active profile selection chooses one Confluence Cloud service profile. Cloud ID and space ID are optional when trusted profile context supplies them. An explicit space overrides a default, but it must match an enabled saved write record for that exact space. A read record grants no create or update permission. Arguments cannot supply service actor, credential grant, raw URL, private/draft/live modes or force-overwrite settings.

A create candidate accepts this structured argument object:

```json
{
  "operation_id": "publish-plan",
  "apply": true,
  "space_id": "100",
  "title": "Plan draft",
  "placement": {"kind": "root"},
  "body": {"representation": "storage", "value": "<p>Review this plan together.</p>"}
}
```

The corresponding flags include `--operation-id`, `--apply`, `--space-id`, `--title`, `--placement` and `--body`. Complex flag values are strict JSON objects; duplicate/unknown fields fail. Root is the flag placement default. Homepage placement resolves the observed space homepage, and explicit parent placement requires an ID. Both verify the current parent belongs to the selected space. A plan draft remains an ordinary published shared page.

Update arguments contain `id`, `expected` and `changes`, plus the same operation/apply/context options. Changes contain title and/or body. Omission preserves the current value; null is rejected. For example, a title change uses `"changes": {"title": "Reviewed plan"}`. The expected revision contains version, digest and representation. Both version and fingerprint must match fresh published content before a PUT is prepared. Updates also require `"editor_draft_policy": "native_reconciliation_may_overwrite"` (flag `--editor-draft-policy`). Confluence reconciles a published update into any open editor draft and can overwrite divergent unpublished edits; there is no preservation mode. The policy is bound into the operation digest, never sent to Confluence, and grants no permission. Missing, null or other values fail before state or network access. Every update receipt repeats the policy with `editor_draft_preservation_verified: false`. Coordinate editing outside Craft if unpublished drafts must survive.

## Fetch only the expected revision

Page get now supports selecting `revision`. This performs one current-page GET with storage content internally and returns the fingerprint without returning the body. Selecting an ADF body plus revision uses that exact representation instead. A revision covers page ID, space ID, title, published version and the exact body representation/value. It is a write precondition, not human approval.

A revision-only response keeps body_state `not_requested` because the body is not returned to the caller. Missing required body/metadata fails without inventing a digest. Lists and search reject revision selection; their results are not complete publication snapshots. Existing metadata/body reads behave as before when revision is not selected.

## Configure write authority

`ProfileStore::set_confluence_write_policy` stores non-secret per-profile/per-space authority. The format-one record binds installation, profile, its actual native grant, numeric space, expected service actor, enabled state, revision and explicit create/update allowances. It requires apply, receipt-output preflight and exact revision CAS. Disable an enabled record before actor/target transfer. Saved read/Jira permissions cannot grant page writes, including when profiles share a credential grant.

Schema eleven atomically adds an empty write table and preserves existing policy payloads, profiles, credentials, journals and buffered cursors. Malformed/foreign records, conflicting tables and triggers fail without repair. Candidate execution captures the entire record and rechecks it before authentication, around reads and after the committed send claim. A changed enabled revision also stops execution.

## Preserve complete outcomes

Candidate response schemas advertise the complete publication host receipt, including known resources, local persistence/journal evidence, HTTP status/request ID/retry delay, separate credential-cache receipts and replay_authorized false. Local input, apply and receipt-capacity checks precede private state or authentication. Dispatch preserves the original request identity and the complete handler envelope.

See [publication behavior](confluence-publication.md), [observed command evidence](evidence/confluence-write-command.md) and [trash, comment, badge and attachment writes](confluence-space-writes.md).
