# Startup performance

Frame performance is `docs/performance.md`. This file is about the other number a player
notices: how long the game takes to show its first frame.

Reference machine: Ryzen 7 5800X, Radeon RX 6700 XT, Windows 10, 1920x1080. Every number here
was measured with the `startup-timing` feature on this machine; none of it is estimated.

## How to measure it

```bash
cargo run -p engine-app --features editor,startup-timing -- --editor --scene-file assets/scenes/showcase.json
```

The stopwatch is `crates/engine-app/src/startup.rs`: one global, a name and a duration per
stage, and a table logged once the first frame is on screen.

```text
[startup] D3D12 device            115.0 ms
[startup] renderer objects         36.6 ms
[startup] GPU upload: environment   26.1 ms
...
[startup] TOTAL                   368.2 ms
```

It is **not in a shipping build**. Without the `startup-timing` feature every function in that
module is an empty inline body, the renderer takes no clock, and `engine-renderer`'s
`upload_timing` counters are compiled out. The feature also forwards to
`engine-renderer/startup-timing`, which is what splits "D3D12 device" from "renderer objects"
and reports what an upload spent on creating resources, compressing and copying.

## Where it went, before and after

Debug build (`opt-level = 1`), minimum of three runs, `--frames 30`:

| Configuration | Before | After |
| --- | ---: | ---: |
| Engine + empty scene (`--scene character`) | 747 ms | 256 ms |
| Engine + `my_level.json` | 1163 ms | 302 ms |
| Engine + `showcase.json` | 1412 ms | 336 ms |
| Editor + empty scene | 765 ms | 269 ms |
| Editor + `my_level.json` | 1151 ms | 309 ms |
| Editor + `showcase.json` | 1474 ms | 340 ms |

Release build:

| Configuration | Before | After |
| --- | ---: | ---: |
| Engine + empty scene | 713 ms | 290 ms |
| Engine + `my_level.json` | 1110 ms | 277 ms |
| Engine + `showcase.json` | 1431 ms | 320 ms |
| Editor + empty scene | 732 ms | 263 ms |
| Editor + `my_level.json` | 1058 ms | 355 ms |
| Editor + `showcase.json` | 1310 ms | 310 ms |

`my_level.json` is an authored level with **no objects in it**; `showcase.json` is 27 objects,
a terrain, a cave built from files on disk and a lake, which is why it is the honest "loaded
level" row. Release is barely faster than debug, before or after — that is the first clue
below.

Stage by stage, editor + showcase, debug:

| Stage | Before | After |
| --- | ---: | ---: |
| D3D12 device and swap chain | 110 ms | 110 ms |
| renderer objects (pipelines, heaps, atlases) | 43 ms | 44 ms |
| shared uploads (character, decal, VFX, debug) | 174 ms | 12 ms |
| environment library | 334 ms | 31 ms |
| hero models | 251 ms | 45 ms |
| models named by the level, from disk | 234 ms | 18 ms |
| the GPU wait | — | 2 ms |
| audio device and sound bank | 126 ms | 0 ms (moved off the critical path) |
| first frame | 74 ms | 56 ms |
| **total** | **1397 ms** | **347 ms** |

## What was actually wrong

**It was never our arithmetic.** Authoring every asset the game owns — eighteen environment
models with their textures, three character models with skeletons and clips — is about 13 ms.
Block-compressing every texture is 15 ms in debug and 9 in release. Parsing a scene file is
0.3 ms. Building the simulation is 0.1 ms. Release being no faster than debug said the same
thing from another direction.

The cost was in *how* a resource reached the GPU.

### 1. A full GPU round trip per resource

Every `create_texture`, `create_mesh` and `create_material` built a `StagingUploader`,
submitted one copy and then waited for the queue to go idle. A showcase launch is a few
hundred resources, so it was a few hundred CPU-GPU round trips carrying about a megabyte in
total. The copies were free; the waiting was everything.

`Renderer::begin_upload_batch` / `end_upload_batch` opens a window in which every resource
queues its copy into one uploader and nothing waits. The application opens one around the
whole of startup and closes it before the first frame — 346 copies, one submission, one wait
of 2 ms. A resource created inside a batch is not readable until the batch ends, which is why
it is a load-time tool and the doc comment says so.

### 2. `CreateCommittedResource`, 893 times

With the waiting gone, the remaining time was resource creation: 893 of them, 431 ms, about
half a millisecond each. It is a driver allocation, which is why the optimizer had no opinion
about it.

Two changes, one on each side of a copy:

* **staging** — `StagingUploader` suballocates from 4 MB upload-heap chunks it keeps and
  reuses across flushes, instead of creating a buffer per copy. Writing is a bump and a
  `memcpy` into permanently mapped memory.
