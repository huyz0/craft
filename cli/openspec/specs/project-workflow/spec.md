# Project workflow

## Purpose

Keep Craft's development spec-driven, inexpensive to resume, and independent of planning metadata in the implementation.

## Requirements

### Requirement: Small spec-driven changes
Contributors SHALL maintain a proposal, behavior scenarios, a design and a dependency-aware task checklist for substantive features. They SHALL use existing records for small fixes rather than require a new framework ceremony.

#### Scenario: A new public command is proposed
- **WHEN** a change adds a public command
- **THEN** its effect, input, result and failure scenarios are specified before implementation and its task names the acceptance evidence.

### Requirement: Evidence-based completion
Task status SHALL reflect observed work. A planning check SHALL NOT be represented as a Rust, native-vault, tenant or agent-evaluation pass.

#### Scenario: Provider credentials are unavailable
- **WHEN** deterministic fixture tests pass but a live tenant test has not run
- **THEN** the evidence records the live test as unrun and the corresponding acceptance task remains incomplete.

### Requirement: Planning and code separation
Planning identifiers SHALL remain in planning/evidence records only. Implementation artifacts and commit subjects SHALL use semantic domain names. Legitimate provider/resource/request/operation identifiers SHALL remain supported.

#### Scenario: A regression test is added
- **WHEN** a contributor adds an input-validation regression
- **THEN** its test name describes the rejected behavior, its code/comments contain no planning label, and the planning record links to the test as evidence.

### Requirement: Bounded contributor context
The contributor router and skill SHALL direct agents to the selected task, capability spec and relevant design section. They SHALL NOT require loading every research document or all task details for a local change.

#### Scenario: A contributor resumes
- **WHEN** a new session starts
- **THEN** the state and local helper identify dependency-ready work and reveal one task without requiring a runtime CLI or external service.

### Requirement: Behavior and design comments
The coding standard SHALL require comments/doc comments to explain actual behavior, API contracts, design intent, invariants, failures or non-obvious tradeoffs. Comments SHALL NOT contain planning labels, progress reports or unnecessary restatements of obvious code.

#### Scenario: Credential storage changes
- **WHEN** a change modifies credential commit ordering
- **THEN** relevant comments explain the actual durability invariant and failure behavior, remain consistent with the implementation/design, and contain no milestone or task identifier.


#### Scenario: Change-aware verification without repeated full matrices
- **WHEN** a contributor runs the normal local verifier after changing a bounded implementation slice or documentation
- **THEN** checks cover the changed inputs and affected dependents, documentation-only changes run planning checks, and unchanged inputs do not repeat tests. The selected commands and scope are visible before execution.
- **AND** missing qualification history, shared foundation/dependency/toolchain changes, unknown impact or explicit full release/CI qualification select the full matrix. A focused or cached result cannot be reported as full qualification.
- **AND** failed or concurrently changed inputs remain unqualified. Stable checked inputs are retained so a later documentation edit does not repeat an unchanged Rust matrix. A preview neither runs checks nor advances qualification.

#### Scenario: A later check fails or the verifier is interrupted
- **WHEN** earlier commands passed for the exact current source and execution environment
- **THEN** the next normal invocation reuses those command results and retries the failed or unstarted checks. Each successful command is checkpointed before the next command starts.
- **AND** changed execution inputs, changed environment, malformed checkpoint data and explicit `--full` prevent reuse. Reused checks are reported separately from executed checks and cannot establish a new full qualification.

#### Scenario: Prose changes after a later check fails
- **WHEN** completed Cargo format, lint or test checks are followed by edits confined to `STATE.md`, `HISTORY.md` or Markdown under `docs/`
- **THEN** a normal retry retains those Cargo passes for identical command arguments and environment, while documentation-consuming checks rerun.
- **AND** edits to code, fixtures, tools, configuration, skills, specifications or unknown paths invalidate the checkpoint. In-run drift still cannot qualify changed inputs, and `--full` always runs every selected command.
