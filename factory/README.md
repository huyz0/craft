# Craft factory launcher

The factory launcher is a separate Rust executable. It keeps local workflow state in SQLite and can run headless Codex or accept commits from a developer's approved agent or editor. An operator publishes the PR in the forge and records observations against the exact candidate commit.

## Verify and install

Install Git and the Rust toolchain pinned in `rust-toolchain.toml`. Run these commands from this directory.

```powershell
cargo fetch --locked
cargo test --locked --offline
cargo install --path . --locked
```

The tests use temporary Git repositories and SQLite files. The same commands run in the credential-free Windows and Linux PR workflow. The simulated review records use example addresses and identities. Edits confined to the approved Factory guides run local link, section, and JSON example checks. Code, configuration, skills, and changes outside the documentation allowlist run the full suite. The `Factory required checks` result applies to both paths, and the job summary shows which checks ran and why.

New automatic runs for a PR cancel its superseded automatic runs. Manual reruns, merge-queue checks, and release runs use separate concurrency groups keyed by run ID.

To get a shared-service release from CI, run **Factory checks** manually on the source revision you intend to install. Download `craft-factory-release-attempt-N` after **Assemble qualified Factory release** succeeds. The bundle contains both platform archives with their checksums and qualification receipts. The top-level `release.json` binds those files to one source commit, workflow commit, run, and attempt.

Linux release receipts at version 5 bind the native worker comparison to the bundled kit. The verifier checks the report checksum and the digests of the launcher and queue inside the Linux archive. The report contains the verified Git tree, independent verdicts, and recorded usage. Both workers run the same task against scripted loopback endpoints. Their usage verifies accounting and provides no provider billing data. The regular pull request lane uses the credential-free fixtures.

New bundles use version 7 receipts. The Windows receipt requires the manual release job to pass the full HTTPS integration suite against the extracted kit. It also carries the checksum and packaged smoke result, including the Confluence drift scenario. Previously assembled bundles can still be verified with their original receipt versions.

```powershell
gh run download $runId -R $repository -n "craft-factory-release-attempt-$attempt" -D $bundle
python -B .\tools\release_kit_receipt.py install-bundle $bundle $installDir --source-commit $commit --workflow-commit $workflowCommit --repository $repository --run-id $runId --run-attempt $attempt
```

Run the installer from the source checkout with Python 3.11 or newer. Give it a new `$installDir` under an existing directory. It verifies both archives and extracts the kit for your operating system. Use `--platform windows` or `--platform linux` to prepare a kit for another host. Follow `QUICKSTART.md` in the installed directory. Use `verify-bundle` with the same pins and without `$installDir` when you only need verification.

Use full commit SHAs and the repository in `owner/repo` form. The workflow commit for a manual run of this workflow is the selected source revision. If a release attempt fails, rerun all jobs so both kits are produced in the new attempt. For a historical single-platform download, use the `verify` command with the archive, its adjacent receipts, and the expected source commit, repository, run ID, and attempt. [Download a workflow artifact](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/download-workflow-artifacts)

Version 3 qualification receipts require `queue_smoke` on Windows and Linux. The workflow runs the shipped test from each extracted kit against a disposable PostgreSQL database and validates authenticated access over loopback HTTPS. Earlier receipts retain their original check sets.

For a local preview, use Python 3.11 or newer and a new installation directory. Get the ZIP's SHA-256 from its delivery record. Check the digest before running the installer embedded in the ZIP.

```powershell
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $previewZip).Hash -ne $previewDigest) {
    throw 'Factory preview digest mismatch'
}
python -I -B $previewZip $installDir --sha256 $previewDigest
```

On Ubuntu, use `sha256sum` before running the installer.

```bash
printf '%s  %s\n' "$previewDigest" "$previewZip" | sha256sum --check --status &&
    python3 -I -B "$previewZip" "$installDir" --sha256 "$previewDigest"
```

The installer checks the ZIP again before extracting the selected platform. It runs without a source checkout. A local preview records no CI run or source commit. Use a CI release when you need that provenance.

To rehearse a takeover across both kits on one Windows machine, run the source-checkout fixture from Ubuntu WSL with mirrored networking and Windows interop enabled. Install both kits first. Set `ubuntuKit` to the Ubuntu installation, `windowsLauncher` to the Windows executable under a mounted drive, `windowsScratch` to an existing private directory on that drive, and `postgresBin` to the Ubuntu PostgreSQL bin directory.

```bash
python3 -B tools/qualify_wsl_handoff.py "$ubuntuKit" "$windowsLauncher" "$windowsScratch" --postgres-bin "$postgresBin"
```

The fixture uses an Ubuntu queue and disposable credentials over loopback HTTPS. An Ubuntu protocol client reports a stopped attempt, a separate Windows client takes over, and the Windows launcher independently verifies the replacement. The JSON report records the two generations and the rejection of the first client's late submission.

To drive both attempts through native Codex and the installed launchers, use the native handoff fixture. Set `ubuntuCodex` to the Ubuntu Codex executable and `windowsCodex` and `windowsPython` to the Windows executables as seen from WSL. Each platform needs Git and the pinned Rust toolchain to build the local model shim. The fixture uses scripted responses on each platform's loopback model server.

```bash
python3 -B tools/qualify_native_wsl_handoff.py qualify \
    --ubuntu-kit "$ubuntuKit" --ubuntu-codex "$ubuntuCodex" \
    --windows-launcher "$windowsLauncher" --windows-codex "$windowsCodex" \
    --windows-python "$windowsPython" --windows-workspace "$windowsScratch" --postgres-bin "$postgresBin"
```

The Ubuntu worker attempts an out-of-scope edit and records a stop. The Windows worker takes the next generation and produces an in-scope candidate. A separate verifier credential imports the exact Windows candidate from the queue into a checkout built from an independent source clone. The verifier rejects a tracked edit before recording the verdict. The report includes both launcher and Codex binary digests, the candidate pack digest, and the verification sequence. This exercise runs on one physical machine.

Add `--failure crash` after `qualify` to kill the disposable Ubuntu launcher during a model request. The check observes the model connection closing and the worker processes stopping. The Windows launcher must return no claim while the 15-second fixture lease is live. After the lease expires, it takes generation two and the queue rejects a late generation-one submission. The ledger records the original attempt with unknown token usage and no worker stop receipt.

To rehearse a network partition, run this source-checkout command as a non-root Linux user with unprivileged user and network namespaces enabled. The fixture needs `ip`, `unshare`, `nsenter`, `setpriv`, `curl`, Git, OpenSSL, Python, the pinned Rust toolchain, and the Ubuntu kit and Codex executable.

```bash
python3 -B tools/qualify_network_partition.py qualify \
    --kit "$ubuntuKit" --codex "$ubuntuCodex" --postgres-bin "$postgresBin"
```

The fixture starts the queue and two native runners in three isolated network namespaces on one machine. It drops network setup capabilities before starting either worker. It then disconnects the first runner's virtual link during a model request. The launcher stops the model after losing its queue connection, and the other runner waits for lease expiry. The fixture reconnects the first link to check that the queue rejects a late pack, and a separate verifier checks the replacement commit.

To rehearse queue contention, run the load fixture from the source checkout on either platform. Set `kit` to the installed shared kit and `postgresBin` to the local PostgreSQL bin directory.

```bash
python3 -B tools/qualify_shared_load.py "$kit" --postgres-bin "$postgresBin"
```

On Windows, use `python` in place of `python3`. The fixture uses eight concurrent protocol clients, 24 synthetic orders, and four review slots. It checks that claims and verifier pickups are unique, slots return after each batch, and reported usage reconciles across all attempts. Its p95 measurements include local HTTPS requests and apply to this fixture.

To build the Windows pilot kit, run these commands in PowerShell 7 from this directory. Packaging checks the launcher from a fresh extraction and prints the archive's SHA-256.

```powershell
cargo build --release --locked --offline
.\tools\package-windows.ps1 -Launcher .\target\release\craft-factory.exe -Output .\target\craft-factory-windows-x64.zip
```

Add `-FullQualification` when qualifying a release from the source checkout. This also checks the extracted documentation links and runs the disposable CI repair workflows against the launcher inside the archive. It requires Python, the pinned Rust toolchain, and the locked dependencies already fetched. With `-QueueService`, it also uses OpenSSL (available from Git for Windows) to run a loopback HTTPS check for authenticated runner setup, restart replay, and graceful stop. The installed kit's quickstart needs only PowerShell and Git for its offline smoke test.

To repeat the manual release job's Windows HTTPS check, supply the extracted shared-service kit and the directory containing the PostgreSQL server and client tools. The command creates and shuts down its own loopback database.

```powershell
python -B tools/qualify_windows_queue.py $extractedKit --postgres-bin $env:PGBIN
```

To rehearse an upgrade locally on Windows or Linux, pass the two extracted shared-service kits in upgrade order. The fixture stops the old queue after pausing claims, runs the new kit's migration helper against the same disposable database, and checks that the upgraded queue preserves a previously verified candidate and claims a pending order only after an explicit resume.

```powershell
python -B tools/qualify_queue_upgrade.py $previousKit $extractedKit --postgres-bin $env:PGBIN --report upgrade.json
```

