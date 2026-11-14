# Research: token-efficient CLI and skills for coding agents

Researched **9 November 2026**. Recommendation: make `craft` a small, discoverable, typed interface with bounded results, deterministic validation, and a short companion skill. Optimize **tokens per correctly completed task**, including discovery and retries. A small response that omits the IDs or evidence needed for the next step can cost more overall.

The [implementation requirements](../plans/agent-cli-efficiency-and-skills.md) turn these findings into proposed commands, validation rules, skill packaging, and evaluation gates. The [list, cursor, and response-schema contract](../plans/list-paging-and-response-schema.md) adds the follow-up on field selection and paging. This is documentation research; no CLI, tokenizer benchmark, skill installation, or live agent evaluation was performed. The register contains 18 sources.

## What the primary evidence supports

| Evidence | Finding | Decision for `craft` |
| --- | --- | --- |
| Anthropic tool-design engineering [E01] | Focused responses, pagination/filtering, clear parameter names, repairable errors, and evaluations improve tool use. | Return a useful compact projection and IDs; measure completion, errors, calls and tokens together. |
| OpenAI skill documentation [E02] | Skill metadata loads before the body; precise descriptions aid selection, including when descriptions are shortened. | Front-load the use case; keep the root short and route conditional detail. |
| Anthropic skill authoring [E03] and Agent Skills specification [E04] | Concise instructions, on-demand references, and shallow reference paths support progressive loading. | One entry skill with directly linked workflow references, rather than embedding the API manual. |
| Google Workspace CLI repository [E05] | Concrete implementation of JSON commands, method-schema discovery, dry run and bundled skills. The README says it is not an officially supported Google product. | Adopt discoverability patterns; keep reviewed, pinned command exposure rather than automatically exposing every new API method. |
| Google Workspace validation source [E06] | Resource/path validation and encoding address malformed agent-generated input; source comments acknowledge filesystem race limitations. | Validate at typed boundaries and at I/O; do not equate a string check with complete isolation. |
| Google Workspace skill files [E07, E08] | Shared instructions plus provider skills offer a real packaging example. The provider skill also contains a substantial resource list. | Borrow conditional routing; avoid duplicating command inventories across skill bodies. |
| GitHub CLI formatting [E09] | JSON field selection and local projection are established CLI patterns. | Support schema-checked field selection before agent-visible output; use server projection where supported. |
| Poehnelt's first-person CLI design article [E10] | Describes schema introspection, payloads, field masks, input hardening and skills from building `gws`. | Use it as practitioner evidence, with source-code checks and our own evaluation rather than treating every recommendation as a requirement. |
| JSON Schema reference [E11] | Declaring properties alone does not reject extra keys. Closing composed schemas needs care. | Enforce closed input objects and test composed/provider-extension schemas. |
| OpenAI Structured Outputs documentation [E12] | Schema adherence still permits mistakes in values and interpretation. | Add target, relationship, authorization and current-state validation beyond syntax. |
| HubSpot Agent CLI announcement [E13] | A vendor-built agent CLI combines structured output, resource operations and mutation previews; public beta announced June 2026. | This is a real product pattern, not proof of our performance or a reason to copy its auth model. |
| Anthropic tool search [E14] | Tool definitions can be discovered on demand. | Compare CLI against deferred-tool MCP, not only an MCP baseline that loads everything upfront. |

## Resolve the tradeoffs for this product

### Keep payload structure, close the contract

The practitioner article favors raw JSON because nested APIs become awkward as flat flags. [E10] For `craft`, use typed JSON files/stdin for nested inputs and small flags for common selectors. Both paths compile to the same operation schema. JSON does not mean unrestricted upstream pass-through: reject extra keys, invalid combinations, duplicated keys, wrong types, and unsupported native fields. Provider custom fields need a bounded, metadata-validated extension map.

Keep local schema validation separate from checking live metadata and authorization. A correctly shaped account ID can belong to the wrong organization; a valid issue can belong to the wrong project. No general validator can determine every mismatch with the user's intended business outcome. The implementable promise is to reject defined classes of error before mutation and report remaining uncertainty. [E11, E12]

