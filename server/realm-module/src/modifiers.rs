//! Key modifiers in a running dungeon: the reroll.
//!
//! ```text
//! modifier_reroll(index) ─► checks (engine_realm::modifiers::plan_reroll)
//!        │                    run Active, >= 50 % of the placed monsters dead,
//!        │                    a reroll left for the run, the caller can pay
//!        ▼
//!   server dice ─► new magnitude ─► instance row (modifiers, rerolls, reward bonus, last_reroll)
//!        │                        └► instance_spend row (what the caller has spent)
//!        ├─► the live fight: set_modifiers(resolve(level, mods)), set_reward_bonus(pct)
//!        └─► a notice to the whole party; the instance row's `reroll_seq` tells every client
//!            to play the reveal
//! ```
//!
//! The currency is not touched here. The row says how much the player has spent; the
//! persistence API debits it, once, in the transaction that records the run's result
//! (`pending_result.currency_spent`).

use engine_realm::RealmError;
use engine_realm::instance::InstanceState;
use engine_realm::modifiers::{
    RerollEvent, RerollRequest, decode_modifiers, encode_modifiers, magnitude_text,
};
use engine_realm::rate_limit::Operation;
use spacetimedb::ReducerContext;

use crate::schema::*;
use crate::util::{notify, rate_limit};

/// The key of a player's spend row in an instance.
pub fn spend_id(instance_id: u64, account_id: &str) -> String {
    format!("{instance_id}:{account_id}")
}

/// Spends one reroll on the key's `index`th modifier. Any party member in the run may; it
/// costs the caller's own currency.
#[spacetimedb::reducer]
pub fn modifier_reroll(ctx: &ReducerContext, index: u8) -> Result<(), String> {
    rate_limit(ctx, Operation::Party)?;
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
    if InstanceState::from(instance.state) != InstanceState::Active {
        return Err(
            RealmError::dungeon("A key can only be rerolled while the run is under way.").into(),
        );
    }
    if engine_realm::community::grants_nothing(&instance.dungeon_id) {
        return Err(RealmError::dungeon("Keys cannot be rerolled on a community map.").into());
    }
    let catalog = crate::level::catalog()?;
    let rules = catalog.modifiers();
    let spend = ctx
        .db
        .instance_spend()
        .spend_id()
        .find(spend_id(instance.id, &player.account_id))
        .ok_or_else(|| RealmError::dungeon("You have nothing to spend in this run.").to_wire())?;
    let mods = decode_modifiers(&instance.modifiers);
    let progress = crate::combat::with_fight(ctx, &instance, |fight| fight.kill_progress())?;
    let resource = &rules.config().reroll_resource;
    let resource_name = catalog
        .economy()
        .resource(resource)
        .map_or_else(|| resource.clone(), |found| found.name.clone());
    let request = RerollRequest {
        mods: &mods,
        index: usize::from(index),
        progress,
        rerolls_done: instance.rerolls,
        balance: spend.balance,
        spent: spend.spent,
    };
    let (roll_a, roll_b) = (ctx.random::<f64>(), ctx.random::<f64>());
    let reroll = rules.plan_reroll(&request, &resource_name, roll_a, roll_b)?;

    // Commit: the rows first, then the live fight, so a fight that is rebuilt from the rows
    // (a restart) comes back with the new magnitudes.
    let seq = instance.reroll_seq + 1;
    let event = RerollEvent {
        seq,
        by: player.name.clone(),
        modifier: reroll.id.clone(),
        delta_pct: reroll.delta_pct,
        magnitude_pct: reroll.magnitude_pct,
    };
    let updated = DungeonInstance {
        modifiers: encode_modifiers(&reroll.apply(&mods)),
        rerolls: instance.rerolls + 1,
        reward_bonus_pct: rules.config().reroll_reward_bonus_pct,
        reroll_seq: seq,
        last_reroll: event.to_json(),
        ..instance.clone()
    };
    ctx.db.dungeon_instance().id().update(updated.clone());
    ctx.db.instance_spend().spend_id().update(InstanceSpend {
        spent: spend.spent.saturating_add(u64::from(reroll.cost)),
        ..spend
    });
    crate::combat::with_fight(ctx, &updated, |fight| {
        crate::combat::apply_run_rules(&updated, fight);
    })?;

    let name = rules
        .modifier(&reroll.id)
        .map_or_else(|| reroll.id.clone(), |definition| definition.name.clone());
    let how = if reroll.strengthened() {
        "stronger"
    } else {
        "weaker"
    };
    let message = format!(
        "{} rerolled {name}: {} ({how}). Rewards +{}% if the run succeeds.",
        player.name,
        magnitude_text(reroll.delta_pct),
        updated.reward_bonus_pct
    );
    for other in ctx.db.instance_player().instance_id().filter(instance.id) {
        notify(
            ctx,
            other.identity,
            "modifier_rerolled",
            NoticeLevel::Info,
            message.clone(),
        );
    }
    log::info!(
        "event=modifier_reroll instance_id={} account_id={} modifier={} delta_pct={} magnitude_pct={} spent={}",
        instance.id,
        player.account_id,
        reroll.id,
        reroll.delta_pct,
        reroll.magnitude_pct,
        reroll.cost
    );
    Ok(())
}
