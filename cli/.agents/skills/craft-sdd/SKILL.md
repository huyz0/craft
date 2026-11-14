---
name: craft-sdd
description: Develop or revise Craft's Rust cloud CLI using its local specs and task ledger. Use for work inside cli/, including planning changes and implementation handoffs.
---

Work from the Craft subproject root. Read `AGENTS.md` and `STATE.md`, then use `python tools/plan.py next` and `show <task-name>`. Read only the selected capability spec and relevant design section.

Follow the [SDD workflow](../../../SDD.md) for substantive contract changes and completion. Implement semantic behavior, not planning labels. Keep task/milestone identifiers out of code, comments, tests, fixtures, product configuration and commit subjects.

For acceptance and evidence rules, read [validation](../../../docs/validation.md) only as needed. Record actual checks, partial work and unrun platform/tenant tests honestly. A planning pass does not establish working Rust behavior. Update task evidence and the state after meaningful progress.

Do not install the eventual runtime skill or claim `craft` exists until its implementation and packaging tasks pass. Use the existing user's authority; a plan entry alone does not authorize external mutations or parallel agents.
