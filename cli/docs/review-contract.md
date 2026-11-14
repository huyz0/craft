# Internal review verification contract

The review verifier in `craft-workflows::review` is implemented over a host-owned evidence port. Native [reviewer policy storage/administration](reviewer-policy.md) is implemented, with a full-record enabled-policy bridge. The CLI host has a complete native comment collector in `confluence_reads::inventory`, and [review preparation](review-preparation.md) now creates live source/design/target bindings and immutable context. [Public verification](review-verification.md) now composes these reads and records immutable local approval evidence. No ticket path consumes this type yet.

## Authority and source binding

Operator-managed policy names eligible human account IDs, excluded worker identities, distinct-reviewer quorum, enabled state, exact Confluence space and policy revision. Its constructor rejects duplicates, overlap, unknown identities and the configured execution service actor as a reviewer. The verifier checks the authenticated execution actor against that policy before and after observation. Account type or approval-looking text cannot establish human eligibility.

The host must read the current visible plan, every referenced design revision and the complete resolved Jira field/target mapping. `ReviewBinding::from_plan` validates the plan, requires a mapping for exactly the included tasks' project keys, checks project/cloud identities, computes the semantic plan and mapping digests, and retains the exact source page/version/representation fingerprint and sorted design references. The native adapter must establish that these values came from current authenticated reads; the constructor itself performs no network verification.

A human decision is a compact visible JSON statement containing `format: craft.review/v1`, `decision`, `plan_id`, `page` and `binding_digest`. The digest binds the complete review snapshot, including design and target/field mappings, without copying that snapshot into every comment. The statement contains no author or eligibility claim. Identity comes from the observed current comment version.

Only one unmarked paragraph or code block containing strict JSON is accepted. Duplicate members, unknown fields, unsupported/marked content and extra visible blocks cannot count as approval. An eligible reviewer's unstructured comment remains unresolved. Native host integration must present the full plan and resolved mapping alongside the statement before human review; a digest alone is not a review interface.

## Decisions and rechecks

- `approve` counts only for the exact current binding. Multiple comments by the same actor contribute one quorum vote.
- `changes_requested` blocks the same plan/page until the current request is explicitly withdrawn or edited into a new decision. A source edit does not silently resolve an earlier request.
- `withdraw` removes that actor's approvals for the same plan/page. Timestamps do not infer which conflicting decision wins.
- Ineligible and excluded actors supply no votes. Missing identities, non-current records, wrong-page comments or incomplete inventory cannot establish review.

Disabled policy is refused immediately after the local policy read, before execution-identity or resource reads. The verifier reads trusted policy and execution identity, the live binding and a complete current root/reply inventory. It then rechecks the binding, reads a second complete inventory, and rechecks policy, binding and execution identity. Inventories are compared independently of enumeration order. Duplicate IDs, unreadable gaps, pending cursors and partial/unknown coverage are refused. Bounds are 1,024 comments, 65,536 encoded bytes per body and 8 MiB total body data. Wall and monotonic elapsed time must both remain below 30 seconds without moving backward; invocation cancellation/deadlines still apply.

`VerifiedReview` has private fields and no deserializer. A serialized receipt cannot restore authority. Before each materialization batch, the future native consumer must call `revalidate`, which repeats observation and retains the original approving comment IDs, versions, actors and content fingerprints. New approvals cannot substitute for a deleted or edited original. These are bounded observations, not an atomic lock across cloud services; final native send-boundary checks remain required.

## Remaining integration

Authenticated complete root/reply traversal is implemented as an internal host component. Live design/mapping binding, durable source context and human-facing statement preparation are available through `review prepare`. Public verification and durable local approval receipts are available through `review verify`; [ticket preview](ticket-preview.md) additionally retains complete approved issue/link manifests. Shared cloud artifact publication and ticket application/recovery remain unfinished. Native adapters must retain credential-cache receipts even when a review check fails. Pure verifier tests do not prove an actual human review. [Inventory evidence](evidence/comment-inventory.md) and [preparation evidence](evidence/review-preparation.md) record native qualification and limits.

See [acceptance evidence](evidence/review-contract.md).

## Native inventory read design

The inventory dependencies are separate paged reads: `confluence.comment.list` for current page footer roots and `confluence.comment.replies` for an exact parent and page. Both require explicit read-policy allowances; comment-get authority does not imply list authority. Public selection uses the existing bounded list/cursor host and projected comment schema. Neither command alone asserts a complete review inventory.

Native root pages use `/wiki/api/v2/pages/{page-id}/footer-comments`. Replies use `/wiki/api/v2/footer-comments/{comment-id}/children`. The pinned child model has no page ID. A replies read therefore proves its parent on the requested page using current comment GET before and after fetching children, and checks each returned child's exact parent and current version. The two parent observations must match. Page identity is inherited only from this authenticated parent proof, never supplied as a substitute for contradictory native data. Unavailable provenance refuses the page.

The transport preserves bounded Link-header observations separately from bodies. Unknown, malformed, oversized or conflicting continuation metadata cannot establish terminal coverage. Header-only and body-only next links pass through fixed-route normalization; if both are present their normalized cursors must agree. No link supplies a credential origin. Missing next in both observed locations establishes only that native collection's terminal page. The complete-inventory collector visits all roots and replies, detects cycles/duplicates and retains receipts. Its consumer must still satisfy the verifier's two-pass/source-binding contract.

The complete-inventory host captures the native read policy from an explicitly named service profile and requires both list/replies operations before authentication. It opens sealed access once, checks captured policy/actor/grant/scope at native boundaries, walks all root pages, then breadth-first visits every observed comment's direct replies, including leaves. Per-collection cursor history and global identity history detect repeated continuations, cycles and duplicate cross-branch membership. Only exhausting the queue and every continuation can return complete coverage. No public JSON input can restore inventory authority.

The collector shares the verifier's comment/body limits, caps native collection pages at 2,048 and rejects elapsed or backward wall/monotonic observations. The caller's invocation deadline bounds network I/O; late results cannot pass the 30-second freshness check. Credential-cache receipts remain separate from the all-or-error inventory result. This collector is one observation pass; the review verifier still requires two passes and fresh source/design/target binding.

Complete coverage describes the exhausted authenticated traversal, not an atomic cloud snapshot or approval. The collector supplies one observation to `review verify`, which records the complete local audit result.

[Public verification evidence](evidence/review-verification.md) records native integration, stale-list refusal and current qualification.
