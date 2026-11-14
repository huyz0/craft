# Provider continuation library

The internal provider-neutral paging engine preserves fetched records across item and byte limits using immutable local cursors. It is not yet connected to a public cloud listing command. Local `profile list` keeps its existing typed cursor implementation.

The contracts live in [core pagination](../crates/craft-core/src/pagination.rs); [runtime pagination](../crates/craft-runtime/src/pagination/mod.rs) composes the engine and [private state](../crates/craft-runtime/src/pagination/state.rs). Their tests use normalized in-process sources and real private SQLite state on Windows. They do not establish a live provider's paging or permission behavior.

## Bindings and access

`ListBinding` includes operation, descriptor/schema digests, API revision, exact resource target, issued local grant, verified actor, effective projection digest and resolved semantic query. Output budgets, local cursor and token generation are absent. Resolve defaults and reject unsupported fields before constructing the binding. A token renewal under the same grant and actor does not change it. A change to target, actor, grant, query, operation, descriptor, schema, API revision or fields requires a new traversal.

`PageSource` is a trusted composition port. `check_access` must verify the exact binding using the normal actor, scope and access machinery. The engine calls it before serving a native or cached page, and again before output publication. A cursor, profile default, or implementation of this trait supplies no permission by itself. The current library tests verify call order and denial behavior with an injected source, not sealed live-auth composition. Cached visibility describes the original provider fetch; it is not proof of current per-record visibility after an item-specific permission change.

`fetch` returns one normalized native page with records and explicit continuation. Adapters must qualify their own terminal conditions, native parameters and route constraints. The engine never follows URLs or infers a position from item count. It fetches only when the buffer is empty and does not refill a short buffered result from another native page. Transport retries, when a qualified read permits them, remain the transport's responsibility.

## Budgets and output

The caller supplies `fits`, which must measure the complete projected envelope, including mandatory page controls and any error. It must be free of I/O/publication and monotone for growing nonterminal prefixes. The engine first reserves the complete recovery form with worst-case fixed metadata lengths. An insufficient recovery budget fails before source access or fetching. It checks the actual final page as well.

The largest fitting ordered prefix is returned. Every remaining record is persisted before a continuation handle is exposed. A first record that cannot fit produces `ITEM_TOO_LARGE`, zero items and a recovery cursor at that exact record. Retrying with a larger byte budget preserves the observation. Changing fields changes the binding and requires an explicit new traversal. The host must preserve `ListPage.page` when promoting `ListPage.error` to the public error envelope.

| Stop reason | Meaning |
| --- | --- |
| `item_limit` | The selected item limit retained records locally |
| `byte_limit` | The byte budget retained records locally |
| `item_too_large` | The first complete record cannot fit; its recovery cursor retains it |
| `native_page` | This native page is fully emitted but provider continuation remains |
| `exhausted` | No buffered records or native continuation remain |

`truncated` is true only when this invocation retained normalized records. Empty/short nonterminal pages remain resumable. Observation time and normalized native-page digest remain unchanged during buffered replay, with `cached: true`. A fresh native page gets a new observation. `consistency` is always `live`: replaying a position that requires a fresh fetch can observe changed data.

## Persistence and failure

SQLite schema four introduced `provider_cursors`, separate from `profile_cursors`; current schema five preserves both tables while adding private project-read configuration. Upgrades from schemas one through four are atomic. Existing profile/grant/settings/preferences, local-profile cursors, journal rows and create policies are preserved where present. Conflicting tables or triggers are refused, and earlier DDL in the same migration rolls back. Settings retain format version one. Opening an older database does not establish a downgrade path.

Handles contain 128 random bits and never encode a provider token. Their rows are insert-only during a traversal; reading or replaying does not advance or consume an existing handle. Concurrent replay can publish different successor handles at the same logical position. The state root's existing private-path and SQLite protections apply. The exact-payload checksum detects accidental corruption, not malicious modification by the same OS principal.

| Bound | Value |
| --- | --- |
| Traversal lifetime | Fifteen minutes from traversal creation, without renewal |
| Live cursor handles | 256 |
| Stored payload per handle | One MiB |
| Aggregate stored payload | Sixteen MiB |
| Normalized native page | 1,000 object records and 512 KiB |
| Selected output item limit | One through 100, matching the shared CLI selection contract |
| Native-position history | 256 requested-position digests |
| Native continuation | At most sixteen parameters, 32 KiB encoded total |

Capacity failures do not evict live handles. Expired rows are reclaimed transactionally when publishing a new handle. The fixed lifetime bounds observation age but does not imply an immutable provider snapshot. Raw response bodies and credentials are not cursor contents; providers must normalize only required records and continuation.

Invalid, missing/foreign, expired, mismatched, corrupt, capacity-exceeded and repeated/cyclic continuation failures have distinct error codes. Errors never silently restart. Callback/access/cancellation/persistence failures return no successful successor page and preserve the predecessor. A local commit acknowledgment failure may leave an unreachable successor row; it expires normally, and is not reported as an unknown cloud mutation.

The shared response schema now includes the eight paging error variants. Derived projection digests change with that schema. An existing command cursor bound to the previous schema can therefore be refused after upgrading even though its database row was preserved. Explicitly start a new traversal in that case; do not weaken schema binding to continue it.

## Remaining integration

Each provider needs typed normalization, native continuation validation, scope/actor/access composition, actual envelope/projection measurement, dispatch and fixture/live acceptance. The internal [Jira project-list host](jira-project-list.md) now composes these library boundaries using explicit next offsets and a constrained site-link parser; its public dispatch and live acceptance remain unimplemented. The generic engine introduces no authenticated URL bypass. The broader cursor/listing tasks remain open until their dependencies and actual command acceptance hold. Overflow artifact export, optional totals and a provider-specific snapshot mode are not supplied by this library.
