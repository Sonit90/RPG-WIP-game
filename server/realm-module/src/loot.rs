//! Loot: from the roll at the end of a run, and the drops during it, to the bridge's outbox.
//!
//! ```text
//! finish ──► pending_loot "rolled"   (one row per chest item, durable)
//!                 │
//!   combat events │  LootAwarded ──► "bag"                   (opened the chest)
//!                 │  LootMailed  ──► "mail" + bag_full/...   (the sim had no room)
//!   teardown      │  still rolled ─► "mail" + unclaimed      (never opened)
//!                 ▼
//!   svc_take_loot (settled rows) ─► API delivers once ─► svc_ack_loot
//!
//! combat events ─ MobLoot ──────► pending_loot "mail" + dungeon_drop  (settled at once)
//!               └ MobCurrency ──► run_loot tally ─► finish folds it into pending_result
//! ```
//!
//! The chest roll is written at `finish` and never again, so a lost fight (a restart) loses
//! nothing: the rows are in the table, and teardown settles them. A row settles once - later
//! events for the same item are ignored - and the API's `loot_id` key makes delivery
//! exactly-once in effect. Monster drops are recorded from the events of the tick that
//! produced them, in the transaction that stepped the fight, so a tick that is rolled back
//! drops nothing and a tick that commits drops exactly what it rolled; nothing a client sends
//! reaches any of it.

use engine_game::character::ItemProps;
use engine_realm::combat::{EntityRef, RolledItem, WireEvent};
use engine_realm::haul::{self, Drop, RunTally};
use engine_realm::instance::{run_result_id, uid_hex};
use engine_realm::loadout::{MAIL_BAG_FULL, MAIL_DUNGEON_DROP, MAIL_UNCLAIMED};
use spacetimedb::{ReducerContext, Table};

use crate::schema::*;
use crate::service::require_service;

const ROLLED: &str = "rolled";
const BAG: &str = "bag";
const MAIL: &str = "mail";

/// The key of a player's tally in one instance. Local to the realm's tables, never persisted.
fn run_key(instance_id: u64, account_id: &str) -> String {
    format!("{instance_id}:{account_id}")
}

/// The uid of the instance `instance_id`, which names its loot in the persistence API. The
/// instance outlives every call here; zero (logged) only if a row is somehow gone.
fn uid_of(ctx: &ReducerContext, instance_id: u64) -> u128 {
    ctx.db
        .dungeon_instance()
        .id()
        .find(instance_id)
        .map_or_else(
            || {
                log::error!("event=loot_instance_missing instance_id={instance_id}");
                0
            },
            |instance| instance.uid,
        )
}

/// Whether the instance is a community-map run.
pub fn is_community_run(ctx: &ReducerContext, instance_id: u64) -> bool {
    ctx.db
        .dungeon_instance()
        .id()
        .find(instance_id)
        .is_some_and(|instance| engine_realm::community::grants_nothing(&instance.dungeon_id))
}

/// Records a chest item, if the player does not already have a row for that roll.
pub fn roll(
    ctx: &ReducerContext,
    instance_id: u64,
    uid: u128,
    account_id: &str,
    character_id: &str,
    index: usize,
    item: &RolledItem,
) {
    if is_community_run(ctx, instance_id) {
        return;
    }
    let loot_id = haul::chest_loot_id(uid, account_id, index);
    if ctx.db.pending_loot().loot_id().find(&loot_id).is_some() {
        return;
    }
    ctx.db.pending_loot().insert(PendingLoot {
        loot_id,
        account_id: account_id.to_owned(),
        character_id: character_id.to_owned(),
        instance_id,
        item: item.key.to_owned(),
        state: ROLLED.to_owned(),
        reason: String::new(),
        created_at: ctx.timestamp,
        quality: item.props.quality,
        enhancement: item.props.sharpening,
        broken: item.props.broken,
    });
}

