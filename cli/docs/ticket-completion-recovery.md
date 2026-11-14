# Restore a retained confirmed issue completion

`plan tickets-record-completion` repairs one local evidence gap: the original issue write returned a full confirmed completion, but the controller could not save its outcome artifact. It imports that exact retained envelope into the same run. It contacts no provider, opens no credential store and changes no native journal. Resume is a separate command.

Save the original `current_attempt` Envelope from the failed controller response into a regular JSON file inside the current project. Save the nested envelope itself, including its command, request identity, data and warnings. Do not use the entire outer controller response, a digest, an issue-search result, a partial projection or a reconstructed success envelope. The import validates consistency with local confirmed evidence; it cannot prove the supplied envelope came from Jira.

Create `record-completion.json` with the unchanged original run arguments, the observed task identity and the retained file name:

```json
{
  "run_id": "approved-delivery",
  "preview_artifact": "tickets-0000000000000000000000000000000000000000000000000000000000000000.json",
  "result_title": "Approved ticket results",
  "consistency": "revalidate_each_batch",
  "task_id": "implement-payment",
  "completion_file": "retained-native-completion.json",
  "apply": true
}
```

Replace example identities with the original observed values. Use the original explicit Confluence run profile and private state root:

```powershell
craft --profile confluence-test --max-bytes 16777216 plan tickets-record-completion --input record-completion.json
craft --profile confluence-test --max-bytes 16777216 plan tickets-resume --input ticket-run.json
```

`apply` authorizes immutable local publication only. The fixed output budget reserves complete exceptional journal/artifact evidence; normal output stays compact. The completion file uses the existing bounded project-file reader, rejecting links, traversal, special files, oversize data and duplicate JSON members. It observes cancellation/deadlines and is parsed before private state opens.

The existing run intent, preview receipt/digest and task-derived step intent must match. The native journal must have exactly the confirmed issue-create step, its original full binding, one qualified issue identity and no remote job. The full native completion must agree on operation, target, resource, journal revision, recorded persistence and successful HTTP outcome, with no replay or unresolved secondary effect. Unknown/refused writes, missing envelopes and incomplete receipts cannot use this command.

An identical import returns the same immutable outcome receipt. A changed existing outcome is refused. If publication acknowledgment is uncertain, retain the returned outcome name and publication state and inspect before retrying. If the journal changes after publication, the command reports conflict while retaining the published artifact receipt and both journal observations. Do not delete artifacts or reset the native operation.

Successful import does not refresh approval or declare the run complete. Ordinary resume revalidates original human approval and current policies for unfinished work; it skips the recovered confirmed create. Unknown-outcome adoption, controller link/page completion imports and replacement-preview rebinding remain unfinished. See [observed acceptance](evidence/ticket-completion-recovery.md).
