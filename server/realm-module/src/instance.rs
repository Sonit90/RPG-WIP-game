//! Dungeon instances: creation, countdown, barrier, idle physics, and the janitor that cleans
//! up whatever nobody can come back to.
//!
//! An instance is created by the leader's `party_start` and owns its players' runtime rows and
//! its monsters. It shares nothing with any other instance: every query here starts from an
//! instance id, and every view a client reads starts from the caller's own membership.

use engine_realm::RealmError;
use engine_realm::meter::ReportMember;
use engine_realm::instance::{
    Countdown, InstanceState, MemberPresence, barrier_closed, should_clean_up,
};
use engine_realm::party::PartyState;
use spacetimedb::{ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::party::{Loaded, depart, load, transition};
use crate::schema::*;
use crate::util::{config, instance_config, micros, notify, timestamp};

/// Fires when a countdown ends.
#[spacetimedb::table(accessor = countdown_timer, scheduled(countdown_finish))]
pub struct CountdownTimer {
    /// Row id.
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    /// When the barrier lifts.
    pub scheduled_at: ScheduleAt,
    /// Which instance.
    pub instance_id: u64,
}

/// Drives [`janitor`].
#[spacetimedb::table(accessor = janitor_timer, scheduled(janitor))]
pub struct JanitorTimer {
    /// Row id.
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    /// Every two seconds.
    pub scheduled_at: ScheduleAt,
}

/// Starts the periodic passes.
pub fn schedule(ctx: &ReducerContext) {
    ctx.db.janitor_timer().insert(JanitorTimer {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(2 * engine_realm::SECOND)),
    });
}

/// Moves an instance to `next`, refusing what the lifecycle forbids.
fn advance(
    ctx: &ReducerContext,
    instance: &DungeonInstance,
    next: InstanceState,
) -> Result<DungeonInstance, RealmError> {
    let current: InstanceState = instance.state.into();
    if !current.can_become(next) {
        log::error!(
            "event=instance_transition_refused instance_id={} from={} to={}",
            instance.id,
            current.key(),
            next.key()
        );
        return Err(RealmError::dungeon("The dungeon cannot do that right now."));
    }
    let updated = DungeonInstance {
        state: next.into(),
        // A completed run keeps ticking so the boss's chest can fall and be opened; it stops
        // when the janitor tears the instance down. A failed run stops at once.
        live: matches!(
            next,
            InstanceState::Countdown | InstanceState::Active | InstanceState::Completed
        ),
        barrier_open: !barrier_closed(next),
        ..instance.clone()
    };
    ctx.db.dungeon_instance().id().update(updated.clone());
    log::info!(
        "event=instance_state instance_id={} state={}",
        instance.id,
        next.key()
    );
    Ok(updated)
}

/// Builds a party's instance and starts its countdown.
pub fn create(ctx: &ReducerContext, loaded: &Loaded) -> Result<(), RealmError> {
    let dungeon = crate::level::dungeon(&loaded.party.dungeon_id)?;
    // The key is checked again here: the party's state says it was inserted, the rules say
    // whether it still opens this dungeon.
    let owner = loaded
        .party
        .key_owner
        .and_then(|identity| loaded.members.iter().find(|m| m.identity == identity))
        .ok_or_else(|| RealmError::party("No key has been inserted."))?;
    engine_realm::key::validate_key(
        &dungeon.definition,
        &owner.key_dungeon_id,
        loaded.party.key_level,
    )?;
    let party = transition(ctx, &loaded.party, PartyState::Starting)?;

    let instance = ctx
        .db
        .dungeon_instance()
        .try_insert(DungeonInstance {
            id: 0,
            party_id: party.id,
            dungeon_id: party.dungeon_id.clone(),
            key_level: party.key_level,
            state: InstanceStateTag::Created,
            live: false,
            countdown_ends_at: None,
            barrier_open: false,
            created_at: ctx.timestamp,
            finished_at: None,
        })
        .map_err(|_| RealmError::dungeon("This party already has a dungeon."))?;
    ctx.db.party().id().update(Party {
        instance_id: Some(instance.id),
        ..party
    });
    let instance = advance(ctx, &instance, InstanceState::Loading)?;

    // The party, behind the barrier, at the dungeon's start points in party order.
    let max_health =
        engine_game::character::CharacterSheet::bare(engine_game::hero::Hero::Warrior).max_health();
    let catalog = crate::level::catalog()?;
    for member in &loaded.members {
        let point = dungeon.definition.spawn_point(usize::from(member.slot));
        let health = catalog
            .character(&member.definition_id)
            .map_or(max_health, |definition| {
                engine_game::character::CharacterSheet::starting(definition.hero).max_health()
            });
        ctx.db.instance_player().insert(InstancePlayer {
            identity: member.identity,
            instance_id: instance.id,
            account_id: member.account_id.clone(),
            name: member.character_name.clone(),
            definition_id: member.definition_id.clone(),
            role: member.role,
            position: V3 {
                x: point.position[0],
                y: point.position[1],
                z: point.position[2],
            },
            velocity: V3::default(),
            ground_normal: V3 {
                x: 0.0,
                y: 1.0,
                z: 0.0,
            },
            yaw: point.yaw,
            grounded: true,
            last_seq: 0,
            last_input_at: ctx.timestamp,
            budget_tokens: engine_realm::movement::InputBudget::BURST,
            budget_refilled_at: ctx.timestamp,
            health,
            max_health: health,
            presence: PresenceTag::Present,
        });
    }

    // The fight: the real simulation with the party in it. Its first projection writes the
    // monsters' rows and every player's health.
    crate::combat::start(ctx, &instance)?;
    crate::combat::decorate_placed(ctx, instance.id, &dungeon);
    let instance = advance(ctx, &instance, InstanceState::Ready)?;

    let countdown =
        Countdown::starting(micros(ctx.timestamp), dungeon.definition.countdown_seconds);
    let instance = advance(ctx, &instance, InstanceState::Countdown)?;
    ctx.db.dungeon_instance().id().update(DungeonInstance {
        countdown_ends_at: Some(timestamp(countdown.ends_at)),
        ..instance.clone()
    });
    ctx.db.countdown_timer().insert(CountdownTimer {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(timestamp(countdown.ends_at)),
        instance_id: instance.id,
    });
    log::info!(
        "event=instance_created instance_id={} party_id={} dungeon_id={} players={}",
        instance.id,
        loaded.party.id,
        loaded.party.dungeon_id,
        loaded.members.len()
    );
    Ok(())
}

