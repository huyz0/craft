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