To check an installed Codex build with the extracted launcher, run the optional native fixtures from the source checkout. They use an empty Codex home and a loopback model endpoint with scripted responses. The coordinator fixture checks the draft schema, streaming result, usage, and review handoff. The worker fixture executes an edit in a temporary checkout, imports the exact in-scope commit, rejects an out-of-scope edit, and checks usage across both model responses.

```powershell
python -B tools/qualify_native_coordinator.py $extractedLauncher (Get-Command codex.exe).Source
python -B tools/qualify_native_worker.py $extractedLauncher (Get-Command codex.exe).Source
```

The shared worker fixture also needs the extracted shared-service kit and PostgreSQL binaries. It runs the installed worker against a private queue and replays each candidate submission after its model endpoint has stopped. A separate verifier checks each detached candidate and records a verdict. A simulated draft and review exercise the repair path, including rejection of stale verification and a substituted PR URL.

```powershell
python -B tools/qualify_native_shared_worker.py $extractedKit (Get-Command codex.exe).Source --postgres-bin $env:PGBIN
```

Add `--github-issue` or `--jira-issue` to start from a fixture issue. Both check that the launcher stays idle until an operator approves the work order, then verify the initial and repaired candidates against the same simulated GitHub draft. The Jira variant changes the issue after its first approval and checks that replaying the old approval returns the original receipt while the runner stays idle. Use `--report PATH` to save the evidence to a new file. The isolated Linux release qualification runs both variants with the packaged executables.

On Linux, build the launcher and shared queue on a distribution compatible with your deployment hosts. From this directory, use Python 3.12 or newer to package them together with the migration assets. Packaging checks the extracted docs, service executable, and offline workflow, then prints the archive's SHA-256.

```bash
cargo build --release --locked
cargo build --release --locked --manifest-path shared/Cargo.toml
python3 tools/package_linux.py --launcher target/release/craft-factory \
  --queue-service shared/target/release/craft-factory-queue \
  --output target/craft-factory-shared-linux.tar.gz
```

The [Linux quickstart](QUICKSTART-LINUX.md) covers the extracted kit. The Linux CI job installs migrations from that kit and runs the shared HTTPS integration suite against both packaged executables.

From this source checkout, use one command to repeat the shared-service qualification locally on Linux. Run as a non-root user with Python 3.12+, PostgreSQL tools, Git, OpenSSL, the pinned Rust toolchain, Bubblewrap, `systemd-analyze`, Prometheus, and `promtool` installed. Pass `--postgres-bin` when the PostgreSQL server tools are outside `PATH`.

```bash
cargo fetch --locked
python3 -B tools/qualify_shared_kit.py target/craft-factory-shared-linux.tar.gz \
  --postgres-bin "$(pg_config --bindir)"
```

The command verifies the extracted smoke workflow, CI repair and coordinator scenarios, unit files, and alert rules. It runs the SQL and authenticated HTTPS fixtures against a fresh PostgreSQL instance on a private Unix socket, uses temporary identities and a local Prometheus scraper, and shuts down the database on exit. It prints the SHA-256 of the archive it qualified. Windows and Linux CI require the same launcher scenarios.

To check native Claude Code from the source checkout on Linux or WSL2, set `launcher` and `claude` to absolute paths to the extracted launcher and native Claude executable. Set `bubblewrap` to the absolute Bubblewrap path. The fixture uses the installed Claude binary and scripted model responses in a loopback-only network namespace. It checks file edits and creation, denial of a read outside the checkout, exact candidate import, and reported usage.

```bash
(
  scratch=$(mktemp -d)
  trap 'rm -rf -- "$scratch"' EXIT
  "$bubblewrap" --unshare-net --unshare-pid --ro-bind / / --proc /proc --dev /dev \
    --bind "$scratch" "$scratch" --setenv TMPDIR "$scratch" -- \
    python3 -B tools/qualify_native_claude_worker.py "$launcher" "$claude" "$bubblewrap"
)
```

Add `--shared-kit "$kit" --postgres-bin "$postgresBin"` to the Python command to run the shared qualification as well. Use the launcher from that extracted kit and the directory containing the PostgreSQL binaries. The shared test starts a private database and verifies repository approval, candidate replay, and independent verification. It then revokes approval during a native model request and checks that the connection closes, no candidate is submitted, and the next claim is blocked.

Run `craft-factory smoke --destination $newAbsoluteDirectory` to exercise delegation, uncertain publication recovery, stale evidence rejection, and a same-PR repair with synthetic inputs. The command requires Git and creates a new directory. It runs without a model account or forge connection.

Run `craft-factory --state pilot.db doctor` to check Git and the local execution database. To probe authentication as well, pass `--codex $codexExe --codex-home $pilotCodexHome` and, for GitHub, `--gh $ghExe`. Each external probe has a 15-second deadline. An authentication probe that finishes reports login availability. The supervised smoke and actual tenant checks qualify sandbox and repository permissions separately.

## Try the supervised workflow

Choose a private local folder for the database and work-order files. Use the same database for all orders in the pilot so the launcher can enforce the single review slot. Copy the [work-order example](examples/work-order.json) and replace the example values using a human-approved snapshot of the Jira issue.

For a CI repair, use a work order whose base is the exact failed commit, whose intent summarizes the redacted failure, and whose acceptance criteria say how to demonstrate the fix. Point the checkout's `origin` at the repository that failed.

For GitHub Actions, add `ci_repair` to the [GitHub policy](examples/github-policy.json) with the approved numeric `workflow_id` and exact `job_name`. That name must appear once in `required_checks` with the GitHub App ID the repository owner approved. Use a trusted, authenticated GitHub CLI with Actions read access. Select a failed push run on the approved branch, and use its exact run ID and attempt number.

```powershell
craft-factory --state $state delegate-github-failure $order $githubPolicy --gh $gh --run 123456789 --attempt 2
```

Admission confirms that the branch still points to the failed commit and the same run attempt is current. The receipt links to the job without collecting its logs. After publishing a draft in that repository, run `check-github` with the same policy. It verifies the candidate's required checks and reads the original failure again. A rerun of the original job or a branch move requires the owner to revisit the repair intent.

Use `poll-github-failure` with the same approved policy and work order for a one-shot check a scheduler can call. It examines the newest push run of the configured workflow for the pinned commit. A successful or pending newer run supersedes an older failure. If a different job failed or the workflow history is incomplete, the poll records its decision without opening a repair.

```powershell
craft-factory --state $state poll-github-failure $order $githubPolicy --gh $gh
```

The default is shadow mode. Pass `--admit` to let an eligible failure reserve the local review slot, subject to `--daily-limit` and `set-ci-intake pause`. The limit counts admissions from both forge pollers in the same database over the previous 24 hours. `ci-poll-history` shows the decisions and evidence. Keep the work order at the approved commit and refresh its intent and base through review when the branch advances.

Use `run-ci-cycle` as a single command in your scheduler. It checks one approved job or step and returns a JSON object with `poll` and `dispatch` receipts. Start in shadow mode with the commands below. After reviewing the poll history, add `--admit` to allow one repair start per invocation, subject to the rolling admission limit and the local review slot. Keep the same database and work order across invocations. If a process exits after admission, the next cycle can start that same repair while the forge still reports the same eligible failure. Point `--attempt-root` at an existing private directory outside the source repository.

```powershell
craft-factory --state $state run-ci-cycle $order --github-policy $githubPolicy --gh $gh --attempt-root $attemptRoot --codex $codex --codex-home $codexHome
craft-factory --state $state run-ci-cycle $order --bitbucket-policy $twgPolicy --step-name 'Verify candidate' --attempt-root $attemptRoot --codex $codex --codex-home $codexHome
```

For a scheduled lane, add `--order-digest $approvedOrderDigest --policy-digest $approvedPolicyDigest --report-file $report`. Use the exact approved `sha256:` digests. A mismatch stops the next cycle before it queries the forge or writes a poll receipt. The cycle uses the same policy snapshot for polling and dispatch. The report file retains the last completed cycle, so check the scheduler result if the report stops updating.

For a recurring lane approved to follow its configured branch, add `--follow-branch` with both digests. The work order's `base` is the approved starting commit. Keep the checkout synchronized through your normal repository sync so it contains the tip reported by the forge. The cycle includes that tip in its report and blocks if the commit is missing locally, falls outside the approved ancestry, or the branch moves during verification. Any repair is pinned to the exact failing commit. Omit the flag for a lane scoped to a single approved commit.

`ci-poll-history` retains the branch name, approved starting commit, and observed tip with each branch-following decision, even after the last report is replaced. If the forge could not identify a tip, the observed tip is null.

`dispatch` is `null` in shadow mode or when the poll does not match an admitted work order. A scheduler can also keep polling and dispatch separate. Use `run-ci-repair` against the same database to find and recheck a persisted admission. For Bitbucket use `--bitbucket-policy $twgPolicy` in place of `--github-policy $githubPolicy --gh $gh`.

If Bitbucket returns incomplete target details for a newer pipeline, Craft blocks the poll or dispatch until it can identify whether the newer run superseded the failure. A newer run of the same default pipeline blocks the older repair even while the newer run is still in progress.

