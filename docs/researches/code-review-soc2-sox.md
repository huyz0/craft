# Code review in an AI software factory: SOC 2 and SOX

Research date: **8 November 2026**. This extends the [outer-loop research](outer-loop-and-human-control.md). Published company policies and platform features below are observations. The proposed controls are recommendations for this factory, not assertions that every organization must use the same reviewer count or workflow.

## Answer

Use agents to inspect changes, reproduce defects, suggest repairs, and prepare review evidence. For changes governed by a human-approval control, require an eligible independent person to approve the current revision before release. Enforce this in the repository and release system, rather than relying on an agent to follow a prompt.

Two agents do not automatically provide segregation of duties. Independence depends on who directed the work, who controls the reviewer, which credentials each process holds, and whether the author can bypass the approval gate. A different model or bot username does not establish those boundaries.

## What SOC 2 and SOX change

| Topic | SOC 2 | SOX-related change controls |
| --- | --- | --- |
| Scope | Controls over the system and services in the examination, against applicable Trust Services Criteria | Controls relevant to reliable financial reporting, including supporting IT controls |
| Review question | Does the organization have suitable change controls, and do they operate as described? | Could an unauthorized or incorrect change undermine financial reporting or a relied-upon financial control? |
| Practical boundary | Follow the scoped control description and commitments | Identify financially relevant applications, interfaces, reports, configurations, and supporting infrastructure |
| Reviewer count | Do not infer a universal requirement for two humans on every PR | Do not infer a universal requirement for two humans on every PR |
| AI implication | Replacing a documented human control changes its design | Determine whether the revised control still addresses the financial-reporting risk before relying on it |

SOC 2 is an examination against criteria, not a universal repository settings template. The AICPA publishes the applicable criteria; access to the full current criteria download required an account in this research, so this report does not claim to have verified its detailed wording directly. [C1]

For SOX, PCAOB AS 2201 concerns internal control over financial reporting. Its audit guidance requires evidence beyond inquiry alone when testing operating effectiveness (.50). Reliance on unchanged automated application controls depends on effective supporting controls over program changes, access, and operations (B29); relevant files, tables, data, and parameters also matter (B30). These provisions explain why code, configuration, access, and deployment evidence can all be relevant. They do not prescribe a particular GitHub approval count. [C2]

Agree a proposed replacement for a documented human control with the control owner and auditor before treating agent approval as equivalent. That is a control-design decision, not a model selection decision.

### A recent proposal is not an effective requirement

On 28 October 2026, the AICPA proposed revised SOC 2 criteria with additional AI considerations. Comments close on 15 January 2027. The announcement describes a proposed effective date for engagements beginning on or after 31 December 2027, if approved, with early adoption permitted. As of this research date, this is a proposal. [C3]

## What companies actually publish

**GitLab:** its policy allows independent agent approval for eligible changes, while retaining human approval for SOX-controlled changes. A person who directed the coding agent counts as an author and cannot approve that work. Eligibility is evaluated for the current commit, and review instructions come from the protected default branch. Authors cannot control their own assessment or reviewer configuration. This is GitLab's published policy, not a universal legal interpretation. [C4]

GitLab's composite identity documentation also connects agent activity to the triggering human and discusses segregation of duties. Attribution therefore needs to preserve the responsible person, rather than presenting a service account as an unrelated author. [C5]

GitLab publishes concrete implementation settings for its own main development project: prevent creator and committer approvals, prevent editing approval rules in merge requests, remove approvals following new source commits, and require designated approval for CODEOWNERS changes. These are useful examples of enforcing policy with repository controls. [C6]

**GitHub:** Copilot review defaults to comments, but an enabled Copilot approving review can satisfy required approvals. A textual approval assessment alone does not count. New commits dismiss Copilot approval. Its review instructions and skills come from the PR head branch. Consequently, a generic approval count is insufficient evidence of required human approval, and author-editable review instructions need a separate control. [C7]

