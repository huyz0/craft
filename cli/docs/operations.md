# Inspect local operation evidence

`craft operation get --operation-id <identity>` reads the selected installation's durable mutation journal. Use it to recover the recorded target, actor, grant, request digests, revision, named steps, confirmed resource IDs and async handles after interruption. It does not verify current provider state or authorize another send.

```text
craft operation get --operation-id publish-summary
craft --state-root C:\craft-state operation get --operation-id publish-summary
craft operation get --input request.json
craft operation get --operation-id publish-summary --fields steps.name,steps.state,steps.confirmed_resources,steps.job
craft schema --command operation.get --response --view full --max-bytes 65536
```

For structured input, `request.json` contains `{"operation_id":"publish-summary"}`. `--input -` reads the same object from stdin. Action flags and structured input are exclusive; output controls remain flags. An exact opaque identity is required. It is not interpreted as a filename or normalized. Unknown fields, duplicate JSON keys, malformed identities and invalid field selections fail before opening state. The local input validator applies the same identity rules.

## What the result establishes

| Field | Meaning |
| --- | --- |
| `operation_id`, `found` | Exact lookup and presence in the selected local installation only |
| `binding`, `revision` | Stored immutable operation binding and journal revision; null when absent |
| `steps` | Named steps in stable name order, including request digest, submission state, confirmed resources and optional job |
| `reconciliation_required` | True for any started/unknown submission or unknown job; null when no local record exists |
| `evidence_scope` | Always `local_installation` |
| `live_verified` | Always false; no provider call is made |
| `replay_authorized` | Always false, including for prepared, failed, confirmed or absent records |

An accepted or running job remains pending in `steps.job.state`; a confirmed submission does not establish job completion. Failed or unknown steps can retain known resource IDs. A false reconciliation indicator only describes the stored uncertainty flags. It grants no execution authority and does not establish overall workflow completion.

The operation ID, local presence, reconciliation indicator, evidence scope and both authority flags survive `--fields`. Select step fields to reduce output or raise `--max-bytes` for a large complete record. A record that exceeds the output budget produces an error without a partial success payload. An explicit projection can omit evidence; use the full record before making a recovery decision.

## Select the correct installation

State-root precedence is explicit `--state-root`, trusted `CRAFT_STATE_ROOT`, then the native user location, matching other local state commands. The existing private state opener can initialize or migrate metadata. Inspection does not change operation records. `--profile` is rejected: operation identity belongs to an installation journal, and inspecting it does not load credentials, resolve repository profile hints, log in or refresh tokens. Stored actors and grants are historical binding evidence, not proof of current permissions.

`found: false` means only that this installation has no record under that identity. A different machine, state root or operation ID may hold the evidence. It is never proof that a remote effect did not occur. Corrupt, locked, unavailable or cancelled state reads return an error instead of local absence.

Provider refresh, `operation list`, `operation wait` and `operation reconcile` remain unavailable in the public CLI. The [internal Jira reconciliation host and candidate](jira-reconciliation.md) bind supported original writes to exact journal evidence and inspect known/explicit issue IDs under separate persisted project recovery policy. Missing local evidence or create identities return without opening credentials. Recovery preserves uncertainty and grants no replay authority. Public/live qualification and broader correlation/job strategies remain unfinished. No marker reset, retry, deletion or remote compensation is offered. See the [journal contract](journal.md), [local inspection evidence](evidence/operation-inspection.md) and [current implementation state](../STATE.md).

## Listing local records

`craft operation list` pages the same records in operation-identity order, so an agent can find every interrupted write after a crash without remembering its identity.

```text
craft operation list --limit 20
craft operation list --unresolved-only
craft operation list --limit 20 --after-operation-id bravo
```

Each item is the record `operation get` returns. `has_more` and `next_after_operation_id` continue the walk; pass the latter back as `--after-operation-id`. Paging is by primary key, so records that already exist are never skipped or repeated; a record created during the walk appears only if its identity sorts after the current position. `--unresolved-only` keeps records with a started or unknown submission or an unknown job, and `scanned` reports how many records the page examined, including excluded ones. The list rejects `--profile`, reads only the selected state root and, like `get`, carries `live_verified: false` and `replay_authorized: false`. An empty page is not evidence that no remote write happened. Use `operation reconcile` on an unresolved record to investigate it.

## Previewing a write

`craft operation preview` checks a mutating command's input and describes what apply would do, without doing it.

```text
craft operation preview --command jira.issue.delete --file delete.json
craft --profile jira-test operation preview --command jira.issue.delete --file delete.json --check-remote
```

The result has the input and descriptor digests, the effect and whether it is destructive, retry and reconciliation classes, the operation ID and the `expected*` preconditions you supplied. Without `--check-remote` it opens no state and makes no request. With `--check-remote` it also runs the read that shows the current target (for example `jira.issue.get` for an issue delete) under the selected profile and that read's policy, and returns the read's envelope as `remote`. Creates have no existing target and refuse `--check-remote`. `write_sent`, `lock_held` and `approval_granted` are always false: someone else can change the target after the preview, and apply compares your preconditions against a fresh read again.
