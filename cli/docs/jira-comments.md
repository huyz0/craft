# Manage Jira comments

`jira comment get` and `jira comment list` read issue-bound comments through the configured Jira Cloud service profile. Configure the exact site under `policy set --kind jira-read` with explicit `jira.comment.get` and/or `jira.comment.list` allowances. Native OAuth scope `read:jira-work`, identity-probe scopes, Jira Browse permission, issue security and comment visibility still apply.

Policy updates replace the operation list. Retain the earlier operations you still need; all thirteen Jira read allowances can coexist. Comments require record format three; attachment allowances use format four. Historical formats one/two retain their previous operation sets and cannot silently grant comment access. Local setup uses apply and expected-revision comparison and does not establish live eligibility. See [policy setup](policy-setup.md).

Use observed numeric issue and comment identities:

```powershell
craft --profile jira-test jira comment list --issue-id 10000 --limit 10 --fields id,updated
craft --profile jira-test jira comment get --issue-id 10000 --comment-id 10001 --fields body,author_account_id
```

Metadata defaults omit body and optional author fields. Output always retains comment/issue identity, native visibility, service-desk privacy flags, body state and available revision fingerprint. Group restrictions preserve their stable identifier and observed name; native role identifiers are role names. A missing visibility object does not establish anonymous access or override issue security and service-desk privacy.

`body_state` is `not_requested`, `not_returned` or `returned`. Selected body is inert bounded ADF JSON, preserving its content rather than interpreting instructions, links or extensions. Emails, display names, returned endpoint URLs and unrelated native properties are not normalized into output. Missing native account identity is not inferred from display text. Each body is bounded to 256 KiB and the native response to 4 MiB; malformed/oversized data fails without returning a truncated document.

`revision` fingerprints available native body, identity, relevant privacy, author and time evidence. Its value is independent of output body selection. It is not a Jira atomic version, write authority or verified human approval. A missing body leaves the fingerprint unavailable.

List separates native `--page-size` (1–100, default 25) from output `--limit` (1–100, default 20). One invocation fetches at most one native comment page. Native created ordering, advertised offsets/size/total, clamping and duplicate identities are validated. Empty or short windows may continue. Returned URLs never supply a request destination.

Continue using the returned opaque cursor with unchanged issue, native page size, selected fields, site, profile and state root. Buffered remainders avoid another comment GET and recheck current local policy, actor/grant/site and token access. `cached` and the original observation describe data fetched earlier; they do not prove current native group membership, collection stability or absence of invisible comments. Changing the projection cannot reuse the cursor.

404 and denial are errors, not an empty complete result. Reads do not retry automatically. Late policy refusal suppresses content/continuation while retaining credential-cache effects. [Acceptance evidence](evidence/jira-comment-reads.md) records the actual local checks.

## Create, update and delete

Writes use an explicit service profile, exact numeric project and issue IDs, `apply: true`, a stable operation ID and `--max-bytes 8388608` to reserve complete recovery evidence. Normal output omits ADF bodies and retains fingerprints, privacy metadata, confirmed identities, credential effects and journal dispositions. The reservation is an output ceiling, not padding.

Configure separate project `jira-mutation` allowances: `jira.comment.create`, `jira.comment.update-own`, `jira.comment.update-any`, `jira.comment.delete-own` and/or `jira.comment.delete-any`. These require format five. Formats one through four grant none of these operations; existing issue/link allowances keep their earlier meanings. Up to sixteen mutation allowances can coexist. Policy updates replace the complete list and require its observed revision. Native `read:jira-work`, `write:jira-work`, identity scope, Browse projects and issue security still apply.

Create input, saved as an operator-provided `comment-create.json`:

```json
{
  "operation_id": "plan-review-comment",
  "apply": true,
  "project_id": "10000",
  "issue_id": "10001",
  "body": {
    "type": "doc",
    "version": 1,
    "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Review complete."}]}]
  },
  "visibility": {"mode": "project"}
}
```

```powershell
craft --profile jira-test --max-bytes 8388608 jira comment create --input comment-create.json
```

`visibility` explicitly selects `{"mode":"project"}`, `{"mode":"group","identifier":"stable-group-id"}` or `{"mode":"role","identifier":"Developers"}`. Project mode removes the comment-specific restriction; issue security and service-desk privacy remain separate. Group identifiers are stable IDs; roles use names. Child comments inherit parent visibility, so Jira can refuse changing a child restriction.

Updates require issue/comment/project IDs and `expected_revision` from a fresh comment get. Include `body`, `visibility` or both. Omission preserves the corresponding field; JSON null is refused. Use project visibility mode to clear a restriction. Deletion requires the same identities/fingerprint and permanently deletes the addressed comment.

`ownership` defaults to `own`. Native Edit own/Delete own or native all permission can satisfy own intent only when the original author is the authenticated actor. The last editor does not establish ownership. `ownership: "any"` requires the separate local any allowance and native Edit all/Delete all, even when the observed author is the actor. Missing author evidence cannot satisfy own intent. Native Add comments is required for create.

ADF authoring supports paragraphs, headings, quotes, bullet/ordered lists, code blocks, rules, text, hard breaks and strong/emphasis/code/strike/underline/HTTP-link marks. Unsupported nodes or attributes fail locally rather than being dropped. Bodies have a 256 KiB encoded limit, with depth/node/text limits. Reads can preserve richer native ADF inertly; a write does not silently flatten it.

Project, comment fingerprint and permission observations bracket preparation. `atomic_revision_check: false` remains explicit: Jira has no documented comment compare-and-swap, so a concurrent edit can occur after checking. One frozen native request is durably claimed and never retried automatically. A documented acknowledgment can retain the addressed identity while `desired_fields_verified: false` and an error report incomplete returned fields. `acknowledged` describes this invocation's HTTP evidence; a recovered journal receipt alone does not create it.

On any failure, retain the full envelope and operation ID. `operation get` inspects local journal evidence; do not submit a fresh ID to bypass a claimed attempt. There is no comment-specific live reconciliation/adoption command yet. A lost create response can leave no known comment ID; absence or matching text cannot establish attribution or replay authority. See [write evidence](evidence/jira-comment-mutations.md) and the [official API](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-comments/). Live tenant behavior remains unqualified.
