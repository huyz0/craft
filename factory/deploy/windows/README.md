# Run Craft Factory with Windows Task Scheduler

## Schedule one CI repair lane on Windows

Use a dedicated Windows account with the approved forge read credential, Git checkout, and Codex home. Set `$launcher` and `$gh` to absolute executable paths and `$state`, `$order`, `$policy`, `$attemptRoot`, `$codexExe`, `$codexHome`, and `$report` to absolute paths. Use an existing private directory outside the checkout for `$workRoot`. Set `$orderDigest` and `$policyDigest` from the approval record, including the `sha256:` prefix.

```powershell
$arguments = '--state "{0}" run-ci-cycle "{1}" --order-digest "{2}" --github-policy "{3}" --policy-digest "{4}" --gh "{5}" --attempt-root "{6}" --codex "{7}" --codex-home "{8}" --report-file "{9}"' -f $state,$order,$orderDigest,$policy,$policyDigest,$gh,$attemptRoot,$codexExe,$codexHome,$report
$account = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$action = New-ScheduledTaskAction -Execute $launcher -Argument $arguments -WorkingDirectory $workRoot
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 5)
$principal = New-ScheduledTaskPrincipal -UserId $account -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Minutes 20)
Register-ScheduledTask -TaskName 'Craft Factory CI repair' -Action $action -Trigger $trigger -Principal $principal -Settings $settings
```

For Bitbucket, substitute `--bitbucket-policy` and `--step-name` for the GitHub policy and `--gh` arguments. Add `--jira-snapshot` when required. Inspect `$report` and `Get-ScheduledTaskInfo -TaskName 'Craft Factory CI repair'` after the first shadow run. Once the CI owner approves unattended repair, add `--admit --daily-limit 1` to the scheduled action. Use the same database with `set-ci-intake pause` before maintenance. [Task Scheduler settings](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtasksettingsset)

Add `--follow-branch` for a recurring lane approved to follow branch advances. Keep the checkout synchronized through your normal repository sync so the scheduled action can verify that the observed tip descends from the approved work order base.

## Schedule shared CI intake on Windows

