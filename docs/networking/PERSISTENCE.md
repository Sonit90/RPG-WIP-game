# Persistence

Long-lived data lives in the realm API's Postgres (the same database as accounts, schema
`game`). The realm holds snapshots for the length of a run and hands back results. No new
database technology was added: the auth service already ran Postgres 16.

## Schema (`backend/migrations/0003_game_schema.sql`)

| Table | Holds |
| --- | --- |
| `game.characters` | id, account, catalog definition, name, level, experience, **loadout** (JSONB: worn items by slot key, bag cells by index — the same vocabulary `character.cfg` uses), **talents** (JSONB keys). One per definition per account. |
| `game.dungeon_completions` | per character and dungeon: count, best time, last completion |
| `game.dungeon_results` | every result the realm reported, keyed by the realm's idempotency key |
| `game.loot_deliveries` | every boss-chest item the realm reported (`backend/migrations/0004_loot_deliveries.sql`), keyed by its idempotency key, with where it went |
| `game.account_settings` | the account settings document, with an optimistic version |

The loadout JSONB also holds the character's **mail** (`mail`: letters with an id, the item's
stable key, sender, subject, reason `bag_full` / `unclaimed`, unix `received_at` and a `read`
flag). Mail is part of the loadout rather than a table of its own, so taking a letter is one
locked read-modify-write of the same row that holds the bag, and a letter can never be both
taken and still waiting.

**Derived, never stored:** gear score (10 × summed item level of what is worn) and stats
(definition + level + gear) are computed with `engine-realm` from the loadout, so they cannot
disagree with it.

## Characters

* `GET /api/v1/characters` creates one of each starter definition the account lacks (so the
  three existing heroes are there on first sign-in) and lists them with server-computed gear
  score.
* `GET /api/v1/characters/{id}` — loadout, bag, talents, stats, completions.
* `POST /api/v1/characters/{id}/equipment/move` — validated with the gameplay rules
  (`CharacterSheet::move_item`): the slot exists, the item fits, the swap is legal both ways.
* `POST /api/v1/characters/{id}/mail/{mail_id}/take` — moves the letter's item into the first
  free bag cell and removes the letter (`take_mail`); a full bag is a 409 `Your bag is full.`
  and the letter stays.
* `POST /api/v1/characters/{id}/mail/{mail_id}/read` — marks the letter read.
  Both answer with the character in full, which the client applies exactly as it applies a
  move, so the mailbox window redraws from the server's copy.
* `POST /api/v1/characters/{id}/enhance` - see [ECONOMY.md](ECONOMY.md). Locked while engaged, like equipment.
* `PUT /api/v1/characters/{id}/talents` — every key must be a talent the character's hero
  has and the set must cost no more than the character's points (400 / 409), and the
  character must not be engaged (409, see below). Answers with the character.

**Talent points** are `starting_points` (`assets/catalog/talents.json`, 5) plus
`game.characters.talent_points_earned` (migration `0007_talent_points.sql`). The realm computes
what a run grants (`engine_realm::rewards`: 1 per success, +1 at key level 3+) and carries it on
`pending_result`; `apply_result` adds it inside the transaction that records the result id, so a
re-delivered result grants nothing twice. `GET` reports `talent_points_total` and
`talent_points_available`.

**The lock.** Talents and equipment (move, salvage, taking mail) cannot change while the
character is queued, in a party (ready check, key selection) or in an instance, disconnected
members included: the realm holds a snapshot for the run. Before each such change the API calls
the realm procedure `svc_engagement(account_id)` and answers 409 "Your talents and equipment are
locked while you are in a dungeon or queue." if engaged; if the realm cannot answer, the change
is refused too (fail closed). The snapshot (`svc_enqueue` carries talent keys and the total
points) is copied ticket -> party member -> fight and is never re-read, so a run and a
reconnect see the same build. The realm validates keys and cost again when it builds the fight
and drops all talents (logged) if they do not fit.

Every query is `WHERE id = $1 AND account_id = $2`: ownership is the query, not a check after
it. Another account's character is a 404 (tested).

## Entering a dungeon

```text
Postgres character ─► API: load, validate, compute gear score ─► svc_enqueue snapshot
  ─► queue ticket ─► party member ─► instance_player (runtime health, position)
```

The snapshot is read-only to the realm. Equipment can only change through the API.

## Results (exactly once)

