# Explicit writes

Discover the exact operation and its schema. Availability, effect and apply behavior vary by command; not every planned CRUD/admin/attachment operation is shipped. A successful input validation is not a dry run of cloud effects.

Use the already authorized target and scope. Configure credentials and policies through supported setup, never by weakening a refusal or swapping identities. Keep grant, profile, state root and operation identity stable. Obtain new human authority only when the requested effect or scope requires it.

For commands with `apply`, inspect the non-applying result first when useful, then apply the exact intended input. Do not assume every command defaults to preview: read its installed contract. Some plan commands use `apply` to publish a local artifact and do not write cloud resources. Distinguish local artifact confirmation from remote completion.

Preserve the full receipt, confirmed resource identities, persistence disposition and reconciliation evidence. A success-looking resource field does not override an error, missing completion record or unknown send. Read [recovery](recovery.md) after a failed or interrupted mutation. Do not create a fresh operation ID to bypass a refusal or uncertain attempt.

Jira comment writes need an explicit profile, project/issue IDs, apply, operation ID and `--max-bytes 8388608` for complete recovery receipts. Discover `jira.comment.create`, `jira.comment.update` or `jira.comment.delete`. Create requires typed ADF and explicit visibility mode; updates preserve omitted body/visibility and reject null. Use a fresh read fingerprint for update/delete. Own intent checks original author; any intent needs separate local and native all permission. Revision checks are non-atomic. Retain acknowledged IDs even when desired fields are unverified; no comment-specific live reconciliation command is available.

`jira.attachment.download` reads cloud content but writes local files immediately. Supply an explicit immutable artifact_name, existing output_root and relative output; native filenames grant no path authority. Byte limits and optional expected_digest bind the transfer. Export creates a new file unless overwrite is explicitly true; the complete private state root is excluded. It needs its own Jira read allowance. Binary bytes never appear on stdout. Read both local publication states before retrying.

`artifact.export` immediately writes a local output from a saved downloads, requests or reviews artifact. Bind expected_digest to the observed receipt. It does not use a profile or cloud access and never refetches. Preserve both source and export receipts; inspect uncertain output before another publication attempt.
