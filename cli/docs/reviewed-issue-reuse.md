# Recover through a newly reviewed existing-issue mapping

A replacement plan can explicitly reuse a known Jira issue instead of creating another one. This handles a changed plan containing already created work, or an uncertain earlier create for which a human selects an existing issue. The replacement needs fresh preparation, human approval, preview and a new run. The earlier run keeps its original preview and native journals.

First inspect the old run's progress, known mappings and native operation IDs. Use `operation get` and, where available, read-only `operation reconcile --saved` under the original Jira profile and recovery policy. A candidate's existence does not establish which operation created it. Without a selected existing numeric identity and usable original journal, keep the task unresolved.

In the replacement plan JSON, add `existing_issue` to the included task's `jira` mapping:

```json
{
  "project_key": "PAY",
  "issue_type": "Task",
  "existing_issue": {
    "issue_id": "11001",
    "origin_operation_id": "observed-earlier-create"
  }
}
```

Use actual observed identities. The plan renderer shows two task metadata rows: **Reuse existing issue ID** and **Original create operation**. Both must be present; a partial pair, duplicate issue/origin, excluded task or malformed identity is refused. Plans without the optional field retain their previous rendered and serialized form.

Publish the replacement as a normal plan page, using `plan page-create`, or have the human edit the shared plan through the supported editor. Use a new `review_id` for `review prepare`, obtain eligible human approval of that exact revision, then run `plan tickets-preview`. Standard task proposals and metadata remain required during preparation. The selected existing issue is read through the compiled Jira issue-get provider and must match the exact numeric ID, project and proposed issue type. These observations and the reuse decision are bound into review material; changed plan/selection/type/project cannot reuse old approval.

Apply the new preview with a deliberately chosen new `run_id`. This is an explicitly reviewed replacement workflow, not an automatic retry. Reuse every prior effect that the replacement intends to retain. Pending tasks without a reuse mapping still represent requests to create issues, so do not accidentally carry an uncertain old task forward as another create.

For each reused task, application stores a separate immutable local decision with the selected issue, complete original journal and new run/task binding. It checks that the original is a same-project create under the installed native descriptor, with one uncertain submission or matching confirmed resource and no job or contradictory evidence. Missing/foreign/refused or inconsistent originals stop the run. This step obtains no native create claim and sends no POST for the selected issue. Dependency linking and result-page publication keep their ordinary policies and approval checks.

Read `reused_issues` in the result, even when `complete` is true. Each entry contains the issue, original journal, decision artifact and `origin_attribution_unresolved`. Completion here means the newly approved ticket workflow completed. Reuse does not prove that the original operation created the selected issue or that the development work described by the task is done. The issue's contents are not updated from the create proposal. The result page and durable result/progress artifacts explicitly label reused identities and preserve unresolved original attribution.

Completed resume sends no HTTP requests. If the original journal changes after adoption, resume stops with the earlier mapping and latest original evidence retained. Inspect that conflict and obtain explicit reconciliation before continuing. Do not reset the original journal or replace the run's preview. Automatic candidate discovery, inferred adoption, native link/page adoption and distributed fencing remain unfinished. [Acceptance evidence](evidence/reviewed-issue-reuse.md) records tested behavior and limits.
