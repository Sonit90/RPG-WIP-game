# Books

Two kinds of book drop from boss chests. Part of P2 (`docs/P2_DESIGN.md` 2.8; the gate on key level
comes from section 0: character level has no gameplay effect).

* **Talent books** (3 per class): reading one grants **+1 talent point**, permanently, once per book.
* **Enhancement books** (3 per class): each changes one signature ability and offers **three options**.
  One option per book is active at a time. It can be switched at the hub, never while the character is
  queued, in a party or in a dungeon.

There is one progression system. An enhancement option is a **hidden talent node** (tier 0, cost 0)
in the hero's talent table. Switching an option on is `TalentGrid::grant_index`, and the numbers are
resolved by the same `<Hero>TalentEffects::from_grid` as every bought talent. There is no second effect
path, and a hidden node can never be bought, saved or sent by a client: `TalentGrid::from_keys` (the
check the API and the realm run on a saved set) answers `UnknownTalent` for it.

## Data: `assets/catalog/talents.json`

Per hero, next to `talents`:

```json
"talents": [ ... visible talents ...,
  { "key": "bk_wide_quake", "name": "WIDE QUAKE", "tier": 0, "cost": 0,
    "description": "WAR STOMP RADIUS +{range_bonus%}%, DAMAGE -{damage_penalty%}%",
    "values": { "range_bonus": 0.5, "damage_penalty": 0.1 } } ],
"books": [ { "id": "codex_of_iron_will", "name": "Codex of Iron Will", "min_key_level": 8 } ],
"enhancement_books": [
  { "id": "manual_of_tremors", "name": "Manual of Tremors", "ability": "war_stomp", "min_key_level": 6,
    "options": [ { "id": "wide_quake", "name": "WIDE QUAKE", "description": "...", "node": "bk_wide_quake" } ] } ]
```

The hidden rows follow every visible talent, in book order, and match the code's defaults in
`talent/{warrior,druid}.rs` and `firemage/talent.rs` row for row (the same rule as every talent). An
option names its node by key; the option's `description` is the node's, with the numbers filled in.

Validated at load (`engine_game::talent::books::validate_hero`, `engine_realm::talents::parse`), with
tests in both crates: hidden rows after visible ones; unique book ids (also across heroes, because a
book id is its item key); exactly three options per enhancement book with unique ids; every option's node
exists, is hidden and serves one option; every hidden node belongs to an option; the book's `ability` is
one of the class's own; `min_key_level` above zero.

## The books

Gates: talent books drop from key level 8, enhancement books from key level 6 (section 0 of the design).

| Class | Talent books | Enhancement book (ability) | Options |
|---|---|---|---|
| Warrior | Codex of Iron Will, Codex of the Shield Wall, Codex of the Long War | Manual of Tremors (War Stomp) | Wide Quake: radius +50 %, damage -10 % / Crushing Quake: damage +40 %, cooldown +3 s / Rending Quake: +2 Sundered stacks, damage -15 % |
| | | Manual of the Charge (Shield Charge) | Twin Charge: 2 charges, impact -15 % / Triple Charge: 3 charges, impact -30 % / Battering Charge: impact +30 %, one charge |
| | | Manual of the Headsman (Colossus Strike) | Headsman's Mark: +60 % below 25 % health / Reaper's Reach: +20 % below 60 % health / Blood Tithe: the bleed pays +2 Valor |
| Druid | Lore of the Grove, Lore of the Deep Roots, Lore of the Waking Dream | Verses of Sunfire (Sunfire) | Wide Bloom: radius +60 %, ticks -15 % / Searing Bloom: ticks +50 %, radius -30 % / Balanced Bloom: radius +25 %, ticks +10 % |
| | | Verses of Mending (Swiftmend) | Quick Mend: cooldown -4 s / Astral Mend: refunds 20 Astral Power / Measured Mend: -2 s and 10 Astral Power |
| | | Verses of Stillness (Tranquility) | Gentle Peace: mana -40 %, healing -10 % / Swift Peace: cooldown -30 s, healing -15 % / Deep Peace: healing +25 %, mana +30 % |
| Mage | Tome of Cinders, Tome of the Ember Court, Tome of the Pale Flame | Grimoire of the Pyre (Pyroblast) | Twin Pyre: +1 charge / Ricochet Pyre: bounces up to 3 times / Searing Pyre: a critical Pyroblast cuts Flame Lance a further 0.5 s |
| | | Grimoire of Falling Stars (Meteor) | Impact Crater: crit damage +50 %, a critical Meteor cuts Pyroblast 2 s / Scorched Earth: leaves burning ground, impact -15 % / Falling Star: critical Ignite ticks cut Meteor |
| | | Grimoire of Slow Embers (Ignite) | Slow Burn: Ignite +4 s, damage -5 % / Hot Core: Detonate +2 % per stack / Flashpoint: Detonate cuts area cooldowns |

