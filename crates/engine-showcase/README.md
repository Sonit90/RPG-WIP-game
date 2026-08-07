# engine-showcase

A development-only visual test bed for the renderer. **Not part of the game.** Nothing
depends on this crate and a shipping build never links it.

It exists so the renderer can be *looked at* rather than only asserted about. Everything on
screen goes through the real engine — `engine_platform::Window`, the engine loop in
`engine_core::application`, `RenderWorld`, `Camera`, `MeshData`, `MaterialDesc` and
`Renderer::render_world` — so the picture and the numbers are evidence about the engine, not
about the showcase. There is no separate rendering path for the demo.

Scope stops at Milestone 15. No render list, no sorting, no batching, no instancing.

## Running

```
cargo run -p engine-showcase --release
```

Debug builds work but enable the D3D12 validation layer, which costs several times the frame
time; measure in release. `--help` lists the options:

```
--width <pixels>     client width (default 1920)
--height <pixels>    client height (default 1080)
--fullscreen         borderless fullscreen
--vsync              wait for vertical blank (off by default)
--validation         enable the D3D12 validation layer
--preset <1-5>       starting object count: 1=10, 2=100, 3=1000, 4=10000, 5=100000
--no-culling         start with frustum culling off
--frames <count>     exit after this many frames, for automated measurement
--report <seconds>   frame-time report interval (default 2.0, 0 disables)
```

## Controls

| Input | Action |
| --- | --- |
| `W` `A` `S` `D` | Move along the camera's own axes |
| `Q` / `E` | Move down / up, always world-vertical |
| `Shift` / `Ctrl` | Move six times faster / five times slower |
| Left click | Capture the mouse for look control |
| Mouse | Look, while captured |
| `Esc` | Release the mouse (press again to re-capture) |
| Mouse wheel | Field of view, 20° to 100° |
| `F1` | Freeze / release the cull frustum — the culling visualization |
| `F2` | Culling stress test: 100,000 objects, camera inside the field |
| `F3` `F4` `F5` `F6` `F7` | 10 / 100 / 1,000 / 10,000 / 100,000 objects |
| `F8` | Frustum culling on / off |
| `F9` | Wireframe on / off |
| `F10` | Statistics overlay on / off |
| `F12` | Reset the camera |

### F1, the culling visualization

Culling removes exactly the objects that are off screen, so with a live frustum there is
nothing to look at — only a counter to believe. `F1` freezes the cull frustum where the
camera is standing and lets the camera keep moving. Fly sideways or turn around and the
culled volume becomes a visible hole in the scene: everything outside the frozen frustum is
gone, and the overlay's `FROZEN ON` says why. `F1` again releases it.

`F8` is the other half of the comparison: it submits every object regardless of the frustum,
so the same scene can be measured with the step removed rather than against a different code
path.

## What the overlay reports

Every number comes from the engine. Frame times and stage averages are
`engine_core::metrics::FrameMetrics`, the visible and culled counts are
`Renderer::cull_stats`, the cull time is measured around the cull pass itself, and draw calls
and triangles are the command context's own counters. GPU time comes from GPU timestamps and
lags the CPU by a few frames; before the first one resolves the overlay prints `N/A` rather
than inventing a value.

The graph at the bottom left is the last 120 frame times. Full height is 4 ms (250 FPS); a
frame over that budget is clamped and drawn in orange, so a hitch shows as a coloured bar
instead of silently rescaling every other bar.

## Known limits

* **Upload memory.** The showcase reserves 40 MiB of per-frame upload memory instead of the
  4 MiB default. One root constant-buffer view per visible object at 256-byte alignment is
  about 25 MiB for 100,000 objects, which is what `F8` with the stress preset submits.
* **One draw call per object.** That is the Milestone 15 baseline, deliberately. The stress
  preset is a draw-call test, not a triangle-throughput test.
* **Only 96 objects animate.** Rewriting 100,000 transforms per frame would put the
  showcase's own cost inside every measurement of the renderer's.
* **The overlay font is uppercase only**, with digits and a little punctuation. Lowercase
  text renders as a hollow box.
