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
3. One `instance_monster` row per enabled monster in the level file — placed, not yet
   simulated (server combat and AI are Phase 9; the client shows them where the server says).
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

* the realm moves players against a collision world **with** the box while closed and
  **without** it once open (both built once per dungeon and cached);
* the client adds the same box to its prediction world while the realm says it is closed.

A client that ignores the barrier is rubber-banded back by reconciliation. Combat is disabled
outside `Active` (`allows_combat`).

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
