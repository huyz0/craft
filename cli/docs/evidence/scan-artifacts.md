# Bounded scans

Date: **15 November 2026**, Australia/Sydney. Windows x86_64 MSVC, pinned Rust 1.99.1. The [design section](../../openspec/changes/build-cloud-cli/design.md#bounded-scans) and the agent-interface scenarios "Bounded scan into artifacts" and "Exporting a scan" are the contract.

## What changed

- `crates/craft-runtime/src/scan.rs` is a provider-neutral driver over any `PageSource`. One invocation advances from an immutable checkpoint until a cap, terminal provider metadata or a failure, then publishes one JSON Lines segment followed by the checkpoint that names it. A failed publication leaves the previous checkpoint as the resumption point, and an orphan segment is never referenced.
- Caps are totals for the scan: 1–100,000 records, 1–10,000 native pages, 1 KiB–64 MiB stored bytes, and the invocation deadline with three seconds kept free to publish. Resume may raise them.
- Outcomes are `complete` (only after terminal metadata), `budget_reached` with the limit that stopped it, `rate_limited` with the adapter's last Retry-After advice, `cancelled` and `error`. Checkpoint validation refuses a non-complete checkpoint that claims `complete`, totals that disagree with segment receipts, unordered or foreign segment names and oversized state.
- A position already requested in this scan stops as `PAGINATION_STALLED` before it is fetched again. Eight consecutive empty pages that still advertise continuation also stop as stalled. Records fetched but not stored because of a cap stay in the checkpoint's remainder and are written before the next fetch, so nothing is skipped. A first record larger than the stored-byte cap is `ITEM_TOO_LARGE`, not a skip.
- Repeated IDs are counted in `duplicate_ids` and kept. Every result says `consistency: live`.
- `scan start`, `scan resume` and `scan export` are public (`crates/craft-cli/src/scans.rs`, `scans/command.rs`). The scannable commands are `jira.issue.search`, `jira.project.list` and `confluence.page.list`. Each host keeps its own profile resolution, private read policy, binding, access rechecks and field projection; it lends the driver its page source through `scan_cached`, and an `AdviceRecorder` keeps provider rate advice from the last native response. The list arguments must not carry `limit` or `cursor`; `--record-fields` takes the list command's own field names.
- Resume loads the checkpoint by exact name and digest before any profile or credential access, rebuilds the list binding from the stored arguments and current policy, and refuses any binding change before a fetch.
- A cancelled invocation publishes what it already fetched once, under a fresh five-second control, as journal receipts do after cancellation.
- `scan export` verifies every segment's digest and size, assembles one `.export.jsonl` artifact and exports it through the existing guarded create-new output path. A non-complete scan needs `--allow-partial`, and its receipt keeps `complete: false`. Records never appear on stdout.
- `artifact get` and `artifact export` accept `--collection scans`.

## Checks

| Check | Result |
| --- | --- |
| `cargo test -p craft-runtime --lib scan::` | 7 passed (below) |
| `cargo test -p craft-cli --test jira_issue_reads_host --features synthetic-acceptance scans` | passed: issue search through the real search adapter and TLS transport |
| `cargo test -p craft-cli --test jira_project_list_host --features synthetic-acceptance project_scan` | passed: project search through site `nextPage` links |
| `cargo test -p craft-cli --test confluence_context_host --features synthetic-acceptance page_list_scan` | passed: Confluence v2 page list through `_links.next` |
| `cargo test -p craft-cli --test local_artifacts scan_checkpoints` | passed: export and refusal paths from a fresh process |
| `cargo test -p craft-runtime --test paging_conformance` | each conformance subject also scans 13 records to completion in three fetches |

| Case | Observation |
| --- | --- |
| `every_paging_style_scans_to_completion_with_verified_segments` | Token, offset and next-link: 30 records in five fetches, one segment, checkpoint reloads equal by digest, assembled export lists every ID once; a complete scan refuses another advance without fetching |
| `caps_stop_mid_page_and_resume_without_refetching_the_remainder` | Item cap 4 of a 10-record page keeps six in the remainder; resume finishes in two more fetches with all 25 IDs; a 1 KiB byte cap keeps unwritten records buffered |
| `rate_limit_and_cancellation_keep_a_resumable_checkpoint` | 429, cancellation and a provider failure on the second page each publish the first five records and a checkpoint; resume from it completes with all 12 IDs in two segments; rate advice is reported |
| `repeated_or_empty_continuation_stops_as_stalled_without_looping` | A repeated position stops after two fetches with three records kept; eight empty nonterminal pages stop as stalled; two empty pages are tolerated |
| `live_changes_are_counted_or_missed_but_never_called_a_snapshot` | An insertion counts one duplicate and keeps it; a deletion silently misses one record, which the result cannot detect |
| `changed_binding_or_tampered_checkpoint_refuses_before_any_fetch` | Changed query is `CURSOR_MISMATCH` with no fetch; wrong digest is refused; tampered totals and a partial scan claiming completion fail validation |
| `an_oversized_first_record_is_an_error_not_a_skip` | `ITEM_TOO_LARGE` with zero records written and all three kept in the remainder |
| `issue_search_scan_follows_tokens_to_completion_and_keeps_rate_advice` | First page, then 429 with `Retry-After: 90` reported as `delay_seconds: 90`; resume repeats the same `nextPageToken` and completes; stored records carry only the selected projection |
| `project_scan_follows_site_links_through_empty_pages_to_completion` | Three project searches on the bound gateway (`startAt` 0, 50, 75) through an empty page; four records |
| `page_list_scan_follows_bound_next_links_and_keeps_rate_advice` | Second request carries the bound cursor; 429 with `Retry-After: 45` keeps two records and the continuation |
| `scan_checkpoints_export_only_with_their_digest_and_never_claim_a_partial_is_complete` | Wrong digest is `CURSOR_MISMATCH`; partial export is refused without `--allow-partial` and writes nothing; with it, three lines and `complete: false`; a second export to the same file is refused; segments are inspectable with `artifact get --collection scans`; resume with a wrong digest fails before profile access; non-scannable commands, a supplied cursor, unknown argument fields, zero caps and `--fields` are refused |

Gate on the final source: `python tools/verify.py` passed planning, skill, contract, provider and dependency checks, formatting, both Clippy modes and `cargo test --workspace` (814 tests). Its composed-host stage then failed 16 Confluence review-verification cases at default parallelism, the known deadline pressure recorded in STATE.md; `confluence_context_host` passed all 69 with `--test-threads=4`, and the other eight host suites, the runtime synthetic-acceptance library tests and `tools/synthetic-acceptance.py` all passed.

## Live

On 15 November 2026 `python tools/live_host.py scans` ran read-only against the test site through the CraftTest service profiles, with one-record native pages and an item cap of two:

| Command | Start | Resume | Export |
| --- | --- | --- | --- |
| `jira.issue.search` | `budget_reached` (items) after 3 native pages, 2 records | `complete`, 4 records, 5 pages including one empty final page | 4 lines, `complete: true` |
| `jira.project.list` | `complete`, 1 record (the site has one project) | refused `VERSION_CONFLICT`: already complete | 1 line |
| `confluence.page.list` | `budget_reached` (items), 2 records | `complete`, 6 records over 6 cursor pages | 6 lines |

Resume with a wrong checkpoint digest returned `CURSOR_MISMATCH` for all three. No duplicates were observed. The project scan did not cross a live `nextPage` link because the site has a single project; link following is covered by the host fixture.

## Not covered

- Other list commands are not scannable yet.
- Cancellation between a segment's publication and its checkpoint's publication leaves an unreferenced segment; it is harmless but not cleaned up.
- macOS and Linux runs.
