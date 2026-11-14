# Candidate project-list arguments and dispatch

The Jira project-list adapter has typed action arguments, profile/site selection, revisioned private read configuration and complete-result dispatch. It is now public with [policy setup and usage](jira-context.md). Live access remains separately unqualified; the evidence below describes its earlier internal qualification.

Implementation: [arguments and preparation](../crates/craft-cli/src/project_list/command.rs), [private configuration](../crates/craft-runtime/src/project_read_policy.rs), [host](../crates/craft-cli/src/project_list.rs), and [dispatch](../crates/craft-cli/src/dispatch.rs). See [acceptance evidence](evidence/project-list-command.md) for actual checks and limits.

## Argument contract

The candidate action is `jira project list`. Its closed JSON argument object and scalar flags represent the same values and defaults:

| Argument | Meaning |
| --- | --- |
| `cloud_id` | Optional exact Cloud UUID; otherwise resolve configured context |
| `query` | Optional literal key/name query, with the provider's bounded-text validation |
| `page_size` | Native page size, one through 100, default 50; bound into continuation |
| `limit` | Output item limit, one through 100, default 20; adjustable when resuming |
| `cursor` | Optional issued opaque handle; malformed shape is refused before state opens |

Fields, byte budget, profile, state root and timeout remain shared invocation controls. No actor, grant, site-origin, enabled-state or revision argument is accepted. An unknown argument or wrong JSON type is rejected. A flag value `--query null` is the literal text `null`; JSON null for this optional read filter means no query. There is no apply flag on the read command.

Input, projection and worst-case paging/cache-recovery output are checked before opening state. The fixed UUID/grant lengths make the pure output probe independent of actual profile selection. A structured-input decoder retains the selected action rather than treating every Jira action as issue creation.

## Profile and site selection

Use existing explicit profile, repository profile and active profile precedence. An explicit profile bypasses repository hints entirely. Otherwise the current directory's bounded `.craft.json` is read once, with no upward search. Invalid hints fail rather than silently falling back.

Require a Jira Cloud service profile. Site precedence is explicit `cloud_id`, a typed repository `site` hint, the profile's `site` default, then the configured profile tenant. Every target must have type `site`, an ID equal to its Cloud UUID and the same tenant as the selected profile. An explicit cloud selects context and cannot widen the profile's tenant. A repository hint can select an existing profile/site, but cannot supply or enable read authority.

The selected site must match an enabled private project-read record. Missing, disabled, corrupt, foreign or mismatched configuration fails before credential acquisition. Cursor semantics are checked before acquisition as well. Credentials remain in the selected native keyring under the issued grant, and the command never implicitly provisions them.

## Private configuration and migration

The `project_read_policies` table was introduced in SQLite schema five; the current private schema is seven. Records bind installation, profile, issued grant, exact site, expected actor, canonical continuation-site origin, listing enabled state and revision. Only the trusted-host API can change them; no public policy-write command was added. Updates require apply, output preflight and expected-revision CAS. Concurrent writers cannot silently replace the same revision.

Payload format one remains list-only. [Format-two project-context permissions](project-context-command.md) add explicit get/status allowance independently of listing. Listing edits preserve those permissions, and shared-actor changes require all enabled context entries to be disabled first. A context edit changes the whole record's revision and invalidates a captured listing preparation.

The configured site origin constrains parsing of project-search next links; it never becomes an authenticated destination. The actual resource call keeps its verified Cloud-ID gateway and provider-declared route. Current scopes and provider visibility remain independently required. This local record is neither human approval nor immutable audit history, same-principal isolation or distributed fencing.

Migrations through the current schema seven are atomic. Existing profile/grant/settings/preferences, journal, create policy, issue/project-read records and both cursor tables are preserved; separate field permissions are added without inferred authority. Settings keep format version one. Exact table shape and absence of triggers are checked on open. Conflicting schema fails without repair, and earlier migration DDL/version changes roll back. Older binaries have no downgrade path; payload-format support is also checked explicitly.

## Execution and output

Preparation captures the exact read record. Execution rechecks it before cached authentication and during source access checks, including the final check before a page or buffered records are published. Disabling listing returns `POLICY_DENIED`; changing an enabled listing record's revision returns `VERSION_CONFLICT`. A later failure preserves a confirmed or uncertain credential-cache update independently from the project read.

The host keeps the [qualified project paging contract](jira-project-list.md). Dispatch returns its complete envelope and output policy rather than rebuilding a success response. Page controls, oversized-item recovery cursor, errors, warnings, cache receipts and the original request identity therefore survive encoding. Public startup never calls candidate registration, so source availability does not imply an advertised capability.

Remaining work includes public activation and live acceptance, operational configuration commands where required, the complete remaining Jira context/CRUD surface, recovery and Confluence workflows. Consequential auth activation still needs the existing independent review. This candidate does not complete the broader delivery goal or its open task dependencies.
