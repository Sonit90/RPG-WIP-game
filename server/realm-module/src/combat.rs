//! Server-side combat: each live instance's fight, ticked here.
//!
//! ```text
//! combat_act ─► CombatInstance::act        (queued for the player's next tick)
//! move_input ─► CombatInstance::apply_frames
//! combat_tick (every 50 ms) ─► advance(due ticks) ─► combat_batch (events, JSON)
//!                                             └──► instance_player / player_combat /
//!                                                  instance_monster rows (changed only)
//! ```
//!
//! The fight itself is the real `engine_game::Simulation`, held in wasm memory per instance:
//! rows are its projection, not its state. A row records the fight tick it reflects, and a
//! fight whose tick does not match the table (a fresh module instance, a transaction rolled
//! back after the fight had stepped) is rebuilt from the rows before it is used. A rebuild
//! keeps positions, health, the dead and the dungeon's progress; it loses casts, threat and
//! cooldowns, which is the price of a server restart mid-pull.

use std::cell::RefCell;
use std::collections::BTreeMap;

use engine_realm::RealmError;
use engine_realm::combat::{
    AbilityRef, ActionError, ActionKind, CastState, CombatAction, CombatInstance, EntityRef,
    MonsterRecord, MonsterState, PlayerRecord, PlayerSpec, Restore, StatusState,
};
use engine_realm::instance::{InstanceState, fight_seed};
use engine_realm::loadout::Loadout;
use engine_realm::movement::barriers_of;
use engine_realm::rate_limit::Operation;
use spacetimedb::{ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp};

use crate::schema::*;
use crate::util::{micros, rate_limit, timestamp};

/// Microseconds per fight tick (60 Hz).
const TICK_US: i64 = 16_667;

/// The most ticks one pass runs. A server that fell further behind than this lets the fight
/// run slow rather than spend a whole pass catching up.
const MAX_TICKS_PER_PASS: u64 = 12;

/// How long an event batch is kept for clients to read.
const BATCH_TTL_US: i64 = 3 * engine_realm::SECOND;

thread_local! {
    static FIGHTS: RefCell<BTreeMap<u64, CombatInstance>> = const { RefCell::new(BTreeMap::new()) };
}

/// Drives [`combat_tick`].
#[spacetimedb::table(accessor = combat_timer, scheduled(combat_tick))]
pub struct CombatTimer {
    /// Row id.
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    /// Twenty times a second.
    pub scheduled_at: ScheduleAt,
}

/// Starts the combat pass.
pub fn schedule(ctx: &ReducerContext) {
    ctx.db.combat_timer().insert(CombatTimer {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(50_000)),
    });
}

pub fn net_entity(reference: &EntityRef) -> NetEntity {
    match reference {
        EntityRef::None => NetEntity::Nobody,
        EntityRef::Player(account) => NetEntity::Player(account.clone()),
        EntityRef::Monster(key) => NetEntity::Monster(*key),
    }
}

fn entity_ref(net: &NetEntity) -> EntityRef {
    match net {
        NetEntity::Nobody => EntityRef::None,
        NetEntity::Player(account) => EntityRef::Player(account.clone()),
        NetEntity::Monster(key) => EntityRef::Monster(*key),
    }
}

fn net_ability(ability: AbilityRef) -> NetAbility {
    match ability {
        AbilityRef::None => NetAbility::Nothing,
        AbilityRef::Player(index) => NetAbility::Player(index),
        AbilityRef::Monster(index) => NetAbility::Monster(index),
    }
}

fn net_cast(cast: CastState) -> NetCast {
    NetCast {
        ability: net_ability(cast.ability),
        elapsed: cast.elapsed,
        duration: cast.duration,
    }
}

fn net_statuses(statuses: &[StatusState]) -> Vec<NetStatus> {
    statuses
        .iter()
        .map(|status| NetStatus {
            kind: status.kind,
            source: net_entity(&status.source),
            remaining: status.remaining,
            duration: status.duration,
            stacks: status.stacks,
        })
        .collect()
}

