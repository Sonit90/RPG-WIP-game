# Dungeon modifiers

A dungeon key has a level (1..99) and a list of modifiers. Modifiers are data
(`assets/catalog/modifiers.json`) run by the shared simulation (`engine_game::modifier`).

```text
modifiers.json ─► engine-realm::modifiers ─ roll / resolve / reroll / downgrade / cleanse
                         │ resolve(level, key modifiers) -> Vec<ActiveModifier>
                         ▼
          Simulation::set_modifiers  ─ tick hook, monster-death hook, reset clears the ground
                         │ events (wire-encoded)
                         ▼
   client mirror (observe) ─► floor indicators, VFX burst, sounds, Plagued icon
```

## Catalog

* `modifiers[]`: `id, name, description, icon, kind, introduced_at_level, params,
  scaling_per_level, amplifiable, reducible, reroll {strengthen_chance, min_pct, max_pct}`.
  Strength = `(1 + scaling_per_level * (level - introduced_at_level)) * (1 + magnitude_pct / 100)`.
* `progression[]`: per key level `modifier_slots, health_mult, damage_mult` for the normal keys
  1-12 (multipliers may only rise). `eternal` continues it from level 13: every odd level adds
  `odd.add_modifier_slots` (up to `max_modifier_slots`), every even level adds
  `even.health_mult_step` / `even.damage_mult_step`. `ModifierCatalog::row_for(level)` is the one
  lookup (slots, `key_scaling`, rolls), synthesising rows past the table in closed form. The
  dungeon's `keys.health_per_level` is only the fallback for a catalog with no table.
* `magnitude_limits`: the clamp for a key's total reroll magnitude (-50..+100).
* `config`: `reroll_unlock_fraction` (0.5), `reroll_resource` (`aether_shard`), `reroll_cost` (1),
  `max_rerolls_per_run` (1), `reroll_reward_bonus_pct` (25), `cleanse_gold_cost` (100).

## The modifiers

Rotating Beams and Plague Explosion (key 3), then the monster-side four. All numbers are
`params` in the catalog; strength is the magnitude above.

* **Rotating Beams**: when a pack engages (and for the boss fight) an emitter appears at the
  pack's centroid: 3 beams, 9 m, 0.9 m wide, 35 deg/s, direction and start angle from the sim's
  seeded RNG. 2 s warm-up with no damage, then 5 % max HP (x magnitude, capped at 9.5 %) every
  0.5 s to anyone in a beam, one tick per player per interval. Ends when the encounter has no
  awake member left, or on reset. The angle is a pure function of the start event and the clock.
* **Plague Explosion**: every monster death (not the boss, not players) lights a 5 s fuse;
  burst of 12 % max HP in 4 m, then a 4 m cloud for 6 s ticking 3 %/s and applying `Plagued`
  (-20 % healing received, refreshed while inside, lingers 3 s). Healing reduction is applied
  in `GameWorld::heal`, the one heal path. Fuses (48) and clouds (16) are bounded arrays.

* **Bolstering** (key 5): when a non-boss monster dies, each living non-boss monster within
  30 m gains a stack (cap 5): +10 % of its base max health (current health rises with it) and
  +10 % damage. Undone by a reset or wipe.
* **Raging** (key 7): a non-boss monster below 30 % health deals +50 % damage. Not a status, so
  not dispellable; it ends if the monster is healed back over the threshold.
* **Necrotic Pools** (key 11): a non-boss death marks the ground (1.5 s warning, 3 m), then a
  pool lasts 8 s dealing 4 %/s of max HP to players inside (capped per tick). It reuses the
  plague's `PlagueArmed` / `PlagueExploded` / `PlagueCloudEnded` events, so clients draw it with
  the plague's floor discs; the burst effect plays when the pool forms.
* **Spiteful Casters** (key 13): interruptible enemy casts are 30 % faster; a completed one
  gives its targets `Spite` (Magic school, so dispellable; -30 % healing received, 10 s). An
  interrupt stops the cast and its curse.

Modifier damage comes from no entity and no ability, skips the incoming multipliers, and is
capped (`max_hit_pct`), so it cannot one-shot a full-health player.

## Client

