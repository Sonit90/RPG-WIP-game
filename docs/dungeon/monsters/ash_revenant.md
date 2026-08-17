# Ash Revenant

## Role

Elite specialist. A single large melee enemy with two phases of its own — the dungeon's
rehearsal for the boss.

## Combat Fantasy

It is the first thing in the dungeon that changes while you are fighting it. For the first
half of its health it is a slow, heavy melee enemy with one avoidable nova. Below 50 % it
speeds up, its cooldowns shorten, and it gains the Brute's cleave on top of everything it
already had. The player who learned the first half has to re-learn the second.

## Base Stats

| | |
|---|---|
| Health | 900 |
| Movement speed | 6.5 m/s (6.89 m/s in phase 2) |
| Radius / height | 0.75 m / 2.3 m |
| Attack range | 7.0 m (Ash Burst), 3.5 m (Sunder), 5.0 m (Cleave, phase 2) |
| Melee range | 3.5 m |
| Basic attack damage | 22 (Sunder) |
| Attack speed | one Sunder per 2.6 s (1.8 s in phase 2) |
| Aggro range | 16 m |
| Leash range | 45 m |
| Preferred range | 2.6 m (± 0.6 m dead band) |
| Alert time | 0.6 s |

## Phases

```text
Phase 1  ── health ≤ 50 % ──▶  Phase 2
```

| Phase | Threshold | Abilities (priority order) | Speed | Cooldowns |
|---|---|---|---|---|
| 1 | start | Ash Burst, Sunder | ×1.00 | ×1.00 |
| 2 | ≤ 50 % health | Ash Burst, Cleave, Sunder | ×1.06 | ×0.70 |

The transition cancels whatever it is casting, drops the telegraph, and staggers it for
1.5 s. That stagger is the player's window to notice something changed.

## Abilities

### ASH BURST

| | |
|---|---|
| Purpose | Punishes melee range. The one thing the player has to move for. |
| Range | 7.0 m to start |
| Cooldown | 11.0 s (7.7 s in phase 2) |
| Cast time | 1.8 s |
| Damage | 46 fire |
| Target selection | Centred on **the Revenant itself** |
| Area | Circle, 7 m radius |
| Telegraph | Ring on the floor around the Revenant for the full 1.8 s |
| Interruptible | **No** — this one has to be walked out of |
| Counterplay | Be more than 7 m away when it resolves. 1.8 s at 7 m/s is 12.6 m of travel: always winnable, never free. |
| Phase availability | Both |
| Recovery | 1.0 s |

### SUNDER

| | |
|---|---|
| Purpose | Filler. Harder than a Brute's Swing, and that is the whole difference. |
| Range | 3.5 m |
| Cooldown | 2.6 s (1.8 s in phase 2) |
| Cast time | instant |
| Damage | 22 physical |
| Target selection | Its current victim |
| Area | Single target |
| Telegraph | None |
| Interruptible | No |
| Counterplay | Stay outside 3.5 m. |
| Phase availability | Both |
| Recovery | 0.5 s |

### CLEAVE

Shared with the Brute — see [brute.md](brute.md). 1.4 s cast, 26 physical, 6 m 60° cone,
interruptible.

| | |
|---|---|
| Phase availability | **Phase 2 only** |
| Cooldown in phase 2 | 6.3 s |
| Counterplay | Step out of the wedge, or interrupt. In phase 2 the player is already moving for Ash Burst, so the cone is a second thing to track. |

## AI Behavior

- **Idle** — stands at its spawn point.
- **Aggro** — 16 m, or any damage.
- **Pack** — it is pulled when *any* member of its pack is, whether or not it noticed
  anything itself. Membership belongs to the encounter; this monster knows only its own
  aggro radius.
- **Alert** — 0.6 s, the longest in the roster: an elite gets a longer beat before it starts.
- **Target selection** — highest threat, else nearest hostile.
- **Movement** — closes to 2.6 m and stops. It never retreats; the Ash Burst is what makes
  melee range dangerous, not repositioning.
- **Ability selection** — its current phase's priority list, first ready and in range.
- **Phase** — re-evaluated every tick from its health fraction. Phases only advance; healing
  a Revenant above 50 % does not put it back into phase 1.
- **Retreat / reset** — 45 m leash. A reset returns it to phase 1 with full health.

## Player Counterplay

**What the player must notice:** the ring around the Revenant, and the moment it staggers at
half health.

**What the player must do:** leave the 7 m circle every 11 s (every 7.7 s after the
transition), and from phase 2 onward also read the cleave cone.

**What happens on failure:** 46 damage from the burst, 26 from the cleave. Neither is lethal
alone; together, with a Brute also hitting the player, they are the fight.

## VFX

- Ash Burst: expanding floor ring at exactly the 7 m gameplay radius, anchored on the
  Revenant and following it for the cast; a large burst on resolution.
- Phase transition: a visible one-off burst and a change in the Revenant's ambient effect, so
  "it changed" is legible without a health bar.
- Cleave: the same cone telegraph the Brute uses.
- Sunder: the generic melee impact.

No sound requirements.

## Encounter Usage

| Pack | Why |
|---|---|
| 7 Vault Sentinel (×1) | The only pack it appears in. Paired with one Brute so it is not fought in a vacuum, and placed immediately before the boss so the phase mechanic is learned once before it matters. |

## Testing

- Starts in phase 1 with the phase-1 ability list.
- Crossing 50 % health advances to phase 2 exactly once, and emits one phase-change event.
- Healing it back above 50 % does not revert the phase.
- Phase 2 has Cleave available; phase 1 does not.
- Phase 2 cooldowns are 0.70× the table values and its speed is 1.06×.
- The transition cancels an in-flight cast and removes its telegraph.
- Reset returns it to phase 1, full health, home, idle.
- Ash Burst is not interruptible and its damage radius equals the drawn ring.
