# Implementation plans

The active implementation package now lives in [`cli/`](../../cli/README.md), with [applied lightweight SDD](../../cli/SDD.md), [Rust design](../../cli/openspec/changes/build-cloud-cli/design.md), [detailed task ledger](../../cli/openspec/changes/build-cloud-cli/tasks.md), and [coding/comment standard](../../cli/docs/coding-standard.md). That ledger owns task status and sequencing. The documents below remain supporting research/contracts; the older milestone table is not the active delivery ledger.

## Craft: the AI software factory

**Craft** is the project name and **`craft`** is its CLI command. It is an extensible cloud CLI for the AI software factory, initially Jira and Confluence, later Bitbucket, Trello, and other systems. It covers general resource operations, attachments, comments and status, with shared JSON, OAuth, native credential storage, policy and recovery. The factory workflow adds a normal Confluence plan page for human review, then creates Jira tickets from the approved revision. Other authorized issue creation remains available.

1. [Research and recommendations](../researches/atlassian-agent-cli-research.md)
2. [Detailed implementation plan](atlassian-agent-cli-plan.md)
3. [Proposed commands and data contracts](atlassian-agent-cli-contract.md)
4. [Primary sources and evidence limits](../researches/atlassian-agent-cli-sources.md)
5. [API-backed command catalog](cloud-cli-command-catalog.md)
6. [Provider architecture and future extensions](cloud-cli-provider-architecture.md)
7. [Token efficiency, input correctness, and companion skills](agent-cli-efficiency-and-skills.md)
8. [Supporting web research and 18 sources](../researches/agent-cli-and-skills-research.md)
9. [Lists, cursors, field selection, and response schemas](list-paging-and-response-schema.md)
10. [Projects, Assets, Artifacts and vendor CLI reuse](../researches/atlassian-projects-assets-and-artifacts.md)

These documents are ready for implementation planning. They do not claim a working CLI, live OAuth setup, or verified native keyring integrations.
