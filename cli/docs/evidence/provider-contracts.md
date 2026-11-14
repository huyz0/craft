# Provider contract inventory acceptance

Date: **10 November 2026**, Australia/Sydney. This records an offline developer inventory and native route syntax correction. No public provider, authenticated request, mutation or consequential auth path was enabled. Review is by the author; independent auth/destructive/recovery review remains required before enabling the owning implementations.

## Scope and requirement audit

| Requirement | Implementation and evidence |
| --- | --- |
| Every researched Core/Extended/Privileged/Conditional resource command remains accounted for | `tools/providers.py` parses the actual catalog tables and compares their exact command set and allowed tiers with authored `provider-catalog.json`; 99 commands, with 57 Core, 29 Extended, 12 Privileged and one Conditional entry |
| Exact source-bound routes and native schemas | Three checksum/length/version-pinned official OpenAPI files under the change's `sources/`; 112 route variants identify native methods/paths, parameter/body/response pointers and operation digests |
| Scope alternatives and permission evidence | Current/Beta alternatives remain separate, including the documented empty scope alternative; source description pointers, bounded permission excerpts and app-access-rule evidence remain available without treating scope as permission |
| Owning work, conditions, effects and recovery | Authored mapping uses existing task names in planning data only; it distinguishes inline/footer, RBAC/legacy, upload/upsert/replacement, current-page/trash/purge and async submission |
| Unverified/deprecated operations stay unavailable | Generated command/variant availability is always false; explicit reasons identify missing auth/tenant/implementation plus permission, lifecycle and scope-definition gaps |
| Match actual core route contracts | A development-only Rust example reads the bounded matrix at runtime, constructs all 112 native route shapes and calls `NativeRoute::validate`; it registers no provider, embeds no planning data in a binary and exercises no network path |
| Support legitimate provider parameter names without credential queries | `craft-core/src/execution.rs` allows safe hyphenated names and normalizes hyphens when checking credential-query exclusions; a regression test exercises `comment-id`, `body-format` and both underscore/hyphen credential spellings |
| Drift and negative controls | The checker refuses source/catalog/ownership/constraint/recovery drift; selftests exercise deliberate failures, missing evidence and source inconsistencies rather than creating live resources |

The [usage and source ownership document](../provider-contracts.md) records exact file locations, source hashes, gaps, change procedure and limits. Machine-readable task ownership remains under `openspec/`; it is not compiled into the production executable. Native OpenAPI schema references are not final Craft schemas or runtime provider descriptors.

## Observed checks

`python tools/providers.py write` generated the inventory after validation. `python tools/providers.py check` passed with **99 commands, 112 variants and three pinned sources**. `python tools/providers.py selftest` passed **24 controls**. `cargo run --quiet --locked --offline -p craft-cli --example provider_routes` passed **112 native route shapes**. `cargo test --locked --offline -p craft-core --test provider_execution` passed **nine tests**, including the new parameter-name/credential-query test. Output remains in `target/provider-final-route-shapes-output.log` and `target/provider-route-tests-output.log`.

The initial public-spec fetch failed DNS resolution in the restricted environment. A separately authorized public HTTPS download succeeded, with exact source metadata in `target/provider-spec-fetch-output.log`. No credentials or tenant URLs were sent. Early matrix generation rejected the documented empty OAuth scope set; inspection established the explicit empty Current alternative for Jira contextual permission checks. It is now preserved as source evidence, distinct from missing evidence. A later strict definition check exposed the Confluence attachment scope discrepancy; it is now flagged as unresolved instead of silently filled in. These were checker-development failures, not live API failures.

An initial native-access development gate passed with the earlier 110-variant inventory. Its log remains `target/provider-inventory-native-verification-output.log`. The final inventory also covers Jira async-task handles for global operation reads/waits, and the checker now rejects tier drift against the catalog. Catalog wording was clarified for inline comment lists and ordinary restriction reads; deprecated version deletion was updated from the pinned source.

The final native-access `python tools/verify.py` passed **218 named Rust tests, zero failures and five ignored opt-ins**, the nineteen-operation public snapshot, ten public contract controls, formatting, locked inventory/layer checks and all-target Clippy. It also passed the new **99-command/112-variant inventory check, 24 matrix controls and 112 native route shapes**. Output: `target/provider-final-native-verification-output.log`. The five opt-ins are the earlier native lifecycle/console and external ADF fixture cases; they were not rerun here. This is not live provider acceptance.

`cargo build --workspace --locked --offline` passed in `target/provider-inventory-build-output.log`. Dependency inventory remains 282 packages with no manifest/lock/package-version change. Local links and code fences passed for the provider documents, CLI README and updated root catalog. A separate audit confirmed all generated command and variant availability flags remain false. Only the provider-inventory task closes; resource adapters, auth and live qualification remain open.

SHA-256 checkpoints, with the full eleven-artifact set retained in `target/provider-inventory-checkpoint-hashes.json` and the three raw-source hashes recorded in the source manifest:

| Artifact | SHA-256 |
| --- | --- |
| `tools/providers.py` | `5df9a726dd7c39b127b9cbbf4fea8144799e9297c54f046c5b91931fc7c6f16c` |
| `provider-catalog.json` | `dbe863e21fb7fbc14a49e6836cb05c36668998c443139a2204f7b28e9df45496` |
| `provider-operations.json` | `9f5cac09ad08bdb48cfbb60419862cc4f447b113fb767a30854f33dc338af518` |
| `crates/craft-core/src/execution.rs` | `051b5fd42135cb7a099841e04bab932866559a6923edfa5a6983b0b35fe31746` |
| `target/debug/craft.exe` | `0e5d2c0820fb8c500326e82fa227f2928569bac1bb9e3a352b54c9418f0fe526` |

The two matrix filenames in the table are relative to `openspec/changes/build-cloud-cli/`. The existing lock and public schema snapshot hashes are unchanged from the local planning checkpoint. This workspace has no Git revision.

## Material limits

The inventory is an author-reviewed source/coverage checkpoint, not independent security review or live conformance. Missing permission sections, the attachment scope dictionary discrepancy and async project-delete lifecycle remain explicit gaps. OAuth grants, product eligibility, tenant permissions and feature settings are unverified. Provider-specific request validation, pagination semantics, response budgets, native error handling, retries/reconciliation and public registration still require implementation and their own acceptance. Bitbucket, Trello and Projects/Assets/Artifacts remain explicit later/research surfaces. Only the Windows toolchain was exercised. No package was installed, no real credential/resource was created and no commit, release or deployment occurred.
