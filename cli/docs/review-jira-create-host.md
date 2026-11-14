# Jira create-host review scope

This is an unregistered candidate. No independent review has occurred. Use the [review checklist](review.md), [host contract](jira-create-host.md) and [observed evidence](evidence/jira-create-host.md).

Inspect these boundaries together:

- [Candidate command/private policy](create-command-policy.md): strict scalar/structured arguments, skipped parser/candidate registration, complete dispatch envelopes, atomic schema-three migration, apply/CAS/preflight administration and exact policy rereads. Inspect `create_policy.rs`, profile migration and `issue_create/command.rs` with their [acceptance evidence](evidence/create-command-policy.md). Local policy does not prove independent review or provide distributed fencing/immutable policy history.

- `crates/craft-cli/src/issue_create/host.rs`: pure preparation, trusted exact policy, complete scope preflight, prior-operation refusal, metadata/account composition and checked send.
- `crates/craft-runtime/src/auth/service/access.rs`: sealed identity/scope/lifetime, bounded internal bearer copies and unchanged descriptors.
- `crates/craft-runtime/src/auth/service/cache.rs`: shared acquisition and receipts after activation failure; compare old session behavior with the new access constructor.
- `crates/craft-provider-jira/src/create_metadata/mod.rs`: explicit issue type, basic validation and metadata binding, including null/omission.
- `crates/craft-cli/src/issue_create.rs`: projected cache effects, output bounds, prior positive evidence and separate issue outcomes.
- Existing journal/HTTP/completion implementations linked by the contract. Early journal inspection never substitutes for an atomic claim.

Check that input cannot select an authenticated origin or bypass apply/policy; scopes precede resource reads; direct-token grant assignment stays trusted-host responsibility; cached execution holds its actual grant lock; expiry after a claim prevents polling without reopening the claim; and output/persistence failure preserves effects.

The initial layer tests remain. Subsequent [composed acceptance](composed-host-acceptance.md) exercises actual cached host/runtime libraries with owned TLS servers, native locks, SQLite and memory credentials. Review the explicit fixture feature, fixed-material/provenance restrictions and non-debug build refusal. It closes the library-composition gap; native-vault, live provider and public CLI qualification remain distinct.

Public registration remains absent pending consequential review and required acceptance. This document does not authorize an external mutation, release or parallel reviewer.
