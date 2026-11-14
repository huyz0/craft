# Internal Jira project discovery

The project-list host composes verified service access, typed Jira reads, the [immutable cursor engine](provider-pagination.md) and exact projected output measurement. Its command is now public with [policy setup and usage](jira-context.md); this document describes its native integration. Live qualification remains separate.

Implementation: [provider](../crates/craft-provider-jira/src/projects.rs), [host](../crates/craft-cli/src/project_list.rs), [transport site constraint](../crates/craft-runtime/src/transport/routing.rs). The [acceptance evidence](evidence/jira-project-list.md) separates owned-server qualification from live support.

## Input and authority

The canonical target is a Jira Cloud `site` resource whose ID equals its Cloud UUID. `ProjectListInput` supports a literal `query` matching key/name and a native `page_size`, default 50 and bounded from one through 100. Output limits, byte budget and fields use the shared selection contract. Native page size remains bound across cursor resumes; output limits can change.

The trusted host supplies `ProjectReadPolicy`: exact target, issued grant, expected active actor and expected Jira site origin. Repository hints and provider responses do not supply that policy. The site origin constrains continuation parsing and is not a credential destination or permission proof. Require a plain HTTPS single-label tenant under `atlassian.net`. Explicit ports, credentials, paths, query strings and fragments are refused, including paths that normalize back to `/`. Custom domains remain unqualified.

Pure preparation rejects invalid targets/input/projections/policy and insufficient paging/cache-recovery output before authentication. Cursor integrity/binding/expiry checks also run before cached acquisition. Credential provisioning remains explicit and separate. Cached execution retains the caller's locked grant and uses invocation-bounded OAuth verification, preserving its issuer, TLS configuration, original freshness origin and fixture provenance.

Every invocation needs verified access for that exact tenant, grant and actor under the current `read:jira-work` alternative. Access is rechecked for buffered output and before publication. The provider still decides project visibility. Seeing a project grants no create/edit/deploy authority, and cached output describes visibility at its original fetch rather than proving current item-specific permissions.

## Native page and continuation

Each native read uses `GET /rest/api/3/project/search` with `action=view`, `orderBy=key`, the bound page size and optional literal query. It requests no expansions and performs no additional total-count call. Default output contains project ID, key and name; project type is selectable. The outer response retains exact target, observation, cache provenance and any credential-cache receipt.

Continuation uses the advertised `nextPage` offset. It never guesses advancement from returned length, `maxResults` or a changing total. Empty or short pages can therefore continue. Returned offset, maximum, record count and within-page unique IDs/keys are checked. A terminal flag and next link cannot contradict each other; absent terminal and next-link metadata is an error. Unexpected extensions are ignored, while malformed supplied known fields fail qualification.

The configured site-link parser applies only to this declared GET project-search route. Exact origin/path, unchanged filters, valid advertised page size, strict percent decoding and duplicate-key checks apply. A returned maximum may be clamped below the requested size; link validation uses that observed maximum, while later authenticated requests keep the original requested size. Only the increasing offset is stored. The actual request is always rebuilt against the original Cloud-ID OAuth gateway. Ordinary next-link rules on other operations and redirect refusal remain intact.

These rules come from the pinned Jira specification and official [project-search API](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-projects/#api-rest-api-3-project-search-get) and [pagination guide](https://developer.atlassian.com/cloud/jira/platform/rest/v3/intro/#pagination). Documentation establishes the intended protocol; live tenant behavior is still unverified.

## Output and recovery

The cursor engine measures the complete projected envelope, including newline, page controls, errors and credential-cache receipts. It persists fetched remainder before returning a successor. A first record that cannot fit returns `ITEM_TOO_LARGE` with its recovery cursor, allowing a larger byte budget without skipping it. Current default schema-bound fields cannot change during resume; changing projection requires a new traversal.

Observation time and normalized page digest survive buffered replay. `cached: true` means no new project page was fetched; normal cached credential/identity verification can still perform an auth read. Consistency remains `live`. Errors before a qualified page contain no observation or successful cursor.

An OAuth/cache renewal is reported independently from a failed project read, with the original request identity. Known credential effects survive output failures through evidence-preserving encoding. Unknown transport outcomes on a GET do not authorize retries or become product mutations. No automatic resource retry, hidden login, arbitrary authenticated URL or raw GraphQL/HTTP surface was added.

The [candidate command](project-list-command.md) now supplies typed arguments, profile/site selection, private configuration checks and complete dispatch behind the existing public activation boundary. Remaining work includes public registration and actual tenant read acceptance, independent review of consequential auth changes and the remaining Jira context/CRUD surface. This integration does not complete the overall cursor/listing or Jira-context tasks.
