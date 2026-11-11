# Bone Ritualist

## Role

Interrupt-critical caster. The monster the player's interrupt exists for.

## Combat Fantasy

It spends three seconds building something enormous. Three seconds is a long time — long
enough to notice, cross the room, and stop it — and if it is allowed to finish, 85 damage
lands in a 6.5 m circle, which is a sixth of the player's health for standing still.

The Ritualist is not dangerous. Ignoring the Ritualist is dangerous.

## Base Stats

| | |
|---|---|
| Health | 260 |
| Movement speed | 5.2 m/s |
| Radius / height | 0.5 m / 1.9 m |
| Attack range | 24 m (Ritual Nova), 25 m (Bolt) |
| Melee range | n/a |
| Basic attack damage | 12 (Bolt) |
| Attack speed | one Bolt per 2.4 s |
| Aggro range | 10.8 m |
| Leash range | 45 m |
| Preferred range | 18 m (± 3 m dead band) |
| Alert time | 0.5 s |

## Abilities

### RITUAL NOVA

| | |
|---|---|
| Purpose | The whole monster. A cast that must be answered. |
| Range | 24 m |
| Cooldown | 13.0 s |
| Cast time | 3.0 s |
| Damage | 85 fire |
| Target selection | Centred on its victim's position when the cast started |
| Area | Circle, 6.5 m radius |
| Telegraph | Ring on the floor for the full 3.0 s, the largest in the trash roster |
| Interruptible | **Yes** — this is the point |
| Counterplay | Interrupt it, or leave a 6.5 m circle within 3 s. Both work; the interrupt is cheaper, and an interrupted Ritualist is locked out for twice the ability's recovery. |
| Recovery | 1.4 s (2.8 s if interrupted) |

### BOLT

Shared with the Ember Adept — see [ember_adept.md](ember_adept.md). Range 25 m, 1.2 s cast,
2.4 s cooldown, 12 arcane, interruptible.

## AI Behavior

- **Idle** — stands at its spawn point.
- **Aggro** — 10.8 m, or any damage.
- **Pack** — it is pulled when *any* member of its pack is, whether or not it noticed
  anything itself. Membership belongs to the encounter; this monster knows only its own
  aggro radius.
- **Alert** — 0.5 s.
- **Target selection** — highest threat, else nearest hostile.
- **Movement** — holds at 18 m. Further out than the Adept, so in a mixed pack the Ritualist
  is the one at the back and reaching it is a decision.
- **Ability selection** — Ritual Nova, then Bolt. Nova has a 13 s cooldown, so the rhythm of
  fighting one is: nova window, quiet, nova window.
- **Retreat / reset** — 45 m leash.

## Player Counterplay

**What the player must notice:** a 6.5 m ring and a three-second cast bar.

**What the player must do:** interrupt it, or be outside the ring when the bar fills.

**What happens on failure:** 85 damage. Twice in one pack without healing is most of the
player's health, which is what makes the interrupt a real decision rather than a formality.

## VFX

- Ritual Nova: floor ring at exactly the 6.5 m gameplay radius, growing intensity over the
  cast; a large fire burst on resolution, and no burst at all when interrupted.
- Cast bar, with the existing interruptible styling.
- Bolt: generic projectile and impact.

No sound requirements.

## Encounter Usage

| Pack | Why |
|---|---|
| 3 Bone Choir (×1) | Introduces the interrupt requirement, with two Brutes making it awkward |
| 6 Weave and Word (×1) | Interrupt while the ground is on fire |
| 8 Last Rite (×1) | Interrupt while a healer is up and the ground is on fire |

## Testing

- Ritual Nova is interruptible, and interrupting it cancels the telegraph and does no damage.
- An interrupted Ritualist stays in Recover for longer than one that completed the cast.
- The nova's damage circle equals the drawn ring.
- Leaving the ring during the cast avoids the damage entirely.
- Nova respects its 13 s cooldown across a whole fight.
