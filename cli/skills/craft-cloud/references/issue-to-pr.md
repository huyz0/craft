# Trace a Jira issue to a pull request

Read the issue with `jira.issue.get` using the scoped Craft profile. Keep the issue key and inspect available description and link evidence for PR URLs. A Jira issue link to another Jira issue is a lead. Keep the PR identity separate.

If the PR URL is missing, use the team's authorized source-control or work-search integration. For TWG, inspect `twg help describe "pull-requests query"` before searching for the exact Jira key in the known repository or workspace. If no repository is known, ask for scope or use an authorized cross-tool search to find a candidate.

Use the source-control integration to verify the candidate URL, repository and PR number. Report the source revision when the request involves checks or review. State whether the link was explicit in Jira or inferred from matching PR evidence. If scope or access prevents a match, report which sources were checked and leave the relationship unresolved.