GitHub's coding agent is a separate role: the documented restrictions prevent it from approving or merging its own PR, and prevent the person who requested that PR from approving it. Do not confuse those restrictions with the configurable approval capability of Copilot code review. [C8]

GitHub protected branches support required reviews, code-owner review, stale-approval dismissal, and approval of the most recent reviewable push by another person. Bypass permissions require attention because protections do not necessarily apply to every administrator or privileged actor by default. [C9]

**Google:** its published approach describes human reviewers examining purpose, implementation, tests, and presubmit results. Significant subsequent changes invalidate approval, and presubmits run again before merge. This provides a concrete engineering comparison; it does not establish a Google policy for AI-only approvals under SOX. [C10]

These are documented practices and capabilities. This research does not establish how frequently the wider industry uses AI-only approval, or independently verify operation of these companies' controls.

## Recommended operating model for this factory

Keep the tracker, repository, and deployment record linked. Linear, Jira, or a custom ledger can hold intent and workflow status. The repository should enforce code approval, and the deployment system should record what actually reached production. A tracker status of `approved` is insufficient if an author can set it directly.

| Stage | Responsible role | Proposed gate and evidence |
| --- | --- | --- |
| Scope and request | Business/control owner | Change ID, intended behavior, affected systems, financial relevance, acceptance criteria |
| Design | Technical owner; control owner when relevant | Approved consequential decisions, including impact on financial controls |
| Implementation | Coding agent under a named owner | Isolated branch, run identity, initiating identity or scheduled-service identity, commits |
| Automated validation | CI and review agent | Tests, scans, concrete review findings, disposition, commit identifier |
| Independent review | Eligible human for human-controlled changes | Review of the actual revision, tests, risk, and unresolved findings; recorded approval |
| Business validation | Finance/process owner where needed | Evidence that intended financial behavior is correct |
| Release | Restricted deployment service | Required gates verified, reviewed artifact identified, environment and release recorded |
| Reconciliation | Control owner or monitoring process | Production changes matched to authorized change records; exceptions investigated |

These roles do not imply a different human for every row. Define the incompatible combinations explicitly. For example, the person responsible for authoring a change should not also satisfy its independent peer-approval requirement. A controlled service can deploy an approved artifact automatically; a person need not manually press every deployment button.

### Identity and authorization

Suppose Alice directs Claude Code or Codex to change revenue recognition logic. Record Alice as the directing author and the agent run as the executor. Bob, an authorized reviewer who did not author or direct that implementation, reviews the diff and relevant evidence. A separate controlled deployment identity releases the approved artifact.

If the implementation is triggered by a scheduled shared agent, preserve the service identity, trigger, and accountable owner. Do not invent a human initiator. Apply the policy's independence rules to the owner, contributors, and approver.

Give the worker credentials to propose changes, without allowing it to modify the effective approval policy or deploy directly. Protect review instructions, CI definitions, CODEOWNERS, gate implementations, and repository settings. A protected review policy can still be proposed for change, but the proposal must not immediately redefine how its own approval is judged.

### The outer loop

Use explicit states such as `implementing`, `automated-review`, `awaiting-human-review`, `changes-requested`, `approved`, and `released`. Store approval against a revision, not merely against a task ID.

When human review is required, notify the responsible review group with the diff, risk classification, failed or passed checks, and decisions needed. Persist the request in the ledger; chat notifications should link to it. Escalate overdue requests according to an owned policy. The coding worker must not mark its own request approved.

When a reviewer requests changes, return the task to implementation. Repairs produce a new candidate and rerun affected checks and required approvals. Immediately before release, verify that the artifact still corresponds to the approved candidate. Configure merge queues and release pipelines so their generated candidates have the checks required by policy.

## What the human should actually review

For ordinary changes, review behavior, failure modes, security boundaries, tests, and maintainability. AI summaries can orient the reviewer, but the underlying diff and test evidence remain available.

For a financially relevant change, add the specific financial assertion or control it could affect. A revenue-calculation change might require examples covering rounding, duplicate events, missing transactions, period boundaries, reversals, and reconciliation. Have the appropriate finance/process owner validate expected outcomes where technical review cannot establish business correctness. These are proposed tests for that example, not mandatory tests for every SOX-scoped change.

