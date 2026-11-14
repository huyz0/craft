# Remote plan and design import

Implemented on 10 November 2026. `plan page-get` and `design page-get` delegate to the existing verified Confluence current-page read host under the saved native page-get allowance. They import the full visible storage/ADF body without reading a local file. Document tenant and space must match the observed page. The native Confluence title overrides the duplicated standalone heading before semantic validation/digesting.

## Observed focused acceptance

- `target/remote-document-unit-final.log`: two passing unit tests cover human acceptance/title preservation and a seven-mode error matrix: unsupported content, ambiguous structure, mismatched target, mismatched page identity, oversized selected document, cancellation and native failure. Errors return no partial document and preserve the credential-cache receipt. Projected schemas validate actual success/failure responses. The initial unit compile used incorrect control-constructor/cancellation APIs; its log is retained as `target/remote-document-unit.log`.
- `target/remote-document-input.log`: public subprocess checks pass invalid IDs, unknown projected fields and insufficient recovery budgets for both commands, before state creation.
- `target/remote-document-host.log`: five owned HTTPS modes pass plan storage, plan ADF, design storage, unsupported body and mismatched document space. Real captured requests use current-page GET with the selected body representation and no resource write. Token renewal occurs and its confirmed receipt survives both successful and failed imports. Imported human changes and native title are retained; approved display labels do not grant authority.
- `target/remote-document-contracts.log`: generated public contracts contain 36 operations. Default output omits the document; explicit selection retains mandatory source and cache metadata.

## Independent review

Authorized read-only reviewer `/root/activation_review` found no concrete blocker in input/projection/budget preflight, native delegation, current-page/source checks, complete visible validation, title precedence, fingerprints or recovery output. No test, state, credential or tenant access occurred during review. Tests were still in progress at review time; this is source review, not live qualification.

## Limits

The returned source snapshot is not a persisted approval artifact. No human eligibility, review-comment evidence, included-design revision qualification or Jira materialization is established. Live two-human editing is unqualified. Existing page update remains unavailable pending editor-draft qualification. No dependency, lockfile, private schema or credential format changed. Owning publication/import tasks remain unchecked.

## Final selected qualification

`python tools/verify.py` passed eight impact-selected checks in `target/remote-document-verification.log`: planning, contract comparison and selftests, formatting, all default CLI tests, default Clippy, all nine composed hosts and feature Clippy. This is focused qualification, not a full-workspace or live-provider pass. `cargo build --workspace --locked --offline` restored the default executable in `target/remote-document-build.log`.

Actual executable discovery returns 36 commands and two providers in `target/remote-document-discovery.json`. Binary SHA-256: `28043962525065c5bca13b11234b53e00868fa2749373daab4456ca57e6f50e0`. Contract SHA-256: `3fc39360d042a57dfc62ffbd206d185e6e8ebf2e073bd30a099d28fa67d1cec1`. Machine-readable identity and qualified source hashes are retained in `target/remote-document-result.json` and `target/remote-document-qualified-source-hashes.json`. Final prose-only validation is recorded in `target/remote-document-documentation.log`.