Under remote authority nothing runs; `ModifierRuntime::observe` mirrors the events (rebasing
the server's clock to receipt) and presentation derives its floor indicators each frame
(`TelegraphIndicators::sync_modifiers`). Dev: `--modifiers rotating_beams,plague_explosion:1.5`.

## Wiring (keys, party, instance, API)

```text
API: key created / level changes ─► roll_key_modifiers(level, OS seed) ─► dungeon_keys.modifiers
POST /realm/queue ─► svc_enqueue(..., key_modifiers JSON) ─► ticket ─► party_member.key_modifiers
                                     every member sees every key's modifiers (KeySelect, Party)
party_start ─► instance.modifiers (the owner's key, sanitized) ─► resolve(level, mods)
                 ─► CombatInstance::set_modifiers   + key_scaling from the progression table
modifier_reroll(index) ─► plan_reroll ─► instance row + fight ─► notice + reveal ─► result
```

* **Storage.** `game.dungeon_keys.modifiers` (JSONB `[{"id","magnitude_pct"}]`) plus
  `modifiers_rolled`. A key is rolled when it is created or its level changes (a run's
  success raises it); an older key with `[]` at a level that has slots is rolled once, lazily,
  when it is next read (`store::get_key` / `keys_of`, a compare-and-set on `modifiers_rolled`).
  A cleansed key stays empty because the flag is already set. The seed is the OS RNG.
* **Snapshot.** The API passes the key's modifiers to `svc_enqueue` (last argument); the realm
  sanitizes them (unknown / repeated modifiers dropped, magnitude clamped, at most the level's
  slots) and copies them to the ticket and the party member. Nothing is picked at random in the
  realm: the inserted key's own modifiers are the instance's.
* **Instance.** `dungeon_instance` carries `modifiers` (JSON, magnitudes after rerolls),
  `rerolls`, `reward_bonus_pct`, `reroll_seq` and `last_reroll` (`RerollEvent`). The fight is
  rebuilt from the row after a restart (`combat::apply_run_rules`). Monster health and damage
  come from `ModifierCatalog::key_scaling` (the progression table), not `key::scaling`.
  The admin snapshot lists the modifiers as `Plague Explosion +17%`.
* **Reroll** (`modifier_reroll(index)`, any party member, on their own currency). Refused
  unless the instance is `Active`, `dungeon_kill_progress` is at least
  `config.reroll_unlock_fraction` (0.5), the run has a reroll left (`max_rerolls_per_run`, 1 for
  the whole party), the modifier can move, and the caller holds `reroll_cost` (1) of
  `reroll_resource` (`aether_shard`) beyond what they already spent this run. The balance is the
  snapshot loadout's; the spend is the private `instance_spend` row (read through
  `my_instance_spend`). The dice come from `ctx.random`, the result goes through
  `reroll_outcome` + `apply_reroll`, the fight gets `set_modifiers(resolve(..))` (the floor
  indicators restart; magnitudes change mid-run), and every member gets a notice
  `<Name> rerolled <Modifier>: +17% (stronger)`. The instance row's `reroll_seq` tells the
  clients to play the reveal: the client only animates "ROLLING... 60% Strengthen / 40% Weaken"
  and then the server's result.
* **Reward bonus.** A reroll sets `reward_bonus_pct = config.reroll_reward_bonus_pct` (25):
  `CombatInstance::set_reward_bonus` forwards it to the simulation's loot rules and the run's
  resource payout is scaled with `RunRewards::with_bonus` when the run succeeds. The Modifier
  panel shows "+25% loot bonus active"; the completion panel shows the bonus.
* **Currency.** The realm reports `pending_result.currency_spent` (`[["aether_shard",1]]`); the
  API debits it in the result transaction (success or failure), saturating at zero, once per
  `result_id`. A member who leaves, or an instance torn down before a result, still settles the
  spend (`instance::settle_spend` writes a result with no rewards and no key change).
* **Downgrade and cleanse** (hub only; both refuse with 409 while the account is queued, in a
  party or in an instance, like equipment): `POST /characters/{id}/key/downgrade` lowers the
  key one level (never below the dungeon's minimum, never deleted) and drops the modifiers the
  lower level has no slot for; `POST /characters/{id}/key/cleanse` removes every modifier for
  `config.cleanse_gold_cost` gold, all or nothing in one transaction, refused when the key has
  none (no charge) or the character has too little gold. Both answer with the character.
* **Participant progression.** On a success the realm flags every member who did not open the
  run and whose own key (same dungeon) was lower than the key used
  (`key::participant_progress`); the result carries `participant_key_bump` and `own_key_level`
  and the API raises that key by one, rolled afresh, with a compare-and-set on `own_key_level`
  (`DungeonResult::key_update`), so it rises once per run.
* **Interface.** The Modifier panel (`ui::modifiers`, key `K` or the KEY button beside Recount)
  lists the modifiers with icon, name, description and magnitude, the cleared share of the
  dungeon, the unlock, the cost and the player's balance, and Reroll buttons. Key selection
  ("Available Keys") lists every member's key with its modifiers and an Insert button on the
  player's own row; the Party panel shows the inserted key's modifiers; the hub has a key panel
  with Downgrade and Cleanse buttons, each asking for confirmation. Sounds: `ModifierReroll`
  (tick), `ModifierResult` / `ModifierResultWeak` (stingers).
* **Operator hooks** (owner only, for `realm-bot scenario modifiers`): `admin_clear_dungeon
  (instance, percent)` and `admin_grant_spend(instance, account, balance)`.
