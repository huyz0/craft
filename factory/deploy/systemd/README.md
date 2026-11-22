# Run Craft Factory under systemd

These examples use separate `craft-queue` and `craft-runner` service accounts. A host can run either service or both. Install the extracted kit at `/opt/craft-factory` with files owned by root. Review the account names, binary paths, and configuration paths in the units before installing them. [Queue unit](craft-factory-queue.service) · [Codex runner unit](craft-factory-runner.service)

## Configure the queue host

- Apply the [queue migrations](../../shared/README.md#run-the-https-service) before starting the service. The queue starts paused after a fresh installation.
- Put the TLS chain, private key, and identity digest file in `/etc/craft-factory/queue`. Give the `craft-queue` account access to those files and restrict the private key and identity file to that account and its administrator.
- Write the service PostgreSQL connection URL on one line in `/etc/craft-factory/secrets/database-url`. Make it root-owned with mode `0600`. systemd supplies it to the queue through `LoadCredential`. Remove older `CRAFT_DATABASE_URL` and `CRAFT_DATABASE_URL_FILE` settings from the service environment if you want the queue to use this systemd credential.
- The example listens on loopback. For a separate runner host, select a private interface and use a hostname covered by the server certificate.

## Schedule one CI repair lane

Install the [CI repair service](craft-factory-ci-repair.service) and [timer](craft-factory-ci-repair.timer) under the dedicated account that owns the checkout, approved forge read credential, and Codex home. The example polls a GitHub job in shadow mode. Keep the attempt directory outside the checkout and retain it and the database across runs.

```bash
install -d -m 0700 "$HOME/.config/craft-factory/ci-repair" \
  "$HOME/.local/state/craft-factory/ci-repair/attempts" \
  "$HOME/.local/state/craft-factory/ci-repair/codex-home" "$HOME/.config/systemd/user"
install -m 0644 deploy/systemd/craft-factory-ci-repair.service deploy/systemd/craft-factory-ci-repair.timer "$HOME/.config/systemd/user/"
```

Place the reviewed files in the configuration directory as `work-order.json` and `github-policy.json`. In `digests.env`, set `CRAFT_FACTORY_CI_ORDER_DIGEST` and `CRAFT_FACTORY_CI_POLICY_DIGEST` to the approved SHA-256 values with the `sha256:` prefix. Adjust the executable paths in the service. For Bitbucket, replace the GitHub-specific arguments with `--bitbucket-policy` and `--step-name` for the approved policy and step. Include `--jira-snapshot` when the order requires one. [CI repair workflow](../../README.md#poll-for-an-approved-ci-repair)

Add `--follow-branch` for a recurring lane approved to follow branch advances. Keep the checkout synchronized through your normal repository sync so the service can verify that the observed tip descends from the approved work order base.

```bash
systemctl --user daemon-reload
systemctl --user start craft-factory-ci-repair.service
journalctl --user -u craft-factory-ci-repair.service -n 30
systemctl --user enable --now craft-factory-ci-repair.timer
```

Read `~/.local/state/craft-factory/ci-repair/last-report.json` for the last completed cycle. Check the service result if the report is old. The timer counts five minutes from completion. After the CI owner approves unattended repair, add `--admit --daily-limit 1` to the service command and reload the user manager. Use the same database with `set-ci-intake pause` before maintenance. [systemd timer behavior](https://www.freedesktop.org/software/systemd/man/latest/systemd.timer.html)

## Schedule shared CI intake

Follow [shared CI lane approval](../../shared/README.md#approve-recurring-shared-ci-intake) first. Use an observer account that has read-only forge access and an approved local checkout. Save the approved manifest as `~/.config/craft-factory/shared-ci.json`, the queue CA as `queue-ca.pem`, and the observer's protected intake token as `shared-ci-intake-token` in the same directory.

Create `~/.config/craft-factory/shared-ci.env` with the queue URL, approved manifest digest, and an empty mode for shadow polling.

```dotenv
CRAFT_FACTORY_QUEUE_URL=https://queue.example.com
CRAFT_FACTORY_CI_LANE_DIGEST=sha256:<approved-manifest-digest>
CRAFT_FACTORY_CI_LANE_MODE=
```

Copy the [service](craft-factory-shared-ci.service) and [timer](craft-factory-shared-ci.timer) into `~/.config/systemd/user/`. If the launcher lives outside `/opt/craft-factory`, update the service's executable path. Run `systemctl --user daemon-reload`, start the service once with `systemctl --user start craft-factory-shared-ci.service`, and inspect `journalctl --user -u craft-factory-shared-ci.service`. To poll every five minutes, use `systemctl --user enable --now craft-factory-shared-ci.timer`.

After the operator approves an admission-enabled manifest, set the corresponding digest and `CRAFT_FACTORY_CI_LANE_MODE=--admit`. During a stop, disable the lane in the queue and stop its timer with `systemctl --user disable --now craft-factory-shared-ci.timer`.

## Configure the runner host

- Enroll the runner and prepare the [checkout map and dedicated Codex home](../../shared/README.md#attach-a-launcher) under the `craft-runner` account. Create `/var/lib/craft-factory-runner/attempts` and `/var/lib/craft-factory-runner/codex-home` with mode `0700` and ownership `craft-runner:craft-runner`. Keep those directories across restarts.
- Put the checkout map and private CA PEM at `/etc/craft-factory/runner/repository-map.json` and `/etc/craft-factory/runner/queue-ca.pem`. Adjust the unit if you use public trust roots or install Codex elsewhere.
- Set `CRAFT_FACTORY_QUEUE_URL=https://queue.example.com` in `/etc/craft-factory/runner.env` using the queue's HTTPS origin. Write the runner token on one line in `/etc/credstore/CRAFT_FACTORY_RUNNER_TOKEN`, owned by root with mode `0600`. The unit loads it through `LoadCredential`. If migrating an existing runner, remove the old token entry from `/etc/craft-factory/secrets/runner.env` before restarting, since an explicitly set process variable takes precedence. [systemd service credentials](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html#Credentials)

## Start and maintain the services

From the extracted kit, install the unit for each role that runs on this host. Verify the installed units and start the intended services. For example, on a queue host:

```bash
sudo install -m 0644 deploy/systemd/craft-factory-queue.service /etc/systemd/system/
sudo systemd-analyze verify /etc/systemd/system/craft-factory-queue.service
sudo systemctl daemon-reload
sudo systemctl enable --now craft-factory-queue.service
sudo journalctl -u craft-factory-queue.service -n 50
```

Use the same commands with `craft-factory-runner.service` on a runner host. Check the queue's `/healthz` over HTTPS with the configured CA. On the runner, confirm that its journal records an idle poll or a submitted attempt. Before planned runner maintenance, [drain the runner](../../shared/README.md#take-a-runner-out-of-rotation) and wait for its active lease to finish before stopping the unit. Before a queue upgrade, pause admission, drain workers and settle publication holds, stop the queue service, apply migrations, then restart the service and verify health before restoring admission.

On `SIGTERM`, the queue closes its listener and lets accepted HTTPS connections finish within the existing handshake and request timeouts. The example unit gives that shutdown up to 45 seconds. If a client misses the response to a queue mutation, use the workflow's existing replay or reconciliation procedure.

For a restore, retain the same recovery epoch in your temporary service override while reconciling. After `finish-recovery`, remove the recovery argument from the override, reload the unit, and restart before resuming admission. [Restore a queue backup](../../shared/README.md#restore-a-queue-backup)

The runner unit delivers `SIGTERM` to the launcher and gives it up to 16 minutes to complete the current cycle and exit. The launcher can renew the active lease and submit its result during that period. Increase the stop timeout if you raise the default 15-minute attempt timeout. At the stop deadline, systemd can terminate the entire process group. Keep the attempt root for journal recovery when restarting. [systemd process termination](https://www.freedesktop.org/software/systemd/man/latest/systemd.kill.html)

## Schedule the Jira standing relay

Install the relay as a **user service** under the account that can read the approved Craft Jira profile from its unlocked Secret Service session. Prepare `~/.config/craft-factory/` with mode `0700` and these files.

- `jira-watch.json` contains the [approved watch policy](../../shared/README.md#relay-watched-jira-tickets) with an absolute Craft executable and state path.
- `jira-relay.env` contains `CRAFT_FACTORY_QUEUE_URL=https://queue.example.com` for the actual queue.
- `queue-ca.pem` contains the queue's trusted CA.
- `intake-token` contains the queue intake token on one line with mode `0600`. The unit mounts this file as a service credential for the relay.

Run a manual discovery or observation first without `--queue` to verify that the account can read Jira. Install both units in the user's manager. Start the service once and inspect its journal before enabling the timer.

```bash
install -d -m 0700 "$HOME/.config/systemd/user"
install -m 0644 deploy/systemd/craft-factory-jira-relay.service deploy/systemd/craft-factory-jira-relay.timer "$HOME/.config/systemd/user/"
systemctl --user daemon-reload
systemctl --user start craft-factory-jira-relay.service
journalctl --user -u craft-factory-jira-relay.service -n 30
systemctl --user enable --now craft-factory-jira-relay.timer
systemctl --user list-timers craft-factory-jira-relay.timer
```

The timer waits 30 seconds after the relay becomes inactive. The relay uses a 105-second batch budget so it can report completed and deferred entries before systemd's two-minute cutoff. It saves the next watch or discovery position in `~/.config/craft-factory/jira-relay.db`. Read `~/.config/craft-factory/jira-last-report.json` for the last completed batch, including partial results. Check its modification time and the journal if the relay failed before it could write a report.

If watches miss the five-minute freshness window, reduce the number in the policy or move some to a separate schedule. The timer runs while the user's service manager is available. Unattended operation after logout also requires that the manager and the approved credential store remain available. If polling stops or the vault is locked, the queue's standing observations expire under the normal five-minute rule. [systemd timer behavior](https://freedesktop.org/software/systemd/man/latest/systemd.timer.html)

## Schedule standing GitHub enqueue

Use the dedicated publisher account. Create `~/.config/craft-factory/enqueue/payments/` and its `checkouts/` subdirectory with mode `0700`. Put the approved GitHub policy in `policy.json`, the trusted source map in `repository-map.json`, and the queue CA in `queue-ca.pem`. Set `CRAFT_FACTORY_QUEUE_URL`, `CRAFT_FACTORY_ENQUEUE_POLICY_ID`, and the absolute `CRAFT_FACTORY_GH_PATH` in `publisher.env`. Set up read-only `gh` authentication on this account. Save the queue publisher token in `publisher-token` and the GitHub writer token in `github-writer-token`, both with mode `0600`. The service receives the tokens as credential files.

```bash
install -d -m 0700 "$HOME/.config/systemd/user"
install -m 0644 deploy/systemd/craft-factory-github-enqueue@.service deploy/systemd/craft-factory-github-enqueue@.timer "$HOME/.config/systemd/user/"
systemctl --user daemon-reload
systemctl --user start craft-factory-github-enqueue@payments.service
journalctl --user -u craft-factory-github-enqueue@payments.service -n 20
systemctl --user enable --now craft-factory-github-enqueue@payments.timer
```

Each run can enqueue one revision. The timer waits 30 seconds after the service stops. The service retains the latest completed result in `last-report.json` in its working directory. Check the journal if the file predates the last run. To suspend dispatch, stop the timer. To fence permissions already issued for sends that have yet to begin, revoke the standing policy in the shared queue. For an uncertain send, inspect the recorded hold and reconcile it with GitHub. [Publisher command and recovery](../../shared/README.md#schedule-standing-github-enqueue)

## Schedule GitHub readiness assessment

Use a dedicated verifier account and a repository-scoped queue credential. Create `~/.config/craft-factory/readiness/payments/` and an existing `checkouts/` subdirectory with mode `0700`. Put the approved GitHub policy in `policy.json`, the trusted repository map in `repository-map.json`, and the queue CA in `queue-ca.pem`. Set `CRAFT_FACTORY_QUEUE_URL`, `CRAFT_FACTORY_GITHUB_PROTECTION_DIGEST`, and the absolute `CRAFT_FACTORY_GH_PATH` in `verifier.env`. Use the approved native protection fingerprint for the digest. Configure read-only `gh` authentication for the verifier and put the queue token in `verifier-token` with mode `0600`.

```bash
install -d -m 0700 "$HOME/.config/systemd/user"
install -m 0644 deploy/systemd/craft-factory-github-readiness@.service deploy/systemd/craft-factory-github-readiness@.timer "$HOME/.config/systemd/user/"
systemctl --user daemon-reload
systemctl --user start craft-factory-github-readiness@payments.service
journalctl --user -u craft-factory-github-readiness@payments.service -n 20
systemctl --user enable --now craft-factory-github-readiness@payments.timer
```

The timer waits 30 seconds between runs. The verifier records readiness or blockers in the shared ledger and retains the latest completed result in `last-report.json` in its working directory. The publisher can then act under the matching standing policy. Use a separate verifier identity for later merge confirmation. [Readiness assessment](../../shared/README.md#schedule-github-readiness-assessment)

## Schedule a GitHub issue relay

Use a dedicated account with read access to the watched GitHub issue. Create a private instance directory such as `~/.config/craft-factory/github-issue-relay/routing/` with mode `0700`. Add the [approved watch and work order](../../shared/README.md#relay-a-watched-github-issue) as `github-watch.json` and `work-order-template.json`. Save the queue CA as `queue-ca.pem`, the queue intake token on one line in `intake-token` with mode `0600`, and the queue origin as `CRAFT_FACTORY_QUEUE_URL=https://queue.example.com` in `relay.env`. The service account's GitHub CLI session must be able to read the issue without prompting.

```bash
install -d -m 0700 "$HOME/.config/systemd/user"
install -m 0644 deploy/systemd/craft-factory-github-issue-relay@.service deploy/systemd/craft-factory-github-issue-relay@.timer "$HOME/.config/systemd/user/"
systemctl --user daemon-reload
systemctl --user start craft-factory-github-issue-relay@routing.service
journalctl --user -u craft-factory-github-issue-relay@routing.service -n 20
systemctl --user enable --now craft-factory-github-issue-relay@routing.timer
```

The timer waits 30 seconds after each run. Read `last-report.json` in the instance directory for the last completed queue decision. If a poll fails, compare the report's modification time with the service journal. The queue stops authorizing work when its last eligible observation expires.

## Schedule a Confluence context check

Use the account that can read the saved Confluence profile. Create a private instance directory, for example `~/.config/craft-factory/context-check/routing/` with mode `0700`. Keep it outside the Craft state directory. Put the [Confluence policy](../../shared/README.md#prepare-a-confluence-excerpt) in `confluence-policy.json`, the queue CA in `queue-ca.pem`, and the queue URL in `check.env` as `CRAFT_FACTORY_QUEUE_URL=https://queue.example.com`. Save the repository-scoped context-reader token on one line in `context-reader-token` with mode `0600`.

```bash
install -d -m 0700 "$HOME/.config/systemd/user"
install -m 0644 deploy/systemd/craft-factory-context-check@.service deploy/systemd/craft-factory-context-check@.timer "$HOME/.config/systemd/user/"
systemctl --user daemon-reload
systemctl --user start craft-factory-context-check@routing.service
journalctl --user -u craft-factory-context-check@routing.service -n 20
systemctl --user enable --now craft-factory-context-check@routing.timer
```

The timer runs every five minutes while the user manager is running. The saved Confluence profile must remain accessible to the service. Inspect `last-report.json` in the instance directory and the service journal after the first run. A completed drift check saves the mismatch and exits nonzero. A connector or queue error leaves the prior report, so compare its modification time with the journal.

## Schedule merge verification

Use a user service under the verifier account. For each repository, choose a simple instance name such as `payments` and make its directory under `~/.config/craft-factory/merge/` private with mode `0700`. The Bitbucket example uses `bitbucket-payments`. Use `github-payments` with the GitHub units.

- Put the approved forge policy in `policy.json` and the queue CA in `queue-ca.pem`.
- Set `CRAFT_FACTORY_QUEUE_URL` and `CRAFT_FACTORY_MERGE_POLICY_DIGEST` in `check.env`. Prefix the lowercase SHA-256 of the approved policy file with `sha256:`. For GitHub, also set `CRAFT_FACTORY_GH_PATH` to the absolute path of `gh` and configure its read-only authentication under this account.
- Store the queue verifier token on one line in `verifier-token` with mode `0600`. For Bitbucket, also store the read-only token on one line in `bitbucket-token` with mode `0600`. For a Bitbucket API token tied to an Atlassian account, set `CRAFT_FACTORY_BITBUCKET_EVIDENCE_EMAIL` in `check.env`. The units mount the token files as service credentials. [systemd service credentials](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html#LoadCredential=ID%5B:PATH%5D)

```bash
install -d -m 0700 "$HOME/.config/systemd/user"
install -m 0644 deploy/systemd/craft-factory-bitbucket-merge-check@.service deploy/systemd/craft-factory-bitbucket-merge-check@.timer "$HOME/.config/systemd/user/"
systemctl --user daemon-reload
systemctl --user start craft-factory-bitbucket-merge-check@payments.service
journalctl --user -u craft-factory-bitbucket-merge-check@payments.service -n 20
systemctl --user enable --now craft-factory-bitbucket-merge-check@payments.timer
```

The timer waits 60 seconds after a run becomes inactive. Each run allows 60 seconds for its batch and keeps `cursor.db` and `last-report.json` in the instance directory. Inspect the report and journal after the first run. The verifier records merge evidence for the queue. For optional GitHub standalone closeout, use a repository-scoped verifier and set `CRAFT_FACTORY_GITHUB_CLOSURE_MODE=--close-scheduled` in `check.env`. [Shared merge checks](../../shared/README.md#find-drafts-awaiting-a-merge-check) · [Scheduled closeout](../../shared/README.md#close-standalone-github-work-on-schedule)
