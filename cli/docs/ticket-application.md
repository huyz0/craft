# Apply and resume reviewed tickets

`plan tickets-create` applies a successful [ticket preview](ticket-preview.md), creates its approved issues in dependency order, links confirmed issue identities and publishes a separate child result page under the reviewed plan. `plan tickets-resume` continues the same run. Both use the exact immutable preview and original approvals; neither replaces the reviewed plan page.

Before applying, configure the original Confluence service profile with review/page/comment access and result-page create policy for the source space. Each Jira profile named by the preview needs an explicit create policy for its exact project. Dependency linking needs `jira.link.create` mutation allowances for both endpoint projects under the **outward task's profile**, with the same linking actor/grant. Native provider scopes and permissions remain required. See [policy setup](policy-setup.md) and [link direction](ticket-preview.md).

Save `ticket-run.json`, using the actual `verification_artifact.name` returned by the successful preview:

```json
{
  "run_id": "approved-delivery",
  "preview_artifact": "tickets-0000000000000000000000000000000000000000000000000000000000000000.json",
  "result_title": "Approved ticket results",
  "consistency": "revalidate_each_batch",
  "apply": true
}
```

```powershell
craft --profile confluence-test --max-bytes 16777216 --timeout-ms 120000 plan tickets-create --input ticket-run.json
craft --profile confluence-test --max-bytes 16777216 --timeout-ms 120000 plan tickets-resume --input ticket-run.json
```

The preview name is an artifact name under the selected private state root, not an arbitrary file path. `run_id`, preview, result title, profile and consistency mode are pinned by the run intent. Keep them unchanged for resume. Repeating `tickets-create` for an existing run refuses before authentication. A new run ID represents another independent application, so do not generate one as a retry. The consistency literal is `revalidate_each_batch` in both flags and JSON.

The output budget reserves capacity for exceptional full receipts/journals; normal output stays compact. Projection is unavailable because partial-write evidence must remain complete. `known_issues` retains confirmed native identities even when later work fails. `resources_complete` means issue and link steps completed; `complete` additionally requires result publication. Artifact receipts contain local paths for the full durable records. `progress_artifact` records step outcomes, native operation IDs and failures. Each acknowledged dependency means the requested relation was confirmed, including possible preexisting duplicates; no new-link attribution or native link ID is invented.

For every batch with unfinished work, Craft rebuilds live source/design/native metadata context and verifies the original human approval identities, versions, body fingerprints and policy. A newer approval cannot substitute for the original preview's set. Fresh issue payloads and metadata, including defaults/configuration and option meaning, must still match. The batch's original observation window and current captured policies remain checked through native sends. These checks bound races; they do not lock Confluence and Jira atomically.

Native steps have deterministic operation identities and exact durable bindings. Confirmed, refused and unknown claims do not replay. If a result artifact fails, resume can republish it without any native request. If result-page metadata/access fails before a send claim, resume can publish that page without revalidating or recreating already completed tickets. An exact confirmed page journal can recover the result-page identity without another POST. Result content is stable across attempts, and publication does not overwrite human planning content.

Each new issue-create step retains its canonical original before locking credentials for that native step. Its `current_attempt.data.saved_request` receipt reports source publication separately from the native write. If source publication fails, that step stops before native access. For an uncertain issue-create, use `operation reconcile --saved --operation-id <current_operation_id>` under the original Jira profile and explicit recovery policy. The source grants no approval or replay rights and investigation cannot complete the run. Controller link/page sources and older runs without saved originals remain gaps. See [controller source evidence](evidence/controller-request-source.md).

An unknown issue/link attempt stops for reconciliation. Inspect `current_operation_id` with `operation get`, plus `current_attempt`, `unresolved_record` and the progress artifact. A confirmed issue whose full completion record is missing remains a known mapping and stops. If the original full confirmed `current_attempt` envelope was retained, [record that completion](ticket-completion-recovery.md) locally, then resume the unchanged run. Without that envelope it remains unresolved. Unexpected saved jobs/resources cannot be silently treated as workflow success. A changed source, original approval or material mapping requires human reconciliation and a new review. [Explicit reviewed issue reuse](reviewed-issue-reuse.md) lets a fresh plan and new run retain selected existing issues with original-operation evidence; resume keeps the old run bound to its original preview. No cleanup automatically deletes created resources.

The workflow has synthetic native TLS acceptance and independent source review. Actual tenant credentials/permissions, live editor behavior, other platforms and automatic adoption and distributed coordination remain unfinished. See [evidence](evidence/ticket-application.md).
