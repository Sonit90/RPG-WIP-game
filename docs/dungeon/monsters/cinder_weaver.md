# Cinder Weaver

## Role

Area controller. It does not try to kill the player — it tries to decide where the player is
allowed to stand.

## Combat Fantasy

Every few seconds a patch of the floor catches fire and stays on fire for nine seconds. Two
Weavers working together turn a room into a shrinking set of safe places, and the fight
becomes about footwork rather than about damage.

## Base Stats

| | |
|---|---|
| Health | 230 |
| Movement speed | 5.72 m/s |
| Radius / height | 0.5 m / 1.9 m |
| Attack range | 22 m (Cinder Pool), 25 m (Bolt) |
| Melee range | n/a |
| Basic attack damage | 12 (Bolt) |
| Attack speed | one Bolt per 2.4 s |
| Aggro range | 10.8 m |
| Leash range | 45 m |
| Preferred range | 15 m (± 3 m dead band) |
| Alert time | 0.5 s |

## Abilities

### CINDER POOL

| | |
|---|---|
| Purpose | Denies ground. The only lasting hazard in the trash roster. |
| Range | 22 m |
| Cooldown | 7.0 s |
| Cast time | 1.6 s |
| Damage | 8 fire on impact, then **14 fire per second** to anything standing in it |
| Duration | 9 s of burning ground after it lands |
| Target selection | Centred on its victim's position when the cast started |
| Area | Circle, 3.5 m radius — both the impact and the lasting pool |
| Telegraph | Ring on the floor for the 1.6 s cast; the pool that replaces it is drawn for its full 9 s |
| Interruptible | Yes |
| Counterplay | Move out of the ring before it lands, and do not stand in what it leaves. Interrupting stops the pool from existing at all. |
| Recovery | 0.6 s |

The impact damage is small on purpose: the pool is the mechanic, and being clipped by the
landing should cost far less than standing in the result.

### BOLT

Shared with the Ember Adept — see [ember_adept.md](ember_adept.md).

## AI Behavior

- **Idle** — stands at its spawn point.
- **Aggro** — 10.8 m, or any damage.
- **Pack** — it is pulled when *any* member of its pack is, whether or not it noticed
  anything itself. Membership belongs to the encounter; this monster knows only its own
  aggro radius.
- **Alert** — 0.5 s.
- **Target selection** — highest threat, else nearest hostile.
- **Movement** — holds at 15 m. It does not reposition to cut the player off; the pools do
  that, and an AI that also flanked would be two mechanics fighting each other.
- **Ability selection** — Cinder Pool, then Bolt. A 7 s cooldown against a 9 s pool duration
  means one Weaver keeps slightly more than one pool alive; two Weavers keep three.
- **Retreat / reset** — 45 m leash. **Its pools do not survive a reset** — see the dungeon
  reset rules.

## Player Counterplay

**What the player must notice:** the ring going down, and the pool that is left behind.

**What the player must do:** keep moving, and pick where to move *before* the ring lands.
The FireMage kit has a blink; this is the pack it is for.

**What happens on failure:** 14 damage a second, which is survivable for a moment and fatal
if the player stops paying attention while casting.

## VFX

- Cast: floor ring at exactly the 3.5 m gameplay radius.
- Pool: a persistent burning-ground effect covering exactly the same 3.5 m, visible for its
  full 9 s and removed the instant the hazard expires or the encounter resets.
- Bolt: generic projectile and impact.

No sound requirements.

## Encounter Usage

| Pack | Why |
|---|---|
| 5 Cinder Loom (×2) | Introduces lasting ground hazards with nothing else in the picture |
| 6 Weave and Word (×1) | Hazards plus an interrupt requirement |
| 8 Last Rite (×1) | Hazards as one of four problems |

## Testing

- The pool's damage radius equals the drawn radius.
- The pool ticks at a fixed cadence and stops at exactly its duration.
- Interrupting the cast leaves no pool.
- Pools are removed on encounter reset, on dungeon reset, and when their owner dies mid-cast.
- Two Weavers can have three pools alive simultaneously without either losing one.
- The pool damages the player and never its own faction.
