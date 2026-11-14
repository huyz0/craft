# Preserve effects during recovery

Classify the error from its structured evidence before retrying. Correct malformed local input against the installed schema. For read failures, follow retry/continuation evidence without treating missing data as an empty result. For policy/auth refusal, report the missing boundary; do not change actor or broaden policy as a retry.

After a mutation timeout, interrupted process, lost response or persistence failure, retain the original operation/run identity and all known resources. Inspect local evidence using the actual operation ID:

```sh
craft operation get --operation-id existing-operation
```

If the identity is lost, `craft operation list --unresolved-only` pages this state root's records that still need reconciliation. This command uses the original state root and rejects `--profile`; it reads saved bindings without selecting credentials. It is local evidence, not live verification or replay authority. `sent_unknown`, reconciliation required, or a confirmed resource without complete completion evidence means stop the write and reconcile. Do not replay the POST, reset state, delete a journal or create a new identity. An unreadable journal cannot establish that no write happened. Report the known resource identities and the unresolved operation without exposing credentials.

Standalone issue writes and reviewed-controller issue-create steps retain a private canonical request before native acquisition. `saved_request` reports its name, digest and local publication state separately from the native receipt. Null means no source was observed in this response, not proof of absence. Unknown source publication stops the new attempt; inspect it under the same operation ID.

For those captured Jira writes, read-only native investigation uses the original service profile and separate recovery policy:

```sh
craft operation reconcile --operation-id existing-operation --saved
```

For older records and controller link/page work without a supported saved source, use an explicit exact original canonical native request through structured input. Explicit original and `--saved` are exclusive; never reconstruct an original from a digest or substitute the CLI argument object. Create-candidate IDs must be explicit known numeric identities. A successful investigation can remain unresolved; matching current values or a later 404 never proves attribution, changes the journal or authorizes replay. For a changed plan or an uncertain create with a human-selected existing issue, use a fresh reviewed plan and new run with explicit reuse; old run previews remain immutable.

For reviewed issue steps, investigate the reported native operation ID with `--saved` under its original Jira profile and recovery policy. This never completes the run or resolves unknown attribution. For reviewed ticket runs, use the same profile, state root and unchanged run input:

```sh
craft plan tickets-resume --input ticket-run.json
```

The controller refuses replay of confirmed, refused and unknown native claims. A completed resume sends no HTTP requests. Result-artifact retry is local; recoverable result-page publication does not recreate tickets. Inspect `known_issues`, current operation/attempt, unresolved records and progress-artifact receipts. Changed source/approval or payload material requires human reconciliation and new review; the existing run cannot be rebound to a replacement preview. Do not promise automated rollback or cleanup.

If the original write returned an authentic full confirmed issue completion but its local outcome could not be saved, `plan tickets-record-completion` imports that retained Envelope. Use the unchanged run arguments, observed task_id, completion_file under the current project, explicit profile, apply and max-bytes 16777216. Read its installed schema. This is local evidence publication only; it rejects unknown/refused/partial completions and cannot prove provider authenticity. Resume separately with the original run input and approvals. Never fabricate the missing envelope.

To reuse a known issue, set the visible plan task jira.existing_issue to {"issue_id":"11001","origin_operation_id":"observed-earlier-create"}, using observed identities. Its rendered paired rows must be reviewed by eligible humans through new preparation/approval/preview. Ordinary task proposals remain required. Apply that preview as a new run. Reuse checks issue identity/project/type and original create evidence, stores a separate decision and never changes original attribution. Read reused_issues and origin_attribution_unresolved even when the new workflow completes. A missing candidate/origin or contradictory origin needs investigation; do not turn it into another create.

For `jira.attachment.download`, retain artifact_name, artifact_write_state, export_write_state and both receipts independently. Export failure can leave a complete private artifact; confirmed local publication remains real after policy/cancellation refusal. Unknown publication requires local inspection under the same state root before another transfer. Do not refetch under a new name or infer absence from a null receipt. Use `artifact.get` with the exact artifact_name and collection (downloads by default) to verify saved bytes locally. Recover export with `artifact.export`, the observed expected_digest and an explicit output_root/output. Neither command accepts a profile or grants cloud replay authority. Missing local data does not resolve a remote write outcome.
