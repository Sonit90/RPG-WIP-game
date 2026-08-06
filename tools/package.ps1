<#
.SYNOPSIS
    Builds a clean release distribution of the game and verifies it contains no test or
    benchmark infrastructure.

.DESCRIPTION
    Produces dist/mygame/ containing only what the game needs to run.

    The verification is the point of the script, not a formality: it fails if any test or
    benchmark binary appears in the package, so "no test code ships" is checked rather
    than assumed. Cargo already makes this true by construction — #[cfg(test)] modules
    compile only under the test profile, and tests/ and benches/ targets are built only
    by cargo test / cargo bench — but a future build script, feature or copy step could
    break it silently.

.PARAMETER Overlay
    Keeps the debug overlay in the package. The overlay ships by default (players and
    testers need frame-time numbers without a command line); pass -Overlay:$false to build
    without it.

.PARAMETER OutputPath
    Where to write the package. Defaults to dist/mygame.

.EXAMPLE
    .\tools\package.ps1
    .\tools\package.ps1 -Overlay:$false -OutputPath C:\builds\mygame
#>
[CmdletBinding()]
param(
    [switch] $Overlay = $true,
    [string] $OutputPath = (Join-Path $PSScriptRoot '..\dist\mygame')
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $repositoryRoot

Write-Host 'Building release binary...' -ForegroundColor Cyan

# --bin mygame, not --workspace: the benchmark crate has its own binaries and must not be
# built, let alone copied.
$buildArguments = @('build', '--release', '--bin', 'mygame')
if (-not $Overlay) {
    $buildArguments += @('--no-default-features')
}

& cargo @buildArguments
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
}

$binary = Join-Path $repositoryRoot 'target\release\mygame.exe'
if (-not (Test-Path $binary)) {
    throw "expected binary not found: $binary"
}

if (Test-Path $OutputPath) {
    Remove-Item -Recurse -Force $OutputPath
}
New-Item -ItemType Directory -Force -Path $OutputPath | Out-Null

Copy-Item $binary -Destination $OutputPath
Copy-Item (Join-Path $repositoryRoot 'README.md') -Destination $OutputPath -ErrorAction SilentlyContinue

Write-Host "Package written to $OutputPath" -ForegroundColor Cyan
Write-Host ''
Write-Host 'Verifying the package...' -ForegroundColor Cyan

$failures = @()
$packaged = Get-ChildItem -Recurse -File $OutputPath

# 1. Nothing that looks like a test or benchmark artefact.
$forbiddenNames = @('performance_test', 'cpu_bench', 'test', 'bench', 'deps', 'incremental')
foreach ($file in $packaged) {
    foreach ($pattern in $forbiddenNames) {
        if ($file.Name -like "*$pattern*") {
            $failures += "test/benchmark artefact in package: $($file.Name)"
        }
    }
}

# 2. Exactly one executable, and it is the game.
$executables = $packaged | Where-Object { $_.Extension -eq '.exe' }
if ($executables.Count -ne 1) {
    $failures += "expected exactly one executable, found $($executables.Count)"
} elseif ($executables[0].Name -ne 'mygame.exe') {
    $failures += "unexpected executable: $($executables[0].Name)"
}

# 3. No test-only symbols linked into the binary. The benchmark and test harness names are
#    distinctive enough that finding them would mean something was linked that should not
#    have been.
$symbols = & cmd /c "findstr /C:`"performance_test`" /C:`"cpu_bench`" /C:`"should_panic`" `"$($executables[0].FullName)`"" 2>$null
if ($LASTEXITCODE -eq 0) {
    $failures += 'the binary contains test or benchmark symbols'
}

# 4. The package actually starts and exits cleanly. Two frames is enough to prove the
#    window, the device and the loop all came up.
Write-Host 'Smoke test: running two frames...'
# No 2>&1 here: redirecting a native command's stderr in Windows PowerShell wraps every
# line in an ErrorRecord and trips $ErrorActionPreference, even on a successful exit.
& (Join-Path $OutputPath 'mygame.exe') --frames 2 --report 0
if ($LASTEXITCODE -ne 0) {
    $failures += "the packaged binary exited with code $LASTEXITCODE"
}

Write-Host ''
if ($failures.Count -gt 0) {
    foreach ($failure in $failures) {
        Write-Host "FAIL: $failure" -ForegroundColor Red
    }
    throw "package verification failed with $($failures.Count) problem(s)"
}

$size = [math]::Round((($packaged | Measure-Object -Property Length -Sum).Sum / 1MB), 2)
Write-Host 'PASS: package contains no test or benchmark infrastructure' -ForegroundColor Green
Write-Host "Contents ($size MB):"
$packaged | ForEach-Object { Write-Host "  $($_.Name)" }
