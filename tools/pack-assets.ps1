<#
.SYNOPSIS
    Zips every git-ignored file under assets/ into one archive.

.DESCRIPTION
    Most of assets/ (models, .blend sources, animations, generated content) lives beside the
    repository rather than in it - see .gitignore. A fresh clone therefore has none of it.
    This script collects exactly those files, as git itself reports them
    (`git ls-files --others --ignored --exclude-standard assets`), into a zip whose paths
    start at the repository root, so extracting it at the root of another checkout restores
    them in place.

    Tracked files (scenes, templates, catalog, audio, assets/README.md) are left out: git
    already carries them.

.PARAMETER Output
    The zip to write. Defaults to dist/mygame-assets-<yyyyMMdd>.zip (dist/ is git-ignored).
    An existing file at that path is replaced.

.PARAMETER Exclude
    Wildcard patterns, matched against the repository-relative path, for files to leave out,
    e.g. '*.blend1', '*.autosave.json'.

.EXAMPLE
    tools\pack-assets.ps1

.EXAMPLE
    tools\pack-assets.ps1 -Output D:\backup\assets.zip -Exclude '*.blend1','*.autosave.json'
#>
[CmdletBinding()]
param(
    [string]$Output,
    [string[]]$Exclude = @()
)

$ErrorActionPreference = 'Stop'

$root = (git -C $PSScriptRoot rev-parse --show-toplevel).Trim()
if ($LASTEXITCODE -ne 0) { throw 'not inside a git repository' }
$root = [System.IO.Path]::GetFullPath($root)

if (-not $Output) {
    $Output = Join-Path $root ("dist/mygame-assets-{0}.zip" -f (Get-Date -Format 'yyyyMMdd'))
}
$Output = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine((Get-Location).Path, $Output))

# -z keeps names with spaces or non-ASCII characters intact (no quoting or escaping).
$raw = git -C $root -c core.quotepath=off ls-files -z --others --ignored --exclude-standard -- assets
if ($LASTEXITCODE -ne 0) { throw 'git ls-files failed' }
$files = @(($raw -join '') -split "`0" | Where-Object { $_ })
foreach ($pattern in $Exclude) {
    $files = @($files | Where-Object { $_ -notlike $pattern })
}
if ($files.Count -eq 0) {
    Write-Warning 'no git-ignored files under assets/ - nothing to pack'
    exit 0
}

New-Item -ItemType Directory -Force -Path (Split-Path $Output) | Out-Null
if (Test-Path $Output) { Remove-Item $Output -Force }

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$total = 0L
$zip = [System.IO.Compression.ZipFile]::Open($Output, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    $i = 0
    foreach ($file in $files) {
        $i++
        Write-Progress -Activity 'Packing assets' -Status $file -PercentComplete (100 * $i / $files.Count)
        $source = Join-Path $root $file
        $total += (Get-Item -LiteralPath $source).Length
        # Forward slashes: the zip spec's separator, and what unzip on macOS/Linux expects.
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $zip, $source, $file.Replace('\', '/'),
            [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
    }
}
finally {
    $zip.Dispose()
    Write-Progress -Activity 'Packing assets' -Completed
}

$packed = (Get-Item $Output).Length
'{0} files, {1:N1} MB -> {2:N1} MB' -f $files.Count, ($total / 1MB), ($packed / 1MB)
"wrote $Output"
