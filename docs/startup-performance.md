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

### 3. The audio device was on the critical path for no reason

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

## What the remaining ~330 ms is

| Stage | Cost | Whose |
| --- | ---: | --- |
| D3D12 device, adapter, swap chain | ~110 ms | the driver |
| pipelines, heaps, shadow atlas, UI atlas | ~40 ms | the driver, mostly PSO creation |
| uploads (43 resources, compression, one wait) | ~45 ms | half driver, half ours |
| first frame | 55–115 ms | the driver: mesh pipeline variants are created on first use |
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
