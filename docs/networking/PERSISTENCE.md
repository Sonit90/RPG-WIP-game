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
| `game.account_settings` | the account settings document, with an optimistic version |

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
* `PUT /api/v1/characters/{id}/talents` — every key must be a talent the character's hero
  has.

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
changes, settings changes (at most every 2 s, only when changed), queue joins (read only), and
results. Runtime HP and positions never leave the realm.
