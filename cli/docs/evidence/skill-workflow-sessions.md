# Skill setup and workflow session evidence

24 November 2026, Australia/Sydney. Windows, Craft preview 0.1.0, Codex CLI
0.171.0 and Claude Code 2.1.331. The [fixture inputs](../../tools/skill_session_fixtures.json)
use invented provider results. The sessions used disposable project repositories.

The release archive was built from the development executable, verified by
SHA-256 and extracted. The packaged companion installed three skills for each
host, and doctor verified the files and reported a compatible Craft executable.
Separate tests cover user destinations, both hosts sharing a destination,
unchanged repeat installs, directory junction refusals, and existing bundle
formats.

| Workflow | Standalone Codex session | Claude Code session with a scripted loopback endpoint |
|---|---|---|
| Trace a Jira issue to its PR | Found the issue-to-PR reference, matched the explicit PR URL against the source-control fixture, reported its source revision and checked the Jira read schema. | Registered and invoked the project skill and read the selected reference and fixture. |
| Create Jira tickets from a reviewed plan | With stale approval, required review of the current revision and a fresh preview. With current approval, identified the approved preview and derived the schema-checked create command with batch revalidation. | Registered and invoked the skill and read the reference and fixtures for both paths. |
| Recover an interrupted write | Found the recovery reference, identified `sent_unknown`, and checked the saved-request reconciliation command using the original binding. Held the replay. | Registered and invoked the skill and read the recovery reference and fixture. |

The desktop-bundled Codex initially registered the project skill but its shell
provisioning service was unavailable. The standalone CLI completed the runs
with its own read-only Windows sandbox. Claude Code reported no active login,
so a temporary loopback endpoint returned fixed tool calls for the host
integration test. That establishes Claude Code skill registration and loading
in these sessions. It leaves Claude model behavior and broader host
qualification open. No session submitted provider writes or used live Jira,
Confluence or Bitbucket results.

To reproduce the Codex cases, extract a verified archive, create one empty Git
project per fixture, and write that fixture as `case.json`. Run the
[project install and doctor commands](../install.md#set-up-codex-and-claude-code-skills)
against each directory. Start a fresh read-only Codex session with the fixture's
`request`. Provide the packaged companion and binary paths. Ask the session to
search by intent, read the matching reference and fixture, and check a relevant
command with `craft schema`. Require a clean completion trace and inspect the
answer for the correct action and decision boundaries.
