# Live read conformance

The `craft-live-conformance` crate checks provider request construction and response parsing against a real Atlassian Cloud site. Mocked fixtures are only as good as their resemblance to the service; this harness is how that resemblance is measured. It is opt-in and never part of the ordinary or full verification gate.

## What it exercises

Each case prepares a request through the provider registry, so input and target validation are the real ones. It then calls the provider's own read entry point (the function the production host calls) with a live context. That context sends the provider's declared route to the site origin and returns the real response to the provider's parser. Two pages are read where a list reports more, so continuation handling runs against live data.

It does not exercise the production host: there is no profile, policy, journal, service-account OAuth or `api.atlassian.com` gateway. Requests use an Atlassian account API token over HTTP Basic, sent to `https://<site>.atlassian.net` with the same REST paths that the gateway serves. Continuation links that the gateway would rewrite are therefore not covered, and a pass says nothing about service-account scopes or permissions.

Only routes whose descriptor declares a read effect can be sent. A read that uses POST, such as JQL search, is allowed on that basis; every other non-GET route is refused unless a scenario explicitly enables writes.

## Save the credential

Run this yourself in an interactive PowerShell window from `cli/`:

```powershell
powershell -ExecutionPolicy Bypass -File tools\live-credential.ps1
```

It asks for the site origin, the account email and an API token created at the Atlassian account security page (a classic token, not a scoped one). The token is read without echo and stored as one Windows Credential Manager generic credential, `craft-live-conformance`, with local-machine persistence for the current user. It never reaches disk, argv, environment variables or the repository. The site origin is written to `cli/.craft-live.json`, which is gitignored. Use `-Check` to confirm a credential exists without printing it and `-Remove` to delete it.

On other platforms, or to override the stored values for one run, set `CRAFT_LIVE_SITE`, `CRAFT_LIVE_EMAIL` and `CRAFT_LIVE_TOKEN` in the environment.

## Run it

```powershell
$env:CRAFT_LIVE = '1'
cargo test -p craft-live-conformance --offline --test live_reads -- --ignored --nocapture
```

The test is `#[ignore]` and additionally fails unless `CRAFT_LIVE=1`, so a broad `--ignored` run cannot reach a tenant by accident. Identifiers (project, issue, comment, attachment, link, space, page, footer comment) are discovered from the site at run time. A command whose data the site lacks is reported as skipped with the reason, never as a pass.

Output is a per-command PASS/FAIL/SKIP summary with the method, path and status of each exchange. Complete request paths, response bodies and a value-free shape of each body are written to `target/live-captures/`, which is never tracked. Captures contain tenant data; do not copy them into tracked files.

`tests/live_probe.rs` prints the status and top-level keys (not values) of arbitrary GET paths named in `CRAFT_LIVE_PROBE`, for checking a hypothesis about a response shape.

## Write lifecycle

`tests/live_writes.rs` changes the site, so run it only with the site owner's agreement:

```powershell
$env:CRAFT_LIVE = '1'
cargo test -p craft-live-conformance --offline --test live_writes -- --ignored --nocapture
```

It creates two Jira issues and a Confluence page titled `craft-test live <timestamp>`, runs provider comment, link, issue edit/assign/unassign/delete and page-create writes through the same prepare, send and interpret sequence the hosts use, reads back comments, links, an attachment and Confluence comment replies, and removes everything at the end even when a step fails. It never edits or deletes data it did not create. Its summary is written to `target/live-captures/writes-summary.txt`.

## Replaying captures

`ReplayContext` serves recorded bodies in order and `prepare_offline` prepares a request against a synthetic tenant, so a provider fix can be checked against captured live data without network access, or a rejected record can be isolated. A divergence that is fixed must also gain a deterministic fixture in the provider's own tests that mirrors the observed shape, since captures are local and untracked.

[Evidence](evidence/live-read-conformance.md) records the latest run.
