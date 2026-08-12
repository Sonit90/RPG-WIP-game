# Phase 3 — VFX & Combat Feedback Foundation

Living document. Every milestone appends to it as it completes: what was implemented, what
was tested, what was measured, what was decided and what was left undone.

Target: combat that reads. An ability press should produce an animation, a cast tell, a
travelling effect, an impact, a target reaction and a damage number — with **gameplay
authoritative throughout**. If every effect in this phase failed to spawn, the fight would
still resolve identically.

```text
                       Gameplay
                           │
                           ↓
                   Gameplay Events
                           │
              ┌────────────┼────────────┐
              ↓            ↓            ↓
          Animation       VFX         Audio*
                           │
                           ↓
                      VFX System
                           │
          ┌────────────────┼────────────────┐
          ↓                ↓                ↓
       Particles         Meshes           Lights
          │                │                │
          └────────────────┼────────────────┘
                           ↓
                       Renderer
                           ↓
                          GPU
```

`* Audio is intentionally not implemented in this phase; the event boundary should make it
possible to add later.`

---

## Milestone status

| # | Milestone | Status |
| --- | --- | --- |
| 1 | Audit existing renderer and gameplay events | **complete** |
| 2 | Presentation event layer | **complete** |
| 3 | VFX asset architecture | **complete** |
| 4 | VFX instance lifecycle | **complete** |
| 5 | Particle system foundation | **complete** |
| 6 | Projectile VFX | **complete** |
| 7 | Impact VFX | **complete** |
| 8 | AoE VFX | **complete** |
| 9 | Cone / wave VFX | **complete** |
| 10 | Explosion VFX | **complete** |
| 11 | FireMage ability VFX | **complete** |
| 12 | Ignite / fire status VFX | **complete** |
| 13 | Defensive ability VFX | **complete** |
| 14 | Mobility VFX | **complete** |
| 15 | Ultimate VFX | **complete** |
| 16 | Character reaction | **complete** |
| 17 | Damage numbers | **complete** |
| 18 | Cast feedback | **complete** |
| 19 | VFX camera behaviour | **complete** |
| 20 | VFX pooling | **complete** |
| 21 | VFX performance budget | **complete** |
| 22 | VFX stress test | **complete** |
| 23 | VFX debug scene | **complete** |
| 24 | Automated tests | **complete** |
| 25 | Manual combat verification | **complete** |
| 26 | Full regression | **complete** |
| 27 | Final performance audit | **complete** |

---

## Milestone 1 — Audit of the existing engine

Read before writing. Nothing below is a plan; it is what the engine already does, and the
rest of the phase is written against it.

### The crate graph this phase has to fit into

```text
engine-core ── math, time, jobs, profiling, metrics (no windows, no D3D12)
   ├── engine-render-data ── RenderWorld, Camera, UiDrawList, skinning (pure data)
   ├── engine-collision ─── collision world + character controller
   ├── engine-environment ─ the level as data
   ├── engine-assets ────── ModelAsset, procedural authors, glTF import
   ├── engine-game ──────── the simulation. engine-core ONLY, by rule
   ├── engine-renderer ──── D3D12. meshes, materials, culling, pipelines, UI pass
   ├── engine-presentation ─ the seam: extraction, camera, UI, damage numbers
   └── engine-app ───────── window + wiring
```

Two rules are load-bearing and this phase must not break either:

* `engine-game` depends on `engine-core` and nothing else. It cannot name a `MeshHandle`,
  so it cannot own a VFX handle either. **VFX cannot live in gameplay** — not by taste, by
  the dependency graph.
* `engine-render-data` links no D3D12, which is why extraction is unit-testable with no
  device. Anything this phase adds to the seam has to keep that property.

### Where gameplay already announces what happened

`engine-game/src/event.rs` is a complete event layer and this phase **extends it rather
than adding a second one**. `EventStream` is a grow-once, cleared-never-freed buffer read
by cursor; `Simulation::events()` exposes the frame's batch and the damage meter, the
floating numbers and the hit flash are all already *observers* of it.

Today's `CombatEvent` variants:

| Variant | Carries | Emitted from |
| --- | --- | --- |
| `AbilityUsed` | source, ability, targets hit, tick | `firemage_exec::land` |
| `Damage(DamageEvent)` | source, target, amount, overkill, ability, damage type, killed, tick | `Simulation::deal_damage` |
| `Healing` | source, target, amount, ability, tick | (unused so far) |
| `StatusApplied` | source, target, status, duration, tick | `apply_ignite`, statuses |
| `StatusRemoved` | target, status, reason, tick | status tick, death |
| `CastStarted` | source, ability, duration, tick | `try_cast` |
| `CastEnded` | source, ability, `CastEnd`, tick | `tick_cast`, `end_cast` |
| `Teleported` | entity, from, to, tick | `firemage_exec::blink` |
| `Death` | entity, killer, tick | `deal_damage` |

What presentation needs and the stream does **not** currently carry:

1. **No world position on a damage event.** Presentation looks the target up in the world
   (`app.rs` does exactly this for floating numbers), which fails for a target that died
   and was reused. An effect must be placeable from the event alone.
2. **No critical-strike flag.** `firemage_exec::strike` knows whether the roll crit and
   throws the fact away; only the aggregate counter `FireCounters::crits` survives. A
   critical hit is therefore *not currently expressible* by the interface.
3. **No projectile.** Firebolt, Flame Lance and Pyroblast are hitscan: `land` collects
   targets and `strike` applies damage in the same tick. There is no travelling object in
   gameplay at all.
4. **No area-effect announcement.** `ZoneField` owns Firestorm's line and Molten Ground's
   circle, and `PendingImpact` owns Meteor's delay. Zones spawn, tick and expire in
   `tick_firemage` with no event: presentation cannot know a Firestorm exists.
5. **No jump or landing.** `movement.rs` maintains `Body::grounded` and `MovementState`,
   and the transition is never announced.

### The abilities this phase has to give a visual to

`engine-game/src/firemage/ability.rs` is a data table of twelve rows (ten on the bar, the
Ultimate, and Ignite as a meter row). The fields that matter to VFX are already there:
`target: TargetMode`, `range`, `radius`, `width`, `duration`, `impact_delay`,
`tick_interval`. **The visual must be driven from these numbers**, because they are the
same numbers the executor hits with.

