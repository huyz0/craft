# Proposed command and data contract

All `craft` commands below describe a CLI to implement, not an installed executable. This contract accompanies the [implementation plan](atlassian-agent-cli-plan.md). The CLI is an extensible cloud integration tool, initially Jira/Confluence, later Bitbucket, Trello, and other providers. The central factory workflow is **normal Confluence plan page → human review → approved page revision → Jira tickets**. “Plan draft” is a review state, not an unpublished Confluence page. The [command catalog](cloud-cli-command-catalog.md) is the canonical resource-command inventory; the [provider architecture](cloud-cli-provider-architecture.md) defines extension boundaries.

The [agent interface requirements](agent-cli-efficiency-and-skills.md) define token budgets, progressive command/skill disclosure, strict inputs, compact output and behavioral evaluation. These are release requirements, not optional style advice.

Local plan/design scaffold, validate and render now have an implemented [usage and model contract](../../cli/docs/planning.md). Use that document and generated schemas for their current arguments and nested document targets. The examples below remain the broader proposed workflow and do not override the implemented local model.

## Invocation rules

- JSON is the default result format. Normal agent commands are noninteractive, never open a browser/editor/pager, and emit one complete envelope to stdout. Diagnostics on stderr are concise and secret-free.
- Accept UTF-8 JSON files or `--file -` for stdin. Document bodies can use `--body-file` with a declared format. No shell interpolation, arbitrary URL fetch, or command execution inside input.
- `--profile` selects an identity/target configuration. Explicit product/site/project/space IDs are required where a default is missing or ambiguous. IDs are strings, including numeric-looking IDs.
- `--limit`, `--cursor`, `--fields`, and `--max-bytes` bound reads. Results indicate continuation and truncation. Never silently truncate machine input or a mutation payload.
- The [list/projection contract](list-paging-and-response-schema.md) defines cursor binding/expiry, lossless mid-page continuation, response-schema discovery, field masks and optional/nullable output semantics. `schema --command ...` describes inputs; `--response` selects output schemas.
- Remote product writes require `--apply`. Without it, return `APPLY_REQUIRED` and perform no write; preview has a separate command. Auth commands and explicit local provisioning have their own named side effects.
- Every write accepts `--operation-id`, requires a stable payload digest, and follows journal/recovery rules. An operation ID does not create provider-side idempotency by itself.
- `--timeout` is a total invocation budget. Honor cancellation and return retry timing instead of sleeping indefinitely. No automatic login on 401/403.
- `--help` is the operator-readable exception. `schema`, `capabilities`, and `help --json` are discoverable machine interfaces. An optional `--output text` can support operators; it never changes write semantics.
- Reject duplicate JSON keys, unknown fields/flags, ambiguous identities and conflicting flag/file values. Do not silently coerce or repair input. Local parse/schema errors occur before network/auth operations; target/semantic checks precede mutation.
- Read/list output uses a compact metadata projection by default. Bodies/history are explicit expansions. `--fields` cannot hide mandatory control, continuation, conflict or mutation-outcome information. Preserve complete review snapshots in artifacts.

## Command families

| Family | Proposed commands | Purpose |
| --- | --- | --- |
| Discovery | `version`, `provider list`, `provider describe`, `command find`, `capabilities`, `schema`, `help --json` | Filtered discovery and compact schemas with property/full/example expansion |
| Validation | `input validate --command ... --file ...` | Local-only strict input checks; unchecked live conditions remain explicit |
| Profile | `profile list`, `profile show`, `profile create`, `profile use` | Non-secret target/identity metadata |
| Authentication | `auth provision`, `auth login`, `auth status`, `auth refresh`, `auth logout`, `auth revoke-info`, `auth doctor` | Secure credential lifecycle without token export |
| Sites | `sites list`, `sites select`, `sites verify` | Explicit product/cloud-ID selection; mode-aware discovery |
| Jira | `jira issue create/get/search/update/delete/assign`, `jira transition list/apply`, `jira comment create`, `jira link create`, `jira attachment upload/download/delete` | General resource operations and ledger maintenance; slash notation groups distinct commands |
| Confluence | `confluence page create/get/update/trash/purge`, `confluence space create`, `confluence comment create`, `confluence attachment upload/replace/download`, `confluence content-state set` | Pages, spaces, comments, files and content badges; full CRUD and API mapping in catalog |
| Documents | `design scaffold`, `design validate`, `design page-create`, `design page-update` | Agent-authored designs in shared pages |
| Plans | `plan scaffold`, `plan validate`, `plan render`, `plan page-create`, `plan page-get`, `plan page-update`, `plan review-status`, `plan tickets-preview`, `plan tickets-create`, `plan tickets-resume` | Shared planning, review, then ticket creation |
| Operations | `operation preview`, `operation get`, `operation wait`, `operation reconcile`, `operation list` | Typed no-write preview, per-step mappings, async jobs, partial/unknown outcomes |

