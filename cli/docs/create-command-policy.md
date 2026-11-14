# Candidate create command and private policy

The internal Jira create command now connects typed arguments, profile/project selection, private policy and the [cached create host](jira-create-host.md). Its parser branch is skipped and its candidate descriptor is not registered at startup. The public executable rejects `craft jira issue create`; these shapes and APIs are contributor/review material.

## Arguments and context

The structured argument shape is:

```json
{
  "operation_id": "create-report",
  "apply": true,
  "project_id": "10000",
  "issue_type_id": "10001",
  "summary": "Reviewed implementation task",
  "description": "Literal task description",
  "labels": ["factory"],
  "custom_fields": {}
}
```

Operation ID, issue type and summary are required. Apply defaults false. Scalar fields have typed candidate flags; label arrays and custom-field maps use structured input and are marked `json_only` in the candidate schema. Optional parent/account IDs use the existing positive metadata qualifications. Unknown fields and credential/policy arguments are refused.

Omission, explicit null and supplied text remain distinct. A description flag containing `null` means literal text. JSON null remains null for field qualification and is never silently dropped. Null project/parent inputs cannot select a fallback; explicit unassignment stays unsupported. Required/local semantic validation, projection and maximum-target recovery output preflight run before state is opened.

Global profile selection remains context, not authority. An explicit profile bypasses repository hints and uses the explicit project or its configured default. Otherwise the current directory's hints and active-profile fallback retain existing precedence; project resolution uses explicit, then loaded repository, then profile default. There is no first-profile fallback. The selected profile must be Jira Cloud service mode, and the project must exactly match its separately configured policy.

## Private policy records

`ProfileStore::create_policy`, `set_create_policy` and `verify_create_policy` operate only on the selected installation's private SQLite state. No public policy-administration command exists. One record per profile binds installation, profile, issued grant, exact project, expected actor, enabled state and revision. Separate profiles sharing a grant can have separate project policies. Missing records do not enable anything.

Trusted-host administration requires explicit apply. An absent expected revision creates only; an existing revision updates only that exact record. Validation and caller output preflight precede SQL writes. Immediate transactions serialize competing writers. A confirmed update returns its record; an uncertain commit requires reading that same policy before another update. Repository hints and normal create arguments cannot change policy.

Opening state upgrades SQLite schema one through two to **three**, adding `create_policies` while preserving settings, profiles, grants, preferences, cursors and operation records. Settings retain format version one; the operation-record format is unchanged. Exact policy-table schema and absent triggers are checked. Foreign/conflicting tables or unsupported versions are refused without repair; migration failure rolls back intermediate schema changes. Older binaries that support only earlier database versions refuse the newer state. No downgrade path is supplied.

## Execution and changing policy

`PreparedInvocation` resolves and snapshots an enabled policy, then constructs the existing host with its exact grant/actor/target. It never derives authority from task content or command input. Before cached acquisition, before prepared resource contexts and at the final claimed-send callback, it rereads the same policy. A disabled policy returns policy denial; a different enabled revision returns version conflict. Verified service actor mismatch still blocks resource operations after identity verification.

The final policy read follows token and metadata freshness checks, immediately before the transport's last invocation-control check and HTTP polling. Refusal after a durable claim retains an unknown marker with no POST. Cache-update receipts remain separate from issue effects, and command dispatch returns the complete handler envelope and original request identity through normal evidence-preserving output.

This is a local pre-send check, not distributed fencing or cancellation of an in-flight HTTP request. A policy change after the final read cannot retract a request already being polled. Private state is not tamper-proof against code running as the same OS principal; see [native boundaries](native-safety.md). Local enablement does not establish live service eligibility, remote permissions, independent human approval or SOX/SOC 2 control evidence. Immutable policy history/snapshots are not implemented; current issue receipts bind actor/grant/target rather than retaining a complete policy audit history.

Public activation still requires consequential review, live/public acceptance and recovery qualification. See [observed evidence](evidence/create-command-policy.md) and [review scope](review-jira-create-host.md).
