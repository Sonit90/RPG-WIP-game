# The P0 game loop

Launch → title → sign in → character → hub → queue → ready check → choose a key → start →
dungeon → boss → completion → rewards → hub → queue again. This page is the design for the
parts the loop added on top of the existing realm (see the other pages in this folder for what
was already there: auth, matchmaking, ready check, instances, server combat, loot, results).

## What already existed (reused, not rewritten)

| Step | Existing system |
| --- | --- |
| Register / login / logout / remembered session | `backend` auth + `engine-net::session`, DPAPI token store |
| Character list + equipment | `GET /characters`, character menu, `equipment/move` |
| Queue, matchmaking 1/1/1, ready check, timeout, decline, re-queue | realm `queue_ticket`, `matchmaking_tick`, `ready_check_*`, `engine_realm::matchmaking` |
| Party FSM, authorization, departures | `engine_realm::party` |
| Instance lifecycle, countdown, barrier, server combat, boss, completion | `engine_realm::instance`, realm `combat_tick`, `instance::finish` |
| Chest loot exactly once, results exactly once | `pending_loot`, `pending_result`, API bridge |
| Reconnection | `engine-net` reconnect + realm presence/grace |

## What the loop adds

1. **Dungeon keys** (server-side, Postgres) and **key selection** in the party (realm).
2. **Resources** (reagents) on the character, **salvage** and a **resource exchange** foundation (API).
3. **Data-driven rewards**: experience and resources scale with key level; key progression.
4. **Client**: title screen, hub (training dummies, equipment, salvage, exchange, key, dungeon
   finder), key-selection screen, completion screen with Return to Hub.

## Client flow state

`engine-presentation::ui::frontend::Screen` is the client's one state enum. Screens the server
decides come from the realm mirror (`OnlineSession::screen`), so a reconnect lands correctly:

```text
Title ─► Login ─► Characters ─► Hub ◄────────────────────────────────┐
                                 │ Dungeon Finder (Dungeons ─► Roles)  │
                                 │ Salvage & Exchange (Workshop)       │
                                 ▼                                     │
                    ticket ─► Hub + queue panel (stays playable)       │
           party ReadyCheck ─► ReadyCheck (match proposal)             │
          party ChoosingKey ─► KeySelect                               │
                party Ready ─► Party (leader = key owner: Start)       │
          instance (running) ─► InWorld                                │
 instance Completed / Failed ─► InWorld + completion panel ─ Return ───┘
```

Server state machines are unchanged in shape; `PartyState` gains one state (below). Invalid
transitions are refused by `PartyState::can_become`, `InstanceState::can_become` and
`party::authorize`.

## Dungeon keys

### Storage (`backend/migrations/0005_keys_resources.sql`)

```sql
game.dungeon_keys (
  character_id UUID PRIMARY KEY REFERENCES game.characters(id) ON DELETE CASCADE,
  account_id   UUID NOT NULL,
  dungeon_id   TEXT NOT NULL,
  level        INT  NOT NULL CHECK (level >= 1),
  modifiers    JSONB NOT NULL DEFAULT '[]',   -- reserved for the modifier ticket; always [] in P0
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
)
```

One key per character. Provisioned with the character (and back-filled on list for existing
characters) for the first dungeon in the catalog at the dungeon's `keys.min_level`.

### Snapshot

`POST /realm/queue` reads the character's key and passes `(key_dungeon_id, key_level)` to
`svc_enqueue` (empty / 0 when the character has no key). The realm copies it to the ticket and
party member. A client never sends a key level.

### Party: `ChoosingKey`

```text
ReadyCheck ─all accept─► ChoosingKey ─member inserts own key─► Ready ─leader starts─► Starting …
     │                        │   ▲                              │
     └─decline/timeout─► Disbanded ◄──── member leaves ──────────┤
                              └───── key owner withdraws ◄───────┘
```

* `ChoosingKey`: party formed, members locked (tickets deleted), **no leader yet**.
* `party_insert_key` (any member, `ChoosingKey`): the member's *own* snapshotted key must be
  for the party's dungeon and level ≥ 1. The party records `key_owner` and `key_level`; the
  owner becomes leader; state → `Ready`. First insertion wins; the realm runs reducers one at a
  time, so two simultaneous inserts cannot both win.