General `jira issue create` is a public, policy-authorized command. The approved-plan gate applies to `plan tickets-create` and to any separately configured project policy requiring it. It does not prohibit unrelated authorized CRUD. Enforce organization-wide mandatory plan review in the provider or a trusted mutation service if bypass by other clients must be prevented. This supersedes the earlier internal-only issue-creation restriction.

There is no guaranteed standalone `ledger claim` command. Distributed claims require the configured mutation/lease authority. There is no raw-token export or unrestricted authenticated HTTP command.

## Authentication examples

Trusted initial setup, performed by a human or secure provisioning service:

```text
craft --profile factory auth provision --mode service --client-id CLIENT_ID --interactive
craft --profile factory auth login --mode service
craft --profile factory auth status --verify
craft --profile personal auth login --mode user --interactive
craft --profile personal auth login --mode user --interactive --no-browser
craft --profile factory auth doctor
craft --profile factory auth logout
craft --profile factory auth revoke-info
```

`auth provision --interactive` securely prompts for a secret without echo. A separate `--secret-stdin` mode supports a trusted provisioning pipe and cannot be combined with a secret value in arguments. User mode requires trusted broker configuration. `--no-browser` changes only how the human receives the broker URL; it is not a provider device-code implementation.

`auth doctor` reports backend availability without unlocking it or revealing stored secrets. Normal agent calls return an actionable error if interactive unlock/login is needed. `logout` removes the selected credential material locally; output explicitly reports whether anything was revoked remotely. The initial implementation promises local removal and mode-specific revocation guidance, not an undocumented provider revocation request.

## Plan input and human-visible page

The agent supplies content such as:

```json
{
  "schema_version": "1.0",
  "plan_id": "PLAN-INVOICE-01",
  "title": "Prevent duplicate invoice processing",
  "context": "Repeated delivery must not create a second ledger entry.",
  "review_state": "proposed",
  "target": {
    "confluence_cloud_id": "CLOUD_ID",
    "space_id": "SPACE_ID",
    "parent_page_id": "PARENT_PAGE_ID",
    "jira_cloud_id": "CLOUD_ID",
    "project_key": "PAY"
  },
  "tasks": [
    {
      "task_id": "T01",
      "title": "Define invoice-event identity",
      "issue_type": "Task",
      "description": "Document the identifier used to recognize event replay.",
      "acceptance": ["Producer and consumer agree on event identity."],
      "repositories": ["billing-api", "finance-ledger"],
      "depends_on": [],
      "include_in_jira": true
    },
    {
      "task_id": "T02",
      "title": "Reject replayed invoice events",
      "issue_type": "Task",
      "description": "Implement deduplication using the agreed identifier.",
      "acceptance": ["Replaying an event creates exactly one ledger entry."],
      "repositories": ["finance-ledger"],
      "depends_on": ["T01"],
      "include_in_jira": true
    }
  ]
}
```

This file stages the initial page. Once created, the visible reviewed page is the source of truth. Later edits to this file cannot silently replace reviewed content.

The page renderer produces ordinary readable headings and tables, for example:

```markdown
# Prevent duplicate invoice processing
Plan ID: PLAN-INVOICE-01
Review state: Proposed

## Context
Repeated delivery must not create a second ledger entry.

## Task T01
| Field | Value |
| --- | --- |
| Title | Define invoice-event identity |
| Jira project | PAY |
| Issue type | Task |
| Depends on | None |
| Include in Jira | Yes |
| Repositories | billing-api, finance-ledger |

### Description
Document the identifier used to recognize event replay.

### Acceptance criteria
- Producer and consumer agree on event identity.
```