/// The party as the fight needs it, in spawn order.
fn specs(
    ctx: &ReducerContext,
    instance: &DungeonInstance,
    dungeon: &crate::level::Dungeon,
) -> Vec<PlayerSpec> {
    let catalog = crate::level::catalog().ok();
    let mut players: Vec<InstancePlayer> = ctx
        .db
        .instance_player()
        .instance_id()
        .filter(instance.id)
        .collect();
    let member_of =
        |player: &InstancePlayer| ctx.db.party_member().identity().find(player.identity);
    players.sort_by_key(|player| {
        (
            member_of(player).map_or(u8::MAX, |m| m.slot),
            player.account_id.clone(),
        )
    });
    players
        .iter()
        .enumerate()
        .map(|(index, player)| {
            let member = member_of(player);
            let slot = member.as_ref().map_or(index, |m| usize::from(m.slot));
            let hero = catalog
                .as_ref()
                .and_then(|c| c.character(&player.definition_id))
                .map_or(engine_game::hero::Hero::Warrior, |d| d.hero);
            let loadout = member
                .as_ref()
                .and_then(|m| serde_json::from_str::<Loadout>(&m.loadout).ok());
            let talents = member
                .as_ref()
                .map(|m| engine_realm::talents::decode_keys(&m.talents))
                .unwrap_or_default();
            PlayerSpec {
                account_id: player.account_id.clone(),
                name: player.name.clone(),
                hero,
                loadout,
                talents,
                talent_points: member.as_ref().map_or(0, |m| m.talent_points),
                spawn: dungeon.definition.spawn_point(slot),
            }
        })
        .collect()
}

/// What the instance's key multiplies its monsters' health and damage by: the progression
/// table of `modifiers.json`, or the dungeon's own linear rule if the catalog has no table.
fn key_scaling(instance: &DungeonInstance, dungeon: &crate::level::Dungeon) -> (f32, f32) {
    let level = instance.key_level.max(1);
    match crate::level::catalog() {
        Ok(catalog) => catalog
            .modifiers()
            .key_scaling(&dungeon.definition.keys, level),
        Err(_) => engine_realm::key::scaling(&dungeon.definition.keys, level),
    }
}

/// Puts the run's key modifiers and reward bonus in force in `fight`, from the instance row:
/// what a new fight starts with, what a rebuilt one gets back, and what a reroll changes.
pub fn apply_run_rules(instance: &DungeonInstance, fight: &mut CombatInstance) {
    let mods = engine_realm::modifiers::decode_modifiers(&instance.modifiers);
    match crate::level::catalog() {
        Ok(catalog) => {
            fight.set_modifiers(
                catalog
                    .modifiers()
                    .resolve(instance.key_level.max(1), &mods),
            );
        }
        Err(error) => log::error!(
            "event=modifiers_not_applied instance_id={} error={error}",
            instance.id
        ),
    }
    fight.set_reward_bonus(instance.reward_bonus_pct);
}

/// The run's reward bonus, per cent: how much more every drop and payout is worth.
///
/// Zero until the dungeon key's modifiers set one. This is the single place that decides it:
/// the fight is armed with it when built or rebuilt, and `instance::finish` scales the run's
/// resource payout with it, so the two cannot disagree. It reads the instance (not the live
/// fight) because a rebuilt fight must come back with the same bonus.
pub fn reward_bonus(_ctx: &ReducerContext, instance: &DungeonInstance) -> u32 {
    instance.reward_bonus_pct
}

/// Gives a freshly built (or rebuilt) fight the loot rules, the key level and the reward bonus.
/// (Its chances - crits, enemy decisions, loot - were seeded from the instance's uid when it
/// was built: see [`fight_seed`].)
fn arm(ctx: &ReducerContext, instance: &DungeonInstance, fight: &mut CombatInstance) {
    match engine_realm::loot::builtin_rules() {
        Some(rules) => fight.set_loot(
            rules.clone(),
            instance.key_level.max(1),
            reward_bonus(ctx, instance),
        ),
        None => log::error!(
            "event=loot_rules_missing instance_id={} the legacy chest is used",
            instance.id
        ),
    }
}

