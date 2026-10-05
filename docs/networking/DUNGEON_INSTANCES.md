# Dungeon instances

Every party gets its own instance. Two parties in the same dungeon share its *definition*
(catalog row and level file) and nothing else.

## Lifecycle (`engine_realm::instance::InstanceState`)

```text
Created ─▶ Loading ─▶ Ready ─▶ Countdown ─▶ Active ─┬─▶ Completed ─┐
                                                    └─▶ Failed ────┤
  (any state, when nobody can come back) ───────────────────────────┴─▶ CleaningUp ─▶ Destroyed
```

`party_start` (leader only, validated) runs Created → Loading → Ready → Countdown in one
transaction:

1. Party → `Starting`; a `dungeon_instance` row (unique per party).
2. One `instance_player` row per member at the dungeon's spawn points (catalog
   `spawn_points`, party order), with the character's health.
3. The **fight** is built (`combat::start`): the real `engine_game::Simulation` for the level,
   with every member in it as their own class and stored loadout (see
   [Server combat](#server-combat)). Its first projection writes one `instance_monster` row per
   monster, every player's health and a `player_combat` row each.
4. `countdown_ends_at = now + countdown_seconds` (5) and a one-shot scheduled
   `countdown_finish`.

`countdown_finish` → `Active`, `barrier_open = true`, party → `InDungeon`.

## Countdown

The countdown is a **server timestamp**. Clients estimate the server clock (`server_time`
procedure, five exchanges, the least-delay answer wins — NTP style) and draw 5, 4, 3, 2, 1, GO
from it with `Countdown::display`. No client counts down on its own, so three clients show the
same number and nothing happens when the fastest one reaches zero.

## Starting barrier

`barrier_closed(state)` is a function of the state, not a separate flag: closed through
Countdown, open from Active. The barrier is a collision box from the catalog
(`start_barrier`, across the crypt gate):

* the realm's fight has the box in its collision world while closed and removes it at
  `countdown_finish`; players are moved in that world, so they meet it exactly as monsters
  and projectiles do;
* the client adds the same box to its prediction world while the realm says it is closed.

A client that ignores the barrier is rubber-banded back by reconciliation. Combat is disabled
outside `Active` (`allows_combat`).

## Server combat

The realm runs each instance's fight with the gameplay code the offline game runs — monsters,
brains, threat, abilities, damage, statuses, the dungeon run and its boss gate — with the
whole party in one simulation (`engine_game::sim::players`: per-player state is swapped in for
that player's phases and whenever damage lands on or comes from them).

```text
move_input  ─► CombatInstance::apply_frames ─► Simulation::drive_player (one 60 Hz step a frame)
combat_act  ─► CombatInstance::act ─► SelectTarget + UseAbility… for that player's next tick
combat_tick (50 ms, scheduled) ─► advance(due ticks, ≤ 12) ─► WireEvents ─► combat_batch (JSON)
                                     └─► changed rows only: instance_player (body, health),
                                         player_combat, instance_monster
```

* **Clock.** `instance_combat.origin` is tick zero; tick *n* is due at origin + n/60 s. A pass
  runs at most 12 ticks; a server further behind lets the fight run slow rather than spiral.
* **Requests, not outcomes.** `combat_act(target, action)` names a bar slot, an extra button or
  the ultimate (and a point for aimed ones) plus the selection. Range, cooldown, resource, line
  of sight, being alive — the simulation decides. Rate-limited (`combat`, 30 burst, 15/s);
  refused outside `Active` (a pure target change is allowed during the countdown).
* **Names on the wire.** Entities travel as `EntityRef` — a player by account, a monster by a
  key that is its spawn order for placed monsters (`1<<32 +` n for summoned ones) — and
  abilities and statuses by their index in the gameplay tables
  (`engine_realm::combat::wire`).
* **Rows are a projection.** The simulation lives in wasm memory (`thread_local`, per
  instance). Each row records the fight tick it reflects; a fight whose tick differs from
  `instance_combat.tick` (a new module instance, a rolled-back transaction) is rebuilt from the
  rows first (`CombatInstance::restore`): positions, health, the dead, completion credit and the
  gate survive; casts, threat, cooldowns and summoned adds do not.
* **Writes are diffed.** Monsters are rewritten when they move more than a centimetre, change
  health, target, cast or statuses; timers a client can run down itself (cooldowns, cast
  progress, status time) only when they jump (`PlayerState::combat_differs`).
* **Events.** Each pass's events go in one `combat_batch` row (kept 3 s); clients apply each
  batch once, on insert, and never replay batches present at subscription time.
* **Completion.** When the dungeon run completes, `combat_tick` calls `instance::finish` —
  results go to the outbox exactly as before. A wipe is every player down; players respawn as
  offline and monsters evade.

### On the client

The local `Simulation` runs under a **remote authority** (`Simulation::set_remote_authority`):
it predicts its own movement and keeps targeting, and does nothing else — no AI, abilities,
damage, healing, regeneration or respawn. Ability presses are diverted to
`take_remote_requests` and sent as `combat_act` with the current selection. Server state is
laid into the world with `present_health`, `present_statuses`, `present_enemy`, `present_cast`
and `present_class_state` (timers aged by how long ago the row arrived), and server events are
decoded and `inject_event`ed, so damage numbers, effects, sound, cast bars and the meter work
unchanged. Monsters are interpolated like remote players. A server-side move of the local
player (respawn, Blink, charge) changes the row without a new sequence number; the client
reconciles on that too.

## Isolation

* Every query in the module starts from an instance id.
* Clients see instances only through `my_instance`, `my_instance_players`,
  `my_instance_monsters`, each derived from `ctx.sender()`'s own membership.
* `realm-bot scenario isolation` runs two parties at once and checks two instances exist and
  each client sees exactly its own three players.

## Cleanup

The janitor (every 2 s) tears an instance down when `should_clean_up` says so: every remaining
member is past the reconnect grace (or nobody is left), or a finished run has lingered
`finished_linger` (120 s). It moves to CleaningUp → Destroyed and deletes the instance's
players, monsters and party in the same transaction. A disconnect alone never ends a run.

## Completion and results

`instance::finish(instance, success)` (today reachable only by the owner through
`admin_finish_instance`; server combat will call it) moves the instance and party to
Completed/Failed and writes one `pending_result` per player with the server-decided
experience. See [PERSISTENCE.md](PERSISTENCE.md).

## Tests

`cargo test -p engine-realm instance movement`: lifecycle order (no skipping the countdown),
barrier follows state, countdown shows 5…1 GO, disconnect does not end a run, everyone gone
past grace cleans up, finished runs linger then clean up, the start barrier holds the party in
and the open level lets it walk on. End to end: `slice` (countdown takes server time, barrier
lifts, everyone moves), `isolation`, `results`.
