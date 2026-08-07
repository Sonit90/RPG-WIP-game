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
| 4 | Descriptor management | done |
| 5 | Resource abstraction | done |
| 6 | Upload system | done |
| 7 | Shader system | done |
| 8 | Camera | done |
| 9 | Transform system | done |
| 10 | Mesh system | done |
| 11 | Basic material | done |
| 12 | Depth buffer | done |
| 13 | 3D scene | done |
| 14 | Basic lighting | done |
| 15 | Frustum culling | done |
| — | Checkpoint: audit, and the `engine-render-data` split | done |
| 16 | Render list | done |
| 17 | Render submission profiling, parallel culling | partial |
| 17b | Instanced rendering | todo |
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

---

# Milestone 4 — Descriptor management

## What was built

Three lifetimes, three strategies, because using one allocator for all of them means
paying the most expensive strategy's cost for the case that runs thousands of times a
second.

| Region | Visibility | Allocation | Reclaimed | Used for |
| --- | --- | --- | --- | --- |
| `CpuDescriptorHeap` | CPU only | free list | on release | RTV, DSV |
| `GpuDescriptorHeap` persistent | shader-visible | free list | on release | material views |
| `GpuDescriptorHeap` per frame | shader-visible | **bump** | whole slice, per frame | per-draw tables |

The allocator logic is two small pure types — `FreeListAllocator` and `BumpAllocator` —
with no D3D12 in them, so the rules that matter (no index handed out twice, no double
free, contiguous tables, peaks that survive resets) are unit-tested without a GPU.

`SwapChain` no longer manages its own RTV heap; it allocates three handles from a
`CpuDescriptorHeap`. That was the point of building it — a module that nothing uses is a
module nothing has validated.

## Why the per-draw case bumps

A descriptor table for one draw lives exactly one frame, and every table in that frame
dies at the same instant. Tracking them individually buys a free-list entry, a release
call and a lifetime to get wrong, in exchange for nothing. Bumping an offset is an add and
a compare; reclaiming a whole frame's worth is one store.

So the requirement "descriptor allocation must not become a per-object CPU bottleneck" is
met by not making the per-object case an allocation problem at all.

## Why one shader-visible heap

Changing the bound descriptor heap is among the more expensive state changes in D3D12 — on
some drivers it flushes. One heap, split into a persistent region and one slice per frame
slot, is bound once per frame and never changed. It is also the layout a bindless renderer
wants, so that path stays open without being built.

## Measured cost

`cargo run --release --bin cpu_bench`, median of 9 samples:

| Operation | Cost |
| --- | --- |
| Per-frame table, 1 descriptor | 2.85 ns |
| Per-frame table, 4 descriptors | 2.86 ns |
| Frame region reset | 0.44 ns |
| Free-list allocate + free | 3.54 ns |

The table size does not matter — 1 and 4 descriptors cost the same, because a bump is a
bump. Against the frame budget:

* 10,000 visible objects, one table each: **28.5 µs**, or 1.4% of a 2.08 ms frame at
  480 FPS.
* Reclaiming a frame's descriptors: **0.44 ns**, whatever was allocated.

Descriptor allocation is not going to be the bottleneck. That is now a measurement rather
than a design intention.

## Tests

Unit tests (no GPU) cover the allocator rules: distinct slots, most-recently-freed reused
first, rejection of freeing an unallocated or already-freed slot, contiguous ranges,
whole-region reset, zero-length requests, refusal without wrapping, non-overlapping frame
slices, and peaks surviving release.

`tests/descriptors.rs` covers what needs a driver:

| Test | Asserts |
| --- | --- |
| `cpu_handles_step_by_the_devices_descriptor_size` | handles step by the device's real stride, for RTV *and* DSV |
| `a_full_cpu_heap_reports_which_heap_it_was` | a full heap names itself and its capacity |
| `gpu_handles_carry_both_addresses_and_step_together` | CPU and GPU views of one heap advance in step |
| `frame_slices_are_disjoint_from_each_other_and_from_the_persistent_region` | no index handed out twice across all regions |
| `a_frame_slice_is_reclaimed_whole_and_reused` | reset returns the same descriptors; peak survives |
| `a_descriptor_table_is_contiguous` | the second table begins exactly `count` descriptors after the first |
| `a_heap_with_no_frame_slices_is_rejected` | zero slices refused; an empty heap is legal |

The stride tests matter because the descriptor size is hardware-specific and differs
between heap types. Computing handles with an assumed size produces views that address the
wrong descriptor — invisible until something renders wrong.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| RTV, DSV, CBV/SRV/UAV supported | pass |
| CPU-visible and GPU-visible separated | pass — different types, different strategies |
| Designed for reuse | pass — free list for long-lived, whole-region reset for per-frame |
| No expensive per-frame dynamic allocation | pass — 2.85 ns per table, measured |
| Allocation benchmarked and documented | pass — table above and `docs/performance.md` |
| Supports future material textures, constant buffers, instancing | pass — persistent region plus per-frame tables |
| Bindless not implemented | correct — the single-heap layout leaves the path open |

## Known limitations

* Nothing *creates views* into these heaps yet beyond the swap chain's RTVs. Constant
  buffer views and shader resource views arrive with the resource and material systems in
  Milestones 5 and 11; the heap is what they will allocate from.
* The renderer does not yet own a `GpuDescriptorHeap` — there is nothing to bind. It is
  created and bound when the first shader-visible descriptor exists.
* Freeing a persistent descriptor is immediate. A descriptor released while a frame in
  flight still references it would be reused too early; today nothing releases them, and
  Milestone 5's deferred destruction will cover both resources and their views.
* Heap capacities are constructor arguments with no defaults chosen yet. The numbers get
  chosen when there is a real scene to size them against, from the reported peaks.

---

# Milestone 5 — Resource abstraction

## What was built

`engine-renderer::resource`: `GpuBuffer`, `GpuTexture` (render target, depth buffer or
plain texture), the `TrackedResource` trait, and `DeferredReleaseQueue`. `RenderDevice`
gained `create_texture_2d`, `create_render_target_view` and `create_depth_stencil_view`.

## A resource knows its own state

Before this milestone a caller wrote:

```rust
context.transition(&buffer, PRESENT, RENDER_TARGET);
```

The "before" state is something the caller has to *remember*, across frames and across
code paths. Now:

```rust
context.transition_to(&buffer, RENDER_TARGET);
```

There is no "before" to state, so it cannot be stated wrongly. The renderer's back-buffer
transitions were the first users; the swap chain hands out `GpuTexture` values adopted in
`PRESENT`, which is both the state DXGI delivers them in and the state they must return to.

Two consequences fall out for free:

* **A transition to the current state costs nothing.** It is a comparison, not a driver
  call — which is the "avoid blindly issuing barriers every frame" requirement, enforced
  rather than remembered. `CommandStats::redundant_transitions` counts them.
* **A wrong barrier is now impossible to write**, rather than being caught by the
  validation layer if someone happens to be running it.

## Tracking is per-resource, single-queue, and says so

Not per-subresource: every texture here has one mip and one array slice. Not cross-queue:
there is one queue. Both limits are real, both are documented, and neither is engineered
around, because nothing in the renderer needs either yet.

The state lives in a `Cell`. Recording borrows resources immutably — a texture is
referenced by its view, its render list entry and the command context in the same frame —
while a transition must update the state. That is single-threaded interior mutation, which
is what command recording is; `GpuTexture` is deliberately not `Sync`.

## Destruction is deferred rather than blocking

Releasing a resource the GPU may still be reading is a use-after-free whose symptom is a
driver crash somewhere else. The two safe options are to block until the GPU is idle —
correct, and a full pipeline drain — or to hold the resource until the fence value covering
its last use has passed.

`DeferredReleaseQueue` does the second. Releasing costs a push; collecting costs a
comparison per released resource, and the renderer collects once per frame against the
fence value the ring already knows. Entries are pushed in increasing fence order, so
everything releasable is a prefix and the scan stops at the first entry the GPU has not
reached.

`peak_pending` exists because a queue that keeps growing is a leak in slow motion: it means
resources are being released faster than frames complete.

## Tests

Unit tests cover the queue's ordering rule without a GPU (nothing released early,
everything up to the completed value released, collection stopping at the first unpassed
entry, shared fence values releasing together).

`tests/resource_lifetime.rs`, eight tests against a real device:

| Test | Asserts |
| --- | --- |
| `buffers_and_textures_are_created_in_the_state_they_report` | creation states match what the wrappers claim |
| `a_zero_sized_resource_is_rejected` | zero dimensions refused before reaching D3D12 |
| `a_transition_updates_the_tracked_state_and_the_driver_accepts_it` | the driver accepts a barrier built from tracked state |
| `transitioning_to_the_current_state_costs_nothing` | no barrier, no driver call, counted as redundant |
| `a_resource_survives_until_the_gpu_has_passed_its_fence_value` | nothing released while work is in flight |
| `resources_are_released_in_submission_order` | prefix collection, peak preserved |
| `a_resource_can_be_created_used_destroyed_and_recreated` | three full create/use/destroy cycles |
| `an_upload_buffer_accepts_data_and_rejects_what_does_not_fit` | size and heap-type checks |

Every one of these submits and waits rather than only recording. A barrier the driver
rejects is invisible until something executes it — the lesson from Milestone 3, applied.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Buffer, Texture, RenderTarget, DepthBuffer abstractions | pass — `GpuBuffer`, `GpuTexture` with render-target and depth constructors |
| Creation and destruction | pass |
| Resource state tracking | pass — per resource, updated at transition time |
| Upload | pass — `GpuBuffer::write` for upload-heap buffers |
| Transition | pass — `transition_to`, destination only |
| Transitions explicit and correct | pass — verified by submitting, not just recording |
| No blind per-frame barriers | pass — redundant transitions are a comparison |
| Tests verify common transitions | pass |
| Deferred destruction where needed | pass — `DeferredReleaseQueue`, collected per frame |
| No render graph | correct — none built |
| No regression | pass — median p50 0.152 ms |

