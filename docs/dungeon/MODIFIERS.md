# Dungeon modifiers

A dungeon key has a level (1..20) and a list of modifiers. Modifiers are data
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
* `progression[]`: per key level `modifier_slots, health_mult, damage_mult`. It is the source of
  key-level monster scaling through `ModifierCatalog::key_scaling`; the dungeon's
  `keys.health_per_level` stays as the fallback (the realm module still calls `key::scaling`
  until it is switched over).
* `magnitude_limits`: the clamp for a key's total reroll magnitude (-50..+100).

## First two modifiers

* **Rotating Beams**: when a pack engages (and for the boss fight) an emitter appears at the
  pack's centroid: 3 beams, 9 m, 0.9 m wide, 35 deg/s, direction and start angle from the sim's
  seeded RNG. 2 s warm-up with no damage, then 5 % max HP (x magnitude, capped at 9.5 %) every
  0.5 s to anyone in a beam, one tick per player per interval. Ends when the encounter has no
  awake member left, or on reset. The angle is a pure function of the start event and the clock.
* **Plague Explosion**: every monster death (not the boss, not players) lights a 5 s fuse;
  burst of 12 % max HP in 4 m, then a 4 m cloud for 6 s ticking 3 %/s and applying `Plagued`
  (-20 % healing received, refreshed while inside, lingers 3 s). Healing reduction is applied
  in `GameWorld::heal`, the one heal path. Fuses (48) and clouds (16) are bounded arrays.

Modifier damage comes from no entity and no ability, skips the incoming multipliers, and is
capped (`max_hit_pct`), so it cannot one-shot a full-health player.

## Client

Under remote authority nothing runs; `ModifierRuntime::observe` mirrors the events (rebasing
the server's clock to receipt) and presentation derives its floor indicators each frame
(`TelegraphIndicators::sync_modifiers`). Dev: `--modifiers rotating_beams,plague_explosion:1.5`.

## Not wired yet

Key storage of `KeyModifier` (JSONB `dungeon_keys.modifiers`), rolling at key creation,
`CombatInstance::set_modifiers` at instance build with `resolve`, the reroll / downgrade /
cleanse reducers (use `reroll_outcome`, `apply_reroll`, `downgrade`, `cleanse`;
`Simulation::dungeon_kill_progress` for the 50 % unlock), the key panel UI, switching the module
to `key_scaling`.
