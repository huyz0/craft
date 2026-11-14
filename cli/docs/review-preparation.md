# Prepare a plan for human review

`review prepare` reads a normal Confluence plan page, verifies its included design references and qualifies explicit Jira field proposals. It records a complete immutable local review artifact. The response contains its receipt and a compact approval statement; `prepared: true` means preparation succeeded, not that anyone approved it.

Use an explicitly named Confluence service profile with enabled reviewer configuration and page-read allowance. Each task proposal names a Jira service profile whose configured create policy permits the exact project preparation. Required OAuth scopes are native read scopes, including current-user probes; this command does not need Jira write scope.

Save this non-secret input as `review.json`, matching identities and titles from the visible plan:

```json
{
  "review_id": "payment-review",
  "apply": true,
  "page_id": "101",
  "space_id": "100",
  "tasks": [
    {
      "task_id": "define-contract",
      "profile": "jira-test",
      "project_id": "10000",
      "input": {
        "issue_type_id": "10001",
        "summary": "Define the contract",
        "description": "The exact proposed Jira description.",
        "custom_fields": {}
      }
    }
  ]
}
```

```text
craft --profile cf-test --max-bytes 1048576 review prepare --input review.json
craft schema --command review.prepare --view full
```

The larger byte allowance reserves recovery capacity; normal output stays compact. `--apply` authorizes the local artifact publication. It does not apply a Jira mutation. Complex proposals work through `--input`; repeated `--task '<JSON object>'` flags are also supported. For plans with included dependencies, also provide an explicit native `dependency_link_type_id` and `dependency_link_direction`; see [ticket preview](ticket-preview.md) for label and endpoint semantics. For designs in another site, supply `design_profiles` as a JSON object mapping that cloud ID to its configured Confluence profile.

Open `artifact.local_path` and inspect the full plan, designs, exact Jira payloads, defaults and mapping. Descriptions/custom fields are explicit proposals: compare them with the source plan rather than assume an automatic rich-text conversion. Only after human review should an eligible human publish the returned statement as one JSON paragraph or code block on the plan page using their own account. This command posts no comment and verifies no approval. Use [review verification](review-verification.md) to check current human approval and retain local evidence. [Ticket preview](ticket-preview.md) now records the approved payloads, batches and links. Ticket creation/recovery remains unfinished.

An existing review ID is never overwritten. Refusal returns its existing artifact receipt for inspection and no new statement. On an uncertain publication, retain the artifact name and effect state; inspect that identity before using another review ID. A local review pack is data and cannot restore verified authority. See [acceptance evidence](evidence/review-preparation.md).

## Binding and authority

Read the live Confluence plan and all referenced designs, then qualify explicit per-task Jira create proposals through the existing native metadata compiler. A proposal supplies a task identity, configured Jira profile, exact project ID and the existing strict create input. Included task identities must match exactly; task title, project key and issue-type name must agree with the visible plan and native identities. Descriptions/custom values are explicit proposals for human review, not an implicit lossy conversion of rich plan content.

The configured create policy already governs the same metadata reads before issue creation. Preparation uses that read phase only: no apply to a Jira mutation, journal claim, frozen-call submission or POST. Full captured policies, grants, actors, targets and scopes are checked at native boundaries. Service credentials stay behind native grant locks. The Confluence reviewer policy must be enabled and match the page's space and authenticated service actor before remote reads.

Design references use their native page version and canonical semantic design document digest. Source artifacts also retain the raw storage body and native revision fingerprint. Different Confluence sites require explicitly named service profiles, and unused profile overrides are refused.

The material mapping digest contains exact native ticket payloads, project/type identities and field metadata sorted by field ID. Observation timestamps and local profile/grant identities are excluded from this portable content digest. Reported native defaults are retained; a declared default that will be used but is not reported causes refusal. The snapshot makes no guarantee about server automation after a future write.

Review preparation records an immutable local artifact with full sources, proposals, qualified payloads and a compact structured approval statement. Its JSON is data, not verified-review authority. A later verifier must rebuild the binding from fresh native reads and use two complete inventories under the existing reviewer policy. Public preparation requires explicit apply for local artifact publication; it makes no remote resource write and grants no ticket authority.
