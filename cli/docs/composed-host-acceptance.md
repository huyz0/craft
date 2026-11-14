# Composed create-host acceptance

The integration target executes the actual `PreparedCreate::execute_cached` host, service-token cache, metadata/account adapters, HTTPS transport, SQLite send claims and completion against owned loopback TLS servers. It closes the earlier gap between separately qualified layers. It does not invoke a public cloud command or use live Atlassian credentials.

## Explicit fixture boundary

`synthetic-acceptance` is a non-default development feature in the CLI/runtime crates. The integration target requires it explicitly. Builds without debug assertions refuse it at compile time. Default builds omit the fixture constructor, routing markers and in-memory credential implementation. No parser flag, profile setting, environment variable or arbitrary origin selects this path.

`ServiceFixture` takes an owned profile store, two localhost ports, trusted fixture CA roots, invocation control and a clock. It supplies only fixed synthetic provisioning values. Its client refuses different credentials before exchange and refuses different bearer values in issuer responses. Cache reopening checks credentials and bearer before probing a product. A private routing marker stays on tokens/sessions; identity probes require matching client provenance, and an ordinary bearer cannot acquire a loopback marker.

The fixture stores generation chunks in an isolated bounded memory vault and uses actual native grant locks. It cannot select or read the installed native vault. Normal request construction still validates fixed provider/tenant/path/routes, applies TLS verification and consumes durable claims. Only marked synthetic sessions change their fixed prefix's destination to localhost.

The CLI integration reuses the runtime's TLS test fixture source, not a copy of production host code or a fake host. Certificate-generation dependencies remain dev-only. The actual host and runtime libraries supply the behavior being qualified.

## Run and inspect

From `cli/`, with native loopback/link access:

```text
cargo test -p craft-cli --test jira_create_host --features synthetic-acceptance --locked --offline
cargo test -p craft-runtime --lib --features synthetic-acceptance --locked --offline
python tools/synthetic-acceptance.py
python tools/verify.py
cargo build --workspace --locked --offline --no-default-features
```

The development gate includes composed integration, runtime feature regressions, default/feature Clippy and isolation checks. The checker verifies explicit target/default-feature separation, no fixture-only packages in the normal CLI dependency tree and actual compiler refusal without debug assertions. This expected refusal is retained in `target/synthetic-acceptance-nondebug-output.log`.

Feature testing can rebuild development artifacts with the fixture feature. Finish a deliverable checkpoint with the default build above. This does not establish release packaging or distribution readiness.

## Evidence and limits

The [acceptance record](evidence/composed-host-acceptance.md) records exact results. Cases inspect complete wire requests, recovered journal rows, projected output, cache-generation changes, refused replay and successful fixture cleanup. A claim-sensitive clock tests metadata/token expiry and cancellation only after the committed marker. A request-sensitive clock waits for the actual stalled read before expiring the invocation, avoiding TLS timing assumptions.

This qualifies actual generation/cache algorithms over memory credentials, native locks and SQLite on the tested Windows host. It does not replace native-vault process acceptance, other platforms, independent consequential review, live tenant qualification, public parser/dispatch or provider reconciliation. Production auth and cloud commands remain unavailable.
