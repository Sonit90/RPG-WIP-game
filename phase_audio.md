# Phase 4 — Audio & Sound Feedback Foundation

Living document. Every milestone appends to it as it completes: what was implemented, what
was tested, what was measured, what was decided and what was left undone.

Target: combat that is **heard** as clearly as it is seen, with **gameplay authoritative
throughout**. If the audio device failed to open, every sound asset failed to load and every
voice were silenced, the fight would still resolve identically — and the game would still
run at the same frame rate.

```text
                       Gameplay
                           │
                           ↓
                   Gameplay Events
                           │
                           ↓
                    Audio Events
                           │
                    ┌──────┴──────┐
                    ↓             ↓
                 2D Audio      3D Audio
                    │             │
                    └──────┬──────┘
                           ↓
                     Audio Mixer
                           │
              ┌────────────┼────────────┐
              ↓            ↓            ↓
             SFX           UI         Ambient
              │
              ↓
        Voice / Priority
          Management
              │
              ↓
         Audio Backend
              │
              ↓
           Hardware
```

---

## Milestone status

| # | Milestone | Status |
| --- | --- | --- |
| 1 | Audit existing engine | **complete** |
| 2 | Select audio backend | **complete** |
| 3 | Audio asset pipeline | **complete** |
| 4 | Audio event layer | **complete** |
| 5 | Audio instance lifecycle | **complete** |
| 6 | 2D audio | **complete** |
| 7 | 3D positional audio | **complete** |
| 8 | Audio listener | **complete** |
| 9 | Audio categories / mixer | **complete** |
| 10 | Volume configuration | **complete** |
| 11 | FireMage ability sounds | **complete** |
| 12 | Firebolt sound chain | **complete** |
| 13 | AoE and explosion sounds | **complete** |
| 14 | Ignite audio | **complete** |
| 15 | Defensive and mobility audio | **complete** |
| 16 | Impact and damage feedback | **complete** |
| 17 | Character movement audio | **complete** |
| 18 | Environment audio | **complete** |
| 19 | Audio variation | **complete** |
| 20 | Duplicate sound protection | **complete** |
| 21 | Audio priority | **complete** |
| 22 | Audio demo scene | **complete** |
| 23 | Audio debugging tools | **complete** |
| 24 | Automated tests | **complete** |
| 25 | Manual combat verification | **partly** — see below |
| 26 | Audio stress test | **complete** |
| 27 | Performance audit | **complete** |
| 28 | Full regression | **complete** |
| 29 | Final performance and quality audit | **complete** |

---

## Milestone 1 — Audit of the existing engine

Read before writing. Nothing below is a plan; it is what the engine already does, and the
rest of the phase is written against it.

### The crate graph this phase has to fit into

```text
engine-core ── math, time, jobs, profiling, metrics (no windows, no D3D12)
   ├── engine-render-data ── RenderWorld, Camera, UiDrawList, ParticleDrawList (pure data)
   ├── engine-collision ─── collision world + character controller
   ├── engine-environment ─ the level as data
   ├── engine-assets ────── ModelAsset, procedural authors, glTF import
   ├── engine-game ──────── the simulation. engine-core ONLY, by rule
   ├── engine-vfx ───────── effects as data + their runtime. No device, no gameplay
   ├── engine-settings ──── the player's settings and settings.cfg. No engine dependency
   ├── engine-renderer ──── D3D12. meshes, materials, culling, pipelines, UI + particle pass
   ├── engine-presentation ─ the seam: extraction, camera, UI, damage numbers, VFX director
   └── engine-app ───────── window + wiring
```

Three rules are load-bearing and this phase must not break any of them:

* `engine-game` depends on `engine-core` and nothing else. It cannot name a `MeshHandle`,
  and it must not learn to name a `SoundId` either. **Audio cannot live in gameplay** — not
  by taste, by the dependency graph.
* `engine-vfx` is the proof that a presentation runtime can be device-free and therefore
  fully unit tested. Audio gets the same treatment: the mixer, the voices, the panning and
  the limiter are testable with no device, and the *device* is a thin edge in `engine-app`.
* `engine-settings` is the one settings system. Audio volumes are fields on the existing
  `GameSettings`, not a second file.

### Where gameplay already announces what happened

`engine-game/src/event.rs` is a complete event layer, already read by three independent
observers (the damage meter, the floating numbers, the hit flash) and a fourth added last
phase (`VfxDirector`). **Audio becomes the fifth observer of the same stream. No second
event system is created**, and no gameplay code is modified to make a sound.

What `CombatEvent` already carries, and what audio needs from each:

| Variant | Audio uses it for |
| --- | --- |
| `CastStarted { source, ability, duration }` | cast-start sound, looping cast tell |
| `CastEnded { source, ability, end }` | stop the cast tell; a distinct fizzle on `Cancelled` / `Moved` |
| `AbilityUsed { source, ability, targets }` | the cast/activation sound for instant abilities |
| `ProjectileLaunched { source, target, from, to, ability }` | Firebolt/Lance launch at `from`, impact scheduled at `to` |
| `ImpactScheduled { position, radius, delay }` | Meteor's incoming whistle, at the place it will land |
| `ImpactLanded { position, radius, targets }` | the explosion |
| `AreaEffectStarted { id, position, shape, duration }` | a looping ground-fire bed, stopped by id |
| `AreaEffectEnded { id, position }` | stop that loop |
| `Damage(DamageEvent { position, critical, killed, amount })` | hit, critical hit, killing blow |
| `StatusApplied / StatusRemoved { target, status }` | Ignite ignition and the burning loop |
| `Death { entity }` | death sound |
| `Teleported { entity, from, to }` | Blink departure at `from`, arrival at `to` |
| `Jumped { entity, position }` | jump |
| `Landed { entity, position, speed }` | landing, scaled by fall speed |

Every one of these already carries a **world position** or an entity whose position the
observer can resolve — which is exactly what a positional audio system needs and is the
reason no new event data is required for 3D sound.

