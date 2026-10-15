# Economy: currencies, monster drops and enhancement

Everything here is decided by the realm or the persistence API; the client only asks and shows.

## Currencies

Gold and aether shards are resources in the character's loadout (`economy.json` names them, like
`crypt_dust`). They come from monsters: each kill rolls, per player present, a gold amount and a
small chance of a shard (`loot.json`: `gold`, `currency`). The boss pays `boss_amount` gold. The
fight emits `MobCurrency` events; the realm tallies them per player in `run_loot` and the run's
result carries the tally (`looted`), paid on success and on failure, once (see
[PERSISTENCE.md](PERSISTENCE.md)). The hub's resource bar always lists both.

## Monster drops

A kill also rolls a small chance (`mob_table.equipment_chance`) of one equipment piece per player,
with a quality rolled like the chest's. It is mailed (`dungeon_drop`, sender "Dungeon") at once,
under `"<instance>:<account>:mob:<n>"`; the toast says "RARE DROP - SENT TO YOUR MAIL" and a chime
plays (`LootDrop`, `LootEpic` for an epic). The completion panel counts the drops.

## Reward bonus

A key modifier will set a bonus in per cent. It scales currency chances and gold, gives each chest
share a chance of one more item, and scales the run's resources. The module reads it from one
function (`combat::reward_bonus`, zero until the key task fills it) and the summary shows it.

## Quality in the interface

Every item tooltip (character sheet, bag, mailbox, loot toast) prints the stats of the actual copy
(`ItemStack::stats`) and: `Base Quality: 106%`, `Enhancement: +7%`, `Total: 113%`, or for a broken
item `BROKEN (-15%) - cannot be enhanced` in red. The Dungeon Finder card states the epic chance
and the chance of a 100%+ item at the character's key level, from the same functions the server
rolls with (`epic_chance_at`, `quality_odds`).

## Enhancement

`POST /api/v1/characters/{id}/enhance` with `{ "slot": { "kind": "equipment", "slot": "ring1" } }`
(or `{ "kind": "inventory", "index": 3 }`) answers `{ "outcome": "success" | "broken",
"new_level": n, "character": { ... } }`.

* The server draws the roll from the operating system's secure source; the request carries no
  roll and cannot influence it.
* One locked read-modify-write of the character row (`engine_realm::enhance::enhance`): the price
  is paid on success and on a break, the item changes, the row is saved. Two requests cannot both
  spend the same purse.
* Refused: an empty slot or an item that is not equipment (400); a broken item, an item at the
  maximum level, an unaffordable price (409); a character that is queued, grouped or in a dungeon
  (409, the same lock as equipment). Rate limited like other character writes.
* Each success adds 1% of the base stats (up to `max_level`). The first levels cannot break; from
  the fifth the break chance in `economy.json` applies. A break removes the enhancement and leaves
  85% of the rolled quality; a broken item cannot be enhanced again.

The Workshop screen has an Enhance panel: worn items and bag items, the item's state, the next
level, the price with what is owned, and the break chance (red whenever it is above zero), with an
Enhance button; the result shows as a toast ("Success: +8" or "BROKEN") and the character is
refreshed from the answer.
