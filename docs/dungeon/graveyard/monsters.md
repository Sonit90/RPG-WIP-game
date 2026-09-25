# Graveyard Crypt — the six monsters

Three ranged, three melee. Every ability is **offensive**: no shields, no damage reduction, no
self-healing — each one is a problem the party has to answer. Numbers are the party-of-three
tuning at level one (`engine-game/src/enemy/mod.rs`, `ENEMY_ABILITIES` / `ARCHETYPES`); the
priority column is the order the brain tries them in, which is the whole decision policy.

| Monster | id | Role | Health | Speed | Aggro | Worth | Teaches |
|---|---|---|---|---|---|---|---|
| Skeleton Bonecaster | `skeleton_bonecaster` | Ranged AoE | 1,000 | 5.4 | 18 | 4 | dodge zones, carry marks away |
| Grave Acolyte | `grave_acolyte` | Ranged caster / ground | 1,150 | 5.4 | 18 | 5 | **interrupt**, leave pools |
| Crypt Archer | `crypt_archer` | Long-range physical | 950 | 5.6 | 18 | 4 | rain, slows, spread damage |
| Skeleton Warrior | `skeleton_warrior` | Melee cleave | 1,500 | 6.3 | 18 | 4 | face it away, kick the execute |
| Grave Brute | `grave_brute` | Heavy melee AoE | 2,200 | 5.9 | 18 | 6 | leave slams and quakes, spread for charges |
| Crypt Ghoul | `crypt_ghoul` | Fast melee / bleed | 1,100 | 6.9 | 18 | 3 | burst it in its frenzy, heal the bleed |

Ranged monsters stop at their preferred range (18 / 16 / 24 m) and **back away** from anyone
who closes inside 7–9 m when they have nothing ready to cast. Melee monsters close to arm's
length and use their **charges and leaps while closing** — a charge is only ever aimed at
somebody at least 6–8 m away, never the one already in their face.

**Charges are casts on a player**, not ground attacks: a cast bar, no telegraph on the floor.
The monster runs at its target and *follows* them, hits them when it arrives, and slows them
**50 % for 1 s**. It is answered by being ready (a heal, a defensive), not by stepping aside.
Pounces keep a landing blast around the target.

Visuals: the enemy model, tinted per monster (bone-blue, violet, pale green, bone, grey-brown,
raw red) and scaled by height (Ghoul 1.6 m … Brute 2.7 m). Empowered forms are taller and glow.

---

## 1. Skeleton Bonecaster — ranged magical AoE

| Pri | Ability | Cast | CD | Damage | Shape / targeting | Answer |
|---|---|---|---|---|---|---|
| 1 | **Grave Explosion** | 1.0 s (kick) | 16 s | 95 Arcane | 5 m circle **following** a random non-tank, lands 4 s later | move away from the party with it — or kill the caster (its marks vanish) |
| 2 | **Bone Spear** | 2.0 s | 13 s | 110 | 34 × 2.6 m **line** through its target | sidestep |
| 3 | **Bone Barrage** | 1.4 s (kick) | 11 s | 55 each | 4 circles r 2.5 around the target, land 1.6 s after the cast | step out of the pattern |
| 4 | Bone Bolt | 1.3 s (kick) | 2.2 s | 34 Arcane | single | — |

**Empowered:** *Grave Chain Explosion* — marks up to **three** players; each detonation marks
everyone it caught with a new 2 s mark (once) — stacking is punished twice. *Massive Bone
Barrage* — **7** zones r 3.2 in two rings, **1.2 s** warning, 70 each, every 9 s.

## 2. Grave Acolyte — the dangerous caster

| Pri | Ability | Cast | CD | Damage | Shape / targeting | Answer |
|---|---|---|---|---|---|---|
| 1 | **Grave Drain** | 4.0 s **channel** (kick) | 18 s | 8 ticks, 10 → 34.5, **178** total | single | interrupt — the ticks grow |
| 2 | **Dark Pulse** | 3.0 s channel (kick) | 15 s | 4 × 28 | 5.5 m circle fixed where the target stood | leave it, or kick |
| 3 | **Corrupted Ground** | 1.2 s (kick) | 12 s | 20 + 26/s for 9 s | 4 m pool under a player | move out |
| 4 | Shadow Bolt | 1.5 s (kick) | 2.4 s | 38 Arcane | single | — |

**Empowered:** *Mass Grave Drain* — the drain on up to **three** players at once; one kick
stops all of it; every 16 s. *Spreading Corruption* — lands at 3.5 m and **grows** 1 m every
1.5 s to 8 m over 12 s: the safe floor shrinks.

## 3. Crypt Archer — long-range positional pressure

