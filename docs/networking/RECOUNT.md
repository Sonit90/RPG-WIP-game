# Recount

Combat statistics computed by the realm from the server's own combat events, shown to the
party, and persisted per player at the end of a run. Nothing is computed from client events.

## Server meter (`engine_realm::meter::InstanceMeter`)

Fed per fixed tick by `CombatInstance::advance` with the encoded `WireEvent`s, on the fight clock
(ticks / 60). Per player (by account): damage, healing, per ability damage / healing / hits /
crits / casts (wire index into `FireAbility::ALL`), damage per target (top 8 kept), active time.
Two scopes:

* **Overall**: the run, from the barrier lifting. Its length is the time to the end of the last
  fight (to now while one is going), so a quiet minute neither dilutes DPS nor makes rows change.
* **Current fight**: starts on party damage dealt or taken after 5 s of quiet, or the boss
  engaging; ends after 5 s without party damage, a boss reset, or the dungeon completing. The last
  finished fight stays as "current" until the next starts. Regeneration (healing with no ability)
  is not counted.

DPS = damage / scope seconds (floored at 1 s). The boss kill time is boss engage to dungeon
completion.

## Projection

Private table `instance_meter` (one row per instance player, JSON `MeterBlob`), caller-scoped
view `my_instance_meter` (the whole party's rows). `MeterProjector` writes at most once a second,
only when the meter changed, and only rows whose text differs. Rows are deleted with the
instance. After a rebuild of the fight from the tables, the meter is re-seeded from the rows (to
within a second); boss timing is lost, so `boss_kill_seconds` is then null.

## Persistence

`instance::finish` freezes the meter and puts a `damage_report` (JSON) on each `pending_result`:
party `[{account_id, character_name, class, role}]`, total damage, dps, duration, boss kill
seconds, abilities `[{ability, damage, casts}]`. Aggregates only. The API's `apply_result`
inserts it into `game.dungeon_damage_reports` in the same transaction as the result, for
completed runs only and with impossible figures dropped (a statistic never costs the reward);
re-delivery of a result id is a no-op. The admin anomaly endpoints read the same columns.

## Client

`engine-net` mirrors `my_instance_meter` (`RealmMirror::meter`). The Recount window
(`ui::recount`) toggles with `O` (`Bindings::recount`) or the REC button beside the mail button:
tabs Current Fight / Overall, modes Damage / Healing, class-coloured bars with totals and DPS,
click a player for abilities (damage, share, casts) and top targets. It opens on the Overall tab
when the completion panel appears. Offline it is fed by the local `CombatMeter`.
