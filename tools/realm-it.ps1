<#
.SYNOPSIS
    The realm's end-to-end suite: brings the local stack up if it is down, then runs every
    realm-bot scenario (slice, isolation, decline, reconnect, security, results, combat, loop).

.DESCRIPTION
    Exits non-zero if any scenario fails. Each scenario prints its checks; the summary lines
    are PASS <name> / FAIL <name>: <check>.

.EXAMPLE
    .\tools\realm-it.ps1
    .\tools\realm-it.ps1 -Scenario security
#>
[CmdletBinding()]
param(
    [string] $Scenario = 'all'
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

& (Join-Path $PSScriptRoot 'realm-dev.ps1') status
$needed = @(5434, 3000, 8080) | Where-Object {
    $client = New-Object Net.Sockets.TcpClient
    try { $client.Connect('127.0.0.1', $_); $false } catch { $true } finally { $client.Close() }
}
if ($needed) { & (Join-Path $PSScriptRoot 'realm-dev.ps1') up }

$spacetime = (Get-Command spacetime -ErrorAction SilentlyContinue).Source
if (-not $spacetime) { $spacetime = Join-Path $env:LOCALAPPDATA 'SpacetimeDB\spacetime.exe' }
$env:SPACETIME_CLI = $spacetime

Push-Location $Root
try {
    Invoke-Native { cargo build -p engine-realm-testclient } 'build'
    # The bots log to stderr; that is output, not failure. The exit code is the verdict.
    $ErrorActionPreference = 'Continue'
    & (Join-Path $Root 'target\debug\realm-bot.exe') scenario $Scenario 2>&1 | ForEach-Object { "$_" }
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
