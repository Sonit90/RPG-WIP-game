# Frame benchmark driver: named configurations, repeated and interleaved, headless.
#
# The performance audit's driver (docs/PERFORMANCE_AUDIT.md). Every run is `--hidden`: no
# window on screen, so nothing is composited by DWM and the machine stays usable while it
# runs. That makes the GPU and Present numbers the uncontended ones - close to the
# fullscreen column of docs/performance.md, not to what a windowed player on a busy desktop
# sees. Every table this produces should say so.
#
# Two modes:
#
#   -Mode res      one launch per configuration drives the in-process resolution matrix
#                  (`--benchmark-at`), so every resolution shares one scene load, one set
#                  of pipelines and one warm driver. Reports CPU/GPU p50/p99, the GPU pass
#                  split, FPS and draw calls per resolution.
#   -Mode pacing   one launch per configuration at one resolution for a long run
#                  (`--frames`), and reads the whole-run distribution the game prints at
#                  exit: p99.9 / 0.1% low, the hitch counts, the CPU stage split, memory,
#                  and - with an `alloc-count` build - allocations per frame.
#
# A configuration is "label|binary|arguments". The binary may be empty for the default,
# which is how an optimization is measured *against* a reference build in the same
# interleaved session rather than against numbers taken earlier on a colder machine:
#
#   .\tools\bench\frame_matrix.ps1 -Mode pacing -Configs "dungeon|target/audit-ref/mygame-base.exe|--scene dungeon;dungeon new||--scene dungeon"
#
# Repetitions are interleaved - every configuration once, then every configuration again -
# and each cell reports the median of its repetitions. docs/performance.md retracted three
# conclusions that came from grouped runs; this file exists so that is not done by hand.
param(
    [ValidateSet("res", "pacing")]
    [string] $Mode        = "res",
    [string] $Configs,
    [string] $Resolutions = "1280x720,1920x1080,2560x1440,3840x2160",
    [string] $Size        = "1920x1080",
    [int]    $Frames      = 600,
    [int]    $Warmup      = 120,
    [int]    $Repeats     = 3,
    [string] $Exe         = "target/release/mygame.exe",
    [string] $Data        = "",
    [string] $Out         = "",
    [string] $Title       = ""
)

