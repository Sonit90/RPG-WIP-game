# Lord Ashgrave, the Hollow Steward

The Graveyard Crypt's boss, inspired by Moroes in Karazhan and built as an original encounter:
a gaunt undead steward in crimson and black who greets the party with his blade, a garrote for
whoever is not the tank, a waltz through the room — and **two guests** drawn from the crypt's
own roster, in their empowered forms. He is the dungeon's final exam: every add mechanic is one
the party has already learned, escalated.

| | |
|---|---|
| id | `lord_ashgrave` |
| Health | 16,000 (party of three) |
| Size | 3.2 m tall, 1.0 m radius — the largest silhouette in the crypt, dark crimson, faint glow |
| Speed | 6.3 m/s (6.6 unmasked) — slower than the player in both phases |
| Aggro / leash | 22 m / 70 m |
| Arena | Hall of the Hollow Steward, 48 × 48 m, four columns inside his Dance of Blades |
| Sealed | until the dungeon reaches 100 % (see the README) |

## Abilities

| Pri | Ability | Cast | CD | Damage | Shape / targeting | Answer |
|---|---|---|---|---|---|---|
| 1 | **Mark of the Host** | 1.0 s | 24 s | 110 Arcane | 6 m circle **following** a random non-tank, lands 5 s later | take it away from the party |
| 2 | **Ghastly Waltz** | 1.5 s | 22 s | 70 + 1 s slow | cast on the **farthest** player (≥ 8 m): he runs at them and hits them, leaving a **trail** of pools (25/s, 5 s) | heal them; stay off the trail |
| 3 | **Dance of Blades** | 2.2 s | 16 s | 95 | **8 m circle** around himself, uninterruptible | everyone out, including the tank |
| 4 | **Garrote** | 0.6 s | 20 s | 20 + **bleed** 8/s per stack, 20 s, 2 stacks a use, up to 6 | a random **non-tank** | the healer's long problem |
| 5 | **Gouging Riposte** | 1.2 s | 12 s | 90 + **50 % slow** 4 s | the tank | a defensive; expect to be slow for the next Dance |
| 6 | Rapier Thrust | instant | 1.5 s | 36 | the tank | — |

**Unmasked (≤ 30 %):** one phase change, announced like every phase (the cast in flight is
dropped, 1.5 s stagger). He gains **Crimson Frenzy** (+50 % attack speed for 10 s, every 25 s),
every cooldown is ×0.8, and he moves ×1.05. The garrote comes off his list; the bleeds already
on the party keep ticking.

Everything is readable: circles on the floor, a violet ring riding the marked player, cast
bars on the target frame (the Waltz is a cast on a player, not a line on the floor), the
frenzy's faster swings.

## The guests

When he is pulled — his encounter goes active, which needs the gate open — he calls
`add_count` = **2** adds from `add_pool` (the six crypt monsters), **without duplicates**, each
spawned in its **empowered** form at the two `add_spawns` either side of him, already aggressive
towards whoever pulled him. They join his encounter: the fight is won when he and both adds are
dead, and `DungeonCompleted` fires once.

| Normal | Empowered | What escalates |
|---|---|---|
| Skeleton Bonecaster | Empowered Bonecaster | marks on three, and chain marks; 7-zone barrage, 1.2 s warning |
| Grave Acolyte | Empowered Acolyte | drain on three at once; corruption that spreads to 8 m |
| Crypt Archer | Empowered Archer | 9 rain zones at 1.3 s; volleys that pierce and curse the ground |
| Skeleton Warrior | Empowered Warrior | 150° Massacre; a charge with a shockwave |
| Grave Brute | Empowered Brute | six-zone Gravequake; a charge with a burning trail |
| Crypt Ghoul | Empowered Ghoul | 2-stack 10-high bleed, extra bites on bleeding targets, exploding pounce |

Empowered adds have about twice the health, glow, and are larger — but each keeps two
abilities the party already knows (so it is recognisable) and replaces two with a *structurally*
harder version: more zones, more targets, less warning, a second hit, a chain, a trail, a
growing pool or a conditional attack. A test asserts no escalation is "bigger numbers only".

**Randomness.** The pick uses the simulation's seeded enemy generator: the same seed calls the
same pair, 24 seeds call all six. For testing and practice,
`simulation.dungeon_run_mut().force_adds(Some(vec![...]))` fixes the pair, and
`simulation.reseed_enemies(seed)` rerolls.

## Suggested kill order

Kill the add whose mechanic the party handles worst first — Acolyte (interrupts) or Ghoul
(bleed on top of the garrote) are the usual picks — while the tank keeps Ashgrave facing away
and everyone leaves the Dance. Fighting him with both adds up past ~70 % is the common wipe.

## Reset

A wipe or a run-away: he loses his target, walks home and goes idle — his adds are despawned,
he is restored to full, and the next pull calls a **fresh** pair. Monsters killed elsewhere in
the dungeon stay dead and completion is kept. `BACKSPACE` while alive is the development reset
of the whole dungeon, sealing him again at 0 %.

## Balance (party of three)

Boss + the two largest empowered adds ≈ 23,700 health: about 2.2 minutes at the assumed
180 party DPS (tested to sit within 1.5–3.5 min). His unavoidable tank damage is Rapier Thrust
(~16 DPS after armour) plus the Riposte; the rest of the fight is movement, a mark, a bleed on a
non-tank and whichever two lessons the adds bring.
