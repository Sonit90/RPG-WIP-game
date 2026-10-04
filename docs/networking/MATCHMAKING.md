# Matchmaking

## Entering the queue

```text
client ── POST /api/v1/realm/queue {character_id, dungeon_id, role} ──► API
  API: character belongs to the caller?        (404 otherwise — not even its existence leaks)
       definition can play the role?           (400)
       dungeon exists, level in range, gear score ≥ minimum?   (400)
       gear score = engine_realm::loadout::gear_score(loadout from Postgres)
  API ── svc_enqueue(account, character, name, definition, level, gear score, dungeon, role)
  realm: caller is the bridge? player connected? not already queued or in a party?
         rate limit for the *player*, catalog re-check, insert queue_ticket (unique per account)
```

Nothing in a client request carries a gear score or a level. One ticket per **account**
(unique index), so the same account cannot queue two characters.

## The rule

A party is exactly **one Tank, one Healer, one DPS**, same dungeon, three different accounts,
gear scores close enough. Never partial, never a role replacement, never a bot.

"Close enough" starts at **±10 %** and widens linearly with how long the *anchor* (the
oldest member) has waited:

```text
tolerance
  20% ┤                 ┌────────────── max_tolerance
      │               ╱
  10% ┤──────────────╯                  min_tolerance
      └──────────────┬────┬───────────▶ seconds waited
                    30   60             expand_start, expand_end
```

All four numbers live in the realm's config row and can be changed on a live realm:

```powershell
spacetime call --server <url> realm admin_set_matchmaking 0.10 0.20 30 60
```

## The algorithm (`engine_realm::matchmaking::find_matches`)

Runs once a second (`matchmaking_tick`, a scheduled reducer):

1. Group eligible tickets by dungeon (eligible = searching and connected), dungeons in a
   fixed order.
2. Sort each group oldest first, ties by ticket id.
3. Each unmatched ticket in turn is the **anchor**; its waiting time sets the tolerance.
4. Candidates of the two other roles: unmatched, different account, within tolerance of the
   anchor; at most 32 per role, nearest first.
5. A trio qualifies if its whole spread `(max − min) / min` is within the tolerance (so two
   candidates on opposite sides cannot stretch a party to twice the tolerance).
6. Pick the smallest spread; ties to the longest combined wait, then the lowest ids.
7. Matched tickets leave the pool; continue with the next anchor.

Oldest-first is what stops anyone waiting forever; smallest-spread is what keeps parties
fair when the queue is busy. Pure and deterministic: the same queue at the same time forms the
same parties regardless of input order (tested).

## Concurrency

SpacetimeDB runs reducers one transaction at a time. A tick, a `queue_leave` and a
`svc_enqueue` cannot interleave, so no ticket is ever claimed twice and a player who leaves
the queue mid-tick is either matched or gone, never both.

## Disconnects

A disconnected player's ticket stays but is **ineligible**; after
`queue_disconnect_grace` (60 s default, `admin_set_timeouts`) the janitor removes it. Back
within the grace, it is eligible again with its original queue time.

## Tests (`cargo test -p engine-realm matchmaking`)

Exact 1/1/1 · missing role waits · within 10 % matches · beyond 10 % rejected at first ·
gradual widening (no jumps) · 15 % matches after 45 s · 20 % ceiling (21 % never) · spread
bound · closer gear preferred · oldest served first · several parties at once · dungeons
never mix · ineligible skipped · one account never fills two roles · deterministic and order
independent · zero gear scores · config validation. End to end: `realm-bot scenario slice`,
`isolation`, `decline`.
