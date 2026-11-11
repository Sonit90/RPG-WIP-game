//! Operator entry points: configuration after a deploy, and the clock procedure every client
//! uses to agree with the server about when a countdown ends.
//!
//! `admin_*` reducers accept only the identity that published the module (recorded by `init`),
//! so they are reachable with `spacetime call` by whoever holds the deploy credentials and by
//! nobody else.

use engine_realm::RealmError;
use spacetimedb::ReducerContext;

use crate::schema::*;
use crate::util::config;

/// Refuses anyone but the owner.
fn require_owner(ctx: &ReducerContext) -> Result<RealmConfig, RealmError> {
    let config = config(ctx)?;
    if ctx.sender() != config.owner {
        log::warn!(
            "event=admin_call_refused identity={}",
            ctx.sender().to_abbreviated_hex()
        );
        return Err(RealmError::unauthorized("That operation is not available."));
    }
    Ok(config)
}

/// Points the realm at an issuer: which tokens it accepts and who the bridge is.
#[spacetimedb::reducer]
pub fn admin_configure(
    ctx: &ReducerContext,
    issuer: String,
    audience: String,
    service_subject: String,
) -> Result<(), String> {
    let config = require_owner(ctx)?;
    if issuer.is_empty() || audience.is_empty() || service_subject.is_empty() {
        return Err(
            RealmError::protocol("Issuer, audience and service subject are required.").into(),
        );
    }
    log::info!("event=realm_configured issuer={issuer} audience={audience}");
    ctx.db.realm_config().id().update(RealmConfig {
        issuer,
        audience,
        service_subject,
        ..config
    });
    Ok(())
}

/// Tunes matchmaking on a live realm.
#[spacetimedb::reducer]
pub fn admin_set_matchmaking(
    ctx: &ReducerContext,
    min_tolerance: f64,
    max_tolerance: f64,
    expand_start_seconds: u32,
    expand_end_seconds: u32,
) -> Result<(), String> {
    let config = require_owner(ctx)?;
    let candidate = engine_realm::matchmaking::MatchmakingConfig {
        min_tolerance,
        max_tolerance,
        expand_start: i64::from(expand_start_seconds) * engine_realm::SECOND,
        expand_end: i64::from(expand_end_seconds) * engine_realm::SECOND,
        ..Default::default()
    };
    if !candidate.is_valid() {
        return Err(RealmError::protocol("That matchmaking configuration is not valid.").into());
    }
    log::info!(
        "event=matchmaking_configured min={min_tolerance} max={max_tolerance} start_s={expand_start_seconds} end_s={expand_end_seconds}"
    );
    ctx.db.realm_config().id().update(RealmConfig {
        mm_min_tolerance: min_tolerance,
        mm_max_tolerance: max_tolerance,
        mm_expand_start_us: candidate.expand_start,
        mm_expand_end_us: candidate.expand_end,
        ..config
    });
    Ok(())
}

/// Tunes the timeouts on a live realm.
#[spacetimedb::reducer]
pub fn admin_set_timeouts(
    ctx: &ReducerContext,
    ready_check_seconds: u32,
    reconnect_grace_seconds: u32,
    queue_disconnect_grace_seconds: u32,
) -> Result<(), String> {
    let config = require_owner(ctx)?;
    if ready_check_seconds == 0 {
        return Err(RealmError::protocol("A ready check needs some time.").into());
    }
    let second = engine_realm::SECOND;
    ctx.db.realm_config().id().update(RealmConfig {
        ready_check_timeout_us: i64::from(ready_check_seconds) * second,
        reconnect_grace_us: i64::from(reconnect_grace_seconds) * second,
        queue_disconnect_grace_us: i64::from(queue_disconnect_grace_seconds) * second,
        ..config
    });
    log::info!(
        "event=timeouts_configured ready_check_s={ready_check_seconds} reconnect_grace_s={reconnect_grace_seconds} queue_grace_s={queue_disconnect_grace_seconds}"
    );
    Ok(())
}

/// Ends an instance as completed or failed. An operator tool, and the integration tests' way
/// to exercise the result pipeline without clearing the dungeon; a real run completes from
/// `combat_tick` when the fight's dungeon run does.
#[spacetimedb::reducer]
pub fn admin_finish_instance(
    ctx: &ReducerContext,
    instance_id: u64,
    success: bool,
) -> Result<(), String> {
    require_owner(ctx)?;
    crate::instance::finish(ctx, instance_id, success)?;
    Ok(())
}

