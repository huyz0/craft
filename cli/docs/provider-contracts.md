# Inspect the pinned provider contracts

The development inventory maps **99 Jira/Confluence catalog commands to 112 native route variants**. It connects the proposed resource surface to exact API operations, native input/output schemas, OAuth scope alternatives, permission evidence, lifecycle/recovery rules and owning work. All entries retain `implemented: false`, `runtime_available: false` and `live_verified: false`.

This inventory is planning evidence, not a cloud provider registration. The public development executable now has 69 operations, including selected Jira/Confluence commands and local journal inspection. The complete inventory still includes unavailable operations; see the cloud quickstart for current availability. Provider adapters need typed Craft inputs, response projection, fixtures, authorization/reconciliation review and live qualification before activation.

Jira issue creation is public through the journaled host; see [current write usage](jira-issue-writes.md). The inventory remains a planning/source map rather than the authority for current runtime registration. [Comment reads](jira-comments.md) add two compiled operations while their writes remain unfinished.

A candidate [create-metadata reader and field validator](jira-create-metadata.md) now implement bounded internal context reads through declared routes. They are not registered in the main CLI and do not close the catalog's context/auth/live acceptance requirements.

Conditional [parent and assignee qualification](jira-parent-assignee.md) now adds a narrow issue read and a separately scoped internal account helper. The helper is not an advertised catalog/user-administration command, and positive assignability is not general mutation or unassignment authority.

## Check or inspect one command

From `cli/`:

```text
python tools/providers.py check
python tools/providers.py selftest
python tools/providers.py show --command confluence.page.purge
cargo run --quiet --locked --offline -p craft-cli --example provider_routes
```

`check` compares the generated inventory with the authored mapping, researched catalog and exact pinned source bytes. `selftest` runs deliberate negative controls. `show` returns one command, bounded to 32 KiB. The Rust example checks every native method/path/query shape through Craft's actual core route validator. Its minimum response bound tests syntax only; it does not choose an adapter response budget or execute a provider.

The full development gate runs these checks offline. No token, browser login, tenant or network is used. The Python tooling is not part of the Rust executable and is not launched by Craft.

## Source ownership

All planning paths below are relative to `openspec/changes/build-cloud-cli/`:

| Artifact | Owns |
| --- | --- |
| [provider-catalog.json](../openspec/changes/build-cloud-cli/provider-catalog.json) | Authored command variants, tiers, effects, recovery, fixed constraints, conditions and owning tasks |
| [provider-sources.json](../openspec/changes/build-cloud-cli/provider-sources.json) | Source URLs, dates, versions, byte lengths and SHA-256 pins |
| [provider-operations.json](../openspec/changes/build-cloud-cli/provider-operations.json) | Generated source-bound inventory, including native schema and permission references |
| `sources/*.json` | Exact public OpenAPI source bytes retained as evidence |
| [Command catalog](../../docs/plans/cloud-cli-command-catalog.md) | Researched command coverage and priorities |

Development ownership labels live in these planning artifacts. They do not enter runtime commands, comments, public schemas or the production binary.

Each route includes its source operation ID, product-relative path, HTTP method, operation digest and JSON pointers to parameters, request body and responses. Resolve each pointer against the matching source file named by its `spec`; `$ref` closure is checked within that source. These are **native OpenAPI schemas**, not final Craft argument/response schemas or directly executable JSON Schema validators. Craft request schemas continue to come from its typed operation models.

The trusted Confluence v2 mount `/wiki/api/v2` is added explicitly. Jira and Confluence v1 source paths already contain their product prefixes. Vendor `servers` values never select a credential destination. Runtime origins and cloud-ID routing remain owned by the trusted transport/auth contract.

## Observed source pins

Official documentation links these OpenAPI downloads. Retrieved **10 November 2026**; the `_v` query value is not treated as an immutability guarantee. SHA-256 and length identify the exact retained bytes.

