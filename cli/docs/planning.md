# Prepare plans and designs locally

Craft provides six local authoring commands: `plan scaffold`, `plan validate`, `plan render`, `design scaffold`, `design validate` and `design render`. They read no profile or credential, make no network request and create no page or Jira ticket. Explicit targets describe the intended destination; local validation does not establish access to that destination.

Run examples from `cli/` with the development executable on PATH, or replace `craft` with `target/debug/craft.exe` on Windows. Build with `cargo build --locked --offline -p craft-cli`.

## Scaffold a document

Supply a semantic identity, title and explicit destination. Numeric resource IDs remain strings in JSON. These IDs are illustrative:

```text
craft plan scaffold --plan-id invoice-replay --title "Prevent invoice replay" --confluence-cloud-id 00000000-1111-2222-3333-444444444444 --space-id 1 --jira-cloud-id 00000000-1111-2222-3333-444444444444 --project-key PAY
craft design scaffold --design-id invoice-design --title "Invoice design" --kind hld --compact --confluence-cloud-id 00000000-1111-2222-3333-444444444444 --space-id 1
```

Both accept optional `--parent-page-id`. Design kinds are `hld`, `detailed` and `decision`; `hld` is the default. A full scaffold contains thirteen sections. `--compact` selects four: context, decision, interfaces/data and testing. Bodies are empty for the caller to author. Plan scaffolds contain empty context, designs and tasks. Neither scaffold is a valid completed document, and its response preserves `ready: false`.

Alternatively, save this seed as `plan-seed.json` and run `craft plan scaffold --input plan-seed.json`. Action flags and `--input` cannot be combined. Output controls such as `--fields` remain flags.

```json
{
  "plan_id": "invoice-replay",
  "title": "Prevent invoice replay",
  "confluence_cloud_id": "00000000-1111-2222-3333-444444444444",
  "space_id": "1",
  "jira_cloud_id": "00000000-1111-2222-3333-444444444444",
  "project_key": "PAY"
}
```

Scaffold inputs use flat target arguments. The returned document uses the nested domain target below. The response is an envelope: extract `data.document` before editing and passing it to validate/render. Passing the whole envelope as the document fails validation.

## Author and validate a plan

Save this complete minimal document as `plan.json`:

```json
{
  "schema_version": "1.0",
  "plan_id": "invoice-replay",
  "title": "Prevent invoice replay",
  "context": "Repeated delivery must preserve one ledger entry.",
  "review_state": "proposed",
  "target": {
    "confluence": {
      "confluence_cloud_id": "00000000-1111-2222-3333-444444444444",
      "space_id": "1"
    },
    "jira_cloud_id": "00000000-1111-2222-3333-444444444444",
    "project_key": "PAY"
  },
  "designs": [],
  "tasks": [
    {
      "task_id": "event-identity",
      "title": "Define event identity",
      "jira": {"project_key": "PAY", "issue_type": "Task"},
      "description": "Document the **event identity** shared by producers and consumers.",
      "acceptance": ["Producer and consumer agree on event identity."],
      "repositories": ["billing-api"],
      "depends_on": [],
      "designs": [],
      "include_in_jira": true
    }
  ]
}
```

```text
craft plan validate --file plan.json
craft plan validate --file plan.json --fields document.tasks.task_id,document.tasks.title,task_order
craft plan render --file plan.json --format storage
craft plan render --file plan.json --format adf
```

Task identities are runtime business data supplied by the caller. They begin with an ASCII letter or digit and contain only ASCII letters, digits, `.`, `_` or `-`, up to 64 bytes. Do not insert development-ledger labels into implementation artifacts. Every task requires explicit Jira project/type mapping, nonempty description, acceptance and repository scope. Optional `estimate` is a bounded single-line string. `include_in_jira: false` retains a visible task while excluding it from intended materialization.

A valid plan contains 1–256 tasks and at most 64 design references. It rejects duplicate identities, missing dependency/design references, cycles and included tasks depending on excluded work. The optional `task_order` response is deterministic dependency order across all tasks, including excluded tasks. It does not reserve a worker.

Each plan-level design reference contains `design_id`, `cloud_id`, `page_id`, positive `version` and a lowercase `sha256:` digest. A task's `designs` array names those references. Local validation checks shape and linkage; fetching the page or verifying its current revision requires a later remote workflow.

[Review preparation](review-preparation.md) now verifies these references remotely. Use the native page version and the semantic `source.document_digest` returned by `design page-get`, rather than its raw-body/native revision fingerprint. Preparation retains both source representations for review and makes no approval claim.

`review_state` accepts `proposed`, `in_review`, `changes_requested` and `approved`. It is display data. Even a locally authored `approved` label produces `review_verified: false`, `live_conditions_checked: false` and `tickets_created: 0`. Projection cannot hide these fields. Content digests are fingerprints, not approval receipts.

## Rich text and visible page import

Context, descriptions, criteria and design section bodies accept a Markdown string or an explicit structured body:

```json
{"format": "markdown", "value": "Explain the **contract**."}
```

`storage` takes a string; `adf` takes an ADF object. The supported [document subset](document-content.md) applies to all formats. Each rich value is limited to 65,536 bytes when serialized as JSON. Acceptance criteria must each be one nonempty rich paragraph. Reserved headings that collide with the visible page protocol are refused.