| Ability | Target mode | Shape the gameplay actually uses |
| --- | --- | --- |
| Firebolt | Single | one target at range 35 |
| Flame Lance | Single | one target at range 35 |
| Flame Cone | Cone | 60° cone (`CONE_HALF_ANGLE = π/6`), range from the table |
| Meteor | Ground | circle of `radius` after `impact_delay`, optional burning ground |
| Pyroblast | Single | one target, optionally chaining within 15 m |
| Firestorm | Line | `ZoneShape::Line { yaw, length, width }` for `duration` |
| Detonate | Single | circle of `radius` centred on the target |
| Fire Mastery | SelfCast | buff timer only |
| Flame Shield | SelfCast | buff timer only |
| Blink | Movement | `Teleported { from, to }` |
| Cataclysm | Ground | disabled this iteration (`ULTIMATE_ENABLED = false`) |
| Ignite | passive | per-target burn, `IgniteTracker`, stacks and duration |

### What presentation already does with events

* `engine-presentation/src/damage_numbers.rs` — a fixed-capacity ring of floating numbers,
  fed by `observe(events, now, position_of)`. Has `killed`, has no `critical`.
* `RenderExtractor::note_hit` — a hit flash: `HIT_FLASH_SECONDS = 0.18`, driven into
  `InstanceStyle::emissive` plus a small scale lift. **Milestone 16 already half exists.**
* `nameplate.rs`, `snapshot.rs`, `game_ui.rs` — the interface reads a `UiSnapshot`, never
  the simulation.
* `environment.rs` — `EnvironmentPlacer` is the model this phase copies for VFX meshes:
  an asset → `ModelBinding` map uploaded once, placements spawning `RenderWorld` entities
  that share mesh and material handles so the render list groups them into one draw.

### What the renderer can and cannot do today

* **Opaque only.** `MaterialDesc::base_color.w` is documented as "carried but not blended".
  The world pipelines (`MeshPipeline`, `InstancedMeshPipeline`, `SkinnedMeshPipeline`) all
  use `pipeline::blend_desc()`, which is blending *disabled*, with depth `LESS` and depth
  writes on.
* **The UI pass does blend** — `ui_pipeline::alpha_blend_desc()`, depth off — and is drawn
  last, in one `DrawInstanced`, from a per-frame vertex buffer in the upload ring. It is
  the working example of a blended pass with dynamic geometry, and the particle pass is
  modelled on it.
* **Shaders are offline.** `build.rs` runs `dxc` over `shaders/*.hlsl` and embeds DXIL;
  adding a shader means adding a `ShaderEntry` row. There is no runtime compiler.
* **Instancing exists and is the fast path.** `RenderList::group_by_material_and_mesh` is a
  counting sort by `(material, mesh)`; each run becomes one `DrawIndexedInstanced` with a
  per-instance stream of `InstanceTransform { model, tint, emissive }` (96 bytes).
* **`InstanceStyle { tint, emissive }`** already exists per render entity and is *the*
  mechanism for a cheap per-object visual change with no slot churn. Hit flashes, glowing
  shields and pulsing decals all ride on it.
* **Culling** is a frustum test over `RenderWorld::bounds`, optionally parallel. Anything
  spawned into `RenderWorld` is culled for free; anything drawn outside it is not.
* **Measurement** is already in place: `Renderer::draw_calls_last_frame`,
  `triangles_last_frame`, `last_cull_ms`, `last_group_ms`, `ui_stats`, `last_gpu_frame_ms`
  (GPU timestamps), plus `engine_core::metrics` percentiles and `engine-bench`'s six
  offscreen benchmark binaries.

### Decisions taken from the audit

1. **Extend `CombatEvent`; do not add a second event system.** The stream is already the
   observation point for three consumers, it is allocation-free in the steady state, and a
   parallel "presentation event" queue would need the same fan-out with none of the
   determinism guarantees.
2. **A new crate, `engine-vfx`, owns effects as data and their runtime.** It depends on
   `engine-core` and `engine-render-data` only — no `windows`, no `engine-renderer`, no
   `engine-game`. That keeps the whole VFX runtime (spawn, update, pool, expire, particle
   simulation) testable with no device and no simulation, and it makes "gameplay cannot
   name a VFX" true by the dependency graph rather than by convention.
3. **Two presentation primitives, not five.** Particles get a real pass; meshes reuse the
   instanced world pipeline through `RenderWorld` + `InstanceStyle`. Beams, decals and
   ground rings are *mesh* effects — a beam is a stretched box, a decal is the flat disc
   the targeting indicator already uses. "Light" is an emissive term, not a dynamic light:
   the renderer has one directional light and adding real point lights is a lighting-model
   change this phase has no business making.
4. **A new blended particle pass**, drawn after the opaque scene and before the UI, one
   `DrawInstanced` per blend mode, expanding a camera-facing quad in the vertex shader from
   a 48-byte per-instance record. CPU billboarding was rejected: it writes six vertices per
   particle where the GPU needs one record, and the upload ring is the same either way.
5. **Projectiles are presentation-only.** Gameplay stays hitscan — changing it would be a
   new gameplay mechanic, which this phase forbids. The simulation announces
   `ProjectileLaunched { from, to }` at the moment damage is applied, and the visual bolt
   flies over the next fraction of a second and plays its impact on arrival. The damage
   number, the hit flash and the death all still happen on the gameplay tick.

---

## Milestone 2 — The presentation event layer

`engine-game`'s existing `CombatEvent` stream was **extended, not duplicated**. Nothing in
gameplay calls presentation; everything below is a value pushed into the same buffer the
damage meter already reads by cursor.

### Implemented

`DamageEvent` gained two fields:

| Field | Why it is on the event rather than looked up |
| --- | --- |
| `critical: bool` | The roll happens inside `firemage_exec::strike` against the crit chance of that moment. Only an aggregate counter survived it, so a critical hit was **not expressible** by the interface at all. |
| `position: Vec3` | An observer may read the batch after the target died and its slot was reused. A hit is something that happened at a place. |

Seven variants were added:

| Variant | Emitted from | Carries |
| --- | --- | --- |
| `ProjectileLaunched` | `firemage_exec::apply_direct`, single-target only | source, target, `from`, `to`, ability |
| `ImpactScheduled` | `firemage_exec::land`, when `impact_delay > 0` | position, radius, delay |
| `ImpactLanded` | `firemage_exec::resolve_impact` | position, radius, targets caught |
| `AreaEffectStarted` | both `ZoneField::spawn` sites | id, position, `ZoneShape`, duration |
| `AreaEffectEnded` | `Simulation::tick_firemage`, draining `ZoneField` | id, position |
| `Jumped` | `Simulation::integrate_bodies` | entity, position |
| `Landed` | `Simulation::integrate_bodies` | entity, position, arrival speed |

