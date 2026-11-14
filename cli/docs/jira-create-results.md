# Jira create-result evidence

Craft has a pure Jira create-response adapter and an internal CLI completion host. They are not a registered cloud operation. Actual project/issue-type metadata, required/custom-field validation, authenticated operation policy, complete send/output composition, reconciliation and live qualification remain required before `jira issue create` can be exposed.

## Source and interpretation

The adapter follows the pinned Jira v3 `createIssue` response at `POST /rest/api/3/issue`: documented 201 `CreatedIssue`, optional `NestedResponse` values for transitions/watchers, and documented creation refusals. The exact source is [retained locally](../openspec/changes/build-cloud-cli/sources/jira-v3.json); [the provider inventory](provider-contracts.md) records its URL, source operation and SHA-256. This is source/fixture qualification, not a live tenant result.

The current Craft qualification rules are deliberately narrower than the native string schema. The original target must be a Jira Cloud project with a UUID cloud ID and a numeric project ID. A created issue ID must be a nonempty numeric string of at most 512 bytes. IDs are preserved exactly, including leading zeros; JSON numbers are refused. An optional key is bounded display metadata containing ASCII letters, digits, underscores or hyphens. These are Craft bounds, not claimed vendor maximums. Unsupported metadata never discards a separately qualified numeric issue ID.

| Native evidence | Normalized result |
| --- | --- |
| 201 with qualified ID and valid optional metadata | Confirmed issue resource under the original tenant |
| 201 with qualified ID but malformed key, URL field type, unknown property or secondary result | Known creation retained; response interpretation requires inspection |
| Failed secondary transition/watcher | Partial result with the created issue retained |
| Accepted or malformed secondary result | Secondary uncertainty; accepted is not completed |
| 201 without a qualified ID | Unknown creation; no inferred success or automatic retry |
| Documented 400/401/403/422 refusal | Failed creation with the corresponding input/auth/permission category |
| 429 or another unqualified status | Uncertain outcome; rate advice is data, not retry authority |

Returned `self` URLs never become routing authority or trigger a follow-up request. Free-text error/warning payloads are not copied into diagnostics. Secondary responses are normalized to confirmed/failed/unknown states; the original transport response remains available to the trusted host until completion.

## Completion and output

`CreateCompletion::prepare` takes the immutable operation binding, original request ID, field selection and output budget before a future send. It validates the Jira project target, compiles the projected response schema and checks a conservative ordinary success/recovery envelope bound. It does not open credentials or obtain a send claim. The host must call it before sending and must use the same bound operation and output controls.

`finish` reads immutable command, route, step and binding metadata from the actual `MutationAttempt`. The expected command is `jira.issue.create` and its native route/step are `create-issue`. It interprets the native response, records the qualified outcome through the existing claim and returns a complete envelope with the original request ID. No HTTP send occurs during completion.

The response keeps remote effects separate from local persistence:

- `receipt` contains operation identity, observed request state, known resource IDs and any retained job evidence.
- `persistence` is `recorded`, `unrecorded` or `uncertain`; revision is present only after a validated local acknowledgment.
- `secondary`, optional issue key, native status/request ID and rate advice describe bounded provider evidence.
- `reconciliation_required` remains true for partial, conflicting, unknown or unacknowledged results. `replay_authorized` is always false.

Projection always retains the receipt, target, step, persistence/revision and reconciliation/replay fields. Already recorded resources/jobs survive a failed final recording. A documented refusal conflicting with existing effect evidence remains uncertain. Additional resources/jobs on a nominally single-create result require inspection rather than a simple success claim.

A completion-binding mismatch preserves the actual attempt identity and any qualified effects while reporting the mismatch. An unexpected journal acknowledgment cannot claim the receipt was saved. A projection failure falls back to unprojected evidence. The common preserve-evidence encoder retains known effects if a host violates the preflight bound, including exceptional concurrent evidence growth. It reports the budget problem instead of dropping the receipt or suggesting no write occurred.

## Review boundary

The Jira crate depends only on core and ordinary serialization/schema libraries. Runtime uses it only as a development dependency for the TLS integration test; runtime production does not import provider semantics. CLI composition depends on both at the host layer. No provider registration, public parser action, generic authenticated HTTP command or unbounded native payload input was added.

Before activation, independently review response qualification, authoritative attempt metadata, partial effects, persistence acknowledgment, output preflight and binding mismatch handling. Then implement and qualify the complete create-input/metadata/policy/send/reconcile path. The current decoder and receipt tests do not prove project permission, issue-type compatibility, reviewer authority, SOX/SOC 2 control effectiveness or live provider support. See [observed evidence](evidence/jira-create-results.md) and the [HTTP review package](review-journaled-http.md).
