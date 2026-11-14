# Configure local profiles

These commands are implemented in the Windows development build. They store non-secret metadata for Jira Cloud and Confluence Cloud. Authentication, live site discovery and cloud operations remain unimplemented. A configured tenant is intended context, not proof of access.

Build from `cli/` with `cargo build --locked`. In PowerShell, use the development executable:

```powershell
$craft = '.\target\debug\craft.exe'
& $craft profile create --name team --provider jira --deployment cloud --auth-mode delegated
& $craft profile use --name team
& $craft profile show
& $craft profile list --limit 20
```

Create returns `data.profile` with a locally issued `grant.id`; use returns `data.active: true`. Show without an explicit name resolves repository hints before the active profile. Every command returns one JSON envelope. Creating an existing alias fails rather than replacing it, and creating a profile does not select it as active.

## Select state and context

State-root precedence is `--state-root DIRECTORY`, then trusted launcher `CRAFT_STATE_ROOT`, then the native per-user location. Roots must be absolute local paths with private permissions. New directories are created privately; existing insecure, linked or foreign state is refused rather than repaired.

| OS | Native location |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Craft` |
| macOS | `$HOME/Library/Application Support/Craft` |
| Linux | `$XDG_STATE_HOME/craft`, otherwise `$HOME/.local/state/craft` |

Only Windows has native qualification evidence so far. SQLite holds metadata, cursors, internal mutation records and private create/read configuration, with no OAuth credentials. Schemas one through six upgrade atomically to schema seven, preserving existing state; settings retain their independent format version one. Unknown or conflicting schema is refused. See the [journal contract](journal.md), [provider continuation library](provider-pagination.md), [project-read configuration](project-list-command.md) and [field permissions](field-read-command.md). The recorded backend does not prove native availability. The internal Windows item adapter has [separate native acceptance](native-vault.md); credential-facing CLI commands remain unimplemented.

`profile show/use` accepts `--name` or global `--profile`. If both are supplied, they must agree. Otherwise selection is explicit alias, then the current directory's `.craft.json`, then trusted active profile. No upward repository search or first-profile fallback occurs. An absent selection returns `TARGET_AMBIGUOUS`; a selected missing profile fails rather than substituting another.

The internal [create-command policy](create-command-policy.md) was introduced in private SQLite schema three and is preserved by schema five. Project-read configuration has a separate record introduced in schema five. Neither is a profile default or repository hint, and existing public profile commands cannot enable them. Migration preserves profiles and operation evidence; older database-version support does not imply a downgrade path.

A repository hint can contain:

```json
{"profile":"team"}
```

Hints may also contain typed target references under `targets`. They cannot configure state roots, backends, auth hosts, brokers or reviewer policy. Explicit profile selection avoids reading repository hints. State/profile flags apply only to profile commands; `--profile` does not apply to create/list. Discovery and input validation do not open profile state.

## Share a grant or set default targets

Use `--grant` with an existing profile's issued `grant.id` to share grant metadata. Jira/Confluence cloud profiles can share the same reference when their auth modes agree. Omission issues a new reference. Arbitrary aliases, OAuth tokens and unissued references are refused.

For typed defaults, use the structured input contract:

```json
{
  "name":"finance",
  "provider":"jira",
  "deployment":"cloud",
  "auth_mode":"delegated",
  "tenant":"primary",
  "defaults":{
    "project":{
      "provider":"jira",
      "deployment":"cloud",
      "tenant":"primary",
      "resource_type":"project",
      "id":"PAY"
    }
  }
}
```

Save this as `request.json` in the current working directory, then run:

```powershell
& $craft input validate --command profile.create --file request.json
& $craft profile create --input request.json
```

`primary` and `PAY` are illustrative identifiers; replace them with the intended tenant and project. Confluence defaults use the `space` resource kind. Defaults require a matching tenant/provider/deployment and never establish permission. Use either action flags or `--input`, without merging them. Inspect the complete contract through `schema --command profile.create --view full`.

## Continue bounded lists and recover writes

Lists default to 20 items with a maximum of 100 per invocation. `--fields name` still preserves mandatory name/provider/deployment/tenant fields and envelope control evidence. `page.next_cursor` is an opaque local handle. Resume with the same state root and effective field selection; `--limit` and `--max-bytes` may change. Stop when `page.has_more` is false, including when a terminal page is empty.

Cursors preserve unreturned items across restarts. `data.cached` and `data.observed_at` identify cached observations; `page.consistency` remains `live`, without promising a cross-page snapshot. Handles expire after 15 minutes. Invalid, lost, foreign, expired or mismatched cursors fail explicitly, so starting over is a separate caller decision. The cache holds at most 256 unexpired handles; capacity exhaustion does not evict a live handle silently.

Create/use checks the projected success and unknown-commit recovery envelopes against the byte budget before committing. `OUTPUT_BUDGET_TOO_SMALL` leaves the requested profile/grant/active selection unchanged, though initial empty state may have been created. If the result is `WRITE_OUTCOME_UNKNOWN`, read the named profile and active selection from the same root before retrying. A successful local write confirms configuration only.

Source and checks: [profile evidence](evidence/profiles.md), [native boundary](native-safety.md), [technical design](../openspec/changes/build-cloud-cli/design.md#trusted-local-profile-state).
