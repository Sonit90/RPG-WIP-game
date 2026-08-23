# Shadow benchmark driver: named configurations across resolutions.
#
# One run of the game per cell. Nothing here touches input, so the camera stands where the
# scene put it and every run of a given scene sees the same frame; the only thing that
# changes between cells is the shadow configuration named in -Configs.
#
# A configuration is "label|--shadows value|shadow-tweak list". The tweak list may be empty.
#
#   .\tools\bench\shadow_matrix.ps1 -Configs "off|off|;ultra|ultra|;no contact|ultra|contact=off"
#
# Semicolons separate configurations, commas separate resolutions. One string each rather
# than an array because these are run through `powershell -File`, where every argument
# arrives as a literal string and a comma-separated array is not one.
#
# Repetitions are **interleaved** - every configuration once, then every configuration again -
# and each cell reports the median of its repetitions. The machine drifts over a few minutes
# (clocks, temperature, whatever else is running), and running one configuration to completion
# before starting the next puts all of that drift on one row. This is the same rule the
# renderer benchmark in engine-bench already follows, and ignoring it produced retracted
# results there.
#
# Identical between milestones by construction: a milestone changes the renderer, never this
# file. The frame rate is derived from the median frame rather than from wall clock, because
# a windowed run's wall clock also contains whatever the desktop compositor did.
param(
    [string]   $Configs,
    [string]   $Resolutions = "1280x720,1920x1080,2560x1440,3840x2160",
    [int]      $Frames      = 900,
    [int]      $Warmup      = 600,
    [int]      $Repeats     = 3,
    [string]   $Scene       = "combat",
    [string]   $Out         = "",
    [string]   $Title       = ""
)

$exe = "target/release/mygame.exe"
if (-not (Test-Path $exe)) { Write-Error "build target/release/mygame.exe first"; exit 1 }

function Median([double[]] $values) {
    $sorted = $values | Sort-Object
    $n = $sorted.Count
    if ($n -eq 0) { return 0.0 }
    if ($n % 2 -eq 1) { return [double]$sorted[[int](($n - 1) / 2)] }
    return ([double]$sorted[$n / 2 - 1] + [double]$sorted[$n / 2]) / 2.0
}

$samples = @{}
$order   = @()

foreach ($res in $Resolutions.Split(",")) {
    $w, $h = $res.Split("x")
    for ($pass = 1; $pass -le $Repeats; $pass++) {
        foreach ($config in $Configs.Split(";")) {
            $parts   = $config.Split("|")
            $label   = $parts[0]
            $quality = if ($parts.Count -gt 1 -and $parts[1] -ne "") { $parts[1] } else { "ultra" }
            $tweak   = if ($parts.Count -gt 2) { $parts[2] } else { "" }
            $key     = "$res`t$label"
            if (-not $samples.ContainsKey($key)) {
                $samples[$key] = [pscustomobject]@{
                    resolution = $res; config = $label; quality = $quality; tweak = $tweak
                    cpu = @(); cpu95 = @(); gpu = @(); gpu95 = @(); stage = @()
                }
                $order += $key
            }

            $argv = @("--width", $w, "--height", $h, "--frames", ($Frames + $Warmup),
                      "--warmup", $Warmup, "--shadows", $quality, "--scene", $Scene,
                      "--report", "0", "--no-audio")
            if ($tweak -ne "") { $argv += @("--shadow-tweak", $tweak) }
            $text = & $exe @argv 2>&1 | Out-String

            $cpu   = [regex]::Match($text, "final CPU window: mean [\d.]+ \| p50 ([\d.]+) \| p95 ([\d.]+)")
            $gpu   = [regex]::Match($text, "final GPU window: mean [\d.]+ \| p50 ([\d.]+) \| p95 ([\d.]+)")
            $stage = [regex]::Match($text, "final shadow-stage GPU: mean [\d.]+ \| p50 ([\d.]+)")
            if (-not $cpu.Success) { Write-Warning "no CPU line for $res '$label'"; Write-Host $text; continue }

            $s = $samples[$key]
            $s.cpu   += [double]$cpu.Groups[1].Value
            $s.cpu95 += [double]$cpu.Groups[2].Value
            $s.gpu   += $(if ($gpu.Success) { [double]$gpu.Groups[1].Value } else { 0.0 })
            $s.gpu95 += $(if ($gpu.Success) { [double]$gpu.Groups[2].Value } else { 0.0 })
            $s.stage += $(if ($stage.Success) { [double]$stage.Groups[1].Value } else { 0.0 })
            Write-Host ("pass {0}  {1,-10} {2,-24} cpu {3,6:N3} | gpu {4,6:N3} | stage {5,5:N3}" -f
                $pass, $res, $label, $s.cpu[-1], $s.gpu[-1], $s.stage[-1])
        }
    }
}

$rows = @()
foreach ($key in $order) {
    $s   = $samples[$key]
    $c50 = Median $s.cpu
    $g50 = Median $s.gpu
    $rows += [pscustomobject]@{
        resolution = $s.resolution
        config     = $s.config
        quality    = $s.quality
        tweak      = $s.tweak
        runs       = $s.cpu.Count
        cpu_p50    = $c50
        cpu_p95    = Median $s.cpu95
        gpu_p50    = $g50
        gpu_p95    = Median $s.gpu95
        gpu_spread = $(if ($s.gpu.Count -gt 1) { ($s.gpu | Measure-Object -Maximum).Maximum - ($s.gpu | Measure-Object -Minimum).Minimum } else { 0.0 })
        shadow_p50 = Median $s.stage
        fps        = $(if ([Math]::Max($c50, $g50) -gt 0) { 1000.0 / [Math]::Max($c50, $g50) } else { 0.0 })
    }
}

Write-Host ""
if ($Title -ne "") { Write-Host "### $Title"; Write-Host "" }
Write-Host "| Resolution | Configuration | CPU p50 | CPU p95 | GPU p50 | GPU p95 | GPU spread | Shadow stage GPU p50 | FPS |"
Write-Host "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
foreach ($r in $rows) {
    Write-Host ("| {0} | {1} | {2:N3} | {3:N3} | {4:N3} | {5:N3} | {6:N3} | {7:N3} | {8:N0} |" -f
        $r.resolution, $r.config, $r.cpu_p50, $r.cpu_p95, $r.gpu_p50, $r.gpu_p95, $r.gpu_spread, $r.shadow_p50, $r.fps)
}
if ($Out -ne "") { $rows | ConvertTo-Json -Depth 3 | Out-File -Encoding utf8 $Out }
