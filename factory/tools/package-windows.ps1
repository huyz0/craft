#Requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $Launcher,
    [Parameter(Mandatory)]
    [string] $Output,
    [string] $QueueService,
    [switch] $FullQualification
)

$ErrorActionPreference = 'Stop'
$launcherPath = (Resolve-Path -LiteralPath $Launcher).ProviderPath
$outputPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Output)
if ([IO.Path]::GetExtension($outputPath) -ne '.zip' -or (Test-Path -LiteralPath $outputPath)) {
    throw 'Output must be a new ZIP file.'
}

$source = Split-Path -Parent $PSScriptRoot
$outputDirectory = (Resolve-Path -LiteralPath (Split-Path -Parent $outputPath)).ProviderPath
$scratch = New-Item -ItemType Directory -Path (Join-Path $outputDirectory "craft-package-$([guid]::NewGuid())")
try {
    $stage = New-Item -ItemType Directory -Path (Join-Path $scratch.FullName 'stage')
    $schemas = New-Item -ItemType Directory -Path (Join-Path $stage.FullName 'schemas')
    $examples = New-Item -ItemType Directory -Path (Join-Path $stage.FullName 'examples')
    $kitTools = New-Item -ItemType Directory -Path (Join-Path $stage.FullName 'tools')
    Copy-Item -LiteralPath $launcherPath -Destination (Join-Path $stage.FullName 'craft-factory.exe')
    foreach ($path in @('QUICKSTART.md', 'README.md', 'pilot-handoff.md', 'skills')) {
        Copy-Item -LiteralPath (Join-Path $source $path) -Destination $stage.FullName -Recurse
    }
    Copy-Item -LiteralPath (Join-Path $source 'examples/pilot-scorecard.csv') -Destination $examples.FullName
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'summarize_scorecard.py') -Destination $kitTools.FullName
    $guidePath = Join-Path $stage.FullName 'README.md'
    $guide = Get-Content -LiteralPath $guidePath -Raw
    $installGuide = @'
## Verify and install

Follow the [Windows kit quickstart](QUICKSTART.md) to verify your installation with a disposable local workflow.

