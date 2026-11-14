$ErrorActionPreference = 'Stop'
$consoleRoot = Split-Path -Parent $PSScriptRoot
$priorConsoleAcceptance = $env:CRAFT_CONSOLE_ACCEPTANCE
Push-Location -LiteralPath $consoleRoot
try {
    $buildOutput = & cargo test -p craft-platform --lib --locked --offline --no-run --message-format json
    if ($LASTEXITCODE -ne 0) { throw 'Console acceptance build failed.' }
    $artifacts = @($buildOutput | ForEach-Object { $_ | ConvertFrom-Json } |
        Where-Object { $_.reason -eq 'compiler-artifact' -and $_.target.name -eq 'craft_platform' -and $_.profile.test -and $_.executable })
    if ($artifacts.Count -ne 1) { throw 'Expected one current platform test executable.' }
    $consoleBinary = [IO.Path]::GetFullPath($artifacts[0].executable)
    $expectedDirectory = [IO.Path]::GetFullPath((Join-Path $consoleRoot 'target\debug\deps')) + [IO.Path]::DirectorySeparatorChar
    if (-not $consoleBinary.StartsWith($expectedDirectory, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Platform test executable is outside the expected workspace target directory.'
    }
    $env:CRAFT_CONSOLE_ACCEPTANCE = 'owned-hidden'
    $consoleOutput = Join-Path $consoleRoot 'target\console-native-output.log'
    $consoleErrors = Join-Path $consoleRoot 'target\console-native-errors.log'
    $consoleTest = Start-Process -FilePath $consoleBinary -ArgumentList @(
        '--ignored', '--exact',
        'console::windows::native_tests::owned_console_preserves_unicode_suppresses_echo_discards_pending_input_and_restores_modes',
        '--nocapture'
    ) -WindowStyle Hidden -RedirectStandardOutput $consoleOutput -RedirectStandardError $consoleErrors -PassThru
    if (-not $consoleTest.WaitForExit(15000)) {
        $consoleTest.Kill()
        $consoleTest.WaitForExit()
        throw 'Owned console test exceeded its acceptance deadline.'
    }
    Get-Content -LiteralPath $consoleOutput
    Get-Content -LiteralPath $consoleErrors
    if ($consoleTest.ExitCode -ne 0) { throw 'Native console acceptance failed.' }
    [pscustomobject]@{
        ok = $true
        executable = $consoleBinary
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $consoleBinary).Hash.ToLowerInvariant()
        exit_code = $consoleTest.ExitCode
    } | ConvertTo-Json -Compress
} finally {
    $env:CRAFT_CONSOLE_ACCEPTANCE = $priorConsoleAcceptance
    Pop-Location
}