/// Kills every living monster of an instance. An operator tool and the integration tests' way
/// to clear a dungeon: unlike `admin_finish_instance` the run then completes through the
/// ordinary ticks, so the boss's chest and every drop are the real ones. Call it again until
/// the instance is completed (the boss's gate opens when the rest are dead).
#[spacetimedb::reducer]
pub fn admin_clear_instance(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    require_owner(ctx)?;
    let instance = ctx
        .db
        .dungeon_instance()
        .id()
        .find(instance_id)
        .ok_or_else(|| RealmError::dungeon("That dungeon no longer exists.").to_wire())?;
    let events = crate::combat::with_fight(ctx, &instance, |fight| fight.admin_kill_all())
        .map_err(|error| error.to_wire())?;
    crate::loot::record_drops(ctx, instance_id, &events);
    Ok(())
}

/// Slays placed monsters in an instance's fight until `percent` of them are dead (the boss and
/// summons are left). An operator tool, and the integration tests' way to reach the key
/// reroll's unlock without clearing half a dungeon.
#[spacetimedb::reducer]
pub fn admin_clear_dungeon(
    ctx: &ReducerContext,
    instance_id: u64,
    percent: u8,
) -> Result<(), String> {
    require_owner(ctx)?;
    let instance = ctx
        .db
        .dungeon_instance()
        .id()
        .find(instance_id)
        .ok_or_else(|| RealmError::dungeon("That dungeon no longer exists."))?;
    let slain = crate::combat::with_fight(ctx, &instance, |fight| {
        fight.slay_placed_until(f32::from(percent.min(100)) / 100.0)
    })?;
    log::info!(
        "event=admin_clear_dungeon instance_id={instance_id} percent={percent} slain={slain}"
    );
    Ok(())
}

/// Sets how much of the reroll currency a player may spend in an instance. A test hook: the
/// persistence API debits `min(spent, held)`, so granting more than the character holds costs
/// nothing real.
#[spacetimedb::reducer]
pub fn admin_grant_spend(
    ctx: &ReducerContext,
    instance_id: u64,
    account_id: String,
    balance: u64,
) -> Result<(), String> {
    require_owner(ctx)?;
    let id = crate::modifiers::spend_id(instance_id, &account_id);
    let spend = ctx
        .db
        .instance_spend()
        .spend_id()
        .find(&id)
        .ok_or_else(|| RealmError::dungeon("That player has no spend row there."))?;
    ctx.db
        .instance_spend()
        .spend_id()
        .update(InstanceSpend { balance, ..spend });
    Ok(())
}

/// The server's clock, in microseconds since the Unix epoch.
///
/// A client calls this a few times, keeps the round trip with the least delay, and from it
/// estimates the offset between its clock and the server's — which is how three clients show
/// the same countdown number at the same moment.
#[spacetimedb::procedure]
pub fn server_time(ctx: &mut spacetimedb::ProcedureContext) -> i64 {
    ctx.timestamp.to_micros_since_unix_epoch()
}

/// Dismisses one of the caller's notices.
#[spacetimedb::reducer]
pub fn dismiss_notice(ctx: &ReducerContext, notice_id: u64) -> Result<(), String> {
    if let Some(notice) = ctx.db.player_notice().id().find(notice_id)
        && notice.identity == ctx.sender()
    {
        ctx.db.player_notice().id().delete(notice_id);
    }
    Ok(())
}

/// The operator's view of the live realm, as JSON text (`engine_realm::ops::AdminSnapshot`).
///
/// For the bridge only; anyone else receives an empty string. Read-only and on demand: it
/// walks the tables once and builds nothing a tick depends on, so it costs nothing while
/// nobody asks. The persistence API's `/admin/ops` endpoints are the only callers.
#[spacetimedb::procedure]
pub fn svc_admin_snapshot(ctx: &mut spacetimedb::ProcedureContext) -> String {
    ctx.with_tx(|tx| {
        if crate::service::require_service(tx).is_err() {
            return String::new();
        }
        serde_json::to_string(&engine_realm::ops::build_snapshot(&snapshot_input(tx)))
            .unwrap_or_default()
    })
}