# Invariant culture: a decimal point and no thousands separator, whatever the machine's
# locale, so the tables paste into the docs as they are.
[System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::InvariantCulture

function Median([double[]] $values) {
    $sorted = @($values | Sort-Object)
    $n = $sorted.Count
    if ($n -eq 0) { return [double]::NaN }
    if ($n % 2 -eq 1) { return [double]$sorted[[int](($n - 1) / 2)] }
    return ([double]$sorted[$n / 2 - 1] + [double]$sorted[$n / 2]) / 2.0
}

function Num($match, [int] $group) {
    if ($match.Success) { return [double]$match.Groups[$group].Value }
    return [double]::NaN
}

# One configuration's samples: metric name -> list of values, per resolution key.
$samples = [ordered]@{}
function Add-Sample([string] $key, [string] $label, [string] $res, [string] $metric, [double] $value) {
    if (-not $samples.Contains($key)) {
        $samples[$key] = [pscustomobject]@{ config = $label; resolution = $res; metrics = [ordered]@{} }
    }
    $entry = $samples[$key]
    if (-not $entry.metrics.Contains($metric)) { $entry.metrics[$metric] = New-Object System.Collections.ArrayList }
    [void]$entry.metrics[$metric].Add($value)
}

if ($Data -ne "" -and -not (Test-Path $Data)) { New-Item -ItemType Directory -Force $Data | Out-Null }

$passNames = @("shadow", "scene", "particles", "ui", "atlas", "prepass", "contact")

for ($pass = 1; $pass -le $Repeats; $pass++) {
    foreach ($config in $Configs.Split(";")) {
        if ($config.Trim() -eq "") { continue }
        $parts  = $config.Split("|")
        $label  = $parts[0]
        $binary = if ($parts.Count -gt 1 -and $parts[1] -ne "") { $parts[1] } else { $Exe }
        $extra  = if ($parts.Count -gt 2) { $parts[2].Trim() } else { "" }
        if (-not (Test-Path $binary)) { Write-Error "no binary at $binary"; exit 1 }

        $argv = @("--hidden", "--no-audio")
        if ($Mode -eq "res") {
            $argv += @("--benchmark-at", $Resolutions, "--benchmark-passes", "1",
                       "--benchmark-frames", $Frames, "--benchmark-warmup", $Warmup, "--report", "0")
        } else {
            $w, $h = $Size.Split("x")
            $argv += @("--width", $w, "--height", $h, "--frames", ($Frames + $Warmup),
                       "--warmup", $Warmup, "--report", "1")
        }
        if ($extra -ne "") { $argv += $extra.Split(" ", [StringSplitOptions]::RemoveEmptyEntries) }
        # Line by line rather than `Out-String`, which wraps at the console's width and cuts
        # the report lines this parses in half. The log goes to stderr, so each line arrives
        # as an ErrorRecord whose text is the line.
        $text = (& $binary @argv 2>&1 | ForEach-Object { $_.ToString() }) -join "`n"
        if ($Data -ne "") {
            $safe = ($label -replace "[^A-Za-z0-9_-]", "_")
            $text | Out-File -Encoding utf8 (Join-Path $Data "$Mode-$safe-pass$pass.log")
        }

        if ($Mode -eq "res") {
            # One block per resolution, each starting at "  WxH (pass 1)".
            $blocks = [regex]::Matches($text, '(?ms)^\s+(\d+x\d+) \(pass \d+\)\s*$(.*?)(?=^\s+\d+x\d+ \(pass|\[\s*[\d.]+\] INFO)')
            if ($blocks.Count -eq 0) { Write-Warning "no benchmark rows for '$label'"; Write-Host $text; continue }
            foreach ($block in $blocks) {
                $res  = $block.Groups[1].Value
                $body = $block.Groups[2].Value
                $key  = "$res`t$label"
                $gpu = [regex]::Match($body, "GPU frame\s+: p50 ([\d.]+) \| p95 ([\d.]+) \| p99 ([\d.]+)")
                $cpu = [regex]::Match($body, "CPU frame\s+: p50 ([\d.]+) \| p95 ([\d.]+) \| p99 ([\d.]+)")
                $fps = [regex]::Match($body, "FPS\s+: (\d+)")
                $drw = [regex]::Match($body, "draws\s+: (\d+)")
                Add-Sample $key $label $res "cpu_p50" (Num $cpu 1)
                Add-Sample $key $label $res "cpu_p99" (Num $cpu 3)
                Add-Sample $key $label $res "gpu_p50" (Num $gpu 1)
                Add-Sample $key $label $res "gpu_p99" (Num $gpu 3)
                Add-Sample $key $label $res "fps" (Num $fps 1)
                Add-Sample $key $label $res "draws" (Num $drw 1)
                foreach ($name in $passNames) {
                    $p = [regex]::Match($body, "GPU $name\s*: p50 ([\d.]+)")
                    Add-Sample $key $label $res "gpu_$name" (Num $p 1)
                }
            }
            Write-Host ("pass {0}  {1,-28} {2} resolution(s)" -f $pass, $label, $blocks.Count)
        } else {
            $res = $Size
            $key = "$res`t$label"
            $cpu = [regex]::Match($text, "run CPU: mean ([\d.]+) \| p50 ([\d.]+) \| p99 ([\d.]+) \| p99.9 ([\d.]+) \| max ([\d.]+) ms \| avg (\d+) FPS \| 1% low (\d+) FPS \| 0.1% low (\d+) FPS")
            if (-not $cpu.Success) { Write-Warning "no run line for '$label'"; Write-Host $text; continue }
            $gpu = [regex]::Match($text, "run GPU: mean ([\d.]+) \| p50 ([\d.]+) \| p99 ([\d.]+) \| p99.9 ([\d.]+)")
            $hit = [regex]::Match($text, "run hitches: (\d+) over threshold \((\d+) created a pipeline, (\d+) waited on an upload\) \| frames over 4.17 ms (\d+) \| over 8.33 ms (\d+) \| over 16.7 ms (\d+)")
            $alc = [regex]::Match($text, "run allocations: \d+ \(([\d.]+)/frame\), [\d.]+ KiB \((\d+) B/frame\)")
            # The last periodic report: its stage split and its memory line.
            $stageLines = [regex]::Matches($text, "frame: events ([\d.]+) \| update ([\d.]+) \| render ([\d.]+) \(cull ([\d.]+) \+ group ([\d.]+) \+\s+shadow prep ([\d.]+) \+ record ([\d.]+)\) \| present ([\d.]+) ms")
            $heads  = [regex]::Matches($text, "\| sim ([\d.]+) ms \(\d+ ticks\) \| extract ([\d.]+) ms \(anim ([\d.]+) ms, (\d+) chars\) \| vfx ([\d.]+) ms \| camera ([\d.]+) ms \| audio ([\d.]+) ms \| cull [\d.]+ ms \| ui ([\d.]+) ms .*?\| visible (\d+) \| draws (\d+) \((\d+) skinned\) \| tris (\d+)")
            $mems   = [regex]::Matches($text, "RAM working set ([\d.]+) MiB \(peak ([\d.]+)\), private ([\d.]+) MiB \| VRAM ([\d.]+) MiB")
            $gpass  = [regex]::Match($text, "run GPU passes \(p50/mean ms\):(.*)")
            # Run-wide stage means - the periodic report's split is one frame's sample.
            $means  = [regex]::Match($text, "run stages \(mean ms\): events ([\d.]+) \| update ([\d.]+) \(sim ([\d.]+) \| extract ([\d.]+), anim ([\d.]+) \| vfx ([\d.]+) \| camera ([\d.]+) \| audio ([\d.]+) \| ui ([\d.]+)\) \| render ([\d.]+) \| present ([\d.]+)")

            Add-Sample $key $label $res "cpu_mean" (Num $cpu 1)
            Add-Sample $key $label $res "cpu_p50" (Num $cpu 2)
            Add-Sample $key $label $res "cpu_p99" (Num $cpu 3)
            Add-Sample $key $label $res "cpu_p999" (Num $cpu 4)
            Add-Sample $key $label $res "cpu_max" (Num $cpu 5)
            Add-Sample $key $label $res "fps_avg" (Num $cpu 6)
            Add-Sample $key $label $res "fps_1low" (Num $cpu 7)
            Add-Sample $key $label $res "fps_01low" (Num $cpu 8)
            Add-Sample $key $label $res "gpu_mean" (Num $gpu 1)
            Add-Sample $key $label $res "gpu_p50" (Num $gpu 2)
            Add-Sample $key $label $res "gpu_p99" (Num $gpu 3)
            Add-Sample $key $label $res "hitches" (Num $hit 1)
            Add-Sample $key $label $res "hitch_pso" (Num $hit 2)
            Add-Sample $key $label $res "over_4ms" (Num $hit 4)
            Add-Sample $key $label $res "over_16ms" (Num $hit 6)
            Add-Sample $key $label $res "allocs_frame" (Num $alc 1)
            Add-Sample $key $label $res "bytes_frame" (Num $alc 2)
            if ($means.Success) {
                Add-Sample $key $label $res "update" (Num $means 2)
                Add-Sample $key $label $res "sim" (Num $means 3)
                Add-Sample $key $label $res "extract" (Num $means 4)
                Add-Sample $key $label $res "anim" (Num $means 5)
                Add-Sample $key $label $res "vfx" (Num $means 6)
                Add-Sample $key $label $res "camera" (Num $means 7)
                Add-Sample $key $label $res "ui" (Num $means 9)
                Add-Sample $key $label $res "render" (Num $means 10)
                Add-Sample $key $label $res "present" (Num $means 11)
            }
            if ($stageLines.Count -gt 0) {
                # One frame's sample, kept for the split of render it alone has.
                $f = $stageLines[$stageLines.Count - 1]
                Add-Sample $key $label $res "record" (Num $f 7)
                Add-Sample $key $label $res "shadow_prep" (Num $f 6)
            }
            if ($heads.Count -gt 0) {
                $h = $heads[$heads.Count - 1]
                Add-Sample $key $label $res "chars" (Num $h 4)
                Add-Sample $key $label $res "visible" (Num $h 9)
                Add-Sample $key $label $res "draws" (Num $h 10)
                Add-Sample $key $label $res "skinned" (Num $h 11)
                Add-Sample $key $label $res "tris" (Num $h 12)
            }
            if ($mems.Count -gt 0) {
                $m = $mems[$mems.Count - 1]
                Add-Sample $key $label $res "ram_ws" (Num $m 1)
                Add-Sample $key $label $res "ram_private" (Num $m 3)
                Add-Sample $key $label $res "vram" (Num $m 4)
            }
            if ($gpass.Success) {
                foreach ($name in $passNames) {
                    $p = [regex]::Match($gpass.Groups[1].Value, "\b$name ([\d.]+)/")
                    Add-Sample $key $label $res "gpu_$name" (Num $p 1)
                }
            }
            Write-Host ("pass {0}  {1,-28} cpu p50 {2,6:F3} p99.9 {3,6:F3} | gpu p50 {4,6:F3} | hitches {5}" -f
                $pass, $label, (Num $cpu 2), (Num $cpu 4), (Num $gpu 2), (Num $hit 1))
        }
    }
}

# Medians, one row per (resolution, configuration).
$rows = @()
foreach ($key in $samples.Keys) {
    $entry = $samples[$key]
    $row = [ordered]@{ resolution = $entry.resolution; config = $entry.config }
    foreach ($metric in $entry.metrics.Keys) {
        $values = [double[]]@($entry.metrics[$metric] | Where-Object { -not [double]::IsNaN($_) })
        $row[$metric] = Median $values
        if ($metric -eq "cpu_p50" -or $metric -eq "gpu_p50") {
            $row["runs"] = $values.Count
            if ($values.Count -gt 1) {
                $row["${metric}_spread"] = ($values | Measure-Object -Maximum).Maximum - ($values | Measure-Object -Minimum).Minimum
            }
        }
    }
    $rows += [pscustomobject]$row
}
# Group by resolution first so a table reads one resolution at a time.
$rows = @($rows | Sort-Object @{ Expression = { [int]($_.resolution.Split("x")[0]) } }, @{ Expression = { $_.config } })

Write-Host ""
if ($Title -ne "") { Write-Host "### $Title"; Write-Host "" }
if ($Mode -eq "res") {
    Write-Host "| Resolution | Configuration | CPU p50 | CPU p99 | GPU p50 | GPU p99 | shadow | atlas | prepass | contact | scene | particles | ui | FPS | draws | runs |"
    Write-Host "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    foreach ($r in $rows) {
        Write-Host ("| {0} | {1} | {2:F3} | {3:F3} | {4:F3} | {5:F3} | {6:F3} | {7:F3} | {8:F3} | {9:F3} | {10:F3} | {11:F3} | {12:F3} | {13:F0} | {14:F0} | {15} |" -f
            $r.resolution, $r.config, $r.cpu_p50, $r.cpu_p99, $r.gpu_p50, $r.gpu_p99, $r.gpu_shadow, $r.gpu_atlas,
            $r.gpu_prepass, $r.gpu_contact, $r.gpu_scene, $r.gpu_particles, $r.gpu_ui, $r.fps, $r.draws, $r.runs)
    }
} else {
    Write-Host "| Configuration | CPU mean | CPU p50 | CPU p99 | CPU p99.9 | max | avg FPS | 1% low | 0.1% low | GPU p50 | GPU p99 | hitches | >4.17 ms | runs |"
    Write-Host "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    foreach ($r in $rows) {
        Write-Host ("| {0} | {1:F3} | {2:F3} | {3:F3} | {4:F3} | {5:F3} | {6:F0} | {7:F0} | {8:F0} | {9:F3} | {10:F3} | {11} | {12} | {13} |" -f
            $r.config, $r.cpu_mean, $r.cpu_p50, $r.cpu_p99, $r.cpu_p999, $r.cpu_max, $r.fps_avg, $r.fps_1low,
            $r.fps_01low, $r.gpu_p50, $r.gpu_p99, $r.hitches, $r.over_4ms, $r.runs)
    }
    Write-Host ""
    Write-Host "Stage columns are run-wide means except shadow prep and record, which are one frame's sample from the last periodic report."
    Write-Host ""
    Write-Host "| Configuration | update | sim | extract | anim (chars) | vfx | camera | ui | render | shadow prep* | record* | present | draws (skinned) | tris | RAM ws | private | VRAM | allocs/frame |"
    Write-Host "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    foreach ($r in $rows) {
        Write-Host ("| {0} | {1:F3} | {2:F3} | {3:F3} | {4:F3} ({5:F0}) | {6:F3} | {7:F3} | {8:F3} | {9:F3} | {10:F3} | {11:F3} | {12:F3} | {13:F0} ({14:F0}) | {15:F0} | {16:F0} MiB | {17:F0} MiB | {18:F0} MiB | {19} |" -f
            $r.config, $r.update, $r.sim, $r.extract, $r.anim, $r.chars, $r.vfx, $r.camera, $r.ui, $r.render,
            $r.shadow_prep, $r.record, $r.present, $r.draws, $r.skinned, $r.tris, $r.ram_ws, $r.ram_private, $r.vram,
            $(if ([double]::IsNaN([double]$r.allocs_frame)) { "-" } else { "{0:F3}" -f $r.allocs_frame }))
    }
    Write-Host ""
    Write-Host "| Configuration | GPU shadow | atlas | prepass | contact | scene | particles | ui |"
    Write-Host "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    foreach ($r in $rows) {
        Write-Host ("| {0} | {1:F3} | {2:F3} | {3:F3} | {4:F3} | {5:F3} | {6:F3} | {7:F3} |" -f
            $r.config, $r.gpu_shadow, $r.gpu_atlas, $r.gpu_prepass, $r.gpu_contact, $r.gpu_scene, $r.gpu_particles, $r.gpu_ui)
    }
}
if ($Out -ne "") { $rows | ConvertTo-Json -Depth 4 | Out-File -Encoding utf8 $Out }
