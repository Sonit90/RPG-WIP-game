# Crypt Warden

## Role

Support / healer. It does almost no damage and makes everything else in the pack take twice
as long to kill.

## Combat Fantasy

A pack with a Warden in it does not die in the order the player chooses — it dies in the order
the Warden allows. Killing the Brute you have been hitting for ten seconds and watching it
come back to full is the lesson, and it is taught exactly once.

## Base Stats

| | |
|---|---|
| Health | 240 |
| Movement speed | 5.72 m/s |
| Radius / height | 0.5 m / 1.9 m |
| Attack range | 22 m (Mend), 25 m (Bolt) |
| Melee range | n/a |
| Basic attack damage | 12 (Bolt) |
| Attack speed | one Bolt per 2.4 s |
| Aggro range | 16 m |
| Leash range | 45 m |
| Preferred range | 14 m (± 3 m dead band) |
| Alert time | 0.45 s |

## Abilities

### MEND

| | |
|---|---|
| Purpose | Undoes the player's damage on somebody else. |
| Range | 22 m |
| Cooldown | 9.0 s |
| Cast time | 2.0 s |
| Healing | 140 health restored |
| Target selection | **The most wounded living ally within 22 m**, the Warden included. If nothing is wounded, the ability is not chosen at all and it Bolts instead. |
| Area | Single target |
| Telegraph | Cast bar only. Healing is not a floor hazard, so nothing is drawn on the ground. |
| Interruptible | Yes |
| Counterplay | Interrupt it, or kill the Warden first. 140 is nearly half a Brute. |
| Recovery | 0.8 s |

### BOLT

Shared with the Ember Adept — see [ember_adept.md](ember_adept.md).

## AI Behavior

- **Idle** — stands at its spawn point.
- **Aggro** — 16 m, the shortest of the ranged monsters, so it is usually pulled *with* its
  pack rather than on its own.
- **Pack** — it is pulled when *any* member of its pack is, whether or not it noticed
  anything itself. Membership belongs to the encounter; this monster knows only its own
  aggro radius.
- **Alert** — 0.45 s.
- **Target selection** — for damage: highest threat, else nearest hostile. For Mend: the ally
  with the lowest health fraction inside 22 m, which may be itself.
- **Movement** — holds at 14 m, closer than the other casters, because a healer that stands
  further back than everything it heals is a healer the player cannot reach.
- **Ability selection** — Mend, then Bolt. Mend is only *available* when a wounded ally
  exists, so an untouched pack's Warden opens with Bolts.
- **Retreat / reset** — 45 m leash.

## Player Counterplay

**What the player must notice:** health going back up on something they were killing, and a
cast bar on a monster at the back of the room.

**What the player must do:** switch targets and kill the Warden, or interrupt the Mend. Its
240 health is the second lowest in the roster — it is designed to die quickly once chosen.

**What happens on failure:** the pack does not die. Nothing kills the player quickly for
ignoring the Warden; the punishment is a fight that never ends, which is the correct
punishment for a target-priority mechanic.

## VFX

- Mend: a visible link or beam from the Warden to the ally it is healing for the duration of
  the cast, and a heal burst on the ally when it lands. The link is how the player learns
  *which* monster is the healer without reading a nameplate.
- Cast bar.
- Bolt: generic projectile and impact.

No sound requirements.

## Encounter Usage

| Pack | Why |
|---|---|
| 4 Warden's Circle (×1) | Introduces target priority, with a Brute and an Adept as the things it keeps alive |
| 8 Last Rite (×1) | Priority under pressure: it is now one of four decisions |

## Testing

- Mend is only chosen when a wounded ally is in range; otherwise the Warden Bolts.
- Mend heals the *most* wounded ally, not the nearest.
- A full-health pack produces no Mend casts at all.
- Mend is interruptible, and an interrupted Mend restores no health.
- Healing is capped at maximum health and emits a healing event with the amount actually
  restored.
- The Warden will heal itself if it is the most wounded thing in range.