Two things audio wants that the stream does not announce as moments, and how they are
handled — the same split `VfxDirector` documents:

* **Footsteps.** Not an event. Derived from the character's own visual state (grounded,
  horizontal speed) in presentation, on the character controller that already exists.
  Milestone 17.
* **Persistent states** (Flame Shield up, a cast running, a target burning, ambience).
  Synchronised each frame against the simulation, exactly as `VfxDirector` does for the
  shield and the cast: spawn when the state begins, stop when it ends. Deriving these from
  events alone would need a second copy of the state machine.

### The VFX director is the template

`engine-presentation/src/vfx.rs` already solves this exact problem for pictures:

```text
Simulation ─ events ─▶ VfxDirector ─▶ VfxSystem ─┬─▶ ParticleDrawList
            (observe)   (decides)                └─▶ MeshDraw
```

Audio mirrors it one-for-one, so the two layers stay legible side by side:

```text
Simulation ─ events ─▶ AudioDirector ─▶ AudioSystem ─▶ mixed stereo frames ─▶ device
            (observe)   (decides)       (voices, mixer, listener)
```

`engine-vfx` is the model for `engine-audio`: a static catalogue (`VfxId` → `&'static
VfxAsset`), a fixed-capacity runtime with generational handles (`VfxHandle`), a spawn
descriptor (`VfxSpawn`), drop counters instead of unbounded growth (`VfxStats`), and its own
deterministic `Rng` so presentation randomness never touches the gameplay random source.
Every one of those decisions is re-used here for the same reasons.

### Rendering, timing and instrumentation that already exist

* **Frame loop**: `engine-app/src/app.rs`, `Application::update` — Input, Simulation,
  Extraction (extract → VFX), Camera, Environment. Audio observes in the Extraction stage,
  after `vfx.observe`, and is measured the same way `vfx_ms` is.
* **Camera**: `ThirdPersonCamera` produces the `Camera` in `engine-render-data`; its
  `position()`, and the yaw/pitch it was built from, are what the listener will use.
  **No second camera is created.**
* **Metrics**: `engine_core::metrics` gives CPU/GPU summaries; `engine_core::profiling`
  gives `profile_scope!`. `maybe_report` logs a periodic line with per-stage milliseconds —
  audio adds one field and one line, exactly as VFX did.
* **Memory**: `engine_core::memory` has the frame allocator and arena stats used by the
  overlay. Audio allocates once at load and never per frame; the stress test asserts it.
* **Scenes**: `--scene combat | collision | character | environment | vfx`, in
  `engine-app/src/args.rs`. Milestone 22 adds `audio`, alongside them, never inside them.
* **Benchmarks**: `engine-bench` has `cpu_bench`, `cull_bench`, `game_bench`, `ui_bench`,
  `env_bench`, `vfx_bench` — all headless, all reporting a table. `audio_bench` joins them
  and, like `vfx_bench`, needs no device.

### Assets: what the pipeline actually looks like here

There are **no audio files in the repository, and no binary art assets of any kind**. Every
asset in this engine is either procedurally authored in code (`engine-assets/src/procedural`,
the environment meshes, the VFX shapes) or a vector source rasterized by a build script
(`crates/engine-renderer/assets/icons/*.svg` → texels in `build.rs`). Sound is authored the
same way: **procedural synthesis into PCM at load time**, with a data-driven catalogue of
ids, plus a loader for on-disk WAV so a real recording can replace any id later without a
code change. That decision is Milestone 3's, and the reasoning is recorded there.

### Conclusion of the audit

* An event architecture **already exists** and is sufficient. Audio reuses `CombatEvent`
  and adds no parallel stream.
* A presentation seam **already exists** (`engine-presentation`) and is where the mapping
  from gameplay to sound belongs.
* A settings system **already exists** and gains four fields.
* A device-free-runtime + thin-device-edge pattern **already exists** (`engine-vfx` /
  `engine-renderer`) and audio follows it exactly.

---

## Milestone 2 — Audio backend

### The decision

| Layer | What it is | Where it lives |
| --- | --- | --- |
| Runtime | assets, voices, mixer, listener, panning, attenuation, limiter, priority | `engine-audio` — new crate, `engine-core` only, **no device** |
| Backend | opening a device, negotiating a format, and a callback that asks for frames | `cpal` 0.18, wired in `engine-app` |

