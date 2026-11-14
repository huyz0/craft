# Build the Craft cloud CLI

## Why

Coding agents need a deterministic, token-efficient interface to shared cloud work. They should read targeted context, manage resources and artifacts, and execute authorized factory workflows without learning each provider's raw HTTP/auth quirks. Humans own intent, workflow/policy and consequential reviews. An agent can prepare a complete Confluence plan for review, then create Jira tickets from the reviewed revision.

## What changes

Create a Rust workspace rooted at `cli/`, a thin `craft` executable, a provider-independent contract library, shared runtime infrastructure, Jira/Confluence provider modules, factory workflows and portable user skills. Introduce strict typed inputs, discoverable request/response schemas, field selection, bounded paging, native keyring auth, operation journals and explicit unknown/partial/async outcomes. Keep administrative/destructive capabilities independently enabled. Prove extension with additional cloud clients after the base contracts work.

## Scope and release boundaries

- Foundation: local discovery/validation, schemas, deterministic fixture harness, profiles, native stores, transport and recovery.
- Initial cloud resources: catalog Core Jira and Confluence operations, including files/comments/space creation/status semantics. General CRUD remains independently authorized.
- Factory workflow: published shared plan page, human edits, version/digest-bound approval, ticket mappings and resume.
- Complete researched surface: Extended/Privileged/Conditional catalog operations are explicitly planned and individually qualified. An API being documented is not enough to enable it.
- Team execution: an explicitly configured mutation authority for cross-machine safety; advisory/single-writer mode is clearly labeled.
- Extensibility: Bitbucket PR integration and Trello card integration are separate future slices with their own API/auth qualification, not initial-support claims.

No LLM planner inside the binary, daemon scheduler, unrestricted authenticated HTTP escape hatch, new task-tracker backend, automatic global skill installation, or production deploy controller is in scope. Third-party plugin execution and Data Center adapters remain deferred until separately specified. Native unpublished Confluence drafts are not the requested plan draft.

## Impact and authority

This package applies the lightweight development workflow now and plans the runtime. It does not create a Cargo workspace, acquire credentials, write to GitHub/Jira/Confluence, deploy a broker or authorize production operations. Implementation tasks describe evidence needed later, not actions already taken.

The original [command inventory](../../../../docs/plans/cloud-cli-command-catalog.md) and detailed [agent interface](../../../../docs/plans/agent-cli-efficiency-and-skills.md) and [paging contract](../../../../docs/plans/list-paging-and-response-schema.md) remain supporting detail. The current change owns implementation sequencing and Rust placement. Its task ledger replaces the older milestone table as the active delivery plan. Planning IDs stay out of implementation artifacts.

## Acceptance of this plan versus the product

Planning is complete when this package contains source-backed repository lessons, an applied SDD workflow, observable capability scenarios, an extensible Rust design, detailed dependency-aware tasks covering the researched surface, validation instructions and an honest handoff. Product delivery remains incomplete until tasks have actual implementation/native/provider/agent evidence. Do not archive this change on the strength of a planning validation pass.
