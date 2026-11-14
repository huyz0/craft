# Current page-bound footer-comment observation

Later additions: [paged roots and replies](comment-paging.md) and [trusted reviewer configuration](reviewer-policy.md). The qualification and limits below describe this earlier GET-only checkpoint.

Implemented on 10 November 2026. Public `confluence comment get` requires exact comment and expected page IDs, a separate saved read allowance and granular comment scope. It uses the existing verified read host, actor/grant/tenant proof, policy rechecks and bounded credential-cache receipts. No approval or human eligibility is inferred.

## Observed acceptance

- `target/comment-observation-provider.log`: a focused provider test passes exact route/query and current metadata/body binding, fingerprint changes after body/author/version edits, wrong identity/page/history/container refusals, missing metadata/body refusals, metadata-only output and incompatible body projection refusal.
- `target/comment-observation-host.log`: seven owned HTTPS modes pass storage body, revision-only, ADF, wrong page, missing author, old policy and insufficient scope. Old policies refuse before acquisition/resource access. Renewal receipts remain confirmed through success and remote-data refusal; captured product requests are GET only. These are synthetic credentials and owned local servers.
- Public contract export contains 37 operations in `target/comment-observation-contracts-final.log`. Actual execution still uses the verified host rather than the generic registry handler. Public interface tests include missing/invalid expected-page refusal before state access.

## Independent review

Authorized reviewer `/root/activation_review` found no concrete blocker in separate allowance/old-policy compatibility, granular scope, fixed current-version footer route, page/container/status checks, version metadata, projection or inherited receipt handling. Inserting the new operation preserves ordering of old enum variants and does not grant access to existing policy rows. No tests, state, secrets or tenant access occurred during review.

The pinned OpenAPI and [current official API documentation](https://developer.atlassian.com/cloud/confluence/rest/v2/api-group-comment/#api-footer-comments-comment-id-get) were inspected. Version-author metadata describes the author of that version; it is not original-author or human-eligibility proof. No account type, reviewer identity or live permission was invented.

## Limits

Comment CRUD/list/reply traversal, original-author/history evidence, trusted reviewer storage and exact-revision approval verification remain incomplete. Live tenant comment behavior is unqualified. Private schema eleven, policy format one, dependencies, lockfile and credential formats remain unchanged. The owning comment task remains unchecked.

## Final local qualification

`python tools/verify.py` passed all fourteen checks in `target/comment-observation-verification.log`: planning, dependency/layer inventory, provider comparison/selftests/routes, formatting, generated contracts and negative controls, workspace tests, all nine composed hosts, feature runtime tests, both Clippy modes and synthetic-feature isolation. Runtime authorization changes selected this full local gate. Native opt-ins and live provider qualification remain separate and were not run.

The default offline workspace build passed in `target/comment-observation-build.log`. Actual executable discovery contains 37 commands in `target/comment-observation-discovery.json`. Binary SHA-256: `97c997fbfb8cf0855764032e486d08ff98af84b308e73e07f7e1aaa6b9aea5cf`. Contract SHA-256: `f3679451492ed3bf23bdbfef127c06476d208cab0e977afc67317fda858d7295`. Machine-readable identity and qualified inputs are retained in `target/comment-observation-result.json` and `target/comment-observation-qualified-source-hashes.json`. The final evidence-only check is recorded in `target/comment-observation-documentation.log`.

A final independent read-only follow-up confirmed the native `comment-id` parameter, exported comment-specific projection/schema helpers and revision schema constraints, with no new blocker.
