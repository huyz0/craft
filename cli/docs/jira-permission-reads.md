# Contextual current-user permission reads

The public GET view of `jira.permission.check` composes typed arguments, verified service access and immutable local paging. It calls `/rest/api/3/mypermissions`. See [current usage](jira-context.md) and earlier [host evidence](evidence/jira-permission-reads.md). Live qualification remains unfinished.

## Interpret the context before the flag

Jira evaluates project permissions differently by context. Craft preserves this in the mandatory `context` output:

| Context | Native selector | Meaning of a reported project permission |
| --- | --- | --- |
| Global | None | Available in at least one project |
| Project | `projectId` or `projectKey` | Available for a qualifying issue or the project, depending on its permission criteria |
| Issue | `issueId` or `issueKey` | Available for the requested issue or its containing project, depending on its permission criteria |
| Comment | `commentId` | Comment visibility and the parent issue's project permission; only `BROWSE_PROJECTS` is supported |

Global permission kinds are unaffected by these contexts. Results retain native `GLOBAL`/`PROJECT` kind where returned. The endpoint does not echo resolved resource identity; Craft records the exact requested selector and native parameter without claiming resolution or resource existence. A true flag does not establish all issue visibility, workflow/screen/value or app-access constraints. It does not approve a write. The endpoint is exempt from app access rules, so its result cannot replace checks made by the eventual resource operation.

These semantics follow the pinned Jira v3 source and the official [permission reference](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-permissions/), reopened on 10 November 2026. The qualified scope is the pinned current classic `read:jira-work`. The live guide's granular scope does not silently activate the pinned beta alternative.

## Strict requests and faithful missing results

Require one to 32 unique explicit permission keys, each at most 256 bytes. ASCII letters, digits, dot, underscore, colon and hyphen support native/plugin keys. Whitespace, control characters and commas are refused. Repeat `--permission` flags or use a JSON string array; the adapter constructs the native CSV only after validation. No argument can supply another account, arbitrary query/expand text, credentials or policy authority.

Accept zero or one of `project`, `issue` and `comment_id`. Project/issue values can use the validated native ID/key forms. A comment must use a numeric string and request only `BROWSE_PROJECTS`. IDs remain raw strings; the adapter does not silently canonicalize leading zeroes. UUID configuration selectors and bulk/other-user POST variants require separate qualification.

One result is emitted for each requested key, in input order. `returned:false` means no matching native entry was returned. `have_permission:null` means no boolean decision was returned; native false remains false. Missing entries/flags never become grant or denial. Key, returned state, decision, native kind and missing-field indicators survive selection. Optional ID, name, description and deprecated-key metadata are bounded inert values. Unknown response properties and links are omitted. Unexpected keys, mismatched map/native keys, wrong decision types, invalid selected metadata, warnings/errors and malformed JSON refuse the page without partial items.

## Host, cache and configuration

The exact site, issued grant and expected actor must match verified service access. Anonymous API availability does not select anonymous execution. A private in-process observation binds operation/principal and a 60-second wall/monotonic freshness window, including the read itself, with backward-clock refusal. It cannot be deserialized, reconstructed from cache or used as mutation authority.

The unpaginated native result is buffered through existing immutable cursors. A resume may change item/byte limits but cannot change key order, context, projection, target or actor. Context and requested-key metadata are rebuilt from the exact bound input; observation and cached state remain visible. A larger byte budget recovers oversized selected metadata without another permission request. Cached records must retain valid types, requested-key order and consistent unknown-decision state. They remain historical observations, not fresh private proof.

Preparation validates input, projection and complete error/cache/page budgets before opening state or authenticating. Candidate context uses existing explicit/repository/active profile and exact-site precedence. The site-bound issue-read record must explicitly include `jira.permission.check`, appended after its four previous operations. Existing records gain no allowance; a permission-only record grants no ordinary issue or metadata command. Apply/preflight/CAS and actor/target-transfer guards are unchanged. SQLite schema seven, settings format one and policy format one remain in use.

Captured policy/access is checked before native or cached work and after final output/cursor work. Denials suppress permission items and continuation while retaining independent renewal receipts. Dispatch preserves original request identity, context, unknown results, errors, warnings and complete recovery evidence. Neither checks nor cursors establish distributed fencing.

Bulk/other-user permission queries, resource-specific write validation/recovery, independent consequential review, public activation and real tenant acceptance remain delivery work.