## Known limitations

* Per-resource, not per-subresource, tracking. A mipmapped texture whose levels need
  different states would need a state per subresource; nothing has mips yet.
* Single-queue. A copy queue for uploads would need cross-queue state handling, which D3D12
  handles through `COMMON` and queue ownership transfers — a real design question, deferred
  until there is a copy queue.
* The deferred queue holds `ID3D12Resource` directly, so a released `GpuTexture` must hand
  over its resource clone. When the mesh and material systems own resources, they will do
  this in their `Drop` rather than the caller doing it by hand.
* Nothing yet uses `GpuBuffer` for geometry — `GpuMesh` still owns its own resource. The
  mesh system in Milestone 10 replaces it.

---

# Milestone 6 — Upload system

## What was built

`engine-renderer::upload` holds two paths, because per-frame data and static data have
nothing in common except the word "upload".

| Path | Type | Lifetime | Cost per use |
| --- | --- | --- | --- |
| Per-frame dynamic | `UploadRing` | one frame | a bump of an offset, no map, no fence |
| Static, once | `StagingUploader` | forever | one copy, batched into one fence wait |

`GpuBuffer::map_persistent` was added to support the first: the ring maps each slot's
buffer once at creation and never unmaps it. D3D12 permits a resource to stay mapped
indefinitely, and releasing it unmaps implicitly, so the per-frame `Map`/`Unmap` pair that
a naive upload path pays is simply not there.

## The ring bumps, for the same reason the descriptor heap does

One upload buffer per frame slot, `[slot 0 | slot 1 | slot 2]`, each reset at
`begin_frame`. Everything written into a slot dies at the same instant — when that frame's
fence passes — so individual tracking buys nothing. Allocation is `align_up(cursor)`, a
bounds check, and a pointer add.

Alignment is a parameter rather than a constant because D3D12 demands different values in
different places, and getting one wrong is a validation error at bind time rather than at
write time:

| Constant | Value | Applies to |
| --- | --- | --- |
| `CONSTANT_BUFFER_ALIGNMENT` | 256 | constant-buffer address **and** size |
| `TEXTURE_DATA_ALIGNMENT` | 512 | texture row data in an upload buffer |
| `DEFAULT_ALIGNMENT` | 16 | instance and vertex streams |

`write_constants` rounds the *size* up as well as the address, because a CBV describes a
256-byte multiple; a 68-byte struct occupies 256 bytes whether or not the caller thinks so.

A full slot **refuses and reports what was left**, and counts the failure. It does not
wrap — wrapping would overwrite data the GPU is reading this frame, which is silent
corruption. `UploadStats { capacity, used, peak, failures }` per slot, plus
`Renderer::upload_peak_bytes` across all slots, so the 4 MiB default gets replaced by a
measured number rather than a guessed one.

## Staging batches because a fence wait is not free

Phase 0's `GpuMesh::upload` created a staging buffer, an allocator, a list and a fence per
mesh, and blocked on each. Twenty meshes cost twenty full GPU round trips.

`StagingUploader` queues any number of `queue_buffer` calls into one command list, keeping
each staging buffer alive until `flush`, and pays **one** synchronization for the batch.
`GpuMesh::upload_many` is the caller that proves it. `GpuMesh::upload` still exists and
still uploads one mesh — but through the same uploader, so there is one code path rather
than two.

The uploader is reusable across batches: `flush` releases the staging buffers and resets,
so a caller can load one group of meshes, flush, and load another without rebuilding the
allocator, list and fence.

## Why not a copy queue

A dedicated copy queue would let uploads run concurrently with graphics. It also needs
cross-queue resource state handling, a second fence, and ownership transfers through
`COMMON`. Uploads happen when the scene changes, not per frame, and the renderer is
CPU-bound on submission — so the copy queue would buy nothing measurable today. The
staging path is one type; replacing its queue later is a local change.

## Tests

`crates/engine-renderer/tests/upload.rs`, nine tests against a real device:

| Test | Asserts |
| --- | --- |
| `ring_allocations_are_aligned_and_distinct` | every allocation meets its alignment and no two overlap |
| `slots_are_independent_and_reset_individually` | resetting slot 0 leaves slot 1 untouched |
| `a_full_slot_refuses_and_says_what_is_left` | refusal, not wrapping; the error names the remaining bytes; `failures` counts it |
| `written_data_reaches_the_gpu_through_the_ring` | data copied from the ring to a default-heap buffer reads back byte-identical |
| `constant_writes_are_padded_to_the_required_size` | a 4-byte struct consumes 256 bytes of the ring |
| `batched_uploads_cost_one_synchronization` | 8 buffers, `total_flushes == 1`, `total_copies == 8` |
| `an_empty_flush_does_nothing` | no submission, no fence wait, no error |
| `an_upload_larger_than_its_destination_is_rejected` | size checked before the copy is recorded |
| `an_uploader_can_be_reused_across_batches` | two batches through one uploader, both correct |

The read-back tests exist because a persistently mapped pointer that writes to the wrong
offset produces a buffer full of plausible-looking zeros. Only reading it back on the GPU
side proves the bytes arrived where the GPU address said they would.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Efficient CPU→GPU transfer | pass — persistent map, bump allocate, no per-use `Map` |
| Upload heap ring buffer | pass — one buffer per frame slot, reset at `begin_frame` |
| Per-frame dynamic data supported | pass — constants, instance streams |
| Static data uploaded once to a default heap | pass — `StagingUploader`, `GpuMesh` |
| Batched uploads | pass — N buffers, one fence wait, asserted |
| No unnecessary synchronization | pass — the ring never fences; staging fences once per batch |
| No per-frame allocation | pass — the ring allocates only at creation |
| High-water mark reported | pass — `UploadStats::peak`, `Renderer::upload_peak_bytes` |
| No regression | pass — median p50 0.133 ms against the 0.152 ms baseline |

## Known limitations

* Nothing writes to the ring in the frame path yet. It is created, reset per frame and
  tested, but constants arrive with the camera in Milestone 8 and instance data with
  Milestone 17. The reset is in place so those milestones add a write, not a lifecycle.
* Textures are not uploaded through either path. `TEXTURE_DATA_ALIGNMENT` is defined and a
  texture upload needs `GetCopyableFootprints` plus `CopyTextureRegion`, which arrives with
  materials in Milestone 11.
* A slot that fills fails the allocation and the caller must handle it. There is no
  fallback to a temporary committed buffer. That is deliberate for now: a failure is
  visible and points at a capacity that needs raising, while a silent fallback would hide
  a per-frame allocation.
* `StagingUploader::flush` blocks. For startup loading that is correct and simple; if
  streaming ever needs to load without stalling, the fence value is already there to poll
  against instead.

---

# Milestone 7 — Shader system

## What was built

`crates/engine-renderer/build.rs` compiles every HLSL entry point to DXIL with `dxc` and
writes a generated `shaders.rs` into `OUT_DIR`; `src/shader.rs` includes it. The runtime
compiler is gone: `D3DCompile`, `d3dcompiler_47.dll` and the `Win32_Graphics_Direct3D_Fxc`
feature have all been removed from the crate.

```rust
// build.rs
ShaderEntry { file: "triangle.hlsl", entry: "VSMain", profile: "vs_6_0", constant: "TRIANGLE_VS" },

// generated, in OUT_DIR
pub const TRIANGLE_VS: &[u8] = include_bytes!(r"…\triangle.VSMain.dxil");

// pipeline.rs
VS: bytecode(TRIANGLE_VS),
```

## What changes as a consequence

| Before | After |
| --- | --- |
| A shader error is a runtime `Err` at pipeline creation | A shader error is a **build** error with `dxc`'s file, line and message |
| Startup compiles four entry points | Startup compiles nothing |
| Shader model 5.1 through FXC | Shader model **6.0**, DXIL, signed |
| `ID3DBlob` lifetimes threaded through pipeline creation | `&'static [u8]`, which cannot dangle |

The last one is not cosmetic. `D3D12_SHADER_BYTECODE` is a raw pointer and a length; with
`ID3DBlob` the caller had to keep the blob alive across `CreateGraphicsPipelineState`, and
nothing in the type system said so. A `&'static [u8]` outlives everything.

## The shader table is explicit, not a directory scan

`build.rs` names each entry point. Scanning `shaders/` would be shorter and would silently
compile a file nobody references, and would silently *stop* compiling one that got renamed.
Adding a shader means adding four lines; that is the correct amount of friction.

## One set of flags for every build profile

`-O3 -WX`, debug and release alike. The alternative — unoptimized shaders with debug info
in debug builds — means the shipped bytecode is not the bytecode that was tested, and a
shader bug that reproduces in one profile and not the other is a bad afternoon. Warnings
are errors because an HLSL warning is nearly always a type or semantic mistake that renders
as garbage rather than failing.

## Finding `dxc`

Search order: `DXC_PATH`, then `PATH`, then the newest Windows SDK
`bin\<version>\<arch>\dxc.exe`. On this machine that resolves to SDK 10.0.17763.0 x64,
which ships `dxil.dll` beside it so the container is signed. If none is found the build
fails with a message naming the SDK and both overrides — the risk listed in the
architecture proposal, mitigated as stated.

## Tests

