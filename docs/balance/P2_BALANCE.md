# P2 economy and power-scaling review

Branch `p2-balance`. Everything below is reproducible with the commands at the end; the numbers
come from `docs/balance/sim-report.md` (sections 7, 8, 10, 11) and `docs/balance/economy.md`.

## Findings

1. **Epics were far too rare for the content P2 added.** With 2.5 % at key 1 and 8 % at most, a
   player farming the Crypt needed about 490 runs for the Gravewarden set (target 40-120) and about
   560 for the 13 best-in-slot pieces. Set pieces also shared a table with the epics of
   every other set.
2. **Gold is a thin currency and the first draft of the sinks left it piling up.** Income is a flat
   122 gold per run (36 kills at 1-3, boss 50); it does not grow with the key level. The sinks are gem
   upgrades (1165 gold per max gem), taking a gem out (5-150) and the reforge. Sharpening is **not** a
   gold sink: it costs Crypt Dust. With the original reforge price a rational player held 20-26 runs of
   income unspent at run 300 (once the drop rates were fixed).
3. **Crypt Dust is not a currency that matters.** Safe sharpening (+1 to +4) costs 115 dust per
   piece, 1 495 for all thirteen; a player earns 16 000 by run 300 (11 times that) and 52 000 by run
   900. The levels past +4 break the item 95-99.5 % of the time, so nobody rationally buys them.
   The real sharpening ceiling is +4 by crafting, plus whatever pre-sharpened drops bring.
4. **The Warrior set was worth +2.4 % DPS over `p2_max`** because the Gravewarden pieces carry
   armour and stamina instead of the Haste, Critical Strike and Strength they displace
   (104 / 37 / 37 against 108 / 44 / 47), so the 4- and 5-piece bonuses mostly paid back the stat
   loss.
5. **The Druid set lowered survivability** (1304 against 1328 for `p2_max`): the five Tidebloom
   pieces carry 74 Stamina against 83 for the pieces they replace, and the set's own Stamina did not
   cover the difference.
6. **The Mage set was worth +12 %**, above the 6-10 % brief.
7. **Warrior `p2_books` survivability (2821) is 0.6 % below `p2_gems` (2839).** Cause measured in
   section 8: the starred option CRUSHING QUAKE (War Stomp +40 % damage, cooldown +3 s) alone costs
   1.4 % effective health because the Warrior stomps 10 times a minute instead of 15, which lowers
   the uptime of what War Stomp feeds (Valor, Battle Fervor). UNBROKEN MOMENTUM (+0.6 %) gives some
   back. It is a declared trade, not a bug: the monotonicity check exempts books for survivability
   by design, and the option stays in the plan because it is the best single-target damage choice
   for the stomp book.
8. **The endgame stack is balanced across classes**: `p2_books` over `high` is 1.47x (Warrior),
   1.42x (Druid) and 1.48x (Mage) in DPS, within 1.9 % of the mean.
9. **Realistic best-in-slot is below the sim's `p2_*` tiers.** Those tiers wear quality 110 and +10
   on every piece (a stat multiplier of 1.20). A player's reachable gear is quality 90-100 from
   drops and reforging, +4 from Dust, i.e. about 0.99: between `medium` (0.88) and `high` (1.06).
   Quality 100 on all thirteen pieces is never reached in 900 runs (a reforge gives 100+ 5 % of the
   time and replaces the roll), and the jackpot 110 is a lottery ticket. Read the `p2_*` rows as
   best case.

## Tuning changes (all data)

`assets/catalog/sets.json`

| Set | Bonus | Before | After |
|---|---|---|---|
| Gravewarden (Warrior) | 4 pieces | Stamina 8; Colossus Strike damage +10 %, cooldown -10 % | Stamina 8, Haste 7, Critical Strike 9; Colossus Strike damage **+12 %**, cooldown -10 % |
| | 5 pieces | War Stomp +10 %; Shield Charge +10 %, cooldown -10 % | the same, plus Strength **+4** |
| Tidebloom (Druid) | 2 pieces | Haste 6, Stamina 6 | Haste 6, Stamina **12** |
| | 4 pieces | Spirit 6; Wrath +8 % | Spirit 6, Stamina **6**; Wrath +8 % |
| Emberwake (Mage) | 4 pieces | Haste 6; Firebolt and Flame Lance +12 % | Haste **2**; Firebolt and Flame Lance **+10 %** |
| | 5 pieces | Detonate +15 %, Flame Lance cooldown -20 %, resource +15 % | Detonate **+12 %**, Flame Lance cooldown **-15 %**, resource +15 % |

