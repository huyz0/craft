# Verify current human review

After [preparing review context](review-preparation.md), an eligible human reviews the complete artifact and posts the returned statement on the plan page using their own account. Run:

```powershell
craft --profile cf-test --max-bytes 1048576 review verify --review-id payment-plan --apply
```

Use the same explicit Confluence profile and review ID used for preparation. Existing enabled reviewer, page/comment-read and per-project create-preparation policies must still match. `--apply` authorizes an immutable local audit artifact. The command performs native reads and credential-cache maintenance; it creates no tickets or review comments. Response projection is unavailable because effect receipts must remain complete.

Verification rebuilds the live plan, design and Jira field mapping, compares the prepared binding and reviewer configuration, and observes two complete current root/reply comment inventories. Eligible distinct humans must meet quorum without unresolved changes requests. Current parent GET metadata must agree with listed comment revisions. Missing, unreadable, edited or stale evidence cannot establish approval. Observation and publication must finish within the original 30-second wall/monotonic window; policy, execution identity and invocation controls are rechecked.

`review_verified: true` reports the completed observation and confirmed local evidence publication. The response includes approval counts, binding digest, preparation/verification receipts and credential-cache effects. Inspect `verification_artifact.local_path` for the full audit record. Confirmed or unknown local effects survive a later refusal, while approval fields reset to false/zero.

The artifact name binds review ID and invocation request ID. Reusing that request ID returns an existing receipt and conflict without verifying again. A serialized artifact cannot restore approval authority. Future ticket creation must freshly revalidate the original approving comment IDs, versions, actors and fingerprints before each batch. These observations do not lock Confluence and Jira atomically.

[Ticket preview](ticket-preview.md) now records a complete approved manifest using this same live verifier. Ticket creation/recovery and live-tenant qualification remain unfinished. See [verification evidence](evidence/review-verification.md).