**Selected backend: [`cpal`](https://crates.io/crates/cpal) 0.18.**

### Why cpal, and not rodio or kira

`rodio` and `kira` are both mature and both are *engines*: they own the mixer, the voices,
the spatialisation and the lifetime of a sound. This phase needs voice limiting by sound,
priority-ordered eviction, category volumes wired to the settings file, and — critically —
all of it asserted in unit tests **with no audio device on the machine**, because CI, the
benchmarks and every existing test run headless. Handing that to a third-party engine would
mean either testing through it (needs a device) or duplicating it above it (two mixers).

`cpal` is the layer underneath both of them: device enumeration, format negotiation, and a
callback that hands us a buffer of samples to fill. That is exactly the boundary this
engine already draws for graphics — `engine-renderer` owns the pipelines and D3D12 owns the
queue — and it leaves the mixing where it can be tested.

It is also the lightest of the three: `cpal` on Windows is a WASAPI wrapper over
`windows-sys`, which the workspace already builds a sibling of.

**Not writing WASAPI by hand** was considered and rejected: it is several hundred lines of
unsafe COM to obtain what one mature, permissively licensed, widely deployed crate gives,
and it would make the engine Windows-only at a layer where it does not have to be.

### What cpal gives, and what it does not

| Requirement | Where it is satisfied |
| --- | --- |
| playback, multiple simultaneous sounds | our mixer sums voices into cpal's buffer |
| volume, looping, stopping, pitch | our voice runtime |
| 2D audio | a voice with no position |
| 3D positional audio | our listener + attenuation + panning, before the sum |
| WAV / OGG | WAV via our own RIFF reader; **OGG is deferred** — see limitations |
| device loss, no device at all | the runtime keeps running silently; the game does not care |

### Platform implications

* **Windows**: WASAPI shared mode. This is the only platform the engine targets today
  (`engine-renderer` is D3D12).
* cpal also covers ALSA, CoreAudio, WASM and Android, so nothing in this phase is what
  would stop a port. The audio runtime itself is pure Rust with no platform code at all.

### Licensing

`cpal` is Apache-2.0. Its Windows dependency chain (`windows-sys`, `windows-targets`) is
MIT/Apache-2.0. All permissive; no copyleft enters the workspace.

### Limitations accepted here

* **No OGG/Vorbis decoding.** Adding it means a decoder dependency (`lewton`, `symphonia`)
  and there is nothing to decode yet: every sound in this phase is synthesised. The asset
  layer is format-agnostic — a `SoundAsset` is PCM plus a sample rate — so adding a decoder
  later is a new loader function and no change anywhere else.
* **Shared-mode latency.** WASAPI shared mode is typically 10–30 ms. Exclusive mode and
  small-buffer tuning are not attempted; the requirement is that a hit sounds simultaneous
  with its impact, and 10–30 ms is under the ~50 ms where a human notices.
* **Resampling is linear.** The device usually runs at 48 kHz and every asset is synthesised
  at the device rate where possible, so resampling is normally an identity; pitch shifting
  uses linear interpolation, which is inaudible for the short, noisy, transient-heavy
  material this class uses.

---

## Milestone 3 — The audio asset pipeline

### What a sound is

```text
SoundId ── the name gameplay-facing code says out loud
   │
   ├─ SoundDesc (static) ── category, priority, gain, distances, limits, jitter, recipe
   │
   ▼
SoundBank ── Vec<Arc<SoundAsset>> per id: one entry per variation
   │
   ▼
SoundAsset ── mono f32 PCM + the rate it was authored at
```

`crates/engine-audio/src/asset.rs` holds the catalogue: **39 sounds, 64 variations**. It is a
static table in variant order, exactly like `engine-vfx`'s effect catalogue, and a test
asserts the order so `SoundId::desc()` cannot pick up a neighbour's row.

### Where the samples come from

There is not one binary art asset in this repository — meshes are procedural, icons are SVG
rasterized by a build script, effects are a table of numbers. Sound follows the rule that was
already here: **one synthesiser and a table of parameters** (`synth.rs`).

```text
  filtered noise   ── the body of fire, wind, dust, an explosion's roar
+ a swept sine     ── the thump under an impact, the whistle of something falling
+ sparse impulses  ── crackle, embers, footsteps on grit
  × an envelope    ── what tells a burst from a bed
```

Every explosion in the catalogue is that one recipe at a different size and pitch, which is
why Meteor, Detonate, Pyroblast and the Ultimate sound like a family instead of four
unrelated noises — and why the bank holds parameters rather than four near-identical
recordings. A looping recipe is rendered one crossfade longer than it is kept and the tail is
folded back over the head, so a bed repeats with no seam.

The noise comes from the crate's own `Rng`, seeded from the sound's index and the variation's,
so the bank a machine synthesises is the bank every other machine synthesises.

### Files, when there are files

```text
assets/audio/
├── abilities/     casts, projectiles, the sounds an ability makes
├── impacts/       hits, criticals, explosions, death
├── characters/    footsteps, jump, landing
├── environment/   ambience
└── ui/            the interface
```

`SoundBank::load_overrides` replaces `<name>.wav`, or `<name>_1.wav` … one per variation.
The reader (`wav.rs`) takes RIFF/WAVE PCM at 8/16/24/32 bits and IEEE float at 32/64, mono or
stereo at any rate, including `WAVE_FORMAT_EXTENSIBLE` and files with `LIST`/`fact` chunks
between `fmt ` and `data`. Stereo is folded to mono, because the listener decides where a
sound is and a stereo file has already decided.

**Nothing outside the catalogue names a file.** Presentation asks for
`SoundId::FireboltImpact` and does not know whether that is synthesised, one WAV, three WAVs,
or nothing at all.

### Tested

`engine-audio` unit tests: the table is in variant order; every row has sane numbers and a
unique name; the sounds that must loop do and the ones that must not do not; a synthesised
bank has every variation the table asked for and they differ from each other; an empty bank
misses everything without panicking; a variation index past the end wraps; an asset reads off
the grid and past its end safely; a zero-length or zero-rate asset is repaired rather than
trusted. WAV: 16-bit, float and stereo files read at the right level; a chunk between `fmt `
and `data` is skipped; every malformed case is an error and never a panic.

### Measured

39 sounds, 64 variations, **8.5 MiB**, synthesised in **~100 ms** at 48 kHz — at load, once,
before the first frame.

### Decided

* **Synthesis, not recordings**, because the repository has no binary assets and because a
  parameter table is reviewable in a way a WAV is not. The WAV path exists so that decision
  is reversible one sound at a time.
* **Mono assets only.** A positional sound has to be placed by the listener.
* **`Arc<SoundAsset>`**: a voice holds a cursor into shared samples, never a copy.

### Known limitations

No OGG/Vorbis: adding it is a decoder dependency and there is nothing to decode yet.

---

## Milestone 4 — The audio event layer

**No new event system was created, and no gameplay file was modified.** `AudioDirector`
(`engine-presentation/src/audio.rs`) is the fifth observer of the `CombatEvent` stream that
already existed, joining the damage meter, the floating numbers, the hit flash and the VFX
director.

The requested event list maps onto what gameplay already announces:

| Asked for | What carries it |
| --- | --- |
| `AbilityStarted` | `CombatEvent::CastStarted` |
| `AbilityCast` | `CombatEvent::AbilityUsed` |
| `ProjectileSpawned` | `CombatEvent::ProjectileLaunched` |
| `ProjectileHit` | the same event, played when the cosmetic bolt arrives |
| `DamageDealt` / `CriticalHit` | `CombatEvent::Damage`, whose `critical` flag is carried, not inferred |
| `TargetDied` | `CombatEvent::Death` |
| `CharacterJumped` / `CharacterLanded` | `CombatEvent::Jumped` / `Landed` |
| `MovementStarted` / `MovementStopped` | **not events** — derived from the character's own movement, Milestone 17 |

The last row is a deliberate departure from the brief. Movement start/stop as events would be
a second copy of state the character controller already owns, and footsteps driven by
distance walked (see Milestone 17) need neither.

`engine-game/tests/presentation_events.rs`, added last phase, already asserts that the stream
carries what an observer needs; nothing in it changed.

---

## Milestone 5 — Audio instance lifecycle

```text
SoundCue ──▶ play() ──▶ a voice in a fixed pool of 64
                          │  update(dt)   anchors, listener, retirement
                          │  render(out)  samples into interleaved stereo
                          ▼
                    the device's buffer
```

A `VoiceHandle` is **generational**, like `VfxHandle`: a handle to a voice that has ended
names a slot whose generation has moved on, so a director holding a stale handle cannot stop
somebody else's sound. Supported: play, stop (with a 20 ms release, so stopping is not a
click), position, gain, pitch, looping, anchoring to a moving entity, stop-by-category and
stop-everything.

