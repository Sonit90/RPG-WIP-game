# Enemy ability audit

All 58 `EnemyAbility` rows, checked against the executor (`sim/enemy_exec.rs`), the brain
(`enemy/brain.rs`) and the player's defences. The checks are backed by
`crates/engine-game/tests/enemy_audit.rs`; run it with `cargo test -p engine-game --test enemy_audit`.

Legend: OK = verified (by test unless stated), FIXED = was wrong, now fixed, NOTE = works as
designed but worth knowing.

## Checks (apply to every ability unless a row says otherwise)

| # | Check | Result | Backed by |
|---|---|---|---|
| T | Targeting: picks who its `select` says, only living hostiles, never someone who left | OK; Pounce and Savage Pounce FIXED; marks on a dead victim FIXED | `a_pounce_leaps_...`, `a_mark_...`, `a_charge_runs_...`, `nothing_is_aimed_at_the_dead_...`, `a_mark_whose_victim_dies_...` |
| L | Line of sight: single-target hits never land through a wall; random picks prefer visible players | OK; random picks now require sight (FIXED) | `single_target_hits_never_land_through_a_wall` |
| C | Cast time / cooldown from cast start; never used again before it is up (frenzy divides) | OK | `no_ability_is_used_again_before_its_cooldown_is_up` |
| I | Interrupt honours `interruptible`; interrupted cast deals nothing, leaves no pool/mark/heal, takes its telegraph down unresolved, stays on cooldown | OK | `an_interrupt_stops_what_can_be_stopped_...`, `an_interrupted_cast_never_resolves_...`, `a_channel_that_is_interrupted_...` |
| B | Shield Bash interrupts only interruptible casts | OK | `shield_bash_interrupts_...` |
| D | Damage scale (level x party x key) applied exactly once, on every path | OK | `damage_scales_once_on_every_path` (scale x2 gives x2 for every ability) |
| S | Telegraph shape is the hit test: who is hit equals who is inside the announced shape | OK | `what_a_telegraph_announced_is_exactly_who_gets_hit` |
| P | Player buffs: Barkskin, Rallying Ward, Battle Fervor multiply every path's damage; Warrior absorb is consumed | OK | `damage_taken_effects_scale_...`, `the_warriors_shield_soaks_...` (trinket absorb: `trinkets.rs`, same `defend_player`) |
| X | Debuffs classified for dispel | OK (see notes) | `what_enemies_put_on_a_player_...`, `natures_cleanse_takes_...` |
| A | Authority: nothing runs in a remote-authority sim; every hit goes through `deal_damage` | OK | `a_client_under_remote_authority_...` |
| V | Cue/VFX/audio mapping | OK (see notes) | table + presentation read |

## Per ability

Columns: T targeting, L sight, C cooldown/cast, I interrupt, D damage scale, S telegraph, P buffs.
"-" means the check does not apply.