'@
    $guide = $guide -creplace '(?ms)^## Verify and install\r?\n.*?(?=^## Try the supervised workflow)', ($installGuide + "`n")
    $guide = $guide -creplace '(?m)^See the \[factory delivery plan\]\(\.\./docs/plans/factory-delivery-plan\.md\)[^\r\n]*\r?\n(?:\r?\n)?', ''
    if (!$QueueService) {
        $guide = $guide.Replace('[approved execution profile](shared/README.md#pin-a-codex-execution-profile)', 'approved execution profile')
        $guide = $guide.Replace(' [Shared release setup](shared/README.md#sequence-a-cross-repository-release)', '')
        $quickstartPath = Join-Path $stage.FullName 'QUICKSTART.md'
        $quickstart = (Get-Content -LiteralPath $quickstartPath -Raw) -creplace '(?ms)^## Verify the shared queue\r?\n.*?(?=^## Try the workflow)', ''
        [IO.File]::WriteAllText($quickstartPath, $quickstart)
    }
    [IO.File]::WriteAllText($guidePath, $guide)
    foreach ($kind in (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'kit-schemas.txt'))) {
        $schema = & $launcherPath schema $kind
        if ($LASTEXITCODE -ne 0) { throw "Could not generate the $kind schema." }
        $schema | Set-Content -LiteralPath (Join-Path $schemas.FullName "$kind.schema.json") -Encoding utf8
        $example = Join-Path $source "examples/$kind.json"
        if (Test-Path -LiteralPath $example) {
            Copy-Item -LiteralPath $example -Destination $examples.FullName
        }
    }
    if ($QueueService) {
        $shared = New-Item -ItemType Directory -Path (Join-Path $stage.FullName 'shared')
        $deploy = New-Item -ItemType Directory -Path (Join-Path $stage.FullName 'deploy')
        foreach ($tool in @('smoke_shared_queue.py', 'disposable_tls.py')) {
            Copy-Item -LiteralPath (Join-Path $PSScriptRoot $tool) -Destination $kitTools.FullName
        }
        Copy-Item -LiteralPath (Join-Path $source 'deploy/prometheus') -Destination $deploy.FullName -Recurse
        Copy-Item -LiteralPath (Join-Path $source 'deploy/windows') -Destination $deploy.FullName -Recurse
        Copy-Item -LiteralPath (Resolve-Path -LiteralPath $QueueService).ProviderPath -Destination (Join-Path $shared.FullName 'craft-factory-queue.exe')
        Copy-Item -LiteralPath (Join-Path $source 'shared/README.md') -Destination $shared.FullName
        Copy-Item -LiteralPath (Join-Path $source 'shared/migrate_queue.py') -Destination $shared.FullName
        Copy-Item -LiteralPath (Join-Path $source 'shared/service-functions.txt') -Destination $shared.FullName
        Copy-Item -LiteralPath (Join-Path $source 'shared/migrations') -Destination $shared.FullName -Recurse
    }

    $archive = Join-Path $scratch.FullName 'pilot-kit.zip'
    Compress-Archive -Path (Join-Path $stage.FullName '*') -DestinationPath $archive
    $extracted = Join-Path $scratch.FullName 'fresh install'
    Expand-Archive -LiteralPath $archive -DestinationPath $extracted
    $installedLauncher = Join-Path $extracted 'craft-factory.exe'
    $smoke = & $installedLauncher smoke --destination (Join-Path $scratch.FullName 'smoke')
    if ($LASTEXITCODE -ne 0) { throw 'The extracted launcher failed its offline smoke test.' }
    $result = $smoke | ConvertFrom-Json
    if ($result.passed -ne $true -or $result.simulated -ne $true -or $result.state -ne 'closed') {
        throw 'The extracted launcher did not close the simulated repair workflow.'
    }
    if ($FullQualification) {
        $tokenFile = Join-Path $scratch.FullName 'fixture-queue-token'
        $identity = & $installedLauncher issue-queue-identity package-fixture --role metrics-reader --token-file $tokenFile
        if ($LASTEXITCODE -ne 0) { throw 'The extracted launcher could not issue a fixture queue token.' }
        $acl = Get-Acl -LiteralPath $tokenFile
        $rules = $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])
        $token = [IO.File]::ReadAllText($tokenFile).TrimEnd("`n")
        $digest = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::ASCII.GetBytes($token))).ToLowerInvariant()
        $record = $identity | ConvertFrom-Json
        if (!$acl.AreAccessRulesProtected -or $rules.Count -ne 1 -or $rules[0].IdentityReference.Value -ne 'S-1-3-4' -or
            $record.token_sha256 -cne $digest -or $record.role -cne 'metrics_reader' -or ($identity -join '').Contains($token)) {
            throw 'The fixture queue token or its file permissions failed qualification.'
        }
        Remove-Item -LiteralPath $tokenFile
        & python -B (Join-Path $PSScriptRoot 'check_ci_scope.py') validate-docs --kit $extracted
        if ($LASTEXITCODE -ne 0) { throw 'The extracted kit contains broken documentation links.' }
        & python -B (Join-Path $extracted 'tools/summarize_scorecard.py') (Join-Path $extracted 'examples/pilot-scorecard.csv')
        if ($LASTEXITCODE -ne 0) { throw 'The extracted kit could not summarize its evaluation scorecard.' }
        if ($QueueService) {
            & python -B (Join-Path $PSScriptRoot 'qualify_shared_runner.py') $installedLauncher
            if ($LASTEXITCODE -ne 0) { throw 'The extracted launcher failed shared runner qualification.' }
        }
        & python -B (Join-Path $PSScriptRoot 'qualify_launcher.py') $installedLauncher
        if ($LASTEXITCODE -ne 0) { throw 'The extracted launcher failed workflow qualification.' }
    }
    if ($QueueService) {
        & (Join-Path $extracted 'shared/craft-factory-queue.exe') --help | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'The extracted queue service could not report its arguments.' }
        if (!(Get-ChildItem -LiteralPath (Join-Path $extracted 'shared/migrations') -Filter '*.sql' -File)) {
            throw 'The shared service archive has no PostgreSQL migrations.'
        }
    }
    [IO.File]::Move($archive, $outputPath)
    Get-FileHash -LiteralPath $outputPath -Algorithm SHA256
} finally {
    $cleanup = Get-Item -LiteralPath $scratch.FullName
    if ($cleanup -is [IO.DirectoryInfo] -and $cleanup.Parent.FullName -eq $outputDirectory -and
        $cleanup.Name -eq $scratch.Name -and !$cleanup.LinkType) {
        Remove-Item -LiteralPath $cleanup.FullName -Recurse -Force
    }
}
