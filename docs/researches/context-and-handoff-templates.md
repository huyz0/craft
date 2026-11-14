# Proposed context, artifact, and handoff records

These examples accompany the [study](context-artifacts-and-agent-communication.md). They are proposed records for a future factory, not installed tooling or native Rovo/Codex/Claude configuration. Names and identifiers are illustrative. References must be resolved and validated by the implementation.

## Task context

Keep the task small enough that a reviewer can connect each change to an acceptance criterion. Read scope and write scope are separate.

```yaml
task_id: FACTORY-142
attempt_id: FACTORY-142-run-03
parent_change_id: CHANGE-28
owner: billing-team
goal: Reject duplicate invoice events without duplicate ledger entries.
acceptance:
  - Replaying the same event creates exactly one ledger entry.
  - A distinct event for the same invoice follows the existing update rules.
non_goals:
  - Change the public event schema.
  - Upgrade dependencies or reformat other modules.
sources:
  - repo: billing-api
    commit: <resolved-full-sha>
    paths: [src/ingestion, tests/ingestion]
  - repo: finance-ledger
    commit: <resolved-full-sha>
    paths: [contracts/invoice-event.json]
  - document: ADR-023
    revision: <immutable-document-version>
read_scope: [billing-api, finance-ledger]
write_scope:
  billing-api: [src/ingestion, tests/ingestion]
required_checks:
  - <existing-ingestion-test-command>
  - <existing-contract-check-command>
scope_change_policy: Record the reason and affected interfaces before expansion.
completion_policy: Required checks and eligible review must validate this revision.
```

Do not treat this YAML as authorization. Credentials, filesystem boundaries, command policies, and merge/release gates must enforce the effective scope. Validate the checked-out commits against the record, and invalidate affected evidence when inputs change.

## Artifact manifest

Record one item per deliverable or evidence object. A run can produce many items; a task can have multiple runs.

```yaml
artifact_id: FACTORY-142-run-03-contract-report
task_id: FACTORY-142
producer_run_id: FACTORY-142-run-03
type: test-report
location: <durable-artifact-location>
digest: sha256:<actual-content-digest>
source_revisions:
  billing-api: <candidate-full-sha>
  finance-ledger: <resolved-full-sha>
validation_state: validated
validation_evidence: <checker-run-reference>
acceptance_state: pending-review
owner: billing-team
supersedes: FACTORY-142-run-02-contract-report
```

Store producer provenance from the execution platform separately from agent-supplied claims. A hash identifies bytes but does not prove that a trusted process produced them or that their contents are correct. Artifact access must not exceed the recipient's authorized data access.

## Agent request and response

Prefer narrow requests with evidence references. The coordinator owns changes to task scope and dependencies.

```yaml
message_id: MSG-301
correlation_id: FACTORY-142-run-03
from: billing-worker
to: contract-specialist
kind: question
question: Does this consumer treat a repeated event ID as a duplicate?
sources:
  - repo: finance-ledger
    commit: <resolved-full-sha>
    path: src/invoice-consumer.ts
allowed_actions: [read, analyze]
expected_response:
  - conclusion
  - source_locations
  - unresolved_questions
reply_budget_words: 200
deadline: <explicit-deadline>
```

The receiver verifies identity and allowed actions. A response does not grant the sender permission to change the consumer. Handle repeated message IDs idempotently where practical, and cancel or expire outstanding requests when the attempt ends.

## Human-facing result

Use a short result that exposes failures and pending decisions:

```text
Outcome: Implemented; awaiting independent review.
Change: <PR or candidate link and revision>
Checks: <passed checks with evidence>; <failed or unavailable checks>
Scope exceptions: <none, or explicit proposed expansion>
Decision needed: <specific question, owner, and consequences>
Artifacts: <design, report, and run links>
```

Detailed diagnostics belong behind those links. Avoid repeatedly posting the full plan or run transcript into the task discussion. Keep enough durable state that a fresh session can identify the current candidate, remaining work, and applicable decisions without reconstructing the entire chat.
