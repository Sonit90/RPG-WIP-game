# Ember Adept

## Role

Ranged caster. Damage from a distance that the player has to close on, break line with, or
interrupt.

## Combat Fantasy

It never comes to you. It stands at the back of the room dropping fire on the ground you are
standing on, and the ground it picks is the ground you were standing on when it started —
so every blast is a question about whether you were paying attention two seconds ago.

## Base Stats

| | |
|---|---|
| Health | 210 |
| Movement speed | 4.2 m/s |
| Radius / height | 0.5 m / 1.9 m |
| Attack range | 25 m |
| Melee range | n/a |
| Basic attack damage | 12 (Bolt) |
| Attack speed | one Bolt per 2.4 s |
| Aggro range | 20 m |
| Leash range | 45 m |
| Preferred range | 16 m (± 3 m dead band) |
| Alert time | 0.5 s |

It stops well short of its own 25 m reach so it stands somewhere the player can see and
reach it, rather than sniping from the far wall.

## Abilities

### GROUND BLAST

| | |
|---|---|
| Purpose | The telegraphed threat. Makes the player move. |
| Range | 25 m |
| Cooldown | 7.0 s |
| Cast time | 2.0 s |
| Damage | 34 fire |
| Target selection | Centred on its victim's position **when the cast started** |
| Area | Circle, 4 m radius |
| Telegraph | Ring on the floor for the full 2.0 s |
| Interruptible | Yes |
| Counterplay | Walk out of the ring, or interrupt it. The ring does not follow. |
| Recovery | 1.0 s |

### BOLT

| | |
|---|---|
| Purpose | Filler. Constant, small, unavoidable-by-movement pressure. |
| Range | 25 m |
| Cooldown | 2.4 s |
| Cast time | 1.2 s |
| Damage | 12 arcane |
| Target selection | Its current victim |
| Area | Single target |
| Telegraph | Cast bar only — there is no area to draw |
| Interruptible | Yes |
| Counterplay | Break the 25 m range, kill it, or eat it: 12 damage is meant to be affordable. |
| Recovery | 0.3 s |

## AI Behavior

- **Idle** — stands at its spawn point.
- **Aggro** — 20 m, or any damage.
- **Alert** — 0.5 s, slightly longer than the Brute's, so a mixed pull reads as two separate
  reactions rather than one.
- **Target selection** — highest threat, else nearest hostile.
- **Movement** — closes only to 16 m and stops. It does not retreat when the player closes;
  a caster that kited would make melee-range play impossible for a class that has none.
- **Ability selection** — Ground Blast, then Bolt.
- **Retreat / reset** — 45 m leash, same behaviour as every other monster.

## Player Counterplay

Two Adepts alternate Ground Blasts, so the floor is only sometimes safe — the intended answer
is to keep walking while casting instants and to interrupt the blast that lands where you
have to stand. Its 210 health is the lowest of the ranged monsters: it dies fast if you
decide it should.

## VFX

- Ground Blast: circular floor telegraph matching the 4 m gameplay radius; fire impact on
  resolution.
- Bolt: the generic projectile trail from caster to victim, and the generic impact.
- Cast bar for both.

No sound requirements.

## Encounter Usage

| Pack | Why |
|---|---|
| 2 Gate Crossfire (×2) | Introduces ranged threat, two of them so the floor is rarely all safe |
| 4 Warden's Circle (×1) | The damage the player must accept while killing the healer |

## Testing

- Ground Blast's telegraph radius equals the radius that deals the damage.
- The blast lands where the target *was*, not where it is: walking out avoids it entirely.
- Interrupting Ground Blast cancels the telegraph and deals no damage.
- Preferred range holds at 16 m; the Adept does not walk into melee.
- Leash and reset as the Brute.