`assets/catalog/loot.json`

| What | Before | After |
|---|---|---|
| Epic chance per equipment roll | 2.5 % at key 1, +0.2 % per key level, at most 8 % | **6 %** at key 1, **+0.5 %** per level, at most **20 %** (15.5 % at key 20) |
| Major set piece weight in its boss table | 12 | **24** (Gravewarden, Emberwake, Tidebloom) |
| Emberwake Regalia source | Lord Ashgrave (Crypt, with Gravewarden) | **Foundry Overseer** (Bone Foundry) |
| Candlelit Vows, Ossuary Wardplate source | Lord Ashgrave | **Ashen Pontiff** (they competed with both major sets in the Crypt table) |

`assets/catalog/economy.json`

| What | Before | After |
|---|---|---|
| Reforge price | 50 + 10 per item level + 10 per sharpening level (a level-12 piece at +4: 210) | 50 + **20** per item level + **20** per sharpening level (**370**) |

Tests that pin the old shipped numbers were updated (`engine-realm` epic rates and the Emberwake
haste, `engine-presentation` set tooltip). Nothing else in Rust changed apart from the simulators.

## Economy (`docs/balance/economy.md`, 600 players per class, 900 runs)

Player: farms the dungeon with the most still-missing target pieces and books, sockets gems, sharpens
to +4 and reforges to 90+ with the gold left after gem upgrades. Keys climb +1 per two runs to key 20,
then +1 per five to key 24 (the Warrior's fully-geared wall, below).

| Per run | Key 1-9 | Key 10-19 | Key 20+ |
|---|---:|---:|---:|
| Gold | 122 | 122 | 122 |
| Crypt Dust | 18-20 | 38-39 | 59 |
| Equipment drops | 2.1 | 2.1 | 2.3 |
| Gems | 1.4 | 1.8 | 2.2 |

| Gold over 300 runs (Warrior) | |
|---|---:|
| Income | 36 598 |
| Gem upgrades | 21 310 |
| Taking gems out | 480 |
| Reforge (37 rolls) | 13 739 |
| Unspent | 1 070 (**8.8 runs of income**; Druid 9.7, Mage 8.2) |

Gold never gates the gem chain (a gem pair waited on gold in 3 of 300 runs). The system that is
gold-limited is quality.

| Runs to ... (median, 10th-90th percentile) | Warrior | Druid | Mage | Target |
|---|---|---|---|---|
| first max-level gem, Ruby | 88 (52-159) | 95 (52-174) | 92 (52-173) | ~100 (60-150) |
| first max-level gem, any kind | 49 | 50 | 49 | |
| full major set | 93 (59-152) | 71 (44-120) | 75 (44-145) | 40-120 |
| all 6 books | 153 (90-273) | 142 (77-255) | 148 (78-284) | 80-200 |
| all 13 best-in-slot pieces | 249 | 247 | 266 | |
| max gems in every socket (14) | 260 | 258 | 270 | |
| sharpening ceiling (+4) on all 13, dust only | 47 | 46 | 45 | |
| quality 90+ on all 13 (gold-bound) | 420 | 400 | 438 | |
| quality 100+ on all 13 | median player: not in 900 | same | same | |

Total gold demand to max every gem and reach quality 90+ everywhere is about 53 000, 438 runs of
income, so gold stays useful until the last system finishes, and no system is blocked by it before
that. If the key curve is stretched (the player times fewer keys), the Warrior's set stays inside its
band at half speed (109 runs) and drifts to 139 at a quarter of the speed; books slow by up to 1.2x
and gear by up to 1.3x.

## Power scaling (`docs/balance/sim-report.md` section 7, `Default` build, single target 60 s)

| Class | Tier | DPS before | DPS after | Survivability before | Survivability after |
|---|---|---:|---:|---:|---:|
| Warrior | `p2_max` | 870 | 870 | 2324 | 2324 |
| | `p2_set` | 891 (+2.4 %) | **954 (+9.7 %)** | 2743 | 2746 |
| | `p2_gems` | 975 | 1042 | 2835 | 2839 |
| | `p2_books` | 1062 | 1132 | 2817 | 2821 |
| Druid | `p2_max` | 804 | 804 | 1328 | 1328 |
| | `p2_set` | 878 (+9.2 %) | 878 (+9.2 %) | **1304 (violation)** | **1424** |
| | `p2_gems` | 963 | 963 | 1354 | 1474 |
| | `p2_books` | 1014 | 1014 | 1354 | 1474 |
| Mage | `p2_max` | 1784 | 1784 | 1328 | 1328 |
| | `p2_set` | 2001 (+12.2 %) | **1954 (+9.5 %)** | 1336 | 1336 |
| | `p2_gems` | 2191 | 2139 | 1386 | 1386 |
| | `p2_books` | 2388 | 2333 | 1386 | 1386 |

Gems add 9.2-9.7 % and books 5.4-9.1 % on top (the Druid's heal books carry no damage). The
monotonicity check passes with no violations.

Relative scaling, `high` to `p2_books`: Warrior 1.47x, Druid 1.42x, Mage 1.48x. From `none`: 5.4x,
5.4x, 5.9x.

## The eternal wall (`sim-report.md` section 11)

Monster growth from `ModifierCatalog::row_for` (health x / damage x): key 12 2.02 / 1.51, key 20
2.50 / 1.75, key 30 3.10 / 2.05, key 50 4.30 / 2.65, key 99 7.18 / 4.09. Modifier slots cap at 6 from
key 20. The player gains 1.47x DPS and 1.31x effective health (Warrior) from `high` to `p2_books`,
less for the casters (1.42-1.48x and 1.13-1.20x).

Reference: `high` gear on a key 10. The **combined wall** (the key where the fight is as hard as that
reference) of a fully geared player is key **24** for the Warrior and key **20** for the Druid and
Mage; the casters meet the defence wall first (Mage 16, Druid 18, Warrior 24), the offence wall at
22-24. With the realistic gear (finding 9) the wall is a few keys lower. Past it, gear no longer pays
for the next level and progress is skill, group and modifier play. The economy simulation caps the
player at key 24.

## Remaining risks

- **Crypt Dust has no demand** (finding 3). Suggested fix, not applied because backend and realm tests
  pin the current prices and rewards: cut the `dungeons.json` dust rewards to about a tenth, or raise
  the four safe sharpening prices tenfold and make the unsafe levels cost embers only.
- **Gold is idle after best-in-slot**: 171 runs of income are unspent at run 900 and growing by 122 a
  run. The key cleanse (100 gold) is the only other use. A late sink (key reroll, cosmetic, repair)
  or a gold reward that falls with the key level is needed before long-term players arrive.
- **The sharpening gamble is a trap by the numbers**: +5 and up break 95-99.5 % of the time. If the
  design intends players to chase +10, the break table needs rethinking (P1 rules were kept as the
  brief asked).
- **Quality 100+ is a pure lottery** (reforge 5 %, replaces the roll). Quality 90+ everywhere is the
  achievable goal, gold-bound at about 420 runs.
- **Dungeon choice is greedy**: the sim player picks the dungeon for the missing piece; a player who
  rotates blindly takes longer for a set (a Crypt visit is one in four). The Ashen Reliquary has no boss table and drops
  what the Crypt drops; it is not modelled as a separate site.
- **Key curve and gear fitness are assumptions**: the curve reaches key 20 at run 39, long before the
  gear to time it. The sensitivity table in `economy.md` shows the milestones tolerate it.
- **Mage Build B gains only about 2 % from its set** (Build A 13 %); the set is tuned on `Default`.
- **Aether Shards, key cleansing and downgrades, experience and talent points are not modelled.**
- Numbers use dummies that do not hit back; survivability is the proxy of section 7, not a fight.

## Regenerating

```text
cargo run --release -p engine-sim -- --iterations 20 --out docs/balance/sim-report.md
cargo run --release -p engine-sim -- --loot-sim --out docs/balance/loot-sharpening.md
cargo run --release -p engine-sim -- --gem-sim --samples 2000 --out docs/balance/gem-economy.md
cargo run --release -p engine-sim -- --economy-sim --samples 600 --out docs/balance/economy.md
```

`economy_sim` has tests holding the max-gem, set, book and unspent-gold bands (seeded, 150 players
per class in release, 40 in debug with bands 25 % wider).
