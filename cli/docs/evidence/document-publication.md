# Typed document publication

Implemented on 10 November 2026. Public `plan page-create` and `design page-create` connect existing document loading/validation/rendering to the journaled Confluence page-create host. Document metadata owns the exact site, space and optional parent. Plans require proposed state. The only provider write is normal current-page creation; no Jira issue operation or approval authority is involved.

## Observed acceptance

- The public refusal test passes non-proposed states, invalid content, missing apply, insufficient recovery-output capacity and double-stdin refusal before local state creation. Log: `target/document-publication-refusal.log`. Its initial run used the wrong expected error name for output capacity; the corrected expectation is `OUTPUT_BUDGET_TOO_SMALL`.
- `target/document-publication-host-native.log` passes four synthetic HTTPS modes: plan JSON, plan storage, plan ADF with lost response, and design JSON. Captured writes contain the complete visible document; semantic import preserves the content digest, dependency graph and proposed state. Each mode records one page POST, no Jira sends and the native actor/grant/target journal binding. The lost response retains sent-unknown state and no replay authority.
- The first composed test compile referenced a nonexistent command field in the journal binding; the binding stores request/descriptor digests. The test now checks actual actor/grant/target fields. The restricted TLS run failed token acquisition; the same test passed with native loopback access. Logs are retained as `target/document-publication-host.log` and `target/document-publication-host-final.log`.
- Contract export succeeds with 34 public operations in `target/document-publication-contract-generation-final.log`. The earlier generation invocation used an invalid helper action; it made no snapshot change.
- Unit assertions cover supported profile/state disclosure, native response-schema reuse, and byte-preserving receipt/error/warning/output-policy behavior when replacing the envelope command with the shorter workflow alias. The alias cannot enlarge the native preflighted envelope.

## Independent review

Authorized reviewer `/root/activation_review` found one profile-disclosure mismatch. Both IDs were added to explicit-profile support, with an exported-contract regression assertion. Read-only re-review found no remaining concrete blocker in document loading, proposed-state refusal, exact target derivation, validation-before-state/auth, reused native authority/journal or alias receipt preservation. The reviewer performed no tenant, secret or state access and did not run tests.

## Limits

Synthetic HTTPS is not live Confluence acceptance. Two-human shared editing, page-update editor behavior, remote human-edit import, exact-revision review and recoverable Jira materialization remain incomplete. The owning publication task stays unchecked. Native policy allowances remain `confluence.page.create`; no new authority store, schema version, dependency or lock change was introduced.

The publication dependency on Confluence comments was removed: page creation does not read or write comments. Comment-backed approval still depends on Confluence comments in the separate review-authority task; full comment functionality remains in scope.

## Final selected qualification

The initial selected verifier passed contracts, formatting, 114 unit tests, executable and issue-context tests, then stopped at a stale 32-command assertion. The corrected discovery test now derives its total from the explicit cloud/workflow list plus twenty local commands. Only that target and the remaining CLI targets were run afterward; no passing unit matrix was repeated.

`target/document-publication-remaining-1.log` records 17 interface, six operation, six planning and seven profile tests passing. Doc tests and default Clippy pass in remaining logs two/three. All nine composed hosts pass in remaining log four; feature Clippy passes in log five. The default offline build passes in `target/document-publication-build.log`. The combined successful evidence advances the verification baseline as focused qualification with failed-target correction. Final documentation verification is separate in `target/document-publication-docs-corrected.log`. Earlier documentation checks rejected a Markdown link in the ledger evidence field, which requires a plain workspace-relative path. The field was corrected without changing task completion or implementation.

The current executable SHA-256 is `4242bf52d9ff11b6f9d1da1dc5782b1feba683b7241dbc7cc787dc5a2a35b1cb`; its 34-operation contract SHA-256 is `2f1f57fbd65bcc37dca24446dc28a63cc58213d0101b86aa040ad78a57634a6f`. Machine-readable identity and qualified inputs are retained in `target/document-publication-result.json` and `target/document-publication-qualified-source-hashes.json`. This is not a full-workspace or live-tenant qualification.