/// A key's modifiers as the dashboard shows them: `Plague Explosion +17%`.
fn modifier_names(json: &str) -> Vec<String> {
    let catalog = crate::level::catalog().ok();
    engine_realm::modifiers::decode_modifiers(json)
        .into_iter()
        .map(|modifier| {
            let name = catalog
                .as_ref()
                .and_then(|catalog| catalog.modifiers().modifier(&modifier.id))
                .map_or_else(|| modifier.id.clone(), |definition| definition.name.clone());
            format!(
                "{name} {}",
                engine_realm::modifiers::magnitude_text(modifier.magnitude_pct)
            )
        })
        .collect()
}

/// Copies the tables into the builder's plain rows.
fn snapshot_input(ctx: &ReducerContext) -> engine_realm::ops::SnapshotInput {
    use engine_realm::Role;
    use engine_realm::instance::InstanceState;
    use engine_realm::ops::*;
    use engine_realm::party::PartyState;
    use spacetimedb::Table;
    use std::collections::BTreeMap;

    let micros = crate::util::micros;

    let online_accounts = u32::try_from(
        ctx.db
            .account()
            .iter()
            .filter(|a| a.connections > 0)
            .count(),
    )
    .unwrap_or(u32::MAX);

    let mut queue: BTreeMap<String, QueueCount> = BTreeMap::new();
    for ticket in ctx.db.queue_ticket().iter() {
        let count = queue
            .entry(ticket.dungeon_id.clone())
            .or_insert_with(|| QueueCount {
                dungeon_id: ticket.dungeon_id.clone(),
                ..QueueCount::default()
            });
        if ticket.status == TicketStatus::Matched {
            count.matched += 1;
            continue;
        }
        match ticket.role {
            RoleTag::Tank => count.tanks += 1,
            RoleTag::Healer => count.healers += 1,
            RoleTag::Dps => count.dps += 1,
        }
    }

    let party_states = ctx
        .db
        .party()
        .iter()
        .map(|party| PartyState::from(party.state).key().to_owned())
        .collect();

    let instances = ctx
        .db
        .dungeon_instance()
        .iter()
        .map(|instance| {
            let combat = ctx.db.instance_combat().instance_id().find(instance.id);
            let players = ctx
                .db
                .instance_player()
                .instance_id()
                .filter(instance.id)
                .map(|p| PlayerRow {
                    alive: ctx
                        .db
                        .player_combat()
                        .identity()
                        .find(p.identity)
                        .map(|c| c.alive),
                    account_id: p.account_id,
                    name: p.name,
                    definition_id: p.definition_id,
                    role: Role::from(p.role).key().to_owned(),
                    present: p.presence == PresenceTag::Present,
                    x: p.position.x,
                    y: p.position.y,
                    z: p.position.z,
                    health: p.health,
                    max_health: p.max_health,
                })
                .collect();
            let members = ctx
                .db
                .party_member()
                .party_id()
                .filter(instance.party_id)
                .map(|m| MemberRow {
                    account_id: m.account_id,
                    character_id: m.character_id,
                    level: m.level,
                    gear_score: m.gear_score,
                    loadout: m.loadout,
                })
                .collect();
            let monsters = ctx
                .db
                .instance_monster()
                .instance_id()
                .filter(instance.id)
                .map(|m| MonsterRow {
                    key: m.key,
                    is_boss: engine_game::enemy::EnemyKind::from_id(&m.archetype)
                        .is_some_and(|kind| kind.is_boss()),
                    kind: m.archetype,
                    name: m.name,
                    x: m.position.x,
                    z: m.position.z,
                    alive: m.alive,
                })
                .collect();
            InstanceInput {
                row: InstanceRow {
                    id: instance.id,
                    party_id: instance.party_id,
                    dungeon_id: instance.dungeon_id.clone(),
                    key_level: instance.key_level,
                    state: InstanceState::from(instance.state).key().to_owned(),
                    created_at_us: micros(instance.created_at),
                    run_starts_at_us: instance.countdown_ends_at.map(micros),
                    finished_at_us: instance.finished_at.map(micros),
                },
                modifiers: modifier_names(&instance.modifiers),
                live: instance.live,
                players,
                members,
                monsters,
                combat: combat.map(|c| CombatRow {
                    tick: c.tick,
                    origin_us: micros(c.origin),
                }),
            }
        })
        .collect();

    SnapshotInput {
        now_us: micros(ctx.timestamp),
        online_accounts,
        queue: queue.into_values().collect(),
        party_states,
        instances,
    }
}