/// Settles a rolled row. A row already settled, collected or absent is left alone.
fn settle(ctx: &ReducerContext, loot_id: &str, state: &str, reason: &str) {
    if let Some(row) = ctx.db.pending_loot().loot_id().find(loot_id.to_owned())
        && row.state == ROLLED
    {
        log::info!("event=loot_settled loot_id={loot_id} state={state} reason={reason}");
        ctx.db.pending_loot().loot_id().update(PendingLoot {
            state: state.to_owned(),
            reason: reason.to_owned(),
            ..row
        });
    }
}

/// Reads a tick's events for the chest's outcomes.
pub fn on_events(ctx: &ReducerContext, instance_id: u64, events: &[WireEvent]) {
    let uid = uid_of(ctx, instance_id);
    for event in events {
        match event {
            WireEvent::LootAwarded {
                player: EntityRef::Player(account),
                index,
                ..
            } => settle(
                ctx,
                &haul::chest_loot_id(uid, account, usize::from(*index)),
                BAG,
                "",
            ),
            WireEvent::LootMailed {
                player: EntityRef::Player(account),
                index,
                reason,
                ..
            } => settle(
                ctx,
                &haul::chest_loot_id(uid, account, usize::from(*index)),
                MAIL,
                if *reason == 0 {
                    MAIL_BAG_FULL
                } else {
                    MAIL_UNCLAIMED
                },
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
        .map(|row| row.loot_id)
        .collect();
    for id in rolled {
        settle(ctx, &id, MAIL, MAIL_UNCLAIMED);
    }
}

/// Opens the tally of what each member takes from monsters in this run.
pub fn open_run(ctx: &ReducerContext, instance_id: u64, members: &[(String, String)]) {
    for (account_id, character_id) in members {
        let key = run_key(instance_id, account_id);
        if ctx.db.run_loot().key().find(&key).is_none() {
            ctx.db.run_loot().insert(RunLoot {
                key,
                instance_id,
                account_id: account_id.clone(),
                character_id: character_id.clone(),
                items: 0,
                currencies: "[]".to_owned(),
                settled: false,
            });
        }
    }
}

/// The monster drops among a tick's `events`: items become settled mail rows at once,
/// currencies join the player's tally for `finish` to fold into their result.
///
/// Call this *before* `finish` handles the same tick's completion: the boss's gold is in the
/// batch that completes the run.
pub fn record_drops(ctx: &ReducerContext, instance_id: u64, events: &[WireEvent]) {
    // A community map grants no loot or currency: nothing is recorded, whatever the fight
    // rolled (it is armed with rules that roll nothing; this is the second lock).
    if is_community_run(ctx, instance_id) {
        return;
    }
    for drop in haul::drops_in(events) {
        match drop {
            Drop::Item {
                account,
                item,
                props,
            } => drop_item(ctx, instance_id, &account, &item, props),
            Drop::Currency {
                account,
                resource,
                amount,
            } => drop_currency(ctx, instance_id, &account, &resource, amount),
        }
    }
}

fn run_row(ctx: &ReducerContext, instance_id: u64, account_id: &str) -> Option<RunLoot> {
    let key = run_key(instance_id, account_id);
    if let Some(row) = ctx.db.run_loot().key().find(&key) {
        return Some(row);
    }
    // A player the run never opened a tally for (a member that joined late): open one, with the
    // character from their membership if they still have it.
    let character_id = ctx
        .db
        .instance_player()
        .instance_id()
        .filter(instance_id)
        .find(|player| player.account_id == account_id)
        .and_then(|player| ctx.db.party_member().identity().find(player.identity))
        .map(|member| member.character_id)?;
    Some(ctx.db.run_loot().insert(RunLoot {
        key,
        instance_id,
        account_id: account_id.to_owned(),
        character_id,
        items: 0,
        currencies: "[]".to_owned(),
        settled: false,
    }))
}

fn drop_item(ctx: &ReducerContext, instance_id: u64, account: &str, item: &str, props: ItemProps) {
    let Some(row) = run_row(ctx, instance_id, account) else {
        log::warn!("event=mob_drop_unowned instance_id={instance_id} account_id={account}");
        return;
    };
    let mut tally = tally_of(&row);
    let loot_id = haul::mob_loot_id(uid_of(ctx, instance_id), account, tally.add_item());
    ctx.db.pending_loot().insert(PendingLoot {
        loot_id: loot_id.clone(),
        account_id: account.to_owned(),
        character_id: row.character_id.clone(),
        instance_id,
        item: item.to_owned(),
        state: MAIL.to_owned(),
        reason: MAIL_DUNGEON_DROP.to_owned(),
        created_at: ctx.timestamp,
        quality: props.quality,
        enhancement: props.sharpening,
        broken: props.broken,
    });
    ctx.db.run_loot().key().update(with_tally(row, &tally));
    log::info!(
        "event=mob_drop loot_id={loot_id} item={item} quality={}",
        props.quality
    );
}

fn drop_currency(
    ctx: &ReducerContext,
    instance_id: u64,
    account: &str,
    resource: &str,
    amount: u32,
) {
    let Some(row) = run_row(ctx, instance_id, account) else {
        return;
    };
    let mut tally = tally_of(&row);
    if !tally.add_currency(resource, amount) {
        // The result already left: a late kill after the run ended pays nothing more.
        return;
    }
    ctx.db.run_loot().key().update(with_tally(row, &tally));
}

fn tally_of(row: &RunLoot) -> RunTally {
    RunTally {
        items: row.items,
        currencies: haul::decode_currencies(&row.currencies),
        settled: row.settled,
    }
}

fn with_tally(row: RunLoot, tally: &RunTally) -> RunLoot {
    RunLoot {
        items: tally.items,
        currencies: haul::encode_currencies(&tally.currencies),
        settled: tally.settled,
        ..row
    }
}

/// What `account` took from monsters, handed to their result: `(items dropped, currencies)`.
/// Marks the tally settled, so nothing is folded into a second result.
pub fn take_haul(
    ctx: &ReducerContext,
    instance_id: u64,
    account_id: &str,
) -> (u32, Vec<(String, u32)>) {
    let Some(row) = ctx
        .db
        .run_loot()
        .key()
        .find(run_key(instance_id, account_id))
    else {
        return (0, Vec::new());
    };
    let mut tally = tally_of(&row);
    let currencies = tally.settle();
    let items = tally.items;
    ctx.db.run_loot().key().update(with_tally(row, &tally));
    (items, currencies)
}

/// The instance is torn down: a player whose run never produced a result (it was abandoned)
/// still gets what they picked up, as a failed run with nothing but loot. Then the tallies go.
pub fn close_run(ctx: &ReducerContext, instance: &DungeonInstance) {
    let rows: Vec<RunLoot> = ctx
        .db
        .run_loot()
        .instance_id()
        .filter(instance.id)
        .collect();
    for row in rows {
        let result_id = run_result_id(instance.uid, &row.account_id);
        let currencies = haul::decode_currencies(&row.currencies);
        if !row.settled
            && !currencies.is_empty()
            && ctx
                .db
                .pending_result()
                .result_id()
                .find(&result_id)
                .is_none()
        {
            log::info!("event=abandoned_haul result_id={result_id}");
            ctx.db.pending_result().insert(PendingResult {
                result_id,
                account_id: row.account_id.clone(),
                character_id: row.character_id.clone(),
                dungeon_id: instance.dungeon_id.clone(),
                instance_id: instance.id,
                success: false,
                duration_us: 0,
                experience: 0,
                created_at: ctx.timestamp,
                key_level: instance.key_level.max(1),
                key_owner: false,
                key_level_after: 0,
                resources: "[]".to_owned(),
                damage_report: String::new(),
                talent_points: 0,
                looted: haul::encode_currencies(&currencies),
                currency_spent: "[]".to_owned(),
                participant_key_bump: false,
                own_key_level: 0,
                run_uid: uid_hex(instance.uid),
                outcome: engine_realm::outcome::RunOutcome::Abandoned
                    .key()
                    .to_owned(),
                timer_us: 0,
                party: String::new(),
                modifiers: instance.modifiers.clone(),
            });
        }
        ctx.db.run_loot().key().delete(&row.key);
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
