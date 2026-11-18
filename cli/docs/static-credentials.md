# Static credentials for Trello and Bitbucket

Trello and Bitbucket Cloud issue long-lived credentials instead of an OAuth exchange, so they use a separate profile and grant path from the Atlassian service grants.

## Profiles

Create a service profile whose provider is `trello` or `bitbucket`, deployment `cloud`, and tenant set to:

| Provider | Tenant | Default resource kind |
| --- | --- | --- |
| `trello` | the 24-character member id the key and token belong to | `board` |
| `bitbucket` | the workspace slug | `repository` |

## Provision

Pipe one JSON object to `craft auth provision-static --name PROFILE --secret-stdin` (add `--replace` to rotate):

| Provider | Object |
| --- | --- |
| `trello` | `{"key":"<api key>","token":"<member token>"}` |
| `bitbucket` | `{"email":"<account email>","api_token":"<api token>"}` |

The secret is never accepted as an argument, file or environment value. Before anything is stored, craft calls the provider's identity route on its fixed host (`GET /1/members/me` or `GET /2.0/workspaces/{slug}`) and refuses the credential unless it resolves to the profile's tenant. The stored grant is read from the native vault only; opening a session never contacts the provider.

## Transport rules

- Trello sends the key and token in an `Authorization: OAuth oauth_consumer_key="...", oauth_token="..."` header, so neither appears in a URL, log or continuation link.
- Bitbucket sends HTTP Basic with the account email and API token.
- Hosts are fixed (`api.trello.com/1`, `api.bitbucket.org/2.0`). Arguments, content and next links cannot choose another origin.
- A credential scheme is bound to its provider's tenant and cannot be presented to another provider.
