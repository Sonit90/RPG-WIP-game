# Reforge

A hub gamble: pay gold, re-roll an owned item's **base quality** (50-110) from a published table.
Part of P2 (`docs/P2_DESIGN.md` 2.9). There are no item tiers and no fusion.

## Rules

- The new roll **replaces** the old one. There is no keep-best, so a 108 can come back a 53.
- Sharpening and the broken flag are never touched. Effective quality stays the one formula,
  `base + sharpening` (`ItemProps::total_pct`).
- Any equipment, worn or in the bag, that is not broken. A broken item is refused: it keeps its
  penalty whatever it rolls (`eligible.allow_broken` in the data).
- Locked while the character is queued, in a party or in a dungeon (the usual hub lock).

## Data: `assets/catalog/economy.json` `reforge`

| Field | Meaning |
| --- | --- |
| `cost` | gold = `gold` + `per_item_level` x item level + `per_sharpening_level` x sharpening |
| `eligible.allow_broken` | whether a broken item may be reforged (false) |
| `bands` | `{min, max, chance}`, lowest first, uniform inside a band |

Validated at load (`Economy::validate_reforge`): bands inside 50..=110, `min <= max`, no overlap,
each chance in (0, 1], chances sum to 1, gold cost above zero.

Shipped table:

| New base quality | Chance |
| --- | --- |
| 50-59 | 25.0 % |
| 60-69 | 25.0 % |
| 70-79 | 22.0 % |
| 80-89 | 15.0 % |
| 90-99 | 8.0 % |
| 100-104 | 4.0 % |
| 105-109 | 0.9 % |
| 110 | 0.1 % |

Expected value 71.5. A drop rolls 72.8 on average at key level 1 (`loot.json` `quality`), so an
average item is a slightly losing bet; only a low roll or a lottery ticket (5 % to reach 100+)
makes it worth paying. Cost of a loot item (level 5, unsharpened) is 150 gold, a little over one run's
gold income, so it is a sink, not a free re-roll. The price (gold 50, 20 per item level, 20 per
sharpening level) is set so gold stays in use up to best-in-slot: `docs/balance/economy.md`.

## Code

- Rules: `engine-realm/src/reforge.rs` (`reforge_odds`, `roll`, `expected_value`, `quote`, `cost`,
  `reforge`). Pure: the caller supplies the random bits.
- Route: `POST /api/v1/characters/{id}/reforge` `{slot, request_id}` ->
  `{item, old_quality, new_quality, cost, gold_left, character}`. `slot` addresses the item as
  sharpening does (a worn slot or a bag cell). `request_id` (UUID) is required.
- Server: `realm_game::store::reforge_character`, one transaction: lock the character row,
  replay lookup in `game.hub_requests`, engagement check, gold check and deduction, roll from the
  operating system's random source (`OsRng`, nothing from the request), write, record the
  response, commit. A retry with the same `request_id` returns the stored answer: no second roll,
  no second charge. A refusal rolls everything back and records nothing.
- Audit: there is no gold ledger table; `game.hub_requests` keeps each reforge's response
  (item, old and new quality, cost) per request id.
- Client: `NetCommand::Reforge` (one UUID per user action, a connection-error retry reuses it),
  `NetEvent::Reforged`, and the Workshop's third mode: pick an item, see base / sharpening /
  effective quality, the whole odds table, the mean, the price and gold held, press Reforge, then
  Confirm. The result shows old -> new; a 100+ or a better roll plays an existing loot cue.