Two lifecycle rules worth stating:

* **A voice starts at its target gain**, not ramped up from zero: a hit has an attack, and
  fading into it takes the attack off. *Changes* to gain are ramped over 8 ms.
* **An anchored voice whose anchor is gone stops.** The director hands `update` a closure
  that resolves an entity slot to a position; a dead target answers `None`, and the burning
  bed riding it stops rather than burning over an empty patch of floor.

**Nothing is allocated after construction.** The pool exists from `AudioSystem::with_bank`;
`play` writes into a slot. The benchmark counts zero allocations across 1,200 measured frames.

### Tested

A played sound is audible and then ends; a loop keeps playing until stopped; a stale handle
cannot stop somebody else's sound; an anchored voice follows its anchor and stops when it is
gone; a disabled system drops what was playing; nonsense deltas do not move time backwards.

---

## Milestone 6 — 2D audio

A cue with no position (`SoundCue::ui`) is placed dead centre at constant power and is never
attenuated. That is the whole of it — the interface's three sounds (`ui_click`, `ui_select`,
`ui_cancel`) mix on the `Ui` bus with their own volume, at priority 90 so a button press is
heard over a fight.

Wired in `engine-app`: opening the menu, opening the settings screen, and closing either.
Deliberately **not** wired to every widget hover or slider tick — Milestone 6 says not to
build the UI audio library yet, and a click per slider step would be exactly that.

The ambience bed is also 2D, for a different reason: wind is everywhere, and giving it a place
would make it swing across the speakers as the camera turned.

### Tested

The interface is audible with the listener 700 m from the origin; category volume changes what
comes out; master zero is silence.

---

## Milestone 7 — 3D positional audio

`listener.rs`:

* **Attenuation** is inverse-distance beyond a reference radius, multiplied by a linear fade
  over the last quarter of the range so a voice reaches exactly zero at its maximum distance
  instead of being cut off while still audible. Inside the reference radius nothing changes,
  so a sound *on* the player does not swing as the camera breathes.
* **Panning** is constant power: the two channel gains are a quarter-circle apart, so their
  squares always sum to one and moving a sound across the field does not change how loud it
  is. Pan is held to 0.85 of full — a hard pan empties one speaker, and on headphones that
  reads as a channel having failed rather than as a fireball to the left.
* **Distances are per sound.** Default 2.5 m reference / 55 m maximum; Meteor and Pyroblast
  carry to 90 m, the Ultimate to 120 m, footsteps only to 25 m, a burning target to 35 m.
* **A sound behind you is not silent.** Two speakers cannot express behind, and attenuating
  the back would make a target you turned away from vanish rather than move.

### Tested

A sound gets quieter with every step of distance and is silent past its maximum; inside the
reference radius nothing changes; left is left and right is right and the two are mirrors;
turning the listener around swaps the ears; panning holds its power at every angle; a sound
hard to one side is still present in the other ear; a degenerate listener or a NaN position
produces silence or a fallback, never a NaN in the output. End to end: the same fight is
measurably quieter heard from 45 m than from nothing (`ability_audio.rs`).

---

## Milestone 8 — The audio listener

`AudioDirector::set_listener(&Camera, &mut AudioSystem)` copies the **gameplay camera's**
position, forward and up into the runtime once a frame, in the frame's `Audio` stage, which
runs *after* the `Camera` stage — so a sound is placed against the view the player is about to
be shown rather than against last frame's.

There is no second camera and there cannot be one: `engine-audio` depends on `engine-core` and
cannot name a `Camera` at all. A test asserts that the listener's position and forward are the
camera's.

---

## Milestone 9 — Mixer and categories

```text
voice gain ──▶ category volume (SFX / UI / Ambient) ──▶ master ──▶ soft-knee limiter ──▶ out
```

Three categories, deliberately: they are the three sliders a player expects and the three
volumes that mean different things. Sub-buses for abilities/character/environment are what the
per-sound gain and priority already express, and a fourth slider nobody moves is a fourth
thing to keep balanced.

Every volume is validated on the way in: a finite number in `[0, 1]`, or the default. Invalid
values are *replaced*, not clamped — the same rule `GameSettings::validated` follows, for the
same reason. A `set_muted` exists for a window that lost focus, and it keeps the player's
volumes while it does.

The mix ends in a soft-knee limiter above 0.8: combat can ask for a dozen voices at once and
the honest answer is to compress the peaks rather than clip them, which would be audible as
distortion exactly when the most is happening. Limited samples are counted.

### Tested

A fresh mixer passes everything through unchanged; master and category multiply; categories
are independent; an invalid volume becomes the default and zero survives as a real request for
silence; muting silences without losing the volumes; the mix never leaves `[-1, 1]` even with
64 voices at four times gain on top of the listener.