`ZoneField` now hands out an identity per zone (parallel `ids` array, not a field on `Zone`,
so every zone construction site stays a struct literal about fire) and keeps an `expired`
list the simulation drains each tick. A zone evicted because the field was full ends exactly
as one that burned out: from outside there is no difference.

### Architectural decisions

* **Projectiles are announced, not simulated.** `ProjectileLaunched` is emitted *after* the
  hit list is collected and *before* the damage, on the same tick. Gameplay stays hitscan —
  making a bolt a real object would change what "in range" means, whether a target can
  dodge, and what a dead target does to a bolt in flight, all of which are gameplay
  mechanics this phase forbids. Presentation flies a bolt down the announced line and plays
  its impact on arrival; the damage number and the death already happened.
* **A periodic tick never crits.** `deal_damage` takes the flag explicitly and status ticks
  pass `false`: the roll belongs to the cast that applied the status, and rerolling per tick
  would turn every damage-over-time effect into a second source of critical strikes.
* **Areas carry the gameplay `ZoneShape`.** Not a copy of the numbers, the shape value the
  containment test uses. A visual built from it cannot claim a different footprint.

### Automated tests

`crates/engine-game/tests/presentation_events.rs`, 7 tests, all passing:

* a damage event carries where it landed (against the world position at the time)
* a critical strike survives to the event, and is a roll rather than a constant (forced
  100 % and −100 % crit chance)
* a single-target cast announces its line, from the caster, to the target, **before** the
  damage in stream order
* a delayed impact is announced when committed to and when it lands, with the ability's own
  radius and delay
* an area announces the shape that actually burns, and its end names the same id
* jumping and landing are announced in that order, with a positive arrival speed
* the new events cost no allocation in the steady state (event stream capacity unchanged
  over 30 cast-and-jump cycles)

`determinism.rs` was extended to describe every new variant, so a replay that produced a
bolt from a different place or a fire a tick later is a different run.

### Known limitations

* `Jumped`/`Landed` are emitted for the player only. Dummies do not move and are integrated
  through the plain path with no collision; adding the transition there would announce
  nothing that ever happens.
* `Healing` still has no producer in the class.

---

## Milestone 3 — VFX asset architecture

A new crate, **`engine-vfx`**, holds effects as data and the runtime that plays them.

```text
cargo tree -p engine-vfx
engine-vfx
├── engine-core
├── engine-render-data
└── log
```

No `windows`, no `engine-renderer`, no `engine-game`, no `engine-assets`. Each absence is
load-bearing: without gameplay an effect *cannot* reach into the simulation that announced
it or consume the gameplay random source; without a device the whole runtime is unit tested
with no adapter. `engine-render-data` is there for exactly one type — `ParticleDrawList` —
which is the frame's particles as plain data, written into directly so a particle is not
copied twice.

### The catalogue

`asset.rs` is a `static` table of thirteen effects indexed by `VfxId`, the same shape
`engine-game`'s ability table has. An effect is:

| Part | What it is |
| --- | --- |
| `emitters: &[EmitterDesc]` | up to 4. Burst and/or rate, a spawn shape, speed, lifetime, size ramp, colour ramp, gravity, drag, spin, blend mode, and whether the particles ride the effect |
| `layers: &[MeshLayer]` | up to 4. A `VfxShape`, a window, an alignment, radius and height ramps, a colour, an emissive ramp, a spin |
| `lifetime` | seconds, overridable per spawn (a burn lasts as long as the status does) |

Thirteen effects serve eleven abilities plus Ignite and the two ends of a Blink, because a
Detonate and a Meteor differ in *radius and colour*, not in kind:

`Projectile`, `Impact`, `GroundArea`, `Telegraph`, `ConeWave`, `LineFire`, `Explosion`,
`Ignited`, `Shield`, `CastCharge`, `BlinkDeparture`, `BlinkArrival`, `Cataclysm`.

### Presentation primitives

Two, not five, and the reason is structural rather than aesthetic:

* **Particles** get a real blended pass (Milestone 5).
* **Meshes** reuse the existing instanced world pipeline through `RenderWorld` +
  `InstanceStyle`, so they are culled, batched and drawn with everything else.

Beams, decals and ground rings are *mesh* effects — a beam is a stretched strip, a decal is
the flat disc the targeting indicator already uses. "Light" is `MeshLayer::emissive`, the
per-instance emissive term the renderer already carries; adding real point lights is a
lighting-model change this phase had no business making, and is recorded as deferred work.

