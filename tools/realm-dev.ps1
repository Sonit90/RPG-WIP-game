<#
.SYNOPSIS
    Runs the whole online stack on this machine: Postgres, SpacetimeDB, the realm module and
    the realm API.

.DESCRIPTION
    up       Starts Postgres (Docker), a local SpacetimeDB server, builds and publishes the
             realm module, points it at the local API, and starts the realm API.
    publish  Rebuilds and republishes the module and regenerates the client bindings.
    down     Stops the API and SpacetimeDB (Postgres keeps its data; -Wipe removes it too).
    status   Shows what is running.

    Everything this script starts writes its logs to .realm-dev/ at the repository root,
    which is ignored by git. SpacetimeDB keeps its data there as well.

    Prerequisites: Docker Desktop, the spacetime CLI (https://spacetimedb.com/install), the
    wasm32-unknown-unknown Rust target. See docs/networking/SPACETIMEDB_DEPLOYMENT.md.

.EXAMPLE
    .\tools\realm-dev.ps1 up
    .\tools\realm-dev.ps1 publish
    .\tools\realm-dev.ps1 down -Wipe
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('up', 'publish', 'down', 'status')]
    [string] $Command = 'status',
    [switch] $Wipe,
    [string] $Database = 'realm'
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
$State = Join-Path $Root '.realm-dev'
New-Item -ItemType Directory -Force $State | Out-Null

$Spacetime = (Get-Command spacetime -ErrorAction SilentlyContinue).Source
if (-not $Spacetime) {
    $Candidate = Join-Path $env:LOCALAPPDATA 'SpacetimeDB\spacetime.exe'
    if (Test-Path $Candidate) { $Spacetime = $Candidate }
}
$Server = 'http://127.0.0.1:3000'
$Module = Join-Path $Root 'server\realm-module'
$Wasm = Join-Path $Module 'target\wasm32-unknown-unknown\release\realm_module.wasm'
$Backend = Join-Path $Root 'backend'

function Write-Step([string] $Text) { Write-Host "==> $Text" -ForegroundColor Cyan }

function Test-Port([int] $Port) {
    $client = New-Object Net.Sockets.TcpClient
    try { $client.Connect('127.0.0.1', $Port); return $true } catch { return $false } finally { $client.Close() }
}

function Wait-Port([int] $Port, [int] $Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Port $Port) { return }
        Start-Sleep -Milliseconds 300
    }
    throw "nothing is listening on port $Port after $Seconds s"
}

function Start-Postgres {
    Write-Step 'Postgres (Docker, 127.0.0.1:5434)'
    Invoke-Native { docker compose -f (Join-Path $Root 'deploy\docker-compose.dev.yml') up -d } 'docker compose up'
    Wait-Port 5434 60
}

function Start-Spacetime {
    if (Test-Port 3000) { Write-Step 'SpacetimeDB already running on 3000'; return }
    if (-not $Spacetime) { throw 'spacetime CLI not found: install it from https://spacetimedb.com/install' }
    Write-Step 'SpacetimeDB (127.0.0.1:3000)'
    $data = Join-Path $State 'spacetime-data'
    $process = Start-Process -FilePath $Spacetime -WindowStyle Hidden -PassThru `
        -ArgumentList @('start', '--listen-addr', '127.0.0.1:3000', '--data-dir', $data, '--non-interactive') `
        -RedirectStandardOutput (Join-Path $State 'spacetime.log') -RedirectStandardError (Join-Path $State 'spacetime.err.log')
    $process.Id | Set-Content (Join-Path $State 'spacetime.pid')
    Wait-Port 3000 30
}

function Publish-Module {
    Write-Step 'Building the realm module (wasm32)'
    Push-Location $Module
    try { Invoke-Native { cargo build --target wasm32-unknown-unknown --release } 'module build' } finally { Pop-Location }
    Write-Step "Publishing '$Database'"
    Invoke-Native { & $Spacetime publish $Database --server $Server --bin-path $Wasm --yes } 'publish'
    Write-Step 'Pointing the realm at the local API issuer'
    Invoke-Native { & $Spacetime call --server $Server $Database admin_configure '"http://127.0.0.1:8080"' '"mygame-realm"' '"service:realm-bridge"' } 'admin_configure'
    Write-Step 'Regenerating client bindings'
    & $Spacetime generate --lang rust --bin-path $Wasm --out-dir (Join-Path $Root 'crates\engine-net\src\bindings') --yes --no-config | Out-Host
}

function Start-Api {
    if (Test-Port 8080) { Write-Step 'Realm API already running on 8080'; return }
    Write-Step 'Realm API (127.0.0.1:8080)'
    $envFile = Join-Path $Backend '.env'
    if (-not (Test-Path $envFile)) { Copy-Item (Join-Path $Backend '.env.development.example') $envFile }
    Push-Location $Backend
    try { Invoke-Native { cargo build } 'API build' } finally { Pop-Location }
    $exe = Join-Path $Backend 'target\debug\generic-auth-api.exe'
    $process = Start-Process -FilePath $exe -WorkingDirectory $Backend -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $State 'api.log') -RedirectStandardError (Join-Path $State 'api.err.log')
    $process.Id | Set-Content (Join-Path $State 'api.pid')
    Wait-Port 8080 60
}

function Stop-Recorded([string] $Name) {
    $pidFile = Join-Path $State "$Name.pid"
    if (Test-Path $pidFile) {
        $id = Get-Content $pidFile
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
        Remove-Item $pidFile
        Write-Step "stopped $Name ($id)"
    }
}

switch ($Command) {
    'up' {
        Start-Postgres
        Start-Spacetime
        Publish-Module
        Start-Api
        Write-Host ''
        Write-Host 'Realm running. Play:   cargo run --release --bin mygame -- --online' -ForegroundColor Green
        Write-Host '           Three clients: .\tools\run-3-clients.ps1' -ForegroundColor Green
        Write-Host "           Logs: $State" -ForegroundColor Green
    }
    'publish' { Publish-Module }
    'down' {
        Stop-Recorded 'api'
        Stop-Recorded 'spacetime'
        if ($Wipe) {
            Invoke-Native { docker compose -f (Join-Path $Root 'deploy\docker-compose.dev.yml') down -v } 'docker compose down'
            Remove-Item -Recurse -Force (Join-Path $State 'spacetime-data') -ErrorAction SilentlyContinue
            Write-Step 'wiped Postgres volume and SpacetimeDB data'
        }
    }
    'status' {
        foreach ($entry in @(@('Postgres', 5434), @('SpacetimeDB', 3000), @('Realm API', 8080))) {
            $up = Test-Port $entry[1]
            $colour = if ($up) { 'Green' } else { 'Red' }
            Write-Host ("{0,-12} {1,-5} {2}" -f $entry[0], $entry[1], ($(if ($up) { 'up' } else { 'down' }))) -ForegroundColor $colour
        }
    }
}