```text
realm instance::finish ─► pending_result rows (same transaction as the run's end)
API bridge (every 5 s): svc_take_results ─► for each: apply_result in one Postgres transaction
     INSERT dungeon_results ON CONFLICT (result_id) DO NOTHING
     if new: experience via engine_realm::progression::gain, completion count/best time
  ─► svc_ack_results(ids) ─► realm deletes them
```

A crash anywhere re-delivers; the idempotency key makes re-delivery a no-op
(`realm-bot scenario results` checks the experience is applied once). What a run is worth is
decided by the realm from server state; there is no endpoint through which a client reports a
completion, loot or experience (`security` scenario: `admin_finish_instance` and
`svc_ack_results` are refused to players).

## Loot (exactly once)

```text
realm instance::finish ─► pending_loot "rolled" (one row per chest item, written once, durable)
combat events ─► LootAwarded{index} ─► "bag"   LootMailed{index} ─► "mail" (+ bag_full | unclaimed)
combat events ─► MobLoot ─► pending_loot "mail" + dungeon_drop   (settled at once)
              └─ MobCurrency ─► run_loot tally ─► finish folds it into pending_result.looted
instance destroy ─► rows still "rolled" ─► "mail" + unclaimed
API bridge (with results, every 5 s): svc_take_loot (settled rows only)
  ─► for each: apply_loot in one Postgres transaction
       INSERT loot_deliveries ON CONFLICT (loot_id) DO NOTHING
            (loot_id = "<instance>:<account>:loot:<index>" or "<instance>:<account>:mob:<n>")
       if new: lock the character; `bag` ─► first free cell, or the mailbox as `bag_full` if
               there is none; `mail` ─► the mailbox with the reason it was sent for
               - the item arrives with the quality, enhancement and broken state it was rolled with
  ─► svc_ack_loot(ids) ─► realm deletes them
```

The simulation rolls the chest when the dungeon completes (`equipment_per_completion` items per
player, from the boss table, at the run's key level) and `finish` writes one `pending_loot` row
per item (state `rolled`) and never again, so a restart mid-fight loses nothing and a row settles
once: later events for the same item are ignored. A player who opens the chest settles their rows
to `bag`; one whose bag was full settles to `mail`; one who never opened it is settled to `mail`
(`unclaimed`) when the instance is destroyed. Only settled rows leave the realm.

**Monster drops** are recorded from the events of the tick that produced them, inside the
transaction that stepped the fight, so a rolled-back tick drops nothing. An item becomes a row
already settled to `mail` with reason `dungeon_drop`; its number `n` is a per-player counter kept
in `run_loot`, so an id is never reused after earlier rows are acknowledged and deleted. Gold and
aether shards accumulate in the same row and are folded, once, into the player's `pending_result`
(`looted`) when the run ends - success or failure, since they were picked up either way.
`apply_result` pays them in the result's own transaction (only resources the economy defines, each
capped), next to the run's resources, which a failure does not pay. A run abandoned before it
produced a result pays what was looted as a failed result when the instance is torn down.

The reward bonus (a key modifier's) is read in one place (`combat::reward_bonus`), arms the fight
(`CombatInstance::set_loot`, `set_reward_bonus`) and scales the run's resource payout
(`RunRewards::with_bonus`).

`loot_deliveries.outcome` says where the item went: `bag`, `mail`, `mail_bag_full`, or - when
nothing could be given - `no_character` / `unknown_item`, so an item a player earned can be
found and re-granted. Nothing is dropped silently, and as with results there is no endpoint
through which a client reports loot.

## Account settings

```text
settings.cfg ─┬─ display, graphics          (this machine only)
              └─ everything else  ─┐
hud.cfg ───────────────────────────┴─► account document {format, settings, hud}
```

* At sign-in the client fetches the document and merges it over local settings
  (`engine_settings::account::apply_account_document` — validated, machine sections kept),
  then pushes the result into audio, camera, UI scale, action bars and the HUD layout.
* Every two seconds the client offers its current document; it uploads only when it differs
  from what was last synced (`PUT /api/v1/player/settings` with the version it replaces). A
  conflict (another machine saved first) re-fetches rather than overwrites.
* Documents are limited to 64 KiB and must be JSON objects; saves are rate limited.

## Write frequency

Nothing is written per frame. Postgres sees: sign-in (provisioning once), equipment and talent
changes, settings changes (at most every 2 s, only when changed), queue joins (read only),
results, chest loot and mail actions (taking or reading a letter). Runtime HP and positions never leave the realm.
