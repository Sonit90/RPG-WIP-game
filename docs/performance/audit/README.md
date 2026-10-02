# Performance audit data

The measurements behind `docs/PERFORMANCE_AUDIT.md`, as the drivers wrote them. Every file
is the median of interleaved repetitions; the number of repetitions is in each file (`runs`
for the frame matrices, `runs` per configuration for the startup ones).

Machine: Ryzen 9 5900X (12 cores, 24 threads), Radeon RX 6700 XT, Windows 10 19045,
1920x1080 desktop at 240 Hz. Every game run is `--hidden`: no window on screen, so nothing is
composited by DWM — see the audit's methodology section for what that does and does not
measure.

| File | Driver | What it is |
| --- | --- | --- |
| `baseline-res.json` | `frame_matrix.ps1 -Mode res` | The unoptimized build at four resolutions, ten scenarios, before any change |
| `baseline-pacing.json` | `frame_matrix.ps1 -Mode pacing` | The unoptimized build, 6,000-frame runs at 1080p: distributions, stage split, memory |
| `baseline-alloc.json` | `frame_matrix.ps1 -Mode pacing` | The unoptimized build with `alloc-count`: allocations per frame |
| `startup-opt1-dungeon.json` | `startup_matrix.ps1` | Table-and-band mip filtering and banded block compression |
| `startup-opt2.json` | `startup_matrix.ps1` | ...plus the word-at-a-time content hash, three scenes |
| `opt3-ui-threshold.json` | `frame_matrix.ps1 -Mode pacing` | Parallel UI batching threshold, before and after |
| `opt5-particle-threshold.json`, `opt5-particle-threshold-stress.json` | `frame_matrix.ps1 -Mode pacing` | Parallel particle threshold, before and after |
| `opt6-palettes.json` | `frame_matrix.ps1 -Mode pacing` | Bone palettes uploaded once per frame, before and after |
| `startup-opt7.json` | `startup_matrix.ps1` | Hero and monster models built on a thread during device creation |
| `startup-opt8.json` | `startup_matrix.ps1` | ...plus their textures prepared on that thread |
| `startup-opt10.json` | `startup_matrix.ps1` | ...plus the environment library |
| `final-pacing.json` | `frame_matrix.ps1 -Mode pacing` | The unoptimized and the final build interleaved in one session, 1080p |
| `final-res.json` | `frame_matrix.ps1 -Mode res` | The same two builds at four resolutions |
| `final-startup.json` | `startup_matrix.ps1` | The same two builds' launch, three scenes |
| `final-editor-startup.json` | `startup_matrix.ps1` | The editor's launch: pristine `HEAD` against the final build |
| `final-alloc.json` | `frame_matrix.ps1 -Mode pacing` | The final build with `alloc-count` |

Raw logs are not kept here — several megabytes of them per matrix. Re-running a driver
reproduces them under `target/`.

## Reproducing

```powershell
# Builds: the measurement build, and the one that counts allocations.
cargo build --release --bin mygame --features startup-timing
cargo build --release --bin mygame --features startup-timing,alloc-count

# A frame matrix: configurations are "label|binary|arguments", interleaved.
.\tools\bench\frame_matrix.ps1 -Mode pacing -Repeats 3 -Configs "dungeon||--scene dungeon;fight||--scene monsters --spawn all --spawn-hostile --autocast"
.\tools\bench\frame_matrix.ps1 -Mode res -Repeats 3 -Configs "arena||--scene combat --autocast"

# Startup, cold launch reported separately.
.\tools\bench\startup_matrix.ps1 -Repeats 7 -Configs "dungeon||--scene dungeon"
```
