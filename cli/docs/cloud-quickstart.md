# Use the Windows development executable

From `cli/`, use `target/debug/craft.exe` after `cargo build -p craft-cli --locked --offline`. This is a development build, not a packaged release. It exposes 75 commands and Jira/Confluence Cloud provider manifests. Live access to the test site is not yet qualified.

## Discover before supplying input

```powershell
$craft = '.\target\debug\craft.exe'
& $craft command find --query confluence
& $craft schema --command confluence.page.create --view full
& $craft schema --command jira.issue.create --response --view full
```

Plan/design publication is also available through `plan page-create` and `design page-create`, with current-page import through `plan page-get` and `design page-get`; see [planning usage](planning.md). Separately allowed [footer-comment get/list/replies](confluence-comments.md) supply current version metadata and bounded root/direct-reply pages. Public cloud commands include Confluence space/page reads and page creation, Jira issue get/search/create, [issue actions and reconciliation](jira-issue-writes.md), [directed links](jira-link-create.md) [Jira comment CRUD](jira-comments.md) and [Jira attachment upload, download and delete](jira-attachments.md). Confluence page update/comment writes and broader administration remain unavailable. An implemented capability does not prove credentials, configured policy or live permission.

[Jira context reads](jira-context.md) include project list/get/statuses, field list/search, edit metadata, transition listing and current-user permission checks. Their local policy setup is public; discovery does not grant permissions or activate mutations.

## Connect a service identity

Create service profiles using the site's Cloud ID. Share the first profile's issued grant only when both profiles should use the same service identity.

```powershell
& $craft profile create --name cf-test --provider confluence --deployment cloud --auth-mode service --tenant '<cloud-id>'
& $craft profile create --name jira-test --provider jira --deployment cloud --auth-mode service --tenant '<cloud-id>' --grant '<issued-grant-id>'
& $craft auth provision --profiles cf-test,jira-test --expected-actor '<service-account-id>' --interactive
```

Enter the service-account OAuth client ID and secret only through the secure local prompt. Browser sign-in and personal API tokens do not provision this authentication mode. Creating an account, credential or changing tenant access is a separate administrative action.

Configure exact local allowances with [policy setup](policy-setup.md). Administration requires explicit `--profile`; repository hints cannot select its authority. Set only operations needed for the chosen site, space or project. An allowance for an unavailable command does not activate it.

Operator-managed [reviewer configuration](reviewer-policy.md) is available through `review policy get/set`. It does not establish approval; use [review verification](review-verification.md) after a human reviews the prepared context. [Ticket application/resume](ticket-application.md) now materializes an immutable preview; live qualification and changed-preview reconciliation remain unfinished.

## Read context and prepare a page

```powershell
& $craft --profile cf-test confluence space list --fields id,key,name
& $craft --profile jira-test jira issue get --issue SCRUM-1 --fields id,key,project.id,issue_type.id
```

Use `schema --command <name> --view full` for exact argument and selectable-field definitions. Jira response projections use nested names such as `project.id`; create input uses `project_id`. Use `--input FILE` for structured non-secret JSON. Output controls such as `--fields` remain flags. Follow returned cursors rather than inventing offsets.

A page-create input file contains:

```json
{
  "operation_id": "plan-page-example",
  "apply": true,
  "cloud_id": "<cloud-id>",
  "space_id": "<space-id>",
  "title": "Plan for human review",
  "placement": {"kind": "root"},
  "body": {"representation": "storage", "value": "<p>Proposed work for review.</p>"}
}
```

After reviewing the target and content, run `& $craft --profile cf-test confluence page create --input page-create.json`. This creates a normal shared page. It does not record human approval or create tickets. Retain the complete receipt and operation ID. If an outcome is uncertain, inspect `operation get --operation-id <id>` before taking further action; do not blindly submit another write.

`review prepare` now reads the live plan and included designs, qualifies explicit per-task Jira field proposals and records complete immutable local review context. Follow [review preparation](review-preparation.md) for input, required profiles/policies and artifact inspection. It requires `--max-bytes 1048576` to retain all possible effects while returning compact output, and `--apply` for local artifact publication. It sends no Jira write or review comment.

`review verify` now checks current human approval against the live revision and prepared mapping, and records immutable local evidence. Follow [verification usage](review-verification.md). `plan tickets-preview` now records exact approved payloads, dependency batches and native links; follow [ticket preview](ticket-preview.md), including explicit type/direction configuration during preparation. General directed linking is available through `jira link create` with independent endpoint policies; see [link usage](jira-link-create.md). Use [ticket application/resume](ticket-application.md) to create the approved issues/links and retain recoverable mappings; result publication is a separate recoverable step. [Activation evidence](evidence/public-activation.md) records initial local verification and live-test limits; [preparation evidence](evidence/review-preparation.md) records the current addition.

Jira link get/list/types/delete/reconcile are also public with separate read, deletion and recovery allowances. Follow [link lifecycle usage](jira-link-lifecycle.md); investigation observes current state without attributing or replaying an earlier write.

An isolated [local development bundle](local-install.md) is also available, with a checked installation and no PATH changes. [Confirmed completion recovery](ticket-completion-recovery.md) handles a retained native issue completion whose local outcome was lost.

[Jira comment CRUD](jira-comments.md) is available with separate read/write allowances, typed ADF/visibility, original-author own/any checks and complete journaled receipts. Native fingerprint checks are non-atomic; live tenant qualification remains unfinished.

[Jira attachment get/list/settings](jira-attachments.md) return bounded inert metadata and upload settings with separate read allowances. Download, upload and delete are separate commands with their own allowances.