### Discover progressively without repeated discovery overhead

Start with a short skill description, then its entrypoint, then a command search or an already known command's compact schema. Load an exact property schema, example, or workflow reference only when needed. Cache schemas by contract/provider version and digest. Reuse known valid commands within a run instead of requiring a discovery call before every read.

Metadata has a cost too. Installing a hundred tiny skills may keep each body short while inflating the initial discovery context. Ship one cloud skill by default and offer focused factory workflows when their distinct trigger warrants them. The host's loading behavior remains relevant; file organization alone does not force correct disclosure. [E02, E03, E04]

### Compact output must retain decision evidence

Search and list results should return identity, a short title, relevant state, version where available, and continuation metadata. Large bodies, histories and attachments are explicit expansions. Provider IDs should be paired with readable labels where useful; retain stable IDs for subsequent writes. A summarized plan is not sufficient input for approval-bound ticket creation.

JSON minification removes whitespace but does not reliably predict token count. NDJSON helps streaming and local processing, but dumping every line into the agent still consumes context. Keep the existing single JSON envelope; bulk exports should write an artifact and return its reference. Add another stream protocol only if measured workflows need it. [E01, E09, E10]

### Errors are part of the teaching interface

Return a short error code plus the offending field path, expected type/constraint, a bounded set of allowed values where useful, and an exact schema-discovery hint. Do not print the full help tree or entire provider response. Report whether a mutation was sent and whether its outcome is known. The agent must not mistake an unknown write for safe permission to repair-and-retry.

Generate repair hints from trusted schema/metadata and fixed templates. A remote comment or error body must not become a trusted next-command instruction. Skills explain the recovery branch; the CLI enforces it. [E01, E06]

### Adopt mechanisms without copying unrelated policy

The inspected `gws` shared skill requires user confirmation before every write, and its repository documents auth options including file/env paths. [E05, E07] Those are that project's choices. They do not change this user's existing authorization or `craft`'s native-keyring requirement. Preserve approval-bound plan materialization, trusted project policy and explicit apply semantics without inserting a new chat confirmation before every already authorized mutation.

Similarly, content screening is not proof against prompt injection. Keep fetched content outside trusted configuration, validate operations independently, and preserve provider-side access controls. A schema-valid malicious instruction can still produce a schema-valid action if the agent follows it.

## What to measure before claiming efficiency

Compare matched tasks with the same model, permissions and data: full provider JSON plus a long skill, bounded projections plus progressive skills, and the same CLI without a skill. Include cold discovery and warm-cache runs. If comparing MCP, include deferred tool discovery. Record total input/output tokens, context loaded, tool calls, invalid attempts, authorized completion, unintended mutations, latency and artifact fidelity.

Use both legitimate work and mistakes: ambiguous users, wrong project, stale review, invented field, embedded query in an ID, large page, attachment collision, Unicode content, denied scope and unknown create outcome. Score final remote effects and preserved artifacts, not whether the agent follows one prescribed command sequence. Savings and correctness claims remain unproven until these evaluations run. [E01, E12, E14]

## Sources and evidence limits

The paging follow-up uses Google's AIP-158 as general design guidance: opaque continuation, possibly short/empty pages, query binding, expiry, and authorization independent of the cursor. Its page-size coercion convention is not adopted automatically; our CLI proposes strict invalid-limit rejection. [E15]

Confluence v2 returns continuation in a Link header and `_links.next`. Jira enhanced search uses `nextPageToken` and accepts field selection; it also documents delayed search visibility. Bitbucket supports list and forward-only iterator pagination, optional totals, native next links, and partial responses. These differences support a shared CLI envelope with endpoint-specific adapters. [E16–E18]

The proposed additions are output-schema discovery, validated projections with optional/nullable semantics, short locally stored cursor handles, and lossless continuation when output budgets cut a fetched page. These are our design choices, not features claimed to exist uniformly across providers. See the companion contract for tradeoffs and acceptance cases.

