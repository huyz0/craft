# Craft CLI

**Craft** is the agent-facing cloud CLI for the AI software factory. The executable is `craft`. Implement it in Rust here, starting with Jira Cloud and Confluence Cloud, then add other cloud clients through provider modules.

Status on 11 November 2026: the Windows development executable exposes 52 commands, including secure service-account provisioning, explicit-profile policy setup, Confluence space/page reads and page creation, and Jira issue reads/creation plus directed issue-link creation, reads, deletion and investigation. Public discovery, contracts, input refusal and composed hosts passed local checks and independent activation review. Live tenant qualification, delegated broker, broader CRUD and other-platform qualification remain unfinished. Start with the [cloud quickstart](docs/cloud-quickstart.md); [workflow evidence](docs/evidence/ticket-application.md) records the current executable and limits.

Local `plan scaffold/validate/render` and `design scaffold/validate/render` prepare and import visible documents, validate task graphs and render storage/ADF without cloud access. `plan page-create` and `design page-create` now publish complete validated documents through the existing page-write policy and journal. See [planning usage](docs/planning.md). `plan page-get` and `design page-get` now import current visible human edits with revision fingerprints. Separately allowed `confluence comment get/list/replies` return page-bound current-version metadata, paged roots/direct replies and optional body fingerprints; see [comment usage](docs/confluence-comments.md). Local `review policy get/set` now stores explicit per-space reviewer/quorum configuration; see [reviewer administration](docs/reviewer-policy.md). `review prepare` now records live source/design/native Jira mapping context in an immutable local artifact; see [review preparation](docs/review-preparation.md). `review verify` now observes current human approval and records local audit evidence; see [verification usage](docs/review-verification.md). `plan tickets-preview` now records approved payloads, dependency batches and native link mappings; see [preview usage](docs/ticket-preview.md). [Directed link creation](docs/jira-link-create.md) has independent endpoint policies and recoverable native receipts. [Link reads, deletion and investigation](docs/jira-link-lifecycle.md) retain exact identities, cursors and uncertain outcomes. [Reviewed ticket application and resume](docs/ticket-application.md) now create approved issues/dependencies with durable outcomes and independent result publication. Live qualification and operator-directed changed-preview reconciliation remain unfinished.

`operation get --operation-id` inspects durable local mutation records without loading credentials or contacting a provider. It preserves stored resource/job evidence and reports local absence explicitly. It never authorizes replay. See [operation inspection](docs/operations.md).

From this directory, bootstrap development commands are now executable:

```text
cargo build --locked
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run --locked -p craft-cli -- --version
cargo run --locked -p craft-cli -- command find --query schema
cargo run --locked -p craft-cli -- schema --command version --response --view full
cargo run --locked -p craft-cli -- capabilities --command version --fields availability.implemented
cargo run --locked -p craft-cli -- command find --input request.json
python tools/verify.py
```

## Start with the question you need to answer

The [delivery path](docs/delivery-path.md) prioritizes the first usable cloud workflow and states its current gaps. Local commands and internal test passes do not establish delivery of that workflow.

`--input FILE` supplies the selected operation's registered argument object; `-` reads stdin. Use either action flags or JSON, without merging the two. For the search example above, `request.json` contains `{"query":"schema","limit":3}`. Output controls such as `--fields`, `--max-bytes` and `--timeout-ms` remain flags. The work budget defaults to 60 seconds; Ctrl-C returns a structured cancellation result. The native signal case is qualified on this Windows development host, not yet on other operating systems.