| Ability | T | L | C | I | D | S | P | Note |
|---|---|---|---|---|---|---|---|---|
| Swing | OK | OK | OK | OK | OK | OK | OK | - |
| Cleave | OK | OK | OK | OK | OK | OK | OK | - |
| GroundBlast | OK | OK | OK | OK | OK | OK | OK | - |
| Bolt | OK | OK | OK | OK | OK | OK | OK | - |
| RitualNova | OK | OK | OK | OK | OK | OK | OK | - |
| Mend | OK | OK | OK | OK | OK | OK | OK | Heals the most wounded visible ally; heal goes through world.heal, so only a Plagued recipient is reduced (only players get Plagued). No heal sound or VFX beyond the cast bed |
| CinderPool | OK | OK | OK | OK | OK | OK | OK | Hazard ticks now flagged periodic (FIXED) so combat text draws them as ticks |
| Sunder | OK | OK | OK | OK | OK | OK | OK | - |
| AshBurst | OK | OK | OK | OK | OK | OK | OK | Uninterruptible (Shield Bash leaves it casting) |
| WardensSweep | OK | OK | OK | OK | OK | OK | OK | Uninterruptible. A stun (Shield Charge) still cancels it |
| EmberLance | OK | OK | OK | OK | OK | OK | OK | - |
| Ashfall | OK | OK | OK | OK | OK | OK | OK | - |
| CinderRain | OK | OK | OK | OK | OK | OK | OK | as Cinder Pool |
| ImmolationNova | OK | OK | OK | OK | OK | OK | OK | Uninterruptible; a stun still cancels it |
| BoneBolt | OK | OK | OK | OK | OK | OK | OK | - |
| BoneBarrage | OK | OK | OK | OK | OK | OK | OK | Delayed zones: telegraph drawn from cast end; interrupt only during the cast |
| GraveExplosion | FIXED | OK | OK | OK | OK | OK | OK | FIXED: a mark is not laid if its victim died during the cast. NOTE: a laid mark that outlives its victim lands where they fell |
| BoneSpear | OK | OK | OK | OK | OK | OK | OK | Line from caster through target, 34 m |
| ShadowBolt | OK | OK | OK | OK | OK | OK | OK | - |
| CorruptedGround | OK | OK | OK | OK | OK | OK | OK | as Cinder Pool |
| DarkPulse | OK | OK | OK | OK | OK | OK | OK | Channel: first tick is paid on the tick after the cast starts (ticks are at the start of each interval). Excluded from the telegraph/hit comparison |
| GraveDrain | OK | OK | OK | OK | OK | OK | OK | Channel: ticks need sight and range+6 m. First tick one tick after cast start |
| BoneArrow | OK | OK | OK | OK | OK | OK | OK | - |
| MultiShot | OK | OK | OK | OK | OK | OK | OK | - |
| CripplingArrow | OK | FIXED | OK | OK | OK | OK | OK | Random pick now requires sight (FIXED). Slow is a Curse: Nature's Cleanse and Mass Dispel take it |
| RainOfArrows | OK | OK | OK | OK | OK | OK | OK | as Bone Barrage |
| BoneSlash | OK | OK | OK | OK | OK | OK | OK | - |
| BoneCleave | OK | OK | OK | OK | OK | OK | OK | - |
| BoneCharge | OK | OK | OK | OK | OK | OK | OK | Instant, no telegraph by design; slow is a Curse |
| ExecutionStrike | OK | OK | OK | OK | OK | OK | OK | Interruptible: Shield Bash stops it |
| CrushingBlow | OK | OK | OK | OK | OK | OK | OK | - |
| GroundSlam | OK | OK | OK | OK | OK | OK | OK | - |
| GraveCharge | OK | OK | OK | OK | OK | OK | OK | as Bone Charge |
| BoneQuake | OK | OK | OK | OK | OK | OK | OK | Delayed ring anchored where the caster stood |
| GhoulStrike | OK | OK | OK | OK | OK | OK | OK | - |
| RendingClaws | OK | OK | OK | OK | OK | OK | OK | Bleed (Rended) scaled once through the hit's scale; not dispellable by design |
| Pounce | FIXED | OK | OK | OK | OK | OK | OK | FIXED: leaped only at the victim, so it fired once on the pull and never again. Now a seeded random hostile in [min range, range], in sight, preferring non-tank |
| BloodFrenzy | OK | OK | OK | OK | OK | OK | OK | Self buff is brain state, not a status: Mass Dispel cannot purge it. No use-time cue (cast animation only) |
| MassiveBoneBarrage | OK | OK | OK | OK | OK | OK | OK | as Bone Barrage |
| GraveChainExplosion | FIXED | OK | OK | OK | OK | OK | OK | FIXED: as Grave Explosion. Chained marks are laid on whoever the first blast caught, tank included |
| SpreadingCorruption | OK | OK | OK | OK | OK | OK | OK | as Cinder Pool; telegraph shows the landing radius, pool grows to 8 m (announced by area effect) |
| MassGraveDrain | OK | OK | OK | OK | OK | OK | OK | as Grave Drain |
| DeadlyRain | OK | OK | OK | OK | OK | OK | OK | as Bone Barrage |
| BoneVolley | OK | OK | OK | OK | OK | OK | OK | Lines hit; the pool (3 m) is announced as an area effect, not by the line telegraph |
| Massacre | OK | OK | OK | OK | OK | OK | OK | - |
| ShatteringCharge | OK | OK | OK | OK | OK | OK | OK | as Bone Charge; shockwave is a delayed strike with its own telegraph |
| Gravequake | OK | OK | OK | OK | OK | OK | OK | as Bone Quake, six zones |
| DevastatingCharge | OK | OK | OK | OK | OK | OK | OK | as Bone Charge; trail damage scaled once |
| RendingFrenzy | OK | OK | OK | OK | OK | OK | OK | as Rending Claws |
| FeastOnTheWounded | OK | OK | OK | OK | OK | OK | OK | - |
| SavagePounce | FIXED | OK | OK | OK | OK | OK | OK | FIXED: same as Pounce |
| RapierThrust | OK | OK | OK | OK | OK | OK | OK | - |
| GougingRiposte | OK | OK | OK | OK | OK | OK | OK | Slow is a Curse |
| Garrote | OK | FIXED | OK | OK | OK | OK | OK | Random pick now requires sight (FIXED). Bleed is not dispellable by design |
| DanceOfBlades | OK | OK | OK | OK | OK | OK | OK | - |
| GhastlyWaltz | OK | OK | OK | OK | OK | OK | OK | as Bone Charge; trail damage scaled once |
| MarkOfTheHost | FIXED | OK | OK | OK | OK | OK | OK | FIXED: as Grave Explosion |
| CrimsonFrenzy | OK | OK | OK | OK | OK | OK | OK | as Blood Frenzy |