/// The countdown ended: lift the barrier, start the run.
#[spacetimedb::reducer]
pub fn countdown_finish(ctx: &ReducerContext, timer: CountdownTimer) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err(RealmError::unauthorized("That operation is not available.").into());
    }
    let Some(instance) = ctx.db.dungeon_instance().id().find(timer.instance_id) else {
        return Ok(());
    };
    if instance.state != InstanceStateTag::Countdown {
        return Ok(());
    }
    let instance = advance(ctx, &instance, InstanceState::Active)?;
    crate::combat::open_barrier(ctx, &instance);
    if let Some(party) = ctx.db.party().id().find(instance.party_id) {
        transition(ctx, &party, PartyState::InDungeon)?;
    }
    for player in ctx.db.instance_player().instance_id().filter(instance.id) {
        notify(
            ctx,
            player.identity,
            "barrier_open",
            NoticeLevel::Info,
            "The barrier lifts. Go!",
        );
    }
    log::info!(
        "event=instance_start instance_id={} dungeon_id={}",
        instance.id,
        instance.dungeon_id
    );
    Ok(())
}

/// Tears an instance down: its rows go, its party is told.
pub fn destroy(ctx: &ReducerContext, instance_id: u64, reason: &str) {
    let Some(instance) = ctx.db.dungeon_instance().id().find(instance_id) else {
        return;
    };
    // Chest items nobody took go to their owners' mail.
    crate::loot::mail_unclaimed(ctx, instance_id);
    if let Ok(cleaning) = advance(ctx, &instance, InstanceState::CleaningUp) {
        let _ = advance(ctx, &cleaning, InstanceState::Destroyed);
    }
    let players: Vec<InstancePlayer> = ctx
        .db
        .instance_player()
        .instance_id()
        .filter(instance_id)
        .collect();
    for player in players {
        ctx.db.instance_player().identity().delete(player.identity);
    }
    let monsters: Vec<u64> = ctx
        .db
        .instance_monster()
        .instance_id()
        .filter(instance_id)
        .map(|monster| monster.id)
        .collect();
    for id in monsters {
        ctx.db.instance_monster().id().delete(id);
    }
    let summaries: Vec<String> = ctx
        .db
        .run_summary()
        .instance_id()
        .filter(instance_id)
        .map(|summary| summary.result_id)
        .collect();
    for id in summaries {
        ctx.db.run_summary().result_id().delete(&id);
    }
    crate::combat::forget(ctx, instance_id);
    ctx.db.dungeon_instance().id().delete(instance_id);
    log::info!("event=instance_cleanup instance_id={instance_id} reason={reason}");
}