| Question | Read |
| --- | --- |
| How do we work without a heavy planning harness? | [SDD workflow](SDD.md), [contributor router](AGENTS.md) |
| How should code be named and commented? | [Coding standard](docs/coding-standard.md) |
| How are parser, schema and selectable-field changes reviewed? | [Contract generation and drift checks](docs/contracts.md) |
| Which exact Jira/Confluence APIs and scopes back the proposed commands? | [Pinned provider inventory](docs/provider-contracts.md) |
| What Confluence reads are implemented internally? | [Confluence context and remaining activation](docs/confluence-context.md) |
| How do I configure and select local profiles? | [Profile commands and recovery](docs/profiles.md) |
| How are large non-secret artifact files stored safely? | [Native artifact store](docs/artifacts.md) |
| How are selected local files read and snapshotted for uploads? | [Workspace input boundary](docs/workspace-inputs.md) |
| How can the host export captured files to selected destinations? | [Internal Windows export](docs/workspace-outputs.md), [review boundary](docs/review-artifact-exports.md) |
| How are mutation intent, send claims and receipts persisted? | [Local mutation journal](docs/journal.md) |
| How do I inspect recorded steps after an interrupted request? | [Local operation inspection](docs/operations.md) |
| What native credential behavior is implemented and tested? | [Native item adapter](docs/native-vault.md) |
| How are service tokens verified, cached and reacquired? | [Internal service authentication](docs/service-auth.md) |
| How do I inspect local auth state without logging in? | [Auth status and doctor](docs/auth-diagnostics.md) |
| How does explicit human credential input protect terminal state? | [Internal interactive input](docs/interactive-input.md) |
| What is ready for auth review before public provisioning? | [Provisioning review package](docs/review-service-provisioning.md) |
| How does the host constrain HTTP and continuation? | [Trusted JSON transport](docs/transport.md) |
| How can a prepared HTTP write require a durable claim? | [Candidate journal-bound JSON mutations](docs/journaled-http.md), [review scope](docs/review-journaled-http.md) |
| How are known Jira create effects retained through partial/error results? | [Candidate create-result interpretation and completion](docs/jira-create-results.md) |
| How are Jira create fields bound to real project/type metadata? | [Candidate metadata reads and frozen-body validation](docs/jira-create-metadata.md) |
| How are subtask parents and assignee IDs qualified? | [Bound parent and active-account evidence](docs/jira-parent-assignee.md) |
| How are service access, metadata and journaled creation composed? | [Internal create host](docs/jira-create-host.md), [review scope](docs/review-jira-create-host.md) |
| How is the complete cached host tested without live credentials? | [Explicit debug-only composed acceptance](docs/composed-host-acceptance.md) |
| How are candidate arguments bound to trusted profile/project policy? | [Create command and private policy](docs/create-command-policy.md) |
| How do issue get and enhanced search select fields and resume pages? | [Internal Jira issue reads](docs/jira-issue-reads.md), [candidate arguments and configuration](docs/issue-read-command.md) |
| How are project details and grouped status context read? | [Internal project context](docs/jira-project-context.md), [candidate arguments and permissions](docs/project-context-command.md) |
| How do field list/search preserve native coverage, filters and inert metadata? | [Internal field discovery](docs/jira-field-reads.md), [candidate arguments and permissions](docs/field-read-command.md) |
| How do edit metadata and transition reads retain issue identity/state across buffered pages? | [Internal issue-bound context](docs/jira-issue-context.md), [candidate arguments and allowances](docs/issue-metadata-command.md) |
| What do current-user permission flags establish in each context? | [Internal contextual permission reads](docs/jira-permission-reads.md) |
| How are partial edits and transitions prepared without copying unrelated fields? | [Internal issue mutation preparation](docs/jira-issue-mutation-preparation.md) |
| How are edit/transition sends, refusals and uncertain effects journaled? | [Internal issue mutation host](docs/jira-issue-mutation-host.md) |
| How do typed edits/transitions obtain exact private project authority? | [Mutation candidates and project policy](docs/issue-mutation-command.md) |
| How are issue-specific assignment, explicit unassignment and subtask deletion prepared and journaled? | [Internal issue actions](docs/jira-issue-actions.md), [typed candidates and explicit policy](docs/issue-action-command.md) |
| How can uncertain Jira writes be investigated without replay or false attribution? | [Internal journal-bound reconciliation](docs/jira-reconciliation.md) |
| What did we learn from your existing repositories? | [Repository audit](docs/repository-audit.md) |
| What are we building and why? | [Proposal](openspec/changes/build-cloud-cli/proposal.md) |
| What behavior must hold? | [Product delta specs](openspec/changes/build-cloud-cli/specs/agent-interface/spec.md), then the capability relevant to your task |
| How will the Rust modules and future clients fit? | [Technical design](openspec/changes/build-cloud-cli/design.md) |
| What exactly should be implemented next? | [Detailed task ledger](openspec/changes/build-cloud-cli/tasks.md) or the bounded planning commands below |
| What is implemented or still unknown? | [Current state](STATE.md), [validation strategy](docs/validation.md), [decisions and risks](docs/decisions.md) |
| What API evidence and command inventory already exist? | [Research and prior design index](../docs/plans/README.md) |

From this directory, Python 3.11+ is sufficient for the documentation-only helper:

```text
python tools/plan.py check
python tools/plan.py status
python tools/plan.py next
python tools/plan.py show cli-bootstrap
python tools/plan.py coverage
python tools/plan.py selftest
```

These commands read/validate the plan and scan implementation files for prohibited planning labels. They do not execute Craft or certify Rust behavior. Use `next` and `show` instead of loading the whole ledger. Run `python tools/dependencies.py` to compare the locked source/license inventory; [dependency choices](docs/dependencies.md) distinguish qualified dependencies from future native candidates.

Planning labels belong in planning records only. Never put milestone/task identifiers into source, comments, tests, fixtures, configuration, logs, error codes, package/module names, public schemas, or commit subjects. Use semantic names that describe the behavior. Provider resource IDs and runtime request/operation IDs are legitimate domain data.
