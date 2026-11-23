# Shared execution service

The PostgreSQL queue is the shared execution authority for this milestone. The migration creates a paused queue. Each claim reserves review capacity and an attempt token allowance. Runners receive only work for repositories they are enrolled in. Use one queue per team or trust boundary.

Run the integration test from a host with Python, `psql`, and a fresh disposable PostgreSQL database. Set the usual `PGHOST`, `PGPORT`, `PGUSER`, and `PGDATABASE` connection settings. Use a PostgreSQL password file or another approved authentication method. The test creates the `craft_queue` schema and refuses a database that already has it.

```powershell
python factory/shared/test_queue.py
```

The test races two database clients, makes the winning lease expire, and verifies that only the new generation can submit. It also tests claim retries, repository scope, review capacity, cancellation, and the token reservation budget. Each call opens a new database connection.

## Authority and recovery

- Use the PostgreSQL clock to decide expiry. A takeover increments the generation and preserves the previous attempt. A lost claim response can be retried with the same request UUID.
- The review slot stays reserved from the first claim until cancellation, standalone merge closure, or release-series completion for that order. An expired lease keeps the slot for takeover. Cancel an abandoned order to free the slot after recording the reason.
- Each claim reserves a token allowance. Budget calculations charge reported usage when available and the full reservation while usage is unknown. Expiry and cancellation do not erase possible spend.
- The queue stores versioned order payloads. Runners receive a repository identity and resolve it to a local checkout. The claim omits the submitter's local filesystem path.
- Leave PostgreSQL reachable only by the service. The migration revokes public schema and function access. The HTTPS service must derive runner identity from authentication and enforce operator-only access to admission and cancellation. A candidate stored here still requires independent verification and the existing forge approval workflow.

### Admit a GitHub CI failure

An operator can admit a selected, still-current failed GitHub job to the shared queue. Use a work-order template with the exact failure commit in a trusted local checkout and a GitHub policy that names the workflow and required job. Use the approved `sha256:` digests of both files. If the work order pins a Jira snapshot, pass its local file with `--jira-snapshot`.

```powershell
craft-factory shared-work --queue https://localhost:8443 --ca queue-ca.pem admit-github-failure repair-order.json --order-digest $orderDigest --policy github-policy.json --policy-digest $policyDigest --gh $gh --run 73 --attempt 2 --token-limit 40000
```

The command reads the current branch, workflow run, and approved job before admission. The queue uses a stable event key for the exact job attempt, so repeating the same admission returns the original work order. The shared queue retains dispatch and review capacity.

For an enrolled runner, add `github_ci` to its trusted repository map entry. Set `policy` to the approved policy file and `gh` to the absolute path of the read-only GitHub CLI. The launcher checks the policy digest and re-reads the original failure before starting the agent. Shared CI orders need a policy pin to launch.

With migration 087, a runner that observes a moved branch or a definitively newer run can hold the order before starting the agent. The hold releases review capacity and records zero token usage for that attempt. Use `dispatch-status` to see the reason and generation. A provider outage or local policy error leaves the lease eligible for normal retry. Once the operator reviews the evidence, cancel obsolete work or release the exact hold to recheck it on a new claim.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem retry-ci-preflight $workId --generation $generation --evidence $decisionReference
```

With migration 086, a pinned CI repair needs both an independent pass and a native GitHub check receipt before reviewer assignment. Record the independent verdict, prepare a fresh verification handoff with its new sequence, and [bind the draft](#bind-a-github-draft-for-shared-review). After GitHub finishes the required checks, use the verifier credential to attach their check run IDs to that verdict and publication.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem attest-github-ci-checks --handoff $currentHandoff --policy github-policy.json --gh $gh
```

The verifier checks the pinned policy, exact PR head and base, expected GitHub App IDs, and current success of every required check. It also re-reads the original failure and rejects protected CI changes. If the candidate or independent verdict changes, record a fresh native receipt. The operator audit retains earlier receipts.

### Admit a Bitbucket Pipelines failure

Migration 088 adds shared admission and native verification for Bitbucket Cloud. Use the approved digests of the repair order and [TWG policy](../examples/twg-bitbucket-policy.json). Pin the repair order to the destination branch commit. Include the failed step name in `required_steps` and use a saved TWG profile with read access to the repository, pull requests, and pipelines. Pass the approved snapshot with `--jira-snapshot` if the order pins one.

```powershell
craft-factory shared-work --queue https://localhost:8443 --ca queue-ca.pem admit-bitbucket-failure repair-order.json --order-digest $orderDigest --policy bitbucket-policy.json --policy-digest $policyDigest --build 14 --step $stepUuid --token-limit 40000
```

In the runner's repository map, set `bitbucket_ci` to the path of the approved policy file. The runner checks the digest and re-reads the exact build and step before launching. A moved branch or a newer build with a matching pipeline selector holds the order. If Bitbucket's response leaves the selector unclear, the runner exits before starting and the lease remains eligible for normal retry. The GitHub retry command above also releases a Bitbucket hold after operator review.

