# Bitbucket commands

Command IDs use dots, invocation paths use spaces. Load the installed schema for exact flags: `craft schema --command bitbucket.pullrequest.get`.

## Reads

| Command | Notes |
| --- | --- |
| `bitbucket.repository.list`, `bitbucket.repository.get` | Newest update first |
| `bitbucket.branch.list` | `--name-contains` filter; newest commit first |
| `bitbucket.commit.list` | `--branch` limits to commits reachable from it |
| `bitbucket.pullrequest.list` | Open unless `--state` is `OPEN`, `MERGED`, `DECLINED` or `SUPERSEDED` |
| `bitbucket.pullrequest.get`, `bitbucket.pullrequest.diffstat` | By `--pull-request-id` |
| `bitbucket.comment.list` | Oldest first |
| `bitbucket.pipeline.list`, `bitbucket.pipeline.get`, `bitbucket.step.list` | UUIDs accepted with or without braces |

## Writes

| Command | Notes |
| --- | --- |
| `bitbucket.branch.create` | From a commit or branch |
| `bitbucket.pullrequest.create`, `bitbucket.pullrequest.update` | Title, description, destination |
| `bitbucket.comment.add` | General, inline or reply |
| `bitbucket.pullrequest.approve`, `bitbucket.pullrequest.unapprove` | Your own approval |
| `bitbucket.pullrequest.request-changes`, `bitbucket.pullrequest.unrequest-changes` | Your own change request |
| `bitbucket.pullrequest.merge` | Explicit merge strategy |
| `bitbucket.pullrequest.decline` | Optional reason |

Example, a general comment (validate first with `craft input validate`):

```sh
craft bitbucket comment add --repo service --pull-request-id 7 --text "Looks good" --operation-id op-bb-1 --apply
```

Limits: raw diffs, step logs and single-branch reads are not available. A 404 means absent or not visible, never proof of absence.
