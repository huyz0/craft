# Read page-bound footer comments

`confluence comment get` reads one current footer comment with an explicit expected page ID. It requires a separate saved `confluence.comment.get` allowance in the `confluence-read` policy and the granular comment-read OAuth scope. Existing page allowances do not grant comment access. Use [policy setup](policy-setup.md) to add the exact operation with revision CAS.

```text
craft --profile cf-test confluence comment get --id 201 --page-id 101
craft --profile cf-test confluence comment get --id 201 --page-id 101 --fields body_storage,revision
craft --profile cf-test confluence comment get --id 201 --page-id 101 --fields body_atlas_doc_format,revision
craft --profile cf-test confluence comment get --id 201 --page-id 101 --fields revision,parent_comment_id
```

Default output includes comment/page identity, current status, version number, `version_author_id`, `version_created_at` and body state. The author identifies the current version, not necessarily the original comment creator. This observation does not prove that the account is human or eligible to approve.

Select one body representation. Selecting `revision` alone fetches storage for fingerprinting without returning it. The digest binds the comment/page/parent, current status, version, version author/time, representation and body. Body or version edits change the fingerprint. A null selected parent identifies a root comment; a returned parent ID identifies a reply.

The read refuses mismatched page/comment IDs, alternate containers, non-current status and missing version-author/time metadata. Requested body/fingerprint output requires the exact body representation. Historical versions are unavailable through this command. Content links cannot change routing. Reads make no resource write and do not retry automatically; failures retain credential-cache renewal evidence when present.

## List roots and replies

`confluence comment list` reads current footer roots for a page. `confluence comment replies` reads direct children for an exact page and parent. Each needs its own saved operation allowance; a GET-only policy grants neither. All use the granular `read:comment:confluence` OAuth scope and existing page/space visibility permissions.

```text
craft --profile cf-test confluence comment list --page-id 101 --limit 20
craft --profile cf-test confluence comment replies --page-id 101 --parent-comment-id 201 --limit 20 --fields body_storage,revision
craft --profile cf-test confluence comment list --page-id 101 --limit 20 --cursor <returned-cur-handle>
```

`--page-size` controls native fetch size (1–100, default 25); `--limit` bounds this output (1–100, default 20). Reuse the returned opaque cursor with the same profile, selectors and fields. Do not pass native provider cursors or invent offsets. Metadata is the default; select a body only when needed. List output always includes the parent ID or null. Unknown/malformed/conflicting continuation evidence refuses the read rather than claiming the collection ended.

The native child model omits page identity. Reply reads prove the current parent on the requested page before and after fetching children, require matching parent observations, and check each child's exact parent. Contradictory native page/container metadata is refused.

These commands return bounded collection pages. An internal native [inventory collector](evidence/comment-inventory.md) now exhausts roots and every observed comment's replies with duplicate/cycle/freshness checks. It is not a public review-verification command. [Reviewer configuration](reviewer-policy.md) is available, while comment mutations, live identity/site qualification, public approval verification and ticket materialization remain unfinished. See [collection acceptance evidence](evidence/comment-paging.md).

Provider source: [Atlassian footer-comment API](https://developer.atlassian.com/cloud/confluence/rest/v2/api-group-comment/#api-footer-comments-comment-id-get), checked 10 November 2026, and the pinned Confluence v2 OpenAPI fixture. The API documents current version metadata and optional body selection. [Acceptance evidence](evidence/comment-observation.md) distinguishes local qualification from live tenant behavior.
