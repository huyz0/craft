# Local mutation journal evidence

Observed on 9 November 2026, Windows x86_64 MSVC, Rust 1.99.1. This is partial evidence for durable mutation identity. Its task remains unchecked because production HTTP/CLI composition, provider reconciliation and independent consequential review remain incomplete. The full CLI delivery goal remains active.

## Implemented artifacts

`crates/craft-runtime/src/journal/` implements bounded SQLite operation records, installation/row binding checks, issued-grant existence, immutable actor/request/descriptor/target identity, revision comparison, declared native-request claims and append-only effect evidence. It opens only from validated profile state. Fault injection exists under test configuration and uses the dev-only fixture crate; there is no production fake selector.

`profiles.rs` migrates schema one to two in one transaction, preserves independent settings format one, verifies the exact journal table and refuses triggers. Its shared commit helper retains an active transaction on SQLite busy/locked, retries only the commit within the deadline and never reruns a transaction body. Evidence recording chooses one immediate attempt, even after cancellation. Profile and cursor writers use the same bounded commit helper.

Core changes bind an effective actor and optional immutable native-request digest per step, require a digest for sent/unknown/confirmed states, preserve monotonic effects and distinguish a logical mutation digest from output-only selection. Fixture bindings were updated to the stronger contract. No dependency, feature or package version changed.

The [design](../../openspec/changes/build-cloud-cli/design.md#local-mutation-journal), [recovery scenarios](../../openspec/changes/build-cloud-cli/specs/operation-recovery/spec.md) and [runtime contract](../journal.md) describe intended use and remaining integration boundaries.

## Observed checks

| Check | Result |
| --- | --- |
| `cargo test -p craft-runtime --lib journal::tests --locked --offline` with native filesystem/loopback access | Twelve tests passed; one is an explicitly selected subprocess helper and otherwise does no work. |
| Identity/replay | Restarted records retain exact binding; changed payload/target/actor/grant/descriptor and native step requests conflict. Changing only output byte budget preserves logical mutation identity. Apply refusal creates no journal intent. |
| Commit faults | Before/after begin, prepared-step and send-marker commits were reached explicitly. No failing/unknown claim returned permission to send; durable markers refused a second claim on reopen. Receipt commit faults retained prior or complete evidence. |
| Migration | Deadline after creating the new table rolled back table/version together. A subsequent upgrade preserved installation/backend settings, profile, issued grant and active preference. A conflicting legacy table was refused without changing the version. |
| Process races/crashes | Two ready-confirmed processes racing one native request produced one claimant. Killing children at uncommitted/committed send and receipt boundaries recovered prepared, send-started or confirmed state as appropriate. Only an uncommitted send could be claimed afterward. |
| Contention/cancellation | A busy commit waited without replaying its body; deadline rolled it back. Confirmed receipts persisted after cancellation when uncontended, while an active reader caused immediate evidence-recording failure without waiting or losing the caller's supplied IDs. |
| Strictness and bounds | Corrupt installation/format/row identity/revision/digest/actor/unknown/duplicate metadata was refused without repair. Excess revision/record size was rejected before persistence. The synthetic raw request-body sentinel was absent from SQLite. |
| Loopback send gating | A lost send-marker acknowledgment sent zero requests. A separate claimed POST reached the synthetic server once, lost its response, recorded unknown state and refused another claim. No real provider or production HTTP write path was used. |

The first process race exposed transient startup commit contention. The old consuming commit helper discarded its active transaction on `BUSY`, returning an unknown outcome. The bounded commit-only path fixed the race, and a separate held-reader regression proves the body is not replayed. The first full gate passed tests but stopped on Clippy's nested-condition finding; the code was corrected without a lint suppression. Its log is retained at `target/journal-verification-failed.log`.

The integrated `python tools/verify.py` gate passed 144 named Rust tests, five subprocess controls, formatting, locked dependency inventory and all-target Clippy. Two opt-in real-vault tests were ignored; their earlier native acceptance was not repeated for this journal-only checkpoint. Output is retained at `target/journal-verification-output.log`. Quality-checker selftests passed nine controls and planning-checker selftests passed fifteen. Dependency inventory remains unchanged at 278 packages, including five workspace members.

Current development executable SHA-256: `7255d75da076b72e128886a2e3281749193abe7c4fecae8304a714da5c13fb4d`. Compiler incremental hard-link warnings used copying fallback. No test, dependency guard or lint gate was relaxed.

## Limits

All tests used owned synthetic metadata, native local SQLite and optional loopback fixtures. They do not qualify real disk-full/I/O corruption, OS restart/power failure, Unix native behavior, external providers, authenticated actor discovery, multi-machine fencing or compliance certification. Synchronous local filesystem/database calls are not made interruptible by the invocation budget.

The journal does not store raw payloads, recover external OAuth rotations or prove a remote operation did not happen. Production HTTP still refuses mutating contexts. The claim API needs reviewed host transport integration so a claim is consumed for exactly its bound request and confirmed/unknown receipts reach CLI recovery output. Public journal inspection/reconciliation commands remain unimplemented. No real OAuth credential, tenant mutation, repository commit, push, publication or release changed.

Review disposition: the implementing agent reviewed binding, transaction and evidence-preservation paths. No independent human or authorized separate reviewer has qualified consequential mutation enablement; no cloud write path was enabled.

The later [local operation inspection checkpoint](operation-inspection.md) exposes existing journal reads through `operation get`, with strict input, projected schemas and explicit lack of live verification/replay authority. Its separate evidence supersedes the earlier statement that public inspection is unimplemented. Listing, provider refresh/reconciliation and reviewed production send integration remain incomplete.

The subsequent [journal-bound HTTP checkpoint](journaled-http.md) consumes a privately prepared JSON request only after its exact native request obtains a durable claim. It retains response/error separately from unknown-recording failure and permits provider-qualified receipt recording after cancellation. Provider/CLI mutation handlers, recovery envelopes, live evidence and independent consequential review remain unfinished; this internal composition does not complete the broader journal task.

## Closure

On 15 November 2026 the owning ledger task was closed. The remaining gaps listed above are answered in [foundation closure](foundation-closure.md), with platform runs in [platform qualification](platform-qualification.md) and review in [foundation review](foundation-review.md).