Avoid treating a human click as sufficient review quality. Define the review's scope, expected evidence, competence, and handling of unresolved findings. Keep changes small enough for that review to be meaningful.

## Evidence to retain

For this factory, retain a linked record of:

1. Request, spec/design revision, scope and risk decision, and accountable owner.
2. Directing identity, agent/service identity, run identifier, and candidate commit.
3. Test and scan results, AI findings, and their resolution.
4. Required approval identities, eligibility, timestamps, and approved revision.
5. Build/artifact identifier, deployment identity, environment, time, and outcome.
6. Exceptions, bypass events, emergency authorizations, and subsequent review.

If agent review is relied on as a control, also version its policy, configuration, and model identifier where available, and retain evidence evaluating its suitability. Full prompts and hidden chain of thought are not universal audit requirements. Choose records that establish the control operated, while respecting secrets and data retention rules.

Preserve evidence outside short-lived runner logs when necessary. Protect it against unauthorized alteration. Retention depends on the applicable control, contract, and legal requirements; there is no justified blanket claim here that every PR and agent log must be kept for seven years.

Reconcile the population of production changes with approved records. Include hotfixes, schema/configuration changes, feature flags, and manual interventions where in scope. A collection of well-documented PRs does not establish that unrecorded production changes did not occur.

## Exceptions and adoption

Define an emergency path with authorized access, recorded reason and scope, monitoring, and timely independent retrospective review. Treat it as an exception process with ownership, rather than allowing a coding agent to classify its own failed gate as an emergency.

For a small team, document where independence is unavailable and assess a precise compensating control with the control owner. Another bot account is not a compensating control by itself.

Start with AI review as advisory and preserve existing human gates. Measure missed defects, false findings, review time, and exceptions. Consider agent-only approval only for explicitly eligible risk classes after evaluating the control design. Keep SOX-scoped and other required-human changes on their mandated path. A vendor's assurance report does not establish that your configuration and workflow operate effectively.

## Sources and access limits

All sources below are primary sources accessed on 8 November 2026. Living documentation may change.

| ID | Source | Use and limitation |
| --- | --- | --- |
| C1 | [AICPA Trust Services Criteria](https://www.aicpa-cima.com/resources/download/2017-trust-services-criteria-with-revised-points-of-focus-2022) | Official landing page reviewed; full download required login |
| C2 | [PCAOB AS 2201](https://pcaobus.org/oversight/standards/auditing-standards/details/AS2201) | ICFR audit guidance, especially .50 and B29-B30; future amendments identified on the page were not treated as already effective |
| C3 | [AICPA proposed SOC 2 update, 28 October 2026](https://www.aicpa-cima.com/news/article/aicpa-proposes-update-of-soc-2-r-trust-services-criteria-to-enhance-the-relevance-usability-and-value-of-cpas-reports) | Proposal and proposed timing |
| C4 | [GitLab code review guidelines](https://handbook.gitlab.com/handbook/engineering/workflow/code-review/) | Published company policy on agent and human approval |
| C5 | [GitLab composite identity](https://docs.gitlab.com/user/duo_agent_platform/composite_identity/) | Agent/human attribution and segregation of duties |
| C6 | [GitLab development processes](https://docs.gitlab.com/development/development_processes/) | Published settings for its own development workflow |
| C7 | [Using GitHub Copilot code review](https://docs.github.com/en/copilot/how-tos/use-copilot-agents/use-code-review) | Approval behavior and instruction origin |
| C8 | [GitHub coding agent risks and mitigations](https://docs.github.com/en/copilot/concepts/security-governance-and-network-settings/risks-and-mitigations) | Authoring-agent restrictions, distinct from review-agent capabilities |
| C9 | [GitHub protected branches](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches) | Review enforcement and bypass settings |
| C10 | [Google's approach to change](https://docs.cloud.google.com/docs/cloud-approach-to-change) | Published engineering review practice |
