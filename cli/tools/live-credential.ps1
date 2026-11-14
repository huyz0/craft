<#
.SYNOPSIS
Saves the Atlassian API token used by the opt-in live conformance harness.

.DESCRIPTION
Prompts for the account email and API token without echoing the token, then
stores them as one Windows Credential Manager generic credential
(target "craft-live-conformance", local-machine persistence for the current
user). The token never touches disk, argv, environment variables or the
repository. The non-secret site URL goes to the gitignored local config
cli/.craft-live.json so the harness knows which site to read.

Run it yourself in an interactive PowerShell window:

    powershell -ExecutionPolicy Bypass -File tools\live-credential.ps1

Use -Remove to delete the stored credential and -Check to report whether
one exists (the token itself is never printed).
#>
[CmdletBinding()]
param(
    [switch]$Remove,
    [switch]$Check
)

$ErrorActionPreference = 'Stop'
$Target = 'craft-live-conformance'
$ConfigPath = Join-Path (Split-Path -Parent $PSScriptRoot) '.craft-live.json'

Add-Type -Namespace CraftLive -Name Cred -MemberDefinition @'
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
public struct CREDENTIAL {
    public int Flags; public int Type; public string TargetName; public string Comment;
    public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten;
    public int CredentialBlobSize; public IntPtr CredentialBlob; public int Persist;
    public int AttributeCount; public IntPtr Attributes; public string TargetAlias; public string UserName;
}
[DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool CredWriteW(ref CREDENTIAL credential, int flags);
[DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool CredReadW(string target, int type, int flags, out IntPtr credential);
[DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool CredDeleteW(string target, int type, int flags);
[DllImport("advapi32.dll")]
public static extern void CredFree(IntPtr buffer);
'@

$CRED_TYPE_GENERIC = 1
$CRED_PERSIST_LOCAL_MACHINE = 2

function Test-Stored {
    $ptr = [IntPtr]::Zero
    if ([CraftLive.Cred]::CredReadW($Target, $CRED_TYPE_GENERIC, 0, [ref]$ptr)) {
        $cred = [System.Runtime.InteropServices.Marshal]::PtrToStructure($ptr, [type][CraftLive.Cred+CREDENTIAL])
        [CraftLive.Cred]::CredFree($ptr)
        return $cred.UserName
    }
    return $null
}

if ($Check) {
    $user = Test-Stored
    if ($user) { Write-Host "Stored credential '$Target' exists for $user." } else { Write-Host "No stored credential '$Target'." }
    if (Test-Path $ConfigPath) { Write-Host "Local config: $ConfigPath" } else { Write-Host "No local config at $ConfigPath" }
    return
}

if ($Remove) {
    if ([CraftLive.Cred]::CredDeleteW($Target, $CRED_TYPE_GENERIC, 0)) { Write-Host "Removed '$Target'." } else { Write-Host "Nothing to remove." }
    return
}

$site = Read-Host 'Atlassian site (for example https://your-site.atlassian.net)'
$site = $site.Trim().TrimEnd('/')
if ($site -notmatch '^https://[a-z0-9][a-z0-9-]*\.atlassian\.net$') {
    throw 'Site must look like https://<name>.atlassian.net'
}
$email = (Read-Host 'Atlassian account email').Trim()
if ($email -notmatch '^[^@\s]+@[^@\s]+$') { throw 'That does not look like an email address.' }

$secure = Read-Host 'API token (input hidden)' -AsSecureString
if ($secure.Length -eq 0) { throw 'Empty token; nothing saved.' }

$bstr = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
$blob = [IntPtr]::Zero
try {
    $chars = $secure.Length
    # Encode the token as UTF-8 directly from unmanaged memory so no managed
    # string copy of it is created.
    $bytes = New-Object byte[] ($chars * 3)
    $n = 0
    for ($i = 0; $i -lt $chars; $i++) {
        $c = [int][System.Runtime.InteropServices.Marshal]::ReadInt16($bstr, $i * 2)
        if ($c -lt 0x80 -and $c -gt 0x20) { $bytes[$n++] = [byte]$c }
        else { throw 'API tokens contain only printable ASCII; check the pasted value.' }
    }
    $blob = [System.Runtime.InteropServices.Marshal]::AllocHGlobal($n)
    [System.Runtime.InteropServices.Marshal]::Copy($bytes, 0, $blob, $n)
    [Array]::Clear($bytes, 0, $bytes.Length)

    $cred = New-Object CraftLive.Cred+CREDENTIAL
    $cred.Type = $CRED_TYPE_GENERIC
    $cred.TargetName = $Target
    $cred.Comment = 'Craft live conformance harness (Atlassian API token)'
    $cred.CredentialBlobSize = $n
    $cred.CredentialBlob = $blob
    $cred.Persist = $CRED_PERSIST_LOCAL_MACHINE
    $cred.UserName = $email
    if (-not [CraftLive.Cred]::CredWriteW([ref]$cred, 0)) {
        throw "CredWrite failed with Win32 error $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
} finally {
    if ($blob -ne [IntPtr]::Zero) {
        for ($i = 0; $i -lt $n; $i++) { [System.Runtime.InteropServices.Marshal]::WriteByte($blob, $i, 0) }
        [System.Runtime.InteropServices.Marshal]::FreeHGlobal($blob)
    }
    [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
    $secure.Dispose()
}

$config = [ordered]@{ site = $site } | ConvertTo-Json
[System.IO.File]::WriteAllText($ConfigPath, $config + "`n", (New-Object System.Text.UTF8Encoding($false)))

Write-Host "Saved token for $email in Windows Credential Manager as '$Target'."
Write-Host "Saved site to $ConfigPath (gitignored)."