| Pri | Ability | Cast | CD | Damage | Shape / targeting | Answer |
|---|---|---|---|---|---|---|
| 1 | **Rain of Arrows** | 1.0 s (kick) | 14 s | 80 each | **2** circles r 3.5 per player (up to 3), land **2.2 s** after the cast | move |
| 2 | **Multi-Shot** | 1.2 s | 9 s | 45 each | one arrow at each of up to **3** players | heal it; kill the archer |
| 3 | **Crippling Arrow** | 0.8 s (kick) | 10 s | 30 + **45 % slow** 5 s | a random non-tank | dispel (curse) or expect the rain |
| 4 | Bone Arrow | 1.1 s (kick) | 2.0 s | 40 | single | — |

**Empowered:** *Deadly Rain* — **3** circles per player (9), **1.3 s** warning, 85. *Bone
Volley* — a 38 m line through each of three players (hits everyone behind them) and a **cursed
pool** r 3 where each was aimed (22/s, 7 s).

## 4. Skeleton Warrior — melee cleave

| Pri | Ability | Cast | CD | Damage | Shape / targeting | Answer |
|---|---|---|---|---|---|---|
| 1 | **Execution Strike** | **2.0 s** wind-up (kick) | 15 s | 130 | the tank | interrupt, or a defensive |
| 2 | **Bone Charge** | 1.0 s | 14 s | 55 + 1 s slow | cast on the farthest player (≥ 7 m); runs at them and hits them | be ready for it |
| 3 | **Bone Cleave** | 1.3 s | 8 s | 70 | **120°** 7 m cone in front of it | tank faces it away; nobody else in front |
| 4 | Bone Slash | instant | 1.6 s | 30 | tank | — |

**Empowered:** *Massacre* — **150°**, 9 m, 100. *Shattering Charge* — the charge hits its
target, then a **7 m shockwave** around the landing 0.9 s later (70 % damage): everybody near
the charged player has to leave.

## 5. Grave Brute — heavy melee AoE and burst

| Pri | Ability | Cast | CD | Damage | Shape / targeting | Answer |
|---|---|---|---|---|---|---|
| 1 | **Bone Quake** | 0.6 s | 18 s | 90 | **9 m ring** where it stood, lands **2.2 s** later | melee get out |
| 2 | **Grave Charge** | 1.3 s | 16 s | 80 + 1 s slow | cast on the farthest player (≥ 8 m); runs at them and hits them | heal the one it is cast on |
| 3 | **Ground Slam** | 1.5 s | 10 s | 60 | 7 m circle around itself | step out |
| 4 | Crushing Blow | instant | 3.2 s | 62 | tank | — |

**Empowered:** *Gravequake* — **6** circles r 4 scattered 8 m around it, 1.8 s. *Devastating
Charge* — the charge leaves a **burning trail** (a 2 m pool every 3 m, 30/s for 6 s).

## 6. Crypt Ghoul — fast melee, bleed, mobility

| Pri | Ability | Cast | CD | Damage | Shape / targeting | Answer |
|---|---|---|---|---|---|---|
| 1 | **Blood Frenzy** | instant | 22 s | — | itself: **+60 % attack speed**, 8 s | burst it now |
| 2 | **Pounce** | 0.7 s | 14 s | 50 + 1 s slow | leaps on a random player ≥ 6 m away; 4 m landing blast around them | spread |
| 3 | **Rending Claws** | instant | 5 s | 14 + **bleed** 6/s per stack, 8 s, up to 8 | tank | heal through / kill it |
| 4 | Ghoul Strike | instant | 1.0 s | 16 | tank | — |

A frenzy divides every cooldown and recovery it starts — it is an escalation, not a defence.

**Empowered:** *Rending Frenzy* — **2** stacks a hit, 9/s a stack, up to **10**. *Feast on the
Wounded* — an extra bite that is only on its list while its target bleeds. *Savage Pounce* — a
6 m landing explosion and an 8 m shockwave after it.

---

## Mechanisms (generic, reusable by monster #7)

Every mechanic above is a **column** on an ability row, not code per monster:

| Column | Used by |
|---|---|
| `shape: Line` | Bone Spear, Bone Volley |
| `targets`, `zones`, `spread` | Multi-Shot, Rain, Barrage, Gravequake, Mass Grave Drain |
| `delay`, `follow`, `chain` | Rain, Barrage, Quake; Grave Explosion's marks; chain marks |
| `effect: Channel { interval, ramp }` | Dark Pulse, Grave Drain |
| `effect: Charge { speed, impact, shockwave, trail, slow }` | Bone/Grave/Shattering/Devastating Charge, Pounce, Ghastly Waltz |
| `effect: Hazard { grow_to, radius }` | Corrupted Ground, Spreading Corruption, Bone Volley |
| `effect: Frenzy` | Blood Frenzy, Crimson Frenzy |
| `rider: Bleed / Slow` | Rending Claws, Garrote; Crippling Arrow, Gouging Riposte |
| `select: Farthest / Random`, `min_range`, `requires` | charges; marks and slows; Feast on the Wounded |
| `cue` | which of the three existing gather sounds a cast plays |

Delayed zones live in `engine-game/src/enemy/strike.rs`; they have their own telegraphs and
land on their own clock, so the caster keeps fighting. A mark's telegraph carries `follow` and
presentation attaches its ring to the victim.