| Source | Bytes | SHA-256 |
| --- | --- | --- |
| [Jira v3](https://dac-static.atlassian.com/cloud/jira/platform/swagger-v3.v3.json?_v=1.8516.153) | 2,511,695 | `486fcb9a32822a629631c651a467f25ac42ffa1eda6744544c7394dced08acf3` |
| [Confluence v2](https://dac-static.atlassian.com/cloud/confluence/openapi-v2.v3.json?_v=1.8516.153) | 735,132 | `40535e2a2c01547d53d824d68299098c9073e5bf7ed98a71634706759becfa31` |
| [Confluence v1](https://dac-static.atlassian.com/cloud/confluence/swagger.v3.json?_v=1.8516.153) | 420,370 | `f80279e4beace901a20174044ff3dc1cf047ea9486c3dd6eff57e33e4161bf12` |

The source versions are recorded in the manifest. They are vendor metadata, not a Craft release version. The exact bytes include vendor documentation/examples; no generated client, installer, toolchain or authorization policy was adopted from them. Source retention does not establish redistribution clearance for a published package.

## Preserve consequential distinctions

- **Scopes:** each list is a conjunction; separate alternatives remain separate. Current and Beta labels are preserved. An explicitly empty documented OAuth scope list is retained; missing scope evidence is unresolved. Basic/anonymous source alternatives do not enable those auth modes in Craft. The OpenAPI OAuth scheme does not qualify Craft service-account access or its delegated broker.
- **Permissions:** the matrix points to the source description and retains a bounded permission excerpt with a truncation flag. A scope never proves project/space permission. Unstated permission details remain unresolved.
- **Lifecycle:** Confluence trash pins `purge=false`; permanent purge pins `purge=true`. Both page commands pin `draft=false`, so unpublished draft discard cannot be selected accidentally. Purge needs a trashed target and stronger permission. Jira project deletion pins recycle-bin behavior on its synchronous variant; the asynchronous variant's lifecycle remains unqualified.
- **Recovery:** mutations require reconciliation before replay. This classification identifies required future work; it is not proof that an adapter can safely retry. Async task acceptance does not mean completion. Native redirects are not trusted auth destinations.
- **Tenant conditions:** RBAC versus legacy space creation is selected before sending. Neither a failed nor an unknown create authorizes fallback to another API. Multipart upload/upsert/ID replacement, inline/footer comments, and content badges/review authority remain distinct.

The pinned sources expose gaps that remain visible:

1. Jira's `DELETE /version/{id}` is deprecated. The mapped command uses `POST /version/{id}/removeAndSwap` with explicit replacement semantics.
2. Confluence v2 attachment deletion declares `delete:attachment:confluence` on the operation but omits it from the central OAuth scope dictionary. The matrix preserves the declaration and flags `oauth_scope_definition_incomplete`.
3. Confluence v1 attachment download, page move and page-tree copy omit explicit permission sections. Their variants retain `permission_evidence_unresolved`.
4. Jira asynchronous project deletion does not establish the exact undo/permanent effect in the checked description. Its variant retains `lifecycle_unqualified`.

Every route is already unavailable because implementation, auth and tenant qualification are missing. These additional reasons identify the specific evidence gaps to resolve. They are not inferred support or a substitute for live provider tests.

## Change the inventory intentionally

1. Update the researched catalog or authored mapping for the intended behavior. Keep every declared command covered and retain explicit deferred surfaces.
2. If source bytes change, inspect the official documentation and download separately. Review the changed endpoint/schema/scope/permission data before updating the source file and its manifest pin. The checker never downloads or silently repins a source.
3. Run `python tools/providers.py check` and inspect the change. A mismatch fails rather than accepting a new generated result.
4. Run `python tools/providers.py write` to regenerate atomically against the pins, then run `python tools/verify.py` in the native development environment.

Task closure proves the checked inventory and route-name compatibility. It does not close provider implementation or security/live acceptance. See [observed evidence](evidence/provider-contracts.md).
