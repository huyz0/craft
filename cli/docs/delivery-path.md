# Deliver the first cloud workflow

The next priority is a usable flow: connect to the test site, read shared context, publish a normal Confluence page containing the plan draft, let a human review that exact revision, then create the reviewed Jira tickets with recoverable mappings. The full scope in the [proposal](../openspec/changes/build-cloud-cli/proposal.md) and [task ledger](../openspec/changes/build-cloud-cli/tasks.md) remains unchanged. This sequence does not declare the remaining commands or platforms delivered.

## Current gap

On 11 November 2026 the public development executable has 75 commands and two cloud providers. Secure provisioning, policy setup, Confluence space/page reads and page creation, Jira issue reads, actions/reconciliation, directed link lifecycle and [Jira context reads/setup](jira-context.md) are public after local qualification and independent review. [Current activation evidence](evidence/jira-context-activation.md) identifies the latest executable/checks; the [quickstart](cloud-quickstart.md) shows usage. [Reviewed ticket application/resume](ticket-application.md) is also public after local synthetic qualification. Live credential/site qualification, page-update editor behavior and automatic candidate discovery and distributed coordination remain unfinished. Public exact-revision verification retains durable local approval evidence. Sixteen of eighty-seven ledger tasks are complete. This count is not a percentage of engineering effort or product readiness.

Repeated infrastructure checkpoints have not produced the requested user flow. Use these outcomes to order the remaining implementation. Keep completion status and evidence in the existing ledger rather than adding another checklist.

| Delivery outcome | Existing work to finish | Evidence the user can assess |
| --- | --- | --- |
| Read shared cloud context | Confluence space/page reads and search, existing Jira context, service provisioning/access qualification | Actual CLI input and bounded output for the selected test site |
| Publish the plan draft | Normal Confluence page create/get/update using the existing renderer and journal | A visible page, returned identity/version and conflict handling when content changes |
| Review the exact plan | Reviewed-plan publication/import and independent reviewer evidence | The reviewed page revision/digest and refusal when the plan or review evidence changes |
| Create reviewed tickets | Existing Jira create host, plan mapping/materialization and resume | Jira identities mapped to approved entries, retained partial/unknown outcomes and no blind duplicates |
| Package the usable flow | Required independent review, public registration, live qualification and local packaging | An identifiable executable, working discoverable commands and a short usage guide |

## Implementation sequence

Public registration and independent activation review are complete for the commands listed above. Live source/design/native field binding and immutable context are now available through `review prepare`. `review verify` now combines that preparation with native complete inventories and durable local evidence. `plan tickets-preview` now retains exact approved payloads, batches and native link mappings. The native journaled `jira link create` host is now public with explicit endpoint policy. [Link inspection, deletion and reconciliation](jira-link-lifecycle.md) are now locally qualified. The [ticket controller](ticket-application.md) now performs reviewed batches and independent result recovery. Use explicit reviewed issue reuse for a selected existing issue in a fresh plan/run, preserve unresolved original attribution, and qualify service-account access on the test site. Keep administrative, attachment, broker, distributed-authority and future-provider work in the full scope, ordered after the central workflow where dependencies permit.

Do not replace missing functionality with documentation or count synthetic success as live qualification. Run checks appropriate to changed behavior, preserve failure evidence, and avoid repeating unchanged full gates. Existing independent-review and credential controls still apply; prepare a concrete reviewable integration before requesting human action.

Report progress as working user outcomes, remaining gaps and specific external dependencies. Test counts support tested behavior; they do not prove CLI delivery.

The current binary, contract and skill also have an [isolated local development bundle](local-install.md). This provides exact archive/binary identity and a checked local installation without changing PATH; host skill loading, live access, notices/signing, upgrades and release qualification are still open.

Confirmed issue completions retained from failed local outcome publication now have an [explicit local import and resume procedure](ticket-completion-recovery.md). Unknown-write adoption and replacement-preview recovery remain separate unfinished work.

[Newly reviewed issue reuse](reviewed-issue-reuse.md) now handles selected existing issues in replacement plans, including an uncertain original create whose attribution stays explicit. It requires fresh human approval and a new run; the original run and native journals remain intact.

[Jira comment CRUD](jira-comments.md) is locally qualified with typed ADF/visibility, original-author own/any allowances, fingerprint checks and journaled no-replay receipts. Comment-specific live reconciliation and tenant qualification remain unfinished.

[Jira attachment get/list/settings](jira-attachments.md) now return bounded inert metadata and upload settings with separate read allowances. Binary transfer and attachment writes remain unfinished.
