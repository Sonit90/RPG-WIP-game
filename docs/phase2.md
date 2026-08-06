# Phase 2 — High-performance D3D12 renderer foundation

Phase 2 turns the Phase 0 spike into a renderer: resource management, synchronization,
descriptors, camera, culling, batching, instancing, depth and one directional light — all
measured against frame-time budgets rather than average FPS.

The objective is **not** beautiful graphics. It is a foundation whose cost is known.

Frame budgets on the target machine (Ryzen 7 5800X, RX 6700 XT, 1920×1080):

| Goal | FPS | Frame |
| --- | --- | --- |
| Minimum | 240 | 4.17 ms |
| Target | 360 | 2.78 ms |
| Stretch | 480 | 2.08 ms |

Architectural targets for a simple scene: CPU renderer overhead < 1 ms, GPU < 1 ms.

Milestone status:

| # | Milestone | Status |
| --- | --- | --- |
| 0 | Architecture proposal | done |
| 1 | Renderer device abstraction | done |
| 2 | Frame resources and synchronization | done |
| 3 | Command context | done |
| 4 | Descriptor management | todo |
| 5 | Resource abstraction | todo |
| 6 | Upload system | todo |
| 7 | Shader system | todo |
| 8 | Camera | todo |
| 9 | Transform system | todo |
| 10 | Mesh system | todo |
| 11 | Basic material | todo |
| 12 | Depth buffer | todo |
| 13 | 3D scene | todo |
| 14 | Basic lighting | todo |
| 15 | Frustum culling | todo |
| 16 | Render list | todo |
| 17 | Instanced rendering | todo |
| 18 | Batching | todo |

---

# Architecture proposal

Written before implementation, as required. Each section states the alternatives that were
considered, the choice, and why. Where a decision is reversible cheaply, it says so.

## What Phase 0 and Phase 1 already provide

Inventory taken from the working tree, not from memory:

| Exists | Where | Phase 2 disposition |
| --- | --- | --- |
| `GpuContext` — factory, adapter, device, queue, tearing support | `device.rs` | Keep; becomes the device layer's core |
| `SwapChain` — 3 buffers, flip-discard, RTV heap, resize | `swapchain.rs` | Keep; RTV heap moves to the descriptor allocator |
| `GpuFence` — signal, wait, wait-for-idle | `sync.rs` | Keep; frame slots grow around it |
| `GpuTimer` — per-slot timestamp pairs, lag-free read | `gpu_timer.rs` | Extend from 1 pair to N regions per frame |
| `GpuMesh` — vertex-only upload | `mesh.rs` | Rewrite: index buffers, bounds, vertex format |
| `ScenePipeline` / `InstancedPipeline` — root sig + PSO, HLSL via `include_str!` | `pipeline.rs`, `instancing.rs` | Replaced by the shader/pipeline system |
| Offscreen render + readback | `offscreen.rs` | Keep; the basis of every GPU test |
| Bench harness, stress grid | `bench.rs`, `stress.rs` | Keep until the new benchmark scenes replace them |
| Debug overlay | `overlay.rs`, `font.rs` | Keep; gains the renderer statistics block |
| Engine lifecycle, time, input, memory, jobs, profiler, config | `engine-core` | Consumed, not changed |

What is missing and must be built: depth, math, camera, transforms, descriptor
allocation beyond the RTV heap, an upload path that is not one-resource-per-mesh, resource
state tracking, materials, `RenderWorld`, culling, render lists, batching.

## 1. Renderer API

The public surface stays small and handle-based. Nothing outside `engine-renderer` sees a
D3D12 interface.

```rust
let mesh     = renderer.create_mesh(&MeshData)?;      // MeshHandle
let material = renderer.create_material(&MaterialDesc)?; // MaterialHandle
let entity   = world.spawn(RenderEntityDesc { mesh, material, transform, .. });
renderer.render(&world, &camera)?;
renderer.present()?;
```