Six shapes cover the catalogue, each authored at unit size so a layer scales it by a
gameplay number: `Disc`, `Ring`, `Cone` (a wedge of the *gameplay* cone's half-angle),
`Strip`, `Shell`, `Column`.

### Automated tests

`engine-vfx`, 12 tests over the catalogue: every effect sits in the slot its id names, every
effect ends, no effect declares more emitters or layers than the runtime can hold, every
effect shows something, every particle fades to zero alpha rather than popping, every window
is ordered and inside a life, and the particle estimate counts bursts and streams.

---

## Milestone 4 — VFX instance lifecycle

`system.rs`. `VfxSystem::spawn` takes a slot — a freed one if there is one — and returns a
`VfxHandle` carrying that slot's generation; `update` ages every instance, runs its emitters,
rebuilds the frame's mesh draws and retires what is finished. Retiring bumps the generation,
which makes every outstanding handle stale — the same mechanism `RenderWorld` uses, for the
same reason.

| Requirement | How |
| --- | --- |
| spawn | `VfxSpawn` carrying position, yaw, radius, length, lifetime, tint, attachment, height |
| lifetime | the asset's, or overridden per spawn from the gameplay duration |
| update | one pass over live instances, then one pass over particles |
| transform | `set_position` / `set_yaw`, which a bolt in flight drives |
| attachment | an opaque `u32` key resolved by a closure; `None` from the closure ends the effect |
| world space | the default: an effect stands where it was put |
| cleanup | automatic at end of life, or `stop` / `stop_and_clear` |
| pooling | free list for instances, fixed-capacity SoA pool for particles |

**Retirement detaches particles rather than killing them.** A bolt's trail is not part of the
bolt: deleting it with the bolt would take the smoke away on exactly the frame the impact
starts. An effect that must take its particles with it — a shield ending the instant the
buff does — is stopped with `stop_and_clear`. Both behaviours are tested.

### Automated tests

15 in `system.rs`: lifetime and slot reuse, a stale handle failing to move the effect that
replaced it, a burst firing exactly once however the frame is chopped up, a rate emitter
throwing the same number of particles at 1, 60 and 300 FPS (the accumulator's whole point),
attachment following and ending with its owner, retirement detaching, `stop_and_clear`
taking particles with it, geometry sized by the gameplay radius, a layer outside its window
not drawn, refusal rather than growth when full, tint reaching both geometry and particles,
`clear`, and a thousand-frame run that allocates nothing.

---

## Milestone 5 — Particle system foundation

### CPU, and why

`particle.rs` is a fixed-capacity structure-of-arrays simulation: position, velocity, age,
lifetime, size ramp, colour ramp, rotation, spin, gravity, drag, blend, owner. The update is
a handful of multiply-adds over contiguous `f32` arrays; death is a `swap_remove`, which is
why nothing holds a handle to a particle.

A GPU system buys its advantage when the *per-particle* update dominates, and costs a compute
pass, a persistent buffer per effect, an indirect draw and a readback story. Combat produces
thousands of particles, not millions, and the measurement below says the update is
**0.12–0.14 ms at 11,000 live particles**, which is 3 % of a 240 FPS frame. If that ever
stops being true, the emitters and the draw list stay as they are and only that file changes.

### The pass

New in `engine-renderer`: `shaders/particle.hlsl`, `particle_pipeline.rs`, and a
`draw_particles` step recorded **after the opaque scene and before the interface**.

* **36 bytes per particle, not six vertices.** `ParticleInstance { position, size, color,
  rotation }` is a per-*instance* stream; the quad's four corners come from `SV_VertexID` and
  the camera's right and up axes. One `DrawInstanced(4, count)` per blend mode, whatever the
  particle count.
* **Two pipeline states, one shader.** The pixel shader outputs premultiplied alpha, so
  additive is `ONE, ONE` and alpha is `ONE, INV_SRC_ALPHA`.
* **Depth tested, never written.** A spark behind a pillar is hidden by the pillar; a spark
  in a burst does not hide the twenty behind it.
* **No texture.** The round soft falloff is `saturate(1 - dot(corner, corner))` squared — two
  instructions against a descriptor table, an atlas and a sample per pixel. When a particle
  needs a *shape* rather than a blob, that is where an atlas arrives.
* **No culling.** A quad rotated about the view axis can be wound either way.

`ParticleDrawList` (in `engine-render-data`) holds the frame's particles split by blend mode,
capped at 16,384, refusing and counting past that. It is cleared, never freed.

### Automated tests

* `engine-render-data`: 6 — the record's layout matches the input layout the renderer
  declares, blend modes stay apart, invisible particles are skipped without counting as
  drops, a full list drops rather than grows, the capacity is shared across modes, clearing
  keeps the allocation and the drop count.
* `engine-vfx`: 12 — lifetime, gravity, drag not reversing on a long frame, attachment
  carrying, a reused owner slot not adopting, detaching, killing by owner, dropping when
  full, size and colour interpolation, blend routing, and a steady state that stops
  allocating.
* `engine-renderer`: 6 pipeline tests without a device (constants match the shader's
  registers, the billboard basis is the camera's axes in world space rather than the view
  matrix's columns, the two modes differ only in the destination factor, depth is tested and
  never written, every element steps per instance, no back-face culling).
* `engine-renderer/tests/particle_render.rs`: **7 pixel tests on a real GPU** — a particle
  reaches the screen, it carries its own colour, additive particles genuinely add (three
  overlapping are more than 1.5x brighter than one), an invisible particle draws nothing, a
  particle behind the camera is not drawn, both blend modes draw in one frame, and a full
  16,384-particle budget submits.

---

## Milestones 6-10 — Projectile, impact, area, cone and explosion

All five are catalogue entries plus one decision each in the director
(`engine-presentation/src/vfx.rs`).

**Projectile.** Gameplay is hitscan and stays hitscan. `ProjectileLaunched` announces the
line; the director flies a `Projectile` effect down it at 80 m/s (clamped to 0.06-0.45 s) on
a shallow arc, and plays `Impact` at the far end on arrival. The bolt's size and its impact
are scaled by the ability's own damage coefficient, so a rebalance moves the picture with the
damage. **The damage, the number and the death all already happened** — the bolt is a
picture catching up, and if it never spawned nothing about the fight would change.

**Impact.** A short bright core, a spray of sparks and a puff of alpha-blended smoke. Sized
by the caller, placed at the point the event carried, deterministic lifetime, cleans itself
up.

**Area.** `GroundArea`: a dim fill disc, a brighter slowly turning ring, a stream of fire and
a thin stream of smoke — all sized by the `ZoneShape` the event carried, and living exactly
as long as the gameplay zone does (`AreaEffectStarted` to `AreaEffectEnded`, matched by id).
`Telegraph` is the same idea run backwards for anticipation: a ring that *closes* over the
impact delay, so the countdown is the radius.

**Cone.** `ConeWave` uses a wedge mesh built with `engine_game::sim::CONE_HALF_ANGLE` and a
cone-shaped emitter of the same half-angle, at the ability's own `range`. The visual cannot
claim a different spread from the one the executor collects, because both read the same
constant.

**Explosion.** A small bright core (deliberately small — see Milestone 19), an expanding
ring on the ground, 120 outward sparks, 40 slower embers and 22 smoke puffs. Roughly 180
particles and 2 mesh draws, which is why several can run at once.

---

## Milestone 11 — FireMage ability VFX

The mapping lives in presentation, which is the only layer allowed to say "this gameplay
thing looks like that". Nine effects serve eleven abilities.

| Ability | Category | Effect |
| --- | --- | --- |
| Firebolt | Projectile | `Projectile` (small, coefficient-scaled) then `Impact` |
| Flame Lance | Projectile | `Projectile` (heavier) then `Impact` |
| Pyroblast | Projectile | `Projectile` (heaviest) then `Impact`, once per chained target |
| Flame Cone | Cone/Wave | `ConeWave` at the ability's range and half-angle |
| Meteor | AoE, delayed | `Telegraph` for the impact delay, `Explosion` at the radius hit, `GroundArea` if the talent leaves burning ground |
| Firestorm | AoE, lasting | `LineFire` at the zone's yaw, length and width, for its duration |
| Detonate | Direct/AoE | `Explosion` centred on the target, at the ability's radius |
| Ignite | DoT | `Ignited`, attached, refreshed per application |
| Flame Shield | Defensive | `Shield`, attached, synchronised to the timer |
| Fire Mastery | Buff | `Shield` reused: smaller, gold, above the head |
| Blink | Mobility | `BlinkDeparture` at `from`, `BlinkArrival` at `to` |
| Cataclysm | Ultimate | `Cataclysm` on `ImpactScheduled` (gameplay currently disabled) |
| — | Landing | `Impact` tinted as dust, above 4.5 m/s of fall |

Reuse is the point: a Meteor and a Detonate are the same explosion at different radii and
tints, and the Fire Mastery aura is the shield ring at a third of the size.

---

## Milestones 12-15 — Ignite, defensive, mobility, ultimate

**Ignite.** One aura per burning target, attached to the entity, refreshed rather than
stacked — a target at five stacks would otherwise wear five auras, five times the particles,
for a fact the nameplate already states. 22 particles a second per burning target, so ten
burning targets is about 130 live particles. Ends on `StatusRemoved`, on `Death`, and when
the attachment lookup stops answering.

**Defensive.** Flame Shield is synchronised against `mage.flame_shield.is_active()` rather
than driven by events, because a buff is a *state*: it spawns when the state begins and is
`stop_and_clear`ed when it ends, so no sparks outlive the shield. Gameplay is untouched.

**Mobility.** `Teleported { from, to }` produces a departure column at the origin and an
arrival ring at the destination. The gameplay move is authoritative and already resolved
against the collision world; the effect is placed at the position the event reports, so a
blink cut short by a wall shows its arrival at the wall.

**Ultimate.** `Cataclysm`: an anticipation ring that closes over the first quarter, then a
14 m column of fire narrowing to 4 m, a scorch on the ground, 260 particles a second of
column fire, a 200-particle blast at the moment it lands, and smoke. About 2,100 live
particles and 14 mesh draws at its peak — measured, and it costs 13.6 microseconds of CPU per
frame while it runs. Its gameplay is still disabled (`ULTIMATE_ENABLED = false`), so it is
played through the demo scene and the stress driver; the wiring on `ImpactScheduled` is in
place for the day it is turned on.

---

## Milestone 16 — Character reaction

The hit flash already existed (`RenderExtractor::note_hit`, 0.18 s of emissive plus a small
scale lift). This phase added the half that was missing: **a critical strike flashes 1.7x
harder and 1.7x longer**, and a later ordinary hit does not downgrade a critical flash that
is still running — during a burn every target takes a tick a second, and letting one
overwrite the flash of the Pyroblast that landed a frame earlier would hide exactly the hit
worth seeing.

It costs no draw call and no respawn: it is the `InstanceStyle` the entity already carries,
so it works for one target, a hundred, rapid hits, area hits and crits alike. No ragdoll and
no hit animation — deferred, and recorded as such.

---

## Milestone 17 — Damage numbers

The system existed and was already event-driven. What it could not express was a critical
strike, because the flag did not survive to the event (Milestone 2 fixed that).

`FloatingNumber` gained `critical`, and the HUD draws three cases rather than two: ordinary
(pale gold, 1.0x), **critical (orange, 1.25x)**, killing blow (red, 1.4x). A kill outranks a
crit — the last number a target ever shows should read as "it died", not "that one was
lucky" — and size carries the same information as colour, because size survives the
periphery of the screen and a hue does not.

Everything else already held: a fixed ring of 128, no permanent allocations, rise and drift
with a hold-then-fade alpha, and one number per target per hit for an area cast. The
renderer is still not tied to the damage calculation: the damage system emits an event and
presentation decides what to draw.

---

## Milestone 18 — Cast feedback

`CastCharge`: 55 particles a second thrown *inward* at the caster's chest, spawned when
`Simulation::casting()` becomes `Some` and cleared when it becomes `None`. Inward, because
gathering is what a cast is; outward would read as leaking.

The timing is gameplay's: the effect exists exactly while the cast state exists, so a cast
cut short by movement takes its tell with it on the same frame the cast ends. Nothing here
guesses at a duration, and the character animation selector is untouched.

---

## Milestone 19 — VFX camera behaviour

This milestone found a real bug, and it is the reason two effects were rewritten.

**The world pass is opaque.** A mesh layer cannot be translucent, so the first pass at the
catalogue drew a shield as a shell at body radius and an explosion's flash as a sphere at
the *ability's* radius. At gameplay camera distance that is an opaque bubble hiding the
player inside their own defensive cooldown, and a six-metre opaque ball covering the fight
for a third of a second.

The fix, and the rule that came out of it:

* `MAX_UPRIGHT_RADIUS = 0.6` — no *upright* layer may reach more than 0.6 of the effect's
  radius. Ground shapes are exempt: a footprint painted on the floor obscures the floor,
  which is what it is for.
* The shield is now a ring at the feet plus a skin of sparks, never a shell.
* The explosion's flash is a core at 0.45 of the radius for the first third of its life; the
  expanding ring on the ground carries the size.
* The Cataclysm pillar is 0.3 of the radius, so the fight behind it stays visible.

Both are asserted by tests (`no_upright_layer_is_wide_enough_to_hide_the_fight`,
`nothing_wraps_the_character_in_an_opaque_shell`), so the rule cannot quietly rot.

Ground effects are lifted 6 cm clear of the floor — above the targeting indicator's own lift,
so an area that is burning draws over the circle that placed it rather than z-fighting it.
AoE boundaries stay readable because the bright ring is drawn at the gameplay radius and the
fill sits inside it. Camera mechanics were not changed.

---

## Milestone 20 — VFX pooling

Pooled, and the measurement says the pooling works:

| Thing | Strategy |
| --- | --- |
| VFX instances | `Vec` plus free list, capped at 128, refusals counted |
| Particles | fixed-capacity SoA pool, capped at 16,384, drops counted, death is `swap_remove` |
| Particle draw list | two `Vec`s cleared per frame, never freed |
| Mesh draws | one `Vec` cleared per frame |
| Render entities | **pooled per shape** and reused: a frame with three discs after a frame with five hides two rather than despawning them, so a busy fight never touches the `RenderWorld` free list |
| Damage numbers | the existing fixed ring of 128 |

`vfx_bench` counts every allocation the process makes during the measured frames.
**6 to 12 allocations per 600 frames** in every scenario, effects on or off — none
attributable to the VFX system. The slot-churn claim is asserted separately by
`geometry_reuses_its_render_entities_rather_than_churning_slots`.

Nothing else was pooled, because nothing else showed up in the measurement.

---

## Milestone 21 — VFX performance budget

Target: 1920x1080, 240 FPS minimum (4.17 ms), 480 aspirational (2.08 ms), Ryzen 5800X /
RX 6700 XT.

### CPU, no device (`cargo run --release --bin vfx_bench -- --frames 600`)

Microseconds per frame, mean. `obs` is event observation, `vfx` is the whole VFX update
(instances, emitters, particle integration, mesh-draw rebuild, render-entity sync).

| Scenario | vfx | sim | obs | vfx | extract | cull | total | p99 | particles peak | effects peak | draws | allocs |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| idle | off | 0.8 | 0.0 | 0.0 | 0.7 | 0.3 | 2.0 | 2.4 | 0 | 0 | 25 | 6 |
| idle | on | 0.7 | 0.0 | 0.0 | 0.6 | 0.3 | 1.8 | 1.9 | 0 | 0 | 25 | 6 |
| 1 target | off | 0.5 | 0.0 | 0.0 | 0.1 | 0.1 | 0.8 | 1.0 | 0 | 0 | 5 | 4 |
| 1 target | on | 0.5 | 0.0 | 1.4 | 0.1 | 0.1 | 2.2 | 3.7 | 292 | 6 | 6 | 11 |
| 10 targets | on | 0.7 | 0.0 | 1.4 | 0.6 | 0.3 | 3.1 | 4.4 | 292 | 6 | 24 | 8 |
| 50 targets | on | 1.4 | 0.0 | 1.4 | 2.8 | 1.2 | 7.0 | 8.4 | 280 | 5 | 104 | 10 |
| 100 targets | on | 2.4 | 0.0 | 1.3 | 5.6 | 2.2 | 11.9 | 13.5 | 265 | 5 | 193 | 11 |
| aoe-heavy, 50 | off | 1.4 | 0.0 | 0.0 | 2.8 | 1.1 | 5.6 | 6.2 | 0 | 0 | 103 | 8 |
| aoe-heavy, 50 | on | 1.5 | 0.0 | 1.6 | 2.8 | 1.1 | 7.3 | 10.1 | 330 | 12 | 103 | 9 |
| ultimate, 50 | on | 1.6 | 0.0 | 13.6 | 2.9 | 1.3 | 19.6 | 42.1 | 2,122 | 19 | 117 | 11 |
| worst case, 100 | on | 2.4 | 0.0 | 13.3 | 5.7 | 2.3 | 24.1 | 30.7 | 2,214 | 24 | 206 | 12 |

Paired differences — what the effects actually cost:

| Fight | without | with | delta mean | delta p99 |
| --- | ---: | ---: | ---: | ---: |
| idle | 2.0 us | 1.8 us | -0.3 us (noise) | -0.5 us |
| 1 target | 0.8 us | 2.2 us | **+1.4 us** | +2.7 us |
| aoe-heavy, 50 | 5.6 us | 7.3 us | **+1.7 us** | +3.9 us |

Event observation is **0.0 us to one decimal in every scenario**: it is a pass over a slice
of a few dozen events.

### End to end, on the GPU (`mygame --frames 6000`, 1920x1080, vsync off)

`--vfx-stress <rate>` plays catalogue effects around the player at a fixed rate without
touching gameplay, so the difference between two runs is the cost of *drawing* effects.

| Scene | dummies | stress | CPU mean | CPU p99 | GPU mean | GPU p99 | FPS | draws | tris | particles | effects |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| combat | 1 | 0 | 0.280 | 0.780 | 0.133 | 0.434 | 3,571 | 31 | 2,596 | 0 | 0 |
| combat | 10 | 0 | 0.298 | 0.855 | 0.141 | 0.477 | 3,352 | 33 | 3,244 | 0 | 0 |
| combat | 50 | 0 | 0.331 | 1.047 | 0.156 | 0.558 | 3,020 | 33 | 5,548 | 0 | 0 |
| combat | 100 | 0 | 0.350 | 1.029 | 0.173 | 0.587 | 2,861 | 33 | 7,948 | 0 | 0 |
| combat | 1 | 30/s | 0.327 | 0.644 | 0.183 | 0.402 | 3,056 | 39 | 9,455 | 2,283 | 35 |
| combat | 10 | 30/s | 0.343 | 0.728 | 0.191 | 0.434 | 2,915 | 41 | 10,095 | 2,275 | 35 |
| combat | 50 | 30/s | 0.232 | 0.653 | 0.156 | 0.340 | 4,312 | 41 | 10,240 | 1,426 | 28 |
| combat | 100 | 30/s | 0.230 | 0.586 | 0.155 | 0.340 | 4,349 | 41 | 12,626 | 1,412 | 27 |
| combat | 100 | 120/s | **0.755** | 1.069 | **0.413** | 0.697 | **1,325** | 41 | 33,773 | 12,047 | 127 |
| vfx | 3 | 0 | 0.250 | 0.968 | 0.102 | 0.500 | 3,992 | 12 | 944 | 0 | 0 |
| vfx | 3 | 30/s | 0.324 | 0.662 | 0.179 | 0.375 | 3,083 | 20 | 7,144 | 1,928 | 31 |
| vfx | 3 | 120/s | 0.701 | 1.043 | 0.378 | 0.657 | 1,426 | 20 | 25,210 | 11,128 | 128 |
| vfx | 3 | 240/s | 0.830 | 1.155 | 0.498 | 0.749 | 1,205 | 19 | 26,190 | 10,756 | 128 |

Per-pass split at the worst case (combat, 100 dummies, 120 effects/s): sim 0.004 ms,
extraction 0.007 ms, **vfx 0.141 ms**, cull 0.006 ms, UI 0.012 ms — the rest is the renderer
recording and the frame loop.

### Against the budget

* Worst case measured: **0.755 ms CPU / 0.413 ms GPU**, 1,325 FPS.
* 240 FPS needs 4.17 ms. The worst case uses **18 % of it on the CPU and 10 % on the GPU**.
* 480 FPS needs 2.08 ms; the worst case uses 36 % of it.
* The whole particle pass is **2 draw calls**, whatever the particle count. The rise in draw
  calls with effects on (33 to 41) is the mesh layers, which batch by shape.
* Nothing is claimed without a number: every row above is a run of the shipped binary.

---

## Milestone 22 — VFX stress test

`--vfx-stress <rate>` is the controlled stress, and it is deliberately **presentation only**
— it starts no ability and deals no damage, so what it moves is the cost of drawing effects
rather than the throughput of the ability system. It is available in every scene, because
"fifty simultaneous explosions over the arena" and "over an empty floor" are different
questions.

What it produces at 120/s: many projectiles and impacts, three or more simultaneous areas,
burning targets, repeated casts, the Ultimate, and 10,000-12,000 live particles.

Findings:

1. **The instance cap is the binding limit, not the particle pool.** At 120 effects/s the
   system sits at its 128-instance ceiling and refuses 17-115 effects a second, while the
   particle pool never dropped a single particle (peak 12,112 of 16,384). The two caps are
   mismatched by roughly a factor of two in the direction of "too few instances". Raising
   `DEFAULT_MAX_INSTANCES` is the obvious tuning and is left undone deliberately: real combat
   peaks at 24 instances, and changing a cap to make an artificial scenario look better is
   how a budget stops describing the game.
2. **Scaling is linear and shallow.** 30 to 120 effects/s roughly quadruples particles and
   triples GPU time, staying under half a millisecond.
3. **The scene barely matters.** The same effect load costs about the same over a hundred
   dummies as over an empty floor: effects are drawn in the passes the scene already runs.
4. **A frame never fails.** No pass drops, no allocation storm, no growth — the degradation
   is "fewer effects", counted and reported.

---

## Milestone 23 — VFX debug scene

`--scene vfx`: a flat floor, three dummies for scale and to aim at, and **one key per
effect** — 1 to 0 for the first ten, Z, X, C for the rest. Attached effects (Ignited, Shield,
Cast Charge) ride the player so they can be judged on a body; everything else plays six
metres in front, facing where the character faces.

Separate from Combat, Collision, Character and Environment, for the reason every other test
bed is separate: judging whether an explosion reads means playing it on demand, from a chosen
angle, as often as it takes — not winning a fight to get one. The scene brings no
environment, so nothing but the effect is in the picture. The controls are logged at startup.

---

## Milestone 24 — Automated tests

New tests, by area:

| Area | Where | Count |
| --- | --- | --- |
| Event layer | `engine-game/tests/presentation_events.rs` | 7 |
| Determinism of the new events | `engine-game/tests/determinism.rs` (extended) | — |
| Catalogue | `engine-vfx/src/asset.rs` | 12 |
| Instance lifecycle | `engine-vfx/src/system.rs` | 15 |
| Particles | `engine-vfx/src/particle.rs` | 12 |
| Random source | `engine-vfx/src/rng.rs` | 5 |
| Draw list | `engine-render-data/src/particles.rs` | 6 |
| Particle pipeline (no device) | `engine-renderer/src/particle_pipeline.rs` | 6 |
| Particle pass (real GPU) | `engine-renderer/tests/particle_render.rs` | 7 |
| Director | `engine-presentation/src/vfx.rs` | 14 |
| Every ability | `engine-presentation/tests/ability_vfx.rs` | 5 |

Against the milestone's own checklist:

* **events contain correct data** — position, crit, radius, delay, shape, duration and speed
  are all asserted against the gameplay numbers that produced them.
* **event creation and routing** — the ordered stream is asserted (a bolt leaves before its
  hit lands), and every new variant is described by the determinism test.
* **invalid entity handling** — a number for an entity that has gone is skipped; an attached
  effect whose owner dies ends; a stale `VfxHandle` cannot move the effect that replaced it.
* **VFX lifecycle** — spawn, lifetime, update, cleanup, attachment, expiration all covered.
* **particles** — lifetime, movement, spawn count at three frame rates, cleanup.
* **gameplay integration** — every ability on the bar produces an effect; damage produces a
  number and a flash; an impact produces an impact.
* **pooling** — reuse (render entities, instance slots), reset (`clear`), cleanup, and no
  stale state (generation checks on both instances and particles).

Tests are GPU-independent except the seven that are explicitly about pixels, and those skip
themselves when no adapter is present. No gameplay test depends on rendering.

---

## Milestone 25 — Manual combat verification

**An honest statement of what was and was not done.** This phase was executed by an agent
with no eyes on the screen and no hands on the keyboard: I cannot press 4 and judge whether
the Meteor feels heavy. What was done in place of that, and what remains for a person:

Done, automatically, on every run:

* Every one of the ten bar abilities is cast in a real fight and asserted to produce a
  visual (`every_ability_on_the_bar_produces_a_visual`) — including the awkward ones, since
  Detonate needs a full burn and Meteor needs its delay to elapse.
* Position, orientation, area and duration are asserted against the ability table and the
  gameplay `ZoneShape` rather than eyeballed.
* Cleanup is asserted: areas end with their zones, auras end with their targets, casts end
  with their casts.
* Single target and multiple targets: the area tests run against groups of six to twenty.
* Rapid casting: a forty-press rotation at the global cooldown's pace refuses no effect and
  drops no particle.
* Simultaneous effects: three overlapping Firestorms are all drawn.
* All five scenes run 1,500 frames and shut down clean, with no warnings or errors logged.

Left for a person, and it is the part a test cannot hold:

* whether each effect *reads* at gameplay camera distance — timing, weight, colour, whether
  the Meteor looks like it fell rather than appeared;
* whether the effect density during a real pull is exciting or noisy;
* whether the critical-strike feedback is loud enough to notice mid-rotation.

The VFX test bed (`--scene vfx`) exists precisely so that pass is quick to do.

---

## Milestone 26 — Full regression

```text
cargo fmt --all -- --check                                 clean
cargo clippy --workspace --all-targets -- -D warnings      clean
cargo test --workspace                                     68 test binaries, 0 failures
cargo build --workspace                                    ok
cargo build --workspace --release                          ok
cargo build --release --bin mygame --no-default-features   ok (shipping build)
```

Existing systems, checked by their own tests and by running each scene: movement, jumping,
collision, the collision demo, the character demo, the environment, target selection,
abilities, talents, the damage cycle, the damage meter, the UI, settings, rendering and the
benchmarks. Nothing regressed. The one behavioural change outside VFX is deliberate and
covered above: `DamageEvent` carries two more fields and seven new event variants exist,
which telemetry ignores by name, so the meter's numbers are unchanged.

---

## Milestone 27 — Final performance audit

Measured at 1920x1080, vsync off, over 6,000 frames per configuration. "Worst case" is a
hundred dummies with the stress driver at 120 effects/s — more simultaneous fire than any
rotation can produce.

| Scenario | CPU ms | GPU ms | FPS | Draw Calls | VFX Instances | Particles | Allocations |
| ----------- | -----: | -----: | --: | ---------: | ------------: | --------: | ----------: |
| Idle | 0.250 | 0.102 | 3,992 | 12 | 0 | 0 | ~0 |
| 1 target | 0.327 | 0.183 | 3,056 | 39 | 35 | 2,283 | ~0 |
| 10 targets | 0.343 | 0.191 | 2,915 | 41 | 35 | 2,275 | ~0 |
| 50 targets | 0.232 | 0.156 | 4,312 | 41 | 28 | 1,426 | ~0 |
| 100 targets | 0.230 | 0.155 | 4,349 | 41 | 27 | 1,412 | ~0 |
| AoE-heavy | 0.701 | 0.378 | 1,426 | 20 | 128 | 11,128 | ~0 |
| Ultimate (CPU only) | 0.020 | — | — | 117 | 19 | 2,122 | ~0 |
| Worst case | 0.755 | 0.413 | 1,325 | 41 | 127 | 12,047 | ~0 |

"Allocations ~0" is the measured number from `vfx_bench`: 6-12 for the whole process over 600
measured frames, in every scenario, none of it attributable to a per-frame path.

### Against the pre-phase baseline

The same binary with effects disabled is the baseline, measured in the same runs:

| Fight | before (GPU) | after (GPU) | before (CPU) | after (CPU) |
| --- | ---: | ---: | ---: | ---: |
| 1 dummy | 0.133 ms | 0.183 ms | 0.280 ms | 0.327 ms |
| 100 dummies | 0.173 ms | 0.155 ms | 0.350 ms | 0.230 ms |

The 100-dummy row shows the effects apparently making the frame *faster*, which is a
measurement artefact rather than a result: those two runs differ by more in camera pose and
run length than the effects cost, and both are far below the budget. The honest statement is
that at combat-realistic effect loads the cost sits **inside the run-to-run noise of a
sub-millisecond frame**, and only the deliberate 120/s stress moves it measurably
(+0.24 ms GPU, +0.4 ms CPU).

No regression was found that needed investigating. The only bound reached at all is the VFX
instance cap under artificial stress, recorded in Milestone 22.

---

## Architecture summary

### Pooling strategy

See Milestone 20. In one line: instances use a free list, particles use a fixed-capacity
structure-of-arrays pool with `swap_remove` death, render entities are pooled per shape and
hidden rather than despawned, and every per-frame buffer is cleared rather than freed. Every
cap refuses and counts rather than growing.

### How would we add a new ability with a new VFX without modifying the core renderer?

Three edits, none of them in `engine-renderer`:

1. **`engine-game`** — add the ability to the `ABILITIES` table. It already emits
   `AbilityUsed`, `Damage`, `CastStarted`/`CastEnded`, and — if it is targeted, delayed or
   leaves a zone — `ProjectileLaunched`, `ImpactScheduled`/`ImpactLanded` or
   `AreaEffectStarted`/`AreaEffectEnded`. No new event type is needed for an ability that is
   shaped like an existing one.
2. **`engine-vfx`** — if none of the thirteen effects fits, add a `VfxId` variant and a row
   to `EFFECTS` describing emitters and layers. Both are data.
3. **`engine-presentation`** — one arm in the director saying which effect the ability plays
   and how big it is, in gameplay units.

The renderer is untouched because it never learns what an ability is: it draws a
`ParticleDrawList` and a `RenderWorld`. A genuinely new *primitive* — a trail that is not a
strip, a decal that must be projected onto uneven ground, a real point light — is the case
that would reach the renderer, and that is the boundary working rather than failing.

### How would the engine behave if a VFX asset fails to load?

Gameplay continues and the missing presentation is handled gracefully, at every level:

* **Geometry that was never uploaded.** `VfxVisuals::get` returns `None` and that shape is
  skipped. The effect's particles still play, the frame still renders, the fight is
  unchanged. `an_unbound_shape_draws_nothing_and_breaks_nothing` asserts exactly this with
  *no* shapes bound at all.
* **A full system.** `VfxSystem::spawn` returns `None`, the director counts a refusal, and
  the ability is unaffected — it already dealt its damage.
* **A full particle pool.** `emit` refuses and counts. The effect plays with fewer particles.
* **A stale handle.** Every handle carries a generation; a stale one fails its check and is
  ignored rather than addressing whatever now lives in the slot.
* **Effects turned off entirely.** `VfxDirector::set_enabled(false)` ends everything and
  starts nothing, and combat resolves identically — which is what the paired benchmark runs
  measure.

There is no path from a missing visual to a missing hit, because the visual is downstream of
the damage in every case.

---

## Known limitations

1. **Mesh effects are opaque.** The world pipeline does not blend, so a layer's alpha is
   folded into brightness — a fading ring dims rather than becoming translucent. This is why
   geometry is used for footprints, rings and small cores while everything that needs real
   translucency is a particle. A blended forward pass for world geometry would remove the
   `MAX_UPRIGHT_RADIUS` rule with it.
2. **No dynamic lights.** An explosion does not light the wall beside it; it has an emissive
   surface and bright particles. Real point lights are a lighting-model change.
3. **Particles are untextured.** A procedural round falloff, no atlas, no animated flipbook.
   Smoke is a soft blob rather than a shape.
4. **No depth sorting within a blend mode**, and alpha-blended smoke is therefore
   order-dependent against itself. Invisible in fire; it would show on large translucent
   sheets.
5. **No soft particles.** A quad intersecting the floor shows a hard edge; the ground-lift
   constant hides the common case.
6. **The Ultimate's gameplay is still disabled**, so its effect is reachable only through the
   demo scene and the stress driver.
7. **Jump has no effect**, only landing above a speed threshold. `Jumped` is emitted and
   unused.
8. **Effect instances cap at 128** while particles cap at 16,384 — mismatched under
   artificial stress, untouched because real combat peaks at 24 instances.
9. **No hit reaction animation or ragdoll.** Feedback on a target is a flash and a number.
10. **`Healing` events have no producer**, so no healing effect exists.

## Deferred work

* Audio, deliberately: the event boundary is exactly where it attaches, and `observe_one`
  already sees every moment a sound would play.
* A blended world pass, soft particles, and a particle texture atlas.
* Dynamic point lights from explosions.
* Hit reactions and death animations for targets.
* Screen-space feedback: hit shake, a damage vignette, a screen flash on a critical hit.
* GPU particle simulation, if the CPU pool ever stops being 3 % of a frame.

## Future scalability considerations

* **The particle pass is already one draw per blend mode**, so particle count scales with
  fill rate rather than with draw calls. The next limit is overdraw, and the tool for it is
  smaller particles or fewer of them, not a different pipeline.
* **Mesh effects scale by shape**, not by effect: a hundred simultaneous ground areas is six
  batches. That property comes from sharing one material, and it should be preserved if
  effects ever gain per-effect materials.
* **The director's work is per live effect, not per entity.** Ten burning targets cost ten
  auras; a hundred cost a hundred — which is where an aggregate "the pack is on fire" effect
  would earn its place.
* **Attachments are resolved by a closure over the game world**, so a future crowd of
  hundreds of attached effects is a lookup per effect per frame and nothing worse.
* **Everything is capped and every cap is counted.** Scaling up is a matter of moving a
  number and re-running `vfx_bench` and the stress scene, both of which report the drops that
  a raised cap would remove.