Options compose with bought talents (scales multiply, cooldowns and counts add, the Warrior's execute
threshold takes the higher). Nothing is hardcoded in Rust except what a node can do, which is
existing talent effect fields; no new effect field was added.

## Items

A book is an item of family `Book` (`ItemFamily::Book`, not in `ItemFamily::ALL`, which lists the equipment
families; it has no slot and never stacks). Its item key is the book id. Books are registered after the
`loot.json` items (`LootCatalog::items_for_registration`), so appending loot rows does not touch them.
The icon is the trinket's until a book icon is drawn.

## Drops: `assets/catalog/loot.json` `book_drops`

```json
"book_drops": { "default": { "talent_point": 0.06, "enhancement": 0.05 },
                "bosses": { "lord_ashgrave": { "talent_point": 0.04, "enhancement": 0.03 } } }
```

Per player's share of a boss chest, after the equipment rolls (`engine_game::loot::roll_book`): a talent
book is tried first, then an enhancement book; each needs a book of the player's own class whose
`min_key_level` is at most the run's key level, and the book is picked uniformly among those. At most one
book per share. The draw is three numbers from the loot generator, only when the rules carry `book_drops`,
so a ruleset without it rolls exactly what it always rolled. The same seed rolls the same chest.
Delivery is the existing path: chest `LootAwarded` -> realm loot outbox -> API `apply_loot` (idempotent
by loot id).

**Where the key-level gate applies: on the drop, not on use.** A held book can always be read: a key can be
downgraded or cleansed and must not strand an item already earned. The same book can drop twice; a
duplicate is refused on use (409) and the item stays in the bag.

## Persistence and routes

`characters.books JSONB` (`migrations/0014_books.sql`), default `{}`:
`{"unlocked": [book ids], "active": {book id: option id}}`. Stale ids (a retired book) count for nothing.

* `POST /api/v1/characters/{id}/books/use` `{request_id, slot}` -> `{book, character}`. `slot` is a bag
  cell. One transaction under the character row lock: the item is removed and the book learned together.
  Refused with nothing changed: not a book / empty cell (400), another class's book (409), already learned
  (409, the item stays). Two uses of two copies at once: one learns, the other is refused. Allowed while
  engaged (it changes no combat number until the next queue).
* `PUT /api/v1/characters/{id}/books/active` `{request_id, book_id, option_id}` -> character. Refused
  (409) while the character is queued, in a party or in a dungeon (`ensure_unlocked`, asked with the row
  locked, before the write); refused when the book is not learned (409), is a talent book or the option is
  unknown (400).
* Both take a required `request_id` (`game.hub_requests`): a replay returns the stored answer and
  changes nothing; the same id for another action is a conflict.

`CharacterDetail.books` carries the state to the client. Nothing about the loadout changed, so the
loadout version is unchanged (`CURRENT_VERSION` stays); an old save has no books because the state is its
own column and defaults to empty.

## Points

Total talent points = starting + earned + learned talent books. `characters::earned_points` adds the books
and is the one number the talent save (`PUT /talents`), the queue snapshot and the detail DTO check, so the
server still rejects a set that costs more than the character has. An enhancement book adds no point.

## The realm

The queue snapshot's `talents` list is the bought talents plus the hidden nodes of the active options
(`characters::snapshot_talents`); `talent_points` is the whole budget. The realm builds the grid with
`TalentGrid::from_keys_with_nodes`: nodes are free and switched on, the visible keys are checked against
the budget exactly as before, and an unknown key or an over-budget set still drops every talent. No realm
module schema changed. Because the active options ride in the snapshot, they cannot change during a run.

## Client

* `NetCommand::UseBook` and `SetActiveBook` (one UUID per user action, reused on a connection retry),
  `NetEvent::BookRead`; `Online::request_use_book`, `request_book_option`.
* Bag: right-click a book to read it (`CharacterMenuOutcome::Use`); an already-learned book does nothing.
  The item tooltip shows the kind, what reading it gives, the options of an enhancement book, the class
  it is written for and "Already learned".
* Talent screen: a Books/Talents toggle beside Reset. The Books page lists the three talent books (learned
  or "boss drop, key level N+") and, per enhancement book, its three options with the active one outlined
  and marked ACTIVE; click to switch. While locked the existing banner says why nothing can change. The
  point total at the top already includes learned talent books (the server sends it).
* Audio: existing cues only (`UiSelect` on a click, `LootRare` when a book is learned).

## Simulator

`engine-sim` has a `p2_books` tier: `p2_gems` gear (all other P2 power), the one talent the three talent books pay for, and the
best option of each enhancement book (`books::shipped_plan`). Section 8 of `docs/balance/sim-report.md`
measures every option alone against `p2_gems` and the tier's gain per class. Options that only move heals
or defence, and the Warrior's execute (the dummies never get low), do not show in a damage number.
