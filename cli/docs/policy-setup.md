# Explicit permission setup

Public `policy get/set` commands configure the existing permission records through CLI dispatch. No credentials are read and no provider request is sent by either command.

## Setup order

1. Create explicit service profiles with the existing public profile commands and the known product Cloud ID. Share a grant only when the same service identity should serve both profiles.
2. Provision credentials through the existing secure interactive or native-pipe adapter. The expected actor and actual Jira/Confluence current-user proofs must agree. Never supply secrets in ordinary arguments or paste them into chat.
3. Read or set the exact local policy using an explicit global profile, kind and target. Select only the required operations. Local configuration does not prove server permission or human approval.
4. Read shared context and publish a normal plan page through the qualified resource commands. Human review and ticket materialization remain separate workflow steps.

Provisioning, policy administration and selected cloud commands are now public after independent review and local qualification. Live tenant access remains unverified. Policy allowances for internal-only commands do not make those commands available; see [current availability](cloud-quickstart.md).

## Supported permission kinds

| Kind | Target | Meaning of operations |
| --- | --- | --- |
| confluence-read | site | Exact space/page get/list/search and separate page-bound comment get allowances |
| confluence-write | space | Page create and/or update, independently enabled |
| jira-create | project | Only jira.issue.create |
| jira-read | site | Issue get/search, edit metadata, transitions, permission checks, link/comment reads and attachment metadata/list/settings |
| jira-project-read | site | Project listing, with explicit continuation_site |
| jira-project-context | project | Exact project get/statuses, using the shared site-policy revision |
| jira-field-read | site | Field list/search, with explicit continuation_site |
| jira-mutation | project | Issue actions, separate cascade intent, link create/delete and independent comment create/update-own/update-any/delete-own/delete-any |
| jira-recovery | project | Read-only investigation of the named original write; no retry or write permission |

Project and field setup uses the same explicit-profile, apply and revision controls. See [Jira context](jira-context.md) for setup order and shared-record receipts. Other provider settings remain in the full CLI scope.

A policy set request's structured arguments look like this:

```json
{
  "kind": "confluence-read",
  "target": {
    "provider": "confluence",
    "deployment": "cloud",
    "tenant": "00000000-1111-2222-3333-444444444444",
    "resource_type": "site",
    "id": "00000000-1111-2222-3333-444444444444"
  },
  "expected_actor": "service-actor",
  "enabled": true,
  "operations": ["confluence.page.get"],
  "expected_revision": null,
  "apply": true
}
```

The explicit profile is an invocation control, never a value taken from repository hints for administration. Flags include kind, target as strict resource-reference JSON, expected actor, enabled true/false, comma-separated operations, expected revision and apply. An absent expected revision creates only. An update must name the observed current revision. Disabling a record retains revision history; it does not remove or revoke the native credential grant.

Get arguments contain kind and the same exact target. A missing record returns policy null. A stored record for a different target is refused rather than substituted. Neither command accepts a supplied grant; the selected trusted profile owns it.

## Receipts

Responses have a fixed bounded schema containing profile, kind, target, policy, proposed_policy, configuration_write_state and live_verified false. Stored policy snapshots include the actual profile grant, revision, target, expected actor, enabled state and exact allowances. Private installation/format fields are not emitted.

A confirmed set returns policy and confirmed configuration state. A lost commit acknowledgment returns sent_unknown and the proposed snapshot separately, with policy null because this invocation did not establish a stored result. Read the same profile/kind/target under the same state root before retrying and preserve competing revisions. The command does not retry automatically. Complete success and uncertainty output are preflighted before persistence; malformed input, missing apply/profile/actor/enabled intent and insufficient initial receipt capacity fail before opening state.

Implementation: `crates/craft-cli/src/policies.rs` and `policies/records.rs`. Review and observed tests: [setup evidence](evidence/policy-setup.md) and [activation review package](review-service-provisioning.md).

For [directed links](jira-link-create.md), explicitly allow `jira.link.create` on both endpoint projects under the same service profile/actor. Trusted updates containing that action use private format three; older formats never inherit link authority. Ordinary operation updates retain format two.

Jira site-read policies accept up to thirteen explicit operations. Comment get/list require format three; [attachment get/list/settings](jira-attachments.md) require format four. Old formats retain their previous authority. Project mutation policies accept up to thirteen operations; explicit comment allowances require format five. Include all old operations you intend to keep when updating either list, using the observed expected revision. [Comment usage](jira-comments.md) explains native visibility, original-author ownership and independent own/any allowances.