**Alternatives considered.** A retained scene graph inside the renderer (rejected: it
duplicates the simulation's structure and dictates how gameplay stores entities); an
immediate-mode `draw(mesh, material, transform)` API (rejected: it forecloses culling and
sorting, which need the whole set before recording).

**Chosen:** the renderer consumes a `RenderWorld` snapshot each frame and owns everything
else. This is the boundary Phase 0's architecture document already committed to.

## 2. Handles

```rust
pub struct MeshHandle(Handle);      // index: u32, generation: u32
pub struct MaterialHandle(Handle);
pub struct RenderEntity(Handle);
```

Index plus generation, packed in 8 bytes, `Copy`, no pointer. A stale handle is *detected*
(generation mismatch) rather than dereferenced. Distinct newtypes so a `MeshHandle` cannot
be passed where a `MaterialHandle` belongs.

**Alternatives.** Raw indices (rejected: a freed-and-reused slot is silently wrong);
`Arc<Mesh>` (rejected: shared ownership across the CPU/GPU boundary hides lifetime from
the fence logic, which is exactly what must stay explicit); a full generational asset
database (deferred: there is no asset loading yet, and §6 says not to build one).

## 3. Frame lifecycle

```text
begin_frame
  wait for this slot's fence          <- only blocks when the CPU is a full lap ahead
  read this slot's GPU timestamps     <- free: the wait already proved they resolved
  reset allocator, reset upload ring, reset per-frame descriptors
prepare                                <- CPU only, no D3D12 calls
  RenderWorld -> cull -> RenderList -> sort -> batch
record
  CommandContext: barriers, clears, per-batch draws
submit
present
signal fence for this slot
```

Preparation touches no D3D12 object, which keeps it trivially testable without a GPU and
leaves the door open to running it on worker threads later.

## 4. Frames in flight

Three slots, matching the swap chain, configurable. Each slot owns: a command allocator, a
slice of the upload ring, a slice of the per-frame descriptor heap, a GPU timestamp region,
and a fence value.

**Why 3.** Two is enough to overlap CPU and GPU; three absorbs a variable frame without
stalling, at the cost of one more frame of latency. Phase 0 already runs 3 back buffers, so
this keeps the pacing identical and the regression comparison honest. The number is a
constant in one place.

## 5. Command context

A thin wrapper over `ID3D12GraphicsCommandList` exposing only what the engine records:
begin/end, transitions, clear, set pipeline, set root constants and tables, set vertex and
index buffers, draw and draw-indexed-instanced. It also owns **barrier batching** (collect
transitions, flush as one `ResourceBarrier` array) and increments the statistics counters.

**Alternatives.** A full wrapper over the whole D3D12 command list API (rejected by §9:
wrap only what is used); no wrapper at all (rejected: barrier batching and statistics need
one place to live).

## 6. Descriptors

Three kinds of allocator, because the three uses have different lifetimes:

| Heap | Type | Allocation | Reset |
| --- | --- | --- | --- |
| RTV | CPU-only | free-list, on resource creation | on release |
| DSV | CPU-only | free-list | on release |
| CBV/SRV/UAV — persistent | shader-visible, low region | free-list, on material creation | on release, deferred by frames-in-flight |
| CBV/SRV/UAV — per frame | shader-visible, high region split into 3 slices | **linear bump** | whole slice reset at `begin_frame` |

The per-frame region is the answer to §11: a per-draw descriptor costs a bump of an offset,
and reclamation is one store per frame. Nothing walks a free list per object.

**Alternatives.** A single general-purpose allocator for everything (rejected: per-draw
descriptors have frame lifetime and do not need free-list bookkeeping); full bindless
(deferred by §10 — the persistent region is already laid out so it can become one heap of
SRV indices later without moving the per-frame region).

## 7. Resources and state tracking

`Buffer`, `Texture`, `RenderTarget`, `DepthBuffer`, each owning its `ID3D12Resource` and its
**current state**. Transitions go through the `CommandContext`, which skips a barrier when
the state already matches and batches the rest.

Tracking is per-resource, not per-subresource, and single-queue. That covers every case the
renderer has; the limitation is documented rather than engineered around.

**Alternatives.** A render graph deriving barriers automatically (explicitly out of scope,
§46 — and it cannot be justified before there are multiple passes); no tracking at all,
transitioning blindly every frame (rejected by §13, and it costs real GPU time).

## 8. Upload

One persistent upload buffer per frame slot, permanently mapped, used as a linear ring:
per-object constants, instance data, and staging for mesh and texture uploads all bump the
same offset. Static mesh data is copied once into a `DEFAULT`-heap buffer at creation.

**Alternatives.** A committed upload resource per upload (what Phase 0's `GpuMesh` does —
fine for one mesh at startup, unacceptable per frame); a general suballocator (deferred:
the ring covers per-frame data, which is all Phase 2 uploads per frame).

The ring's high-water mark is reported in the statistics, so its size is chosen from
measurement.

## 9. Shaders

HLSL lives in `assets/shaders/`. A **build script compiles it to DXIL with `dxc`** and the
bytecode is embedded in the binary. Nothing is compiled at runtime.

**Consequences, stated plainly:**

* Shader errors become *build* errors, with file, line and the compiler's own diagnostic.
* Startup does no compilation work at all.
* The build machine needs `dxc.exe`. It ships with the Windows SDK; the build script
  searches `PATH` then the Windows Kits directories, and fails with an actionable message
  if it is absent. Verified present on this machine (SDK 10.0.17763, x64, with `dxil.dll`
  for signing).
* Shader model 6.0. The RX 6700 XT and every D3D12 driver of interest support DXIL.
* Iterating on a shader means a rebuild. `cargo build` on this workspace is ~3 s, so this
  is acceptable; if it stops being acceptable, a debug-only disk-loading path can be added
  behind `cfg(debug_assertions)` without changing the release path.

## 10. Math

`engine-core::math`, written here: `Vec2/3/4`, `Mat4` (column-major), `Quat`, `Sphere`,
`Aabb`, `Plane`, `Frustum`. No third-party dependency, consistent with the engine's own
logger, allocators and job system.

Conventions, fixed now and documented because getting them wrong is the classic source of
"everything renders inside-out":

* **Right-handed** view space, looking down −Z.
* Projection maps depth to **[0, 1]**, which is what D3D12 expects.
* Column-major storage, column-vector convention: `clip = P * V * M * v`.
* Row-major upload to HLSL is handled by transposing at upload, or by declaring the
  constant buffer `column_major`; the choice is made once, in the shader system, and tested.

§18 requires the test suite regardless of where the math comes from: identity, associativity,
`V * V⁻¹ = I`, projection of known frustum corners, orientation from a quaternion, and
frustum plane extraction.

## 11. RenderWorld

Structure-of-arrays, owned by the caller, consumed by the renderer:

```rust
pub struct RenderWorld {
    transforms: Vec<Transform>,      // translation, rotation (Quat), scale
    meshes:     Vec<MeshHandle>,
    materials:  Vec<MaterialHandle>,
    bounds:     Vec<Sphere>,         // local-space, scaled at cull time
    flags:      Vec<RenderFlags>,    // visible, casts shadow (later), ...
    layers:     Vec<RenderLayer>,
    generations: Vec<u32>,           // handle validation
    free_slots: Vec<u32>,
}
```

Culling reads `bounds`, `flags` and `transforms` — three contiguous arrays, no pointer
chasing, no gameplay type in sight. Transform is stored as translation/rotation/scale and
matrices are generated when needed, per §19.

**Alternatives.** An ECS (out of scope, §32); one `Vec<RenderEntity>` of fat structs
(rejected: culling would stream mesh handles, materials and layers it does not read, for no
benefit).

## 12. Culling

Bounding sphere against six frustum planes extracted from the view-projection matrix
(Gribb–Hartmann). Sphere-only to start, per §28. The loop is flat, allocation-free, and
writes visible indices into a pre-allocated `Vec`.

Threading is **not** applied yet. `docs/threading.md` measured the break-even at roughly
12 µs of work per job; a culling chunk must therefore be thousands of objects before it is
worth submitting, and whether culling even costs that much is unknown until it is measured.
The loop is written so chunking is a later change, not a rewrite.

## 13. Render list, sorting, batching

```text
RenderWorld -> cull -> RenderList -> sort -> batch -> record
```

Each visible object produces a `DrawItem` with a 64-bit sort key:

```text
[ layer 8 | pipeline 12 | material 20 | mesh 16 | depth 8 ]
```

Sorting by that key groups pipeline changes first, then materials, then meshes — so a
single linear pass over the sorted list collapses runs sharing (pipeline, material, mesh)
into one instanced draw. Batching is therefore a consequence of sorting, not a separate
system.

§31 requires measuring whether sorting actually helps. The benchmark scenes include a
state-change scene (G) precisely so sorted and unsorted submission can be compared.

## 14. Instancing

Per-batch instance data (transform, and later per-instance colour) is written into the
frame's upload ring and bound as a second vertex stream. One `DrawIndexedInstanced` per
batch. Phase 0 already measured that this is the difference between 2.65 ms and 0.014 ms of
CPU at 100,000 objects; Phase 2's contribution is making it automatic rather than a special
path.

## 15. Statistics and overlay

`RendererStats`: CPU/GPU frame time, renderables, visible, culled, draw calls, instanced
draws, triangles, PSO changes, descriptor allocations, barriers, upload-ring high water,
GPU memory where measurable. The Phase 1 debug overlay grows a renderer block. It remains
behind the `debug-overlay` feature, so `--no-default-features` strips it.

## 16. Profiling

CPU scopes (Phase 1 profiler) around: frame begin, fence wait, world preparation, culling,
sorting, batching, command recording, submission, present. GPU timestamp regions around
the depth pass and the main pass, via a `GpuTimer` extended from one pair to several.

## 17. Testing

| Kind | Content |
| --- | --- |
| Unit, no GPU | math, transforms, camera, frustum extraction, culling cases, handle validity, RenderWorld add/remove/update, sort key ordering, batch grouping, instance data |
| GPU, offscreen readback | clear, triangle, depth ordering, lighting, instanced vs individual equivalence |
| Multi-frame | three frames in flight, allocator reuse, fence completion, resource still valid after reuse, deferred destruction |
| Benchmarks | scenes A–G, deterministic |

Every GPU test uses the existing offscreen readback path — no window, no compositor,
deterministic. The process-wide GPU lock from Phase 0 still applies.

## 18. What is deliberately not built

Render graph, GPU culling, `ExecuteIndirect`, mesh shaders, full bindless, clustered
shading, shadows, PBR, HDR, post-processing, skeletal animation, texture streaming,
asset loading. Each is listed in the spec as out of scope, and none is needed to establish
whether the CPU-driven baseline meets the frame budget.

## 19. Risks

| Risk | Mitigation |
| --- | --- |
| `dxc` absent on a build machine | Build script fails with an actionable message naming the SDK component |
| Reverse-Z not adopted now, wanted later | Depth format and comparison live in one place; the change is the projection matrix, the clear value and one PSO field |
| The renderer refactor regresses Phase 0 frame time | The Phase 0 stress scene and its 0.166–0.184 ms CPU band are re-measured after every milestone |
| Descriptor or upload ring too small under load | Both report high-water marks; both fail loudly rather than silently wrapping |

---

# Milestone 1 — Renderer device abstraction

## What was built

`GpuContext` became `RenderDevice`, and **every `ID3D12Device::Create*` call in the crate
now goes through it**:

| Method | Replaces raw calls in |
| --- | --- |
| `create_command_allocator` | `renderer.rs`, `bench.rs`, `mesh.rs` |
| `create_command_list` | `renderer.rs`, `bench.rs`, `mesh.rs` |
| `create_buffer` | `mesh.rs` (was a private `create_buffer` free function) |
| `create_descriptor_heap` | ready for Milestone 4 |
| `create_fence` | `sync.rs` |
| `descriptor_size` | ready for Milestone 4 |

`GpuFence::new` and `GpuMesh::upload` / `upload_raw` now take a `&RenderDevice` instead of
a `&ID3D12Device` plus a `&ID3D12CommandQueue`. Passing both separately was an invitation
to pass a queue from one device and a device from another — a class of bug that no longer
has a call signature to live in.

## Why this is worth doing rather than leaving alone

Object creation is where `unsafe`, `HRESULT` mapping and cross-object agreement
(command-list type, heap flags, initial resource state) all meet. Spread across five files
it has to be audited in five places; the milestones that follow add descriptors,
resources, upload buffers, depth textures and pipelines, each of which is another
`Create*` call. Centralizing before adding them is cheaper than after.

`create_command_list` also returns a list that is already **closed**. D3D12 hands back an
open one, and every call site had its own "close it so the frame path can start with
`Reset`" comment. That trap is now gone from every call site.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Application still starts | pass |
| Phase 0 triangle still renders | pass — offscreen pixel tests unchanged and passing |
| All existing tests pass | pass — 302 tests, no `SKIP:` lines |
| No regression in Phase 0 | pass — 60,000 frames at 1000 triangles, CPU mean 0.174 ms against the 0.166–0.184 ms band |
| `engine-core` still has no graphics dependency | pass |

## Known limitations

* `RenderDevice` still exposes `device()` and `queue()`, and the swap chain, GPU timer,
  pipelines and offscreen path still use them directly. Removing those accessors entirely
  is not the goal — the goal is that *creation* is centralized. Direct recording calls
  legitimately need the raw objects.
* One queue, one device, one adapter. Multi-queue (a copy queue for uploads) is a later
  question and would change the fence model; nothing here forecloses it.

---

# Milestone 2 — Frame resources and synchronization

## What was built

`engine-renderer::frame::FrameRing` owns the frame slots and the fence that paces them.
Each slot holds a command allocator and the fence value signalled after its last
submission. `RendererConfig::frames_in_flight` selects the slot count (default 3, bounded
by the swap chain's buffer count and **rejected** rather than clamped when out of range).

The renderer's per-frame path became:

```rust
let slot = self.frames.begin_frame()?;   // waits if needed, resets the allocator
// ... record ...
self.frames.end_frame(queue, slot)?;     // signals, advances to the next slot
```

## Frame slot is not the back-buffer index

Phase 0 used the swap chain's current back-buffer index as the frame slot. That is only
correct while the two counts are equal *and* the presentation model hands them out in the
same order — an assumption that costs nothing to remove and is invisible when it breaks.
The ring now owns a monotonic frame counter and derives its slot from it; the back buffer
is looked up separately, and a `debug_assert` catches the swap chain advancing between
recording and presenting.

## The wait is measured, and the measurement is free

`SyncStats` counts stalled frames, total and maximum stall time. The fence's completed
value is read first, and the clock is only sampled on the path that genuinely blocks, so a
frame that does not wait pays one atomic load and never touches `Instant::now`.

## What the measurement says

Release build, 20,000 frames, 1920×1080, three frames in flight:

| Scene | CPU mean | GPU mean | Frames that stalled |
| --- | --- | --- | --- |
| 1,000 triangles | 0.167 ms | 0.051 ms | 3 / 20,000 (0.02%) |
| 100,000 triangles | 0.129 ms | 0.069 ms | 3 / 20,000 (0.02%) |

**The renderer is CPU-bound, not GPU-bound.** The CPU essentially never waits for the GPU;
the GPU finishes in a third of the time the CPU takes to prepare and submit the frame.
This confirms from the synchronization side what Phase 0 found from the timing side —
`Present` and submission dominate — and it sets the target for the rest of Phase 2: work
that reduces CPU submission cost will move the frame time, and work that reduces GPU cost
will not.

It also means the frame-in-flight count is not currently doing anything for throughput
here. It is still correct to have: it costs nothing when the CPU is ahead, and the moment
the GPU workload grows — depth, lighting, thousands of objects — it is what stops the CPU
from serializing against it.

## Tests

`crates/engine-renderer/tests/frame_sync.rs`, seven tests against a real device with no
window and no swap chain:

| Test | Asserts |
| --- | --- |
| `slots_cycle_and_allocators_are_reused` | slot sequence `0,1,2,0,1,2,…`, allocators reset safely across laps |
| `trivial_work_never_makes_the_cpu_wait` | one slot, empty lists → **zero** stalls |
| `one_slot_stalls_on_every_frame_under_real_gpu_load` | one slot, 64 MiB copies → stalls on every frame after the first |
| `three_slots_absorb_the_first_frames_without_waiting` | first lap cannot stall; second lap must |
| `fence_values_advance_and_complete` | monotonic values, all passed after an idle wait |
| `releasing_slots_lets_the_next_frames_run_without_waiting` | the resize path frees slots correctly |
| `an_out_of_range_frame_count_is_rejected` | 0 and `MAX+1` rejected, 1..=MAX accepted |

The second and third tests exist because **the first version of this suite asserted the
wrong thing**: it assumed a single-slot ring must serialize, and it failed. Empty command
lists complete before the CPU can get back around the loop, so no wait ever happens. What
serializes is one slot *plus GPU work that outlasts a CPU iteration* — hence the buffer-copy
load, and hence a test that now pins down both halves of that statement.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Multiple frames in flight | pass — configurable 1..=3, default 3 |
| Fence synchronization | pass |
| Command allocator recycling | pass — one per slot, reset after its fence passes |
| Frame index | pass — monotonic, owned by the ring |
| CPU/GPU synchronization | pass |
| Safe resource reuse | pass — a slot is only reused after the GPU passes its fence |
| CPU does not block unnecessarily | pass — measured: 3 stalls in 20,000 frames |
| Synchronization stalls measured | pass — `SyncStats`, reported at shutdown |
| No regression | pass — 0.168 ms CPU mean against the 0.166–0.184 ms band |

## Known limitations

* Per-slot *transient resources* are not here yet: a slot owns an allocator and a fence
  value, but the upload ring and descriptor slices arrive in Milestones 4 and 6. The ring
  is the place they will hang from.
* No deferred destruction queue yet. Destroying a resource still means `wait_for_idle`,
  which is correct but fully serializing. Milestone 5 addresses it.
* The stall statistics accumulate for the process lifetime; there is no per-interval view.
  The overlay in Milestone 15 will want one.

---

# Milestone 3 — Command context

## What was built

`engine-renderer::command::CommandContext` is now the only place the engine talks to a
D3D12 command list. It wraps `begin`, `transition`, `flush_barriers`, `set_render_target`,
`clear_render_target`, `set_viewport`, `set_pipeline`, `set_topology`, `set_vertex_buffer`,
`draw`, `close` and `reset_to_empty` — and nothing else. A wrapper method with no caller
has no test and no reason to be correct.

What it adds over the raw list is the part worth centralizing:

| Feature | Why |
| --- | --- |
| **Barrier batching** | Transitions queue and are submitted as one array. Each `ResourceBarrier` call has a fixed driver cost; one call with four barriers beats four with one. |
| **Automatic flushing** | Clear, draw and close flush first. A queued barrier that was never submitted has not happened, and forgetting produces a validation error at best. |
| **Redundant bind elimination** | Binding the pipeline, root signature or topology that is already bound is a driver call that changes nothing. The context skips it and counts it. |
| **Statistics** | Draw calls, instances, triangles, pipeline changes and barriers counted where they happen, not estimated by the caller. |

`transition_barrier`, `viewport_for` and `scissor_for` moved here from `renderer.rs`,
which is where the rest of the crate already imported them from.

## Redundant binds are counted, not silently dropped

`CommandStats::redundant_pipeline_binds` exists so a caller that issues them can see it.
Silently absorbing waste makes the waste invisible; the debug overlay in Milestone 15 will
show this number precisely so a badly-sorted render list is visible rather than merely
slow.

## Performance: no measurable change, and how that was established

The first measurement said 0.248 ms against a 0.168 ms baseline — a 47% regression. It was
wrong, and the way it was wrong is worth recording, because `docs/performance.md` already
warns about exactly this.

| Method | M2 (before) | M3 (after) | Verdict |
| --- | --- | --- | --- |
| Single run, mean, compared against an earlier session | 0.168 ms | 0.248 ms | "47% regression" |
| 3 interleaved runs, mean | 0.229–0.309 ms | 0.219–0.332 ms | indistinguishable |
| 5 interleaved runs, median of means | 0.223 ms | 0.243 ms | overlapping, unresolved |
| 21 reporting intervals, **median p50** | **0.155 ms** | **0.155 ms** | no change |

The machine had picked up background load between sessions — an IDE holding 2.6 GB and
the GPU vendor's service — which inflated *both* binaries and dominated the mean. The mean
is the wrong estimator under interference: a handful of multi-millisecond spikes moves it
while leaving the frame's actual cost untouched. The p50 across reporting intervals is
robust to those spikes, and it says the two builds cost the same to within timer
resolution.

**The lesson, added to the methodology:** an A/B comparison must be interleaved *and*
reported with a spike-robust statistic. Comparing a mean measured today against a mean
measured yesterday measures the machine, not the change.

## Tests

Unit tests in `command.rs` cover the bookkeeping without a GPU: redundant-bind detection,
that clearing the binding state forces the next bind (a reset list has no state, so
remembering one would skip a bind the driver needs), slot independence, and the batching
ratio.

`tests/command_context.rs` covers what needs a driver:

| Test | Asserts |
| --- | --- |
| `barriers_batch_into_one_submission` | 4 transitions → 1 `ResourceBarrier` call |
| `closing_flushes_queued_barriers` | close leaves nothing queued |
| `a_transition_to_the_same_state_is_dropped` | no barrier, no call — D3D12 rejects equal states |
| `exceeding_the_batch_capacity_flushes_rather_than_growing` | 20 transitions, no lost barrier, no mid-frame allocation |
| `beginning_a_frame_clears_the_previous_statistics` | statistics describe one frame, not the process |

The first version of `barriers_batch_into_one_submission` transitioned buffers to
`UNORDERED_ACCESS`. The driver rejected the whole command list with `E_INVALIDARG` at
`Close`, because a buffer created without `ALLOW_UNORDERED_ACCESS` cannot enter that state.
The test was wrong, not the context — and it is a good argument for submitting in tests
rather than only recording: a barrier the driver refuses is invisible until something
executes it.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Command recording abstracted from the rest of the engine | pass |
| Only what the engine uses is wrapped | pass — 12 methods |
| D3D12 details not exposed | pass — `list()` remains for the GPU timer's query calls, documented |
| Barriers batched | pass — asserted on a real device |
| Statistics collected | pass — `CommandStats`, read by the renderer each frame |
| No regression | pass — median p50 unchanged at 0.155 ms, established by interleaved A/B |

## Known limitations

* No index-buffer or `DrawIndexedInstanced` wrapper yet: there are no index buffers until
  Milestone 10. It arrives with the mesh system rather than ahead of it.
* No descriptor-table binding: descriptors arrive in Milestone 4.
* Resource *state tracking* is not here — the caller still states `from` and `to`. The
  context batches and validates equality, but does not know a resource's current state.
  That belongs with the resource abstraction in Milestone 5, which is where the state
  lives.
* One list, one thread. Parallel command recording would need a context per thread and a
  submission order; nothing here forecloses it.
