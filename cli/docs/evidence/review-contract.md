# Internal review evidence-port verification

Later native integration: [complete inventories](comment-inventory.md) and [live binding/context preparation](review-preparation.md). The evidence below describes the original verifier checkpoint; public approval verification remains unfinished.

Implemented on 10 November 2026 in `crates/craft-workflows/src/review.rs`, with focused tests in `review/tests.rs`. No public command, native credential access or ticket authority is enabled. The [contract](../review-contract.md) states policy/source provenance requirements and unfinished native integration.

## Observed tests

`target/review-contract-boundaries.log` passes nine focused tests. They cover exact plan/design/Jira mapping binding, current version actors, distinct quorum, worker/unlisted identities, duplicate actors/comments, unresolved and stale requests, withdrawal, strict visible JSON, unknown/duplicate members, marked/ambiguous content, missing metadata, non-current/wrong-page comments, output-independent complete coverage, all source/policy/comment drift paths, final-read-only drift, cancellation and both clock rollback/expiry paths. Exact count, per-body and total-inventory boundaries pass at their limits and refuse excess. Revalidation preserves original approval revisions even when a replacement quorum exists; extra matching approvals are accepted when originals remain intact.

Earlier focused runs are retained in `target/review-contract-tests.log`, `target/review-contract-tests-final.log` and `target/review-contract-focused.log`. Source refinements added authenticated execution-actor checks, compact binding-digest statements and explicit current status before the final boundary run. A compile-fail doctest also guards against restoring `VerifiedReview` through JSON deserialization; its actual run belongs to final package qualification.

## Independent review

Authorized reviewer `/root/activation_review` found no concrete correctness or authorization blocker in the internal contract. Review covered exact bindings, trusted policy, structured decisions, complete inventory, quorum, withdrawals/unresolved comments, repeated observations, dual-clock freshness and original approval revalidation. The reviewer identified boundary/duplicate/format/final-read test gaps; these were added and all nine focused tests pass. Review was read-only, with no tests, state, secrets or tenant access.

The same bounded review confirmed that workflow changes must select public contract checks because workflow models contribute to CLI schemas. `tools/verification_scope.py` now includes those owner checks and a regression assertion; unrelated native hosts remain outside that focused scope.

## Limits

Tests use in-memory evidence ports and declared fixture identities. They do not prove native identity provenance, real human eligibility, complete Confluence traversal, persisted operator policy or a working materialization host. Future native consumers must enforce live revalidation and preserve all cache/write receipts. No new dependency, lockfile, private database schema, credential format or public contract was introduced. Owning authority/revision tasks remain unchecked.

The initial seven-check selected qualification passed in `target/review-contract-verification.log`, including the non-deserialization compile-fail doctest. A subsequent failure-first check exposed disabled-policy ordering: one identity, binding and comment read occurred before refusal. `target/review-disabled-policy-regression.log` retains that failure. The verifier now refuses immediately after policy loading; the final selected qualification must observe zero identity/resource reads for disabled policy.

## Final selected qualification

The ordering fix passes the final six-check selected run in `target/review-contract-verification-final.log`: planning, public contract comparison/negative controls, formatting, workflow and CLI tests, the non-deserialization compile-fail doctest and all-target Clippy. All ten review tests pass; the workflow package reports 26 passing tests and one unrelated opt-in ignored. The disabled-policy regression now observes zero identity, source and inventory reads. Independent read-only follow-up confirmed the early refusal and the port's provenance/budget/receipt responsibilities.

The default offline build passes in `target/review-contract-build.log`. Actual discovery still has 37 public commands, and `schema --command plan.review.verify` returns `COMMAND_UNKNOWN`. The public contract hash is unchanged from the prior comment checkpoint: `f3679451492ed3bf23bdbfef127c06476d208cab0e977afc67317fda858d7295`. Binary SHA-256: `5432d0b7d3c6985f5d99c17406f455de6182ef8bf5877accee2b0ebff78445bf`. Machine-readable identity, discovery and qualified inputs are retained in `target/review-contract-result.json`, `target/review-contract-discovery.json` and `target/review-contract-qualified-source-hashes.json`. This is focused internal qualification, not a live review pass. Final evidence-only validation is recorded in `target/review-contract-documentation.log`.