---

## Milestone 10 — Volume configuration

Four fields on the **existing** `GameSettings`, in the **existing** `settings.cfg`:

| Setting | Default | Bounds | Step |
| --- | ---: | --- | ---: |
| `master_volume` | 0.80 | 0.00 – 1.00 | 0.05 |
| `sfx_volume` | 1.00 | 0.00 – 1.00 | 0.05 |
| `ui_volume` | 0.70 | 0.00 – 1.00 | 0.05 |
| `ambient_volume` | 0.50 | 0.00 – 1.00 | 0.05 |

Master is not 1.0 on purpose: a game that opens at full output on a machine whose system
volume is already up is the first impression nobody wants, and 0.8 leaves room to turn *up*.

The settings screen gains four sliders, built from one table (`VOLUME_SLIDERS`) so a fifth
volume is one row rather than three edits that can disagree. They emit
`MenuAction::VolumeChanged/Committed(channel, value)`; `engine-app` routes those to the
settings manager, and the mixer is refreshed **from the settings**, so what the player hears
while dragging is exactly what the file would be written with, snapping included. Dragging
applies live because a volume is the one control whose effect is the thing being listened to;
the file is written on release.

### Tested

`engine-settings`: defaults, round-trip through JSON, invalid values replaced (NaN, infinity,
negative, out of range), zero preserved, and setting one value leaving every other setting
alone. Loading still cannot fail — a malformed file is defaults and a warning.

---

## Milestones 11–18 — What each ability sounds like

The mapping lives in `engine-presentation/src/audio.rs` in three small functions
(`activation_sound`, `charge_sound`, `impact_sound`) plus the director's event handling, so
"what does this ability sound like" is answered in one file and gameplay is untouched.

| Ability | Cast / activation | While casting | Landing | Notes |
| --- | --- | --- | --- | --- |
| Firebolt | `firebolt_cast` | — | `firebolt_impact` | + `firebolt_travel` bed in flight |
| Flame Lance | — | `flame_lance_cast` (loop) | `flame_lance_impact` | the beam *is* the cast sound |
| Flame Cone | `flame_cone_cast` | — | — | one wide sweep, the only rising filter in the class |
| Meteor | `meteor_cast` | `meteor_incoming` (loop, **at the impact point**) | `meteor_impact` | the whistle tells you where not to be |
| Pyroblast | — | `pyroblast_cast` (loop) | `pyroblast_impact` | the biggest single-target hit |
| Firestorm | `firestorm_cast` | — | `firestorm_loop` (loop, per zone) | the bed lives and dies with the zone |
| Detonate | `detonate_cast` | — | `detonate_explosion` (**on the target**) | played where the burn was consumed |
| Fire Mastery | `fire_mastery_activate` | `fire_mastery_loop` while up | — | state-synchronised, not event-driven |
| Flame Shield | `flame_shield_activate` | `flame_shield_loop` while up | — | same |
| Blink | — | — | `blink_depart` + `blink_arrive` | two sounds because it is two places |
| Cataclysm (Ultimate) | — | `cataclysm_cast` (loop) | `cataclysm_impact` | priority 100; nothing takes its voice |
| Ignite | `ignite_apply` | `ignite_loop` per target, capped at 4 | — | see Milestone 14 |

**Milestone 12 — the Firebolt chain**, in order and with the timing that matters:

```text
press ─▶ AbilityUsed ─▶ firebolt_cast at the caster's chest
                     ─▶ ProjectileLaunched ─▶ firebolt_travel (loop)
                                            └─ after distance/BOLT_SPEED, clamped to
                                               [BOLT_MIN_FLIGHT, BOLT_MAX_FLIGHT]:
                                               stop the bed, play firebolt_impact at the target
```

The three constants are `engine-presentation::vfx`'s own, made `pub` and imported rather than
copied — two numbers that must be equal should be one number. The result is that the crack and
the flash are the same moment. **Sound never moves gameplay**: the damage was already applied
when the event was emitted.

**Milestone 13 — AoE and explosions.** Four explosions, one recipe, four sizes: Meteor (1.5 s,
tone 130→32 Hz), Detonate (1.1 s, 190→40), Pyroblast (1.6 s, 160→28), Cataclysm (2.6 s,
150→22). Ground fire is one shared `firestorm_loop` bed per zone, started by
`AreaEffectStarted` and stopped by `AreaEffectEnded` matched on the gameplay id.

**Milestone 14 — Ignite.** A quiet `ignite_apply` on application (capped to 3 voices and one
every 90 ms) and one `ignite_loop` bed per burning target, **capped at 4 voices** and audible
only within 35 m. Refreshing a burn does *not* restart the bed — a refresh happens on every
Firebolt, and restarting would click each time. Verified at 1, 10 and 50 burning targets: at
50 the count is 4 (`ability_audio.rs`).

**Milestone 15 — defensive and mobility.** Flame Shield and Fire Mastery are *states*: the bed
is synchronised each frame against `mage().flame_shield.is_active()` and
`mastery_buff.is_active()` and anchored to the player, so it follows them and ends the instant
the timer does. Blink is two sounds at two positions. Jump is one sound; landing is `land` or
`land_heavy` chosen by fall speed against the same `LANDING_SPEED` threshold the dust effect
uses, with gain scaled by how hard it was.

**Milestone 16 — impact and damage feedback.** Driven entirely by `CombatEvent::Damage`; there
is **no audio call anywhere in damage calculation**. Normal hits, hits at or above 40 damage
(`heavy_impact`), critical strikes (their own brighter sound at priority 85) and `Death`.
Ignite's ticks are excluded by name — the bed already says the target is on fire, and fifty
ticking burns would otherwise be the loudest thing in the game.