Unit tests in `shader.rs`, which need no GPU because the bytecode is a compile-time
artefact:

| Test | Asserts |
| --- | --- |
| `every_shader_is_a_signed_dxil_container` | all four blobs are DXBC containers of plausible size |
| `the_container_hash_is_not_all_zero` | the digest `dxc` writes when signing is present — an unsigned blob is refused by the driver, and only at pipeline creation |
| `the_descriptor_points_at_the_blob` | `bytecode()` produces the right pointer and length |
| `a_truncated_blob_is_not_mistaken_for_a_container` | the magic check needs a size too |

The signature test earns its place: if `dxil.dll` is missing beside `dxc.exe`, compilation
succeeds and produces an unsigned container that fails at `CreateGraphicsPipelineState`
with a bare `E_INVALIDARG` naming nothing. This test names it.

Behavioural coverage is the existing offscreen pixel tests: the triangle and the instanced
grid render the same images through DXIL as they did through FXC, which is what "the port
did not change the output" means.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Shaders compiled offline | pass — `build.rs`, `dxc`, DXIL |
| Compiled bytecode loaded, not source | pass — `include_bytes!` |
| Errors reported clearly | pass — build failure carrying `dxc`'s own diagnostic |
| No runtime compilation | pass — `D3DCompile` and its `windows` feature removed |
| Deterministic across builds | pass — one flag set for every profile |
| Shipping binary needs no shader files | pass — bytecode is embedded |
| No regression | pass — median p50 0.150 ms |

## Known limitations

* Shaders live in `crates/engine-renderer/shaders/`, not the `assets/shaders/` the
  architecture proposal named. A build script is crate-scoped, and shaders that only the
  renderer uses belong beside the renderer; when there is an asset pipeline with several
  consumers, moving them is a path change in one table.
* No `#include` support is exercised. `dxc` handles includes natively, but
  `rerun-if-changed` only watches `shaders/`, so an include from outside that directory
  would not trigger a rebuild. The directory-level watch covers every include inside it.
* No reflection. Root signatures are still written by hand and must agree with the shader;
  a mismatch is a pipeline-creation error rather than a build error. Reflection would let
  `build.rs` generate the root signature, which is worth doing when there is more than one.
* No hot reload, by design. If shader iteration ever becomes the bottleneck, a
  `cfg(debug_assertions)` disk-loading path can be added without touching the release path.

---

# Milestone 8 — Camera

## What was built

Two things, because a camera without a math library is a camera that invents one.

**`engine_core::math`** — `Vec2`, `Vec3`, `Vec4`, `Quat`, `Mat4`. Written here rather than
taken from a crate, as decided in §10: the engine's only third-party dependencies are `log`
and `windows`, and the conventions had to be pinned down and tested regardless of where the
code came from.

**`engine_renderer::camera`** — `Camera` and `Projection`.

## The conventions, and why they are tested rather than commented

| Convention | Consequence if wrong |
| --- | --- |
| Right-handed, camera looks down −Z | Every normal points inward; back-face culling removes exactly what should be visible |
| Depth maps to [0, 1], not [−1, 1] | Half the scene is clipped away, and the half that survives looks plausible |
| Column-major storage, `clip = P * V * M * v` | Transforms compose in the wrong order; translation gets scaled |
| `a * b` applies `b` first | A follow camera lags its subject by one composition |

Each has a test whose name states the rule. `projection_maps_the_near_and_far_planes_to_zero_and_one`
is the most valuable one in the file: an OpenGL-style projection produces a scene that is
half missing rather than obviously broken.

## Inverse and determinant share their work

`Mat4::inverse` returns an `Option`. A singular matrix comes from a zero scale, which a
caller may legitimately have and must not silently propagate as a matrix of `NaN`s into a
view transform. The adjugate and the determinant are computed together because the
determinant is the weighted sum of the first column's cofactors — two implementations would
duplicate eighteen products and could drift apart.

## The camera is not a camera controller

`Camera` is position, orientation and a projection. It does not follow anything, collide
with anything or smooth anything. A third-person camera that orbits a character is a
*controller*: it belongs to the game, and it drives this type by setting a position and a
rotation. Putting the orbit here would mean the renderer knows what a character is, which
§3 forbids.

## Matrices are rebuilt on change, not on read

The view, projection and their product are stored. Changing happens at most once per frame;
reading happens once per draw. There is no dirty flag and no interior mutability, because a
setter that recomputes three matrices is cheaper than a getter that has to check.

The view matrix is built by inverting the camera transform *analytically* — conjugate the
rotation, rotate the negated position — rather than running the general 4×4 inverse. For a
rigid transform that is exact and about ten times cheaper.

`look_at` is the exception: it builds the view matrix directly and *derives* the rotation
from it, rather than the other way round, because a round trip through a quaternion is not
exact and `look_at` is.

## What is rejected rather than clamped

| Input | Why it is an error |
| --- | --- |
| Zero or negative aspect ratio | Produces infinities; a minimized window is the usual source |
| `fov_y` outside (0, π) | Degenerate or inverted projection |
| `near <= 0` or `far <= near` | Divides by zero in the depth range |
| `look_at` with eye == target | No view direction exists |
| `up` parallel to the view direction | The cross product is zero; the view matrix fills with `NaN` |

A follow camera that snaps to its subject produces the fourth, and one that looks straight
down produces the fifth. Both would otherwise blank the screen with nothing to point at.

## Tests

19 math tests and 13 camera tests, none needing a GPU:

| Area | Covers |
| --- | --- |
| Vectors | right-handed cross products, normalizing zero without `NaN` |
| Matrices | identity, associativity, operand order, `M · M⁻¹ = I`, singular detection, determinant against a known scale, transpose involution, constant-buffer layout |
| Projection | near/far to 0/1, frustum corners to screen edges, orthographic box to the unit cube |
| Quaternions | matrix agreement, quarter turn about +Y sending +X to −Z, conjugate cancellation, operand order |
| Camera | default pose, view moving the world opposite the camera, `look_at` agreeing with the rotation it reports, both aiming paths producing one view, `VP == P · V`, in-front inside the unit cube, behind outside the depth range, resize touching only aspect, every rejection above |

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| View and projection matrices | pass |
| Perspective and orthographic | pass |
| Aspect ratio handled | pass — `set_viewport`, zero rejected |
| Near/far configurable | pass, with defaults documented in terms of depth precision |
| Right-handed, D3D depth range | pass — tested, not asserted in a comment |
| Matrix math tested | pass — 19 tests |
| No gameplay concepts | pass — no follow, no target entity, no smoothing |
| No regression | pass — median p50 0.142 ms; nothing here runs in the frame path yet |

## Known limitations

* Nothing uses the camera yet. The frame path still draws clip-space triangles; the camera
  reaches the GPU in Milestone 13, when there is a constant buffer and a root signature to
  put it in.
* No `Sphere`, `Aabb`, `Plane` or `Frustum` yet. They belong with culling in Milestone 15,
  and building them now would mean building them against no caller.
* No reverse-Z. The depth range is a single expression in `Mat4::perspective`, and the
  change also touches the clear value and one PSO field — listed in the risk table and
  still cheap.
* `Mat4` is scalar. The stress benchmark does no matrix work at all yet, so a SIMD version
  would be optimizing an unmeasured cost.

---

# Milestone 9 — Transform system

## What was built

`engine_core::transform`: a `Transform` of translation, rotation and scale, and
`write_matrices`, which fills a caller-owned array of matrices from an array of transforms.

It lives in `engine-core`, not the renderer, because the simulation owns transforms and the
renderer only reads them. Putting it in the renderer would point the dependency arrow the
wrong way.

## Why not store the matrix

| Stored as | Rotate again | Interpolate | Read the rotation back | Fix accumulated drift |
| --- | --- | --- | --- | --- |
| `Mat4` | matrix multiply | not a rotation | decomposition | not possible — drift is a shear |
| T/R/S | quaternion product | slerp | it is already there | one square root |

The matrix is the cheaper thing to *read* and the worse thing to *keep*. So it is generated,
not stored — once per object per frame at worst.

## Composition is a shortcut with a stated limit

`parent.then(child)` produces a `Transform`, and a test asserts it equals
`parent.matrix() * child.matrix()`. That equality holds for uniform scale and fails for
non-uniform scale combined with rotation, because rotating a non-uniformly scaled child
introduces a **shear** and no scale vector can encode shear. The documentation says so and
names the alternative (compose matrices) rather than leaving a caller to discover it as a
rendering artefact.

## Measured cost

`cargo run --release --bin cpu_bench`, median of 9 samples:

| Operation | Cost |
| --- | --- |
| `Transform::matrix` | 3.42 ns |
| `write_matrices`, per transform over 100,000 | 2.65 ns |
| `Transform::then` | 12.05 ns |

The batched form is 22% cheaper per transform than the individual call, over an array far
larger than L2 — two contiguous streams and no allocation.

Against the 2.08 ms stretch budget:

| Visible objects | Matrix generation | Share of the frame |
| --- | --- | --- |
| 1,000 | 2.7 µs | 0.13% |
| 10,000 | 26.5 µs | 1.3% |
| 100,000 | 265 µs | 12.7% |

`docs/threading.md` measured the job-system break-even at roughly 12 µs of work per job, so
a chunk would have to be about **4,500 transforms** before submitting it is worth the
overhead. At 10,000 objects that is two chunks for 26 µs of work — not obviously worth it.
The loop is flat and takes slices, so chunking later is a change at the call site, not a
rewrite. It is not being done now because 1.3% of a frame is not where the time is.

## Tests

12 tests, no GPU:

| Test | Asserts |
| --- | --- |
| `the_identity_moves_nothing` | identity, default and `Mat4::IDENTITY` agree |
| `the_matrix_agrees_with_transforming_a_point_directly` | the two code paths are one definition |
| `transforming_a_vector_ignores_translation` | directions are not translated |
| `scale_applies_before_rotation` | the other order stretches along world axes and looks right until something is both scaled and turned |
| `composition_matches_multiplying_the_matrices` | `then` is exactly matrix composition |
| `composing_with_the_identity_changes_nothing` | on both sides |
| `a_transform_composed_with_its_inverse_is_the_identity` | round trip through `inverse` |
| `a_flattened_transform_has_no_inverse` | zero scale returns `None`, not infinities |
| `renormalizing_removes_accumulated_drift` | one square root restores unit length |
| `the_bounding_scale_is_the_largest_axis` | under-estimating culls visible objects |
| `matrices_are_written_in_order_without_allocating` | batch output matches per-item output |
| `a_short_output_buffer_truncates_and_says_how_much_it_took` | truncation over panic |

## A flaky test was fixed, not deleted

`trivial_work_never_makes_the_cpu_wait` asserted **zero** stalls in 20 empty frames. It
failed here on a machine with a busy compositor: the GPU legitimately lost a race. The
assertion now allows up to two, which still distinguishes it from the loaded case that
stalls on *every* frame. An exact count was measuring the machine rather than the renderer —
the same lesson `docs/performance.md` already records for frame-time comparisons.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Position, rotation, scale | pass |
| Matrix generation | pass — `matrix`, `write_matrices` |
| Efficient, no per-frame allocation | pass — 2.65 ns per transform into a caller-owned array |
| Hierarchy composition available | pass — `then`, with its exact limit documented |
| Rotation stored as a quaternion | pass, with the reasons tabulated |
| Benchmarked | pass — table above and `docs/performance.md` |
| No gameplay concepts | pass — a transform knows nothing about what it is attached to |
| No regression | pass — nothing here runs in the frame path yet |

## Known limitations

* No hierarchy *storage*. `then` composes two transforms; there is no parent array, no
  dirty propagation and no traversal order. Whether the renderer ever needs one is an open
  question — the `RenderWorld` in Milestone 13 takes world-space transforms, so a hierarchy
  would live in the simulation.
* `Transform` is 40 bytes and `write_matrices` produces 64 bytes each. At 100,000 objects
  that is 6.4 MB of matrices per frame, which is why instancing writes them straight into
  the upload ring in Milestone 17 rather than into an intermediate array.
* Scalar math. A SIMD `Mat4` would help the 100,000-object row above and nothing below it.
  Worth revisiting when a real scene reaches that count, not before.

---

# Milestone 10 — Mesh system

## What was built

`MeshVertex`, `MeshData` and an indexed `GpuMesh`, plus the primitives the later
milestones' scenes are built from. The Phase 0 raw-stream type survives under its real name.

| Type | Is | Used by |
| --- | --- | --- |
| `MeshVertex` | position, normal, UV — 32 bytes, `repr(C)` | the mesh vertex layout |
| `MeshData` | vertices, indices, bounding sphere, on the CPU | mesh creation, primitives |
| `GpuMesh` | vertex buffer + index buffer + bounds, device-local | the scene from Milestone 13 |
| `VertexStream` | one raw vertex buffer, no indices | the Phase 0 stress scene, per-instance streams |

The old `GpuMesh` was a raw vertex buffer, so it was **renamed** to `VertexStream` rather
than deleted, and the name freed for the indexed mesh the milestone is about. Giving the
stress scene's generated triangles an index buffer and bounds they do not have would be
fields that exist only to be ignored — and deleting the stress path would throw away the
regression baseline every milestone is measured against.

## Indices are validated on the CPU, because the GPU will not

`MeshData::new` rejects an index past the end of the vertex array. D3D12 does not
bounds-check indices: an out-of-range one reads whatever memory follows the buffer and draws
a triangle with a vertex somewhere near infinity. The symptom is a spike across the screen,
which is a long way from the cause.

It also rejects an index count that is not a multiple of three, and empty input.

## Vertices, and what is not being optimized

32 bytes: two vertices per cache line. The normal could be packed to 4 bytes and the UV to
4, halving it. That is not being done, because nothing is vertex-fetch bound yet and an
unmeasured packing buys a decode in every shader against a saving nobody has seen. Indices
are 32-bit for the same reason inverted: 16-bit halves index bandwidth but caps a mesh at
65,536 vertices, and a mesh that crosses the cap needs a splitting step that does not exist.

The layout is pinned by a test — `size_of` 32, offsets 0, 12, 24 — because the input layout
hard-codes those offsets and a silent change would read the normal as part of the position.

## Primitives

| Primitive | Why it exists |
| --- | --- |
| `cube` | The scene's building block. 24 vertices, not 8 — a corner has three different face normals, and sharing them averages a cube into a bad ball |
| `plane` | A ground quad, so depth testing in Milestone 12 has something to test against |
| `uv_sphere` | The only shape whose shading is obviously right or wrong at a glance; a cube's flat faces hide a normal that is subtly off |

`uv_sphere` clamps its segment and ring counts rather than rejecting them. It is a generator
for test scenes, not an asset importer, and a caller asking for one segment should get the
smallest sphere that exists rather than an error to handle.

## Bounds are computed once, at construction

