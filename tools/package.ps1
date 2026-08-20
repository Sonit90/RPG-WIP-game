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
    # Resolved in the body, not here: a param default is evaluated in a scope where
    # $PSScriptRoot is not reliably populated under every invocation form, and an empty
    # string then reaches Join-Path and fails the script before it has done anything.
    [string] $OutputPath
)

$ErrorActionPreference = 'Stop'
$scriptRoot = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
$repositoryRoot = Resolve-Path (Join-Path $scriptRoot '..')
if (-not $OutputPath) {
    $OutputPath = Join-Path $repositoryRoot 'dist\mygame'
}
Set-Location $repositoryRoot

# Runs a native command and returns its exit code, without letting Windows PowerShell turn
# its stderr into a terminating error.
#
# Under $ErrorActionPreference = 'Stop', Windows PowerShell 5.1 wraps each stderr line from
# a native executable in a NativeCommandError and treats it as terminating — even when the
# process exits 0. Cargo writes its progress ("Compiling ...", "Finished ...") to stderr, so
# every successful build looked like a failure. The exit code is the only thing that
# actually reports success, so the preference is relaxed for the call and the code checked.
function Invoke-Native {
    param([string] $Command, [string[]] $Arguments)

    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Command @Arguments
    }
    finally {
        $ErrorActionPreference = $previous
    }
    return $LASTEXITCODE
}

Write-Host 'Building release binary...' -ForegroundColor Cyan

# --bin mygame, not --workspace: the benchmark and showcase crates have their own binaries
# and must not be built, let alone copied.
$buildArguments = @('build', '--release', '--bin', 'mygame')
if (-not $Overlay) {
    $buildArguments += @('--no-default-features')
}

$buildExit = Invoke-Native 'cargo' $buildArguments
if ($buildExit -ne 0) {
    throw "cargo build failed with exit code $buildExit"
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

# Content, beside the executable. The loader searches the executable's own directory before
# the working directory, so a player who double-clicks the .exe from anywhere finds it; a
# checkout with an empty assets/ still packages, because every model has a procedural
# fallback and a build with no content directory must still run.
$assetSource = Join-Path $repositoryRoot 'assets'
$assetFiles = @(Get-ChildItem -Recurse -File $assetSource -Exclude 'README.md' -ErrorAction SilentlyContinue)
if ($assetFiles.Count -gt 0) {
    $assetTarget = Join-Path $OutputPath 'assets'
    New-Item -ItemType Directory -Force -Path $assetTarget | Out-Null
    Copy-Item (Join-Path $assetSource '*') -Destination $assetTarget -Recurse -Exclude 'README.md'
    $assetBytes = ($assetFiles | Measure-Object -Property Length -Sum).Sum
    Write-Host ("Content: {0} file(s), {1:N1} MB" -f $assetFiles.Count, ($assetBytes / 1MB))
} else {
    Write-Host 'Content: none in assets/ - the package uses procedural models only.' -ForegroundColor Yellow
}

Write-Host "Package written to $OutputPath" -ForegroundColor Cyan
Write-Host ''
Write-Host 'Verifying the package...' -ForegroundColor Cyan

$failures = @()
$packaged = Get-ChildItem -Recurse -File $OutputPath

# 1. Nothing that looks like a test, benchmark or development artefact. Every development
#    binary in the workspace is named here explicitly as well as caught by the generic
#    patterns, so adding one to the workspace without adding it here is the only way to get
#    it past this check — and the list is short enough to keep current.
$forbiddenNames = @(
    'performance_test', 'cpu_bench', 'cull_bench', 'showcase',
    'test', 'bench', 'deps', 'incremental'
)
# Content is exempt: an artist's file may legitimately be called anything, and it is the
# build output, not the art, that this check is about.
$buildOutput = $packaged | Where-Object { $_.FullName -notlike (Join-Path $OutputPath 'assets\*') }
foreach ($file in $buildOutput) {
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

# 3. No test-only symbols linked into the binary. The benchmark, showcase and test harness
#    names are distinctive enough that finding one would mean something was linked that
#    should not have been.
#
#    findstr exits 1 when it finds nothing, which is the success case here, so the exit code
#    is inspected rather than allowed to look like a failure.
$symbolPatterns = @('performance_test', 'cpu_bench', 'cull_bench', 'engine_showcase', 'should_panic')
$symbolQuery = ($symbolPatterns | ForEach-Object { "/C:`"$_`"" }) -join ' '
$symbolExit = Invoke-Native 'cmd' @('/c', "findstr $symbolQuery `"$($executables[0].FullName)`" >nul")
if ($symbolExit -eq 0) {
    $failures += 'the binary contains test, benchmark or showcase symbols'
}

# 4. The package actually starts and exits cleanly. Two frames is enough to prove the
#    window, the device and the loop all came up.
Write-Host 'Smoke test: running two frames...'
# The game logs to stderr, so this goes through Invoke-Native for the same reason cargo
# does: under $ErrorActionPreference = 'Stop', Windows PowerShell would treat a log line
# from a successful run as a terminating error.
$smokeExit = Invoke-Native (Join-Path $OutputPath 'mygame.exe') @('--frames', '2', '--report', '0')
if ($smokeExit -ne 0) {
    $failures += "the packaged binary exited with code $smokeExit"
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