When an admitted failure cannot be confirmed, `run-ci-repair` gives the current reason and `status <work-id>` retains one `ci_dispatch_blocked` event across scheduler restarts. A subsequent poll blocked by the review slot names its occupant. Retry after a temporary provider failure. If the repair is obsolete, use `cancel <work-id> --reason <reason>` to release the slot and keep the admission history. That admission still counts toward the rolling limit.

```powershell
craft-factory --state $state run-ci-repair --github-policy $githubPolicy --gh $gh --attempt-root $attemptRoot --codex $codex --codex-home $codexHome --timeout-seconds 900
```

The launcher rechecks the failure with the forge before opening a checkout. It takes the same execution lock as `run` and checks the pause state again when it records the start. A successful attempt returns `candidate_ready` and the commit. Publish the draft through the supervised workflow and run independent verification on that revision. After a failed or interrupted start, subsequent scheduler passes return `needs_operator`. Inspect `status <work-id>` for the recorded attempt. To retry that work order, append `--retry <work-id>` to the `run-ci-repair` command above. The same failure must still be eligible, intake must be unpaused, and any attempt limit in the execution profile still applies. Use `run` after a reviewer requests changes to an existing candidate.

For a Bitbucket branch pipeline failure, select a completed build and failing step UUID. Include the failing step name in the [TWG policy](examples/twg-bitbucket-policy.json).

```powershell
craft-factory --state pilot.db delegate-bitbucket-failure repair-order.json twg-bitbucket-policy.json --build $buildNumber --step $stepUuid --jira-snapshot jira-snapshot.json
```

The launcher records the pipeline and step identities, completion time, commit, and evidence link. It checks the current branch and recent build history at admission and verification. If a newer run of the same workflow supersedes the failure, inspect that run before opening repair work. The same failed attempt occupies at most one work order, and a repair must pass the original required step on the resulting PR. Keep raw pipeline logs out of the work order.

Add `--shadow` to print an admission decision and reason as JSON. The command checks the same provider evidence, snapshot, duplicate identity, and review slot as normal admission. It rolls back the local reservation and starts no runner. A `would_admit` result applies to the state at the time of inspection.

### Poll for an approved CI repair

Run one poll from an operator-owned scheduler using the same local database and pinned repair template. The poll reads Bitbucket through the saved TWG profile. It considers only the default branch pipeline and the exact step name in the approved policy. With this one-shot command, review and update the template when the branch advances. For an approved recurring lane, use `run-ci-cycle --follow-branch` with both digests.

```powershell
craft-factory --state pilot.db poll-bitbucket-failure repair-order.json twg-bitbucket-policy.json --step-name 'Verify candidate' --jira-snapshot jira-snapshot.json
craft-factory --state pilot.db ci-poll-history --limit 20
```

Polling records shadow decisions by default. Once you approve admission, add `--admit` to the scheduled command. The default cap is one admitted repair across the database in a rolling 24-hour window. If the approved cap differs, use the same `--daily-limit` value on every invocation. Admitted work enters the existing review slot. Starting the worker and publishing a PR remain operator actions.

`set-ci-intake pause` holds new poll admissions while current work can finish or be cancelled. `set-ci-intake resume` reopens admissions.

For a digest-pinned Jira snapshot, use an HTTPS issue URL ending in `/browse/<issue>` and a `sha256:<digest>` revision in the context reference. The JSON snapshot must provide `key`, `url`, `intent`, and `acceptance` matching the order. Pass `--jira-snapshot` when delegating, or use `pin-jira-snapshot <id> <file>` for an existing order. The launcher rechecks those exact bytes before running, binding a PR, or recording review evidence. Refresh the Jira issue in the browser and compare its current intent before publishing when using this supervised snapshot path.

To prepare these files from Jira, save a trusted read policy with `schema_version: 1`, absolute `craft` and `craft_state` paths, `craft_profile`, the exact `cloud_id` and numeric `issue_id` as strings, and the HTTPS `issue_url`. Use the [work-order template](examples/work-order.json) with the approved scope and acceptance criteria. The template can leave `context` empty when the Jira issue is its only context pin.

```powershell
craft-factory capture-jira-snapshot jira-policy.json work-order-template.json
craft-factory capture-jira-snapshot jira-policy.json work-order-template.json --approval-digest $approvalDigest --destination $newPrivateDirectory
craft-factory --state $state delegate (Join-Path $newPrivateDirectory 'work-order.json') --jira-snapshot (Join-Path $newPrivateDirectory 'jira-snapshot.json')
```

Review the preview against the Jira issue and proposed work order before passing its `approval_digest`. Preparation rereads Jira. A change to the issue summary, description, assignee, status, or work order requires a fresh preview and review. Use a restricted parent directory for the output. The command reads Jira and writes the two local files. Later supervised stages verify these files against the frozen pin.

For captured snapshots, run `check-jira-snapshot` before publishing the draft in the forge. Add `--jira-policy jira-policy.json` to `run` or `bind-draft` to check Jira as part of that command. These reads use your saved Craft profile before launching a worker or recording the draft. If Jira has changed or is unavailable, those commands stop. Older snapshots can still use the existing local verification and manual Jira review.

```powershell
craft-factory --state $state check-jira-snapshot $workOrderId jira-policy.json
```

A missing or changed pinned snapshot clears review readiness. The receipt reports the problem and retains the candidate and prior evidence for inspection.

### Delegate a GitHub issue

Use a GitHub.com credential with read access to Issues in the source repository. The policy needs `schema_version: 1`, the absolute `gh` executable path, `repository` in `owner/name` form, and the issue's numeric `number` and stable REST `issue_id`. Read the stable ID with `gh api --hostname github.com repos/OWNER/REPO/issues/42 --jq .id`. Use `owner/name#42` as `issue` in the work-order template. The template specifies the approved scope, local checkout, and acceptance criteria. [GitHub Get an issue](https://docs.github.com/en/rest/issues/issues#get-an-issue)

```powershell
craft-factory capture-github-issue-snapshot issue-policy.json work-order-template.json
craft-factory capture-github-issue-snapshot issue-policy.json work-order-template.json --approval-digest $approvalDigest --destination $newPrivateDirectory
craft-factory --state $state delegate (Join-Path $newPrivateDirectory 'work-order.json') --issue-snapshot (Join-Path $newPrivateDirectory 'github-issue-snapshot.json')
craft-factory --state $state check-github-issue-snapshot $workOrderId issue-policy.json
```

Review the issue and work-order template before using the preview's `approval_digest`. Preparation rereads the issue. The snapshot keeps the title and body fingerprint, stable assignee IDs, state, source identity, and the approved work-order intent. The remote title and body stay out of the snapshot and worker handoff. A transferred or closed issue, a reassignment, or an edited title or body requires review. The update timestamp is informational so comment activity alone does not invalidate the pin.

