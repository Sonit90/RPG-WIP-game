# Brute

## Role

Melee bruiser. The dungeon's baseline body: the thing that walks at the player and has to be
positioned around.

## Combat Fantasy

Nothing clever. It closes, it hits, and once in a while it winds up a wide swing that takes
long enough to see coming. Every other monster in the roster is defined against it — the
Brute is what "a normal enemy" means here, so a pack that is only Brutes is a pack about
where the player is standing and nothing else.

## Base Stats

| | |
|---|---|
| Health | 320 |
| Movement speed | 5.2 m/s |
| Radius / height | 0.6 m / 2.0 m |
| Attack range | 5.0 m (Cleave), 3.0 m (Swing) |
| Melee range | 3.0 m |
| Basic attack damage | 14 |
| Attack speed | one Swing per 1.8 s (0.4 s recovery after) |
| Aggro range | 14 m |
| Leash range | 40 m |
| Preferred range | 2.4 m (± 0.6 m dead band) |
| Alert time | 0.35 s |

## Abilities

### CLEAVE

| | |
|---|---|
| Purpose | The telegraphed threat. Punishes standing in front of it. |
| Range | 5.0 m to start |
| Cooldown | 9.0 s |
| Cast time | 1.4 s |
| Damage | 26 physical |
| Target selection | Aimed along the caster's facing at the moment the cast starts |
| Area | Cone, 6 m long, 60° wide |
| Telegraph | Cone on the floor for the full 1.4 s |
| Interruptible | Yes |
| Counterplay | Walk out of the cone sideways, or interrupt it. The cone is fixed when the cast starts — moving works. |
| Recovery | 0.8 s |

### SWING

| | |
|---|---|
| Purpose | Filler. What it does when nothing else is ready. |
| Range | 3.0 m |
| Cooldown | 1.8 s |
| Cast time | instant |
| Damage | 14 physical |
| Target selection | Its current victim |
| Area | Single target |
| Telegraph | None — instants are not telegraphed |
| Interruptible | No |
| Counterplay | Be further than 3 m away. Range is re-checked when it lands, not when it was chosen. |
| Recovery | 0.4 s |

## AI Behavior

- **Idle** — stands at its spawn point facing the way it was placed.
- **Aggro** — anything hostile within 14 m, or anything that damages it from any range.
- **Alert** — 0.35 s of turning to look before it moves. This is the readable beat of a pull.
- **Target selection** — highest threat; nearest hostile if it has no threat yet.
- **Movement** — closes to 2.4 m and stops. It does not circle or strafe.
- **Ability selection** — priority order: Cleave, then Swing. First one off cooldown and in
  range is used.
- **Retreat / reset** — beyond 40 m from its spawn point it drops threat, cancels any cast
  and walks home, then goes Idle facing its spawn yaw.

## Player Counterplay

Kite it. It is slower than the player (5.2 vs 7.0 m/s), so distance is always winnable.
When the Cleave telegraph appears, step out of the wedge — it does not follow. In a pack,
the Brute is the target that can be ignored longest, because it does the least damage per
second of anything in the roster.

## VFX

- Cleave: the existing enemy cone telegraph on the floor, its wedge exactly the gameplay
  cone, plus the generic enemy cast bar.
- Swing: the generic melee impact effect at the victim.
- Death: the generic enemy death effect.

No sound requirements.

## Encounter Usage

| Pack | Why |
|---|---|
| 1 Gatehouse Watch (×2) | Introduces melee threat with nothing else in the picture |
| 2 Gate Crossfire (×1) | Something that has to be dealt with while the Adepts cast |
| 3 Bone Choir (×2) | Pressure while the player is trying to reach the Ritualist |
| 4 Warden's Circle (×1) | The thing the Warden heals |
| 6 Weave and Word (×1) | Body in the way while the ground is on fire |
| 7 Vault Sentinel (×1) | A second target so the Revenant is not fought alone |
| 8 Last Rite (×1) | Same role, now the least of four problems |

## Testing

- Idle → Alert on proximity, and on damage from outside aggro range.
- Alert lasts exactly `alert_time` before Chase.
- Chase stops at `preferred_range` and does not oscillate inside the dead band.
- Cleave's telegraph shape equals the shape that deals the damage.
- Cleave is interruptible; Swing is not.
- Swing does nothing if the target left 3 m between decision and landing.
- Beyond leash: cast cancelled, threat cleared, walks home, returns to Idle.
- Death stops everything and takes any telegraph down.
