# Decisions and unresolved deployment inputs

## Selected now

- Product and executable: Craft / `craft`, as chosen by the user. Namespace conflicts are an install diagnostic, not a reason to rename against that decision.
- Product code root: `cli/`, Rust workspace, thin binary, pure core, shared runtime, compiled provider modules and separate factory workflows.
- Applied SDD: standard OpenSpec artifact shape, manually initialized, small contributor skill and one authoritative task checklist. No milestone IDs in implementation; comments explain code/design under the coding standard.
- Deterministic tool: no embedded LLM or scheduler. Compact JSON with request/response schema discovery, explicit projection, bounded opaque continuation and artifacts.
- Auth: native OS keyrings only; eligible Atlassian service OAuth first, delegated confidential broker as a distinct required capability when supported. No general raw-token/export/HTTP bypass.
- State: transactional local journal/cursor store, not a distributed lock. Provider-native lifecycle/actions remain explicit.
- Runtime skills: one short default skill with conditional lists/writes/review/recovery references; qualify host behavior instead of copying generated integrations wholesale.

These select implementation direction. They do not prove native/backend/API support.

## Resolve through the owning tasks

| Input or uncertainty | Work that resolves it | Until resolved |
| --- | --- | --- |
| Stable Rust version, dependency/backend versions and licenses | `cli-bootstrap`, `dependency-baseline`, `native-vault-backends` | No claim of an executable/MSRV/platform package |
| Actual sites/projects/spaces, reviewer identities and service-account eligibility | `tenant-auth-inventory`, `review-authority` | Use fixtures; do not invent customer configuration or enable unavailable auth |
| Broker hosting/organization identity, callback registration and threat review | `broker-protocol`, `broker-service`, `delegated-login` | Delegated mode unavailable; app secret never moves into binary |
| Authoritative full operation scopes and tenant-specific API behavior | `provider-operation-matrix`, native provider and `live-provider-qualification` tasks | Advertise only implemented/qualified operations; keep unknown capability distinct |
| Windows/macOS/Linux session vault behavior, chunk sizes and crash recovery | Native-vault and OS qualification tasks | Platform support remains unverified |
| Enforceable multi-machine claims/fencing and shared rate budgets | `shared-authority-contract`, `shared-authority-integration`, `multi-machine-qualification` | Standalone advisory/single-writer semantics only |
| Trello callback/token endpoint and OAuth2 operation compatibility | `trello-auth-spike`, `trello-card-client` | No Trello support claim |
| Bitbucket auth/PR semantics and branch-policy independence | `bitbucket-auth-spike`, `bitbucket-pr-client` | No PR approval/merge policy equivalence claim |
| Measured token budgets, cold/warm schema reuse and false rejection | `agent-efficiency-evaluation` | Budget values are targets, not results |
| Exact signed package identifiers, install locations and name collisions | `distribution-packaging`, `upgrade-qualification` | No registry/trademark exclusivity claim, no replacing an unrelated binary |

Unknown tenant/hosting inputs do not block the detailed plan. They block only dependent live implementation/qualification. A failed spike produces a recorded decision and explicit task/spec revision, never a silently weakened contract. Future third-party plugins, Data Center, arbitrary cloud systems and production deployment tools need separate changes before becoming support commitments.