Render returns `data.storage` or `data.adf` inside the envelope. Extract that value into a raw file before using `--source-format storage|adf`. For example, after extracting storage into `plan.storage.xml`:

```text
craft plan validate --file plan.storage.xml --source-format storage --fields document
craft plan render --file plan.storage.xml --source-format storage --format adf
craft design validate --file design.json
craft design render --file design.json --format adf
```

Plan pages have a title, a Field/Value metadata table, Context and Design references sections, then Task sections containing metadata, Description and Acceptance criteria. Design sections use `Section <key>: <title>` headings. Import reads only visible content and preserves supported human changes. Unknown/duplicate fields, unsupported macros, merged tables, missing boundaries and ambiguous extra content fail instead of restoring an older JSON file.

Canonical plan/design digests normalize rich content to ADF, so supported storage/ADF round trips preserve the content fingerprint. `body_digest` identifies the rendered representation; it differs from `document_digest`. These commands have local fixture acceptance, not live Confluence editor acceptance.

## Discover schemas and control output

```text
craft command find --query plan
craft schema --command plan.scaffold --view full
craft schema --command plan.validate --response --view full --max-bytes 1048576
craft schema --command plan.validate --response --fields document.tasks.title
```

Request schemas describe operation arguments such as `file`; the document model appears in scaffold/validation response schemas under `data.document`. Default validation output omits the document and task order to keep the response short. Select them explicitly when needed. Rich-text union fields are selectable whole, such as `document.context`; variant-specific paths such as `document.context.value` return `FIELD_UNKNOWN`. Selected response schemas describe the actual projected envelope.

`--file -` reads the raw document from stdin. `--input -` reads the operation argument object from stdin. Using both for the same invocation is refused. File inputs must be lexically contained within the current project root and pass [native input checks](workspace-inputs.md); linked/nonregular/changed inputs and traversal are refused before decoding. The CLI input limit is 1 MiB, including storage/ADF files, even where the internal converter has a larger bound. Output defaults to 16 KiB and supports explicit `--max-bytes` up to 16 MiB; oversized results return a bounded error without a partial document. `--timeout-ms` and cancellation apply to reading and invocation checks.

The six local authoring commands reject `--profile` and `--state-root`. See [local acceptance evidence](evidence/plan-model.md).

## Publish the proposal for human review

`plan page-create` and `design page-create` validate a complete document, render its visible content and create a normal shared Confluence page. They accept `--profile` and `--state-root` and use the existing service authentication, exact space-write policy and operation journal. Configure `confluence.page.create` permission for the document's space; publication does not need Jira-create permission.

```text
craft --profile cf-test plan page-create --file plan.json --operation-id publish-invoice-plan --apply
craft --profile cf-test design page-create --file design.json --operation-id publish-invoice-design --apply
craft schema --command plan.page-create --view full
```

JSON is the default input format. `--source-format storage|adf` imports visible documents under the same validation rules. The document's destination supplies the site, space and optional parent; these commands have no target override flags. Plan publication requires `review_state: proposed`, preventing a copied approved label from presenting a new page as reviewed. Design publication retains the complete validated sections.

The response is the complete native page-publication receipt, not an approval receipt. It retains page identity/version when known, persistence and uncertainty evidence, and never grants replay. The envelope command identifies the workflow (`plan.page-create` or `design.page-create`); journal request/descriptor digests remain bound to native Confluence creation. Publication creates no Jira tickets and establishes no human approval. An uncertain result requires operation inspection before another write.

The four-mode synthetic HTTPS acceptance preserves full plan JSON/storage/ADF content and design JSON content through visible import, and retains an unknown receipt after a lost response. Live shared editing, page-update editor behavior, eligible human review and ticket creation/resume remain incomplete. See [publication evidence](evidence/document-publication.md).

## Import the current shared page

```text
craft --profile cf-test plan page-get --id 101
craft --profile cf-test plan page-get --id 101 --fields document,task_order
craft --profile cf-test plan page-get --id 101 --format adf --fields document.tasks.title,document.tasks.acceptance
craft --profile cf-test design page-get --id 102 --fields document.sections
craft schema --command plan.page-get --response --fields document.tasks.acceptance
```

These commands use the existing `confluence.page.get` allowance. They read the current published page with `get-draft=false`, validate the complete visible storage/ADF document, and require its Confluence tenant/space metadata to match the observed page. They accept no local source file. The native Confluence page title is authoritative over the repeated heading used by standalone documents; semantic validation and digesting use that title.

Default output returns validity, source fingerprint and task/section counts. Select `document` or its required fields explicitly. All projections retain source identity, version/representation fingerprint, observation time, semantic document digest, credential-cache receipt, `review_verified: false` and `tickets_created: 0`. A remote `approved` label remains display data. The source fingerprint identifies this observed revision; it is not approval or proof that the page stayed unchanged afterward.

Import errors retain the observed source when established and the credential-cache receipt. They return no partial document or stale local content. Use ordinary `confluence page get --id 101 --fields body_storage` (or `body_atlas_doc_format`) to inspect the raw current content when semantic import fails. If selected output exceeds the budget, the error retains the credential receipt; increase the budget or narrow the selection before another read. Complete import still validates the entire page before output projection.

Source snapshots are returned, not yet persisted as approved workflow artifacts. Live human-edit qualification, durable review snapshots, design-revision checks and ticket materialization remain separate work. See [remote import evidence](evidence/remote-document-import.md).