/// Builds an instance's fight and writes its first rows. Called once, when the instance
/// is created.
pub fn start(ctx: &ReducerContext, instance: &DungeonInstance) -> Result<(), RealmError> {
    let dungeon = crate::level::dungeon(&instance.dungeon_id)?;
    let players = specs(ctx, instance, &dungeon);
    let barriers = barriers_of(&dungeon.scene, &dungeon.definition);
    let mut fight = CombatInstance::new_seeded(
        &dungeon.scene,
        &players,
        &barriers,
        key_scaling(instance, &dungeon),
        fight_seed(instance.uid),
    );
    arm(ctx, instance, &mut fight);
    apply_run_rules(instance, &mut fight);
    ctx.db.instance_combat().insert(InstanceCombat {
        instance_id: instance.id,
        tick: fight.tick(),
        origin: ctx.timestamp,
        credited: 0.0,
        unlocked: false,
    });
    FIGHTS.with(|fights| fights.borrow_mut().insert(instance.id, fight));
    with_fight(ctx, instance, |fight| {
        project(ctx, instance.id, fight);
        write_meter(
            ctx,
            instance.id,
            fight.meter_rows(micros(ctx.timestamp), true),
        );
    })?;
    Ok(())
}

/// Writes the Recount rows `rows` (`(account, JSON)`) for `instance_id`: an insert for a
/// player without a row, an update when the text differs, nothing otherwise.
pub fn write_meter(ctx: &ReducerContext, instance_id: u64, rows: Vec<(String, String)>) {
    if rows.is_empty() {
        return;
    }
    let players: Vec<InstancePlayer> = ctx
        .db
        .instance_player()
        .instance_id()
        .filter(instance_id)
        .collect();
    for (account, data) in rows {
        let Some(player) = players.iter().find(|p| p.account_id == account) else {
            continue;
        };
        let row = PlayerMeter {
            identity: player.identity,
            instance_id,
            account_id: account,
            data,
            updated_at: ctx.timestamp,
        };
        match ctx.db.instance_meter().identity().find(player.identity) {
            Some(existing) if existing.data == row.data => {}
            Some(_) => {
                ctx.db.instance_meter().identity().update(row);
            }
            None => {
                ctx.db.instance_meter().insert(row);
            }
        }
    }
}

/// Runs `body` with `instance`'s fight, rebuilding it from the rows first if this module
/// instance does not hold it at the tick the tables say it reached.
pub fn with_fight<R>(
    ctx: &ReducerContext,
    instance: &DungeonInstance,
    body: impl FnOnce(&mut CombatInstance) -> R,
) -> Result<R, RealmError> {
    let combat = ctx
        .db
        .instance_combat()
        .instance_id()
        .find(instance.id)
        .ok_or_else(|| RealmError::dungeon("That dungeon has no fight."))?;
    let current = FIGHTS.with(|fights| {
        fights
            .borrow()
            .get(&instance.id)
            .is_some_and(|fight| fight.tick() == combat.tick)
    });
    if !current {
        let rebuilt = rebuild(ctx, instance, &combat)?;
        FIGHTS.with(|fights| fights.borrow_mut().insert(instance.id, rebuilt));
    }
    FIGHTS.with(|fights| {
        let mut fights = fights.borrow_mut();
        let fight = fights
            .get_mut(&instance.id)
            .ok_or_else(|| RealmError::dungeon("That dungeon has no fight."))?;
        Ok(body(fight))
    })
}

/// The fight as the rows describe it.
fn rebuild(
    ctx: &ReducerContext,
    instance: &DungeonInstance,
    combat: &InstanceCombat,
) -> Result<CombatInstance, RealmError> {
    let dungeon = crate::level::dungeon(&instance.dungeon_id)?;
    let players = specs(ctx, instance, &dungeon);
    let barriers = barriers_of(&dungeon.scene, &dungeon.definition);
    let state = Restore {
        tick: combat.tick,
        credited: combat.credited,
        unlocked: combat.unlocked,
        monsters: ctx
            .db
            .instance_monster()
            .instance_id()
            .filter(instance.id)
            .map(|row| MonsterRecord {
                key: row.key,
                alive: row.alive,
                health: row.health,
                position: row.position.into(),
            })
            .collect(),
        players: ctx
            .db
            .instance_player()
            .instance_id()
            .filter(instance.id)
            .map(|row| PlayerRecord {
                account_id: row.account_id.clone(),
                body: crate::movement::body_of(&row),
                health: row.health,
                last_seq: row.last_seq,
            })
            .collect(),
    };
    let mut fight = CombatInstance::restore_seeded(
        &dungeon.scene,
        &players,
        &barriers,
        key_scaling(instance, &dungeon),
        instance.barrier_open,
        &state,
        fight_seed(instance.uid),
    );
    arm(ctx, instance, &mut fight);
    apply_run_rules(instance, &mut fight);
    // The rebuilt fight dropped what no row holds (summoned monsters); its first projection
    // writes every row again, which removes them.
    fight.forget_written();
    // The statistics are not in the tables' fight state; the rows the previous module instance
    // projected carry them (to within a second), so the run's totals survive.
    for row in ctx.db.instance_meter().instance_id().filter(instance.id) {
        if let Some(blob) = engine_realm::meter::MeterBlob::from_json(&row.data) {
            fight.meter_mut().seed(&row.account_id, &blob);
        }
    }
    log::warn!(
        "event=combat_rebuilt instance_id={} tick={} monsters={}",
        instance.id,
        combat.tick,
        state.monsters.len()
    );
    Ok(fight)
}

