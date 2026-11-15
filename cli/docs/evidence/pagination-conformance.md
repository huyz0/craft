# Paging conformance suite

Date: **15 November 2026**, Australia/Sydney. Windows x86_64 MSVC, pinned Rust 1.99.1. The [design section](../../openspec/changes/build-cloud-cli/design.md#pagination-conformance-suite) and the agent-interface scenario "Shared paging contract" are the contract.

## What changed

- `crates/craft-test-support/src/paging.rs` provides synthetic native list services in the three provider styles (opaque token, numeric offset, next link with an opaque cursor) with scripted faults: rate limit, cancellation, failure, empty nonterminal page, repeated position, insertion and deletion between pages. The service serves the dataset as it is at fetch time, so live changes produce real duplicates and gaps.
- `crates/craft-test-support/src/paging/contract.rs` is the reusable contract. A subject supplies a page source over a dataset, its binding and a changed binding, expected IDs and an identity function; the runtime supplies the real cursor engine (`ProfileStore::provider_page`) through the `Pager` trait. The contract observes only emitted IDs, page metadata and native fetch counts.
- `crates/craft-runtime/tests/paging_conformance.rs` runs the contract against the three synthetic styles and five real adapters. Each real adapter reads simulated native wire responses through its own `read_page`, and continuation links are normalized the way the host does: only the link's query values for the same route. The same subjects then run through the scan driver.

| Subject | Style | Adapter |
| --- | --- | --- |
| Jira issue search | token (`nextPageToken` in the POST body) | `issue_reads::read_search_page` |
| Jira project search | offset with an absolute `nextPage` link | `projects::read_page` |
| Jira comment list | offset with `total` | `comments::read_page` |
| Confluence page list | relative `_links.next` with a cursor | `read_page` for `confluence.page.list` |
| Confluence space list | relative `_links.next` with a cursor | `read_page` for `confluence.space.list` |

## Cases

For every subject:

1. Lossless traversal of 0, 1, 5, 11 and 12 records with page sizes 4–5 and limits 1, 3, 5 and 100: emitted IDs equal the dataset in order, never more than one native fetch per invocation, exactly one fetch per native page, the last page stops as `exhausted`, `has_more` agrees with the cursor, `returned` equals the items, and consistency is always `live`.
2. Mid-page byte truncation: a budget that fits about three records of a ten-record page still emits all 30 IDs in three fetches, with at least one `byte_limit` stop.
3. Buffered replay: the same cursor twice returns the same records and the original observation with `cached: true` and no fetch.
4. Changed binding: the same cursor under changed semantics is `CURSOR_MISMATCH` with no fetch.
5. Oversized record: at the smallest usable budget (the fixed recovery envelope plus the page context) no record larger than the budget is returned; when one is, the `ITEM_TOO_LARGE` recovery cursor resumes at it.
6. Repeated position: stops as `PAGINATION_STALLED` for every subject whose wire can express it. Jira comment offsets are derived from offset plus page size, so a repeat cannot be expressed there.
7. Live insertion and deletion before the second page: traversal completes, labelled live, with one duplicate after an insertion and one missing record after a deletion. These are reported as declared limitations, not failures.

A separate case shows a rate-limited Confluence page fetch publishes no successor and leaves the predecessor cursor usable for the same position.

## Checks

| Check | Result |
| --- | --- |
| `cargo test -p craft-runtime --test paging_conformance` | 2 passed: eight subjects, all three styles, every case above, plus scan completion per subject |

Envelope response schemas are validated by the host tests that wrap these adapters (`jira_issue_reads_host`, `jira_project_list_host`, `confluence_context_host`), not by this suite.

## Not covered

- Jira field search, Confluence CQL search, Confluence footer comments and Confluence attachment list have native continuation but are not registered with the contract yet; their own adapter tests remain their evidence.
- Field masks and local filtering are exercised by projection in the host tests, not by this suite.
- Engine-level `ITEM_TOO_LARGE` and `OUTPUT_BUDGET_TOO_SMALL` cases remain in `crates/craft-runtime/tests/provider_pages.rs`; the suite's small records rarely exceed the recovery envelope.
