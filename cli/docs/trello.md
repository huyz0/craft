# Trello reads

Twelve read commands cover what an agent needs to find and understand work on Trello. All are read-only and need a Trello profile, a provisioned credential and an enabled policy.

## Set up

1. Create a service profile whose provider is `trello` and whose tenant is the member id the key and token belong to, then provision it with `craft auth provision-static` (see [static credentials](static-credentials.md)).
2. Enable reads for the profile, naming each command you allow. The expected actor for Trello is the member id itself.

```text
craft policy set --profile cards --kind trello-read --apply --enabled true \
  --target '{"provider":"trello","deployment":"cloud","tenant":"<member id>","resource_type":"account","id":"<member id>"}' \
  --expected-actor <member id> \
  --operations trello.board.list,trello.board.get,trello.list.list,trello.card.list,trello.card.get
```

A policy covers the whole account. Boards are limited by what the credential can see, not by the policy.

## Commands

| Command | Reads |
| --- | --- |
| `trello board list` / `get --id` | Open boards the credential can see; one board |
| `trello list list --board-id` | Open lists on a board |
| `trello card list --list-id` | Open cards in a list |
| `trello card board-list --board-id` | Every open card on a board |
| `trello card get --id` | One card with label, member, list and badge counts |
| `trello card search --query [--board-id]` | Card text search, optionally within one board |
| `trello comment list --card-id` | Comments, newest first |
| `trello label list --board-id` | Labels defined on a board |
| `trello checklist list --card-id` | Checklists with item state |
| `trello attachment list --card-id` | Attachment names, links and sizes |
| `trello member list --board-id` | Board members, for assignment |

## Behavior

- Ids are 24 lowercase hex characters. Anything else is refused before any request.
- Lists page with `--page-size` (native batch, 1 to 100), `--limit` (items returned) and an opaque `--cursor`. Collections Trello returns whole are sliced by offset; comments page by `before` id; search pages by `cards_page`.
- Responses are normalized to stable snake_case names. Default fields are small; add fields with `--fields`.
- A 404 means the resource is absent or not visible; absence is never claimed.
- No request is retried beyond the shared safe-read rule, and credentials never appear in a URL.

# Trello writes

Fifteen journaled writes cover what an agent changes while doing development work. None deletes a board, list, card or comment, and none changes workspace, membership or billing settings.

## Enable

Writes need their own policy, separate from reads, naming each allowed command:

```text
craft policy set --profile cards --kind trello-write --apply --enabled true \
  --target '{"provider":"trello","deployment":"cloud","tenant":"<member id>","resource_type":"account","id":"<member id>"}' \
  --expected-actor <member id> \
  --operations trello.card.create,trello.card.move,trello.comment.add
```

## Commands

Every write needs `--operation-id NAME --apply` and `--max-bytes 65536` or more. Use a fresh operation id for each intended change.

| Command | Changes |
| --- | --- |
| `trello card create --list-id --name [--description --due --label-ids --member-ids --position top\|bottom]` | Creates a card |
| `trello card update --id [--name --description --due --due-complete]` | Edits fields; at least one change |
| `trello card move --id --list-id [--board-id --position]` | Moves a card |
| `trello card archive --id [--archived false]` | Archives, or restores with `false`; never deletes |
| `trello comment add --card-id --text` / `update --card-id --comment-id --text` | Adds, or edits one's own comment |
| `trello label create --board-id --name [--color]` | New board label |
| `trello label add\|remove --card-id --label-id` | Attaches or detaches an existing label |
| `trello checklist create --card-id --name` | New checklist |
| `trello checkitem add --checklist-id --name [--complete true]` | Adds an item |
| `trello checkitem set-state --card-id --item-id [--complete false]` | Checks or unchecks an item |
| `trello attachment add-url --card-id --url [--name]` | Attaches an https link (file upload is not supported yet) |
| `trello member add\|remove --card-id --member-id` | Assigns or unassigns a board member |

## Safety rules

- The request is recorded in the local journal and its send is claimed before anything leaves the machine. It is never repeated automatically.
- A refusal (400, 401, 403, 404, 429) is recorded as failed. A server error, an unclear answer or a lost connection is recorded as unknown. Read the resource before trying again, with a new operation id.
- Reusing an operation id for a changed or already started request is refused without a request.
- Trello answers `200` with the new state. craft compares it with what was asked and reports `desired_fields_verified`; a mismatch is a partial-apply error rather than success.
- The credential is sent in a header, never in a URL or journal record.
