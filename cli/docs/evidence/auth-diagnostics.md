# Local auth diagnostics and secret input evidence

Date: **10 November 2026**, Australia/Sydney. Partial auth lifecycle/service provisioning implementation. Neither owning task is complete.

## Changed behavior

- `crates/craft-cli/src/auth.rs` and its argument/dispatch/catalog registration add `auth status` and `auth doctor` with typed projected output. Status inspects the locked generation index, reports unverified storage state and shared profile aliases. Doctor reports native read health with unknown write access. Both preserve `live_verified: false` under output selection.
- CLI profile-context selection and development control export use shared applicability helpers. The existing selector handles explicit, repository and active profile precedence; invalid selectors/flags/projection fail before state access.
- `craft-runtime/src/profiles.rs` adds a bounded validated query for profiles sharing one issued grant.
- `craft-runtime/src/auth/input.rs` adds a separate bounded secret-aware pipe worker and strict provisioning decoder. Cancellation/deadline remain observable when a read blocks. Scratch and owned result buffers zeroize on drop; worker delivery after receiver cancellation does not expose bytes.
- `craft-platform` checks stdin's native type without reading it or changing terminal state. Windows uses a borrowed stdin handle with `GetFileType`; Unix source uses cloned-descriptor metadata. Only pipe/socket input is accepted. Unix compilation/native acceptance remains unrun.
- The reviewed development snapshot adds two local read operations, for thirteen operations total. [User contract](../auth-diagnostics.md) distinguishes local diagnostics from verified cloud access.

No new dependency, feature or Cargo version was needed. No public provision/login/refresh/logout command was enabled.

## Observed checks

Focused secure-input tests passed valid Unicode credentials, malformed/unknown/duplicate fields, empty/oversized bytes and a failing reader whose error contains a sentinel. Ongoing cancellation is triggered after a blocked worker starts reading; deadline and cancellation return safe typed errors. A child process exercised the actual native stdin pipe check and accepted strict synthetic input without echo. Another child received a regular file as stdin and refused it. All fixture credentials are synthetic.

CLI negative tests passed invalid selectors, conflicting profile aliases, unknown secret flags, unsupported verification/secret input fields and unknown output fields before state creation. The Windows positive case exercises absent credentials, exact shared-grant aliases excluding an unrelated grant, active/explicit selection, doctor metadata and actual output validation against matching selected response schemas. These commands make no OAuth/product request.

The first full `python tools/verify.py` gate passed before the final worker cancellation checks and index-only diagnostic regression were added. The final native-access gate passed **177 named Rust tests, zero failures and two ignored opt-in real-vault tests**, plus snapshot comparison, ten contract controls, formatting, locked inventory/layer checks and all-target Clippy. Its log is `target/auth-diagnostics-verification-output.log`; the earlier passing checkpoint is `target/auth-diagnostics-initial-verification-output.log`. A failed intermediate gate caught a reference-plus-constant projection shape. Doctor's write-access schema now uses a scalar constant retaining the unknown-only restriction; `target/auth-diagnostics-projection-failed.log` preserves the failure. An attempted field-inline annotation was rejected by the pinned schema derive and was removed.

`cargo build --workspace --locked --offline` passed. A direct command-search smoke check found both implemented auth reads. Quality/planning selftests passed nine/fifteen controls. The locked inventory remains 278 packages, with no dependency/feature/version change. Incremental-cache hard-link warnings used copying and did not fail the gate.

| Checkpoint | SHA-256 |
| --- | --- |
| `target/debug/craft.exe` | `8d234a6686cd5809dd3e7b7c385c2a50dbc9e59fe735aa347a7975a9aa3dadc9` |
| `schemas/local-contracts.json` | `79f9e192fe325890277544c34bac399105a42ed5b8abaa1e681aa5d277f02e9c` |
| `crates/craft-cli/src/auth.rs` | `cd7785364651bbfdd9cfa94865c51d292b39f703e0b0d670fd24f05435e3fb3c` |
| `crates/craft-runtime/src/auth/input.rs` | `587571f4d1d0aa77489a3aee77d2354e53fdd73632473bf59074b1e16ecf360b` |

## Scope and limits

These commands intentionally report stored generations as unverified. Read health is not write permission or credential validity. Status does not load token chunks; the generation fixture regression arms a second vault-read failure and proves inspection consumes only one index read and performs no credential write.

The input worker retains bounded memory while a native read is blocked. A handled return drops buffers when delivery or the read completes; CLI process exit releases a still-blocked worker. Immediate zeroization inside an uninterruptible blocked read, internal standard-I/O/parser allocations and forced process death are not promised. Native pipe type does not authenticate or encrypt its producer. The pipe adapter does not provide the separate interactive no-echo prompt.

The Windows diagnostic gate uses read-only probes under the current principal and randomly namespaced test installations; it does not create native credential entries. Existing opt-in actual-vault write acceptance remains separate. Service/container sessions, Linux/macOS native backends, live scopes/permissions, prompt qualification, consequential auth command review and provider writes remain unfinished. No real credential was read from user input, OAuth credential provisioned, cloud resource changed, commit/push made or release published.