/// Drops an instance's fight and its combat rows.
pub fn forget(ctx: &ReducerContext, instance_id: u64) {
    FIGHTS.with(|fights| fights.borrow_mut().remove(&instance_id));
    ctx.db.instance_combat().instance_id().delete(instance_id);
    let players: Vec<_> = ctx
        .db
        .player_combat()
        .instance_id()
        .filter(instance_id)
        .map(|row| row.identity)
        .collect();
    for identity in players {
        ctx.db.player_combat().identity().delete(identity);
    }
    let batches: Vec<u64> = ctx
        .db
        .combat_batch()
        .instance_id()
        .filter(instance_id)
        .map(|row| row.id)
        .collect();
    for id in batches {
        ctx.db.combat_batch().id().delete(id);
    }
    let meters: Vec<_> = ctx
        .db
        .instance_meter()
        .instance_id()
        .filter(instance_id)
        .map(|row| row.identity)
        .collect();
    for identity in meters {
        ctx.db.instance_meter().identity().delete(identity);
    }
}

/// Lifts the start barrier in the fight.
pub fn open_barrier(ctx: &ReducerContext, instance: &DungeonInstance) {
    if let Err(error) = with_fight(ctx, instance, CombatInstance::open_start_barrier) {
        log::error!(
            "event=combat_barrier_failed instance_id={} error={error}",
            instance.id
        );
    }
}

/// Writes what changed in the fight into the rows.
fn project(ctx: &ReducerContext, instance_id: u64, fight: &mut CombatInstance) {
    let tick = fight.tick();
    // Players: body and health on the movement row when the fight moved them (a charge, a
    // knock-back, a respawn, gravity), the rest on the combat row when it changed.
    for (state, combat_changed) in fight.player_updates() {
        let Some(row) = ctx
            .db
            .instance_player()
            .instance_id()
            .filter(instance_id)
            .find(|row| row.account_id == state.account_id)
        else {
            continue;
        };
        let identity = row.identity;
        let moved = (engine_core::math::Vec3::from(row.position) - state.body.position).length()
            > 1e-3
            || (row.yaw - state.body.facing_yaw).abs() > 1e-3
            || row.grounded != state.body.grounded;
        if moved || row.health != state.health || row.max_health != state.max_health {
            ctx.db.instance_player().identity().update(InstancePlayer {
                health: state.health,
                max_health: state.max_health,
                ..crate::movement::with_body(row, &state.body)
            });
        }
        if combat_changed {
            let combat = PlayerCombat {
                identity,
                instance_id,
                account_id: state.account_id.clone(),
                alive: state.alive,
                target: net_entity(&state.target),
                cast: state.cast.map(net_cast),
                resources: NetResources {
                    heat: state.resources.heat,
                    energy: state.resources.energy,
                    mana: state.resources.mana,
                    max_mana: state.resources.max_mana,
                    valor: state.resources.valor,
                    astral: state.resources.astral,
                    global_cooldown: state.resources.global_cooldown,
                    absorb: state.resources.absorb,
                    trinket1: state.resources.trinket1,
                    trinket2: state.resources.trinket2,
                },
                cooldowns: state
                    .cooldowns
                    .iter()
                    .map(|c| NetCooldown {
                        ability: c.ability,
                        charges: c.charges,
                        recharge: c.recharge,
                    })
                    .collect(),
                statuses: net_statuses(&state.statuses),
                tick,
            };
            if ctx.db.player_combat().identity().find(identity).is_some() {
                ctx.db.player_combat().identity().update(combat);
            } else {
                ctx.db.player_combat().insert(combat);
            }
        }
    }

    // Monsters: changed rows rewritten, new ones (summoned adds) inserted, gone ones deleted.
    let (changed, gone) = fight.changed_monsters();
    if changed.is_empty() && gone.is_empty() {
        return;
    }
    let mut rows: BTreeMap<u64, InstanceMonster> = ctx
        .db
        .instance_monster()
        .instance_id()
        .filter(instance_id)
        .map(|row| (row.key, row))
        .collect();
    for state in changed {
        let row = monster_row(instance_id, &state, rows.remove(&state.key));
        if row.id == 0 {
            ctx.db.instance_monster().insert(row);
        } else {
            ctx.db.instance_monster().id().update(row);
        }
    }
    for key in gone {
        if let Some(row) = rows.remove(&key) {
            ctx.db.instance_monster().id().delete(row.id);
        }
    }
}