* `party_withdraw_key` (key owner, `Ready`): back to `ChoosingKey`, no leader.
* `party_start` (leader, `Ready`): additionally requires `key_owner == leader` and the key
  valid. The instance is built with the key level.
* Leader leaves before the start: the existing rule (disband, re-queue the others in place
  with their original queue time). Deterministic, never a half party.
* Transfer leadership is no longer allowed in `Ready` (the leader is whoever inserted the key);
  still allowed in `Starting`/`InDungeon` (successor rules unchanged).
* The random leader pick at ready-check time is gone.

### Effect of the key level

Catalog (`assets/catalog/dungeons.json`, per dungeon):

```json
"keys":    { "min_level": 1, "max_level": 20,
             "health_per_level": 0.10, "damage_per_level": 0.05 },
"rewards": { "experience": 200, "experience_per_level": 50,
             "resources": [ { "id": "crypt_dust", "amount": 5, "per_level": 2 } ] }
```

* Monster health × `1 + health_per_level × (level − 1)`, damage × `1 + damage_per_level ×
  (level − 1)`, on top of the party-size scaling (`DungeonScaling` gains a key factor).
* Rewards (`engine_realm::rewards::run_rewards`): on success experience and each resource scale
  linearly with level; on failure nothing.
* Progression (`engine_realm::key::progress`): success → owner's key `level + 1` (capped at
  `max_level`); failure → unchanged. Never below `min_level`.

### Exactly once

`instance::finish` writes one `pending_result` per player with the rewards *and* the key
fields: `key_level` (used), `key_owner` (bool), `key_level_after`. The API applies the result in
one transaction keyed by `result_id`: experience, completion, resources into the loadout, and
— for the owner on success — `UPDATE dungeon_keys SET level = key_level_after WHERE
character_id = … AND level = key_level`. A re-delivered result is a no-op (idempotency key),
and the `level = key_level` guard means a key can never advance twice for one run.

## Resources, salvage, exchange

* `Loadout.resources: BTreeMap<String, u64>` (serde default empty) — inside the same JSONB as
  the bag, so salvage is one locked read-modify-write.
* Catalog `assets/catalog/economy.json`: resource definitions (id, name, description), salvage
  yield (`resource`, `base`, `per_item_level`), exchanges (`id`, `from`, `from_amount`, `to`,
  `to_amount`).
* `POST /characters/{id}/salvage {index}`: a *bag* item only (never worn); removes one of the
  stack, adds `base + per_item_level × item_level` of the salvage resource. Answers with the
  character.
* `POST /characters/{id}/exchange {exchange_id, times}`: validated against the catalog,
  refuses insufficient funds. Answers with the character.

## Completion screen data

Realm table `run_summary` (one per player per instance, private; view `my_run_summary`):
success, dungeon, key level used, key level after (owner only), duration, experience,
resources (JSON), loot item key (filled when the chest roll settles). Deleted with the instance.
The client shows it over the world when the instance is `Completed`/`Failed`; **Return to Hub**
sends `party_leave`, then re-fetches the character and key (showing "Saving rewards…" until the
API has applied the result).

## Hub

The hub is the client's local world while the player is not in an instance: the start area of
the loaded level with training dummies the local simulation spawns (local authority, the same
combat code as offline). Nothing done in the hub is persistent except through API calls
(equipment, salvage, exchange), so it needs no server simulation. While queued the player keeps
playing in the hub; the queue panel shows dungeon, role, state and elapsed time (from the
ticket's server `enqueued_at`).

The hub is built and taken down by `OnlineSession::sync_hub` (`engine-app/src/online.rs`) from the
screen the client is on, not by directives: it stands behind the Hub and every page opened from it
(Dungeon Finder, Workshop, ready check, key selection, party) and is gone before an instance is
entered. Enemies are removed with `Simulation::despawn_enemy`, never with the world's `despawn`:
the latter leaves the enemy's runtime in its slot, and the next entity to take the slot (a hub
dummy) would inherit its name, rank and mind.

**Known limitation:** the hub reuses the dungeon level's geometry (the renderer loads one
environment per process). A dedicated hub scene needs runtime scene switching — a separate
ticket.