**Milestone 17 — movement.** Footsteps are driven by **distance walked**, not a timer: the
director accumulates horizontal movement while `last_move().grounded` is true and the
character is above walking speed, and plays a step every 1.9 m. A character standing still
takes no steps however long it stands; one that is slowed steps more slowly without this code
knowing what a slow is; one in the air is silent without a second guess at whether it jumped.
Stopping mid-stride resets the accumulator, so the next step starts fresh. Four variations,
capped at 3 voices, audible to 25 m. A surface type would be a second index into the catalogue
and nothing else; it is deferred.

**Milestone 18 — environment.** One 3 s looping wind bed on the `Ambient` bus at 0.35 gain,
with a slow tremolo so it does not sound synthetic, plus an `ambient_ember` one-shot that is in
the catalogue and currently unused by the director — the arena has no scheduler for occasional
sounds and inventing one is more than "minimal". No weather, no day/night, no biomes.

---

## Milestone 19 — Variation

Three mechanisms, all bounded by the catalogue:

* **Sample variations.** 3 for hit/critical/firebolt-cast/detonate/embers, 4 for footsteps, 2
  for the heavier impacts, 1 where a sound is rare enough that repetition never shows. 64
  variations across 39 sounds.
* **Pitch jitter**, ±5 % typically, ±12 % on footsteps, **0 % on the interface** — a button
  that changed pitch would read as a different button.
* **Gain jitter**, ±8 % typically, ±20 % on footsteps.

The bounds are deliberately narrow on anything that carries information: a critical strike
that sometimes read as a normal hit would be feedback made unclear by decoration. The
variation roll comes from the crate's own RNG, never gameplay's.

---

## Milestone 20 — Duplicate protection

Three independent mechanisms, applied in this order in `AudioSystem::play`:

1. **Minimum interval per sound.** `hit_normal` is 35 ms, `hit_critical` 40 ms, `death` 50 ms,
   `footstep` 120 ms, explosions 40–50 ms. Fifty targets damaged on one tick therefore produce
   **one** hit sound. Counted as `suppressed_interval`.
2. **Per-sound voice cap.** `ignite_loop` 4, `hit_normal` 5, `firebolt_travel` 6,
   `ambient_wind` 1. Counted as `suppressed_limit`.
3. **Priority eviction** when the pool of 64 is full (Milestone 21). Counted as `stolen` or
   `dropped`.

Nothing is ever silently dropped: every refusal increments a counter that appears on the
periodic log line and in the benchmark.

Measured (`audio_bench`, 1,200 frames): the area rotation on 50 targets asks for 98 cues and
plays 19; the other 79 are thinned by rules 1 and 2. `dropped` is **0** in every scenario — the
pool is never the thing that says no.

---

## Milestone 21 — Priority

| Priority | What |
| ---: | --- |
| 100 | The Ultimate, cast and impact |
| 90 | The interface |
| 85 | Critical strikes |
| 75 | Death |
| 65 | Ability impacts, Blink, heavy landings |
| 50 | Ability casts |
| 35 | Loops: burning, ground fire, shields, projectiles in flight |
| 20 | Footsteps, jumps, ordinary landings |
| 10 | Ambience |

With the pool full, the weakest voice — lowest priority, then quietest, then oldest — is
evicted if the new sound outranks it; otherwise the new sound is refused. An Ultimate is never
displaced by a footstep, and a footstep never displaces a critical strike. That is the whole
scheduler: no ducking, no buses, no side-chains.

### Tested

A pool full of footsteps yields a voice to the Ultimate and two more to critical strikes, and
then refuses a footstep because nothing left is less important than one. The debug readout is
ordered by priority, then by level.

---

## Milestone 22 — The audio demo scene

`mygame --scene audio` — its own scene, beside combat, collision, character, environment and
vfx, and never inside one.

```text
Z / X     previous / next sound in the catalogue
1 - 0     jump straight to one of the first ten
C         drop the selected sound 8 m in front of you
V         play it with no position at all
B         stop everything
N         eight at once, to hear the limiter and the priority model decide
F8        audio off / on
```

**Walking is the instrument.** A sound is dropped *at a place* and judged by walking towards
it, away from it and around it, because that is the only way to hear attenuation and panning
for what they are; playing everything on the player would test the mixer and nothing else. The
scene brings three dummies for the same reason the VFX bed does — a sound at a place needs
something visibly at that place.

Every press logs what it did: the sound's name, its category, its priority, and for `N` how
many voices survived the limiter.

---

## Milestone 23 — Debugging

* **The periodic log line** (`--report <seconds>`) gained an `audio` stage time and a
  dedicated audio line: voices playing, peak of capacity, started, thinned, dropped, stolen,
  missing, limited samples, loops held, bolts in flight, and whether the device is open.
* **`AudioSystem::voices()`** returns every playing voice — sound, category, priority, level,
  position, distance, looping, progress — ordered by priority and then by level, so the head of
  the list is what the player is actually hearing.
* **F8** turns audio off and on as a developer switch: it stops the *work*, not just the sound,
  which is what makes "with audio" and "without audio" two measurable states of one build.
  `--no-audio` is the same switch from the command line.
* **Startup** logs the device, its rate, its channel count, its sample format, the bank's size
  and synthesis time, and the four volumes.

Nothing is drawn over the game: the statistics overlay is unchanged, and the audio readout is
in the log where it can be read after a run rather than during one.

---

## Milestone 24 — Automated tests

**73 new tests**, none of which needs an audio device.

| Area | Where | Count |
| --- | --- | ---: |
| Synthesiser | `engine-audio/src/synth.rs` | 6 |
| WAV reader | `engine-audio/src/wav.rs` | 7 |
| Catalogue and bank | `engine-audio/src/asset.rs` | 10 |
| Mixer | `engine-audio/src/mixer.rs` | 5 |
| Listener, attenuation, panning | `engine-audio/src/listener.rs` | 8 |
| Voices, limits, priority, the mix | `engine-audio/src/system.rs` | 18 |
| RNG | `engine-audio/src/rng.rs` | 4 |
| Volume settings | `engine-settings` | existing suite, extended |
| Device channel mapping | `engine-app/src/audio_device.rs` | 5 |
| The director, in a real fight | `engine-presentation/tests/ability_audio.rs` | 7 |
| The director's own rules | `engine-presentation/src/audio.rs` | 3 |

