# Equipment sets

Pieces that are worth more together. Data lives in `assets/catalog/sets.json`; pieces are items
in `assets/catalog/loot.json`.

## Format

```json
{ "id": "emberwake_regalia", "name": "Emberwake Regalia",
  "pieces": ["emberwake_hood", "emberwake_mantle", "..."],
  "bonuses": [
    { "count": 2, "stats": { "critical_strike": 6, "haste": 4 } },
    { "count": 4, "effects": [ { "kind": "damage_pct", "ability": "firebolt", "value": 0.12 } ] }
  ] }
```

Each piece item carries `"set": "<id>"` in `loot.json`. A *minor* set has 2 pieces, a *major*
set 4 or 5 with tiers such as 2 / 4 / 5.

* `stats` use the item stat keys. `main_stat` becomes the wearer's own main stat. They are added
  as written: **not** scaled by an item's quality or sharpening.
* `effects` kinds (values are fractions, `0.1` = 10 %):
  * `damage_pct{ability}` - that ability deals more damage (direct and periodic).
  * `cooldown_pct{ability}` - that ability's cooldown is shorter (at most 50 % in total).
  * `resource_gen_pct` - casts pay more into the class resource: Heat (FireMage) and Astral Power
    (Druid). It has no effect on the Warrior, whose Valor counts hits.
* `ability` is the stable config name (`firebolt`, `colossus_strike`, `wrath`, ...).

## Rules

* Pieces are counted as *distinct items*: two copies of one ring count once.
* A tier is on when the count reaches `count`; tiers ascend and none exceeds the piece count.
* Nothing is stored: sets are derived from what is worn
  (`engine_game::character::active_sets`). The server and the client call the same function; the
  client only displays. Stats ride `CharacterSheet::gear_stats`; effects are applied where damage
  is dealt, in the ability book (cooldowns) and where a class gains its resource.
* The catalog is checked at load in both directions (`engine-realm/src/sets.rs`): every piece is
  an item that names the set, every item that names a set is listed by it, counts are reachable
  (a set cannot need two helms), stats / abilities / effect kinds exist.

## Adding a set

1. Add the piece rows at the **end** of `loot.json` `items` (ids are positions: never reorder)
   with `"set": "<id>"`, and put them in the boss tables they should drop from.
2. Add the set to `sets.json`.
3. `cargo test -p engine-realm` (catalog checks, every piece can drop) and
   `cargo test -p engine-game --test sets`.
4. Re-measure: `cargo run --release -p engine-sim -- --iterations 20 --out docs/balance/sim-report.md`.
   The `p2_set` tier wears the class's major set (the registered set of 4+ pieces whose effects
   name that hero's abilities) over `p2_max`; set pieces are never picked by the plain tiers.
   Aim for the 4- and 5-piece effects to be worth about +6-10 % DPS.
