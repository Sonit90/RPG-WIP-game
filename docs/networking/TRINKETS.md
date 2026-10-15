# Active trinkets

A trinket in `Trinket1` / `Trinket2` can carry an **active**: a button with a cooldown of its own,
decided by the realm like any other combat action. Item data is in `assets/catalog/loot.json`;
the client only asks and shows.

## Data

A trinket row gains an optional block (read by `engine-realm`, handed to `engine-game` as plain
`TrinketActive` on `ItemDef` / `CatalogItem`; `engine-game` stays serde-free):

```json
"active": { "effect": "group_absorb", "cooldown_s": 120, "radius_m": 30, "duration_s": 10, "magnitude": 0.20 }
```

| effect | what it does | shipped item |
| --- | --- | --- |
| `group_damage_reduction` | party within 30 m takes 15% less damage (all schools) for 8 s; 120 s cd | Warden's Rallying Bell (rare) |
| `mass_dispel` | removes harmful magic/poison/disease/curse statuses from party within 12 m and helpful (purgeable) statuses from enemies within 12 m; 90 s cd | Censer of Cleansing Ash (rare) |
| `group_cooldown_reduction` | every ability cooldown of party within 30 m loses 10% of its remaining time; 120 s cd | Hourglass of the Patient Dead (epic) |
| `group_absorb` | party within 30 m gets a shield of 20% of max health for 10 s; 120 s cd | Aegis of the Last Vigil (epic) |

All four are in the boss table (weight 7). **Quality and enhancement scale the passive stats only**;
cooldown, radius, duration and magnitude are the row's numbers for every copy.

Which statuses a Mass Dispel takes is the status table's: a harmful status with a school
(`StatusKind::is_dispellable`) from allies, a helpful one with a school (`is_purgeable`: Fortitude,
Battle Shout, Arcane Intellect, Bloodlust, Regeneration) from enemies. Bleeds and plain buffs stay.

## Simulation

`PlayerCommand::UseTrinket(n)` -> `Simulation::use_trinket` (`sim/trinket_exec.rs`). Off the
global cooldown. Refused (and the reason kept for the HUD) when dead, stunned (nothing stuns a
player yet), nothing with an active is worn, or the slot is cooling down. Events: `TrinketUsed`,
then the usual `StatusApplied`, `Dispelled` + `StatusRemoved`, `AbsorbApplied`.

* Ward: `StatusKind::TrinketWard` (`DamageReductionMagnitude`), applied to each living party member in range.
* Absorb: a pool on the player (`TrinketState::absorb`, the Warrior's `Absorb` type) consumed
  before the Warrior's own in `defend_player`, plus `StatusKind::TrinketAbsorb` as its aura; the
  aura goes when the pool is spent, the pool goes when the aura does. `absorb_of` sums both.
* Cooldown cut: `AbilityBook::reduce_remaining_fraction` on the hero's abilities only (not the
  global cooldown, not the trinkets); it shortens a running recharge and never grants a charge.
* Putting a trinket on starts a cooldown of `min(its cooldown, 30 s)` so swapping is not a second use.

## Wire

`combat_act(ActionKind::Trinket(slot))` (appended last in `ActionKind` / `NetAction`), rate
limited as other combat actions and refused outside `Active`. `WireEvent::TrinketUsed` drives
sound and the ring VFX. Cooldowns ride `player_combat.resources.trinket1/trinket2` (seconds left
when the row was written), diffed like other timers - written only when they jump. The client
(`Simulation::set_remote_authority`) never applies an effect: the press becomes a request.
**The `player_combat` column type changed, so publishing needs `--delete-data=on-conflict`
(what `tools/realm-dev.ps1 publish` already passes).**

## Client

Two buttons above the Ultimate/extras (icon, cooldown sweep and seconds, glow while the ward or
shield stands, key label, tooltip with effect text, cooldown and radius). Default keys `T` / `V`
(`Bindings::trinket_one` / `trinket_two`, reserved against rebinding). Offline and in the hub
the same command runs the local simulation.