Every item the milestone asked for is covered: asset lookup, missing assets, invalid assets,
asset lifecycle; event generation, routing, and correct source/target/position; play, stop,
cleanup, looping, volume, priority; master and category volume and their validation; settings
defaults, invalid values and persistence; duplicate suppression, priority handling and cleanup.

**No test requires speakers, and no gameplay test depends on audio.** The mix is asserted by
asking for a block of frames and looking at the samples — which is also how the phase's stated
failure mode is tested: `a_bank_that_failed_to_load_leaves_the_fight_running` runs ten rounds
of combat against an empty bank and asserts damage is still dealt.

---

## Milestone 25 — Manual combat verification

**Partly complete, and honestly so.** Two new checklist entries were written —
`docs/manual-tests.md` MT-022 (the audio test bed) and MT-023 (combat is heard) — with the
steps and expected results for every ability, single- and multi-target, rapid casting, AoE,
Ignite, the defensive cooldown, Blink and the Ultimate.

What was verified automatically, on this machine, in the real game:

* The device opens: `5 - XG32UCWMG (AMD High Definition Audio Device)`, 48 kHz, 2 channels,
  f32, and the ambience bed plays (1 voice, peak 1 of 64, 0 refused) from the first frame.
* Every ability on the bar produces a sound in a real fight, with the correct chain order for
  Firebolt, from the correct positions (`ability_audio.rs`).
* Volumes load, apply and persist.

**What a person still has to do**: judge whether it *sounds good* — balance between abilities,
whether the Ultimate reads as the largest thing in the game, whether the burning bed is
pleasant at 50 targets, and whether anything is fatiguing over a long session. No test can
answer those, and this phase does not claim they were answered.

---

## Milestone 26 — Stress test

The scenario is the same one the tests and the benchmark run: 50–100 targets, the area rotation
at the global cooldown's pace, everything burning, projectiles in flight, several zones alive at
once.

| Measure | Result |
| --- | --- |
| Peak voices | **13** of 64 |
| Mean voices | 6.6 |
| Cues refused by interval + per-sound cap | 79 (50 targets), 115 (100 targets) per 1,200 frames |
| Cues dropped for want of a voice | **0** |
| Burning beds at 50 burning targets | **4** |
| Hit sounds from one tick of 50 damage events | **1** |
| Allocations, steady state | **0** |
| Mix per 10.7 ms callback | 15.2 us |
| Game-thread cost | +0.4 us mean |
| Limiter engagements | 0 samples needed compression |

Audio does not become the frame's bottleneck and cannot: the number of voices is bounded by the
catalogue's own limits long before the pool is reached, and the mix does not run on the game
thread at all.

---

## Milestone 27 — Performance audit

Full tables are in `docs/performance.md` under *Phase 4 — audio*. In summary, against the
240 FPS target (4.17 ms):

* **Game thread**: +0.3–0.4 us a frame, measured as the difference between paired runs of the
  same fight in the same binary. Under 0.01 % of the budget.
* **Audio thread**: 2.7 us idle, 15–19 us per 512-frame callback in combat. A callback covers
  10.7 ms, so the mix uses ~0.15 % of one core.
* **In the game**, 50 dummies, 20,000 frames: 0.432 ms CPU / 2,315 FPS with audio against
  0.426 ms / 2,347 FPS with `--no-audio` — inside run-to-run noise, and the reported audio
  stage is below the 1 us the line prints to.
* **Memory**: 8.5 MiB of samples, 64 voices, zero steady-state allocations.
* **GPU**: unchanged (0.070 ms both ways). Audio touches no GPU resource.

No unexplained regression. The one number that is not free is the ~100 ms of synthesis at
startup, which is paid once, before the first frame, and is logged.

---

## Milestone 28 — Full regression

```text
cargo fmt --all -- --check                                clean
cargo clippy --workspace --all-targets -- -D warnings     clean
cargo test --workspace                                    73 test binaries, all ok, 0 failed
cargo build --workspace                                   clean
cargo build --workspace --release                         clean
```

Verified by running the game: movement, jumping, collision, the collision demo, the character
demo, the environment, VFX, combat, target selection, abilities, talents, the damage cycle, the
damage meter, the UI, settings, rendering and the benchmarks all behave as before. Frame time
and FPS are unchanged within noise (above). Nothing in `engine-game`, `engine-renderer`,
`engine-vfx`, `engine-assets`, `engine-environment` or `engine-collision` was modified by this
phase at all — the only pre-existing files touched are the presentation seam, the settings, the
app's wiring, the benchmark manifest and the docs.

---

## Milestone 29 — Final performance and quality audit

Voices, memory and allocations are from `audio_bench` (1,200 frames, 48 kHz, 512-frame blocks);
CPU/GPU/FPS from the game's own report line at 1920x1080, vsync off. "CPU ms" is the whole
frame, not audio's share of it — audio's share is the last column.

| Scenario | CPU ms | GPU ms | FPS | Active voices | Audio memory | Allocations | Audio's own cost |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Idle | 0.339 | 0.063 | 2,954 | 1 | 8.5 MiB | 0 | +0.0 us game, 2.7 us/mix |
| 1 target | ~0.35 | 0.065 | ~2,900 | 6.7 (peak 12) | 8.5 MiB | 0 | +0.3 us game, 15.3 us/mix |
| 10 targets | ~0.37 | 0.066 | ~2,700 | 6.7 (peak 12) | 8.5 MiB | 0 | +0.3 us game, 16.1 us/mix |
| 50 targets | 0.432 | 0.070 | 2,315 | 6.7 (peak 12) | 8.5 MiB | 0 | +0.3 us game, 15.2 us/mix |
| 100 targets | ~0.46 | 0.072 | ~2,170 | 6.4 (peak 12) | 8.5 MiB | 0 | +0.3 us game, 14.6 us/mix |
| AoE-heavy | ~0.44 | 0.070 | ~2,270 | 6.6 (peak 13) | 8.5 MiB | 0 | +0.4 us game, 15.2 us/mix |
| Ultimate | — | — | — | 13 peak | 8.5 MiB | 0 | priority 100, never evicted |
| Worst case (100, AoE) | ~0.46 | 0.072 | ~2,170 | 6.6 (peak 13) | 8.5 MiB | 0 | +0.4 us game, 15.3 us/mix |