Add `--github-issue-policy issue-policy.json` to `run` or `bind-draft` to require a live match at that step. Run the explicit check before manually publishing a draft. For an operator-approved standing rule and a scheduled watch, see [Relay a watched GitHub issue](shared/README.md#relay-a-watched-github-issue).

### Prepare and submit a candidate

```powershell
craft-factory schema work-order
craft-factory schema review-record
craft-factory schema receipt
craft-factory --state pilot.db delegate work-order.json
craft-factory --state pilot.db status PILOT-1
craft-factory --state pilot.db submit PILOT-1 --commit $candidate --note 'Candidate ready for checks'
```

The order needs a unique ID, an issue and owner, an absolute local repository path, a full base commit, intent and acceptance criteria, allowed repository paths, and references to the approved context and skill revisions. The repository is the local checkout where candidates can be resolved. Use literal paths such as `src` or `docs/guide.md` for scope. Record the actual review policy in the pilot setup before using the launcher with live work.

Use `prepare --handoff` to continue in an approved editor or interactive agent. The command creates a separate checkout with no remote and writes `worker-handoff.json` beside it. The file includes the work order's intent, acceptance criteria, scope, context pins, the exact committed skill, and any feedback for the reviewed head. Open the returned checkout in the tool and give it the handoff. Commit the scoped work there, then submit the exact commit from that checkout. Include the tool version and any test evidence in the handoff note.

```powershell
$prepared = craft-factory --state pilot.db prepare PILOT-1 --destination $newAbsoluteDirectory --handoff | ConvertFrom-Json
# After committing in the prepared checkout
$candidate = git -C $prepared.checkout rev-parse HEAD
craft-factory --state pilot.db submit PILOT-1 --checkout $prepared.checkout --commit $candidate --note 'Editor version and test evidence'
```

For a repair, add `--replaces $prepared.head` to submission. The checkout starts at that reviewed head. Craft checks every commit before importing it and keeps the trusted source branch at its current revision. Bind the same draft to the replacement head and repeat verification. For unattended execution, use the launcher-managed runner and its operating-system sandbox.

To run Codex, use an approved executable and a dedicated, authenticated Codex home. Keep publication credentials out of that home. Use a new absolute run directory for each attempt. The pinned skill must be a `repo:` reference to a committed `SKILL.md` in the selected repository. The launcher loads its contents from that exact Git revision.

```powershell
craft-factory --state pilot.db run PILOT-1 --destination $runDirectory --codex $codexExe --codex-home $pilotCodexHome
```

The runner observes process completion and reported token usage. The agent leaves edits in the checkout. The launcher checks scope, creates one commit, and retains it under a local `refs/craft-factory/candidates/` ref without moving the source branch. After a request for changes, the same command starts from the reviewed head and passes the recorded feedback. If the order has attempts remaining, restart a timed-out or interrupted run with a fresh run directory. The launcher requires process containment before authorizing the worker. On Windows it uses the Codex MXC sandbox.

Add an [approved execution profile](shared/README.md#pin-a-codex-execution-profile) to a Codex or Claude work order to pin the requested model, effort, timeout, and attempt limit. Local execution counts each supervised start against the limit, including failed starts and repair runs. The local receipt records the profile and effective timeout.

Manual submissions may contain several linear commits. The launcher checks the paths in each commit, including edits that a later commit reverts. A repair must descend from the head that received feedback.

After publishing the candidate as a draft in the forge, bind the PR and its observed head. Verify CI and independent checks in the forge and record the observation using a [review record](examples/review-record.json). Use the same schema for a request for changes or final outcome. The [verification skill](skills/verify-candidate/SKILL.md) shows what evidence to collect.

Set `source` to `operator_observed` when you checked the evidence and actor in the forge. Set it to `simulated` for a rehearsal. Older records without this field report `unspecified`. Use the forge to verify reviewer eligibility and the actual PR decision. A local review record captures the operator's observation.

Run `craft-factory --state pilot.db prepare-verification PILOT-1 --head $candidate --destination $newAbsoluteDirectory` to give the verifier a clean checkout of the submitted commit. The command returns the checkout path and full SHA without running candidate code. Run the trusted checks in the approved test environment and record the tested SHA with the results.

For an order admitted by a CI poller, bind the published draft and record the passing native checks first. Add `--handoff` to `prepare-verification` to write `verification-handoff.json` beside the checkout. It contains the approved acceptance criteria, changed paths, context pins, and the recorded native checks. Give the packet and checkout to the independent verifier. After their checks, record the evidence with the matching packet.

```powershell
craft-factory --state $state attest-checks $workId $reviewRecord --handoff $handoffFile
```

The launcher checks the packet against the issued digest and the current PR revision. A new native observation makes the old packet stale even if the head is unchanged. Prepare a fresh packet and record a fresh evidence reference before closing the order.

```powershell
craft-factory --state pilot.db bind-draft PILOT-1 --reference $pullRequest --head $candidate
craft-factory --state pilot.db attest-checks PILOT-1 checks.json
craft-factory --state pilot.db request-changes PILOT-1 feedback.json
craft-factory --state pilot.db submit PILOT-1 --commit $repair --replaces $candidate --note 'Addressed feedback'
craft-factory --state pilot.db bind-draft PILOT-1 --reference $pullRequest --head $repair
craft-factory --state pilot.db attest-checks PILOT-1 repaired-checks.json
craft-factory --state pilot.db close PILOT-1 outcome.json
```

For a GitHub repo, copy the [GitHub policy example](examples/github-policy.json). Have the repository owner confirm the check names, issuing GitHub App IDs, base branch, and paths that define required checks. Use an authenticated GitHub CLI with read access to the PR and checks. The launcher confirms the live PR head and pinned base before and after collecting check runs for the candidate.

```powershell
craft-factory --state pilot.db check-github PILOT-1 github-policy.json --gh $ghExe
craft-factory --state pilot.db brief PILOT-1
```

The brief reports the pinned base, candidate revision, and Git line counts. It shows the largest changes and calls out binary files. After a change request it also shows the last recorded feedback and measures the repair against the reviewed revision. The brief reports the last observed native check results and the independent human verification separately. Confirm current check and approval status in the forge before merging.

To return a GitHub review to a worker, set `reviewer_ids` to the numeric IDs of the approved reviewers in the repository policy. After a reviewer submits **Request changes**, take the review ID from the `pullrequestreview-` link and put the actionable feedback in a short, operator-reviewed UTF-8 summary. The launcher checks that the review is active on the current commit and that the reviewer is approved and distinct from the PR author. It records the native review link and passes the reviewed summary to the worker. [GitHub review API](https://docs.github.com/en/rest/pulls/reviews)

```powershell
craft-factory --state $state request-changes-github $workId $githubPolicy --gh $gh --review $reviewId --summary $summaryFile
```

The repaired candidate stays on the same work order and PR. Bind the new head after updating the PR, then collect fresh native and independent verification before review.

After GitHub reports the PR merged, run `close-github` with the same policy. It closes the local order only when the merged PR points to the exact verified candidate and the destination in policy. The receipt keeps the PR head, pinned base, merge actor, time, and resulting commit. GitHub reports the result commit according to the merge method, so that commit can differ from the reviewed head. [GitHub pull request API](https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request)

Set `minimum_merge_approvals` above zero before running `check-github` to require approval evidence at closure. The first successful check with this setting pins the minimum and eligible `reviewer_ids` for the work order, including subsequent repairs. `close-github` counts the latest effective approval for each eligible reviewer on the exact merged head. Each counted approval must precede the merge, and an outstanding request for changes from an eligible reviewer blocks closure. The receipt preserves links to the qualifying reviews. [GitHub review API](https://docs.github.com/en/rest/pulls/reviews)

```powershell
craft-factory --state $state close-github $workId $githubPolicy --gh $gh
```

For Bitbucket Cloud, copy the [feedback policy example](examples/bitbucket-feedback-policy.json) and set the repository and approved reviewer UUIDs. The reviewer selects **Request changes** and posts a top-level comment with the full reviewed commit and work item. The comment supplies the exact revision because Bitbucket's participant state reports the decision without a reviewed commit. [Bitbucket pull request API](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-pullrequests/)

```text
Craft request changes v1
{"source_commit":"<full reviewed commit>","work_item":"PILOT-1"}
```

Use an existing read-only Bitbucket credential with pull request and repository access. The command uses `CRAFT_FACTORY_BITBUCKET_EVIDENCE_TOKEN` and the optional `CRAFT_FACTORY_BITBUCKET_EVIDENCE_EMAIL` for Atlassian API-token authentication. It checks the active review decision, comment author, and exact live source and destination refs. The worker receives the operator-reviewed summary file, and the receipt links back to the comment.

```powershell
craft-factory --state $state request-changes-bitbucket $workId $feedbackPolicy --comment $commentId --summary $summaryFile
```

After the Bitbucket PR is merged, use `close-bitbucket` with the same repository policy and read-only credential. It matches the native merge activity to the verified head and configured destination, reads the merge actor and result from the PR, and resolves abbreviated commit IDs. The local review slot is released after Craft records the outcome. [Bitbucket pull request activity API](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-pullrequests/#api-repositories-workspace-repo-slug-pullrequests-pull-request-id-activity-get)

```powershell
craft-factory --state $state close-bitbucket $workId $feedbackPolicy
```

To require independent Bitbucket approvals at closure, set `minimum_merge_approvals` above zero in the feedback policy and pass `--feedback-policy $feedbackPolicy` to `check-bitbucket-twg`. That pins the reviewer UUIDs and minimum across subsequent checks and repairs. Each eligible reviewer approves in Bitbucket and posts a top-level comment naming the exact reviewed commit and work item before the merge. [Bitbucket PR comment API](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-pullrequests/#api-repositories-workspace-repo-slug-pullrequests-pull-request-id-comments-comment-id-get)

```text
Craft approve merge v1
{"source_commit":"<full reviewed commit>","work_item":"PILOT-1"}
```

Pass `--approval-comment $commentId` to `close-bitbucket` once per distinct approving reviewer. Craft checks the comment timestamps and current participant approvals, then records the qualifying comment links in the merge receipt.

Set `require_native_checks` to `true` in a work order to require native verification from the first candidate. Once a work order uses a native verifier, each replacement revision requires a fresh passing check. A subsequent failed or unavailable check blocks review readiness and closure until verification succeeds. Use `status` to inspect `native_ci` and `review_ready`.

If the launcher stopped during a provider check, run the check again. Review stays pending until the latest check completes, and a slower result from an earlier check cannot override it.

For Bitbucket, copy the [Bitbucket policy example](examples/bitbucket-policy.json). Use a Craft build with the Bitbucket provider and an existing profile whose policy permits `bitbucket.pullrequest.get`, `bitbucket.pipeline.get`, and `bitbucket.step.list`. The profile must be able to read the repo's PRs and Pipelines. The launcher reuses that profile through Craft and never handles the token value.

```powershell
craft-factory --state pilot.db check-bitbucket PILOT-1 bitbucket-policy.json --pipeline $pipelineUuid
craft-factory --state pilot.db brief PILOT-1
```

The Craft command records pipeline steps for the candidate commit. The Bitbucket native CI gate requires a TWG observation that also verifies the live PR and its pinned base.

To use an existing TWG OAuth login, copy the [TWG policy example](examples/twg-bitbucket-policy.json) and set the installed executable, profile, site, and pilot repository. The launcher checks the open PR, live source and destination branch SHAs, successful PR-targeted pipeline, and configured steps. It expects the destination branch to remain at the pinned base during the pilot. If the base advances, review the new base and replan before verification.

```powershell
craft-factory --state pilot.db check-bitbucket-twg PILOT-1 twg-bitbucket-policy.json --build $buildNumber
```

Each review record includes the same PR reference, its exact reviewed head, the human actor, a stable evidence reference, and a brief note. These are operator attestations. Approval and merge stay in the forge. Check the actual PR state before binding an uncertain publication. Use `status` to resume from the durable record, or `cancel PILOT-1 --reason ...` to release the slot and keep the history.

If publication has an unknown result, use `hold-publication PILOT-1 --head $candidate --reason ...`. The slot remains held until you reconcile it. Bind the observed PR if it exists. If authoritative evidence shows publication did not occur, record that evidence using `clear-publication PILOT-1 reconciliation.json` before deciding whether to publish again.

If Bitbucket needs a new source-branch event to start its PR pipeline, create a content-identical child commit and register it with `retry-ci PILOT-1 --previous $candidate --commit $retry`. The launcher accepts this only for a published candidate awaiting verification. After pushing the child and observing the remote head, bind the same draft at the new commit before recording checks. [Atlassian's PR pipeline guidance](https://support.atlassian.com/bitbucket-cloud/kb/how-do-i-manually-trigger-a-pull-requests-pipeline-on-a-pr/)

## Evaluate a merge in shadow mode

Use the `shadow-merge` command to evaluate a proposed Bitbucket merge from exported evidence and local Git history. Run it from a trusted checkout with an origin for the selected repository. Supply the work order, the full head from the candidate handoff, and the SHA-256 digest issued with the approved policy.

```powershell
craft-factory shadow-merge work-order.json --head $candidateHead --policy shadow-merge-policy.json --approved-policy-digest $policyDigest --observation shadow-merge-observation.json --require-eligible
```

The policy pins the repository, destination branch, branch restriction revision, observation issuer, permitted paths, required check issuers, and eligible reviewer account IDs. The snapshot contains the observed target and source SHAs, effective restrictions, current checks, and the latest review decision for each account. Collect it through a trusted read-only export. Use exact-commit review evidence, since an account's approval of an earlier revision does not satisfy this check.

The report includes input digests, counted approvals and check references, and any blockers. The path check covers the whole candidate history, including protected files touched and later reverted. `--as-of` accepts a Unix timestamp for offline replay. When `--require-eligible` is set, any blocker produces a nonzero exit code after the JSON report. The result is a dry-run decision.

[Example policy](examples/shadow-merge-policy.json) · [Example observation](examples/shadow-merge-observation.json)

For a GitHub work order with independent verification and native checks already recorded, `shadow-merge-github` reads the PR, branch tip, classic branch protection, active branch rules, check runs, and review history. It requires the approved SHA-256 of the policy file and a separately approved protection fingerprint. The fingerprint includes active rules inherited from higher levels. A blocked protection-mismatch report includes the observed fingerprint so an operator can inspect the corresponding GitHub settings before approving it. The GitHub CLI credential needs read access to these endpoints.

```powershell
craft-factory --state $state shadow-merge-github $workOrderId $githubPolicy --gh $gh --approved-policy-digest $policyDigest --approved-protection-digest $protectionDigest --require-eligible
```

The dry run requires classic branch protection and checks all active branch rules. Unknown rules, a moved base, or changed evidence block eligibility. For a branch that requires the GitHub merge queue, set `"require_merge_queue": true` in the approved policy. Craft requires an active queue ruleset with the `ALLGREEN` grouping strategy. It verifies that every applicable ruleset has a visible, empty bypass list and includes their identities and conditions in the protection fingerprint. GitHub only exposes the bypass list to identities that can edit the ruleset. Use an authorized operator identity for that inspection. [Branch protection API](https://docs.github.com/en/rest/branches/branch-protection#get-branch-protection) · [Repository rules API](https://docs.github.com/en/rest/repos/rules#get-a-repository-ruleset)

Merge-queue assessments also check that the exact PR is open, has the expected head and base, and has no existing queue entry or pending auto-merge request. An eligible report includes the PR node ID and the applicable ruleset IDs. The command reads merge readiness. GitHub runs the required checks again against the merge group when a PR is enqueued. Configure any required Actions workflows to handle `merge_group`. [Managing a GitHub merge queue](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue)

### Corroborate the snapshot with Bitbucket

Add `--native-preflight` to read the PR, both branch refs, and Bitbucket's mergeability checks. Use `CRAFT_FACTORY_BITBUCKET_SHADOW_TOKEN` for a read-scoped Bearer credential. For an Atlassian API token, also set `CRAFT_FACTORY_BITBUCKET_SHADOW_EMAIL`. The token needs PR and repository read access. The preflight reports the permissions of the authenticated Bitbucket account, which is the identity the provider checks. [Bitbucket mergeability checks](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-pullrequests/?trk=article-ssr-frontend-pulse_little-text-block#api-repositories-workspace-repo-slug-pullrequests-pull-request-id-mergeability-checks-get)

Use Bitbucket account UUIDs in the export's author, reviewers, and policy reviewer allowlist. The command checks whether counted reviewers still show as approved in Bitbucket. The trusted export must still supply each approval's exact reviewed commit and the named CI issuer evidence.

The first preflight reports the fingerprint of the returned standard restrictions and blocks on the missing pin. After reviewing the restrictions, put that fingerprint in the approved policy's `native_standard_checks_digest` and refresh the policy digest in the export and command. A pinned policy always runs the preflight on the current clock. Changes to the supported restrictions, branch heads, or approval state can block the result. Bitbucket documents conditions it evaluates only during merge, so actual repository-control qualification remains open. [Bitbucket OpenAPI schema](https://api.bitbucket.org/swagger.json)

## Trace a running release

Export a release evidence bundle from the systems that own runtime inventory, deployment history, build provenance, and change approvals. Include the source, stable record reference, and observation time for each record. Use the same service, environment, region, and resource identifiers in the inventory and deployment exports. Each export should contain only the records the investigator is authorized to see. [Example evidence bundle](examples/release-evidence.json)

```powershell
craft-factory schema release-evidence
craft-factory trace-release examples/release-evidence.json --service sample-api --environment staging --region au-east --resource sample-api-blue --as-of 1795238400 --max-age-seconds 120
```

Set `--as-of` to the incident time in UTC Unix seconds and `--max-age-seconds` to the maximum age your team accepts for inventory and deployment observations. The trace compares the observed deployment ID, configuration revision, and SHA-256 artifact digest. It then uses the digest and repository to find build provenance and the full commit to find the PR and approved intent revision. The result includes the team responsible for the running resource and the references used at each step.

`gap: null` indicates a complete chain in the supplied evidence. A gap names the first missing, ambiguous, stale, mismatched, or time-inconsistent step. Use the returned references to investigate that step. The command reads a local bundle and relies on the access controls and authenticity checks of the systems that exported it.

### Import a Kubernetes Pod observation

Have the authorized inventory collector capture the named Pod as JSON, for example with `kubectl --context <approved-context> --namespace <namespace> get pod <name> -o json`. Retain the observed UTC timestamp and the mapping from that Pod UID and container to the incident's service, environment, region, and resource. The snapshot can contain fields unrelated to diagnosis, so keep it with the collector's restricted evidence. [kubectl get reference](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_get/)

```powershell
craft-factory import-kubernetes-inventory release-evidence.json --pod-snapshot pod.json --cluster staging-one --namespace sample-stage --pod api-7c6-94k --pod-uid $podUid --container api --service sample-api --environment staging --region au-east --resource sample-api-blue --owner 'Sample API team' --observed-at $capturedAt > enriched-release-evidence.json
```

The importer checks the Pod UID, the container's running state and start time, and agreement between the image pin and the runtime-reported digest. It reads the Pod annotations `craft.factory/deployment-id` and `craft.factory/config-revision` and emits the normalized observation with the source file's SHA-256 digest. The observation covers one container in one Pod. Some runtimes resolve an image pin to a different identifier, such as a platform-specific manifest. Those snapshots need separate provenance to establish the artifact mapping. [Kubernetes Pod API](https://kubernetes.io/docs/reference/kubernetes-api/core/pod-v1/) [Image names and digests](https://kubernetes.io/docs/concepts/containers/images/)

Run `trace-release` against the enriched bundle. The separately exported deployment record must agree with the observed deployment ID, configuration revision, and artifact digest. For this source, use the deployment time at which the version was applied to the target. A deployment applied after the container started cannot explain that running instance. The build and change records supply the source and approval chain.

### Import Kubernetes deployment evidence

For a Deployment-managed Pod, the authorized collector can also capture its controlling ReplicaSet from the same cluster and namespace. Pass the Deployment name and UID from the approved service mapping, and record the two capture times separately. The Pod and ReplicaSet template should both carry the deployment and configuration annotations above and `craft.factory/source-repository`. [Kubernetes ReplicaSets](https://kubernetes.io/docs/concepts/workloads/controllers/replicaset/) [Kubernetes object metadata](https://kubernetes.io/docs/reference/kubernetes-api/definitions/object-meta-v1-meta/)

```powershell
craft-factory import-kubernetes-deployment release-evidence.json --pod-snapshot pod.json --replica-set-snapshot replica-set.json --cluster staging-one --namespace sample-stage --pod api-84fd4-dz --pod-uid $podUid --container api --deployment-name api --deployment-uid $deploymentUid --repository bitbucket:example/sample-api --service sample-api --environment staging --region au-east --resource sample-api-blue --owner 'Sample API team' --observed-at $podCapturedAt --replica-set-observed-at $replicaSetCapturedAt > enriched-release-evidence.json
```

Craft checks the controller UIDs and matches the template hash, repository, configuration, and image digest before emitting both records. It uses the Pod creation time as the deployment time for that specific resource. The result describes the observed Pod and its ReplicaSet template. Use rollout evidence when deciding whether every target received the release.

### Capture the approved Kubernetes resources

Use `craft-factory schema kubernetes-capture-policy` to prepare the collector policy. Pin the kubeconfig bytes, context, namespace, Pod name and UID, named ReplicaSet, expected Deployment, and release target. Use a trusted `kubectl` executable and a kubeconfig for a collector identity with read access to those resources. [kubectl get reference](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_get/)

```powershell
craft-factory collect-kubernetes-release capture-policy.json --policy-digest $approvedPolicyDigest --kubectl $kubectlPath --kubeconfig $approvedKubeconfig --destination release-capture
```

Craft reads the named Pod, reads its approved ReplicaSet, then checks the Pod again for changes. On success, it writes both original snapshots and `sources.json` in the new directory. Keep the archive and its manifest digest for assembly in the restricted evidence store. A direct trace of the Kubernetes archive reports a missing build link.

### Capture and import signed build provenance

Use a DSSE envelope containing an in-toto Statement with a SLSA v1 build provenance predicate. Create the policy with `craft-factory schema build-provenance-policy`. Pin the builder ID, build type, expected external parameters, exact source dependency URI, artifact subject name, and Base64-encoded raw Ed25519 public key. Use the repository identifier that appears in the deployment and change records. Obtain the policy hash from the approving owner through a separate trusted channel. [in-toto envelope specification](https://github.com/in-toto/attestation/blob/main/spec/v1/envelope.md) [SLSA build provenance](https://slsa.dev/spec/v1.2/build-provenance)

The protected release job can publish the envelope after building the artifact. Write `build.json` using `craft-factory schema build-provenance-input`. Take the artifact digest and full checkout commit from that job's build output and use the CI run ID and RFC 3339 finish time. Use `craft-factory schema build-archive-policy` to pin the archive's HTTPS path prefix, signing policy digest, and optional private CA digest. Mount the Ed25519 PKCS#8 DER key into the protected job and inject its archive credential as `CRAFT_FACTORY_BUILD_PUBLISH_TOKEN`. Keep the key and credential outside agent and pull request jobs.

```bash
craft-factory publish-build-provenance build.json \
  --archive-policy build-archive-policy.json --archive-policy-digest "$APPROVED_ARCHIVE_DIGEST" \
  --signing-policy build-policy.json --signing-key "$BUILD_SIGNING_KEY_FILE" > build-publication.json
```

The archive credential needs conditional object creation and read-back within the approved prefix. The archive must honor `If-None-Match: *`. Craft writes to a path named by the envelope hash. A replay succeeds only when the read-back matches the signed original. Pass the receipt's `capture_policy` through the approving evidence channel and pin the exact serialized file bytes for collection. [HTTP conditional requests](https://www.rfc-editor.org/rfc/rfc9110.html#name-if-none-match)

Have the trusted builder publish the signed envelope to an immutable object in the evidence archive. Use `craft-factory schema build-capture-policy` to pin that object's direct HTTPS URL and file digest, the approved signing policy digest, the observed artifact digest, and the expected full source commit. If the archive requires authentication, inject an archive-scoped read-only bearer token as `CRAFT_FACTORY_BUILD_EVIDENCE_TOKEN`. The collector requires a direct HTTP 200 response over TLS and refuses redirects or URLs with query strings.

For an archive with a private CA, put the CA certificate file digest in `archive_ca_digest` and pass the matching PEM file using `--archive-ca`.

```powershell
craft-factory collect-build-provenance capture-build.json --policy-digest $approvedCaptureDigest --signing-policy build-policy.json --destination .\build-evidence
```

The collector verifies the signature and release binding before writing the original envelope and signing policy. Keep `build-source.json` and the receipt's `source_digest` for assembly. Use the import command to apply a previously collected envelope to an existing evidence bundle.

```powershell
craft-factory import-build-provenance release-evidence.json --attestation build.dsse.json --policy build-policy.json --policy-digest $approvedPolicyDigest --artifact-digest $observedArtifactDigest --observed-at $capturedAt > verified-build-evidence.json
```

The importer verifies the signature before reading the statement. It matches the approved subject to the runtime artifact digest and the approved source URI to a full Git commit. The resulting build record keeps the invocation ID and hashes of the envelope and policy. Keep the original files for re-verification. The importer adds an absent build record and accepts an exact replay. Conflicting build evidence requires reconciliation before import. [DSSE protocol](https://github.com/dsse-lf/dsse/blob/master/protocol.md)

This profile uses a pinned signing key. The normalized bundle is ordinary JSON and should stay in the same trusted evidence store as the deployment and approval exports.

### Capture and import a merged Bitbucket change

Prepare a change record using `craft-factory schema bitbucket-change-approval`. Bind the PR number, destination repository UUID and branch, reviewed source head, exact merge result, and approved work item and intent revision. Record the approval time and use the merge event's timestamp and reference for the merge fields. The PR's `updated_on` field can include later activity and serves only as a timing check.

Supply the approving channel's digest and inject a read-only credential into the collector process as `CRAFT_FACTORY_BITBUCKET_EVIDENCE_TOKEN`. For an Atlassian user API token with `read:pullrequest:bitbucket`, also supply `CRAFT_FACTORY_BITBUCKET_EVIDENCE_EMAIL`. For a Bitbucket bearer access token, leave the email unset. The collector makes one GET to the PR in the pinned record and verifies the response before archiving it. On Windows, put the destination under a restricted parent directory so it inherits the parent's permissions. [Bitbucket Cloud pull request API](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-pullrequests/#api-repositories-workspace-repo-slug-pullrequests-pull-request-id-get)

```powershell
craft-factory collect-bitbucket-change change-approval.json --approval-digest $approvedDigest --destination .\bitbucket-change
```

The receipt includes the digest of `change-source.json` for assembly. For an existing evidence bundle and separately archived PR response, use the import command.

```powershell
craft-factory import-bitbucket-change release-evidence.json --pull-request-snapshot pull-request.json --approval change-approval.json --approval-digest $approvedDigest --observed-at $capturedAt > change-linked-release-evidence.json
```

Get the approval digest through the approved evidence channel. Craft checks the archived PR against that record and preserves the two input hashes. The release trace joins the build commit to the merge result, while retaining the reviewed source head for audit. Use the archived provider response and approval record from the trusted collector. A missing native merge commit leaves that link unresolved.

### Capture a revision-bound Bitbucket approval

The human approver can record the release decision on the PR before the merge. Put the marker on the first line of a top-level comment followed by one JSON object. Use the full reviewed commit ID and the same work item, intent reference, and intent revision as the approved release scope.

```text
Craft release approval v1
{"decision":"approve","source_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","work_item":"OPS-42","intent_reference":"https://jira.example/browse/OPS-42","intent_revision":"r3"}
```

Use `craft-factory schema bitbucket-review-policy` to pin the PR and comment IDs, authorized approver UUID, intent, reviewed commit, destination repository and branch, and exact merge result. Omit `merged_at` and `merge_reference` to use native activity. The collector reads up to eight pages for the same PR and requires a single merge event with the exact full source commit and destination. For an independently trusted merge record, supply both merge fields. The collector uses the same read-only Bitbucket credential as `collect-bitbucket-change`. [Bitbucket PR activity API](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-pullrequests/#api-repositories-workspace-repo-slug-pullrequests-pull-request-id-activity-get) [Bitbucket comment API](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-pullrequests/#api-repositories-workspace-repo-slug-pullrequests-pull-request-id-comments-comment-id-get)

```powershell
craft-factory collect-bitbucket-review review-policy.json --policy-digest $approvedPolicyDigest --destination .\bitbucket-review
```

Craft checks the comment author's UUID against the approved policy and excludes the PR author. The latest comment text must predate the merge. Deleted comments and mismatched commits or intent revisions fail verification. Keep the generated `change-source.json` and source digest for assembly. The trace replays the original comment, policy, and any native activity pages to reproduce the archived approval. Repository merge checks still govern permission to merge.

### Capture a merged GitHub change

Use `craft-factory schema github-change-approval` to prepare an owner-approved record. Include the lowercase repository slug, GitHub repository ID, destination branch, PR number, reviewed head, exact merge result, and approved work item and intent revision. Record the approval and merge times in Unix seconds. This collector supports PRs whose source and destination belong to the approved repository.

```powershell
craft-factory collect-github-change github-approval.json --approval-digest $approvedDigest --gh $gh --destination .\github-change
```

Use an absolute path to a GitHub CLI executable with read access to the PR. Supply the approval digest from the approving evidence channel. Craft verifies the native merge details against the approval and saves the original response. Use the generated `change-source.json` and its digest for assembly. The build policy and deployment evidence must identify the same repository as `github:owner/repository`. [GitHub pull request API](https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request)

### Assemble and verify the release archive

Keep the Kubernetes, build, and change folders under a common evidence directory. Supply the digests from the three collector receipts. Craft resolves each fragment's paths from its original folder and verifies the entire chain before creating the new manifest.

```powershell
craft-factory assemble-release-sources `
  --kubernetes .\release\runtime\sources.json --kubernetes-digest $kubernetesDigest `
  --build .\release\build\build-source.json --build-digest $buildDigest `
  --change .\release\change\change-source.json --change-digest $changeDigest `
  --destination .\release\sources.json
```

Omit `--as-of` to use the latest collector observation time. To trace at a specific time, pass that time in UTC Unix seconds. The Kubernetes freshness limit applies in both cases. If a link is missing or stale, the command returns a nonzero status and a trace gap without creating the manifest. Take the resulting manifest digest through the approving evidence channel before relying on it for shared diagnosis or rollout verification.

### Trace directly from archived sources

Use `craft-factory schema release-source-manifest` to describe the archive. The manifest pins the Kubernetes snapshots and scope, the build envelope and approved signing policy, and the forge snapshot and change approval. Use paths relative to the manifest and `sha256:` digests of the original file bytes. Take the manifest digest from the approving evidence channel.

```powershell
craft-factory trace-release-sources sources.json --manifest-digest $approvedManifestDigest --require-complete
```

Craft reads and verifies the original files in one invocation and emits a report with the manifest digest and release trace. Omit the `build` or `change` section when the source is unavailable. The trace reports the corresponding gap. `--require-complete` returns a nonzero status for a gap while preserving the report. A changed input file or invalid source record fails before a report is issued.

## Diagnose an observed release

Use an operator-approved export for one service, environment, region, and resource. Export structured log event codes and severity, and numeric metric samples with names and units. Include the observed deployment ID, configuration revision, and artifact digest on each sample. Use the source system to redact raw messages, labels, and other sensitive fields before exporting. [Example telemetry export](examples/diagnostic-telemetry.json)

Use `craft-factory schema diagnostic-policy` to see the policy fields. Set the target, approval reference, approved time window, and the SHA-256 digests of both files. Set the maximum query window and record count, and allowlist specific log codes and metric name and unit pairs. Use lowercase hex with a `sha256:` prefix for each digest. The schema for the export is available through `craft-factory schema diagnostic-telemetry`.

```powershell
craft-factory diagnose-release diagnostics-policy.json --release release-evidence.json --telemetry diagnostic-telemetry.json --from 1795238300 --to 1795238400
```

The command returns the release trace and samples for the verified deployment within the requested window. Samples from an earlier rollout are excluded even when it used the same artifact. A gap in inventory or deployment evidence produces `telemetry_correlated: false` and empty sample lists. A later gap, such as missing PR evidence, remains visible alongside any telemetry that was bound to the deployment. An unapproved target, digest, field, metric, time window, or output size fails before a report is emitted.

The policy and exported files must come from the approving operator. This command reads local files and emits JSON to stdout. Source-system permissions, export approval, and control of the output destination remain with the operator.

### Diagnose from original release evidence

To diagnose against a release source manifest, set the diagnostic policy's `release_basis` to `source_manifest` and put the manifest's hash in `release_digest`. Pin the telemetry export as usual. Take the diagnostic policy digest from the approving evidence channel. Set the manifest's `as_of` to the requested window end.

```powershell
craft-factory diagnose-release-sources diagnostic-policy.json --policy-digest $approvedPolicyDigest --manifest sources.json --telemetry telemetry.json --from 1795238130 --to 1795238400
```

Craft verifies the source archive and uses the stricter freshness limit from the manifest or policy. The trace target and time must match the diagnostic scope. When the runtime evidence includes a container start time, telemetry from before that start is excluded. Missing build or change evidence stays visible without suppressing telemetry already bound to a verified deployment.

### Prepare diagnostic telemetry from OTLP exports

Use `craft-factory schema diagnostic-otlp-policy` to pin the logs and metrics archives, exact release and resource, service instance, instrumentation scopes, allowed event codes and metric units, and observation window. Use the OTLP/HTTP JSON request format with `resourceLogs` and `resourceMetrics` arrays. An empty signal can use an empty array. [OTLP JSON specification](https://opentelemetry.io/docs/specs/otlp/)

The resource must supply string attributes for `service.name`, `service.instance.id`, `deployment.environment.name`, `cloud.region`, `craft.resource.id`, `craft.deployment.id`, `craft.config.revision`, and `craft.artifact.digest`. The collector supplies the Craft attributes for the approved target and release.

For original Kubernetes source evidence with raw OTLP, set the import policy's `kubernetes_container` fields `pod_uid`, `container_name`, and `container_id`. The collector must attach the corresponding string attributes `k8s.pod.uid`, `k8s.container.name`, and `container.id` to both signals. Use the full runtime ID from the selected Pod status entry after removing its runtime prefix, such as `containerd://`. The service compares all three fields with the pinned Pod snapshot. [Kubernetes Pod API](https://kubernetes.io/docs/reference/generated/kubernetes-api/v1.34/#containerstatus-v1-core) · [OpenTelemetry Kubernetes attributes](https://opentelemetry.io/docs/specs/semconv/registry/attributes/k8s/) · [OpenTelemetry container attributes](https://opentelemetry.io/docs/specs/semconv/registry/attributes/container/)

```powershell
craft-factory import-diagnostic-otlp otlp-policy.json --policy-digest $approvedPolicyDigest --logs otlp-logs.json --metrics otlp-metrics.json > telemetry.json
```

Craft keeps allowlisted event names with numeric severity and approved unlabeled gauge samples. It omits raw log bodies, trace identifiers, labels, and exemplars. Ambiguous samples and output above the approved record limit fail without producing an export. Pin the resulting file's hash as `telemetry_digest` in the diagnostic policy. [OpenTelemetry logs](https://opentelemetry.io/docs/specs/otel/logs/data-model/) [OpenTelemetry gauges](https://opentelemetry.io/docs/specs/otel/metrics/data-model/#gauge)

## Verify a maintenance rollout

Pin the expected artifact, source commit, PR, work item, intent revision, and approval in the remediation policy. List each required service, environment, region, and resource with its expected configuration revision. Set the maximum age for inventory and deployment observations. The policy accepts up to 32 distinct targets and an observation age of up to one hour. [Example remediation policy](examples/remediation-policy.json)

```powershell
craft-factory schema remediation-policy
craft-factory verify-remediation examples/remediation-policy.json --release examples/release-evidence.json --as-of 1795238400 --require-complete
```

Each target reports whether the approved change is verified, the artifact or configuration is still pending, the source and approval differ, or the evidence has a gap. `rollout_verified` becomes true when every required target has fresh, matching runtime and release evidence. Use `--require-complete` in CI to fail an open rollout while keeping the JSON report available. The report includes hashes of the policy and evidence files. Tracker closure and deployment changes remain separate actions.

To verify a rollout directly from the original archives, use `craft-factory schema remediation-source-index`. Put the index in the common evidence directory and list one assembled manifest per observed target with its path relative to the index and its SHA-256 digest. Use the same approved remediation policy and add its digest and the index digest from the evidence channel.

```powershell
craft-factory verify-remediation-sources rollout-policy.json --policy-digest $approvedPolicyDigest --sources rollout-sources.json --sources-digest $approvedIndexDigest --as-of $rolloutCutoff --require-complete
```

The verifier replays each archive at the common cutoff and applies the tighter freshness limit from the source manifest or rollout policy. An absent target is reported as missing, and duplicate or unapproved targets cause an error. The report's `evidence_digest` identifies the index, which pins the original release manifests.

Use `prepare-rollout-archive` to create the index and get a catalog entry for the shared queue. Put the approved policy, assembled manifests and their source files under the directory where the queue catalog will live. Pass the reviewed PR head from the queue and repeat `--release` with each manifest path and its digest. Use a current observation cutoff in UTC Unix seconds.

```powershell
craft-factory prepare-rollout-archive .\release\rollout-policy.json --policy-digest $approvedPolicyDigest `
  --work-id $workId --reviewed-head $candidateSha --as-of $rolloutCutoff `
  --release .\release\sources.json $manifestDigest --destination .\release\rollout-sources.json
```

Craft writes a new index only if every required target verifies and each archive identifies the exact reviewed head. Add the returned `catalog_entry` to the operator-owned catalog after review. The queue still checks the ledger and replays the archived evidence using its own clock.

## Prepare a dependency update for review

Start with one direct dependency update from an approved Renovate identity. Record the repository, PR reference, stable bot identity, exact base and head, package name, and old and new resolved versions in `proposal`. The supported intake uses a root npm project and a version 3 lockfile. The original updater history must touch only `package.json` and `package-lock.json`. The reviewed work-order template may also allow the application and test paths that need compatibility work. [Renovate use cases](https://docs.renovatebot.com/getting-started/use-cases/) · [npm lockfile fields](https://docs.npmjs.com/files/package-lock.json/)

Have the approved scanner identity run OSV-Scanner against each exact revision's committed lockfile. Use full-package JSON output. For each scan, preserve the issuer, successful completion, evidence reference, commit, and SHA-256 digests of the scanned lockfile and JSON report. Put these in `baseline_scan` and `candidate_scan`. The command checks package coverage, including transitive packages. New findings or findings for the proposed package return `review_required`. [OSV-Scanner JSON output](https://google.github.io/osv-scanner/output/) · [Full-package output](https://google.github.io/osv-scanner/usage/#show-all-packages)

```powershell
craft-factory schema dependency-update-input
craft-factory prepare-dependency-update update-input.json --baseline-osv baseline-osv.json --candidate-osv candidate-osv.json
```

The input also contains `schema_version`, the approved `repository`, `updater_actor` and `scanner_issuer`, and the `work_order` template. Obtain the updater observation and scanner receipts from trusted forge and CI records. The launcher checks their bindings against the local Git objects and report files.

For signed scans, set `scanner_public_key_base64` in the approved intake to the Base64 encoding of the scanner's raw 32-byte Ed25519 public key. Configure the protected scanner job to use an approved scanner version and configuration, emit JSON with `--all-packages`, and supply the exact lockfile, repository, commit, and run reference from the job. Only seal reports after the scanner has completed successfully, including completed scans that found advisories. Give only the protected job access to the PKCS#8 DER private key. [OSV-Scanner output options](https://google.github.io/osv-scanner/usage/#show-all-packages)

```powershell
craft-factory seal-dependency-scan scan.json --lockfile package-lock.json --repository $repository --commit $checkoutSha --issuer $issuer --reference $runReference --completed-at $scanCompletedAt --public-key-base64 $scannerPublicKey --signing-key $protectedKeyFile
```

Put the resulting JSON envelopes in `baseline_scan` and `candidate_scan`. Still pass the original, byte-for-byte OSV JSON files through `--baseline-osv` and `--candidate-osv`. Review the scanner key before approving the intake digest. Craft verifies both signatures, the repository and commit bindings, and full package coverage against the committed lockfiles. The preparation and compatibility reports show the scanner signing-key digest. Existing intakes without a scanner signing key use the original receipt format.

Set `scanner_max_age_seconds` in the signed intake to require recent scans during preparation and compatibility verification. Use a value from 1 to 604800. Pass each scanner job's actual completion time in UTC Unix seconds to `--completed-at`. Craft rejects missing, future, or expired timestamps under that policy. The reports show the earlier deadline as `scanner_fresh_through`.

### Refresh scans for an existing candidate

If the scans expire before the first rollout, rerun both scans for the same baseline and original updater commits. Seal them with the key pinned in the original intake. Put the two new envelopes in `baseline_scan` and `candidate_scan` in a JSON object with `schema_version: 1`. Use `craft-factory schema dependency-scan-refresh` for its format.

```powershell
craft-factory prepare-dependency-update update-input.json --refresh-scans refreshed-scans.json --baseline-osv fresh-baseline-osv.json --candidate-osv fresh-candidate-osv.json
```

Keep the original intake file and work order. The preparation result supplies `scanner_refresh_digest` and the new `scanner_fresh_through`. Check the new scan findings, then approve both values in the existing candidate's evidence pins using a new approval ID and `previous_approval_id` if there is already an approval. Migration 053 requires a later deadline when the refresh changes. The verifier uses the same refresh file and new OSV reports with `attest-shared-dependency-compatibility`. After a passing verdict, use `shared-series approve-rollout` for the new verification sequence. An earlier rollout approval is tied to its own sequence.

For `ready_for_supervised_verification`, save the returned `work_order` as JSON. For shared work with a deployment target, register the order in a single-member release series before delegating it. Submit `proposal.head` as the initial candidate, bind the existing Renovate PR, and collect native checks and independent compatibility evidence. Use the normal request-changes path for follow-up code or test work. For local verification of the running rollout, use `verify-remediation`. [Shared release setup](shared/README.md#sequence-a-cross-repository-release)

### Verify baseline and candidate behavior

Have the reviewer approve a compatibility policy for the exact `input_digest` from intake, final candidate SHA, trusted CI issuer, harness digest, and named checks. A `preserve` check must pass on both revisions. A `regression` check must record an assertion failure on the baseline and pass on the candidate. A skipped check, execution error, or missing result leaves the gate open.

Run the same approved harness against the baseline and candidate in separate CI jobs. Save a receipt for each job with its unique reference, repository, commit, issuer, harness and lockfile digests, and per-check outcomes. Mark the run complete when the harness reports every required result, including an expected baseline assertion failure. Use trusted CI artifacts for these receipts.

Set `run_public_key_base64` in the compatibility policy to require signed receipts. Use the Base64 encoding of the raw 32-byte Ed25519 public key. Run the signing step in a protected job that obtains repository and checkout SHA from the CI platform and accepts results only from the approved harness. Give only that job access to the corresponding PKCS#8 DER private key and isolate it from candidate code, PR jobs, and agents. Run this once for each receipt. [DSSE protocol](https://github.com/dsse-lf/dsse/blob/master/protocol.md)

```powershell
craft-factory seal-dependency-compatibility-run run.json --policy compatibility-policy.json --policy-digest $policyDigest --repository $repository --commit $checkoutSha --signing-key $protectedKeyFile --destination run.dsse.json
```

For a signing policy, pass the two envelopes through `--baseline-run` and `--candidate-run`, and use the envelope digests when approving shared evidence. The report includes the signing-key digest. Existing policies without a signing key accept the original receipt format.

```powershell
craft-factory schema dependency-compatibility-policy
craft-factory schema dependency-compatibility-run
craft-factory verify-dependency-compatibility update-input.json --baseline-osv baseline-osv.json --candidate-osv candidate-osv.json --policy compatibility-policy.json --baseline-run baseline-run.json --candidate-run candidate-run.json --require-complete
```

For a shared queue, admit the work order with its `dependency_intake_digest` intact. An operator registers the approved intake, policy, and CI receipt digests for the exact candidate. Save the approval JSON with an `id`, `work_id`, `generation`, `head`, `evidence` reference, and a `pins` object containing `intake_digest`, `policy_digest`, `baseline_run_digest`, and `candidate_run_digest`.

If the intake reports `scanner_fresh_through`, include that exact Unix timestamp in `pins`. The verifier recomputes it from the signed receipts. With migration `050_dependency_scan_release_freshness.sql`, the queue uses its database clock to enforce the deadline at attestation and before the first rollout approval and recording. A previously observed rollout keeps its own runtime evidence expiry and refresh rules.

```powershell
craft-factory shared-ops --queue $queue --ca queue-ca.pem approve-dependency-evidence dependency-evidence-approval.json
craft-factory attest-shared-dependency-compatibility update-input.json --queue $queue --ca queue-ca.pem --handoff $handoffFile --approval-id $approvalId --report compatibility-report.json --baseline-osv baseline-osv.json --candidate-osv candidate-osv.json --policy compatibility-policy.json --policy-digest $policyDigest --baseline-run baseline-run.json --baseline-run-digest $baselineDigest --candidate-run candidate-run.json --candidate-run-digest $candidateDigest
```

The independent verifier uses a handoff from `prepare-shared-verification --verifier`. The attestation replays the scanner and CI evidence against that checkout. The work audit retains the approval and verification digests. Keep the input files and generated report in the approved evidence store.

To approve replacement evidence for the same candidate, use a new approval ID and set `previous_approval_id` to the current one. The queue invalidates the previous passing verdict immediately. Prepare a fresh handoff before attesting the replacement. Upgrading the queue also returns any pending dependency candidate with a preexisting passing verdict to verification.

The gate rechecks the original intake and both scanner reports. A follow-up candidate must descend from the updater commit, stay within the delegated paths, and retain the scanned manifest and lockfile bytes. Use `--require-complete` to fail CI if any approved expectation remains open. The report records the exact commits, approvals, and receipt digests for review.

See the [factory delivery plan](../docs/plans/factory-delivery-plan.md) for technical verification, protected CI setup, and future rollout criteria.

For a local second-developer rehearsal, use the [SCRUM-22 handoff instructions](pilot-handoff.md) with the pre-repair bundle. The [pilot scorecard](examples/pilot-scorecard.csv) preserves the observed factory run and leaves unavailable measurements blank.

From the kit directory, run `python tools/summarize_scorecard.py examples/pilot-scorecard.csv` for a brief comparison. Add `--json` for outcome, attempt, token, human-time, and cost breakdowns by task class. Use one row per work item and comparison path. Use `accepted`, `verified draft`, `rejected`, `abandoned`, or `in progress` for the outcome, with `human` or `simulated` as the review kind. An accepted change needs human review and an evidence reference. Record the metered cost of all attempts for each case, including failed and abandoned work. Leave incomplete totals blank. Metered cost per accepted change is shown only when every case has a recorded cost in the same currency.