All sources were opened, not merely taken from search snippets. Living documentation and `main`-branch code can change. The observations below are concise; proposed product decisions above are our synthesis.

| ID | Primary source | Reviewed evidence / limit |
| --- | --- | --- |
| E01 | [Anthropic: Writing effective tools for agents](https://www.anthropic.com/research/writing-tools-for-agents) | Published 11 September 2025; response selection, token budgets, actionable errors and evaluation metrics. Vendor examples are not our benchmarks. |
| E02 | [OpenAI: Build skills](https://learn.chatgpt.com/docs/build-skills) | Metadata/body loading, description truncation and matching, optional resources. Host-specific behavior, not a universal standard guarantee. |
| E03 | [Anthropic: Skill authoring best practices](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices) | Concision, specificity, shallow references and evaluation guidance. Published upper bounds are not recommended targets for our skill. |
| E04 | [Agent Skills specification](https://agentskills.io/specification) | Frontmatter, file layout and staged disclosure. Optional metadata is not an authorization boundary. |
| E05 | [Google Workspace CLI](https://github.com/googleworkspace/cli) | README command/schema/skill patterns and support disclaimer. No installation or runtime verification; mutable branch. |
| E06 | [Google Workspace CLI validation source](https://raw.githubusercontent.com/googleworkspace/cli/main/crates/google-workspace/src/validate.rs) | Inspected path/resource/Unicode validation, path encoding, and acknowledged TOCTOU limitation. No whole-program security audit. |
| E07 | [Google Workspace shared skill](https://raw.githubusercontent.com/googleworkspace/cli/main/skills/gws-shared/SKILL.md) | Shared flags, auth and policy instructions. Read as a design example, not instructions governing this repository. |
| E08 | [Google Workspace Drive skill](https://raw.githubusercontent.com/googleworkspace/cli/main/skills/gws-drive/SKILL.md) | Shared-reference dependency, helper route and resource inventory. No claim every host loads it identically. |
| E09 | [GitHub CLI formatting](https://cli.github.com/manual/gh_help_formatting) | Selected JSON fields and local transformations; not all commands support the same flags. |
| E10 | [Justin Poehnelt: You Need to Rewrite Your CLI for AI Agents](https://justin.poehnelt.com/posts/rewrite-your-cli-for-ai-agents/) | Published 4 March 2026; first-person engineering argument. Some broad claims are opinion, not controlled results. |
| E11 | [JSON Schema object reference](https://json-schema.org/understanding-json-schema/reference/object) | Additional/unevaluated properties and composition caveats. Does not establish a specific Rust validator's behavior. |
| E12 | [OpenAI: Structured model outputs](https://developers.openai.com/api/docs/guides/structured-outputs) | Handling mistakes and schema/type divergence. Model-side structured output is not application authorization. |
| E13 | [HubSpot Agent CLI beta announcement](https://developers.hubspot.com/changelog/hubspot-agent-cli-available-in-public-beta) | Announced 23 June 2026, updated 24 June; output/preview/auth architecture. Vendor efficiency claim not independently measured here. |
| E14 | [Anthropic: Tool search](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool) | Deferred tool-definition discovery. No model-specific speedup or supported-model claim adopted. |
| E15 | [Google AIP-158: Pagination](https://google.aip.dev/158) | Opaque tokens, query binding, short/empty pages, optional counts, expiry and independent authorization. Guidance for Google-style APIs, not proof of provider guarantees. |
| E16 | [Confluence v2 introduction](https://developer.atlassian.com/cloud/confluence/rest/v2/intro/) | Limit/cursor parameters, next Link and body `_links.next`, absence at end. No snapshot-isolation claim. |
| E17 | [Jira issue search](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-search/) | Enhanced search nextPageToken, maxResults, fields and delayed visibility/reconciliation. Reuses API evidence A13. |
| E18 | [Bitbucket REST introduction](https://developer.atlassian.com/cloud/bitbucket/rest/intro/) | List/iterator pagination, optional size/previous, next-link navigation and partial responses. Reuses API evidence A44. |