"-" in the note column means nothing special.

## Fixes made

1. **Pounce / Savage Pounce never fired after the opening.** They select a random player, but
   the brain and the instant-charge path both measured and aimed at the monster's own victim,
   who is almost always inside the 6 m minimum range. The brain now treats a random pick with
   a minimum range like a farthest-hostile charge (somebody out there in range), the executor
   scans for it, and `charge_recipient` picks a seeded random hostile in range and sight,
   preferring anyone other than the current target.
2. **Random picks ignored sight.** Marks, Garrote, Crippling Arrow and the leaps could choose a
   player behind a wall and then lose the cast. `eligible` now requires line of sight (falling
   back to the victim, as before, when nobody is visible).
3. **Marks laid on the dead.** A mark whose victim died or left during the cast was still laid
   on the corpse. Delayed follow zones are now skipped if the victim is not alive at cast end.
4. **Hazard ticks were not flagged periodic**, so burning ground drew combat text as full hits.
   Hazard (and trail) ticks now set the periodic flag.

## Notes (no change)

- Damage scale: `DungeonScaling` is applied once, at spawn (`runtime.damage_scale = level x
  party x key`). Adds inherit the boss's already-scaled value with their own level at 1. Hits,
  channel ticks, marks, shockwaves, trails, pools and bleeds all use it exactly once.
- Cooldowns start at cast start and are divided by frenzy; recovery is divided too; cast time
  is not. An interrupt leaves the cooldown running and doubles the recovery.
- A stun (Shield Charge) cancels any cast, uninterruptible ones included.
- Bleed (`Rended`) has no dispel school: "a wound, not a spell". Slows (`Crippled`) are Curses.
  Mass Dispel strips enemy *purgeable buffs* only; frenzy is brain state, so it cannot be purged.
- Plagued only lands on players, so enemy heals are never reduced by it.
- A mark that outlives its victim still lands where they fell. A multi-target cast whose recipient dies mid-cast still emits an
  `AbilityUsed` event claiming one target.
- Channels pay their first tick one tick after the cast begins and their last before it ends.
- With two charge-like abilities of different range on one monster the farthest-hostile scan
  uses the longest range; none exists in the roster.
- Trinket absorb is consumed by the same `defend_player` as the Warrior's (`trinkets.rs`).

## Cue / VFX / audio mapping

Every ability has an `EnemyCue` (a required field) driving the cast bed sound
(`audio.rs::begin_enemy_cast`), an enemy cast VFX on `CastStarted`, a floor telegraph for every
area (`telegraph_indicator.rs`, resolved burst on landing), area-effect VFX for pools and
trails, and hit sounds from `Damage`. Maps to nothing beyond that: **Blood Frenzy** and
**Crimson Frenzy** (instant self buffs: no sound/VFX at use), **Mend** (no heal sound or
VFX), and the instant fillers (Swing, Sunder, Bone Slash, Crushing Blow, Ghoul Strike, Rending
Claws, Rapier Thrust, Feast on the Wounded) have no use cue beyond the animation and the hit
sound. Not fixed: no existing asset is a trivial fit and the presentation files are owned by
parallel work.