T02 follows the same structure. Humans edit these visible fields, add review comments, and ask the agent to revise the page. Stable task IDs must survive revisions; ambiguous/missing IDs or unrecognized field shapes are validation errors. Do not resolve ambiguity by consulting hidden stale JSON. Page bodies retrieved from Confluence need an AST parser for the supported structure, not brittle regex replacement of arbitrary HTML.

Use normal published page status explicitly in the API adapter. Proposed/Approved are workflow meanings, not content lifecycle values to send as the page status. Include an HLD/detailed-design section or link separate design pages where needed. A review bundle must snapshot and hash any external design revision materially included in approval; a mutable live link is not a frozen decision.

## Review-to-ticket walkthrough

Proposed commands, with placeholder IDs/hashes:

```text
craft --profile factory plan validate --file invoice-plan.json
craft --profile factory plan page-create --file invoice-plan.json --operation-id plan-invoice-create-01 --apply
craft --profile factory plan page-get --page-id PAGE_ID
craft --profile factory plan validate --page-id PAGE_ID
craft --profile factory plan tickets-preview --page-id PAGE_ID --expected-version 7 --out ticket-preview.json
craft --profile factory plan review-status --page-id PAGE_ID --expected-version 7 --approval-comment-id COMMENT_ID
craft --profile factory plan tickets-create --page-id PAGE_ID --expected-version 7 --approval-comment-id COMMENT_ID --operation-id plan-invoice-v7 --apply
craft --profile factory operation get --operation-id plan-invoice-v7
craft --profile factory plan tickets-resume --operation-id plan-invoice-v7 --apply
```

The human review happens in Confluence between page creation/revisions and ticket creation. The preview supplies the exact page revision and digest for approval. A proposed explicit comment convention is `Approve PLAN-INVOICE-01 page PAGE_ID version 7 digest sha256:...`. This is an application convention, not native Atlassian approval syntax. The reviewer must post it through their own authorized account; the agent cannot manufacture a qualifying comment.

The CLI checks the comment's page, body and API identity metadata against trusted reviewer policy, not the provided comment ID alone. Edited/deleted/revoked approval must fail validation. If the available API metadata cannot establish eligibility, the implementation needs the trusted approval adapter instead. Confluence status badges, likes, or resolved comments alone are not approval evidence.

Hash the reviewed canonical plan representation and retained page snapshot using a versioned canonicalization rule. Bind both to the exact page version, target IDs, included design revisions, and Jira field mapping used for ticket creation. Any page version change requires renewed approval for a new creation attempt. Mutating mappings or targets also invalidates the approved mutation manifest.

Fetch the page again at apply time. Validate human edits, cycle-free dependencies, included/excluded tasks, permitted targets and live Jira metadata. No issue can be created from the original local JSON instead. Before each batch, recheck the source/approval; if changed, stop and return confirmed partial work. This narrows the race window but does not make cross-product operations atomic. A strict frozen-review mode needs page edit restrictions or a workflow freeze with verified permissions.

The creation result maps T01/T02 to actual issue IDs/keys, includes links/dependency outcomes, and links the approved source revision. Write a separate result page/comment rather than changing the reviewed task body during the run. A result-page write failure does not mean the created issues failed. Resume uses confirmed IDs and remote reconciliation, not blind replay.

## Result envelopes

Illustrative successful ticket creation:

```json
{
  "schema_version": "1.0",
  "ok": true,
  "command": "plan.tickets-create",
  "request_id": "req-001",
  "data": {
    "operation_id": "plan-invoice-v7",
    "source": {"page_id": "PAGE_ID", "version": 7, "digest": "sha256:EXAMPLE"},
    "status": "confirmed",
    "mappings": [
      {"task_id": "T01", "issue_id": "10001", "issue_key": "PAY-101"},
      {"task_id": "T02", "issue_id": "10002", "issue_key": "PAY-102"}
    ]
  },
  "warnings": [],
  "page": null
}
```

Illustrative refusal after review becomes stale:

