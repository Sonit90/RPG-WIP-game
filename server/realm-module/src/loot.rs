//! Chest loot: from the roll at the end of a run to the bridge's outbox.
//!
//! ```text
//! finish ──► pending_loot "rolled"   (every member's roll, durable)
//!                 │
//!   combat events │  LootAwarded ──► "bag"                   (opened the chest)
//!                 │  LootMailed  ──► "mail" + bag_full/...   (the sim had no room)
//!   teardown      │  still rolled ─► "mail" + unclaimed      (never opened)
//!                 ▼
//!   svc_take_loot (settled rows) ─► API delivers once ─► svc_ack_loot
//! ```
//!
//! The roll is written at `finish` and never again, so a lost fight (a restart) loses
//! nothing: the row is in the table, and teardown settles it. A row settles once - later
//! events for the same player are ignored - and the API's `loot_id` key makes delivery
//! exactly-once in effect.

use engine_realm::combat::{EntityRef, WireEvent};
use engine_realm::loadout::{MAIL_BAG_FULL, MAIL_UNCLAIMED};
use spacetimedb::{ReducerContext, Table};

use crate::schema::*;
use crate::service::require_service;

const ROLLED: &str = "rolled";
const BAG: &str = "bag";
const MAIL: &str = "mail";

fn loot_id(instance_id: u64, account_id: &str) -> String {
    format!("{instance_id}:{account_id}:loot")
}

/// Records a player's roll, if they do not already have one for this instance.
pub fn roll(
    ctx: &ReducerContext,
    instance_id: u64,
    account_id: &str,
    character_id: String,
    item: &str,
) {
    let loot_id = loot_id(instance_id, account_id);
    if ctx.db.pending_loot().loot_id().find(&loot_id).is_some() {
        return;
    }
    ctx.db.pending_loot().insert(PendingLoot {
        loot_id,
        account_id: account_id.to_owned(),
        character_id,
        instance_id,
        item: item.to_owned(),
        state: ROLLED.to_owned(),
        reason: String::new(),
        created_at: ctx.timestamp,
    });
}

/// Settles a rolled row. A row already settled, collected or absent is left alone.
fn settle(ctx: &ReducerContext, instance_id: u64, account_id: &str, state: &str, reason: &str) {
    let id = loot_id(instance_id, account_id);
    if let Some(row) = ctx.db.pending_loot().loot_id().find(&id)
        && row.state == ROLLED
    {
        log::info!("event=loot_settled loot_id={id} state={state} reason={reason}");
        ctx.db.pending_loot().loot_id().update(PendingLoot {
            state: state.to_owned(),
            reason: reason.to_owned(),
            ..row
        });
    }
}

/// Reads a tick's events for the chest's outcomes.
pub fn on_events(ctx: &ReducerContext, instance_id: u64, events: &[WireEvent]) {
    for event in events {
        match event {
            WireEvent::LootAwarded {
                player: EntityRef::Player(account),
                ..
            } => settle(ctx, instance_id, account, BAG, ""),
            WireEvent::LootMailed {
                player: EntityRef::Player(account),
                reason,
                ..
            } => settle(
                ctx,
                instance_id,
                account,
                MAIL,
                if *reason == 0 { MAIL_BAG_FULL } else { MAIL_UNCLAIMED },
            ),
            _ => {}
        }
    }
}

/// The instance is going away: whatever was never opened is mailed.
pub fn mail_unclaimed(ctx: &ReducerContext, instance_id: u64) {
    let rolled: Vec<String> = ctx
        .db
        .pending_loot()
        .instance_id()
        .filter(instance_id)
        .filter(|row| row.state == ROLLED)
        .map(|row| row.account_id)
        .collect();
    for account in rolled {
        settle(ctx, instance_id, &account, MAIL, MAIL_UNCLAIMED);
    }
}

/// Hands the bridge every settled delivery not yet acknowledged (at most 100). Read-only, like
/// `svc_take_results`.
#[spacetimedb::procedure]
pub fn svc_take_loot(ctx: &mut spacetimedb::ProcedureContext) -> Vec<PendingLoot> {
    ctx.with_tx(|tx| match require_service(tx) {
        Ok(_) => tx
            .db
            .pending_loot()
            .iter()
            .filter(|row| row.state != ROLLED)
            .take(100)
            .collect(),
        Err(_) => Vec::new(),
    })
}

/// Removes deliveries the API has applied.
#[spacetimedb::reducer]
pub fn svc_ack_loot(ctx: &ReducerContext, loot_ids: Vec<String>) -> Result<(), String> {
    require_service(ctx)?;
    for id in &loot_ids {
        if ctx.db.pending_loot().loot_id().delete(id) {
            log::info!("event=loot_persisted loot_id={id}");
        }
    }
    Ok(())
}
