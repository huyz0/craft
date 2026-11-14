# Shared plan before tickets

Create a normal shared Confluence plan page for human review, then create tickets from approved material. A local draft, published page, validation or agent-written comment is not human approval.

Discover exact input contracts as each stage becomes relevant. The available workflow commands are `plan.scaffold`, `plan.validate`, `plan.render`, `plan.page-create`, `plan.page-get`, `review.prepare`, `plan.tickets-preview`, `plan.tickets-create` and `plan.tickets-resume`. These are command IDs, not shell invocations.

Prepare review context from the current visible plan, referenced designs and live Jira metadata. Humans review the actual plan and task breakdown before ticket creation. Preserve the returned review identity and immutable artifacts. A ticket preview verifies human review and freezes exact issue payloads, dependency batches and native link mappings. Its successful artifact is not permission to skip the controller's live checks.

Apply using the returned preview artifact name and an explicit run identity, result title and consistency mode from the schema. Read the complete artifact when assessing proposed work; counts and digests alone do not describe its content. Do not generate placeholder artifact names or infer dependency direction from endpoint names.

The controller checks original approvals and material equality for each unfinished batch, creates issues and links, and publishes a separate result child page. It does not overwrite the reviewed plan. Preserve confirmed mappings if a later step fails. Resume the same immutable run as described in [recovery](recovery.md). Changed-preview rebinding and operator completion reconciliation are unfinished; stop and report the specific unresolved step instead of inventing a repair command.

A fresh plan may explicitly reuse an existing issue through task jira.existing_issue: issue_id plus origin_operation_id. Both are visible in the rendered plan and bound into current human approval. Create proposals still supply desired material for review; reuse records a mapping to the selected native issue. Use a new review and run for changed plans, preserving original journals and known mappings. Report reused_issues and any unresolved original attribution.
