# Gems

Socketable stat stones. Part of P2 (`docs/P2_DESIGN.md` 2.7). Data lives in
`assets/catalog/gems.json`; sockets are a field of the item rows in `assets/catalog/loot.json`.

## Rules

- Equipment has **0-2 sockets** (`loot.json` item row `"sockets"`, default 0). Every epic has 1;
  six have 2 (`crown_of_the_hollow_steward`, `stewards_funerary_raiment`,
  `overseers_ironclad_helm`, `furnace_wardens_cuirass`, `gravewarden_hauberk`, `tidebloom_robe`).
  Rares have none.
- A gem is a stackable inventory item `gem_<kind>_<level>` (family `Gem`, 99 per cell), 7 kinds
  x 5 levels: Ruby (main stat), Amber (stamina), Topaz (critical strike), Emerald (haste),
  Amethyst (versatility), Onyx (armor), Opal (spirit). A gem's value is the stat on its item row.
- A socketed gem's stat is added to the wearer **as written**: not scaled by the piece's quality or
  sharpening (like a set bonus). `CharacterSheet::gear_stats` adds
  `ItemProps::gem_stats` for every worn piece; a `main_stat` gem becomes the hero's own stat with
  the piece's. The server (`sheet_of` from the stored loadout), the client and the simulator all
  take this one path.
- A forged socket (gems on an item with no socket, or more than it has) grants nothing.

## Hub services (locked while queued, in a party or in a dungeon)

| Route | Does | Price |
| --- | --- | --- |
| `POST /characters/{id}/gems/socket` `{request_id, item_slot, socket, gem_slot}` | one gem from bag cell `gem_slot` fills the empty `socket` of the item (worn slot or bag cell) | free |
| `POST /characters/{id}/gems/unsocket` `{request_id, item_slot, socket}` | the gem returns to the bag (joins a stack when it can) | gold by gem level (`unsocket.gold`) |
| `POST /characters/{id}/gems/upgrade` `{request_id, gem_slot}` | consume `consume` (2) gems of one kind and level plus gold; success gives one gem a level higher, failure `keep_one` hands one back | gold per step, success chance per step |

Each is one transaction (`realm_game::store::socket_gem`, `unsocket_gem`, `upgrade_gem`): lock the
character row, replay lookup in `game.hub_requests` under `request_id`, engagement check, rules,
write, record the response, commit. A retry with the same `request_id` returns the first answer and
changes nothing (no second roll, no second charge). A refusal rolls everything back and records
nothing. The upgrade rolls from the operating system's random source; nothing in the request
influences it. An upgrade needs room in the bag for the next-level gem **whatever the roll**, so a
refusal never leaks the outcome of a try. Refusals: occupied or missing socket, item without
sockets, not a gem, too few gems, gold short, bag full, max level.

## Data: `gems.json`

| Field | Meaning |
| --- | --- |
| `max_level`, `level_names` | 5 levels: Chipped, Flawed, Cut, Flawless, Perfect |
| `kinds[]` | `{id, name, stat, values[max_level]}`, values strictly increasing |
| `drops` | per source (`mob`, `elite`, `boss`, `chest`): `chance` per player per event and `level_weights`; `key_bonus_per_level` / `max_multiplier` raise the chance with the key level |
| `upgrade` | `{consume, on_fail: "keep_one", steps[max_level-1]: {gold, success_chance}}`, `steps[0]` is level 1 to 2 |
| `unsocket.gold` | gold per gem level |

Validated at load (`GemCatalog::parse`, tested): values increase, chances in (0, 1], the upgrade
table has `max_level - 1` rows and the unsocket table `max_level`, stats exist, sources are known
and unique, level weights are non-negative with a positive sum. Gem item keys use the `gem_` prefix,
which `loot.json` rows may not.

Shipped values (stat per level 1..5): main stat / crit / haste / versatility / spirit
0.2, 0.5, 0.8, 1, 1.5; stamina 1, 1.5, 2.5, 3.5, 5; armor 1.5, 2.5, 4, 5.5, 8. They are small on
purpose: a full set of max gems (about 20 sockets' worth across a character's epics) must be worth
about +9-10 % damage, not a second item slot's worth. Upgrade: 2 gems; gold 10 / 25 / 60 /
150 at 90 / 75 / 60 / 45 %. Unsocket: 5 / 12 / 30 / 70 / 150 gold. Drops (key 1): mob 0.4 %, elite
5 %, boss 50 %, chest 25 %, with higher levels possible from elites (20 % level 2), bosses (30 % /
10 % level 2 / 3) and chests; +4 % chance per key level above 1, at most x2.

## Persistence

`Loadout` version 3 adds `gems` (the gem item keys, `""` for an empty socket, omitted when none) to
worn, carried and mailed items. A version 2 row reads with empty sockets and everything else
unchanged (`loadout::migrate`, tested over a captured v2 row). Gems on the wire/DB are keys, never
the runtime item ids, so a catalog change cannot renumber anyone's gems. Drops travel the existing
pipeline unchanged: mobs, elites and bosses drop a gem as a `MobLoot` (a `pending_loot` row,
delivered to the mailbox by the API's idempotent `apply_loot`); a chest may hold one more stack.

## Economy

`cargo run --release -p engine-sim -- --gem-sim --samples 2000 --out docs/balance/gem-economy.md`
simulates the farm (30 mobs, 6 elites, a boss and a chest per run, half the gold income spent on
gems) and the upgrade chain. One max-level gem of a chosen kind is worth about 42 level-1 gems and
1165 gold of upgrades; the median is about 108 runs at key level 10 (85 at key 20, 153 at key 1,
with a wide spread because the kind is a lottery). `gem_sim::tests` hold the median at key 10 to
60-150.

## Power

The `p2_gems` tier in the balance simulator is `p2_set` with a level-5 Ruby (main stat) in the first
socket of every piece and a level-5 Amber (stamina) in a second one. `docs/balance/sim-report.md`
section 7 (20 iterations): single-target 60 s DPS Warrior 891 -> 975 (+9.4 %), Druid 878 -> 963
(+9.7 %), Mage 2001 -> 2191 (+9.5 %); survivability rises a little too (stamina). Section 8 of the
same report and `docs/balance/gem-economy.md` hold the economy.

## Client

Workshop mode **Gems** next to Sharpen and Reforge: pick a worn item or a bag item to see its
sockets (a hollow `( )` marker when empty, `(*)` with the gem and its stat when filled), pick a gem
in the bag to socket it, take a gem out (price shown on the button), or upgrade the picked gem
(consume count, gold, success chance and the failure rule are shown; Upgrade asks to confirm).
Item tooltips list the sockets; a gem has its own tooltip. `NetCommand::SocketGem` /
`UnsocketGem` / `UpgradeGem` each carry one UUID per user action, reused on a connection retry.
Sounds are existing cues only (UI select/click, the rare loot chime on a successful upgrade).
Gems have no icon of their own yet: they borrow the trinket's.
