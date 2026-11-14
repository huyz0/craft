# Issue-bound edit metadata and transitions

The Jira host reads `jira.metadata.edit` and `jira.transition.list` through verified service access. These reads are now public; see [current usage](jira-context.md). Earlier [acceptance evidence](evidence/jira-issue-context.md) distinguishes local fixtures from live-provider qualification.

## Issue observation

Each native read resolves the requested issue ID/key with `GET /rest/api/3/issue/{issueIdOrKey}` selecting only `project,issuetype,status,updated`. It disables history updates and field-key interpretation and enables fail-fast field handling. The metadata call uses the resolved numeric issue ID. A second narrow issue read uses that same numeric ID. Changed ID, key, project, issue type, status or updated evidence refuses the result.

This brackets an observation. It cannot establish an atomic snapshot, detect every intervening configuration change or authorize a mutation. Output always includes `atomic_snapshot:false` and `mutation_authorized:false`. The issue observation records the requested selector, resolved issue, resolution kind and observed state. Empty transition results retain this observation; they do not establish issue absence or permission to transition.

An in-process, non-deserializable `LiveIssueContext` additionally binds the prepared operation digest and principal. It expires at 60 seconds on either wall or monotonic time, including time spent obtaining the metadata, and rejects backward clocks. Serialized observations and cached pages cannot recreate this proof. Write-specific validation and recovery remain separate work.

## Native metadata and selection

Edit fields come from `/editmeta`. Transition records come from `/transitions`, using actual transition IDs rather than status names. Transition inputs allow an exact ID filter, unavailable transitions and native status ordering. Only selecting transition field metadata requests `expand=transitions.fields`. Administrative screen/editability/remote-condition bypass flags are unavailable.

Edit defaults select ID, name, required, operations and schema type. Transition defaults select ID, name, destination ID/name, screen presence and availability. IDs and missing-field indicators are retained. Native field-map keys become field IDs; a native field's `key` remains a distinct property. Mandatory native metadata is checked even when unselected. Selected null and absent values remain distinct.

Schema/plugin identifiers, configuration, defaults and allowed values are bounded inert data. They are not executable schemas or instructions, and autocomplete links are neither returned nor followed. Unknown native properties are omitted. Edit/transition metadata does not reuse create-only schema rules.

## Buffered pages and authority

Normalized pages carry an optional object context, bounded to 32 KiB of serialized JSON. Context participates in the normalized page digest and total page/payload limits. It survives buffered continuation, replay, process/store reopen, empty native pages and oversized-item recovery. A new native page replaces the prior context, including clearing it when absent.

Absent context is omitted from page and cursor serialization, retaining legacy encodings and format-one cursor compatibility. Explicit null is preserved during decoding so runtime object validation rejects it instead of treating it as an absent legacy field. Invalid or oversized cached context fails even when its payload checksum has been recomputed. This addition does not change the SQLite schema or settings format.

The host validates exact target/grant/actor policy, typed input, projection and complete recovery budgets before authentication. It validates cached typed issue observations and rechecks current access/policy before returning results. Neither a cursor nor its evidence grants access. Errors suppress metadata and continuation while retaining independent credential-cache receipts. Larger byte budgets can recover a buffered oversized record without repeating the three resource reads.

## Remaining delivery work

[Typed command adapters and explicit operation allowances](issue-metadata-command.md) connect this host to public invocation. Contextual permission probes are also public. Live qualification, public edit/transition execution and wider recovery remain unfinished. No live tenant was changed by this addition.

Source contracts: the pinned Jira v3 document under `openspec/changes/build-cloud-cli/sources/jira-v3.json`, and the official [issues API](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/).
