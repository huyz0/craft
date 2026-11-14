# Read Jira context

The development executable exposes eight context reads through the existing service-authenticated hosts:

| Commands | Local policy kind | Target |
| --- | --- | --- |
| `jira project list` | jira-project-read | site |
| `jira project get`, `jira project statuses` | jira-project-context | exact project ID or key |
| `jira field list`, `jira field search` | jira-field-read | site |
| `jira metadata edit`, `jira transition list`, `jira permission check` | jira-read | site, with explicit operation allowances |

Use installed `schema` for exact inputs, response fields and limits, and `capabilities --command <command-id>` for native scope alternatives. Each command requires configured service credentials, its separate enabled local allowance and actual server access. Discovery leaves unobserved policy/scope/live capability states unknown.

Examples after configuring the selected profile and policy:

```powershell
craft --profile jira-test jira project list --fields id,key,name
craft --profile jira-test jira project get --project SCRUM
craft --profile jira-test jira project statuses --project SCRUM --limit 10
craft --profile jira-test jira field search --query summary --limit 5
craft --profile jira-test jira metadata edit --issue SCRUM-1
craft --profile jira-test jira transition list --issue SCRUM-1
craft --profile jira-test jira permission check --issue SCRUM-1 --permission BROWSE_PROJECTS
```

Follow opaque continuation with unchanged context, filters and projection. Metadata brackets the current issue observation but is not an atomic snapshot or write authority. Transition IDs and permission observations do not enable writes. [Issue edits, assignment, deletion and transition apply](jira-issue-writes.md) have their own public commands and independent mutation policies.

For project listing or field reads, create an explicit site policy through `policy set --input ...` under the service profile. A project-site input looks like:

```json
{
  "kind": "jira-project-read",
  "target": {
    "provider": "jira", "deployment": "cloud",
    "tenant": "00000000-1111-2222-3333-444444444444",
    "resource_type": "site", "id": "00000000-1111-2222-3333-444444444444"
  },
  "expected_actor": "actual-service-account-id",
  "enabled": true,
  "operations": ["jira.project.list"],
  "continuation_site": "https://craft-test.atlassian.net",
  "expected_revision": null,
  "apply": true
}
```

Replace the illustrative Cloud ID and actor with observed values. The continuation origin is explicit trusted configuration for validating native continuation, not an arbitrary authenticated HTTP destination. It must be an accepted Jira site origin; the setter canonicalizes its spelling. `jira-field-read` uses the same site shape with explicit `jira.field.list` and/or `jira.field.search` operations.

After the project-site record exists, configure `jira-project-context` with a project target, the same actor, `jira.project.get` and/or `jira.project.statuses`, and the current **whole site-record revision**. Omit `continuation_site`; the existing site's identity/origin remain bound. Project keys are exact and case-sensitive. A context update preserves siblings and listing enablement. Disabling listing does not revoke separately enabled project context; disable each context explicitly when that is intended.

Project-context get/set returns the full shared site-policy snapshot: its site target, revision, listing `enabled`, origin and `context_reads` entries. The outer target is the project requested for administration. Inspect the matching context entry for its enablement; the top-level enabled flag describes listing. A context may be absent even though the site policy exists. Preserve all records in recovery; a lost local commit acknowledgment is uncertain, not proof of rollback. Actual precommit output reservation includes retained sibling entries.

These public adapters have synthetic/native-host qualification and independent review recorded in [activation evidence](evidence/jira-context-activation.md). Live tenant credentials and effective permissions remain unqualified.
