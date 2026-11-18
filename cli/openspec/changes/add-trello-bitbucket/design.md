# Design: table-driven commands for Trello and Bitbucket

## Problem

A Jira or Confluence command today spans a provider registration, native route, response model, field plan, private read/write policy record, Clap arguments, dispatch arms, a host test suite and a contract snapshot entry. The Confluence read surface alone is about 1,500 lines for five commands. Trello and Bitbucket need roughly twenty-five agent commands each. Repeating that pattern per command is slow and invites drift.

## Approach

Add one declarative command table per provider and drive the existing machinery from it. Nothing bypasses the registry, routes, projection, policy or journal.

1. **Table entry** (provider crate): command id, effect, native method and path template, query parameters, typed input struct, response item projection (JSON pointer, always-returned and default fields), paging style (Trello `before` id and `limit`; Bitbucket `next` link) and, for writes, the journal step description and reconciliation class.
2. **Generated pieces**: `OperationDescriptor`, `NativeRoute`, response schema and example, and field plan come from the entry. Paged reads reuse the shared paging contract; next links are accepted only on the same fixed origin and route.
3. **Host**: one shared read host and one shared journaled write host in `craft-cli` take a table entry plus validated input. Policy records are per provider and per command class (read, write, destructive) and stay explicit and private, matching Jira and Confluence.
4. **Writes** are journaled with a send claim and never retried automatically. Trello writes are idempotent by resource id where the API allows it; creates record the returned id as the receipt, and an uncertain create is reconciled by reading before any replay.
5. **Mocks**: each entry has a fixture-backed mock test through the existing TLS test server, covering success, permission refusal, rate limit with retry advice, malformed body and paging. Fixture shapes are verified against the live service with a locally saved token before the provider is declared qualified.

## Delivery slices

1. Trello reads: board list/get, list get, card get/list/search, comment list, label list, checklist get, attachment list.
2. Trello writes: card create/update/move/archive, comment add, label add/remove, checklist and item create/check, attachment add by link and file.
3. Bitbucket reads: repository get/list, branch list/get, pull request list/get/diff/diffstat/comments/activity, pipeline list/get/steps/log.
4. Bitbucket writes: branch create, pull request create/update, comment, approve/unapprove, request changes, merge, decline.
5. One skill per provider, qualified with `tools/skills_qualify.py` and an A/B run.