Follow [shared CI lane approval](../../shared/README.md#approve-recurring-shared-ci-intake). Use the observer's Windows account with its existing read-only forge login. Set the absolute paths to the launcher, approved manifest, CA and protected observer token. Use an existing private directory for `$workRoot`. Set `$laneDigest` to the approved manifest digest.

```powershell
$arguments = 'shared-work --queue "{0}" --ca "{1}" poll-ci-lane "{2}" --digest "{3}" --intake-token-file "{4}"' -f $queue,$ca,$manifest,$laneDigest,$observerToken
$account = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$action = New-ScheduledTaskAction -Execute $launcher -Argument $arguments -WorkingDirectory $workRoot
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 5)
$principal = New-ScheduledTaskPrincipal -UserId $account -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Minutes 5)
Register-ScheduledTask -TaskName 'Craft Factory shared CI intake' -Action $action -Trigger $trigger -Principal $principal -Settings $settings
```

Run the poll command interactively first to inspect the JSON decision. After operator approval of an admission-enabled manifest, update the task with the new digest and an explicit `--admit` flag. The interactive task principal requires the observer account to be logged in. Disable the lane in the queue before disabling the task.

## Run a shared runner on Windows

Use the same Windows account for the scheduled task, Git checkouts, and the dedicated Codex home. Create a private runner directory outside every mapped checkout, with an existing `attempts` subdirectory. Put the checkout map, queue CA, and owner-only runner token file in the runner directory. [Enroll a runner and prepare its checkout map](../../shared/README.md#attach-a-launcher)

```powershell
$runnerRoot = Join-Path $env:LOCALAPPDATA 'Craft Factory\Shared runner'
$launcher = (Resolve-Path '.\craft-factory.exe').Path
$queue = 'https://queue.example.com'
$ca = Join-Path $runnerRoot 'queue-ca.pem'
$map = Join-Path $runnerRoot 'repository-map.json'
$tokenPath = Join-Path $runnerRoot 'runner-token'
$attemptRoot = Join-Path $runnerRoot 'attempts'
$codexExe = (Get-Command codex.exe).Source
$codexHome = Join-Path $runnerRoot 'codex-home'

& $launcher check-shared-runner --queue $queue --ca $ca --runner-token-file $tokenPath --repository-map $map --attempt-root $attemptRoot --codex $codexExe --codex-home $codexHome
```

After the setup check passes, register the loop for the current user's logon. Task Scheduler limits the task to one instance, and the launcher also locks the attempt directory. The task runs while the user is signed in. [Logon triggers](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtasktrigger) · [Task settings](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtasksettingsset)

```powershell
$account = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$taskName = 'Craft Factory shared runner'
$arguments = 'run-shared-loop --queue "{0}" --ca "{1}" --runner-token-file "{2}" --repository-map "{3}" --attempt-root "{4}" --codex "{5}" --codex-home "{6}" --write-status' -f $queue,$ca,$tokenPath,$map,$attemptRoot,$codexExe,$codexHome
$action = New-ScheduledTaskAction -Execute $launcher -Argument $arguments -WorkingDirectory $runnerRoot
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $account
$principal = New-ScheduledTaskPrincipal -UserId $account -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Seconds 0) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings
Start-ScheduledTask -TaskName $taskName
```

Check both the scheduled task and the runner's most recent report.

```powershell
Get-ScheduledTask -TaskName $taskName
Get-ScheduledTaskInfo -TaskName $taskName
Get-Content -LiteralPath (Join-Path $attemptRoot '.runner-status.json') -Raw | ConvertFrom-Json
```

The report updates after each completed poll. Compare its timestamps with the supervisor's last run time if the task exited unexpectedly or was killed. A long attempt can leave the previous poll visible until the attempt finishes. An operator can also inspect [claim polls for an enrolled order](../../shared/README.md#diagnose-a-waiting-work-order).

To stop after the current attempt, create `$attemptRoot\.stop` and wait for the task to leave the running state. Remove the marker before restarting. Retain the same attempt directory so the loop can resume pending submissions.

## Schedule the Jira relay on Windows

Run these commands in PowerShell as the Windows user who owns the approved Craft Jira profile. The task uses that user's interactive session and Credential Manager. It runs the launcher directly and keeps its last completed JSON report in the relay directory.

Set `$launcher` to the absolute path of the extracted `craft-factory.exe`. Prepare `%LOCALAPPDATA%\Craft Factory\Jira relay` as a private directory. Keep `jira-watch.json`, the queue CA PEM, and an owner-only `intake-token` there. The watch policy uses the absolute Craft executable and state paths for the saved read-only Jira profile. [Prepare Jira watches and queue credentials](../../shared/README.md#relay-watched-jira-tickets)

```powershell
$relayRoot = Join-Path $env:LOCALAPPDATA 'Craft Factory\Jira relay'
$launcher = (Resolve-Path '.\craft-factory.exe').Path
$queue = 'https://queue.example.com'
$policy = Join-Path $relayRoot 'jira-watch.json'
$ca = Join-Path $relayRoot 'queue-ca.pem'
$tokenPath = Join-Path $relayRoot 'intake-token'
$cursor = Join-Path $relayRoot 'relay.db'
$report = Join-Path $relayRoot 'last-report.json'
```

Run a read-only pass first. Check the report contains the expected watches or discovery suggestions.

```powershell
& $launcher poll-standing-jira $policy --budget-seconds 105 --report-file $report
Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
```

Register the task when the approved policy is ready for queue submission. The arguments contain the intake token's file path. The trigger runs once a minute and skips an overlapping invocation. [Task Scheduler repetition pattern](https://learn.microsoft.com/en-us/windows/win32/taskschd/repetitionpattern) · [Task Scheduler settings](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtasksettingsset)

```powershell
$arguments = 'poll-standing-jira "{0}" --queue "{1}" --ca "{2}" --intake-token-file "{3}" --report-file "{4}" --budget-seconds 105 --cursor-db "{5}"' -f $policy,$queue,$ca,$tokenPath,$report,$cursor
$action = New-ScheduledTaskAction -Execute $launcher -Argument $arguments -WorkingDirectory $relayRoot
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 1)
$principal = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Minutes 2) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName 'Craft Factory Jira relay' -Action $action -Trigger $trigger -Principal $principal -Settings $settings
```

Check `Get-ScheduledTaskInfo -TaskName 'Craft Factory Jira relay'` and the timestamp of `last-report.json` after the first run. A partial batch keeps its successful observations and reports its per-watch errors. If the task cannot start or is terminated before reporting, the last report stays in place. Use Task Scheduler's result and `LastRunTime` to distinguish a recent report from an earlier one. If the Windows session signs out or the computer sleeps, standing observations eventually expire in the queue.

## Schedule a GitHub issue relay on Windows

Run under the signed-in account that can read the pinned issue through GitHub CLI. Set `$root` to a private working directory. Set `$launcher`, `$ca`, `$watch`, `$order`, and `$intakeToken` to the absolute paths to the installed launcher, queue CA, [approved watch and work order](../../shared/README.md#relay-a-watched-github-issue), and protected one-line queue intake token. Set `$queue` to the queue's HTTPS origin.

```powershell
$taskName = 'Craft Factory GitHub issue relay - routing'
$arguments = 'poll-standing-github-issue "{0}" "{1}" --queue "{2}" --ca "{3}" --intake-token-file "{4}" --report-file last-report.json' -f $watch,$order,$queue,$ca,$intakeToken
$action = New-ScheduledTaskAction -Execute $launcher -Argument $arguments -WorkingDirectory $root
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 1)
$principal = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings
```

Check `Get-ScheduledTaskInfo -TaskName $taskName` and the modification time of `last-report.json` in `$root`. A failed source read or queue request leaves the previous report in place. If the account signs out or the computer sleeps, the last observation eventually expires.

## Schedule a Confluence context check on Windows

Run under the signed-in account that can read the saved Confluence profile. Set `$root` to a private directory outside the Craft state directory. Set `$launcher`, `$ca`, `$policy`, and `$readerToken` to the absolute paths to the installed launcher, queue CA, [Confluence policy](../../shared/README.md#prepare-a-confluence-excerpt), and protected one-line repository-scoped context-reader token file. Set `$queue` to the queue's HTTPS origin.

```powershell
$taskName = 'Craft Factory Confluence context - routing'
$arguments = 'shared-ops --queue "{0}" --ca "{1}" check-confluence-context "{2}" --context-reader --context-reader-token-file "{3}" --require-current --report-file last-report.json' -f $queue,$ca,$policy,$readerToken
$action = New-ScheduledTaskAction -Execute $launcher -Argument $arguments -WorkingDirectory $root
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 5)
$principal = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings
```

Use `Get-ScheduledTaskInfo -TaskName $taskName` and inspect `last-report.json` in `$root` after the first run. A completed drift check saves the mismatch and returns a nonzero result. A connector or queue error leaves the prior report, so compare its modification time with the task's last run.

## Schedule merge verification on Windows

Use a separate private directory and task name for each repository. Run under the verifier's signed-in Windows account. Prepare the absolute paths to the installed launcher, the approved forge policy, the queue CA, and a one-line verifier token file. Restrict the token file and directory to the verifier account and its administrators. The example uses Bitbucket and a second protected file containing its read-only token.

```powershell
$policyDigest = 'sha256:' + (Get-FileHash -LiteralPath $policy -Algorithm SHA256).Hash.ToLowerInvariant()
$arguments = 'shared-ops --queue "{0}" --ca "{1}" check-bitbucket-merges --policy "{2}" --policy-digest {3} --verifier-token-file "{4}" --bitbucket-token-file "{5}" --cursor-db cursor.db --report-file last-report.json --budget-seconds 60' -f $queue,$ca,$policy,$policyDigest,$verifierTokenFile,$bitbucketTokenFile
```

If the Bitbucket token belongs to an Atlassian user, append `--bitbucket-email` and that account's email to the task arguments. For GitHub, configure read-only authentication in the verifier's GitHub CLI session, use `check-github-merges`, and replace `--bitbucket-token-file` and its path with `--gh` and the absolute path to `gh.exe`. Use the GitHub policy file that matches the recorded shadow assessment and calculate its digest the same way.

```powershell
$action = New-ScheduledTaskAction -Execute $launcher -Argument $arguments -WorkingDirectory $mergeRoot
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 2)
$principal = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Seconds 90) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName 'Craft Factory merge verification - payments' -Action $action -Trigger $trigger -Principal $principal -Settings $settings
```

Start the task once and inspect its result and the modification time of `last-report.json` in `$mergeRoot`. The task leaves its credential values in the protected files and keeps its position in `cursor.db`. The interactive task runs while the account is signed in. [Task Scheduler settings](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtasksettingsset) · [Shared merge checks](../../shared/README.md#find-drafts-awaiting-a-merge-check)

## Schedule GitHub readiness and enqueue on Windows

Use separate Windows accounts for the readiness verifier and publisher. Give each an existing private working directory with its own repository map, pinned GitHub policy, queue CA, and empty checkout directory outside the mapped repository. Set `$queue` to the queue's HTTPS origin. Set `$launcher`, `$gh`, `$ca`, `$repositoryMap`, `$policy`, `$checkoutRoot`, `$root`, and the relevant token paths to absolute paths for the signed-in account. Both accounts need read-only authentication in GitHub CLI. Keep the publisher's separate GitHub writer token in a protected file. [Readiness setup](../../shared/README.md#schedule-github-readiness-assessment) · [Publisher setup](../../shared/README.md#schedule-standing-github-enqueue)

As the readiness verifier, use the approved protection fingerprint in `$protectionDigest`.

```powershell
$taskName = 'Craft Factory GitHub readiness - payments'
$arguments = 'shared-ops --queue "{0}" --ca "{1}" poll-github-readiness --repository-map "{2}" --checkout-root "{3}" --policy "{4}" --gh "{5}" --approved-protection-digest "{6}" --verifier-token-file "{7}" --report-file last-report.json' -f $queue,$ca,$repositoryMap,$checkoutRoot,$policy,$gh,$protectionDigest,$verifierToken
```

As the publisher, use the approved standing policy ID in `$policyId`. Register the publisher task when the repository's native enqueue controls have been qualified and standing dispatch is authorized.

```powershell
$taskName = 'Craft Factory GitHub enqueue - payments'
$arguments = 'shared-github-enqueue --queue "{0}" --ca "{1}" --queue-token-file "{2}" poll "{3}" --repository-map "{4}" --checkout-root "{5}" --policy "{6}" --gh "{7}" --github-token-file "{8}" --report-file last-report.json' -f $queue,$ca,$publisherToken,$policyId,$repositoryMap,$checkoutRoot,$policy,$gh,$githubWriterToken
```

Register the corresponding action from each account. Use a distinct task name and working directory for every repository and role.

```powershell
$action = New-ScheduledTaskAction -Execute $launcher -Argument $arguments -WorkingDirectory $root
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 1)
$principal = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Minutes 3) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings
```

Inspect `Get-ScheduledTaskInfo -TaskName $taskName` and the modification time of `last-report.json`. A failed poll leaves the last completed report in place. The queue ledger carries the authoritative send status if GitHub or the local report fails after an attempt. Disable the task to stop polling, revoke the standing policy to fence sends that have yet to begin, and reconcile any unresolved send. [Task Scheduler settings](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtasksettingsset)
