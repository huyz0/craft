# Delivery quality

## ADDED Requirements

### Requirement: Synthetic composed acceptance
An explicit non-default development feature MAY expose owned loopback fixture construction to test the actual CLI/runtime composition. It SHALL be absent from default builds, unavailable without debug assertions and unreachable through public CLI configuration. It SHALL accept only compiled synthetic credentials and bearer values, including cache reuse and identity probes, and SHALL NOT redirect real credentials, tokens or ordinary host sessions.

#### Scenario: Complete create host
- **WHEN** the integration target exercises cached create through the actual host and runtime libraries
- **THEN** observed TLS requests, native journal state and projected completion prove scope/policy refusal before resource reads, one claimed create, no replay of unknown/terminal work and final refusal after claim-time expiry/cancellation without reacquisition.
- **AND** the explicit fixture uses an isolated in-memory credential store with actual generation/cache logic and native grant locks, never a selectable production credential fallback; default discovery is unchanged and a build without debug assertions rejects the feature.

#### Scenario: Concurrent fixture roots with equal timestamps
- **WHEN** synthetic host fixtures observe identical wall-clock timestamps within one process
- **THEN** their temporary-root names remain distinct, so a test cannot open another fixture's profile, journal or cursor state; production profile-conflict controls remain unchanged.

### Requirement: Lightweight Rust workspace
Craft SHALL use a thin Rust binary with provider-independent contracts, shared infrastructure, compiled providers and isolated workflows under cli/. Planning metadata SHALL not leak into implementation.

#### Scenario: Workspace grows
- **WHEN** a provider or workflow is introduced
- **THEN** Cargo dependency direction enforces the declared boundaries and dev-only test support stays out of production dependencies.

### Requirement: Behavioral and platform qualification
Support SHALL be established by relevant subprocess/fault/conformance/native-vault/live-provider checks, with unrun matrices explicit and no weakened controls to obtain a pass.

#### Scenario: Fixture-only success
- **WHEN** a provider fixture passes but native auth/live access has not been tested
- **THEN** capability/release evidence states the limitation instead of claiming production support.

#### Scenario: Storage commit acknowledgment is lost
- **WHEN** a fault fires before or after a simulated vault, journal or artifact commit
- **THEN** the fixture distinguishes unchanged state from an unknown acknowledgment, preserves any committed effects on reopened backing state, and refuses to overwrite an existing binding or complete artifact to hide uncertainty.

#### Scenario: Wall time changes during an invocation
- **WHEN** a test adjusts wall time backward while advancing the invocation clock
- **THEN** monotonic elapsed time continues forward independently, so later deadline tests cannot gain time from the wall adjustment.

### Requirement: Progressive runtime skills
The shipped skill SHALL have a short entrypoint and directly linked conditional workflow references, use discoverable installed contracts and remain compatible with qualified Codex/Claude Code hosts.

#### Scenario: Simple known read
- **WHEN** an agent performs a known resource read
- **THEN** it does not load the entire command inventory, planning workflow or all provider manuals.

#### Scenario: Repository skill preparation
- **WHEN** a development skill is prepared before host qualification
- **THEN** its local links and cited commands/flags are checked against the contract snapshot, compatibility identifies development status and unqualified hosts, and optional tokenizer measurements report the actual encoding without claiming host token savings.
- **AND** skill-only changes select offline skill checks rather than unrelated native provider matrices; command-contract changes also recheck skill compatibility.

#### Scenario: Find a workflow reference
- **WHEN** an agent searches the bundled reference catalog by workflow intent
- **THEN** offline search returns up to three deterministic matches with their owning skill and relative reference, or an empty result for unrelated intent.
- **AND** search indexes curated metadata without opening reference bodies or consulting a provider.

#### Scenario: Install and diagnose project and user skills
- **WHEN** an operator explicitly selects Codex or Claude Code and project or user scope
- **THEN** the release companion installs the compatible bundle into the host's discovery directory and treats an identical install as a successful no-op.
- **AND** existing conflicting or redirected skill content is refused before copying, and unrelated skills remain untouched.
- **AND** read-only doctor reports missing or changed files and checks the selected Craft executable's product, version and contract version. A doctor pass alone does not qualify runtime loading.

### Requirement: Measured agent efficiency
Token/call/latency goals SHALL be measured over correctly completed tasks, including cold discovery, warm reuse, failures, recovery and artifact fidelity.

#### Scenario: Smaller output fails a task
- **WHEN** a projection lowers bytes but hides required decision evidence
- **THEN** the evaluation records the task failure rather than counting it as an efficiency win.

### Requirement: Honest distribution and support
Release packages SHALL identify the Craft binary, compatible contracts/skills, supported OS/auth/provider capabilities and integrity evidence. Publication SHALL remain a separately authorized action.

#### Scenario: Qualified release
- **WHEN** a package is prepared for a claimed platform
- **THEN** its native/runtime checks and integrity/upgrade evidence exist; a documentation-only plan cannot qualify the release.

#### Scenario: Local development bundle
- **WHEN** the current executable, contract and skill are bundled before release qualification
- **THEN** the bundle identifies development status, exact file hashes and platform, includes the dependency license inventory and explicitly disclaims unrun platform/live/host qualification.
- **AND** verification and isolated installation require an independently retained archive digest, reject unexpected/duplicate/unsafe members and incompatible skill contracts before writing, and refuse an existing destination rather than overwriting another Craft or changing PATH.