fn monster_row(
    instance_id: u64,
    state: &MonsterState,
    existing: Option<InstanceMonster>,
) -> InstanceMonster {
    let (id, pack, scale, level) = existing.as_ref().map_or((0, String::new(), 1.0, 1), |row| {
        (row.id, row.pack.clone(), row.scale, row.level)
    });
    InstanceMonster {
        id,
        instance_id,
        archetype: state.kind.to_owned(),
        name: state.name.clone(),
        pack,
        position: state.position.into(),
        yaw: state.yaw,
        scale,
        level,
        health: state.health,
        max_health: state.max_health,
        key: state.key,
        alive: state.alive,
        target: net_entity(&state.target),
        cast: state.cast.map(net_cast),
        statuses: net_statuses(&state.statuses),
    }
}

/// Fills in the level's own data for the first projection of placed monsters: pack, size
/// and level come from the placement, which the fight does not carry.
pub fn decorate_placed(ctx: &ReducerContext, instance_id: u64, dungeon: &crate::level::Dungeon) {
    let placements: Vec<_> = dungeon
        .scene
        .monsters
        .iter()
        .filter(|m| m.enabled && engine_game::enemy::EnemyKind::from_id(&m.monster).is_some())
        .collect();
    let rows: Vec<InstanceMonster> = ctx
        .db
        .instance_monster()
        .instance_id()
        .filter(instance_id)
        .collect();
    for row in rows {
        // Placed monsters are keyed in spawn order, which is pack order; match by kind and
        // nearest placement rather than trusting the two orders to agree.
        let Some(placement) = placements
            .iter()
            .filter(|p| p.monster == row.archetype)
            .min_by(|a, b| {
                let da = (engine_core::math::Vec3::from(row.position) - a.position).length();
                let db = (engine_core::math::Vec3::from(row.position) - b.position).length();
                da.total_cmp(&db)
            })
        else {
            continue;
        };
        ctx.db.instance_monster().id().update(InstanceMonster {
            pack: placement.pack.clone().unwrap_or_default(),
            scale: placement.scale,
            level: u32::from(placement.level),
            ..row
        });
    }
}