* **destinations** — `BufferArena` hands out `BufferSlice` ranges of 8 MB device-local
  chunks. A mesh and a material hold a range rather than a resource; a view or a root CBV is
  the chunk's address plus an offset, aligned to the vertex stride, the index element or 256
  bytes as the case needs. A chunk is read as vertices, indices and constants at once, so its
  state is the union of the three read states.

Together: **893 resources → 43**, and the buffers among them 329 → 11.

`BufferSource` is what kept this from forking the code — the renderer passes its arena, a test
or a one-off upload asks for a dedicated resource, and `GpuMesh::upload` and `Material::new`
still mean what they did.

### 3. Uploads that happened after the batch closed

The terrain, the water and the editor's spawn markers are built after the level is placed, and
the first version of the batch closed before them — so each of those meshes went up on its own
and waited for the GPU, on the first frame, where it read as "rendering is slow". The batch now
closes after them. The editor's re-place path is batched too, which is felt on every edit and
not only at startup.

### 4. The audio device was on the critical path for no reason

Enumerating the default output device and synthesising the 8.5 MiB sound bank is about 120 ms,
and nothing about the first frame depends on it: no cue plays until something happens in the
game. `AudioHost::open_in_background` builds the `AudioSystem` on the game thread, empty, and
hands it to a thread that picks the device, synthesises the bank at the device's own rate,
installs it into that same system and then holds the stream — a `cpal::Stream` is not `Send`,
so the thread that built it is the thread that keeps it alive.

The application polls once a frame; the frame it arrives on is the frame the player's volumes
are applied. Because the system behind the mutex never changes identity, a volume change, `F8`
or a listener move that happened meanwhile is not lost. On this machine the device is playing
about 120 ms after the first frame is on screen.

## The two things left, measured

The first pass left two items nobody had looked inside: "D3D12 device" and "first frame".
Both were opened with the same feature, and both answers were surprising enough to be worth
writing down.

### D3D12 device: 117 ms, of which `D3D12CreateDevice` is 102

```text
[startup] D3D12: factory 1.8 ms, 2 adapter(s) enumerated 0.1 ms,
          D3D12CreateDevice 102.4 ms, queue and caps 4.5 ms
```

Creating the DXGI factory is 1.8 ms. Enumerating both adapters and reading their descriptions
is 0.1 ms. Creating the command queue and asking for the highest feature level is 4.5 ms. The
swap chain is the balance, about 9 ms. **`D3D12CreateDevice` alone is 102 ms**, and there is
nothing of ours inside it: it is the D3D12 runtime and the vendor's user-mode driver being
loaded and initialized on the first call in the process.

That number cannot be reduced from here. It can only be *hidden*, and the ceiling on hiding it
is however much of our own work can be made to happen alongside — see below.

### First frame: 57 ms, and it was not what it looked like

Split into the three stages the engine already has, the first frame was:

| Stage | Cost |
| --- | ---: |
| update (events, simulation, extraction, the editor's first frame, the UI) | 33 ms |
| record (the whole render list into a command list) | 12 ms |
| present | 12 ms |

The suspicion was pipeline state objects: mesh pipelines are created the first time a material
variant is drawn, so a first frame pays for them. The counter says **7 pipelines, 12 ms in
total for the whole process**, most of them at renderer init — so that was not it.

What it actually was: the terrain mesh, the water quads and the editor's spawn markers are
uploaded *after* the startup batch had already been closed, each one waiting for the GPU on
its own. The counters show it plainly — 43 resources and 6 flushes at the end of the batch, 53
resources and 14 flushes by the time the first frame was on screen.

Closing the batch after the terrain and the water are built, rather than before, took the
first frame's update from 33 ms to 13 ms. The editor's own re-place path — which runs on every
edit, not only at startup — is batched the same way now, so placing an object no longer
stalls once per mesh.

Debug, editor + showcase: **347 ms → ~335 ms**, and the first frame is 37 ms of it: 13 update,
12 record, 12 present. What is left there is command recording and the driver's first present,
neither of which is a stall we are choosing.

## What the remaining ~330 ms is

| Stage | Cost | Whose |
| --- | ---: | --- |
| `D3D12CreateDevice` | ~102 ms | the driver, on the first call in the process |
| factory, adapters, queue, swap chain | ~15 ms | the driver |
| pipelines, heaps, shadow atlas, UI atlas | ~40 ms | the driver, mostly PSO creation |
| uploads (44 resources, compression, one wait) | ~45 ms | half driver, half ours |
| first frame (update, record, present) | ~37 ms | mostly the driver |
| everything the game itself does | ~15 ms | ours |

The game's own work — reading the scene, authoring models, building the simulation, settings,
HUD, the editor's asset scan — is now about 15 ms of a 330 ms launch. Everything else is
D3D12 asking the driver for something.

## What was considered and not done

* **Uploading in parallel on several threads.** The obvious next step, and the measurement
  says it is now worth about 20 ms: what is left on the CPU side is 15 ms of block
  compression and a few milliseconds of glTF import. It also cannot be done as the renderer
  stands — a D3D12 resource handle is neither `Send` nor `Sync` here, and the arena, the
  uploader and the handle tables all belong to the thread that owns the device. Worth
  revisiting only if content grows by an order of magnitude, at which point the compression
  is the part to move, not the uploading.
* **Creating the device on a worker thread, to hide it behind our own loading.** A D3D12
  device handle and a DXGI factory are both `Send` and `Sync` in `windows-rs` — there is a
  compile-check test in `device.rs` that says so, because the answer decides whether this is
  even possible. It is: a thread could call `RenderDevice::new` while the main thread creates
  the window, reads the settings and the scene file, authors the environment library and the
  hero models on the CPU, and imports the level's `.glb` files.
  The ceiling is what that CPU work adds up to, because the main thread has to wait for the
  device before it can upload any of it: about 20–25 ms of a 335 ms launch. It also means
  reordering `App::new` so that authoring happens before uploading rather than interleaved
  with it. Recorded rather than done: 7% for a restructure of the startup path is a trade
  worth making deliberately, not in passing.
* **Creating pipeline state objects in parallel.** The largest remaining item after device
  creation is the first frame, and most of it is mesh pipeline variants being created the
  first time a material needs one. `CreateGraphicsPipelineState` is free-threaded, so warming
  the variants a level needs on worker threads during startup could hide most of it —
  but it needs an explicit `Send` assertion around the device, which is a change to how this
  renderer thinks about threads and wants its own decision rather than being smuggled in with
  an upload change.
* **Uploading only the environment assets a level names.** The whole 18-model library goes up
  even for an empty level. That was 300 ms and is now 26; the saving no longer justifies
  making "what a level needs" a question the loader has to answer.
* **Caching compressed textures on disk.** 9 ms in release, for a cache that can go stale.

## The 2026-10 audit: one second again, and back to a quarter of one

By the time of the audit the launch was ~1.0 s again (Ryzen 9 5900X, warm, median of 9):
945 ms for an empty scene, 997 for the dungeon, 1022 for the graveyard. The cause was new
content on an old path — the imported Fire Mage's two 2048x2048 textures went through
`create_texture` on the main thread, after the device existed:

| Inside `GPU upload: heroes` (dungeon) | Before |
| --- | ---: |
| mip filtering, `powf` for every sRGB conversion | 385 ms |
| BC1/BC5 compression on one core | 263 ms |
| byte-at-a-time content hash and alpha scan | 46 ms |
| import (two JPEG decodes) and the rest | ~5 ms + ~40 ms |

What changed, each measured alone and interleaved (`docs/PERFORMANCE_AUDIT.md`, 12.1-12.7):

* Mip filtering reads two tables built from the same formulas — exact for every float, proven
  by an exhaustive test — and splits each level into bands of rows across threads.
* Block compression splits into bands of block rows.
* The content hash reads a word at a time.
* `ModelPreload` builds and imports the hero, monster and environment models on a thread that
  starts before the window, and `engine_renderer::prepare_model_textures` does their mips,
  compression and hashing there too. The main thread finds it all finished when it reaches the
  uploads; it waited 0.0 ms in every measured launch. The thread is capped at eight bands so the
  driver initializing inside `D3D12CreateDevice` keeps its cores.

The texture bytes the GPU receives are unchanged: `MYGAME_TEXTURE_DIGEST=1` folds every
uploaded level into one digest, and it is the same before and after every step.

| Launch (warm, 9 runs) | Before | After |
| --- | ---: | ---: |
| Empty | 945 ms | 223 ms |
| Dungeon | 997 ms | 241 ms |
| Graveyard | 1022 ms | 262 ms |
| Editor + showcase (7 runs) | 1037 ms | 282 ms |

What is left, dungeon: `D3D12CreateDevice` and the swap chain ~128 ms (the driver), renderer
objects 45 ms (the UI state's synchronous flush and pipelines, the shadow renderer), the map
9 ms, and ~40 ms of uploads. The considerations above about hiding the device still hold —
except that the CPU work worth hiding behind it is now ~160 ms, and it is hidden.

`tools/bench/startup_matrix.ps1` runs these comparisons: interleaved, per-stage medians, and
the first launch of each binary reported separately as "cold" (a freshly copied executable's
first launch is slower for reasons outside the engine, consistent with antivirus scanning).
