# SCRUM-22 pilot handoff

Use this handoff to exercise the Windows install and manual candidate intake with a second developer and an approved interactive tool. Work in the extracted directory with a fresh SQLite database. The bundle ends at the verified initial PR head and excludes the later repair commit. Keep the replay local.

## Install and start from the reviewed revision

Extract the Windows kit and the handoff archive. From the handoff directory, point `$factory` to the extracted launcher. Run these commands in PowerShell with Git on PATH.

```powershell
$handoffRoot = (Get-Location).Path
$factory = 'C:\path\to\craft-factory.exe'
$repo = Join-Path $handoffRoot 'replay-repo'
$db = Join-Path $handoffRoot 'replay.db'
$replay = Get-Content -LiteralPath '.\handoff.json' -Raw | ConvertFrom-Json
$bundle = Join-Path $handoffRoot 'pre-repair.bundle'

& $factory smoke --destination (Join-Path $handoffRoot 'offline-smoke')
git clone --branch main $bundle $repo
git -C $repo fetch --no-tags $bundle "$($replay.source_ref):refs/heads/handoff-start"
git -C $repo switch handoff-start
if ((git -C $repo rev-parse HEAD) -ne $replay.reviewed_head) { throw 'Unexpected handoff head' }

$order = Get-Content -LiteralPath '.\work-order.template.json' -Raw | ConvertFrom-Json
$order.repository = $repo
[IO.File]::WriteAllText((Join-Path $handoffRoot 'work-order.local.json'), ($order | ConvertTo-Json -Depth 10))
& $factory --state $db delegate '.\work-order.local.json' --jira-snapshot '.\jira-scrum-22.snapshot.json'
& $factory --state $db submit SCRUM-22 --commit $replay.reviewed_head --note 'Imported handoff candidate'
& $factory --state $db bind-draft SCRUM-22 --reference $replay.pr --head $replay.reviewed_head
& $factory --state $db attest-checks SCRUM-22 '.\initial-verification.json'
& $factory --state $db request-changes SCRUM-22 '.\simulated-feedback.json'
```

## Produce and verify a manual repair

In your approved interactive tool, open `replay-repo`. Load `skills/verify-candidate/SKILL.md` from the pinned revision in the order, then use the work order and simulated feedback as the handoff. Keep the edit within `display/greeting.py`. Commit the edit on the current branch and return its full SHA through the launcher.

```powershell
$repair = git -C $repo rev-parse HEAD
& $factory --state $db submit SCRUM-22 --commit $repair --replaces $replay.reviewed_head --note 'Imported handoff repair'
if ($LASTEXITCODE -ne 0) { throw 'Repair submission failed' }
$verified = & $factory --state $db prepare-verification SCRUM-22 --head $repair --destination (Join-Path $handoffRoot 'independent-checkout') | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $verified.head -ne $repair) { throw 'Could not prepare the submitted revision' }
Push-Location $verified.checkout
try {
    python -B -m unittest discover -s tests
    if ($LASTEXITCODE -ne 0) { throw 'Baseline test failed' }
    python -B (Join-Path $handoffRoot 'verify_repair.py')
    if ($LASTEXITCODE -ne 0) { throw 'Independent acceptance test failed' }
} finally { Pop-Location }
& $factory --state $db status SCRUM-22
```

Record the developer, tool and version, install result, repair SHA, launcher result, and both test outcomes. Record active setup, coding, and review minutes separately from elapsed time. The replay exercises handoff and local verification. Use the live draft for any independent approval decision.