/// Ends a run: records each player's result in the outbox for the persistence bridge.
///
/// What a run is worth is decided here, from server state. Nothing a client sends reaches it.
pub fn finish(ctx: &ReducerContext, instance_id: u64, success: bool) -> Result<(), RealmError> {
    let instance = ctx
        .db
        .dungeon_instance()
        .id()
        .find(instance_id)
        .ok_or_else(|| RealmError::dungeon("That dungeon no longer exists."))?;
    let next = if success {
        InstanceState::Completed
    } else {
        InstanceState::Failed
    };
    let instance = advance(ctx, &instance, next)?;
    ctx.db.dungeon_instance().id().update(DungeonInstance {
        finished_at: Some(ctx.timestamp),
        ..instance.clone()
    });
    if let Some(party) = ctx.db.party().id().find(instance.party_id) {
        let next = if success {
            PartyState::Completed
        } else {
            PartyState::Failed
        };
        transition(ctx, &party, next)?;
    }
    let duration = micros(ctx.timestamp) - micros(instance.created_at);
    let dungeon = crate::level::dungeon(&instance.dungeon_id)?;
    let key_owner = ctx
        .db
        .party()
        .id()
        .find(instance.party_id)
        .and_then(|party| party.key_owner);
    // Each player's roll from the boss's chest, read before the fight can change. Nothing for
    // a failed run, or one ended without a chest (an admin finish).
    let rolls: Vec<(String, String)> = if success {
        let accounts: Vec<String> = ctx
            .db
            .instance_player()
            .instance_id()
            .filter(instance_id)
            .map(|player| player.account_id)
            .collect();
        crate::combat::with_fight(ctx, &instance, |fight| {
            accounts
                .iter()
                .filter_map(|account| {
                    fight
                        .loot_of(account)
                        .map(|key| (account.clone(), key.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    // Recount: the statistics freeze here, and each player's report is built from what the
    // server's simulation saw (aggregates only), then travels with their result.
    let catalog = crate::level::catalog().ok();
    let party: Vec<ReportMember> = ctx
        .db
        .instance_player()
        .instance_id()
        .filter(instance_id)
        .map(|player| ReportMember {
            class: catalog
                .as_ref()
                .and_then(|c| c.character(&player.definition_id))
                .map_or_else(|| player.definition_id.clone(), |d| d.class.clone()),
            role: engine_realm::Role::from(player.role).key().to_owned(),
            account_id: player.account_id,
            character_name: player.name,
        })
        .collect();
    let (reports, meter_rows) = crate::combat::with_fight(ctx, &instance, |fight| {
        let now = fight.time();
        fight.meter_mut().finish(now);
        let rows = fight.meter_rows(micros(ctx.timestamp), true);
        let reports: Vec<(String, String)> = party
            .iter()
            .map(|member| {
                let report = fight
                    .meter()
                    .damage_report(&member.account_id, party.clone())
                    .map(|report| report.to_json())
                    .unwrap_or_default();
                (member.account_id.clone(), report)
            })
            .collect();
        (reports, rows)
    })
    .unwrap_or_default();
    crate::combat::write_meter(ctx, instance_id, meter_rows);
    for player in ctx.db.instance_player().instance_id().filter(instance_id) {
        let member = ctx.db.party_member().identity().find(player.identity);
        let result_id = format!("{instance_id}:{}", player.account_id);
        if ctx
            .db
            .pending_result()
            .result_id()
            .find(&result_id)
            .is_some()
        {
            continue;
        }
        let mut loot_item = String::new();
        if let Some((_, key)) = rolls
            .iter()
            .find(|(account, _)| *account == player.account_id)
        {
            let character = member.as_ref().map(|m| m.character_id.clone());
            crate::loot::roll(
                ctx,
                instance_id,
                &player.account_id,
                character.unwrap_or_default(),
                key,
            );
            loot_item.clone_from(key);
        }
        let is_owner = key_owner == Some(player.identity);
        let key_level = instance.key_level.max(1);
        let rewards =
            engine_realm::rewards::run_rewards(&dungeon.definition, key_level, success, is_owner);
        let experience = rewards.experience;
        let key_level_after = rewards.key_level_after.unwrap_or(0);
        let resources =
            serde_json::to_string(&rewards.resources).unwrap_or_else(|_| "[]".to_owned());
        ctx.db.pending_result().insert(PendingResult {
            result_id,
            account_id: player.account_id.clone(),
            character_id: member.map(|m| m.character_id).unwrap_or_default(),
            dungeon_id: instance.dungeon_id.clone(),
            instance_id,
            success,
            duration_us: duration,
            experience,
            created_at: ctx.timestamp,
            key_level,
            key_owner: is_owner,
            key_level_after,
            resources: resources.clone(),
            damage_report: reports
                .iter()
                .find(|(account, _)| *account == player.account_id)
                .map(|(_, report)| report.clone())
                .unwrap_or_default(),
        });
        ctx.db.run_summary().insert(RunSummary {
            result_id: format!("{instance_id}:{}", player.account_id),
            identity: player.identity,
            instance_id,
            dungeon_id: instance.dungeon_id.clone(),
            dungeon_name: dungeon.definition.name.clone(),
            success,
            key_level,
            key_owner: is_owner,
            key_level_after,
            duration_us: duration,
            experience,
            resources,
            loot_item,
            created_at: ctx.timestamp,
        });
        notify(
            ctx,
            player.identity,
            if success {
                "dungeon_completed"
            } else {
                "dungeon_failed"
            },
            if success {
                NoticeLevel::Info
            } else {
                NoticeLevel::Warning
            },
            if success {
                if key_level_after > 0 {
                    format!(
                        "Dungeon complete! +{experience} experience. Key upgraded to level {key_level_after}."
                    )
                } else {
                    format!("Dungeon complete! +{experience} experience.")
                }
            } else {
                "The run has failed.".to_owned()
            },
        );
    }
    log::info!(
        "event=instance_completion instance_id={instance_id} dungeon_id={} success={success} duration_us={duration}",
        instance.dungeon_id
    );
    Ok(())
}

/// Expires what has waited too long: queue tickets of the departed, members past their
/// reconnect grace, instances nobody can return to, old notices and rate-limit rows.
#[spacetimedb::reducer]
pub fn janitor(ctx: &ReducerContext, _timer: JanitorTimer) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err(RealmError::unauthorized("That operation is not available.").into());
    }
    let config = config(ctx)?;
    let now = micros(ctx.timestamp);
    let disconnected_since = |identity| {
        ctx.db
            .account()
            .identity()
            .find(identity)
            .and_then(|account: Account| account.disconnected_at)
            .map(micros)
    };

    // Searching tickets whose player has been gone longer than the queue grace.
    let stale: Vec<QueueTicket> = ctx
        .db
        .queue_ticket()
        .iter()
        .filter(|ticket| ticket.status == TicketStatus::Searching)
        .filter(|ticket| {
            disconnected_since(ticket.identity)
                .is_some_and(|since| now - since >= config.queue_disconnect_grace_us)
        })
        .collect();
    for ticket in stale {
        ctx.db.queue_ticket().id().delete(ticket.id);
        log::info!(
            "event=queue_leave reason=disconnected account_id={} dungeon_id={}",
            ticket.account_id,
            ticket.dungeon_id
        );
    }

    // Members past their reconnect grace leave their party.
    let expired: Vec<PartyMember> = ctx
        .db
        .party_member()
        .iter()
        .filter(|member| {
            disconnected_since(member.identity)
                .is_some_and(|since| now - since >= config.reconnect_grace_us)
        })
        .collect();
    for member in expired {
        if let Some(loaded) = load(ctx, member.party_id) {
            if loaded.party.state == PartyStateTag::ReadyCheck {
                crate::party::resolve_ready_check(ctx, member.party_id);
            } else {
                depart(ctx, &loaded, &member, "lost connection and left");
            }
        }
    }

    // Instances nobody can return to.
    let instances: Vec<DungeonInstance> = ctx.db.dungeon_instance().iter().collect();
    let rules = instance_config(&config);
    for instance in instances {
        let members: Vec<MemberPresence> = ctx
            .db
            .instance_player()
            .instance_id()
            .filter(instance.id)
            .map(|player| MemberPresence {
                disconnected_at: disconnected_since(player.identity),
            })
            .collect();
        if should_clean_up(
            instance.state.into(),
            &members,
            instance.finished_at.map(micros),
            now,
            &rules,
        ) {
            let party_id = instance.party_id;
            destroy(ctx, instance.id, "abandoned_or_finished");
            if let Some(loaded) = load(ctx, party_id) {
                for member in &loaded.members {
                    ctx.db.party_member().identity().delete(member.identity);
                }
                ctx.db.party().id().delete(party_id);
                log::info!("event=party_disbanded party_id={party_id} reason=instance_closed");
            }
        }
    }

    // Old notices and idle rate-limit buckets.
    let old_notices: Vec<u64> = ctx
        .db
        .player_notice()
        .iter()
        .filter(|notice| now - micros(notice.created_at) >= config.notice_ttl_us)
        .map(|notice| notice.id)
        .collect();
    for id in old_notices {
        ctx.db.player_notice().id().delete(id);
    }
    crate::chat::prune(ctx);
    let idle_buckets: Vec<String> = ctx
        .db
        .rate_limit()
        .iter()
        .filter(|bucket| now - micros(bucket.refilled_at) >= 600 * engine_realm::SECOND)
        .map(|bucket| bucket.key)
        .collect();
    for key in idle_buckets {
        ctx.db.rate_limit().key().delete(&key);
    }
    Ok(())
}