Every mesh carries a local-space `Sphere`, computed from the box centre and the furthest
vertex from it. That is one pass and never under-estimates. The tightest possible sphere
(Welzl's algorithm) costs far more and buys a marginally tighter cull — and the property
that matters is the direction of the error: an under-estimating bound culls something that
is on screen.

`Aabb` and `Sphere` were added to `engine_core::math` for this. `Plane` and `Frustum` were
not, because culling arrives in Milestone 15 and building them now means building them
against no caller.

## Command context

`set_index_buffer` and `draw_indexed` were added — the wrapper methods Milestone 3
deliberately left out until there were index buffers to bind. `draw_indexed` takes a
`start_instance` so the individual-versus-instanced benchmark can index one instance buffer
two ways.

## Tests

Ten unit tests (no GPU) and five against a real device:

| Test | Asserts |
| --- | --- |
| `a_mesh_rejects_an_index_past_its_vertices` | the check the GPU does not do |
| `a_mesh_rejects_an_incomplete_triangle` | index count is a multiple of three |
| `a_mesh_rejects_emptiness` | neither array may be empty |
| `a_cube_has_flat_faces_and_the_right_counts` | 24 vertices, 12 triangles, every normal on exactly one axis |
| `a_cube_is_bounded_by_the_sphere_through_its_corners` | radius is the half-diagonal; every vertex inside |
| `a_sphere_stays_on_its_radius` | every position at `r`, every normal unit |
| `a_sphere_indexes_every_quad_of_its_grid` | `segments × rings × 2` triangles, all indices in range |
| `a_degenerate_sphere_request_is_clamped_rather_than_rejected` | zero segments still produces a mesh |
| `a_plane_faces_up_and_is_flat` | all `y = 0`, all normals +Y |
| `a_vertex_is_the_size_the_input_layout_describes` | 32 bytes, offsets 0/12/24 |
| `a_cube_reaches_the_gpu_vertex_for_vertex` | read back from device-local memory, byte-identical |
| `the_views_describe_the_buffers_they_point_at` | stride, sizes and non-null addresses |
| `a_meshs_bounds_survive_the_upload` | the CPU bounds reach the GPU mesh |
| `a_batch_of_meshes_uploads_correctly` | all three meshes complete, not just the last queued |
| `a_mesh_can_be_dropped_and_another_uploaded_in_its_place` | three create/use/destroy cycles |

The batch test is the one that would catch the subtle failure: a staging buffer released
before its copy executed leaves *one* mesh full of garbage while the others look fine.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Vertex and index buffers | pass |
| Vertex format: position, normal, UV | pass — 32 bytes, layout pinned by test |
| Bounding volume per mesh | pass — local-space sphere, computed at construction |
| Efficient GPU upload | pass — default heap, one staging batch per mesh or per batch |
| Mesh creation and destruction | pass — tested across three cycles |
| Indexed drawing supported | pass — `set_index_buffer`, `draw_indexed` |
| No asset loading | correct — primitives are generated, nothing is read from disk |
| No regression | pass — median p50 0.144 ms |

## Known limitations

* Nothing draws a `GpuMesh` yet. The frame path still uses `VertexStream` and the clip-space
  triangle pipeline; a `MeshVertex` pipeline needs a camera constant buffer and a root
  signature, which is Milestone 13.
* No mesh handles. `MeshHandle` and the storage behind it arrive with the `RenderWorld`;
  today a mesh is owned directly by whoever uploaded it.
* No LODs, no tangents, no vertex colours, no submeshes. Submeshes are what a material per
  mesh section needs, and that question belongs to Milestone 11.
* Vertex and index data live in separate buffers. One buffer with two offsets would halve
  the resource count; it would also complicate state tracking, and the resource count is not
  a measured problem.

---

# Milestone 11 — Basic material

## What was built

`MaterialDesc` (what a surface looks like), `MaterialConstants` (the same thing in the
shader's memory layout) and `Material` (the constants resident on the GPU).

A material is a colour and two scalars. It is deliberately **not** a shader, a pass, a
technique or a permutation system: one pipeline draws every material in Phase 2, and the
material only supplies constants to it. §46 rules out a material graph, and nothing here
needs one.

```rust
let material = Material::new(&device, MaterialDesc::colored(0.8, 0.2, 0.2)
    .with_roughness(0.4))?;
// material.gpu_address() -> a root constant-buffer view
```

## Constants live in device-local memory, not the upload ring

A material's parameters change when the material changes, which is approximately never. The
upload ring is for data that changes every frame; putting a material there would rewrite the
same bytes 480 times a second. So the constants are written once through the staging
uploader, exactly like mesh data, and several materials created together share one
synchronization.

## Why a root CBV rather than a descriptor table

With no textures, a material's descriptor table would contain exactly one CBV. A root
constant-buffer view puts the GPU address directly in the root signature: no descriptor
allocation, no table bind per draw.

When a material gains a texture that stops being true, and the material moves to a table in
the persistent descriptor region — which is what Milestone 4 built that region for. The
change is local to this type and the root signature.

## The constant layout is the shader's layout

HLSL packs constant buffers into 16-byte registers and never straddles one, so
`MaterialConstants` writes its padding out explicitly rather than relying on Rust and HLSL
happening to agree:

| Field | Offset | Register |
| --- | --- | --- |
| `base_color` | 0 | b.x — b.w |
| `emissive` | 16 | b+1.x — b+1.z |
| `roughness` | 28 | b+1.w |
| `metallic` | 32 | b+2.x |
| padding | 36 | b+2.y — b+2.w |

A test pins all of it, because a field inserted without matching padding shifts every
subsequent value and shades the scene with whatever landed in the wrong register.

The buffer itself is 256 bytes, not 48: a constant buffer's **size** must be a 256-byte
multiple, not only its address. The slack is never read. A test asserts the GPU address is
256-aligned, because a misaligned CBV is rejected at bind time — a long way from the code
that created it.

## Validation

`roughness` and `metallic` are clamped by the builders and *rejected* by `validate`. Both,
deliberately: the builder is the normal path and a typo there should not become an error to
handle, while a value that bypassed the builder must not reach the GPU.

Non-finite values are rejected outright. `NaN` propagates through the whole lighting
expression and produces a black or white pixel with nothing pointing at where it came from.

`roughness` and `metallic` exist now, before lighting does, because adding a field to a
constant buffer later means changing the struct, the shader and the padding together. They
are not a claim that the renderer is physically based.

## Tests

Six unit tests and four against a real device:

| Test | Asserts |
| --- | --- |
| `the_default_material_is_a_plain_grey_dielectric` | a sane default |
| `builders_clamp_rather_than_produce_unusable_values` | 2.0 roughness becomes 1.0 |
| `out_of_range_parameters_are_rejected` | the bypass path still fails |
| `non_finite_parameters_are_rejected` | `NaN` and infinity |
| `the_constant_layout_matches_the_shader_registers` | size 48, offsets 0/16/28/32 |
| `a_description_converts_field_for_field` | no field silently dropped |
| `a_materials_constants_reach_the_gpu_unchanged` | read back from device-local memory |
| `the_constant_address_is_aligned_for_a_root_view` | 256-byte multiple |
| `a_batch_of_materials_uploads_correctly` | three distinct materials, not three copies of the last |
| `an_invalid_material_is_rejected_before_it_reaches_the_gpu` | validation runs before allocation |

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Material with colour and simple parameters | pass |
| Constant buffer per material | pass — 256 bytes, device-local |
| Layout matches the shader | pass — pinned by test |
| Efficient binding | pass — root CBV, no per-draw descriptor |
| Created and destroyed like any resource | pass — `GpuBuffer`, same lifetime rules |
| Batched creation | pass — `new_many`, one synchronization |
| No material graph, no permutations | correct — none built |
| No regression | pass — nothing here runs in the frame path yet |

## Known limitations

* No textures. That is what a descriptor table and an SRV are for, and the material moves to
  one when there is an image to sample. Nothing in the current shape prevents it.
* No `MaterialHandle`. Handles arrive with the `RenderWorld` in Milestone 13; a material is
  currently owned by whoever created it.
* No transparency. `base_color.w` is carried through but nothing blends — Phase 2 is opaque
  only, and a blended pass needs sorting by depth, which the sort key does not yet do.
* Nothing binds a material yet. The mesh pipeline and its root signature arrive with the 3D
  scene in Milestone 13; this milestone supplies what that pipeline will read.

---

# Milestone 12 — Depth buffer

## What was built

`DepthBuffer`: one depth texture the size of the back buffer, its DSV, and a resize that
keeps the two in step. The frame path clears it and binds it; both pipelines test and write
against it.

Without it, geometry is drawn in submission order and whatever was recorded last wins. That
happens to look correct for one triangle and stops looking correct the moment there are two
objects — which is the scene Milestone 13 builds.

## The format and the comparison

| Choice | Value | Why |
| --- | --- | --- |
| Format | `D32_FLOAT` | No stencil work exists. A 32-bit float spends precision near the eye, which is where depth fighting shows. Same 4 bytes as `D24_UNORM_S8_UINT` |
| Clear | 1.0 | The far plane |
| Comparison | `LESS` | A fragment is drawn when it is *nearer* than what is stored |
| Stencil | off | Nothing writes it; enabling it costs a read-modify-write per fragment for a buffer nothing reads |

`LESS` rather than `LESS_EQUAL` is pinned by a test. The two look interchangeable and are
not: with `LESS`, a coplanar surface drawn second is rejected, which makes z-fighting
deterministic instead of dependent on rasterization order.

Reverse-Z is not adopted. It is a real precision win at long range and it touches the clear
constant, the projection matrix and one PSO field together — which is exactly what the risk
table said, and it remains a cheap change.

## Both pipelines had to change together

`ScenePipeline` and `InstancedPipeline` are swapped between benchmark modes against the same
render target and depth buffer. A PSO whose `DSVFormat` disagrees with the bound view is a
draw-time driver error, so both declare the same format and the same depth state.

The offscreen path also gained a depth buffer. A PSO that declares a `DSVFormat` with no view
bound silently disables depth — which would have left every pixel test measuring something
different from what the window path runs.

## The depth transition is free

The depth texture is created in `DEPTH_WRITE` and never leaves it: nothing samples it. The
per-frame `transition_to` therefore finds the state already correct, emits no barrier, and
counts a redundant transition. It is written explicitly rather than omitted because the day
something *does* sample depth, the transition has to be there — and `CommandStats` shows
that today it costs nothing.

## Measured cost

Release build, 1000-triangle stress scene, three reporting intervals, median of per-interval
p50:

| | Before (M11) | After (M12) | Change |
| --- | --- | --- | --- |
| CPU frame | 0.144 ms | 0.161 ms | +0.017 ms |
| GPU frame | 0.034 ms | 0.054 ms | +0.020 ms |

This is a cost, not a regression: a full 1920×1080 depth clear plus per-fragment testing is
work that did not exist before. The GPU number is the honest one — 20 µs for a 2 MB clear
and depth testing across the frame — and it is 1% of the 2.08 ms stretch budget. The CPU
number is one extra `ClearDepthStencilView` and one extra bind per frame; 17 µs is more than
those two calls should cost, so part of it is the machine, and it is recorded as measured
rather than explained away.

## Tests

`tests/depth_ordering.rs`, four tests that judge pixels rather than API calls:

| Test | Asserts |
| --- | --- |
| `the_nearer_surface_wins_whatever_order_it_was_submitted_in` | the same image from both submission orders — the whole point of the milestone |
| `a_surface_behind_another_does_not_show_through` | zero pixels of the occluded surface, not "few" |
| `coplanar_surfaces_keep_the_first_one_drawn` | `LESS`, not `LESS_EQUAL` |
| `nothing_beyond_the_far_plane_is_drawn` | depth outside [0, 1] is clipped — an OpenGL-range projection would draw it |

The first test is the one that would have failed before this milestone, and it fails in the
right way: it compares two renders against each other rather than against a magic colour.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Depth buffer created and bound | pass |
| Depth testing enabled | pass — proven by pixels |
| Depth cleared per frame | pass |
| Resize keeps depth and colour in step | pass — same call path, same idle wait |
| Format chosen deliberately | pass — `D32_FLOAT`, reasons recorded |
| Cost measured | pass — +0.020 ms GPU, table above |
| No regression beyond the feature's own cost | pass — CPU +0.017 ms, attributed |

## Known limitations

* No depth pre-pass. It would help when overdraw is high and cost a full extra geometry
  submission when it is not; there is no scene yet with enough overdraw to tell.
* No reverse-Z, as above.
* Depth is never read. When something samples it — SSAO, soft particles, a depth-aware
  effect — the texture needs a shader-resource view and a real transition, which is why the
  transition is already in the frame path.
* One depth buffer, matching the back buffer. Shadow maps in a later phase would each need
  their own; nothing here assumes there is only ever one.

---

# Milestone 13 — 3D scene

## What was built

The first frame that draws real geometry: `RenderWorld`, typed handles, a mesh pipeline with
three constant buffers, and `Renderer::render_world`.

```rust
let mesh     = renderer.create_mesh(&MeshData::cube(1.0))?;   // MeshHandle
let material = renderer.create_material(MaterialDesc::colored(0.8, 0.2, 0.2))?;
let entity   = world.spawn(RenderEntityDesc { mesh, material, transform, bounds, visible: true });
renderer.render_world(&world, &camera, CLEAR)?;
renderer.present()?;
```

That is the API §1 of the architecture proposal committed to, reached without an ECS, a
scene graph or a render graph.

## `RenderWorld` is arrays, not objects

Structure-of-arrays: transforms, meshes, materials, world bounds, visibility, generations.
Culling in Milestone 15 reads `bounds` and `visible` and nothing else, and streaming mesh
handles it does not look at would waste the cache line it is reading through.

Handles are index plus generation in eight bytes. A despawn bumps the slot's generation,
which makes every outstanding handle to it stale — detected by comparison rather than
dereferenced. A test spawns, despawns, spawns again into the same slot and asserts the first
handle is refused; without the generation it would silently address the second entity.

Freed slots keep their array entries. Clearing them would be work for data nobody reads;
`visible = false` is what makes a slot invisible, and `is_drawable` is the single question
the draw loop asks.

## World bounds are computed on write, not on read

`spawn` and `set_transform` place the mesh's local sphere into world space immediately. The
draw loop and the culler then read a ready value instead of transforming bounds per frame —
transforms change far less often than frames happen.

The radius is scaled by the **largest** axis. A non-uniformly scaled sphere is an ellipsoid;
keeping it a sphere means over-estimating, and the direction matters: over-estimating draws
something that could have been culled, under-estimating culls something that is on screen.

## The shader, and why no transpose happens

`shaders/mesh.hlsl` declares its matrices `column_major`, which is both the HLSL default and
the layout `Mat4` already has — columns in consecutive registers. So a matrix is memcpy'd
into the constant buffer and `mul(M, v)` is the same `M * v` the CPU-side math means. A test
asserts the translation lands in elements 12-14, so a transpose added "for safety" fails
loudly instead of silently mirroring the world.

There is no lighting. That is Milestone 14; this milestone is about geometry arriving in the
right place, and a flat colour makes "the right place" unambiguous.

## Three root CBVs, ordered by update frequency

| Slot | Register | Changes |
| --- | --- | --- |
| 0 | `b0` frame | once per frame |
| 1 | `b2` material | once per material run |
| 2 | `b1` object | once per draw |

Root CBVs, not descriptor tables: each is a GPU address written straight into the root
arguments, with no descriptor to allocate and no table to bind. That is the cheapest binding
D3D12 offers and it is available precisely because nothing samples a texture yet.

Frame and object constants are written into the **upload ring** — its first real use. They
are exactly what it was built for: transient data, alive for one frame, reclaimed by the
per-slot reset that was already in the frame path since Milestone 6.

## The frame path is now shared

`render` and `render_world` both call `begin_recording` and `finish_recording`. The pacing
wait, the timestamp read, the deferred-release collection and the ring reset have to happen
in that order; having that written twice is how one of them ends up missing from one path.

The overlay now binds its own pipeline instead of inheriting whatever the scene left bound.
It draws clip-space position+colour vertices, which the mesh pipeline's input layout does
not describe.

## Tests

Eleven unit tests for the world and constants, six pixel tests for the scene:

| Test | Asserts |
| --- | --- |
| `a_stale_handle_is_detected_rather_than_dereferenced` | the reason handles carry a generation |
| `slots_are_reused_rather_than_growing` | four spawns after four despawns use four slots |
| `bounds_follow_the_transform_into_world_space` | translation and the largest-axis radius |
| `a_rotated_offset_bound_ends_up_where_the_geometry_is` | rotation applies to the centre |
| `hiding_an_entity_keeps_it_alive` | hidden is not despawned |
| `frame_constants_keep_the_matrix_column_major` | no transpose anywhere |
| `a_cube_in_front_of_the_camera_is_visible_and_its_material_colours_it` | drawn, and *not* filling the frame — a wrong projection passes a naive centre-pixel test |
| `an_object_moved_out_of_view_is_not_drawn` | the transform reaches the shader |
| `the_nearer_of_two_objects_occludes_the_other` | identical images from both submission orders |
| `a_more_distant_object_covers_fewer_pixels` | perspective, not orthographic |
| `each_object_gets_its_own_material` | the material CBV is bound per draw, not once |
| `an_empty_scene_renders_the_clear_colour` | zero objects is not an error |

`offscreen::render_scene` was added so these judge the *same* pipeline, constant buffers and
depth state the window path runs, rather than a simplified copy of it. It takes a closure
that hands back what it recorded: a command list holds raw pointers, so a pipeline dropped
between recording and submission is a use-after-free the type system cannot otherwise see.

## Measured cost

Release build, 1000-triangle stress scene (the `render` path, unchanged by this milestone),
median of per-interval p50:

| | M12 | M13 |
| --- | --- | --- |
| CPU frame | 0.161 ms | 0.160 ms |
| GPU frame | 0.054 ms | 0.053 ms |

No regression. The world path's own cost is not measured yet — there is no benchmark scene
that drives it, which is what Milestones 15 to 18 need and will add.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Multiple objects with transforms | pass |
| Camera drives the view | pass — proven by moving an object out of frame |
| Per-object constants | pass — through the upload ring |
| Per-material constants | pass — bound per draw, proven by two colours |
| Depth-correct rendering | pass — order-independent image |
| Typed handles, no raw D3D12 exposed | pass |
| `RenderWorld` owned by the caller | pass |
| No gameplay concepts in the renderer | pass — a `RenderEntity` is a slot in three arrays |
| No regression | pass — 0.160 ms |

## Known limitations

* **One draw per object, no sorting, no culling, no instancing.** That is the point: this is
  the baseline Milestones 15 to 18 have to beat, and beating an unmeasured baseline is not a
  measurement.
* Meshes and materials are append-only. `create_mesh` never frees, so handle generations are
  always 1 on that side. Destruction needs the deferred-release queue and a free list, and
  it needs a caller that destroys things — the world has one, the asset side does not yet.
* The object constant buffer is written per object per frame even when nothing moved. A
  static object could keep its constants in device-local memory; whether that is worth the
  bookkeeping is a measurement Milestone 17 will be able to make.
* Normals are transformed by the model matrix, which is correct only for uniform scale. The
  inverse transpose costs an extra matrix per object and nothing scales non-uniformly yet —
  Milestone 14 is where a wrong normal would start to show.

---

# Milestone 14 — Basic lighting

## What was built

One directional light with an ambient term, Lambert diffuse, in the mesh shader.
`Lighting` carries direction, colour, intensity, ambient colour and ambient intensity, and
is passed to `Renderer::render_world` per frame.

Directional because a sun is the only light an outdoor third-person scene cannot do without,
and because it needs no attenuation, no position and no per-object light list. Point and
spot lights need all three, and §46 rules out clustered shading — which is what makes many
of them affordable.

## What it deliberately is not

Lambert diffuse plus constant ambient. No specular, no Fresnel, no BRDF. `roughness` and
`metallic` are carried in the material and **unused**: a specular lobe would be a second
thing to be wrong while establishing that normals and light direction are right. The job of
this milestone is that a lit surface is brighter than an unlit one *for the correct
geometric reason*, and that is testable to within a few percent.

`intensity` is not in physical units. There is no exposure control to calibrate against, so
1.0 means "a fully lit surface shows its base colour".

## Direction is stored the way it is authored

`Lighting::direction` is the direction the light *travels* — "the sun points down and to the
left" — and is negated once in the shader rather than at every call site that sets it. It is
also normalized once on the CPU: normalizing per pixel would be a square root per fragment
for a value that is constant across the frame.

## Ambient exists because there is no bounce

With a single directional light and nothing else, every surface facing away from the sun is
pure black. Ambient is a flat term standing in for the light that would have bounced. The
default is 0.15, which keeps an unlit face readable without washing out the lit one.

## This milestone found a real bug in Milestone 10

`MeshData::cube` built each face from a (normal, tangent, bitangent) basis where
`tangent × bitangent` did **not** equal the normal. Every face therefore wound clockwise seen
from outside, the outward faces were back-face culled, and the *inward* faces were drawn in
their place.

That is invisible without lighting: the cube looks exactly the same, because the inward face
of the far side occupies the same pixels as the outward face of the near side and carries the
same flat colour. It became visible the instant a normal was read — every surface was lit
from the wrong side, and the first lighting test returned pure ambient.

The face table is now chosen so `tangent × bitangent == normal`, with a `debug_assert` that
says so, and the comment explains the failure mode rather than just the rule.

**The general point:** flat shading hides a whole class of geometric errors. This is the
milestone that makes normals observable, so it is the milestone that finds them.

## Tests

`tests/lighting.rs`, six pixel tests that compare renders against each other rather than
against magic colours:

| Test | Asserts |
| --- | --- |
| `a_surface_facing_the_light_is_brighter_than_one_facing_away` | head-on is near full, turned away is exactly zero |
| `brightness_follows_the_cosine_of_the_angle` | at 45° the ratio is cos 45° ± 0.05 — a shader lighting by distance rather than angle passes the on/off test and fails this |
| `ambient_light_keeps_an_unlit_face_visible` | 0.25 ambient on white is 64 ± 2 |
| `light_colour_tints_the_surface` | a red light on a white surface leaves green and blue at exactly zero |
| `rotating_an_object_changes_which_of_its_faces_is_lit` | the normal is transformed by the model matrix — a normal left in local space renders identically to the unrotated cube |
| `emissive_material_ignores_the_light` | lit from behind, the emissive term still shows |

The cosine test is the one worth having. On/off lighting is easy to get accidentally right;
the *shape* of the falloff is not.

## Measured cost

Release build, 1000-triangle stress scene, median of per-interval p50:

| | M13 | M14 |
| --- | --- | --- |
| CPU frame | 0.160 ms | 0.160 ms |
| GPU frame | 0.053 ms | 0.054 ms |

Unchanged. The stress scene runs the Phase 0 pipeline, which this milestone does not touch;
the lit path's own cost is a dot product and two multiplies per fragment, and there is no
scene yet that drives enough fragments to measure it.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| One directional light | pass |
| Diffuse lighting from surface normals | pass — Lambert, verified against cos θ |
| Ambient term | pass, with the reason it exists |
| Light colour and intensity configurable | pass |
| Material colour respected | pass |
| Normals transformed to world space | pass — proven by rotating an object |
| No PBR, no shadows, no multiple lights | correct — none built |
| No regression | pass — 0.160 ms |

## Known limitations

* `roughness` and `metallic` are carried and ignored. They exist so the constant buffer does
  not have to change shape when a specular term arrives.
* Normals use the model matrix, not its inverse transpose, so non-uniform scale skews them.
  Nothing scales non-uniformly yet; the fix is one more matrix per object and belongs with
  whatever first needs it.
* One light, hard-coded into the frame constants. A light list means a per-object light
  assignment, which means culling lights, which is the clustered-shading path §46 excludes.
* No shadows. A directional shadow map is the single biggest visual win available and is
  explicitly out of Phase 2's scope.
* Lighting is computed per pixel in the same pass as the material. There is no deferred
  path and no light pre-pass; with one light there is nothing to amortize.

---

# Milestone 15 — Frustum culling

## What was built

`Plane` and `Frustum` in `engine_core::math`, and `Culler` in the renderer. `render_world`
now records only what survives the frustum.

## Plane extraction, and the two ways to get it wrong

The frustum planes come from the rows of the view-projection matrix (Gribb–Hartmann). Two
mistakes are easy and both produce a frustum that *mostly* works:

* **Normalizing the normal without scaling the distance.** The plane then moves. Culling
  stays correct near the centre of the screen and goes wrong at the edges — much harder to
  notice than culling nothing. `Plane::new` scales both together, and a test asserts every
  extracted plane has a unit normal.
* **Using the OpenGL near plane.** With depth in [−1, 1] the near plane is `w + z`; with
  D3D12's [0, 1] it is `z` alone. The wrong form puts the near plane in the wrong place and
  the symptom is objects vanishing just in front of the camera. A test pins the plane's
  position for a known camera.

## The test is conservative on purpose

A sphere that straddles a plane counts as visible. Culling something visible is a rendering
error; drawing something invisible is only work. The same asymmetry is why world bounds
scale by the largest axis (Milestone 13) — every approximation in the chain errs towards
drawing too much.

## Hidden is not culled

`visible = false` entities never reach the frustum test and are not counted as culled.
Counting them would flatter the cull ratio, which exists precisely to answer "is culling
doing anything?".

`CullStats` reports considered, visible, culled and the ratio. A culler that rejects nothing
is pure cost; one that rejects everything is a bug that looks like a fast frame. Both are
visible in the numbers.

## Measured cost

`cargo run --release --bin cpu_bench`, median of 9 samples, roughly half the scene outside
the view so both the accept and reject paths are exercised:

| Scene | Per object | Whole pass | Share of a 2.08 ms frame |
| --- | --- | --- | --- |
| 1,000 | 3.59 ns | 3.6 µs | 0.17% |
| 10,000 | 3.69 ns | 36.9 µs | 1.8% |
| 100,000 | 3.69 ns | 369 µs | 17.7% |

Flat per object, which is what a linear pass over two contiguous arrays should be.

**Threading is not applied**, and now for a measured reason rather than a suspicion.
`docs/threading.md` put the job break-even at roughly 12 µs of work per job, so a
10,000-object cull is three chunks — three job submissions to save at most 25 µs. At 100,000
objects it becomes worth doing, and the loop reads slices so chunking is a change at the call
site rather than a rewrite.

## Culling can be switched off

`Renderer::set_culling_enabled(false)` makes the frame draw everything, through the same
loop. That exists so "does culling help?" compares one code path with one step removed,
rather than two different code paths — the same discipline the interleaved A/B rule in
`docs/performance.md` demands of timing runs.

## Tests

Six frustum tests in `engine_core::math` and eight for the culler:

| Test | Asserts |
| --- | --- |
| `frustum_planes_are_normalized` | the distance was scaled with the normal |
| `the_near_plane_uses_the_d3d_depth_convention` | `z`, not `w + z` |
| `a_point_in_front_of_the_camera_is_inside_the_frustum` | behind, too near, too far and off to the side all rejected |
| `a_sphere_straddling_a_plane_counts_as_visible` | conservative, deliberately |
| `a_sphere_at_the_edge_of_view_is_kept` | the plane is where the projection says it is |
| `only_what_is_in_front_of_the_camera_survives` | the whole pass, end to end |
| `a_hidden_entity_is_not_even_considered` | hidden ≠ culled |
| `moving_an_entity_changes_whether_it_survives` | bounds follow transforms |
| `a_large_object_just_off_screen_is_kept` | centre outside, surface inside |
| `scale_grows_the_bound_and_keeps_the_object` | the largest-axis scaling matters |
| `the_cull_ratio_reports_what_the_pass_achieved` | 3 of 4 culled is 0.75 |
| `culling_does_not_allocate_after_the_first_pass` | the visible buffer is reused |

The allocation test is the one that would catch a slow regression: a `Vec` rebuilt each
frame would pass every correctness test and quietly allocate 480 times a second.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Frustum planes extracted from the view-projection | pass |
| Bounding-sphere test | pass |
| Only visible objects recorded | pass |
| Culling statistics available | pass — `CullStats`, `cull_ratio` |
| No per-frame allocation | pass — asserted by test |
| Cost measured | pass — 3.7 ns per object, table above |
| Threading decision based on measurement | pass — 37 µs at 10,000 objects against a 12 µs break-even |
| No GPU culling | correct — out of scope |
| No regression | pass — 0.155 ms |

## Known limitations

* Spheres only. An oriented box would cull long thin objects better and costs more per test;
  §28 says sphere-first, and nothing in the scene is long and thin yet.
* No hierarchical culling — no octree, no BVH, no portal system. At 3.7 ns per object a flat
  pass is affordable well past any scene this engine can currently build, and a spatial
  structure has to be maintained as objects move.
* No occlusion culling. That needs a depth pyramid and a readback or a GPU pass, and §46
  excludes GPU culling from Phase 2.
* The culler runs on the main thread, as measured above.
* Culling is per camera, and there is one camera. Shadow-map culling in a later phase would
  want a second frustum, which is a second `Culler` rather than a change here.

---

# Checkpoint — audit, and the `engine-render-data` split

Between Milestones 15 and 16 the whole of Phase 2 so far was audited against the code
rather than against this document, measured, and corrected where the audit found something
worth correcting. Nothing was implemented ahead of schedule.

## What the audit found

Every milestone from 1 to 15 verified as behaving, not merely existing. Two were downgraded
from "done" to "built but not wired", which is a distinction this document had not been
making:

* **Milestone 4, descriptors.** `GpuDescriptorHeap` — the shader-visible heap with a
  persistent region and a bump region per frame slot — is created by no production code
  path. Nothing samples a texture, so the mesh root signature binds three root CBVs, which
  is cheaper than any descriptor table. Only the RTV and DSV heaps are live.
* **Milestone 5, resources.** `DeferredReleaseQueue::collect` runs every frame but
  `release` is called only from tests. Every destruction the renderer performs — replacing
  the stress scene, resizing the swap chain — happens behind an explicit `wait_for_idle`.

Both are correct, tested, and deliberately staged; both now say so in their module
documentation, because dead code that looks alive invites the assumption that the
capability is there. `InstancedPipeline` and the unwired `render_list.rs` draft were
labelled the same way.

The audit's architecture decisions — continue with a CPU-driven renderer, no render graph,
no bindless, no ECS, no GPU-driven rendering yet — each came with the evidence for them.
The measurement baseline it established is what Milestone 16 was compared against.

## The `engine-render-data` split

The one structural problem worth fixing before Milestone 16. `RenderWorld` is pure data —
transforms, handles, bounds, a visibility flag — but it lived in `engine-renderer` next to
`MeshPipeline`, so anything that wanted to *build* a world had to link D3D12. Including the
Phase 3 gameplay simulation, which must not.

```text
engine-core
    ^        ^
    |        |
engine-render-data <- engine-renderer
    ^
    |
(future) gameplay
```

Moved: `RenderWorld`, `RenderEntity`/`Desc`, the handle types, `world_bounds`, `Lighting`,
`Camera` and `Projection`. `Camera` moved because a camera *controller* is gameplay-shaped
code and has to be able to name the type it drives — the showcase's free-fly camera now
imports nothing from `engine-renderer`, which is what the Phase 3 player camera will need.
It required an error type that carries no `HRESULT`, so `RenderDataError` exists and
converts into `EngineError`.

Deliberately not moved: `Culler`, because visibility is the renderer's policy rather than
data the game authors; `FrameConstants` and `ObjectConstants`, because they mirror an HLSL
`cbuffer` and shader ABI belongs beside the shader; `MeshData` and `MaterialDesc`, because
they are asset-authoring data and belong in a future asset crate.

`engine-renderer` re-exports everything that moved, so `use engine_renderer::RenderWorld`
still compiles. This was a boundary change, not an API one. The absence of `windows` in the
new crate's dependency tree is the property that matters and it is checkable with
`cargo tree`.

## The other corrections

* **A root-signature regression test.** The mesh root parameters are ordered by update
  frequency and the HLSL registers by declaration, so the mapping is `[b0, b2, b1]`. Nothing
  validated it: `SetGraphicsRootConstantBufferView` takes a slot and an address and checks
  neither against the shader, so binding the material where the model matrix belongs
  produces no validation message and no `HRESULT` — just every object collapsed to the
  origin and scaled by a colour. The mapping now lives in one constant the signature is
  built from, and a test reads the registers back out of `mesh.hlsl` with `include_str!` and
  asserts them against it. Mutation-checked in both directions.
* **Five frustum edge-case tests.** The existing ones all placed the camera at the origin
  looking down −Z, which is exactly the configuration where the view matrix is the identity
  — so a bug in how the view participates in plane extraction could not show. Added: the
  vertical planes, the far plane from both sides, a translated camera, a rotated camera, and
  a non-square aspect ratio. All five passed on the first run; what was missing was the
  evidence, not the correctness.
* **`cull_bench`.** A benchmark that measures culling with no device and no draws, across
  three scenarios, counting its own heap allocations with a counting global allocator.
* **`package.ps1`** could not complete under Windows PowerShell 5.1 — with
  `$ErrorActionPreference = 'Stop'`, a native command's stderr becomes a terminating error
  even on exit 0, and cargo writes its progress there, so the verification never ran. Fixed,
  and taught about the `showcase` and `cull_bench` binaries.
* **p50** added to the showcase's report and overlay; the audit could not fill in that
  column.

---

# Milestone 16 — Render list

## What was built

```text
RenderWorld  ->  Culler  ->  RenderList  ->  command recording  ->  D3D12
 (the world)   (visibility)  (this frame)
```

`RenderList` in `engine-renderer/src/render_list.rs`: four parallel arrays holding a compact
copy of what one frame draws.

```rust
pub struct RenderList {
    entities:   Vec<RenderEntity>,    //  8 B
    transforms: Vec<Transform>,       // 40 B
    meshes:     Vec<MeshHandle>,      //  8 B
    materials:  Vec<MaterialHandle>,  //  8 B
}                                     // 64 B per entry
```

`Renderer::render_world` became `build_render_list` followed by `draw_render_list`. The
first is pure CPU and touches no D3D12 object; the second reads the list and the GPU
resources it names, and never looks at the `RenderWorld`.

## A copy, not indices

Three shapes were possible and the compact copy was chosen.

Handles back into `RenderWorld` keep the list to four bytes an entry, and every consumer
then dereferences into five separate arrays in whatever order the list happens to be in.
That is tolerable exactly while the order matches the world's, and stops being tolerable at
Milestone 17: a sorted index array turns every recorded draw into a random access across
the whole world.

An array of structs keeps one object's fields together, which sounds right until you notice
that no consumer reads all of them at once. A sort would move whole 64-byte elements;
instancing wants the transforms contiguous and would get them interleaved with handles it
does not care about.

Parallel arrays cost one sequential copy per visible object at build time and give every
consumer afterwards a dense linear read of exactly the field it wants.

This deviates from §13 of the proposal, which described a `DrawItem { key, entity }` — the
handles shape. The reason is the one above, and it was reached from the audit's scaling
analysis rather than by discovering a problem in code.

## Only what the frame consumes

`RenderWorld` also stores world-space bounds, a visibility flag and generations. None
reaches recording: bounds are read by the culler straight out of the world, the visibility
flag has already been answered — an entry existing *is* the answer — and a generation
detects a stale handle, which a list built and consumed inside one frame cannot hold.

And nothing from gameplay. No health, no AI state, no ability state, no network identity.
That costs nothing to honour, because the list is built from `RenderWorld`, which has never
had any of it.

## The test and the copy are one pass

The culler no longer writes slot indices into a buffer of its own for the recording loop to
walk. The survivor is gathered as it is found: the frustum test has just touched that slot's
bounds, so the object's other fields are in cache at the cheapest possible moment to copy
them — and an object that fails never pulls them through at all, which at a 90% cull ratio
is most of them.

`Culler` therefore holds no buffer. The list *is* the visible set, and two representations
of one answer was the thing worth removing. `cull_stats().visible` and `render_list().len()`
agree by construction; the integration tests assert it on every pass they perform.

## The transform, not the matrix — measured, not reasoned

The obvious design stores the composed `Mat4`: 64 bytes, exactly what a constant buffer
wants, and a contiguous array instancing could upload in one memcpy. It was built that way
first and cost 14% of the frame.

| Variant | CPU mean, ms (6 runs, sorted) | Median |
| --- | --- | --- |
| Before M16 | 0.698 0.715 0.723 0.730 0.734 0.758 | 0.727 |
| `Mat4` | 0.779 0.814 0.814 0.846 0.854 0.867 | 0.830 |
| `Transform` | 0.715 0.716 0.719 0.729 0.757 0.759 | 0.724 |

The matrix is composed once per visible object per frame either way; what changed is where.
In the recording loop it sits between five latency-bound driver calls and disappears into
their shadow. Hoisted into a tight gather loop it has nothing to hide behind. The cull-stage
timer showed it directly — 0.032 ms to 0.093 ms — while the render stage did not drop to
match.

So the list carries the 40-byte `Transform` and recording composes the matrix where it
always did. The list is also smaller, which makes a future sort cheaper, and `translation`
is a better depth key than digging a matrix column out.

**This wants revisiting at Milestone 17.** Instancing removes the per-draw calls the
composition is currently hiding under, and composing a run of transforms straight into the
upload ring becomes the right shape. It is a change to one loop, not to this structure.

## Capacity is reused, and that is asserted

`clear()` keeps every allocation. After the first few frames each array has reached the
scene's high-water mark and the build path allocates nothing at all. Two tests pin it — one
in the module, one in the integration suite — because a list rebuilt each frame would pass
every correctness test and quietly allocate four vectors 240 times a second.

## Measured cost

1920×1080, release, vsync off, 4,000 frames, window n=1024. Runs alternated between the two
binaries on the same machine, because the first attempt at this measurement showed GPU time
rising in lockstep with CPU time across consecutive runs — thermal drift, not code.

| Scene | Before, median | After, median | Verdict |
| --- | --- | --- | --- |
| 10,000 objects | 0.727 ms | 0.756 ms | no meaningful change |
| 100,000 objects | 3.828 ms | 3.882 ms | no meaningful change |

Ranges overlap fully at both scales; the medians differ by less than the spread within a
single variant. That is the correct outcome for an architectural milestone, and it is said
plainly rather than dressed up.

Steady-state heap allocations during the build pass, from `cull_bench`'s counting allocator:

| Scenario | 10,000 | 100,000 |
| --- | --- | --- |
| all visible | 0 | 0 |
| 99% outside | 0 | 0 |
| 50% visible | 0 | 0 |
| `accept_all` | 0 | 0 |

The isolated cull numbers rose — 4.45 to 9.59 ns per object at 100,000 all-visible, and
`accept_all` from 0.50 to 6.56 — because the pass now includes the gather that used to live
in the recording loop. Work moved; the frame measurement is what says it was not added.

## Tests

Nine unit tests on the list, ten integration tests on the pipeline, one on `entity_at`.

| Test | Asserts |
| --- | --- |
| `an_empty_world_produces_an_empty_render_list` | 0 objects, 0 entries |
| `a_fully_visible_world_produces_one_entry_per_object` | N visible, N entries |
| `a_fully_culled_world_produces_an_empty_render_list` | N invisible, 0 entries |
| `mixed_visibility_produces_exactly_the_visible_entries` | interleaved, so a run-copy bug shows |
| `an_entry_carries_the_data_its_entity_was_spawned_with` | entity, transform, mesh, material |
| `a_hidden_entity_never_reaches_the_render_list` | hidden is still not culled |
| `rebuilding_leaves_no_trace_of_the_previous_frame` | no stale entries through any accessor |
| `repeated_frames_stop_allocating` | capacity reused |
| `moving_the_camera_changes_what_the_list_holds` | translation and rotation both re-select |
| `accepting_everything_bypasses_the_test_but_fills_the_same_list` | the F8 path has the same shape |
| `a_stored_transform_still_composes_the_matrix_recording_needs` | the copy round-trips |
| `an_entry_is_the_size_the_layout_reasoning_assumes` | 64 bytes, so the costing stays honest |
| `every_array_stays_the_same_length` | index `i` means one object in all four |

The helper every integration test funnels through asserts `stats().visible == list.len()` on
each pass, so the statistic and the list cannot drift apart without the suite noticing.

## Acceptance criteria

| Criterion | Result |
| --- | --- |
| Compact SoA render list | pass — four arrays, 64 B per entry |
| Only renderer-relevant fields copied | pass — bounds, flags and generations excluded, with reasons |
| Owned by the renderer, not `engine-render-data` | pass |
| No D3D12 or `windows` types in the list | pass |
| Per-frame transient, storage reused | pass — `clear()` keeps capacity |
| No per-object heap allocation | pass — 0 allocations at 10k and 100k |
| Culling feeds the list; invisible objects never reach recording | pass |
| Visibility and recording separated | pass — `build_render_list` / `draw_render_list` |
| Statistics real, not duplicated | pass — overlay shows `VISIBLE == RENDERLIST` |
| No sorting | correct — out of scope |
| No batching or instancing | correct — out of scope |
| Single-threaded | correct — out of scope |
| Visual behaviour unchanged | pass — captured and compared |
| No meaningful performance change | pass — table above |

## Known limitations

* **The gather is real work with no payoff yet.** About 113 KB per frame at 10,000 objects
  and 1.1 MB at 100,000. It currently pays for itself by removing the scattered reads it
  replaced, which is why the frame is flat. Milestones 17 and 18 are what make it
  profitable; if they do not, the copy deserves re-examining rather than defending.
* **Storing `Transform` is right today and probably wrong after instancing**, for the reason
  above.
* The list is unsorted, so runs of identical mesh and material are contiguous only by
  accident of spawn order.
* One list, one camera, one thread. A second camera — shadow mapping — wants a second list,
  which is a second `RenderList` rather than a change here, exactly as a second frustum is a
  second `Culler`.
* `RenderListEntry` gathers a whole entry for tests and diagnostics. Nothing on the frame
  path uses it, and nothing should: the arrays exist because the common operations read one
  field across many entries.
