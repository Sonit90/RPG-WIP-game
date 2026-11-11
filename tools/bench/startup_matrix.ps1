# Startup benchmark driver: named configurations, launched repeatedly and interleaved.
#
# Needs a build with `--features startup-timing` (docs/startup-performance.md): each launch
# prints its stage table once the first frame is presented, and this reads it. Every launch is
# `--hidden`, renders a handful of frames and exits.
#
# A configuration is "label|binary|arguments", exactly as in frame_matrix.ps1:
#
#   .\tools\bench\startup_matrix.ps1 -Configs "base|target/audit-ref/mygame-base.exe|--scene dungeon;new||--scene dungeon"
#
# The first launch of each binary is reported on its own as "cold": the GPU driver keeps a
# shader cache per executable, so the first launch of a freshly built or freshly copied binary
# compiles every pipeline from DXIL, and that launch is a different measurement from the rest.
# The warm launches are then interleaved - every configuration once, then again - and each
# stage reports its median.
param(
    [string] $Configs,
    [int]    $Repeats = 7,
    [string] $Exe     = "target/release/mygame.exe",
    [string] $Out     = "",
    [string] $Title   = ""
)

[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture

function Median([double[]] $values) {
    $sorted = @($values | Sort-Object)
    $n = $sorted.Count
    if ($n -eq 0) { return [double]::NaN }
    if ($n % 2 -eq 1) { return [double]$sorted[[int](($n - 1) / 2)] }
    return ([double]$sorted[$n / 2 - 1] + [double]$sorted[$n / 2]) / 2.0
}

function Launch([string] $binary, [string] $extra) {
    $argv = @("--hidden", "--no-audio", "--frames", "5", "--report", "0")
    if ($extra -ne "") { $argv += $extra.Split(" ", [StringSplitOptions]::RemoveEmptyEntries) }
    $text = (& $binary @argv 2>&1 | ForEach-Object { $_.ToString() }) -join "`n"
    $stages = [ordered]@{}
    foreach ($m in [regex]::Matches($text, "startup::measured: \[startup\] (\S.*?)\s+([\d.]+) ms\s*$", "Multiline")) {
        $stages[$m.Groups[1].Value.Trim()] = [double]$m.Groups[2].Value
    }
    $tex = [regex]::Match($text, "textures so far: \d+ uploaded, [\d.]+ Mtexels at level 0 \| ([\d.]+) ms hashing and scanning, ([\d.]+) ms filtering mips")
    $upl = [regex]::Match($text, "uploads so far: \d+ resources created in [\d.]+ ms \(of which \d+ buffers, [\d.]+ ms\), ([\d.]+) ms compressing")
    if ($tex.Success) {
        $stages["  (textures: hashing and scanning)"] = [double]$tex.Groups[1].Value
        $stages["  (textures: filtering mips)"] = [double]$tex.Groups[2].Value
    }
    if ($upl.Success) { $stages["  (textures: compressing)"] = [double]$upl.Groups[1].Value }
    return $stages
}

$entries = [ordered]@{}
$configList = @($Configs.Split(";") | Where-Object { $_.Trim() -ne "" })
foreach ($config in $configList) {
    $parts  = $config.Split("|")
    $label  = $parts[0]
    $binary = if ($parts.Count -gt 1 -and $parts[1] -ne "") { $parts[1] } else { $Exe }
    $extra  = if ($parts.Count -gt 2) { $parts[2].Trim() } else { "" }
    if (-not (Test-Path $binary)) { Write-Error "no binary at $binary"; exit 1 }
    $entries[$label] = [pscustomobject]@{ binary = $binary; extra = $extra; cold = $null; warm = [ordered]@{} }
}

# Cold: the first launch of each configuration, before any of the warm ones.
foreach ($label in $entries.Keys) {
    $e = $entries[$label]
    $e.cold = Launch $e.binary $e.extra
    Write-Host ("cold  {0,-30} TOTAL {1,7:F1} ms" -f $label, $e.cold["TOTAL"])
}

for ($pass = 1; $pass -le $Repeats; $pass++) {
    foreach ($label in $entries.Keys) {
        $e = $entries[$label]
        $stages = Launch $e.binary $e.extra
        foreach ($name in $stages.Keys) {
            if (-not $e.warm.Contains($name)) { $e.warm[$name] = New-Object System.Collections.ArrayList }
            [void]$e.warm[$name].Add($stages[$name])
        }
        Write-Host ("pass {0} {1,-30} TOTAL {2,7:F1} ms" -f $pass, $label, $stages["TOTAL"])
    }
}

# Stage names in first-seen order across every configuration.
$names = New-Object System.Collections.ArrayList
foreach ($label in $entries.Keys) {
    foreach ($name in $entries[$label].warm.Keys) { if (-not $names.Contains($name)) { [void]$names.Add($name) } }
}

Write-Host ""
if ($Title -ne "") { Write-Host "### $Title"; Write-Host "" }
$header = "| Stage |"; $rule = "| --- |"
foreach ($label in $entries.Keys) { $header += " $label |"; $rule += " ---: |" }
Write-Host $header; Write-Host $rule
$result = [ordered]@{}
foreach ($name in $names) {
    $line = "| $name |"
    foreach ($label in $entries.Keys) {
        $values = if ($entries[$label].warm.Contains($name)) { [double[]]@($entries[$label].warm[$name]) } else { @() }
        $median = Median $values
        $line += $(if ([double]::IsNaN($median)) { " - |" } else { " {0:F1} |" -f $median })
        $result["$label`t$name"] = $median
    }
    Write-Host $line
}
$line = "| first launch (cold) TOTAL |"
foreach ($label in $entries.Keys) { $line += " {0:F1} |" -f $entries[$label].cold["TOTAL"] }
Write-Host $line
Write-Host ""
Write-Host ("Warm rows: median of {0} interleaved launches per configuration." -f $Repeats)
if ($Out -ne "") {
    $json = [ordered]@{}
    foreach ($label in $entries.Keys) {
        $warm = [ordered]@{}
        foreach ($name in $entries[$label].warm.Keys) { $warm[$name] = Median ([double[]]@($entries[$label].warm[$name])) }
        $json[$label] = [ordered]@{ args = $entries[$label].extra; binary = $entries[$label].binary; cold = $entries[$label].cold; warm_median = $warm; runs = $Repeats }
    }
    $json | ConvertTo-Json -Depth 5 | Out-File -Encoding utf8 $Out
}
