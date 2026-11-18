# Add Trello and Bitbucket Cloud providers

## Why

Agents doing development work track tasks on Trello boards and ship code through Bitbucket pull requests. The base change leaves both as separate future slices that need their own API and auth qualification. Today's auth and transport only know Atlassian service-account OAuth (client credentials at the Atlassian token endpoint, cloud-ID routing, bearer requests), so neither provider fits without a new credential contract.

## What changes

1. **Static API credential mode.** A profile auth mode for providers that issue a long-lived key or token rather than an OAuth grant. Secret material is provisioned only through the existing secure prompt/pipe path and stored in the native vault. No argv, file or environment fallback. The transport attaches it by provider-specific rule: Trello sends its key and token as request parameters, Bitbucket sends an API token with HTTP Basic or a workspace access token as bearer.
2. **Fixed-host routing.** Routes for `api.trello.com` and `api.bitbucket.org` are compiled in. Content, links and arguments cannot choose the host. Continuation links are accepted only on the same host and route.
3. **Trello commands** (agent scope): board, list, card get/list/search/create/update/move/archive, comment add/list, label list/add/remove, checklist and item create/check, attachment list/add/delete. No workspace, member or billing administration.
4. **Bitbucket commands** (agent scope): repository get/list, branch list/get/create, pull request list/get/create/update/comment/approve/request-changes/merge/decline, diff and diffstat reads, pipeline list/get/log status. No workspace, permission, webhook or deployment administration.
5. Journaled writes, strict inputs, response projection, paging, mock-backed host tests and a runtime skill per provider, following the Jira and Confluence conventions.

## Out of scope

Administration, webhooks, power-ups, Bitbucket repository/permission management, Trello Enterprise features, delegated OAuth for either provider.

## Delivery order

Shared foundation first, then Trello, then Bitbucket, each as its own pull request. Mock fidelity is checked against the live services only with tokens Huy saves locally through a secure-save script; no token is ever requested in chat or committed.