Record an independent pass, then prepare a fresh verification handoff. [Reconcile the Bitbucket draft](#reserve-and-reconcile-a-draft) with the exact candidate and wait for a PR-targeted pipeline to pass. Use the verifier credential and the successful PR build number to record its native evidence.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem attest-bitbucket-ci-checks --handoff $currentHandoff --policy bitbucket-policy.json --build 15
```

The verifier checks the PR, source and destination commits, pipeline UUID, and each required step. It rechecks the original failure and inspects candidate history for changes to protected pipeline definitions. The queue requires this evidence for the current candidate and independent verdict before reviewer assignment or merge closure. The operator audit retains earlier receipts.

### Approve recurring shared CI intake

Use migration 089 to run a recurring GitHub Actions or Bitbucket Pipelines lane through the shared queue. Before enabling it, finish any transfer from the local queue and stop the local CI admission schedule for the same repository. Enroll a runner with its provider policy and issue a separate queue identity with the `intake` role for the observer. Use the observer's read-only forge credential and keep the queue operator credential off the scheduled task.

Save a lane manifest with absolute paths and the approved digests of the work order and forge policy. For GitHub, use `"source": {"forge": "github", "gh": "/absolute/path/to/gh"}`. The GitHub workflow and job come from the pinned policy. Add `jira_snapshot` with an absolute path if the work order pins a Jira snapshot.

```json
{
  "schema_version": 1,
  "lane": "service-ci-shadow",
  "observer": "service-ci-observer",
  "order": "/srv/craft/service/repair-order.json",
  "order_digest": "sha256:<approved-order-digest>",
  "policy": "/srv/craft/service/bitbucket-policy.json",
  "policy_digest": "sha256:<approved-policy-digest>",
  "source": {"forge": "bitbucket", "step_name": "Verify candidate"},
  "follow_branch": true,
  "allow_admission": false,
  "daily_limit": 1,
  "token_limit": 40000
}
```

With `follow_branch` on, keep the observer and runner's trusted checkouts synchronized. Each must have the approved anchor and the latest commit objects to verify ancestry. With it off, the lane remains pinned to the exact work order base.

```powershell
craft-factory shared-work --queue $queue --ca $ca approve-ci-lane $manifest --digest $laneDigest --evidence $approvalReference
craft-factory shared-work --queue $queue --ca $ca poll-ci-lane $manifest --digest $laneDigest --intake-token-file $observerToken
```

Inspect the shadow output before promotion. For promotion, disable the shadow lane, approve a new manifest ID and digest with `allow_admission` set to true, then add `--admit` to the poll. One active lane per repository prevents overlapping configurations. Both flags are required for admission. The queue enforces deduplication against manually admitted failures, one outstanding CI repair per repository, the rolling 24-hour cap even across lane replacements, and shared review and token limits. Runners launch from the queue and independently recheck native evidence. Use `shared-ops audit` to see the work order's lane, observer, and approval reference.

```powershell
craft-factory shared-work --queue $queue --ca $ca disable-ci-lane $laneId --evidence $decisionReference
```

Disabling a lane stops new admissions from it. Use the queue's status and cancellation commands for work it already admitted. See the kit's deployment guide for the systemd timer or Windows scheduled task.

## Run the HTTPS service

Create a database and a separate restricted login for the HTTPS process, such as `craft_queue_api`. From the repository, run the migration helper with Python and `psql` on PATH. Point the standard PostgreSQL connection settings at the intended database and authenticate as the migration owner through your approved credential store.

```powershell
python factory/shared/migrate_queue.py --service-role craft_queue_api
```

From the extracted Windows kit, use `python .\shared\migrate_queue.py --service-role craft_queue_api`. The helper serializes migration sessions, stores a checksum for each migration, and grants schema usage and execution of the functions in [the service manifest](service-functions.txt). It clears old blanket function grants to the service login and `PUBLIC`. Stop the service during upgrades, rerun the helper from the same build you will deploy, and run `--check-config` with the service login before restarting. A failed migration rolls back with its ledger entry. A fresh queue remains paused for enrollment and setup.

For a queue installed manually before this helper was introduced, independently confirm the last applied migration from your deployment records. Run once with `--baseline-through <number>` to record that verified history and apply subsequent migrations. The ledger distinguishes those entries from migrations run by the helper.

Preflight checks the permissions of the service login and any roles it can assume through `SET ROLE`. If a reachable role grants direct access to queue tables or execution of an internal queue function, remove that access before restarting. Group membership that adds no disallowed permissions can remain.

Start the queue paused and [enroll the runner](#enroll-a-runner-for-a-repository) before resuming.

Supply the database URL through the service's secret store. To mount it as a file, set `CRAFT_DATABASE_URL_FILE` to the protected file path. Store one UTF-8 URL on its own line, outside the repository, with read access for the service account. The queue checks `CRAFT_DATABASE_URL` first, then `CRAFT_DATABASE_URL_FILE`, then the file named `CRAFT_DATABASE_URL` under `CREDENTIALS_DIRECTORY`. An unreadable configured file blocks startup. Use a private database connection with certificate verification for a networked database. Put the TLS certificate chain, private key, and identity configuration outside the repository. Start the service with the appropriate bind address.

```powershell
cargo run --locked --manifest-path factory/shared/Cargo.toml -- --bind 127.0.0.1:8443 --certificate server.pem --private-key key.pem --identities identities.json
```

From the extracted Windows shared-service kit, run the packaged executable with the same arguments.

```powershell
.\shared\craft-factory-queue.exe --bind 127.0.0.1:8443 --certificate server.pem --private-key key.pem --identities identities.json
```

From the extracted Linux kit, run `python3 shared/migrate_queue.py --service-role craft_queue_api` and use `./shared/craft-factory-queue` with the same service arguments.

Add `--check-config` to either command to validate the certificate and key, identity file, evidence catalogs, and service login without binding a port. Its JSON reports whether the queue is paused and whether an operator identity is configured. Every startup checks for missing functions, missing execute grants, direct table or column access, and permission to execute internal functions or delegate execution. It also compares the database's recorded migration names and checksums with those embedded in the service build. A missing, altered, or newer migration history blocks startup before the service accepts traffic or fences a restored queue. Use matching installer and service builds. If the helper has already run and permissions are still too broad, check the service login's inherited grants.

The identity file is a JSON array. Each entry has an `id`, a `role`, and a `token_sha256` field containing the SHA-256 hex digest of an independently generated random token. Supported roles include `operator`, `runner`, `verifier`, `reviewer`, `publisher`, `intake`, `diagnostician`, `release_observer`, `profile_scheduler`, and `metrics_reader`. Keep the original token in the client credential store. The database enrollment ID must match the runner identity.

Use `issue-queue-identity` to generate a random token into a new file and print its digest-only identity record. Run it as the intended recipient, using an existing protected directory outside Git. The launcher creates the token file with owner-only permissions on Unix or a protected owner-only ACL on Windows. Add the returned JSON object to the server's identity array and deliver the token file through your secret store. The command refuses to overwrite an existing token file.

```powershell
craft-factory issue-queue-identity reviewer-one --role reviewer --token-file $protectedTokenPath
```

With migration 085, use `--verifier-repository` to give a verifier access to exact repository keys. Repeat it for up to 32 distinct repositories. This restricts candidate status and packs, verdict submissions, shadow and merge receipts, and merge-check lists. The identity record stores the list as `verifier_repositories`. An existing verifier record without the field has access to the whole queue. Reload identities after changing a verifier's scope.

```powershell
craft-factory issue-queue-identity payments-verifier --role verifier --verifier-repository team/payments --token-file $protectedTokenPath
```

For launcher clients, prefer a token file from your secret store. For example, set `CRAFT_FACTORY_REVIEWER_TOKEN_FILE` to the path of the reviewer token file. Every launcher queue token variable supports this `_FILE` form. Store just the bearer token on one line, restrict the file to the launcher account, and keep it outside repositories, worker directories, and agent homes. The launcher checks the direct token variable first, then `_FILE`, then the file with that token variable's name under `CREDENTIALS_DIRECTORY`. Restart a persistent runner after rotating its token.

To activate a rotation or revocation, atomically replace the configured identity file on the queue host. Preserve an operator identity in the replacement. Then use a currently authorized operator token to load the exact replacement by digest.

```powershell
$identityDigest = 'sha256:' + (Get-FileHash -LiteralPath identities.json -Algorithm SHA256).Hash.ToLowerInvariant()
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem reload-identities --digest $identityDigest
```

An invalid replacement leaves the running queue's current identities active. New requests use the new identities as soon as reload succeeds. Already authenticated requests can finish. A rotated runner must reopen its queue connection with the new token before it can renew its lease. The queue logs the operator ID and replacement digest.

## Admit and manage shared work

Perform [runner preflight](#run-a-worker-continuously) while a new queue is paused. Have the operator confirm the enrollment, approved work order and its `sha256:` digest, repository key, and token allowance before resuming. Use a stable event key from the approved work so retries address the same admission.

```powershell
$queueOptions = @('shared-work', '--queue', $queue, '--ca', $ca, '--operator-token-file', $operatorTokenFile)
craft-factory @queueOptions resume
craft-factory @queueOptions admit approved-work-order.json --order-digest $approvedDigest --repository team/service --event-key $eventKey --token-limit 40000
craft-factory @queueOptions status $workId
```

Use the exact queue repository key enrolled for the runner. Admission sends that key in place of an absolute local checkout path in the work order. If the work order already uses a repository key, it must match. Retry with the same inputs and event key if the response is lost. The queue returns the original work ID or blocks a conflicting reuse. Use the existing [verification handoff](#prepare-independent-verification) once the runner submits a candidate.

```powershell
craft-factory @queueOptions pause
craft-factory @queueOptions cancel $workId --reason 'Superseded by the approved replacement work order'
```

Pause stops new admissions and claims. Check outstanding work before maintenance. A cancellation can be retried with the original reason. The queue can hold cancellation until an ambiguous publication is reconciled. Resume follows the queue's recovery and cutover state.

### HTTPS request contract

Send JSON with `Authorization: Bearer <token>` over HTTPS and verify the server certificate. Use `POST /v1/operator` for `admit`, `cancel`, `set_paused`, and `status`. Use `POST /v1/runner` for `claim`, `renew`, and `submit_candidate`. Each request includes an `action` field. The service takes the runner ID from authentication. Workers send a request UUID for a claim and reuse it if the response is lost. Renew and submit requests include the work ID and lease generation.

| Action | Additional request fields |
| --- | --- |
| `admit` | `repository`, `event_key`, `work_order`, `token_limit` |
| `cancel` | `work_id`, `reason` |
| `set_paused` | `paused` |
| `status` | `work_id` |
| `claim` | `request_id` |
| `renew` | `work_id`, `generation` |
| `submit_candidate` | `work_id`, `generation`, `commit`, `evidence_ref`, optional `tokens` |
| `submit_pack` | `work_id`, `generation`, `commit`, `pack_base64`, optional `tokens` |
| `download_pack` | `work_id` |
| `record_verification` | `work_id`, `generation`, `head`, `digest`, `previous_sequence`, `passed`, `evidence`, `note` |

A claim returns `claim: null` when there is no eligible work. A stale renewal or submission returns HTTP 409. A worker can replay a successful submission with the same generation and payload if it loses the response. `submit_pack` accepts a Git pack up to 512 KiB and stores it atomically with the candidate. Only an operator can download it. Treat a downloaded pack as untrusted until a verifier confirms its digest, objects, ancestry, scope, and tests.

## Attach a launcher

Enroll a runner only for repositories present in its local checkout map. Use one dedicated OS account or sandbox with approved Codex access and Git checkouts. Pass a protected file with `--runner-token-file`, or supply the runner token through a service credential, `CRAFT_FACTORY_RUNNER_TOKEN_FILE`, or `CRAFT_FACTORY_RUNNER_TOKEN`. The explicit file takes precedence over the environment. The launcher reads the credential itself and starts Codex with an allowlisted environment.

### Enroll a runner for a repository

Apply migration 070 and use the matching queue build. Add the runner identity's digest to the [active identity file](#run-the-https-service) first. A token grants a runner identity. An enrollment grants that identity access to one queue repository key. Use the operator credential to inspect and change that enrollment while keeping the queue paused during setup.

```powershell
craft-factory shared-ops --queue $queue --ca $ca runner-enrollment runner-one team/service
craft-factory shared-ops --queue $queue --ca $ca set-runner-enrollment runner-one team/service --expected-revision 0 --decision grant --evidence CHANGE-123
```

Use the current revision shown by `runner-enrollment`. Existing database enrollments appear at revision zero until their first recorded decision. The queue returns the original receipt on an exact retry and rejects a conflicting stale decision. Verify that `runner_configured` and `enrolled` are both true, then run [preflight](#run-a-worker-continuously) on the runner.

```powershell
craft-factory shared-ops --queue $queue --ca $ca set-runner-enrollment runner-one team/service --expected-revision $currentRevision --decision revoke --evidence CHANGE-124
```

Revocation expires live leases for this runner and repository at the queue clock. The queue keeps attempt history and can offer the work to another eligible runner. Regranting access does not renew an old lease. For retirement across all repositories, use [runner lifecycle controls](#take-a-runner-out-of-rotation) and remove the identity. After restoring a database, reconcile restored enrollments with current approvals before resuming. During recovery, revocation remains available and new grants wait for recovery to finish.

### Prepare the checkout map

```json
{
  "bitbucket-workspace/repository": {
    "path": "D:/work/repository",
    "jira_snapshot": "D:/work/factory-inputs/approved-issue.json"
  }
}
```

`jira_snapshot` is optional for orders without a digest-pinned Jira snapshot. For a digest-pinned order, place the approved file at the mapped path. Keep the queue TLS trust, checkout map, and credentials in the runner's operator-managed configuration.

Install migration 023 and upgrade the service before the launcher. Each claim advertises mapped repositories whose absolute paths resolve to Git worktree roots. Missing paths, ordinary directories, and worktree subdirectories are skipped so other runners can claim their work. The queue intersects the advertised list with the authenticated runner's enrollment. A replay with a narrower map returns the original lease only if its repository is still listed. The launcher validates the pinned commit and any required snapshot when it prepares the selected work.

```powershell
craft-factory run-shared --queue https://localhost:8443 --ca queue-ca.pem --request-id $requestId --repository-map repository-map.json --destination $newAbsoluteAttemptDirectory --codex $codexExe --codex-home $dedicatedCodexHome
```

The launcher uses public trust roots by default. Pass `--ca` for a private PEM root. Set `--renew-seconds` below the configured lease duration. If the queue rejects renewal or becomes unreachable during execution, the launcher stops the worker. Retry an interrupted upload with the same request UUID and directory to resend the prepared pack without running Codex again. An incomplete run without a prepared submission needs a fresh request and directory after lease expiry.

The HTTPS integration test runs with Python, `psql`, OpenSSL, a compiled service binary, and a separate disposable database. Run `python factory/shared/test_service.py <service-binary> --launcher <launcher-binary>` with the owner connection in the `PG*` settings and the fixture login `craft_test_service` in `CRAFT_TEST_DATABASE_URL`. The launcher test also requires Git and the pinned Rust compiler for its synthetic agent. CI creates both test databases automatically.

### Run a worker continuously

With migration 069, check setup before starting a worker loop. Use the runner credential and the same checkout map, attempt root, and adapter options as the loop.

```powershell
craft-factory check-shared-runner --queue https://localhost:8443 --ca queue-ca.pem --repository-map repository-map.json --attempt-root $attemptRoot --codex $codexExe --codex-home $dedicatedCodexHome
```

The command checks adapter paths and whether the attempt directory is outside the mapped checkouts. It reports each Git worktree's availability and its enrollment for this runner. If configured, it reports whether the Jira snapshot file exists. For Claude, migration 129 lets it check for an active Claude approval on each enrolled repository. It exits unsuccessfully if the runner is draining or stopped, or has no usable enrolled worktree for the selected adapter. Queue pause is reported separately so setup can be checked before intake opens. The check reads enrollment and approval without taking a lease.

Start `run-shared-loop` under a process supervisor with the same runner credential, checkout map, and dedicated Codex home as `run-shared`. Give each runner and queue combination its own existing attempt root outside the mapped repositories. Keep the root when the process restarts so it can recover a prepared submission. If the previous process stopped before renaming the submission file, the loop checks the request ID, queue endpoint, and candidate pack digest before completing the rename and retrying the upload. The loop takes an exclusive lock on that root and supervises one attempt at a time.

With migration 073 and the matching launcher, every supervised worker reserves one start for its lease generation before it launches. Replaying a claim can recover the lease or resend a prepared submission. A second launcher using another attempt directory receives a conflict if that generation has already reserved its start. An expired lease can be taken over under a new generation, subject to the order's normal attempt and budget rules.

With migration 079, a launcher that reserved the start can report a failed or stopped worker as soon as it exits. The queue expires that generation's lease for takeover. The audit shows `worker_stop_reported_at`, and token accounting retains the reservation when the adapter provided no usable usage. If the launcher disappears before the report arrives, takeover follows the normal lease expiry.

On Unix, `SIGTERM` or `SIGINT` stops the loop after the current cycle returns. The active worker can finish within its normal attempt limit, and a pending submission remains available for replay after restart. An idle loop exits by its next poll. Send the signal to the launcher process so the active worker can finish.

```powershell
craft-factory run-shared-loop --queue https://localhost:8443 --ca queue-ca.pem --repository-map repository-map.json --attempt-root $attemptRoot --codex $codexExe --codex-home $dedicatedCodexHome --poll-seconds 15
```

The loop emits one JSON record per poll. Track `submitted`, `idle`, `retrying`, and `attempt_failed` by request ID. Connection failures before work starts keep the request ID. If the runner has a prepared pack, it can resend that pack with the original request ID after a restart. An interrupted attempt with a checkout but no prepared pack waits for normal lease expiry and queue pickup. Keep the attempt directory if you need its local evidence. Apply your retention policy to completed directories while leaving pending requests and their directories available.

Pass `--write-status` for a supervisor that does not keep standard output. The loop replaces `.runner-status.json` in the private attempt root under its exclusive lock. The report contains the loop state, Unix timestamps, and the last completed poll's state, request ID, and HTTP status if present. Failure and retry records carry fixed reason codes such as `candidate_pack_mismatch` and `queue_unreachable`. The report retains the most recent failed attempt in `last_failed_poll` and the most recent process failure in `last_failed_run`, including across restarts that reuse the same attempt directory. A forced stop or an error before the loop owns the root can leave an earlier report in place. Check the supervisor's process state and exit result as well.

Use `--max-polls` for a bounded run. To stop a running loop after its current attempt, create a `.stop` file in the root. Remove it before restarting. Forced termination uses the existing process containment and lease-expiry behavior.

### Attach a Claude Code runner

Apply migration 031 and refresh the queue service's function grants. Enroll the runner for the exact repository, set `"agent_adapter": "claude"` on the work order, and have an operator record the repository's approval to send source to Claude. Existing work orders default to Codex.

```bash
craft-factory shared-ops --queue https://queue.example.com approve-claude-repository team/service --approval-id claude-service-2026-01 --evidence CHANGE-123 --valid-seconds 86400
craft-factory shared-ops --queue https://queue.example.com revoke-claude-repository claude-service-2026-01 --evidence CHANGE-124
```

The queue records the approval ID on each attempt and caps the lease at the approval's expiry. Revocation ends its active leases and returns the work to the queue. The next pickup check returns naturally expired work to the queue as well. The runner checks the lease again before starting Claude and stops it when it detects lease loss. Requests already sent to Anthropic are outside the queue's control. Use a fresh approval ID after expiry, revocation, or database restore.

Use the native Linux Claude Code executable and Bubblewrap on Linux or WSL2. The Claude build must support the restricted print mode and structured output flags. Supply `ANTHROPIC_API_KEY` through the runner's secret store and keep the queue credential with the launcher. The same adapter flags work with the local `run` command and with `run-shared-loop`. [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference)

```bash
craft-factory run-shared-loop --queue https://queue.example.com --repository-map repository-map.json --attempt-root "$attempt_root" --claude "$claude_binary" --bubblewrap /usr/bin/bwrap
```

Craft mounts the disposable checkout into a separate filesystem namespace and makes its Git directory read-only. Claude uses file tools for the scoped edit. Shell commands, repository-discovered instructions, and MCP tools are excluded from the launch configuration. Craft passes the pinned skill and approved context in its existing work-order prompt. The isolated CLI retains its Anthropic connection for model calls. Use the independent verifier or CI to run tests on the resulting candidate.

With migration 112, an operator can [pin an execution profile](#pin-a-codex-execution-profile) on a Claude work order at admission. Craft passes the requested model and effort to Claude Code and applies the same timeout and attempt limits as Codex. Use an effort the selected Claude model supports. Craft accepts `low`, `medium`, `high`, `xhigh`, and `max` for Claude. The repository approval is still required for each lease. [Claude Code model and effort options](https://code.claude.com/docs/en/cli-reference)

### Take a runner out of rotation

Apply migration 024 and refresh the queue service's function grants. Use an operator credential to read the runner's mode, live lease count, and current revision before changing its mode.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem runner-status $runnerId
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem set-runner-mode $runnerId --expected-revision $revision --mode draining --evidence $decisionReference
```

In `draining` mode the runner can finish and submit its existing leases. It receives no new leases. Use `stopped` to expire its live leases immediately. The managed process stops when its next renewal is rejected. Another eligible runner can take over under the existing attempt and token limits. A stop revokes unused automatic fallback approvals tied to the leases it expired.

Use `active` to allow new claims again. Each change records the operator and evidence. Retrying the same command returns its original receipt. Fetch runner status to see the current mode after any later change. Runner mode applies to the identity across its enrolled repositories.

## Pin a Codex execution profile

An operator can add `execution_profile` when admitting a work order. Apply migration 019, grant the service login access to the new functions, and upgrade the service before rolling out the new launcher.

```json
{
  "execution_profile": {
    "id": "routine",
    "model": "approved-model-id",
    "reasoning_effort": "low",
    "timeout_seconds": 120,
    "max_attempts": 2,
    "approval_reference": "change:PROFILE-7"
  }
}
```

Use the model ID and effort your team has approved for the repository's data and checked in the runner's account. Keep the approval record at the referenced location. Standing Jira intake uses the runner's existing configuration. Runners that predate execution profiles can still claim ordinary work.

Every new lease generation uses one attempt, including takeovers after expiry and repairs. The queue permits one supervised start per generation, even if the claim request is replayed with a different attempt directory. Resending a prepared submission uses the existing attempt. The launcher stops the process at the shorter of the profile timeout and its own timeout. The queue still reserves tokens and accounts for reported usage. That accounting cannot interrupt a provider request after it starts.

If the adapter reports a completed turn with usage but the launcher rejects its result, the queue records usage from the authorized runner and expires the lease. With migration 115, reported usage above the allowance also counts against the rolling budget. The queue rejects the over-budget candidate and stores no pack. The token allowance reserves capacity and sets a candidate acceptance limit. It does not cap the provider's spend during a running turn. A timed-out or incomplete turn with no usage receipt retains its reservation.

Queue status shows the requested profile and whether the attempt limit blocks pickup. The attempt database records the requested model and effort and the effective timeout. Profile selection and any change of model are explicit operator decisions.

### Approve an escalation after an attempt cap

Apply migration 021 and refresh the queue service's function grants. After the last lease expires, an operator can approve one replacement for a standalone order with no publication history. Save the new execution profile object in `approved-profile.json`. It must change the requested model or reasoning effort. Record why the work needs a different profile at the evidence reference.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem escalate-profile $workId --generation $lastGeneration --replacement-id $newWorkId --profile approved-profile.json --evidence $decisionReference
```

The command admits the replacement and closes the original in one transaction. It copies the repository, work inputs, and per-attempt token reservation. The original attempts remain in the ledger and count against the queue's token budget within its existing accounting window. Audit either order to see the source generation, approval actor, evidence reference, and replacement ID. Active leases, standing pickup work, cross-repository release members, and work with a linked draft require their existing operator workflows.

### Schedule a preapproved fallback

Apply migration 022 and refresh the queue service's function grants. An operator can bind one fallback to a standalone work order before its last lease expires. The approval pins the replacement ID, profile, supporting evidence, and expiry. For example, this approval is valid for one hour.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem approve-profile-fallback $workId --replacement-id $newWorkId --profile approved-profile.json --evidence $decisionReference --valid-seconds 3600
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem revoke-profile-fallback $workId
```

Use the first command to approve and the second to withdraw an unused approval. Configure the scheduled process with an identity whose role is `profile_scheduler` and supply its credential through `CRAFT_FACTORY_PROFILE_SCHEDULER_TOKEN`.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem apply-profile-fallbacks
```

Each pass applies up to 20 eligible approvals while admission is open. The last lease must have expired at the pinned generation. The scheduler can apply those approvals only. The normal queue decides when a runner can claim the replacement under the token and review budgets. Audit the source order for the approving operator, scheduler identity, and outcome. If a replacement ID collides with existing work, the pass records a blocker for manual review. Restore recovery revokes unused approvals from the backup.

### Compare two execution profiles

Use an isolated evaluation repository with publication disabled. Admit each sample twice with distinct work and event IDs. Both orders in a pair must use the same repository, work inputs, and token reservation. Pin one adapter and execution profile for each side across all samples. With migration 113, the two sides can differ by adapter, model, effort, timeout, or attempt limit. Run both sides with the same launcher version and independent test harness.

Save one to 20 pairs in a JSON file. Give each pair a unique case ID and use each work order once.

```json
[
  {"case_id": "input-validation", "baseline": "sample-1-routine", "challenger": "sample-1-deep"}
]
```

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem compare-profiles pairs.json
```

Apply migration 113 and refresh the queue service's function grants to compare adapters. The command uses the operator credential to read current ledger evidence and identifies the adapter for each side.

The report scores the latest independent verdict for the current supervised candidate, bound to its generation, commit, and pack digest. It shows cases without this evidence as unscored, including work still running or awaiting review. Token totals cover all lease generations, including retries. Reported usage and reservations for attempts with missing usage appear in separate fields. Compare those fields alongside the unscored count before drawing conclusions. Use provider billing records for monetary charges.

### Inspect usage across a profile change

Use `profile-usage` with the operator credential to inspect a standalone profiled order or an escalation and its replacement. Either work order ID returns the same combined report.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem profile-usage $workId
```

The report shows each order's approved profile, latest applicable independent verdict, supervised starts, reported tokens, and reservations for attempts without usage. Totals include all recorded generations, including expired attempts on the original profile. An unscored original order with unknown token usage remains visible alongside any passing result on the replacement. Use the report to evaluate the escalation, and use provider billing records for monetary charges.

### Compare usage across repositories and profiles

With migration 078, `queue-usage` groups attempts by repository, adapter, and pinned model and effort. The window uses attempt start times and defaults to 24 hours. Omit `--repository` for the whole queue. `--limit` bounds the groups returned, while the totals cover the full selection.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem queue-usage --repository team/service --since-hours 168 --limit 20
```

Check `attempts_with_usage` and `reserved_tokens_without_usage` alongside `reported_tokens` to account for missing usage. Unprofiled attempts have empty model and effort fields. Candidate attempts count worker submissions. Use `compare-profiles` for independent quality verdicts on comparable work and provider billing for monetary charges.

With migration 116, `overrun_attempts` and `tokens_above_reservation` show the number of overruns and the excess tokens in each profile group and across the full selection. The `overruns` list gives the work ID, generation, runner, and usage for the largest overruns. `--limit` applies to this list as well as the profile groups. The reported token total already includes the excess.

## Prepare independent verification

Set `CRAFT_FACTORY_VERIFIER_TOKEN` through the verifier's credential store. Use a checkout map that points to the verifier's trusted source checkout and approved snapshot.

With migration 118, a verifier whose identity grants access to the repository can list candidates awaiting a first verdict. Each entry gives the work ID, generation, head, and pack digest. Pass `next_after_work_id` as `--after-work-id` to continue if more candidates are available. A candidate leaves this list when it receives a verdict, is superseded, or is cancelled. Failed checks appear in the operator's review inbox for resolution.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem pending-verifications team/service --limit 10
```

Use `prepare-next-verification` to open the first pending candidate directly. It uses the same scoped verifier credential, verifies the generation and pack digest, and requires that the candidate still awaits its first verdict. An empty inbox returns `status: idle` and leaves the destination untouched. Use `--after-work-id` to continue after a selected item or `--verifier-token-file` to read the credential from a protected file.

With migration 119, the queue reserves an automatic pickup for ten minutes and records `pickup_id` and `pickup_expires_at` in the handoff. Other verifiers skip that candidate in automatic pickup. Preparation failures attempt to release the reservation. To return an unused handoff sooner, run `shared-ops release-verification-pickup <repository> <pickup-id>` with the same scoped verifier credential. A verdict still goes through the independent verification checks. Explicit preparation by work ID supports operator-directed checks and reverification.

With migration 120, verdicts submitted from a pickup handoff require a repository-scoped verifier credential and a current pickup owned by that verifier. Expired, released, or pre-recovery pickups cannot record a fresh verdict. The verifier can retry an exact verdict already recorded under its pickup. Use a fresh pickup or an explicit handoff to start a new check. Stop the queue service before applying this migration and restart it with the updated queue binary, since the migration installer changes the service's function grants.

With migration 121, the owner or its verifier automation can renew an active pickup while checks run. Call `shared-ops renew-verification-pickup <repository> <pickup-id>` with the same verifier credential before the deadline. The response supplies the server's current `pickup_expires_at` and `renewable_until`. Each successful renewal keeps up to ten minutes on the pickup, capped at one hour from the original claim. The existing handoff remains usable with the same ID. An expired, released, or superseded pickup returns HTTP 409. Longer checks can use an explicit handoff.

Use `--verifier-token-file` with either preparation command, pickup renewal or release, and both attestation commands to use the same protected one-line credential throughout. For explicit preparation, also pass `--verifier` to select the verifier role. Restrict the file to the verifier account. A service may also use the mounted credential named `CRAFT_FACTORY_VERIFIER_TOKEN`.

```powershell
craft-factory prepare-next-verification --queue https://localhost:8443 --ca queue-ca.pem --repository team/service --repository-map verifier-map.json --destination $newAbsoluteVerificationDirectory
```

Use the explicit preparation command for a particular work ID and commit, including a candidate that needs reverification.

```powershell
craft-factory prepare-shared-verification --verifier --queue https://localhost:8443 --ca queue-ca.pem --work-id $workId --head $candidateSha --repository-map verifier-map.json --destination $newAbsoluteVerificationDirectory
```

The launcher checks the candidate's generation, digest, Git objects, ancestry, and changed paths before checking it out. It writes the pinned work order, a handoff receipt, and `review-brief.md` next to the detached checkout. The brief shows the approved intent and checks to run, counts the Git changes, and compares a repair to the exact previously reviewed commit. The receipt records the brief's path and SHA-256. Run the trusted acceptance checks against the prepared checkout in the approved test environment and record the observed result using the handoff and a stable evidence reference.

Keep the prepared checkout until attestation completes. Both attestation commands check that its HEAD still matches the handoff and its tracked files and index are clean. Untracked build output can remain. The check establishes the checkout state at submission. The verifier's evidence must identify the revision actually tested.

```powershell
craft-factory attest-shared-verification --queue https://localhost:8443 --ca queue-ca.pem --handoff $handoffJson --result passed --evidence $testResultReference --note 'Focused acceptance checks passed on the prepared revision'
```

With migration 071, add `--review-summary 'Checked absent input and confirmed the original value is preserved.'` to give reviewers a single-line account of the test and observed behavior. It accepts up to 240 characters and is bound to the same verification sequence as the evidence. The assigned-review brief shows it beside the evidence reference. Use `--note` for the full verification record.

The verifier role can use `POST /v1/verifier` for `download_pack`, `status`, and `record_verification`. The queue binds the decision to the candidate SHA, pack digest, generation, and authenticated verifier. It keeps prior results and rejects a new decision based on a superseded handoff. Use a fresh handoff after another decision has been recorded. Verification and publication remain separate decisions.

## Share an approved team skill

Have the skill owner review and test the Markdown. With migration 101, an operator records the approval and the repositories allowed to use it through the CLI. Pass the approved file's lowercase SHA-256 with the `sha256:` prefix. The launcher and PostgreSQL check the exact UTF-8 bytes, and the runner checks them again before adding them to the prompt.

```powershell
craft-factory shared-ops --queue $queue --ca $ca register-team-skill team:acceptance-checks SKILL.md --revision $approvedDigest --owner 'Example engineering team' --repository team/service --repository team/other --evidence 'decision:accepted-skill/3'
craft-factory shared-ops --queue $queue --ca $ca team-skill-release team:acceptance-checks $approvedDigest
```

Registration and inspection return metadata without the Markdown. A release is immutable. New revisions at the same location keep its owner and repository scope. Use a new location for a different scope or owner.

Set `skill.location` and `skill.revision` in each managed work order to the returned values. Repository-local `repo:` pins still work. An order using a `team:` pin requires a shared queue lease. A runner can fetch only the skill pinned to its own current lease using `POST /v1/runner` with action `team_skill`, `work_id`, and `generation`. The local attempt records the approval and revision without copying the Markdown into its receipt.

Skill, project context, and repair material all require current authorization for the same lease. If a release prerequisite or standing delegation becomes unavailable, the queue withholds new reads even before the lease expires.

To withdraw a release, use the operator credential, the exact revision, and a reference to the revocation decision.

```powershell
craft-factory shared-ops --queue $queue --ca $ca withdraw-team-skill team:acceptance-checks $approvedDigest --evidence 'decision:withdrawn-skill/4'
```

Revocation blocks new admission, fences active runners, and cancels affected work that can still be cancelled. A reserved publication cannot begin after revocation. With migration 099, pending publications and enqueues stay available for reconciliation, and work with a verified merge stays available for closeout. To resume development, approve a new skill release and delegate a new order pinned to it.

## Pin approved decisions and runbooks

Have the source owner approve the excerpt, its exact source revision, and its use by managed runners for the repository. Register only text authorized for that audience. The shared queue serves the stored excerpt to the runner that leases an order with the exact pin. It uses the runner's queue enrollment. The source connector's permissions apply when collecting and approving the excerpt.

### Prepare a Confluence excerpt

Use a trusted Craft CLI build and a saved Confluence profile authorized for `confluence.page.get` on the site. Save a policy with `schema_version: 1`, absolute `craft` and `craft_state` paths, `craft_profile`, the exact `cloud_id`, `space_id`, and `page_id`, and the intended `repository`, `location` such as `project:routing`, and `kind` of `decision` or `runbook`.

```powershell
craft-factory capture-confluence-context confluence-policy.json
craft-factory capture-confluence-context confluence-policy.json --excerpt reviewed-excerpt.txt
craft-factory capture-confluence-context confluence-policy.json --excerpt reviewed-excerpt.txt --approval owner-approval.json
```

The first read returns the published page title, version, and fingerprint. Write the exact text to share in a UTF-8 file of up to 4 KiB. Capture the `approval_digest` with `--excerpt`. Have the source owner review the page, repository, and excerpt, then record `schema_version: 2`, that digest, and an `approval_reference` identifying the review. Keep the receipt under the approving team's control. The last command rechecks the current page revision, destination, and exact excerpt bytes before producing `release`. Reissue any version 1 approval receipt before preparing a new release.

### Register the release

For Confluence, use the operator credential to recheck the source and register the approved excerpt in one command. The launcher sends the release only after it reads the published page and verifies the approval against the excerpt and destination.

```powershell
craft-factory shared-ops --queue $queue --ca $ca register-confluence-context confluence-policy.json --excerpt reviewed-excerpt.txt --approval owner-approval.json --replace $activeRevision
```

Use `check-confluence-context` to compare the published page version and fingerprint with the active pin. The report identifies a current pin, a changed source, a different registered binding, or an empty location. Add `--require-current` to make any of the last three outcomes fail a preflight or scheduled check. A context owner reviews any updated excerpt before an operator registers or withdraws a pin.

```powershell
craft-factory shared-ops --queue $queue --ca $ca check-confluence-context confluence-policy.json --require-current
```

For scheduled checks, issue a `context-reader` identity with one to 32 exact repository keys. Add its digest-only record to the queue identity file and reload identities. The launcher reads the active pin's metadata for the requested repository and location with this role. Keep the Confluence read profile separate from the queue reader credential.

```powershell
craft-factory issue-queue-identity context-scan --role context-reader --context-repository team/service --token-file $readerTokenFile
craft-factory shared-ops --queue $queue --ca $ca check-confluence-context confluence-policy.json --context-reader --context-reader-token-file $readerTokenFile --require-current
```

For a scheduled check, add `--report-file last-report.json` in an existing private directory outside the Craft state directory. A completed check replaces the report even when drift causes a nonzero exit. A queue or source error leaves the previous report in place. Use the scheduler's exit status and the report's modification time together when diagnosing a failed run.

With migration 039, operators can also register an owner-approved release from a prepared JSON file. Omit `--replace` for a location with no active release. To replace an existing release, pass its exact active revision. The queue retires the old pin and registers the replacement in one transaction. Repeating the same request with the same operator credential returns the original receipt while that release is active. Pin the returned `release.location` and `release.revision` on the work order.

```powershell
craft-factory shared-ops --queue $queue --ca $ca register-project-context prepared-context.json --replace $activeRevision
```

With migration 040, use the operator credential to inspect the active revisions for a repository or an exact historical revision. The response returns metadata without the excerpts and lists up to ten locations. Narrow by `--location` when looking up a specific pin. To withdraw a release without a replacement, give its exact revision and the evidence reference. Withdrawal or replacement fences work pinned to the retired revision. Migration 099 keeps pending publications and enqueues available for reconciliation and preserves verified merges for closeout.

```powershell
craft-factory shared-ops --queue $queue --ca $ca inspect-project-context team/service --location project:routing
craft-factory shared-ops --queue $queue --ca $ca withdraw-project-context team/service project:routing $activeRevision --evidence $ownerDecision
craft-factory shared-ops --queue $queue --ca $ca inspect-project-context team/service --location project:routing --revision $activeRevision
```

The administrator can also register an owner-approved record directly in PostgreSQL.

```sql
WITH approved(kind, source_ref, source_rev, approval_ref, excerpt) AS (
    VALUES ('decision', 'wiki:engineering/route-selection', '5', 'decision:route-selection/5',
            'Use the regional endpoint for requests from this service.')
)
INSERT INTO craft_queue.project_context_releases
    (repository, location, revision, kind, source_reference, source_revision, approval_reference, excerpt)
SELECT 'team/service', 'project:route-selection',
       craft_queue.project_context_revision(kind, source_ref, source_rev, approval_ref, excerpt),
       kind, source_ref, source_rev, approval_ref, excerpt
FROM approved
RETURNING location, revision;
```

Add the returned pair to the work order's `context` array. Each order can include up to four approved excerpts of 4 KiB each. Existing Jira and Git references still work. Managed launchers request `context_capable: true` when claiming and fetch the exact excerpts through the runner action `project_context`. Older launchers skip orders that require them. The receipt records the pins, source revisions, and approvals.

When the source owner supersedes or withdraws the excerpt, call `craft_queue.retire_project_context(repository, location, revision, evidence_reference)` as the queue administrator. Retire the old excerpt and insert its replacement in one database transaction when possible. Retirement fences active leases and cancels queued work pinned to the retired release. It blocks reserved sends and keeps unresolved publications available for reconciliation. Subsequent work orders need the replacement pin.

### Propose a skill or lesson for owner review

Apply migration 032 and refresh the queue service's function grants. Pass `--collect-knowledge` to `run-shared` or `run-shared-loop` to invite one optional suggestion from a successful Codex or Claude worker. The launcher validates it and records it after the queue accepts the candidate pack. The worker supplies content through its final structured result under the existing process restrictions.

The candidate receipt reports whether the proposal was recorded, rejected, or could not be confirmed. To retry an unconfirmed proposal, run `run-shared` with the original queue, request ID, and attempt directory while the work is still in the candidate state. Replay uses the same proposal ID from the saved submission. A proposal delivery issue leaves the candidate available for verification.

After uploading a candidate pack, the trusted runner can also submit a proposal from a file using its normal queue credential. The file contains the proposed content and a reference to the source evidence. The queue allows up to four proposals per generation and 20 pending proposals per repository.

```json
{
  "kind": "runbook",
  "location": "project:clock-check",
  "body": "Compare the clocks on both hosts before investigating a timestamp mismatch.",
  "evidence": "https://example.com/incident/evidence"
}
```

Use `decision` or `runbook` with a `project:` location, or `skill` with a `team:` location and the proposed skill Markdown in `body`.

```bash
craft-factory shared-ops --queue "$queue" propose-knowledge clock-check-1 --work-id "$order" --generation "$generation" --head "$head" --proposal proposal.json
craft-factory shared-ops --queue "$queue" pending-knowledge team/service
craft-factory shared-ops --queue "$queue" inspect-knowledge clock-check-1
craft-factory shared-ops --queue "$queue" audit "$order"
craft-factory shared-ops --queue "$queue" review-knowledge clock-check-1 --decision promote --proposal-digest "$digest" --verification-sequence "$verdict" --owner "Service owners" --evidence "$approval"
```

The inspection and decision commands use the operator credential. Take the proposal digest from `inspect-knowledge` and the verification sequence from `audit`. Have the named owner review that proposal and its supporting evidence in the team's approval system. Record their decision using its reference in `--evidence`. Promotion requires the latest passing independent verdict for the source candidate. The verdict covers the candidate. The owner reviews whether the proposed text or skill is suitable for reuse.

Use `--decision reject` and omit `--verification-sequence` to close a proposal without publishing it. A successful promotion returns `release_revision`, which can be pinned on future work orders. By default the release is scoped to the source repository.

For a cross-repository release, have the owner approve the exact list of repositories with the proposal digest. Pass a `--repository` flag for each approved repository, including the source. Craft accepts up to 16 distinct repositories and records their exact set with the decision.

```powershell
craft-factory shared-ops --queue $queue --ca $ca review-knowledge clock-check-1 --decision promote --proposal-digest $digest --verification-sequence $verdict --owner "Service owners" --evidence $approval --repository team/service --repository team/other
```

Craft publishes one skill revision scoped to the approved set. For a decision or runbook it publishes the same revision and source reference in each approved repository. A conflicting destination context blocks the whole promotion.

Use the promotion receipt's `release_revision` to withdraw the exact approved release across its recorded repository scope. The withdrawal receipt distinguishes the copies retired by this action from copies retired earlier. Newer context stays active, and pending publications must still be reconciled. Use the per-repository context command when the owner approves withdrawal in one repository only.

```powershell
craft-factory shared-ops --queue $queue --ca $ca withdraw-knowledge clock-check-1 --release-revision $releaseRevision --evidence $withdrawalDecision
```

Run `inspect-knowledge` to retrieve both decisions. If the withdrawal response is lost, retry it with the same proposal, revision, and decision reference.

## Pick up work under standing delegation

Use an approved rule for one source, project, repository, human assignee, ticket status, and opt-in label. A queue administrator records the approval, allowed path prefixes, token allowance, and eligible runner identities. Rules start disabled. Both runner identities must already be enrolled for the repository. Omit the fallback identity and delay if only the preferred runner may claim work.

```sql
INSERT INTO craft_queue.standing_rules (
    id, intake_id, source, project_key, repository, human_account, human_owner, required_label,
    eligible_status, allowed_paths, preferred_runner, fallback_runner, fallback_seconds,
    token_limit, approval_reference, enabled
) VALUES (
    'service-pickup', 'external-relay', 'tracker:example', 'SERVICE', 'team/service', 'human-account-id',
    'Service owner', 'automate', 'Ready', ARRAY['src'], 'preferred-runner', 'backup-runner',
    120, 100000, 'decision:approved-service-pickup/1', true
);
```

The relay reads the ticket and builds a JSON file with `snapshot` and, for eligible work, a pinned `work_order`. The snapshot contains `source`, `issue`, `source_revision` (an increasing integer per issue), `observed_at` (UTC Unix seconds), `assignee` (the native account ID or null), `status`, and `labels`. Preserve tracker revision order when relaying snapshots. The queue assigns the work ID and takes the human owner from the rule. It leaves the native assignee untouched.

```powershell
craft-factory observe-standing-ticket observation.json --queue https://localhost:8443 --ca queue-ca.pem --rule service-pickup
```

The command calls `POST /v1/intake` with action `observe`. Use a separate credential restricted to the approved relay. Pass its protected file with `--intake-token-file` or provide `CRAFT_FACTORY_INTAKE_TOKEN`. Supply fresh observations for work awaiting a claim or draft send. Observations older than five minutes cannot authorize those actions. Relay reassignment, removal of the opt-in label, or status changes to the original rule even after the ticket leaves its eligible set.

The queue checks review capacity and token budget at admission. The preferred runner has the first claim window. The fallback may claim after that window if the order is still available or the current lease has expired. Reassignment or revocation fences affected work. Existing unresolved publications remain available for reconciliation.

If the pinned work order changes, the queue fences the old work and returns `review_required`. After the human reviews the current snapshot, the queue administrator can call `craft_queue.approve_standing_intent(rule_id, issue_key, source_revision, approval_reference)` and have the relay re-submit that exact observation. To withdraw standing consent, call `craft_queue.disable_standing_rule(rule_id)`. Operator status shows the delegation and approval references retained for each order.

### Relay watched Jira tickets

With migration 100, an operator can approve a Jira standing rule from a digest-pinned JSON file. The rule binds a dedicated intake identity, Jira site, project, assignee, status and opt-in label to one repository. The preferred runner and any fallback must already be enrolled for that repository.

```json
{
  "id": "service-pickup",
  "intake_id": "jira-relay",
  "source": "jira:11111111-2222-3333-4444-555555555555",
  "project_key": "SERVICE",
  "repository": "team/service",
  "human_account": "human-account-id",
  "human_owner": "Service owner",
  "required_label": "automate",
  "eligible_status": "10001",
  "allowed_paths": ["src"],
  "preferred_runner": "preferred-runner",
  "token_limit": 100000,
  "approval_reference": "decision:approved-service-pickup/1"
}
```

```powershell
craft-factory shared-ops --queue $queue --ca $ca approve-jira-standing-rule rule.json --digest $ruleDigest
craft-factory shared-ops --queue $queue --ca $ca disable-jira-standing-rule service-pickup --evidence $withdrawalDecision
```

Use the file's lowercase SHA-256 with the `sha256:` prefix. The queue records the approving operator, policy digest, and withdrawal evidence. To change a rule, withdraw it and approve a policy with a new ID. Set both `fallback_runner` and `fallback_seconds` to allow takeover after the specified delay.

With migration 114, a rule can include an [execution profile](#pin-a-codex-execution-profile) and an optional `agent_adapter` of `codex` or `claude`. The default for a profiled rule is `codex`. Put the profile on the operator-approved rule and omit it from the relay's work-order template. The queue applies the rule's adapter, model, effort, timeout, and attempt limit to each admitted order. The rule's `token_limit` still applies. Claude also requires active repository approval. [Claude runner approval](#attach-a-claude-code-runner)

Use an existing Craft Jira profile with read access to the watched tickets. Set the rules' `source` to `jira:<Cloud ID>` and `eligible_status` to the native status ID. A relay policy has `schema_version: 1`, absolute `craft` and `craft_state` paths, `craft_profile`, `cloud_id`, and a `watches` array. Each watch supplies `rule`, the Jira `issue` key, its stable numeric `issue_id` as a string, an optional `approved_intent` digest, and the pinned `work_order`. Use the ordinary work-order schema for the template and let the queue assign the work ID and owner. A relay can watch up to 20 entries.

```powershell
craft-factory poll-standing-jira jira-watch.json
craft-factory poll-standing-jira jira-watch.json --queue https://localhost:8443 --ca queue-ca.pem
```

Run the first command to inspect the issue identity, native status, and digest of the summary and description. Once the human reviews the Jira content and work order, copy `snapshot.intent_digest` to the watch's `approved_intent`. The second command submits to the queue using `CRAFT_FACTORY_INTAKE_TOKEN`. It sends the content digest and work-order template. Jira access remains read-only. Configure an operator-owned scheduler to rerun the command within the queue's five-minute window. The Linux kit includes a user timer in `deploy/systemd`. Keep watching reassigned tickets long enough to withdraw outstanding work.

Migration 124 enforces the Jira intent fields for any relay using a Jira Cloud rule. A work order must carry exactly one `jira-intent:<Cloud ID>/<issue ID>` context pin matching the observed digest. Invalid submissions leave the prior observation and its expiry unchanged. During upgrade, the queue fences any historical Jira work admitted without an intent digest and preserves its records. For affected tickets, observe a later Jira revision and use the replacement approval procedure before resuming delegated work.

Capture stdout even when the relay exits with status 1. The JSON report keeps successful observations and queue decisions for the batch and lists individual watch failures in `errors` by issue, rule, and stage. A read or identity mismatch sends no observation for that watch. A failed submission can still show the snapshot without a queue decision. Later watches continue, and the next scheduled run can safely repeat the same watches. Queue decisions such as `review_required` appear with their observation and use the usual human review path.

Each Jira connector process has a 12-second deadline and a 64 KiB output limit. The relay stops the affected process tree when either limit is reached and continues with the remaining watches or discovery queries.

Set `--budget-seconds` for scheduled batches that need to finish before a supervisor stops the relay. The initial pass starts with watches. It defers an entry when it cannot fit the next read and, for a watch connected to the queue, its submission timeout. Completed observations remain in the report. Deferred entries appear under `errors` with stage `budget` and make the command exit with status 1.

Add `--cursor-db` with a private SQLite path to start the next pass at the first deferred watch or discovery query whose timeout fits the configured budget. The relay wraps to the start and attempts each entry at most once per pass. A complete pass or a change to the watches' identities, discovery queries, or queue resets the starting position. Use one cursor file per scheduler and queue.

For a scheduler that runs the launcher directly, `--intake-token-file` reads the queue credential from an explicit protected path. `--report-file` replaces the previous completed report with the current JSON, including partial results. Use a separate report path in a private directory. When the relay stops before producing a report, the previous file stays in place.

Apply migration 037 to enforce the five-minute freshness window throughout a standing attempt. Once the last valid observation ages out, the queue declines claim replay, worker start authorization, renewal, first candidate submission, and draft send authorization. The launcher stops a running worker when renewal is declined. A fresh eligible observation restores authorization while the lease still exists. An already recorded candidate pack remains available for exact submission replay.

When the native description or summary changes, the queue fences outstanding work and reports `review_required`. Have the human review the new content and update the work order and `approved_intent`. Poll again to get the `decision.review_token` for that exact replacement. Use the operator credential to record the decision, then relay the same observation again. An older token or revision cannot authorize replacement work.

```powershell
craft-factory shared-ops --queue $queue --ca $ca approve-jira-standing-intent service-pickup --issue $issue --source-revision $sourceRevision --review-token $reviewToken --evidence $approvalReference
```

The relay checks the configured Cloud ID, exact Jira key, and stable numeric issue ID. A missing field, access error, or moved issue stops the poll and leaves earlier submissions intact. Failed reads do not extend the queue's freshness window. [Jira Cloud Get issue](https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issues/#api-rest-api-3-issue-issueidorkey-get)

### Relay a watched GitHub issue

Migration 125 adds standing delegation for GitHub.com issues. Use `approve-standing-rule` with the SHA-256 of an operator-reviewed rule file. The [Jira rule example](#relay-watched-jira-tickets) has the same fields. For GitHub, set `source` to `github:OWNER/REPO` using the repository's canonical casing, `project_key` to `GITHUB`, `human_account` to the assignee's numeric GitHub ID as a string, and `eligible_status` to `open`. Use an enrolled runner and a separate queue intake identity. The delivery repository and allowed paths are pinned by the rule.

Set up a read-only GitHub CLI session with access to Issues. Save a watch file with `rule` and an `issue` object using the [GitHub issue policy fields](../README.md#delegate-a-github-issue). Use a work-order template whose `issue` is `OWNER/REPO#NUMBER`. The queue uses `GITHUB-NUMBER` as the rule's issue key and retains the source repository and stable REST issue ID in the intent pin.

```powershell
craft-factory shared-ops --queue $queue --ca $ca approve-standing-rule github-rule.json --digest $ruleDigest
craft-factory poll-standing-github-issue github-watch.json work-order-template.json
craft-factory poll-standing-github-issue github-watch.json work-order-template.json --queue $queue --ca $ca --intake-token-file $relayTokenFile
```

Add `--report-file last-report.json` in the scheduler's private working directory to keep the last completed decision. A failed read or queue request leaves the previous report in place. If a request completed before the report write failed, the next poll can safely replay the observation.

Review the issue and work-order template using the preview. Submit the first observation to open review. Copy `snapshot.intent_digest` to `approved_intent` in the watch file and submit again to confirm the reviewed source. Starting with migration 126, the queue requires an operator decision on the first work order and on any changed intent or work order. Use the rule ID, `GITHUB-NUMBER`, `snapshot.source_revision`, `decision.review_token` from the confirmed poll, and your decision evidence.

```powershell
craft-factory shared-ops --queue $queue --ca $ca approve-standing-intent $ruleId --issue GITHUB-NUMBER --source-revision $revision --review-token $reviewToken --evidence $evidence
```

The relay can then replay the approved observation. Configure its scheduler to poll within the queue's five-minute freshness window. The watch is eligible only while the issue is open, carries the required label, and has exactly the approved assignee. Closure, label removal, and reassignment fence existing work. Failed reads supply no fresh authorization. Use `disable-standing-rule` with the rule ID and evidence to withdraw the rule.

GitHub revisions use `updated_at` at second precision. A timestamp-only change keeps the same approved input. Conflicting source observations at the same revision fence work and require a later revision before the queue can accept a replacement. When the queue reports `review_required`, repeat the confirmation and operator approval steps for the changed input.

If you previously configured a generic relay with a `github:` source, migration 125 fences its observed work and preserves its records. A later native revision and the replacement approval procedure are required to resume.

Migration 126 also fences active GitHub standing work created before the queue retained the approved input digest. Use a later GitHub revision and record a new operator decision before resuming that work.

### Discover Jira issues for review

Add up to five `discoveries` to the relay policy. Each entry has a `rule` and a bounded `jql` query. For example, use `{"rule":"service-pickup","jql":"project = SERVICE AND labels = automate ORDER BY updated DESC"}`. You can run discovery with an empty `watches` array.

Each query reads one live page of up to 20 issue keys and stable IDs. The relay omits issues already watched by that rule and reports `has_more` when the result continues. Use a narrower query when you need to review every match. Discovery only suggests new watches, even when the relay is connected to a queue. A human can open a suggestion in Jira, approve its exact intent and work order, and add it to `watches` using the existing approval procedure.

## Return a reviewed draft for repair

Use the operator credential to record the change request against the confirmed draft URL and the exact generation and commit that received feedback. Use a stable reference to the feedback and include the specific change the runner should make.

```powershell
craft-factory request-shared-repair --queue https://localhost:8443 --ca queue-ca.pem --work-id $workId --generation $generation --head $reviewedSha --draft $draftUrl --evidence $feedbackReference --note $requestedChange
```

The order keeps its review slot while it waits for a runner. The runner reconstructs the reviewed revision from the stored pack. The verifier can reconstruct the repaired candidate independently from the original pinned base and checks that it descends from the reviewed revision. Any new verification applies only to the repaired generation. `shared-ops summary` reports the number of queued repairs.

The queue serves the feedback and pack only to the current runner while its lease and work permissions are current. If a release prerequisite or standing delegation becomes unavailable after pickup, the runner cannot fetch the reviewed material until that prerequisite is restored or an authorized runner takes over.

| Endpoint | Action | Additional fields |
| --- | --- | --- |
| `/v1/operator` | `request_repair` | `work_id`, `generation`, `head`, `native_url`, `evidence`, `note` |
| `/v1/runner` | `repair_context` | `work_id`, `generation` |

The publication record preserves the draft URL and reviewed commit. After fresh verification, use a draft update reservation to send the repair to that same PR.

## Transfer local authority

Use the same SQLite file the local launcher has been using. Stop old scheduler invocations and retire launcher binaries that predate the cutover fence. Drain local work and reconcile any publication hold. Pause the destination shared queue and drain its work. Keep a backup of the existing local database and use one UUID for the entire transfer.

```powershell
craft-factory --state $localDb cutover-shared --queue https://localhost:8443 --ca queue-ca.pem --epoch $epoch
```

The command acquires the local execution lock, freezes local admission, and persists an archive of terminal work-order receipts before sending it to the service. The service checks its own pause and drain state, stores the archive, and enables shared admission. If activation is interrupted, rerun with the same database, queue origin, and epoch. The local freeze remains in effect until rollback is confirmed. The original local receipts remain readable throughout the transfer.

To return authority, run the same command with `--rollback`. If it reports `shared_draining`, shared admission is paused and the local queue stays frozen. Resolve the remaining shared orders, then rerun the command. Once it reports `local`, the service stays paused and the local launcher restores its previous CI intake setting. Each authority retains its historical receipts. Order IDs archived from the other authority are blocked from fresh admission.

Operators can inspect the transfer or retrieve the exact local archive through `POST /v1/cutover` with action `status` or `archive` and the `epoch`.

## Reserve and reconcile a draft

Draft policies are disabled by default. An operator can configure the allowed repository, destination branch, and queue publisher identity after the control owner approves the lane. Use `bitbucket` for publisher sends and `github` for operator-observed GitHub drafts. Each queue repository slug pins one forge, destination branch, and publisher.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem draft-policy team/service
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem configure-draft-policy team/service --base-branch main --publisher draft-publisher --forge bitbucket --expected-revision 0 --enabled true --evidence 'change:approved-draft-lane'
```

Use revision `0` to create the first policy. For an existing policy, use the revision returned by `draft-policy`. To withdraw permission, use that revision and `--enabled false` with a decision reference. Withdrawal blocks new permits and holds reserved sends for reconciliation. Senders that already received permits can still report their outcomes. Reconcile all open sends before enabling the policy again. Restore recovery disables affected policies and advances their revisions.

| Endpoint | Action | Additional fields |
| --- | --- | --- |
| `/v1/publication` | `reserve` | `work_id`, `generation`, `head`, `verification_sequence`, optional `update_draft` |
| `/v1/publication` | `status` | `action_key` |
| `/v1/publication` | `reconcile` | `action_key`, `observation` |
| `/v1/publisher` | `prepare` | `action_key`, optional `update_draft` |
| `/v1/publisher` | `begin` | `action_key`, optional `update_draft` |
| `/v1/publisher` | `record` | `action_key`, `observation` |

An observation includes `outcome`, `head`, and a stable `evidence` reference. A confirmed observation also includes `native_url`, `source_branch`, and `base_branch`. A publisher gets one send permit per reserved action. If the response is lost, the action remains held for reconciliation. A timeout, pause, or expired runner lease grants no second send. For an unresolved draft, read the provider before recording an outcome.

```powershell
craft-factory reconcile-shared-draft --queue https://localhost:8443 --ca queue-ca.pem --action-key $actionKey --policy twg-bitbucket-policy.json --pull-request $pullRequestNumber
```

The reconciliation command uses the operator credential for the queue and a saved TWG credential for read-only Bitbucket access. It confirms the open draft state, repository, source and destination branches, and their pinned commits. If any identity differs or evidence is missing, leave the action held. A recorded absence ends that action.

### Run a reserved Bitbucket draft publication

Use a dedicated publisher host. Give it the queue publisher credential through `CRAFT_FACTORY_PUBLISHER_TOKEN` and a Bitbucket repository access token through `CRAFT_FACTORY_BITBUCKET_TOKEN`. Grant only the repository and pull-request access needed for the target repository. Keep these credentials outside worker accounts. Qualify the actual token and branch protections in a disposable repository before allowing live sends. [Bitbucket repository access token permissions](https://support.atlassian.com/bitbucket-cloud/docs/repository-access-token-permissions/)

```powershell
craft-factory publish-shared-draft --queue https://localhost:8443 --ca queue-ca.pem --action-key $actionKey --repository-map publisher-map.json --destination $newAbsolutePublisherDirectory
```

Before the send permit, the publisher verifies the local candidate and confirms the pinned destination and absent source branch. It creates the reserved source branch with a create-only Git lease and submits a draft PR. It reads back the draft and the branch heads before confirming the ledger. If an external call or receipt is uncertain, use reconciliation. Approval and merge remain in the forge.

### Update the existing Bitbucket draft after repair

Reserve the update with the operator credential and the repaired candidate's verification sequence. The new receipt links to the confirmed publication for the reviewed commit.

```powershell
craft-factory reserve-shared-draft --queue https://localhost:8443 --ca queue-ca.pem --work-id $workId --generation $generation --head $repairedSha --verification-sequence $verificationSequence --update-draft
```

Use the publisher credential for the reserved update.

```powershell
craft-factory publish-shared-draft --queue https://localhost:8443 --ca queue-ca.pem --action-key $updateActionKey --repository-map publisher-map.json --destination $newAbsolutePublisherDirectory --update-draft
```

The publisher checks that the existing PR is still an open draft and both branch tips match the reservation. It advances only the source branch using a Git lease that requires the reviewed commit. The draft and both branch tips must match the repaired revision at readback before the action is confirmed. A concurrent change to the base or PR can still occur during the Git push. If readback cannot confirm the expected state, the update remains held.

Use `reconcile-shared-draft` with the update action key and the original PR number to confirm a successful push after a lost acknowledgement. If the old head is still visible or the draft no longer matches, investigate before recording an outcome. Replaying the send command grants no second send permit.

### Find drafts awaiting a merge check

With migration 084, use the verifier credential to list current confirmed drafts in a repository that still need an independent merge observation. The list shows the exact candidate SHA and PR URL. GitHub entries also include the shadow receipt, report digest, and policy file digest. Bitbucket entries include the publication key. The list reads the queue. Use the appropriate confirmation command to verify the PR in the forge. [GitHub confirmation](#confirm-a-later-github-merge) · [Bitbucket confirmation](#close-a-merged-bitbucket-draft)

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem pending-merge-checks team/service --limit 20
```

If the result includes `next_after_work_id`, pass it to `--after-work-id` to continue through the current queue. Start without a cursor on a new pass to catch newly eligible work. After the verifier records an observation, standalone work moves to the operator's closure action. A pause in admission still permits merge checks. Recovery holds the list until the operator finishes it.

For one GitHub repository, the verifier can check a page and record any confirmed merges in one command. Use the same policy file as the recorded shadow evaluation and an absolute path to the trusted GitHub CLI. The batch limits native reads per draft and reserves time for the queue requests. The JSON report distinguishes PRs that have yet to merge, failed or timed-out checks, and confirmed or uncertain receipt writes.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem check-github-merges --policy github-policy.json --gh $gh --limit 10 --budget-seconds 60
```

For one Bitbucket repository, use the Bitbucket feedback policy and the same read-only credential as individual confirmation. The per-draft timeout covers PR reads, activity pages, and resolution of abbreviated commits.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem check-bitbucket-merges --policy bitbucket-feedback-policy.json --limit 10 --budget-seconds 60
```

If a report has deferred entries, continue at its `next_after_work_id`. A null cursor after zero attempts means retry the same page with more time. Start a new pass without a cursor to revisit open PRs and failed checks. If a receipt write was uncertain, the next pass rebuilds its targets from the queue.

For recurring checks, add `--cursor-db` and `--report-file` with separate paths in the verifier's private state directory. Give each queue, repository, and forge its own pair. The cursor advances even if PRs stay open and wraps to the beginning after the last page. A policy change starts a fresh pass. Two invocations that share the cursor cannot run at the same time. The launcher replaces the report before advancing the cursor, so an interrupted run can retry the page.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem check-bitbucket-merges --policy bitbucket-feedback-policy.json --cursor-db merge-cursor.db --report-file merge-last-report.json --budget-seconds 60
```

Use a budget above 15 seconds with a saved cursor to allow a page read, an exact target read, and a receipt write. Keep the verifier credential and the forge's read credential under the account running the check. The last report's modification time shows when a pass last completed, including a partial pass. A startup failure leaves the previous report available.

Scheduled commands can require `--policy-digest` and load the verifier token through `--verifier-token-file` or a mounted service credential. Bitbucket also supports `--bitbucket-token-file` or its mounted evidence credential. The kit's platform guide under `deploy` shows how to schedule the command.

### Bind a GitHub draft for shared review

For an enabled GitHub draft policy, publish the verified candidate to `craft-factory/<work-id>` and open a draft PR manually. After an independent verifier records a passing verdict, run `prepare-shared-verification` again to obtain a handoff with the current verdict sequence. Use the queue operator credential and a GitHub CLI executable with read access to the PR.

```powershell
craft-factory bind-shared-github-review --queue https://localhost:8443 --ca queue-ca.pem --handoff $handoffFile --policy github-policy.json --gh $gh --pull-request $pullRequestNumber
```

The command checks that the open draft is in the configured repository and that its branches and commits match the queue candidate. The queue confirms the binding to that candidate and verdict, making the draft available for review assignment. After a repair, update the same draft manually and bind it using the new candidate's handoff. The command only reads from GitHub and records the result in the queue.

#### Record a GitHub merge-readiness decision

Approve the GitHub policy file and reviewed protection fingerprint together. Then assess the confirmed PR against a fresh checkout of the queue's candidate pack. The assessment checks the native PR, protection, reviews, and required check runs. Use a new absolute destination for each observation.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem approve-github-shadow-policy --policy github-policy.json --approved-protection-digest $protectionDigest --evidence $approvalReference --valid-seconds 3600
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem assess-github-shadow --work-id $workId --head $head --repository-map repos.json --destination $newAbsoluteDirectory --policy github-policy.json --gh $gh --approved-protection-digest $protectionDigest
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem record-shadow-merge $receiptId --handoff (Join-Path $newAbsoluteDirectory handoff.json) --report (Join-Path $newAbsoluteDirectory github-shadow-report.json)
```

To discover the current fingerprint, run an assessment with a placeholder fingerprint such as `sha256:` followed by 64 zeroes. Inspect the GitHub branch settings represented by `observed_protection_digest` before approving it. Assess again after approval. Record either result within ten minutes while the candidate and verdict are current. Keep the generated report with the evidence archive. The ledger records its digest, the approved policy and protection pins, and any blocker codes. GitHub retains authority over merging.

#### Confirm a later GitHub merge

After GitHub reports a merge, use the independent verifier credential and the policy file used for the evaluation. The command reads GitHub to confirm the exact PR head and destination. It checks that the result descends from the pinned base and is still on the destination branch. The queue records the result commit, merge actor, and time alongside the original receipt.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem confirm-github-shadow-merge $receiptId --head $head --report-digest $reportDigest --policy github-policy.json --gh $gh
```

The recorded merge appears in the work audit. A later merge can follow changes to approvals or checks, so it leaves the original eligibility decision and qualification counts as recorded.

With migration 080, an operator can close standalone work after the independent verifier confirms its GitHub merge. Use the receipt ID and exact reviewed head from that confirmation. The queue checks that the verified draft is still current and that publication has been reconciled. It records the source head and merge commit, marks the work `merged`, and frees its review slot. Release-series members use their rollout evidence to close.

With migration 081, repeated independent verification can unblock closure of the same candidate pack and draft. The latest verdict must pass. The closure records that verification sequence alongside the original merge receipt.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem close-github-work $workId --head $head --merge-receipt-id $receiptId --evidence $closureReference
```

### Close a merged Bitbucket draft

With migration 082, a verifier can record the merge of a standalone Bitbucket draft already confirmed by the queue. Use a current passing verdict and the [Bitbucket feedback policy](../examples/bitbucket-feedback-policy.json) for the repository UUID and destination branch. The verifier uses the read-only Bitbucket credential in `CRAFT_FACTORY_BITBUCKET_EVIDENCE_TOKEN` and sets `CRAFT_FACTORY_BITBUCKET_EVIDENCE_EMAIL` for an Atlassian user API token. The launcher checks the merged PR, merge activity, and exact source and result commits. The verifier receipt includes the native merge actor and time.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem confirm-bitbucket-work-merge $workId --head $head --policy bitbucket-feedback-policy.json
```

With the operator credential, use the returned `publication_key` and a closure reference. The queue releases the review slot after checking the current passing verdict and confirmed publication again. The audit retains the same merge observation and the verification used at closure. Release-series members close through their rollout evidence.

With migration 098, once a verifier records the exact Bitbucket merge, Jira reassignment records the withdrawal and leaves the work available for closeout. Cancellation and repair requests are blocked for that already merged revision. Any post-merge correction needs a new work order.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem close-bitbucket-work $workId --head $head --publication-key $publicationKey --evidence $closureReference
```

## Sequence a cross-repository release

An agent can draft a [series proposal](../examples/series-proposal.json) for the release owner. It names the source decision and revision, compatibility and rollback plans, and ordered work with an owner, checks, edit scope, budget, and target for each repository. Use the queue repository name in both `repository` fields. Each runner resolves its checkout through its local map.

### Draft with a headless coordinator

Use the same format for a seed. Have the responsible owners agree on the repository boundaries, base commits, permissions, owner assignments, and release targets. Review the seed and retain its digest. Use a dedicated Codex account and home approved for the seed data, and a new private destination outside Git checkouts. The launch uses Codex's read-only sandbox, which still permits reads under the account's filesystem permissions.

```powershell
$seed = craft-factory review-series-proposal series-seed.json | ConvertFrom-Json
$draft = craft-factory draft-series-proposal series-seed.json --seed-digest $seed.proposal_digest --codex $codexExe --codex-home $dedicatedCodexHome --destination $newPlanningDirectory | ConvertFrom-Json
```

A successful run writes `proposal.json` and reports its digest and token usage. The coordinator can reorder the supplied work and refine the intent, acceptance criteria, compatibility, and rollback. The launcher preserves the approved repository, base, owner, scope, context, skill, target, and budget for each work ID. The run directory also retains the prompt. A blocked result reports what the coordinator needs. The proposal still goes through release-owner review.

To use an approved model and effort, add `--profile approved-profile.json --profile-digest $approvedProfileDigest`. The [profile example](../examples/execution-profile.json) uses the same format as worker execution profiles. Record the exact file digest with the profile approval. The coordinator runs a single turn and uses the shorter of its CLI timeout and the profile timeout.

`run.json` records the requested profile, effective timeout, elapsed time, outcome, and usage reported by a completed turn. A rejected draft can still have reported usage. A timeout without usage leaves the field empty, so it should be counted as unknown in a cost comparison.

### Compare drafts from two profiles

Run each approved planning profile on the same seed per case. Keep every `run.json` and its sibling `proposal.json` if produced. Ask the reviewer to assess the drafts against the approved scope, sequencing, compatibility, rollback, and test intent. Record their `accepted` or `changes_requested` decision with the exact proposal digest and the review reference. Use the same criteria on both sides.

Save the profile files and attempt folders under the directory that will hold the comparison manifest. Fill in the relative paths and SHA-256 digests in the [comparison example](../examples/series-comparison.json). List every attempt, including failures and runs over the profile limit. Omit `review` until a reviewer has made a decision.

```powershell
craft-factory compare-series-profiles series-comparison.json
```

The report shows outcomes, usage, and attempts over the profile limit overall and by task class. It includes failed attempts in the token cost of accepted cases. An unfinished run, missing usage, or pending review leaves the per-acceptance figure unavailable. Compare provider billing separately if you need monetary cost.

### Review and register the proposal

Use the generated `$draft.proposal` or a manually prepared proposal file.

For a draft based on an approved seed, show the changes with `--brief`. The brief lists moves in the sequence and previews changed intent, acceptance criteria, compatibility, and rollback. Changes to approved constraints are listed separately and cause a nonzero exit. Omit `--brief` to see the full JSON with the same seed comparison.

```powershell
craft-factory review-series-proposal $draft.proposal --against-seed series-seed.json --seed-digest $approvedSeedDigest --brief
```

For a standalone proposal, inspect the complete JSON.

```powershell
$review = craft-factory review-series-proposal series-proposal.json | ConvertFrom-Json
$review | ConvertTo-Json -Depth 20
```

Review the proposed sequence with the responsible repository owners. Save the displayed `proposal_digest` in the release owner's decision record alongside the approval reference. This command only reads the local file. To admit the approved work, an operator uses the digest from that decision record.

```powershell
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem register-proposal series-proposal.json --proposal-digest $approvedDigest --approval-reference $approvalReference
```

Registration rejects any change to the proposal bytes. The queue stores the reference and proposal digest with the series. Retain the exact proposal with the approval record so reviewers can retrieve the original compatibility and rollback plans. Git and forge checks still apply to each resulting change.

For seed-based work, also pass `--against-seed series-seed.json --seed-digest $approvedSeedDigest` to `register-proposal`. This checks the constraints again before contacting the queue and records both digests with the approval reference. If a constraint must change, review and approve the revised seed before registering the proposal.

### Register an existing approved plan

Have the release owner approve the order, compatibility plan, member owners, and rollback path. A series JSON file contains `id`, `owner`, `approval` (the approved plan reference), and an ordered `members` array. Register 2 to 12 work orders across at least two repositories. A prepared dependency update can also be the sole member. Each member has `repository`, `token_limit`, the pinned `work_order`, a designated `observer` identity, and a `target` with its exact `service`, `environment`, `region`, and `resource`.

For a dependency update that must reach a deployment, register its prepared work order through the release series in place of individually admitting the order. Set `requires_rollout_proof` to `true`. Once the exact candidate passes compatibility verification, approve a rollout policy and archive index through `approve-rollout`. The designated observer can use `observe-archived-rollout --check-only` before recording the rollout. Compatibility verification leaves the series open until the approved archive proves the reviewed change reached the pinned target. A manual observation and a rollout approval without an archive index cannot close dependency work in a series. For a pending series upgraded from a policy-only approval, approve and replay fresh compatibility evidence before approving the archive.

```powershell
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem register series-plan.json
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem status series-id
```

The queue registers all members in one transaction. A worker can claim a member after every predecessor has a current release observation. A claim pins the preceding commits and artifact digests. Status reports each owner, the first blocking predecessor, and whether the series is open, partially released, or released.

The designated observer can find and inspect assigned work with their observer credential.

```powershell
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem observer-inbox
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem observer-status work-id
```

The inbox includes outstanding assignments, including releases whose evidence has expired. Use `--repository` to filter, `--include-closed` to include cancelled work and current releases, and `--after` with the returned `next_after` to continue the listing. A page contains up to 50 assignments, sorted by work ID. Keep the same filters when following a cursor.

The report includes the assigned target, current candidate and verification pins, and any approved rollout policy. It distinguishes a missing current prerequisite release from a prerequisite that differs from the pinned commit or artifact. It also identifies publication holds and the need to verify or refresh rollout evidence. Predecessor identities and evidence are available in the operator's series view. Treat `next_action` as guidance from the current ledger snapshot. The observer commands recheck the applicable gates before recording evidence.

### Transfer a release observation assignment

Use the operator's `status` output to get the member's `observer_revision` and record the approved handover reference. The replacement needs an enrolled release-observer identity.

```powershell
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem transfer-observer handover-17 --work-id $workId --expected-revision $observerRevision --observer-id staging-backup --evidence decision:approved-handover/17
```

Reuse the same transfer ID and arguments to retrieve the original receipt after a lost response. Each successful handover advances the assignment revision and moves the item to the replacement observer. Existing release and approval pins remain valid under their current checks. The queue records who originally observed the release and who verifies a refresh or withdraws it after handover.

After the native release completes, the designated observer verifies the deployed artifact and exact candidate commit on the assigned target. Use the independently recorded verification sequence for that candidate. Create an observation JSON with `work_id`, `generation`, `head`, `verification_sequence`, `target`, `artifact_digest`, and a stable `evidence` reference. The receipt assumes the observed deployment preserves the exact candidate SHA. The approved provider or release system supplies the evidence. The observer credential proposes no external write.

```powershell
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem observe observed-release.json
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem withdraw work-id --sequence $releaseSequence --evidence $rollbackEvidence
```

Set `CRAFT_FACTORY_RELEASE_OBSERVER_TOKEN` for the observation commands. A rollback withdraws the observation. The queue blocks later claims, active renewals, submissions, and draft send permits that depend on it. Fresh evidence for the same pinned commit and artifact can resume work. If the predecessor changes, cancel the affected outstanding work after reconciling any publication hold, then approve a revised plan with new work IDs. Restore recovery withdraws release observations from the recovered database and requires fresh evidence.

### Require a verified rollout for a series member

Set `requires_rollout_proof: true` on the member when registering its plan. Once an independent verifier has checked the candidate, have the operator approve the exact remediation policy for that release. The approval JSON contains `work_id`, `generation`, `head`, `verification_sequence`, the `policy_digest` as lowercase SHA-256 prefixed by `sha256:`, and an `approval` reference. The policy digest covers the exact file bytes.

```powershell
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem approve-rollout rollout-approval.json
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem observe-rollout --work-id $workId --generation $generation --head $candidateSha --verification-sequence $verificationSequence --policy rollout-policy.json --release release-evidence.json
```

Use the operator credential for approval and the designated release observer credential to submit the policy and evidence. The server runs the rollout checks using its database clock. It requires a fresh matching result for every policy target, including the member's assigned target, and binds the result to the repository, work item, artifact, and approved candidate. It records both file digests and checks freshness again when committing the observation. The server limits the policy to 64 KiB and the release evidence to 512 KiB. [Remediation policy and evidence](../README.md#verify-a-maintenance-rollout)

A member that requires rollout proof uses `observe-rollout` on a queue configured for normalized evidence. If the candidate or independent verification changes, review and approve the policy for the new revision. Use trusted release and inventory exports. The server evaluates their contents, while the approved source and observer establish where they came from.

#### Verify original archives on the queue

Start the queue with `--rollout-archives rollout-catalog.json` to require a server-managed archive for every verified rollout on that queue. The catalog is an array of entries with `work_id`, `policy`, `policy_digest`, `sources`, and `sources_digest`. Use paths relative to the catalog directory. The `sources` file is the pinned rollout index described by `craft-factory schema remediation-source-index`. Give the queue read access to the catalog and original evidence. Restrict writes to the operator responsible for the approved inputs.

Run `prepare-rollout-archive` in the evidence directory to produce the index and its catalog entry. Review the returned entry before adding it to the catalog. Use its `sources_digest` in the rollout approval. [Prepare a rollout archive](../README.md#verify-a-maintenance-rollout)

Publish new evidence under new filenames, then atomically replace the catalog file at the configured path. Use the operator credential to load the reviewed version by its exact file digest while the queue is running.

```powershell
$catalogDigest = 'sha256:' + (Get-FileHash -LiteralPath rollout-catalog.json -Algorithm SHA256).Hash.ToLowerInvariant()
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem reload-rollout-catalog --digest $catalogDigest
```

The queue checks the catalog digest, paths, and referenced policy and index digests before activating it. A rejected replacement leaves the last loaded catalog active. Reload changes which files the queue can use for approvals and verification. Ledger approvals and release receipts still govern which versions can be recorded.

Include `archive_index_digest` in `rollout-approval.json` along with the existing approval fields. Set it to the catalog entry's `sources_digest`. Apply the packaged migrations and refresh the service login's function grants before restarting the queue. With the release observer token, submit the current coordinates to let the server look up and replay the archive. For a later observation of the same release, use the refresh procedure below.

```powershell
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem observe-archived-rollout --work-id $workId --generation $generation --head $candidateSha --verification-sequence $verificationSequence
```

Add `--check-only` to inspect the approved archive before recording a rollout or refresh. The queue reads the same evidence and returns the verified artifact and freshness deadline, or a blocker code and any pending targets. A blocked check prints its JSON report and exits unsuccessfully. The check leaves release receipts and downstream eligibility untouched. The recording command rechecks the current ledger before committing.

For a first rollout with an active approval, `dependency_scan_expired` tells the designated observer that the current dependency verdict has expired scan evidence. Have the operator and independent verifier refresh the scans and approve rollout for the new verification sequence. [Refresh scans for an existing candidate](../README.md#refresh-scans-for-an-existing-candidate)

The candidate SHA must match the reviewed source head in every required target. The rollout policy's `source_commit` pins the actual merge result deployed to those targets. For GitHub, bind the draft, record the shadow evaluation for the same verification sequence, and have an independent verifier confirm the later merge. The archived PR and deployed merge commit must match the verifier's observation. The queue stores that binding on the release receipt and checks it again on refresh. [GitHub merge confirmation](#confirm-a-later-github-merge)

The queue uses its database clock to check freshness. PostgreSQL checks the approved policy and archive index digests again when it commits the receipt. An approval that pins an archive requires that archive even if the service configuration changes.

Migration `035_rollout_release_freshness.sql` retains the earliest freshness deadline across the required targets. Once the deadline passes, `shared-series status` reports `release_current: false` and identifies that predecessor on waiting members. The queue blocks downstream claims, lease renewal, and publication while the evidence is stale. The historical receipt remains visible with `rollout_fresh_through` in Unix seconds. Replaying the receipt preserves its original deadline.

#### Refresh evidence for an unchanged rollout

Apply migration `036_rollout_archive_refresh.sql` and refresh the service login's function grants. Collect fresh runtime evidence for every required target and update the work's catalog entry to point to the new index. Keep the existing rollout policy digest and load the revised catalog with `reload-rollout-catalog`. Prepare `rollout-refresh-approval.json` with `work_id`, a new `refresh_id`, the original receipt's `observation_sequence`, `policy_digest`, the new `archive_index_digest`, and an `approval` reference.

```powershell
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem approve-rollout-refresh rollout-refresh-approval.json
craft-factory shared-series --queue https://localhost:8443 --ca queue-ca.pem observe-archived-rollout --work-id $workId --generation $generation --head $candidateSha --verification-sequence $verificationSequence --refresh-id $refreshId
```

The refresh uses the existing independent verification. The queue requires the same artifact, reviewed head, and rollout policy, and verifies that the new evidence extends the deadline. It keeps the original receipt and records the latest successful refresh under `latest_rollout_refresh` in series status. Use the withdrawal workflow when a release is rolled back or changes artifact. Archive receipts recorded before migration `035` use their recording time as their original deadline and can be refreshed.

## Share a scoped diagnostic report

Prepare the approved diagnostic policy and redacted export on the service host. Start the service with `--diagnostic-cases cases.json`, where the catalog is a JSON array with up to 16 entries. For normalized evidence, each entry has an `id` and absolute `policy`, `release`, and `telemetry` paths. Keep the files under operator control. [Diagnostic evidence format](../README.md#diagnose-an-observed-release)

For an original source archive, use an absolute `release_manifest` path in place of `release` and include the approved `policy_digest` in the entry. Set `release_basis` to `source_manifest` in the policy and pin the manifest bytes with `release_digest`. The service verifies the input files, builder signature, target, and time before opening the listener. Archive requests must end at the manifest's `as_of` time. [Archive diagnostic format](../README.md#diagnose-from-original-release-evidence)

An original source case can also load OTLP exports directly. Use an `otlp` object with absolute `policy`, `logs`, and `metrics` paths in place of `telemetry`. In the diagnostic policy, set `telemetry_basis` to `otlp_archive` and set `telemetry_digest` to the approved OTLP import policy's hash. That policy pins both raw exports, the service instance, the Kubernetes container, the allowed scopes, and the release. Before serving correlated records, the service compares the policy's release and container identity with the original Kubernetes evidence. For an existing raw OTLP case, collect the container identity, regenerate the pins, and approve the updated case before restarting the service. [OTLP import policy](../README.md#prepare-diagnostic-telemetry-from-otlp-exports)

Apply migration 033 and refresh the queue service's function grants. Create a separate `diagnostician` identity. With the operator credential, bind it to the case ID, the SHA-256 digest of the exact policy file, the allowed UTC Unix window, an expiry, and a read allowance. The grant window can be tighter than the policy window.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem grant-diagnostic-access incident-read-17 --diagnostician-id staging-investigator --case-id staging-incident --policy-digest $approvedPolicyDigest --from 1795238330 --to 1795238400 --valid-seconds 1800 --max-reads 3 --evidence decision:incident-read-access/17
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem audit-diagnostic-access incident-read-17
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem revoke-diagnostic-access incident-read-17 --evidence decision:incident-closed/17
```

Supply the credential through `CRAFT_FACTORY_DIAGNOSTIC_TOKEN`.

```powershell
craft-factory diagnose-shared-release --queue https://localhost:8443 --ca queue-ca.pem --case staging-incident --from 1795238330 --to 1795238400
```

`POST /v1/diagnostics` accepts only `case_id`, `from`, and `to`. It returns the bounded report to the authenticated TLS caller with `Cache-Control: no-store`. PostgreSQL charges the specific grant and records each redeemed window. A lost response may still consume a read. Use a new grant ID when issuing replacement access. Revocation prevents further redemptions under the old ID, and the audit retains its read receipts. Restore recovery revokes restored grants and preserves their receipts.

The service hosts approved exports. Live telemetry query identities and provider permission checks require separate qualification.

### Reload the diagnostic catalog

Put each replacement policy and its evidence at a new path. Include `policy_digest` for every case in the replacement catalog, including normalized cases. After review, atomically replace the configured catalog file on the queue host with the approved bytes. Run this command against the running service using the replacement file's SHA-256 digest and the decision reference.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem reload-diagnostic-catalog --digest $catalogDigest --evidence decision:updated-incident-evidence/17
```

The service validates the entire replacement before retiring any grants. Changing a policy digest or removing a case retires that exact case and policy pair in PostgreSQL, revokes its grants across processes that share the database, and retains its read receipts. A later case can use the same ID with a newly approved policy digest and fresh grants. Unchanged cases keep their current grants.

Each queue process loads its local catalog. Reload each serving process to make the new cases available through it. A restarted process reads its configured files. If you replace the catalog while the service is stopped, you can retire the old case and policy through any operator API that shares the database.

### Inspect and retire access to one case

When closing a case or recovering from a catalog change during downtime, list the case's grants to identify the exact policy digest. The list includes grant metadata and whether each grant still has usable reads. Use `--policy-digest` to filter the list and inspect the recorded retirement, and use `--after` with the returned `next_after` value to continue listing.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem diagnostic-case-access staging-incident
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem retire-diagnostic-case staging-incident --policy-digest $oldPolicyDigest --evidence decision:incident-closed/17
```

Retirement works even if that case or policy is absent from the process's catalog. It revokes all grants for the exact case and digest, blocks new grants for that pair, and preserves individual read receipts. It is permanent for that pair. Use a newly approved policy digest to reopen access.

## Operate and recover the queue

Probe `GET /healthz` over HTTPS for service and database readiness. Use the operator credential to read queue age, lease age, saturation, token reservations, and held publication counts. The summary contains aggregate telemetry. Use the audit and holds commands for actionable work IDs and retained evidence.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem summary
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem held-publications
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem audit $workId
```

### Scrape aggregate queue metrics

Enroll a `metrics_reader` identity and mount its original bearer token as a read-only file in the scraper. Use the queue CA and a host name that matches its certificate. The token can read `GET /metrics`. Work IDs, repository names and evidence stay in the scoped operator views. [Prometheus scrape configuration](https://prometheus.io/docs/prometheus/latest/configuration/configuration/#http_config)

```yaml
scrape_configs:
  - job_name: craft-factory-queue
    scheme: https
    scrape_interval: 30s
    scrape_timeout: 5s
    authorization:
      type: Bearer
      credentials_file: /run/secrets/craft-queue-metrics-token
    tls_config:
      ca_file: /etc/prometheus/craft-queue-ca.pem
    static_configs:
      - targets: [queue.example.com:8443]
```

The `craft_queue_` gauges cover admission pause, recovery, review capacity, queue depth, oldest queue and lease ages, overdue leases, tokens and held publications. Migration 061 adds `craft_queue_standing_delegation_blocked`, the count of queued, leased, and candidate work whose standing delegation no longer authorizes progress. Age samples are absent when the corresponding set is empty. Each queue process handles one scrape at a time and gives the summary query up to two seconds. An unavailable scrape returns HTTP 503 so the collector can distinguish it from an idle queue. [Prometheus text format](https://prometheus.io/docs/instrumenting/exposition_formats/#prometheus-text-format)

Migration 117 adds `craft_queue_token_overrun_attempts_15m` and `craft_queue_tokens_above_reservation_15m`. These gauges use the time the queue recorded each overrun, so a long-running attempt appears as soon as its usage arrives. `CraftQueueTokenOverrun` warns after two minutes of healthy scrapes with an overrun in that window, including while intake is paused. Use `shared-ops operator-brief` to identify the affected work and check the audit for the attempt.

Migration 074 adds `craft_queue_pending_review_assignments` and `craft_queue_oldest_pending_review_seconds`. They count undecided assignments with active grants on a current verified draft and measure the wait from the assignment time. Multiple reviewers on one draft count as separate assignments. The operator's `shared-ops summary` includes both values.

Migration 123 separates candidates waiting for a first verdict from those already picked up. The `craft_queue_waiting_verification_candidates` and `craft_queue_active_verification_pickups` gauges use the same eligibility rules as automatic pickup. `craft_queue_oldest_waiting_verification_seconds` measures age from pack submission among unclaimed candidates. Candidates submitted before migration 123 count toward the backlog and have no recorded submission time. The `CraftQueueVerificationWaiting` warning starts after a candidate has waited at least 15 minutes and the condition persists for five minutes. Pause or recovery suppresses the warning. Use `shared-ops operator-brief` to check which candidates and verifiers need attention.

The shared-service kit includes warning rules in [deploy/prometheus/craft-factory.rules.yml](../deploy/prometheus/craft-factory.rules.yml). Add the rules file to the scraper's `rule_files` and use the `craft-factory-queue` job name above. The starting thresholds are two minutes for scrape failures, recovery, or blocked standing delegation and five minutes for overdue leases or publication holds. The queue age warning requires at least 15 minutes of queued age and the review warning requires at least four hours since assignment, both held for five minutes. Pause or recovery suppresses the lease, queue age, review wait, and standing delegation warnings. Review the thresholds and notification routing for your queue before enabling paging.

Run `promtool test rules craft-factory.rules.test.yml` from `deploy/prometheus` to check the hold times and the pause, recovery, and unavailable-scrape cases. For a backlog or overdue lease warning, start with `shared-ops summary` and check runner status and review capacity. For a publication hold, use `shared-ops held-publications` before retrying an external write. For a standing delegation warning, check `shared-ops review-inbox --brief` and the relay's last report. If the work has an unresolved publication hold, the inbox shows reconciliation first. [Prometheus rule testing](https://prometheus.io/docs/prometheus/latest/configuration/unit_testing_rules/)

With migration 075, use `review-wait` with the operator credential to investigate a review wait warning. Results start with the oldest assignment and include its reviewer, exact head, and draft URL. The total covers the whole selected repository or queue, even when `--limit` returns fewer rows. Omit `--repository` for the same queue-wide scope as the warning.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem review-wait --repository team/service --limit 5
```

### Diagnose a waiting work order

With migration 072, an operator can record an expedited priority for a waiting order. Use the approved ticket or decision reference as evidence. Read the current revision from `dispatch-status`. Orders start at revision 0 with normal priority.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem set-dispatch-priority $workId --expected-revision 0 --priority expedited --evidence $decisionReference
```

The queue uses priority among eligible orders in the same group. Work that already holds review capacity goes first. Within each group, expedited orders go ahead of normal orders, then admission order breaks ties. Scope, budgets, dependencies, pause, and active leases still apply. Set `--priority normal` with the latest revision to restore the default. The dispatch report shows the effective priority and the last ten decisions.

With migration 067, use the operator credential to see why a specific order is waiting.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem dispatch-status $workId
```

The snapshot shows whether a lease is live, how many enrolled runners are active, and whether pause, review capacity, token budget, standing delegation, release dependencies, or an attempt limit is in the way. It also lists the required adapter and whether the runner needs to support project context or execution profiles.

Migration 068 retains the latest claim poll per runner and adapter, including idle polls. The report lists up to 20 currently enrolled runners for the order's adapter. It shows when each runner last polled, whether it reported this repository as available, and which capabilities it advertised. A missing poll or checkout value means the queue has no report for it. An old poll is historical evidence, and a busy worker may go longer between claim polls. Host checkout paths stay local. Recovery clears the saved polls so restored evidence cannot suggest that a runner has contacted the recovered queue.

### Triage the shared queue

Apply migration 096. Use the operator credential to get one read-only snapshot of the shared queue.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem operator-brief --repository team/service --limit 5
```

The first line identifies the snapshot time and filter. Queue pause, recovery, review slots, and token budget are shared across repositories. The other sections follow the repository filter and show pending review decisions and evidence, outstanding assignments, oldest queued work, expired leases, and GitHub merge queue entries. Use the work ID with `dispatch-status` or `audit` for details. Resolve review and approval in the usual forge workflow.

With migration 130, **Work waiting on runners** also shows the current dispatch blockers beside each waiting item. The terminal displays up to three blockers per row. Use `--json` for the complete list on each included row and `dispatch-status` for runner observations on one work order.

**Standing issue approvals** shows GitHub issues with migration 127 and Jira issues with migration 131, including issues that have no work ID. Open the linked GitHub issue or find the Jira key on the site identified by its Cloud ID, then review the work-order template with the owner. The brief shows whether to confirm the reviewed input in the relay, refresh an expired observation, or approve the current revision. Fresh, confirmed observations include the rule, issue key, source revision, and review token used by `approve-standing-intent`. Ready decisions appear first, counts include items beyond the display limit, and the section follows the delivery repository filter. Run the Jira relay again after upgrading to populate pending Jira decisions. [GitHub approval steps](#relay-a-watched-github-issue) · [Jira approval steps](#relay-watched-jira-tickets)

With migration 132, recording a new approval requires a matching observation from the past five minutes. If the brief asks for a refresh, run the relay again and use the current review token. Repeating an already recorded approval with the same actor and evidence returns the original receipt.

With migration 111, **GitHub readiness** shows current drafts under standing enqueue policies that lack a recent successful check. The brief links to the PR and displays the last selection time or receipt, with up to three blocker codes. It puts checks that produced no result within 90 seconds first. Use the verifier's scheduler report and the recorded receipt to investigate. Once a successful check is recorded, the draft leaves this section. Watch for an enqueue or an unresolved send in **GitHub merge queue**.

With migration 122, a candidate with a current automatic pickup appears as **Verification in progress** under **Review and operator decisions**. The brief shows the verifier identity and current deadline. An expired or released pickup no longer appears as in progress. The repository and item limit still apply.

The default usage window is 24 hours. Change it with `--usage-hours`. The usage section reports metered token counts and separately counts tokens reserved for attempts without usage. It lists the largest overruns in the selected window and repository scope. Add `--json` for structured totals, bounded rows, and per-profile usage. Omit `--repository` for the full operator view.

### Review the decisions waiting on people

Apply migration 025 and refresh the queue service's function grants. Use the operator credential to list current candidates, held publication actions, and exhausted attempt caps. Filter by exact repository or work-order owner when triaging one area.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem review-inbox --repository team/service --limit 5 --brief
```

The inbox puts unresolved publication sends, failed checks, and exhausted attempts near the top. Each entry identifies the next action and shows the issue, approved intent excerpt, up to three acceptance criteria, and any evidence for the exact candidate generation and commit. Repairs link the prior draft separately until Craft has publication evidence for the new generation.

With migration 083, **Close merged work** surfaces standalone work with a verified forge merge, a reconciled publication, and a current passing verdict. The operator brief includes the merge commit and the key needed by the closure command. The item leaves the inbox when closure frees its review slot. [GitHub closure](#confirm-a-later-github-merge) · [Bitbucket closure](#close-a-merged-bitbucket-draft)

With migration 038, **Check standing delegation** surfaces queued, leased, or candidate work whose delegation no longer authorizes progress. Check the relay result for that issue. If the observation expired, a fresh eligible observation can clear the blocker. If the human changed or withdrew delegation, resolve the work through the existing review or cancellation path. An unresolved publication send takes precedence so the operator can reconcile the external action first.

**Resolve requested changes** shows a current recorded change request awaiting operator action. The brief includes its handoff ID, feedback excerpt, and any pause or attempt-cap blocker. With migration 065, it also counts active assignments for the same verified revision that still need a decision. Revoked or expired assignments stop contributing to the count. Inspect the work audit for the recorded notes and use `apply-review-repair` to apply them together when ready. The entry leaves the inbox when a repair is queued or the reviewed head is no longer current.

Omit `--brief` for JSON, including stage counts across the full filtered inbox and the bounded list of items. Use the audit for older revisions. Inspect current check and approval status in the forge before a merge.

### Give a reviewer access to one repository

Apply migration 026 and refresh the queue service's function grants. Configure a separate service identity with the `reviewer` role. An operator grants that identity access to an exact repository for a fixed period. Use a new grant ID for each approval.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem grant-review-inbox $grantId --reviewer-id $reviewerId --repository team/service --evidence $decisionReference --valid-seconds 86400
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem revoke-review-inbox $grantId --evidence $revocationReference
```

The reviewer uses `CRAFT_FACTORY_REVIEWER_TOKEN` and adds `--reviewer` to the inbox command.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem review-inbox --reviewer --brief
```

Repository grants limit entries, counts, and the visibility of cross-repository dependency IDs. An expired or revoked grant stops contributing scope. Replaying the original grant keeps its recorded expiry and revocation, and restoring the queue revokes grants from the backup. Verifiers, publishers, and operators use their own credentials for their queue actions.

### Assign a draft for human review

Apply migration 027 and refresh the queue service's function grants. An operator can assign a confirmed draft to a reviewer with an active repository grant. Use the current generation, full candidate SHA, and latest passing independent verification sequence from the inbox or audit.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem assign-review $handoffId --work-id $workId --generation $generation --head $head --verification-sequence $sequence --reviewer-id $reviewerId --evidence $requestReference
```

With migration 077, `route-review` lets an operator supply an approved roster and a per-reviewer limit for this assignment. The queue chooses the eligible reviewer with the fewest current pending assignments across the queue. Roster order breaks ties. The chosen reviewer must have an active grant and must be independent of the runner. The command blocks when the draft changed or all eligible reviewers are at capacity. The audit retains the roster, limit, and the selected reviewer's pending count before assignment.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem route-review $handoffId --work-id $workId --generation $generation --head $head --verification-sequence $sequence --reviewer-id $primaryReviewerId --reviewer-id $backupReviewerId --max-pending 3 --evidence $requestReference
```

With migration `051_assigned_review_context.sql`, the reviewer can open a compact inbox of their assigned drafts. The brief shows the exact head and draft, the issue, intent and acceptance excerpts, and the independent verification reference. With migration 071 it also shows the verifier's test summary when provided. Once a decision is recorded or a newer revision supersedes the assignment, it leaves this view. Use `--repository`, `--owner`, or `--limit` to narrow the assignment list.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem review-inbox --reviewer --assigned-only --brief
```

With migration `052_review_assignment_pages.sql`, continue from the brief's `--after` hint, or use the JSON `next_after` value. Keep the same repository and owner filters. Pages follow assignment order and still work after you finish reviewing the assignment that served as the cursor. If that cursor is no longer in your granted scope, start again without `--after`.

Check the revision and current native checks in the forge, then record either `acknowledged` or `changes-requested` with a link or other stable evidence reference. Use the reviewer credential for this command.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem record-review-decision $handoffId --head $head --decision changes-requested --evidence $feedbackReference --note $feedback
```

For a GitHub draft, migration 062 lets the assigned reviewer use their existing GitHub change request. Sign in to the trusted `gh` executable as the reviewer, use the approved repository policy with your numeric reviewer ID, and pass the numeric review ID from the review permalink. Craft checks that your review still requests changes on the assigned head and uses the review's main message as the repair note. Write the actionable feedback in that message in up to 1,000 characters after whitespace normalization. [GitHub review API](https://docs.github.com/en/rest/pulls/reviews)

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem import-github-review $handoffId --review $reviewId --policy github-policy.json --gh $absoluteGhPath
```

For a Bitbucket draft, migration 063 adds the equivalent import. Use your own read-only Bitbucket credential in `CRAFT_FACTORY_BITBUCKET_EVIDENCE_TOKEN` and set `CRAFT_FACTORY_BITBUCKET_EVIDENCE_EMAIL` for an Atlassian API token. The token needs the Bitbucket user, repository, and pull-request read scopes. Request changes and leave one unresolved top-level PR comment containing the following two lines. Replace the commit, issue, and note with the assignment's full SHA, issue key, and actionable feedback. [Bitbucket identity API](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-users/#api-user-get) · [Bitbucket API scopes](https://developer.atlassian.com/cloud/bitbucket/bitbucket-cloud-rest-api-scopes/)

```text
Craft request changes v1
{"source_commit":"FULL_CANDIDATE_SHA","work_item":"PROJECT-123","note":"Retain the original value when the optional setting is missing."}
```

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem import-bitbucket-review $handoffId --comment $commentId --policy bitbucket-feedback-policy.json
```

An operator can apply recorded changes to the same draft using the existing repair path. The work audit retains the reviewer's decision, and the apply command returns the operator's repair receipt. The next candidate requires fresh checks. Approval and merge stay in the forge.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem apply-review-repair $handoffId
```

Append `--queue-repair` when assigning the review to let that reviewer queue a repair by recording `changes-requested` on the assigned revision. The queue waits for every active assignment on that revision. With migration 066, the last reviewer can acknowledge the draft or request more changes. Either decision lets Craft queue the repair if a current change request already has automatic repair authorization. The repair includes all recorded change requests and their evidence. An acknowledgment adds no repair instructions.

With migration 076, an operator can move one undecided assignment to another reviewer with an active grant for the repository. Give the replacement a new handoff ID and use the exact head from `review-wait`. Both handoffs stay in the audit, and the previous reviewer keeps their repository grant and other assignments. Append `--queue-repair` to explicitly authorize the replacement reviewer to queue a repair.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem reassign-review $handoffId $replacementHandoffId --head $head --reviewer-id $newReviewerId --evidence $reassignmentReference
```

A paused queue, an exhausted attempt cap, an outstanding assignment, or another blocker can leave `needs_operator` on the handoff. An exact decision retry returns the same outcome. Use `apply-review-repair` after resolving a blocker that persists after the final decision. If the combined notes exceed the 4,000-character repair limit, the requests remain in the inbox. The operator can review them together and submit an explicit consolidated note with `request-shared-repair`.

### Keep a record of shadow merge decisions

Apply migration 028 and refresh the queue service's function grants. An operator registers the approved policy digest for the repository before evaluating a candidate. Use `--require-native` when qualifying the live preflight lane. A pin's expiry also limits when the operator can record new evaluations under it.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem approve-shadow-policy team/service --policy-digest $policyDigest --require-native --evidence $approvalReference --valid-seconds 86400
```

After independent verification and confirmed draft publication, prepare a fresh verification handoff for the current revision. Run [`shadow-merge`](../README.md#evaluate-a-merge-in-shadow-mode) against the same commit in a trusted checkout and save its JSON report. The ledger takes the current handoff and a report evaluated within the last ten minutes. It records eligible and blocked decisions, validates the candidate and passing verification for eligibility, and ties each decision to the report and observation digests. Keep the original report and evidence with your qualification artifacts.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem record-shadow-merge $receiptId --handoff $handoffFile --report $reportFile
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem shadow-merge-summary team/service --since $unixTimestamp
```

The summary counts recorded evaluations by outcome and blocker. The work audit retains recent receipts across candidate generations. An exact retry returns the original receipt. Fresh submissions for superseded revisions are rejected. The queue treats the report as an operator attestation and checks its bindings against the ledger. The shadow result remains evidence for qualification.

### Compare the shadow decisions with independent verdicts

Apply migration 029 and refresh the queue service's function grants. Give the verifier the saved candidate and policy evidence for the evaluation. The verifier applies the approved policy to those artifacts and records the expected result for the same head and report digest. Keep the evidence file with the qualification records. The command stores its SHA-256 digest, a stable reference, and a short reason.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem record-shadow-outcome $receiptId --head $head --report-digest $reportDigest --basis fixture --expected blocked --evidence-file $verdictFile --evidence $verdictReference --note $reason
```

Use `CRAFT_FACTORY_VERIFIER_TOKEN` for this command and `--basis forge-observed` for an assessment backed by actual forge observations of the recorded revision. Each receipt accepts one independent verdict. An exact retry returns the stored verdict.

`shadow-merge-summary` groups the comparisons by approved policy digest and basis. It counts correct allows and blocks, false allows, false blocks, and evaluations still awaiting an independent verdict. Review false allows before making any live qualification decision. The work audit includes recent verdicts for the underlying evaluations.

For a GitHub policy with `require_merge_queue` enabled, an eligible receipt also records the PR node ID, the merge-queue ruleset ID, and the applicable ruleset IDs. This ties the readiness observation to the same generation, approved protection fingerprint, independent verdict, and confirmed draft. The receipt represents eligibility at the time of observation. Merge observation and closure still require the separate verifier step. [GitHub shadow assessment](../README.md#evaluate-a-merge-in-shadow-mode)

### Authorize and enqueue a reviewed GitHub revision

Migration 090 adds a one-time enqueue approval. Review the current readiness receipt, then name a registered publisher and approve the exact head and report digest. Use a new intent ID for each approval. An approval expires after five minutes by default. Approval never sends a forge request.

```powershell
craft-factory shared-github-enqueue --queue $queue --ca $ca approve $intentId --receipt-id $receiptId --head $head --report-digest $reportDigest --publisher $publisherId --evidence $decisionReference
```

The publisher needs its queue credential and a protected file containing a dedicated GitHub writer token. The publisher also needs the verification checkout from `assess-github-shadow` and the same approved policy file. The GitHub CLI uses its authorized read configuration for preflight. Craft supplies the writer token only to the enqueue operation.

```powershell
craft-factory shared-github-enqueue --queue $queue --ca $ca send $intentId --checkout $preparedCheckout --policy $policy --gh $gh --github-token-file $githubWriterToken
```

Craft repeats native readiness checks, including checks, approvals, ruleset bypass lists, the exact head and base, and the absence of an existing queue entry, auto-merge request, or PR stack. The ledger checks the current verdict, pause state, and expiration again before authorizing a single sender. GitHub receives a queue-only request with the expected head and the ordinary queue position. Follow the separate [merge confirmation](#confirm-a-later-github-merge) after GitHub finishes its merge-group checks. [GitHub enqueue schema](https://docs.github.com/en/graphql/reference/pulls#enqueuepullrequestinput)

If Craft loses the response, use `holds` or `status $intentId` to inspect the transfer. Use `reconcile $intentId --gh $gh --evidence $decisionReference` to record an exact queue entry seen on GitHub. Craft blocks a second send for that head. If the entry is gone, stop the sending process and reconcile the native PR and queue history before using `abandon $intentId --evidence $decisionReference`. An abandoned transfer that might have reached GitHub stays fenced for that head.

Restore recovery holds both approvals and unfinished sends because the source publisher could have acted after the backup. List the holds with `shared-github-enqueue --queue $queue --ca $ca holds` and reconcile or abandon each before finishing recovery. Use `--after-intent` with the last ID for more results.

With migration 091, an acknowledged queue entry blocks local cancellation and repair. Craft keeps any reviewer feedback and leaves the repair awaiting operator action. Use `holds --include-queued` to find acknowledged entries still awaiting clearance or merge confirmation. An independent merge observation for that publication and exact head marks the enqueue merged and preserves the original queue-entry receipt.

If a human removes the PR from the GitHub queue, or GitHub ejects it, verify the removal before canceling or applying a repair. Craft checks the bound PR twice and requires an unmerged PR with no current queue entry or auto-merge request. The clearance records the observed head even if it advanced after enqueue. [Remove a PR from the GitHub merge queue](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/merging-a-pull-request-with-a-merge-queue#removing-a-pull-request-from-a-merge-queue)

```powershell
craft-factory shared-github-enqueue --queue $queue --ca $ca clear $intentId --gh $gh --evidence $decisionReference
craft-factory shared-ops --queue $queue --ca $ca apply-review-repair $handoffId
```

### Authorize low-risk enqueue by policy

Migration 092 lets an operator approve a standing policy for one repository and base branch. The policy names a publisher, pins the combined GitHub shadow policy digest, and limits exact file paths, commits, changed lines, and authorizations in a rolling 24-hour window. Use the digest returned by `approve-github-shadow-policy` as `shadow_policy_digest`.

```json
{
  "id": "routine-source-edits",
  "repository": "team/service",
  "base_branch": "main",
  "publisher": "service-publisher",
  "shadow_policy_digest": "sha256:<approved-shadow-policy-digest>",
  "allowed_paths": ["src/status_text.rs"],
  "max_files": 1,
  "max_commits": 2,
  "max_changed_lines": 20,
  "daily_limit": 1,
  "valid_seconds": 86400,
  "evidence": "CHANGE-123"
}
```

The report measures the whole candidate history, including edits later reverted. It requires existing text files with unchanged modes. Binary files, added or deleted files, Git attributes and submodule configuration, and changes above the approved limits remain on the per-revision approval path. The publisher independently measures the same checkout again before sending. Required reviews, CI, and the current readiness receipt still apply.

```powershell
craft-factory shared-github-enqueue --queue $queue --ca $ca approve-policy $policyFile --digest $policyFileDigest
craft-factory shared-github-enqueue --queue $queue --ca $ca authorize $intentId --policy-id $policyId --receipt-id $receiptId --head $head --report-digest $reportDigest
craft-factory shared-github-enqueue --queue $queue --ca $ca send $intentId --checkout $preparedCheckout --policy $githubPolicy --gh $gh --github-token-file $githubWriterToken
```

The operator runs `approve-policy`. The named publisher runs `authorize` and `send`. Authorizing does not contact GitHub. The audit identifies the standing policy for each intent. `revoke-policy $policyId --evidence $decisionReference` fences any send that hasn't started. Restore recovery revokes standing policies. Already sent requests follow the same clearance and merge confirmation workflow.

### Schedule standing GitHub enqueue

Apply migration 093. Run the publisher under a dedicated service account with its ordinary read-only `gh` authentication, its queue publisher token, and the separate GitHub writer token. Use a trusted repository map and a private existing checkout root outside the mapped source repository.

```powershell
craft-factory shared-github-enqueue --queue $queue --ca $ca --queue-token-file $publisherToken poll $policyId --repository-map $repositoryMap --checkout-root $checkoutRoot --policy $githubPolicy --gh $gh --github-token-file $githubWriterToken
```

Each invocation selects at most one candidate from fresh, recorded readiness receipts under the named policy. The queue reuses an outstanding scheduled authorization for the same receipt while it remains current. The publisher verifies the candidate pack in a temporary checkout and uses the exact-head native preflight and send. The JSON result is an idle report or the acknowledged queue entry. An uncertain send requires reconciliation before another send of that revision. An expired scheduled authorization that never permitted a send releases its slot on the policy's next poll. It needs a new readiness receipt to be scheduled again.

Use `--report-file last-report.json` in the private scheduler directory to replace the previous completed result. Compare the report timestamp with the scheduler status after a failure. Use the audit and holds commands for the authoritative queue state. See the scheduler guide in your kit's `deploy` directory.

For work under standing Jira delegation, migration 097 requires a current Jira observation and active approved skill and project context when granting an enqueue and when starting its send. If Jira withdraws delegation after a send may have started, Craft records the withdrawal and preserves the GitHub hold. Reconcile the send, then use the observed merge outcome or withdraw the queued entry before cancelling the work.

### Schedule GitHub readiness assessment

Apply migration 110 and approve the standing enqueue policy first. Run readiness under a queue verifier identity scoped to the repository with read-only `gh` authentication. Use a separate identity for merge confirmation so that one verifier cannot confirm the outcome of its own readiness receipt.

```powershell
craft-factory shared-ops --queue $queue --ca $ca poll-github-readiness --repository-map $repositoryMap --checkout-root $checkoutRoot --policy $githubPolicy --gh $gh --approved-protection-digest $protectionDigest --verifier-token-file $verifierToken
```

Each invocation selects at most one confirmed PR with a current passing verdict. The queue records the selection before the launcher loads the candidate pack or reads GitHub. That revision and policy can be retried after 90 seconds, measured from the later of selection or receipt. Other PRs remain eligible when the launcher exits before recording a result. Add `--report-file last-report.json` to retain the latest completed assessment or idle result. The publisher uses ready receipts under the same policy and repeats native checks before sending.

### Close standalone GitHub work on schedule

Apply migration 095. To authorize closeout when approving a standing enqueue policy, include `"close_standalone_work": true` in the policy JSON. Existing policies keep manual closure. Run the GitHub merge checker with a repository-scoped verifier identity separate from the readiness verifier and publisher.

```powershell
craft-factory shared-ops --queue $queue --ca $ca check-github-merges --policy $githubPolicy --gh $gh --verifier-token-file $mergeVerifierToken --close-scheduled --budget-seconds 60
```

The checker can release standalone work after an acknowledged scheduled enqueue and an independent merge observation for the exact reviewed PR and revision. The latest verdict for the same candidate pack must pass. The sweep retries on later runs after a pause or fresh passing verdict. The authorizing standing policy must still be active. Review `merge_check_report.scheduled_closure` for completed, deferred, or failed requests. Closure releases the queue review slot and records the enqueue and standing policy in the audit. Release series still require their rollout evidence.

### Restore a queue backup

For a restore rehearsal, use a trusted `pg_dump --format=custom` archive and restore into a fresh PostgreSQL database with `pg_restore --exit-on-error`. Keep the old service offline and revoke its access before connecting workers to the restored endpoint. Start the service on the restored database with `--recover-restored-database $epoch` and a fresh recovery UUID. It fences leases before opening the HTTPS listener, pauses new work, disables draft policies, and holds unfinished publications.

Use the same epoch if the service restarts during reconciliation. With an operator credential, run `recovery-status` to retrieve the active recovery or, once finished, its saved receipt. Its `recovery` field is `null` when the queue has no recovery history. Verifiers can inspect retained evidence, and the queue accepts verdict submissions again once recovery finishes.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem recovery-status
```

Inspect the holds and compare provider activity after the backup point with the retained receipts. A draft sent after the backup might have no restored ledger entry. Record the evidence from that reconciliation, rotate client credentials as needed, and finish the recovery explicitly.

```powershell
craft-factory shared-ops --queue https://localhost:8443 --ca queue-ca.pem finish-recovery $epoch --evidence $reconciliationReference
```

Finishing recovery leaves admission paused and draft policies disabled. Remove `--recover-restored-database` from the startup configuration, including any temporary service override, and restart in normal mode. A startup that still specifies the finished epoch is rejected before accepting traffic. Check the completed receipt with `recovery-status` and confirm admission is still paused before resuming it. Re-enabling draft publication requires a separate release decision.
