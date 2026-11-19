# Bitbucket reads

Eleven read commands cover what an agent needs to review code and follow builds on Bitbucket Cloud. All are read-only and need a Bitbucket profile, a provisioned credential and an enabled policy.

## Set up

1. Create a service profile whose provider is `bitbucket` and whose tenant is the workspace slug, then provision it with `craft auth provision-static` (see [static credentials](static-credentials.md)). Craft proves the token against `GET /2.0/workspaces/{slug}` first, and the stored actor is the workspace UUID that call returns.
2. Enable reads for the profile, naming each command you allow.

```text
craft policy set --profile code --kind bitbucket-read --apply --enabled true \
  --target '{"provider":"bitbucket","deployment":"cloud","tenant":"<workspace>","resource_type":"workspace","id":"<workspace>"}' \
  --expected-actor <workspace uuid, no braces> \
  --operations bitbucket.pullrequest.get,bitbucket.pullrequest.list,bitbucket.repository.get
```

A policy covers the whole workspace. Repositories are limited by what the credential can see, not by the policy. The workspace always comes from the profile; no command takes one.

## Commands

| Command | Reads |
| --- | --- |
| `bitbucket repository list` / `get --repo` | Repositories in the workspace, newest update first; one repository |
| `bitbucket branch list --repo [--name-contains]` | Branches, newest commit first |
| `bitbucket commit list --repo [--branch]` | Commits, newest first, optionally those reachable from a branch |
| `bitbucket pullrequest list --repo [--state]` | Pull requests, open unless `--state` is `OPEN`, `MERGED`, `DECLINED` or `SUPERSEDED` |
| `bitbucket pullrequest get --repo --pull-request-id` | One pull request with branches, reviewers and participants |
| `bitbucket pullrequest diffstat --repo --pull-request-id` | Changed files with added and removed line counts |
| `bitbucket comment list --repo --pull-request-id` | Comments, oldest first, with inline file and line and the parent comment |
| `bitbucket pipeline list --repo` / `get --pipeline-uuid` | Pipeline runs, newest first; one run |
| `bitbucket step list --repo --pipeline-uuid` | A run's steps with state and result |

## Behavior

- A repository is its slug (lowercase letters, digits, `-`, `_`, `.`). Pull request ids are positive integers. Pipeline and step ids are UUIDs, accepted with or without braces and returned without. Anything else is refused before any request.
- Lists page with `--page-size` (native batch, 1 to 50), `--limit` (items returned) and an opaque `--cursor`. The cursor wraps the `page` token of Bitbucket's own `next` link, which craft accepts only when it stays on the same host, route and filters.
- `--name-contains` and `--branch` are validated to the characters a branch name can hold, so they cannot change the query Bitbucket evaluates.
- Responses are normalized to stable snake_case names. Default fields are small; add fields with `--fields`.
- A finished pipeline reports its `result` (for example `SUCCESSFUL`); a running one reports its stage there.
- A 404 means the resource is absent or not visible; absence is never claimed.
- Credentials are sent as HTTP Basic and never appear in a URL. Raw diffs and pipeline step logs are plain text, not JSON, and are not read yet.

## Writes

Ten journaled write commands change branches, pull requests and comments. They need a second, separate policy of kind `bitbucket-write` (class `bitbucket.write`), enabled with the same `craft policy set` form and naming each command allowed. `bitbucket.pullrequest.merge` and `bitbucket.pullrequest.decline` are never implied by other entries; enable them on their own.

| Command | Effect |
| --- | --- |
| `bitbucket branch create` | New branch from a commit or branch |
| `bitbucket pullrequest create` / `update` | Open a pull request; change its title, description or destination |
| `bitbucket comment add` | General, inline (path and line) or reply (parent id) comment |
| `bitbucket pullrequest approve` / `unapprove` | Set or clear your approval |
| `bitbucket pullrequest request-changes` / `unrequest-changes` | Set or clear a change request |
| `bitbucket pullrequest merge` | Merge with an explicit `--merge-strategy` |
| `bitbucket pullrequest decline` | Decline, optionally with a reason |

Every write takes `--operation-id` and `--apply`. Without `--apply` nothing is sent. A write sends one request and is never replayed: a transport failure or an asynchronous `202` leaves an `unknown` receipt that must be checked with a read, and reusing the operation id is refused. A `409` is reported as a version conflict and failed. Creates and updates are verified against the response; a success that does not show the requested change is reported as a partial apply.
