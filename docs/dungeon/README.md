# The Ashen Reliquary

The engine's first complete dungeon. One entrance, six areas — five of them fought in — eight packs, six monster types
and a three-phase boss. No loot, no rewards, no progression — the dungeon ends when the boss
dies and can be reset and replayed immediately.

This file is the overview. **It does not replace the per-monster and per-boss documents**,
which are the source of truth for mechanics.

---

## Theme

A reliquary that burned. The order that kept it is still here, still working, and still on
fire. Stone halls, ash on the floor, braziers that were never put out, and a gate at the far
end that something large is standing in front of.

---

## Layout

```text
   ENTRANCE (an arch in a closed wall)
      │
      ▼
  ┌────────────────┐
  │ 0 ENTRANCE HALL│  no pack — arriving is not the same event as being noticed
  └────────────────┘
      │  corridor
      ▼
  ┌─────────────┐
  │ 1 GATEHOUSE │  Pack 1 · Pack 2
  └─────────────┘
      │  corridor
      ▼
  ┌──────────────┐
  │ 2 OSSUARY    │  Pack 3 · Pack 4
  └──────────────┘
      │  corridor
      ▼
  ┌──────────────────┐
  │ 3 CINDER GALLERY │  Pack 5 · Pack 6
  └──────────────────┘
      │  corridor
      ▼
  ┌───────────────────┐
  │ 4 RELIQUARY VAULT │  Pack 7 · Pack 8
  └───────────────────┘
      │  corridor
      ▼
  ┌──────────────────┐
  │ 5 ASHEN GATE     │  BOSS
  └──────────────────┘
```

The dungeon runs along −Z. Rooms are rectangles built from eight-metre wall segments and
joined by 16 m corridors; the doorway between a room and a corridor is exactly two segments
left out, so the two line up with no gap at the junction. Every wall, pillar and arch carries
its own collision box — there are no invisible walls anywhere inside the dungeon.

| Area | Centre (x, z) | Half-extent | Packs |
|---|---|---|---|
| Entrance Hall | (0, −14) | 12 × 12 | — |
| Gatehouse | (0, −58) | 16 × 16 | 1, 2 |
| Ossuary Hall | (0, −106) | 16 × 16 | 3, 4 |
| Cinder Gallery | (0, −154) | 16 × 16 | 5, 6 |
| Reliquary Vault | (0, −202) | 16 × 16 | 7, 8 |
| Ashen Gate | (0, −266) | 24 × 24 | Boss |

A combat room is 32 m across because two packs stand at opposite corners of it and the widest
aggro radius in the roster is 20 m: 32 m puts them 28 m apart, which is the smallest room in
which two packs are two packs rather than one. The player starts in the entrance hall, out of
everything's notice, and every pull after that is the player's own doing.

## Encounter order

Packs are pulled in order because the dungeon is linear: each stands inside its own room, and
the corridor between rooms is long enough that a pack cannot be noticed from the previous one.
Nothing sequences them — the order is a consequence of where they stand, which is asserted by
`walking_forward_pulls_the_packs_in_order` in `crates/engine-game/tests/dungeon.rs`.

1. **Gatehouse Watch** — 2 × Brute
2. **Gate Crossfire** — 1 × Brute, 2 × Ember Adept
3. **Bone Choir** — 1 × Bone Ritualist, 2 × Brute
4. **Warden's Circle** — 1 × Crypt Warden, 1 × Brute, 1 × Ember Adept
5. **Cinder Loom** — 2 × Cinder Weaver
6. **Weave and Word** — 1 × Cinder Weaver, 1 × Bone Ritualist, 1 × Brute
7. **Vault Sentinel** — 1 × Ash Revenant, 1 × Brute
8. **Last Rite** — 1 × Crypt Warden, 1 × Bone Ritualist, 1 × Cinder Weaver, 1 × Brute
9. **Warden of the Ashen Gate** — the boss

---

## Monster roster

| Monster | Role | Document |
|---|---|---|
| Brute | Melee bruiser | [brute.md](monsters/brute.md) |
| Ember Adept | Ranged caster | [ember_adept.md](monsters/ember_adept.md) |
| Bone Ritualist | Interrupt-critical caster | [bone_ritualist.md](monsters/bone_ritualist.md) |
| Crypt Warden | Support / healer | [crypt_warden.md](monsters/crypt_warden.md) |
| Cinder Weaver | Area controller | [cinder_weaver.md](monsters/cinder_weaver.md) |
| Ash Revenant | Elite specialist | [ash_revenant.md](monsters/ash_revenant.md) |

## Boss

[Warden of the Ashen Gate](boss/warden_of_the_ashen_gate.md) — 6,000 health, three phases.

---

## Difficulty progression

Difficulty is added as *axes of attention*, not as health and damage. Across the whole
dungeon the largest pack's total health is under 2× the first pack's; what changes is how
many things must be true at the same time.

| Pack | Position | Line of sight | Interrupt | Target priority | Keep moving |
|---|---|---|---|---|---|
| 1 | ● | | | | |
| 2 | ● | ● | | | |
| 3 | ● | | ● | | |
| 4 | ● | | | ● | |
| 5 | ● | | | | ● |
| 6 | ● | | ● | | ● |
| 7 | ● | | | ● | ● |
| 8 | ● | ● | ● | ● | ● |
| Boss | ● | | ● | | ● |

---

## What this dungeon deliberately does not have

Loot, drops, rewards, equipment, currencies, XP, achievements, quests, matchmaking,
networking, a second class, crafting or an inventory. It also adds **no new ability sounds** —
existing generic combat audio continues to play, and nothing in this phase authored a cue.