/// Steps every live fight to the present and publishes what happened.
#[spacetimedb::reducer]
pub fn combat_tick(ctx: &ReducerContext, _timer: CombatTimer) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err(RealmError::unauthorized("That operation is not available.").into());
    }
    let now = micros(ctx.timestamp);
    for instance in ctx.db.dungeon_instance().live().filter(true) {
        let Some(combat) = ctx.db.instance_combat().instance_id().find(instance.id) else {
            continue;
        };
        let due = u64::try_from((now - micros(combat.origin)) / TICK_US).unwrap_or(0);
        if due <= combat.tick {
            continue;
        }
        let run = (due - combat.tick).min(MAX_TICKS_PER_PASS);
        // Fallen behind: let the fight run slow instead of catching up later.
        let origin = if due - combat.tick > MAX_TICKS_PER_PASS {
            timestamp(now - i64::try_from(combat.tick + run).unwrap_or(0) * TICK_US)
        } else {
            combat.origin
        };
        let outcome = with_fight(ctx, &instance, |fight| {
            let events = fight.advance(u32::try_from(run).unwrap_or(0));
            project(ctx, instance.id, fight);
            // Recount: at most once a second, and only rows whose text changed.
            let meter_rows = fight.meter_rows(now, false);
            (
                events,
                fight.tick(),
                fight.credited(),
                fight.unlocked(),
                fight.completed(),
                meter_rows,
            )
        });
        let (events, tick, credited, unlocked, completed, meter_rows) = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                log::error!(
                    "event=combat_tick_failed instance_id={} error={error}",
                    instance.id
                );
                continue;
            }
        };
        write_meter(ctx, instance.id, meter_rows);
        if !events.is_empty() {
            match serde_json::to_string(&events) {
                Ok(json) => {
                    ctx.db.combat_batch().insert(CombatBatch {
                        id: 0,
                        instance_id: instance.id,
                        tick,
                        created_at: ctx.timestamp,
                        events: json,
                    });
                }
                Err(error) => log::error!(
                    "event=combat_batch_unencodable instance_id={} error={error}",
                    instance.id
                ),
            }
        }
        ctx.db
            .instance_combat()
            .instance_id()
            .update(InstanceCombat {
                tick,
                origin,
                credited,
                unlocked,
                ..combat
            });
        // Monster drops are durable the moment they fall, and before `finish` reads them: the
        // boss's gold is in the batch that completes the run.
        crate::loot::record_drops(ctx, instance.id, &events);
        if let Some(seconds) = completed {
            if InstanceState::from(instance.state) == InstanceState::Active {
                log::info!(
                    "event=dungeon_cleared instance_id={} seconds={seconds:.1}",
                    instance.id
                );
                crate::instance::finish(ctx, instance.id, true)?;
            }
        }
        // After `finish`, which writes the rolls these events settle.
        crate::loot::on_events(ctx, instance.id, &events);
    }
    let stale: Vec<u64> = ctx
        .db
        .combat_batch()
        .iter()
        .filter(|batch| now - micros(batch.created_at) > BATCH_TTL_US)
        .map(|batch| batch.id)
        .collect();
    for id in stale {
        ctx.db.combat_batch().id().delete(id);
    }
    Ok(())
}

/// A button press: an ability, or only a change of target.
#[spacetimedb::reducer]
pub fn combat_act(
    ctx: &ReducerContext,
    target: NetEntity,
    action: NetAction,
) -> Result<(), String> {
    rate_limit(ctx, Operation::Combat)?;
    let player = ctx
        .db
        .instance_player()
        .identity()
        .find(ctx.sender())
        .ok_or_else(|| RealmError::dungeon("You are not in a dungeon.").to_wire())?;
    let instance = ctx
        .db
        .dungeon_instance()
        .id()
        .find(player.instance_id)
        .ok_or_else(|| RealmError::dungeon("That dungeon no longer exists.").to_wire())?;
    let state: InstanceState = instance.state.into();
    let only_target = matches!(action, NetAction::Target);
    // The chest is opened after the run completes; everything else needs a live fight.
    let allowed = match state {
        InstanceState::Active => true,
        InstanceState::Countdown => only_target,
        InstanceState::Completed => matches!(action, NetAction::OpenChest),
        _ => false,
    };
    if !allowed {
        return Err(RealmError::dungeon("You cannot act yet.").into());
    }
    let kind = match action {
        NetAction::Target => ActionKind::Target,
        NetAction::Ability(slot) => ActionKind::Ability(slot),
        NetAction::AbilityAt(aimed) => ActionKind::AbilityAt(
            aimed.slot,
            [aimed.position.x, aimed.position.y, aimed.position.z],
        ),
        NetAction::Ultimate(p) => ActionKind::Ultimate([p.x, p.y, p.z]),
        NetAction::Extra(index) => ActionKind::Extra(index),
        NetAction::OpenChest => ActionKind::OpenChest,
        NetAction::ChannelAim(p) => ActionKind::ChannelAim([p.x, p.y, p.z]),
        NetAction::Trinket(index) => ActionKind::Trinket(index),
    };
    let request = CombatAction {
        target: entity_ref(&target),
        kind,
    };
    let result = with_fight(ctx, &instance, |fight| {
        fight.act(&player.account_id, &request)
    })?;
    result.map_err(|error| match error {
        ActionError::NotInFight => RealmError::dungeon("You are not in this fight.").to_wire(),
        ActionError::Malformed => RealmError::protocol("That action is not understood.").to_wire(),
    })
}

/// When a fight tick is due, for tests and logs.
#[allow(dead_code, reason = "diagnostics")]
pub fn due_at(combat: &InstanceCombat) -> Timestamp {
    timestamp(micros(combat.origin) + i64::try_from(combat.tick).unwrap_or(0) * TICK_US)
}
