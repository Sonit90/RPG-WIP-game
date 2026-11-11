<#
.SYNOPSIS
    Starts three game clients on this machine — a Tank, a Healer and a DPS — against the local
    (or -RealmConfig) realm, each signed in as its own account.

.DESCRIPTION
    Each client gets its own --profile, so their remembered sessions do not collide. With
    -Autoplay the clients sign in, queue, accept and start on their own (development flags
    --dev-login and --dev-autoplay); without it each opens on the login screen.

    Run .\tools\realm-dev.ps1 up first.

.EXAMPLE
    .\tools\run-3-clients.ps1 -Autoplay
    .\tools\run-3-clients.ps1 -Windowed -Width 960 -Height 540
#>
[CmdletBinding()]
param(
    [switch] $Autoplay,
    [int] $Width = 960,
    [int] $Height = 540,
    [string] $RealmConfig,
    [switch] $DebugBuild
)

$ErrorActionPreference = 'Stop'

function Invoke-Native([scriptblock] $Block, [string] $What) {
    # Windows PowerShell turns a native tool's stderr into an error record; with
    # ErrorActionPreference = Stop that would abort on cargo's progress output. Run with
    # Continue, show everything, and judge the tool by its exit code instead.
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $Block 2>&1 | ForEach-Object { "$_" } | Out-Host } finally { $ErrorActionPreference = $previous }
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit $LASTEXITCODE)" }
}
$Root = Split-Path -Parent $PSScriptRoot
$BuildProfile = if ($DebugBuild) { 'debug' } else { 'release' }
Push-Location $Root
try {
    if ($DebugBuild) { Invoke-Native { cargo build --bin mygame } 'build' } else { Invoke-Native { cargo build --release --bin mygame } 'build' }
} finally { Pop-Location }
$Exe = Join-Path $Root "target\$BuildProfile\mygame.exe"

$Clients = @(
    @{ Name = 'tank';   Character = 'warrior';  Role = 'tank' },
    @{ Name = 'healer'; Character = 'druid';    Role = 'healer' },
    @{ Name = 'dps';    Character = 'firemage'; Role = 'dps' }
)
foreach ($client in $Clients) {
    $arguments = @('--online', '--profile', $client.Name, '--width', $Width, '--height', $Height, '--background')
    if ($RealmConfig) { $arguments += @('--realm-config', $RealmConfig) }
    if ($Autoplay) {
        $arguments += @(
            '--dev-login', "$($client.Name)@local.mygame.test:local-password-1",
            '--dev-autoplay', "$($client.Character):$($client.Role)"
        )
    }
    Start-Process -FilePath $Exe -ArgumentList $arguments -WorkingDirectory (Split-Path $Exe)
    Write-Host "started $($client.Name) ($($client.Character) as $($client.Role))"
}