```json
{
  "schema_version": "1.0",
  "ok": false,
  "command": "plan.tickets-create",
  "request_id": "req-002",
  "error": {
    "code": "PLAN_REVIEW_STALE",
    "message": "The approved page version is no longer current.",
    "retryable": false,
    "details": {"page_id": "PAGE_ID", "approved_version": 7, "current_version": 8},
    "next_action": "Review and approve the changed plan before creating more tickets."
  },
  "data": {"operation_id": "plan-invoice-v7", "mappings": []},
  "warnings": []
}
```

`--out` writes a non-secret payload artifact and returns its location/digest in the JSON envelope. Create files atomically with restrictive permissions where appropriate. An existing file requires explicit replacement. List envelopes use `data.items`, `page.returned`, `page.next_cursor`, `page.has_more`, `page.truncated`, `page.stop_reason`, and `page.consistency`; cursors are opaque. A short/empty page can still have continuation. Ordinary paging is distinct from byte-budget truncation. Optional total counts are absent by default. Field selection cannot remove mandatory paging/outcome metadata.

Do not mark partial workflow failure `ok: true`. Partial/unknown failures include confirmed mappings and recovery references in `data`. An async submission can succeed with `ok: true` and `data.state: accepted`; an explicitly pending poll is also distinct from completed work. The catalog defines bounded polling. Strip raw upstream response bodies from generic errors if they could contain sensitive information; return sanitized fields and request/trace IDs instead.

## Error and exit contract

| Exit | Categories | Agent response |
| --- | --- | --- |
| 0 | Completed, successful preview, or accepted async submission | Inspect operation state; accepted/pending does not mean remotely completed |
| 10 | INPUT_INVALID, FIELD_UNKNOWN, CURSOR_INVALID, CURSOR_EXPIRED, CURSOR_MISMATCH, CURSOR_NOT_FOUND, ITEM_TOO_LARGE, input/schema/config/target ambiguity, APPLY_REQUIRED, UNSUPPORTED_CAPABILITY, OUTPUT_BUDGET_TOO_SMALL | Correct input/configuration or select a compatible operation; use schema hints, narrower fields or an explicit traversal restart |
| 20 | AUTH_REQUIRED, AUTH_EXPIRED, AUTH_REVOKED | Request explicit operator authorization/provisioning |
| 21 | KEYRING_LOCKED, KEYRING_UNAVAILABLE, KEYRING_WRITE_FAILED | Resolve vault/session; never request a plaintext fallback |
| 30 | PERMISSION_DENIED, SCOPE_REQUIRED, POLICY_DENIED | Ask the designated owner; do not retry with a broader identity automatically |
| 40 | VERSION_CONFLICT, PLAN_REVIEW_STALE, REVIEW_REQUIRED, OPERATION_ID_REUSED | Reconcile source/review before another write |
| 50 | RATE_LIMITED | Wait at least reported retry_after_seconds within the caller's budget |
| 51 | WRITE_OUTCOME_UNKNOWN | Reconcile before retrying |
| 52 | PARTIAL_APPLY | Inspect mappings and resume the same operation after resolving the failed step |
| 60 | REMOTE_UNAVAILABLE, PAGINATION_STALLED, transport/internal failure | Respect retryability and operation semantics; do not loop a no-progress cursor |
| 130 | CANCELLED | Inspect journal if a write may already have been sent |

If a pipe closes, terminate cleanly without dumping credential-bearing errors. Exit codes are coarse classes; the JSON error code is the stable detailed contract. JSON schema should enumerate supported codes and preserve compatibility within a major contract version.

## Agent skill obligations

The default skill teaches targeted discovery, compact reads, strict file/stdin input, selected-profile/target checks, and safe recovery. Load only the applicable workflow reference. For plan-derived tickets, teach complete-plan preparation, published review pages, importing human edits, and actual review completion before creation. Writing an approval-looking field/comment is not completing human review. Ordinary resource CRUD does not require loading the planning workflow. The companion interface requirements include a proposed short entrypoint and packaging/evaluation rules.

After ticket creation, use the returned mappings for updates rather than guessing issue keys. On conflicts and unknown outcomes, reconcile. Never ask the user to paste tokens into chat. Stop on missing authority, invalid plan content, or unavailable secure storage, while preserving review progress and non-secret artifacts.