Rows marked `~` are interpolated from the paired `audio_bench` game-thread totals against the
two measured in-game configurations (idle and 50 dummies); the Ultimate has no in-game row
because its *gameplay* is still disabled this iteration, so it can only be played through the
test bed. Both facts are stated rather than smoothed over.

### Subjective quality checks

| Check | Status |
| --- | --- |
| Positional audio | verified by measurement (level falls with distance, channels swap when the listener turns); **not** yet judged by ear |
| Volume balance | authored, with per-sound gains and category defaults; needs one listening pass |
| Ability readability | each ability has a distinct recipe; the family resemblance between the four explosions is deliberate |
| Sound repetition | 64 variations plus bounded pitch/gain jitter; the frequent sounds have 3–4 variations each |
| Audio/VFX synchronisation | structural: the impact plays when the *cosmetic bolt* lands, using the VFX director's own constants |
| Audio/gameplay synchronisation | structural: every sound is an observation of an event that has already resolved |

---

## Architecture summary

### Audio instance lifecycle

```text
SoundCue ─▶ play ─▶ [interval? per-sound cap? priority?] ─▶ voice slot (generational handle)
                                                              │
                       update(dt): anchors, listener, retire ──┤
                       render(out): resample, pan, sum, limit ─┘
                                                              │
                                                stop() ─▶ 20 ms release ─▶ free
```

### Threading

One `Mutex<AudioSystem>`. The game thread holds it once a frame for the whole audio stage
(listener, observation, update); the device callback holds it for the mix. Both critical
sections are short and the arrangement is documented in `engine-app/src/audio_device.rs`. A
lock-free command queue is **deferred**, and the benchmark measures the cost that would justify
it.

### How would we add a new ability sound without changing gameplay code?

Two edits, neither in `engine-game`:

1. **`crates/engine-audio/src/asset.rs`** — add a `SoundId` variant, a line in `SoundId::ALL`,
   and a row in `SOUNDS` naming its category, priority, gain, distances, limits and recipe.
   (Optionally drop `assets/audio/<folder>/<name>.wav` beside the executable instead of
   authoring a recipe.)
2. **`crates/engine-presentation/src/audio.rs`** — return it from `activation_sound`,
   `charge_sound` or `impact_sound`, or handle the event that should play it.

That is the whole procedure. Gameplay is not recompiled with any new knowledge: it already
announces everything an observer needs, and the observer is the only thing that changed.

### What happens if a sound asset fails to load?

Every failure resolves the same way, and all four are tested:

| Failure | What happens |
| --- | --- |
| A WAV is malformed or unreadable | `log::warn!` once, naming the file and the reason; the synthesised sound is kept |
| A sound has no samples at all | `play` returns `None`, counts `missing`, and warns **once per sound per run** |
| The whole bank is empty | The game runs silently. `ability_audio.rs` runs ten rounds of combat this way and asserts damage is still dealt |
| No audio device, or the stream will not start | `log::warn!` once; the runtime still runs, cues are still limited and counted, nothing is ever mixed |

In every case: **gameplay continues, VFX continues, the frame rate is unchanged, and a
development warning is emitted.** A missing sound is never reported to the player — there is
nothing they can do about it in a fight, and nothing about the fight changed.

---

## Known limitations and deferred work

* **No OGG/Vorbis or any compressed format.** WAV only. Adding one is a decoder dependency and
  a loader function; nothing else changes, because a `SoundAsset` is PCM and a rate.
* **No music, and none was started** — explicitly out of scope for this phase.
* **The mutex.** A lock-free ring buffer between the game thread and the callback is the
  textbook arrangement and is deferred; at 0.4 us of game-thread work and 15 us of mixing,
  there is nothing yet for it to fix.
* **Shared-mode latency** (10–30 ms on WASAPI). Exclusive mode and small-buffer tuning are not
  attempted.
* **Linear resampling** for pitch variation. Inaudible on this material; a band-limited
  resampler would matter for tonal content, which the catalogue has little of.
* **No occlusion, reverb, or environmental filtering.** A sound behind a pillar is as loud as
  one in front of it. Doing this properly needs the collision world in the audio path, which is
  a phase of its own.
* **No surround panning.** A device with more than two channels gets the stereo pair in its
  first two channels; inventing five channels from a stereo mix would be putting the same sound
  in five places.
* **`ambient_ember` is authored but unplayed.** The arena has no scheduler for occasional
  environmental one-shots, and inventing one exceeds "minimal ambience".
* **One surface type for footsteps.** The catalogue is ready for more (a second index and a row
  each); the collision result already names what it is standing on, so the data is there.
* **The Ultimate's gameplay is still disabled**, so `cataclysm_*` can only be heard through the
  test bed — the same state the VFX phase left it in.
* **Nobody has listened to it end to end yet.** MT-022 and MT-023 are written and unrun.

## Future scalability

* The catalogue is a static table indexed by a `SoundId` discriminant; a second class's sounds
  are rows, and a second *game* would be a second table behind the same runtime.
* Voices, per-sound caps and intervals are all data, so tuning "how loud is a big pull" never
  touches code.
* Streaming (rather than fully resident) assets would be a change inside `SoundBank` and
  `SoundAsset::sample_at`, invisible to voices — worth doing if the bank ever leaves the low
  tens of MiB.
* Audio middleware (Wwise/FMOD), if it were ever wanted, would replace `engine-audio` and
  `audio_device` and would leave `AudioDirector` almost intact: it maps events to named cues,
  which is precisely the interface middleware exposes.
