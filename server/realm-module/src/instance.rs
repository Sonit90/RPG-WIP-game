//! Dungeon instances: creation, countdown, barrier, idle physics, and the janitor that cleans
//! up whatever nobody can come back to.
//!
//! An instance is created by the leader's `party_start` and owns its players' runtime rows and
//! its monsters. It shares nothing with any other instance: every query here starts from an
//! instance id, and every view a client reads starts from the caller's own membership.

use engine_realm::RealmError;
use engine_realm::api::RunMember;
use engine_realm::combat::RolledItem;
use engine_realm::haul::{self, SummaryItem};
use engine_realm::instance::{
    Countdown, InstanceState, MemberPresence, barrier_closed, run_result_id, should_clean_up,
    uid_hex,
};
use engine_realm::meter::ReportMember;
use engine_realm::outcome::{self, RunEnd, RunOutcome};
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
    let dungeon = crate::level::dungeon(ctx, &loaded.party.dungeon_id)?;
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
    let catalog = crate::level::catalog()?;
    let rules = catalog.modifiers();
    // The inserted key's modifiers, as its owner's snapshot carried them: every member saw
    // them before the start, and nothing here picks or changes them.
    let key_mods = rules.sanitize(
        loaded.party.key_level,
        &engine_realm::modifiers::decode_modifiers(&owner.key_modifiers),
    );
    let party = transition(ctx, &loaded.party, PartyState::Starting)?;

    let instance = ctx
        .db
        .dungeon_instance()
        .try_insert(DungeonInstance {
            id: 0,
            // Drawn here, once: persisted ids and the fight's seed come from it.
            uid: ctx.random::<u128>(),
            party_id: party.id,
            dungeon_id: party.dungeon_id.clone(),
            key_level: party.key_level,
            state: InstanceStateTag::Created,
            live: false,
            countdown_ends_at: None,
            barrier_open: false,
            created_at: ctx.timestamp,
            finished_at: None,
            modifiers: engine_realm::modifiers::encode_modifiers(&key_mods),
            rerolls: 0,
            reward_bonus_pct: 0,
            reroll_seq: 0,
            last_reroll: String::new(),
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
    for member in &loaded.members {
        let point = dungeon.definition.spawn_point(usize::from(member.slot));
        let health = catalog
            .character(&member.definition_id)
            .map_or(max_health, |definition| {
                engine_game::character::CharacterSheet::starting(definition.hero).max_health()
            });
        // What the member can spend on a reroll: the currency in the loadout they queued with.
        let reroll_resource = rules.config().reroll_resource.clone();
        let balance = serde_json::from_str::<engine_realm::loadout::Loadout>(&member.loadout)
            .ok()
            .and_then(|loadout| loadout.resources.get(&reroll_resource).copied())
            .unwrap_or(0);
        ctx.db.instance_spend().insert(InstanceSpend {
            spend_id: crate::modifiers::spend_id(instance.id, &member.account_id),
            identity: member.identity,
            instance_id: instance.id,
            account_id: member.account_id.clone(),
            resource: reroll_resource,
            balance,
            spent: 0,
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

    // What each member takes from monsters is tallied from the first kill.
    let members: Vec<(String, String)> = loaded
        .members
        .iter()
        .map(|member| (member.account_id.clone(), member.character_id.clone()))
        .collect();
    crate::loot::open_run(ctx, instance.id, &members);

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
    // Chest items nobody took go to their owners' mail; what a player picked up from monsters
    // in a run that never produced their result still reaches them.
    crate::loot::mail_unclaimed(ctx, instance_id);
    // A run torn down before it ended is abandoned: every player still in it gets a result with
    // what they spent and picked up, so currency spent is still spent and the history says so.
    settle_abandoned(ctx, &instance, None);
    crate::loot::close_run(ctx, &instance);
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
    let spends: Vec<String> = ctx
        .db
        .instance_spend()
        .instance_id()
        .filter(instance_id)
        .map(|spend| spend.spend_id)
        .collect();
    for id in spends {
        ctx.db.instance_spend().spend_id().delete(&id);
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
    let duration = run_duration(ctx, &instance);
    let dungeon = crate::level::dungeon(ctx, &instance.dungeon_id)?;
    // How it ended against the timer, once, for everyone in the run.
    let timer_us = outcome::timer_us(&dungeon.definition);
    let outcome = outcome::classify(
        if success {
            RunEnd::Completed
        } else {
            RunEnd::Failed
        },
        duration,
        timer_us,
    );
    let party_json = run_party_json(ctx, &instance);
    let key_owner = ctx
        .db
        .party()
        .id()
        .find(instance.party_id)
        .and_then(|party| party.key_owner);
    // Each player's roll from the boss's chest, read before the fight can change. Nothing for
    // a failed run, or one ended without a chest (an admin finish).
    // A community map pays nothing: no chest roll, no rewards, no key progress, no drops.
    let community = engine_realm::community::grants_nothing(&instance.dungeon_id);
    let rolls: Vec<(String, Vec<RolledItem>)> = if success && !community {
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
                .map(|account| (account.clone(), fight.loot_items_of(account)))
                .filter(|(_, items)| !items.is_empty())
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
        let result_id = run_result_id(instance.uid, &player.account_id);
        if ctx
            .db
            .pending_result()
            .result_id()
            .find(&result_id)
            .is_some()
        {
            continue;
        }
        let mut chest_items: Vec<SummaryItem> = Vec::new();
        if let Some((_, items)) = rolls
            .iter()
            .find(|(account, _)| *account == player.account_id)
        {
            let character = member
                .as_ref()
                .map(|m| m.character_id.clone())
                .unwrap_or_default();
            for (index, item) in items.iter().enumerate() {
                crate::loot::roll(
                    ctx,
                    instance_id,
                    instance.uid,
                    &player.account_id,
                    &character,
                    index,
                    item,
                );
                chest_items.push(SummaryItem::new(item.key, item.props));
            }
        }
        let is_owner = key_owner == Some(player.identity);
        let key_level = instance.key_level.max(1);
        let bonus_pct = crate::combat::reward_bonus(ctx, &instance);
        let rewards = if community {
            engine_realm::community::zero_rewards()
        } else {
            engine_realm::rewards::run_rewards(&dungeon.definition, key_level, success, is_owner)
                .with_bonus(bonus_pct)
        };
        let experience = rewards.experience;
        // A member whose own key was lower than the one used rises one level with the run.
        let own_level = member
            .as_ref()
            .filter(|m| m.key_dungeon_id == instance.dungeon_id)
            .map_or(0, |m| m.key_level);
        let participant_bump = if community {
            None
        } else {
            engine_realm::key::participant_progress(
                &dungeon.definition.keys,
                key_level,
                own_level,
                success,
                is_owner,
            )
        };
        let key_level_after = rewards.key_level_after.or(participant_bump).unwrap_or(0);
        let currency_spent = take_spend(ctx, instance_id, &player.account_id);
        let resources =
            serde_json::to_string(&rewards.resources).unwrap_or_else(|_| "[]".to_owned());
        // What the player took from monsters is paid with the result, win or lose.
        let (mob_drops, looted) = crate::loot::take_haul(ctx, instance_id, &player.account_id);
        let (mob_drops, looted) = if community {
            (0, Vec::new())
        } else {
            (mob_drops, looted)
        };
        let looted_text = haul::encode_currencies(&looted);
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
            talent_points: rewards.talent_points,
            looted: looted_text.clone(),
            currency_spent: serde_json::to_string(&currency_spent)
                .unwrap_or_else(|_| "[]".to_owned()),
            participant_key_bump: participant_bump.is_some(),
            own_key_level: if participant_bump.is_some() {
                own_level
            } else {
                0
            },
            run_uid: uid_hex(instance.uid),
            outcome: outcome.key().to_owned(),
            timer_us: timer_us.unwrap_or(0),
            party: party_json.clone(),
            modifiers: instance.modifiers.clone(),
        });
        ctx.db.run_summary().insert(RunSummary {
            // The client reads this row, so it carries the instance id, never the uid.
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
            loot_items: haul::encode_items(&chest_items),
            mob_drops,
            looted: looted_text,
            bonus_pct,
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
                if community {
                    "Community map complete!".to_owned()
                } else if key_level_after > 0 {
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
        "event=instance_completion instance_id={instance_id} dungeon_id={} success={success} outcome={} duration_us={duration}",
        instance.dungeon_id,
        outcome.key()
    );
    Ok(())
}

/// Takes a player's spend row out of the instance and returns what it holds to be debited:
/// `(resource, amount)`, empty when they spent nothing.
fn take_spend(ctx: &ReducerContext, instance_id: u64, account_id: &str) -> Vec<(String, u32)> {
    let id = crate::modifiers::spend_id(instance_id, account_id);
    let Some(spend) = ctx.db.instance_spend().spend_id().find(&id) else {
        return Vec::new();
    };
    ctx.db.instance_spend().spend_id().delete(&id);
    if spend.spent == 0 {
        return Vec::new();
    }
    vec![(
        spend.resource,
        u32::try_from(spend.spent).unwrap_or(u32::MAX),
    )]
}

/// When the run's clock started: the barrier lifting. An instance that never counted down
/// (torn down while loading) counts from its creation.
fn run_started(instance: &DungeonInstance) -> i64 {
    micros(instance.countdown_ends_at.unwrap_or(instance.created_at))
}

/// How long the run has lasted, microseconds: from the barrier lifting to now, never negative.
/// The dungeon timer and the leaderboards read this, and the client's timer counts from the same
/// instant (`dungeon_instance.countdown_ends_at`).
fn run_duration(ctx: &ReducerContext, instance: &DungeonInstance) -> i64 {
    (micros(ctx.timestamp) - run_started(instance)).max(0)
}

/// Who is in the run now, as the history lists the party, as JSON text.
fn run_party_json(ctx: &ReducerContext, instance: &DungeonInstance) -> String {
    let catalog = crate::level::catalog().ok();
    let members: Vec<RunMember> = ctx
        .db
        .instance_player()
        .instance_id()
        .filter(instance.id)
        .map(|player| RunMember {
            character_id: ctx
                .db
                .party_member()
                .identity()
                .find(player.identity)
                .map(|member| member.character_id)
                .unwrap_or_default(),
            name: player.name,
            hero: catalog
                .as_ref()
                .and_then(|c| c.character(&player.definition_id))
                .map_or_else(|| player.definition_id.clone(), |d| d.class.clone()),
            role: engine_realm::Role::from(player.role).key().to_owned(),
        })
        .collect();
    serde_json::to_string(&members).unwrap_or_else(|_| "[]".to_owned())
}

/// Ends the run for players who never got a result because nobody finished it: they left, or
/// the instance was torn down under them (everyone gone past the reconnect grace). Each gets a
/// failed, `abandoned` result with what they spent and picked up, so the persistence API still
/// debits the currency exactly once (the `result_id` is the idempotency key) and the history
/// records the attempt. `only` limits it to one account (a member leaving a run that goes on:
/// their result carries no party, so it cannot write the run's own row); `None` settles everyone
/// still in the run, with the party. A finished run already has its results: nothing to do.
pub fn settle_abandoned(ctx: &ReducerContext, instance: &DungeonInstance, only: Option<&str>) {
    if matches!(
        InstanceState::from(instance.state),
        InstanceState::Completed
            | InstanceState::Failed
            | InstanceState::CleaningUp
            | InstanceState::Destroyed
    ) {
        return;
    }
    let party = if only.is_none() {
        run_party_json(ctx, instance)
    } else {
        String::new()
    };
    let timer = crate::level::dungeon(ctx, &instance.dungeon_id)
        .ok()
        .and_then(|dungeon| outcome::timer_us(&dungeon.definition))
        .unwrap_or(0);
    let players: Vec<InstancePlayer> = ctx
        .db
        .instance_player()
        .instance_id()
        .filter(instance.id)
        .filter(|player| only.is_none_or(|account| player.account_id == account))
        .collect();
    for player in players {
        let result_id = run_result_id(instance.uid, &player.account_id);
        if ctx
            .db
            .pending_result()
            .result_id()
            .find(&result_id)
            .is_some()
        {
            continue;
        }
        let spent = take_spend(ctx, instance.id, &player.account_id);
        let (_, looted) = crate::loot::take_haul(ctx, instance.id, &player.account_id);
        // A community map grants nothing, abandoned or not.
        let looted = if engine_realm::community::grants_nothing(&instance.dungeon_id) {
            Vec::new()
        } else {
            looted
        };
        let character_id = ctx
            .db
            .party_member()
            .identity()
            .find(player.identity)
            .map(|member| member.character_id)
            .or_else(|| {
                ctx.db
                    .party_member()
                    .party_id()
                    .filter(instance.party_id)
                    .find(|member| member.account_id == player.account_id)
                    .map(|member| member.character_id)
            })
            .unwrap_or_default();
        ctx.db.pending_result().insert(PendingResult {
            result_id,
            account_id: player.account_id.clone(),
            character_id,
            dungeon_id: instance.dungeon_id.clone(),
            instance_id: instance.id,
            success: false,
            duration_us: run_duration(ctx, instance),
            experience: 0,
            created_at: ctx.timestamp,
            key_level: instance.key_level.max(1),
            key_owner: false,
            key_level_after: 0,
            resources: "[]".to_owned(),
            damage_report: String::new(),
            talent_points: 0,
            looted: haul::encode_currencies(&looted),
            currency_spent: serde_json::to_string(&spent).unwrap_or_else(|_| "[]".to_owned()),
            participant_key_bump: false,
            own_key_level: 0,
            run_uid: uid_hex(instance.uid),
            outcome: RunOutcome::Abandoned.key().to_owned(),
            timer_us: timer,
            party: party.clone(),
            modifiers: instance.modifiers.clone(),
        });
        log::info!(
            "event=run_abandoned instance_id={} account_id={}",
            instance.id,
            player.account_id
        );
    }
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
    crate::community::prune(ctx);
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
