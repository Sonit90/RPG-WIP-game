//! The persistence bridge's entry points.
//!
//! ```text
//! client ── POST /api/v1/realm/queue ──► realm API ── checks ownership, level, gear ──┐
//!                                                                                     ▼
//!                                       svc_enqueue(account, snapshot, dungeon, role) here
//! ```
//!
//! A client never sends a gear score, a level or a character to this module. It asks the API,
//! which owns characters, to queue it; the API calls `svc_enqueue` with the snapshot it read
//! from its own database. Only the bridge's identity — derived from the configured issuer and
//! the service subject — may call anything here.

use engine_realm::rate_limit::Operation;
use engine_realm::{RealmError, Role};
use spacetimedb::{Identity, ReducerContext, Table};

use crate::schema::*;
use crate::util::{config, notify, service_identity};

/// Refuses anyone but the bridge.
pub fn require_service(ctx: &ReducerContext) -> Result<RealmConfig, RealmError> {
    let config = config(ctx)?;
    if ctx.sender() != service_identity(&config) {
        log::warn!(
            "event=service_call_refused identity={}",
            ctx.sender().to_abbreviated_hex()
        );
        return Err(RealmError::unauthorized("That operation is not available."));
    }
    Ok(config)
}

/// The identity a player of `account_id` connects as.
pub fn player_identity(config: &RealmConfig, account_id: &str) -> Identity {
    Identity::from_claims(&config.issuer, account_id)
}

/// Puts a character into the queue. Called by the realm API on a player's behalf.
#[allow(
    clippy::too_many_arguments,
    reason = "a reducer's arguments are its wire format"
)]
#[spacetimedb::reducer]
pub fn svc_enqueue(
    ctx: &ReducerContext,
    account_id: String,
    character_id: String,
    character_name: String,
    definition_id: String,
    level: u32,
    gear_score: u32,
    loadout: String,
    dungeon_id: String,
    role: RoleTag,
    key_dungeon_id: String,
    key_level: u32,
    talents: String,
    talent_points: u32,
    key_modifiers: String,
) -> Result<(), String> {
    let config = require_service(ctx)?;
    let identity = player_identity(&config, &account_id);

    let account = ctx.db.account().identity().find(identity).ok_or_else(|| {
        RealmError::matchmaking("Connect to the realm before joining the queue.").to_wire()
    })?;
    if account.connections == 0 {
        return Err(
            RealmError::matchmaking("Connect to the realm before joining the queue.").into(),
        );
    }
    // The player's own call is what is being limited, even though the bridge relays it.
    crate::util::rate_limit_for(ctx, identity, Operation::Queue)?;

    let catalog = crate::level::catalog()?;
    // A community map must be one the bridge has published (and not withdrawn since).
    if engine_realm::community::is_community(&dungeon_id)
        && !crate::community::is_active(ctx, &dungeon_id)
    {
        return Err(RealmError::matchmaking("That community map is not available.").into());
    }
    let dungeon = crate::level::definition(ctx, &dungeon_id)
        .map_err(|_| RealmError::matchmaking("That dungeon does not exist.").to_wire())?;
    let definition = catalog
        .character(&definition_id)
        .ok_or_else(|| RealmError::matchmaking("That character cannot queue.").to_wire())?;
    let role_rule = Role::from(role);
    if !definition.can_play(role_rule) {
        return Err(RealmError::matchmaking(format!(
            "{} cannot queue as {}.",
            definition.name,
            role_rule.name()
        ))
        .into());
    }
    dungeon.admits(level, gear_score)?;
    if ctx.db.queue_ticket().identity().find(identity).is_some() {
        return Err(RealmError::matchmaking("You are already in the queue.").into());
    }
    if ctx.db.party_member().identity().find(identity).is_some() {
        return Err(RealmError::matchmaking("Leave your party before queueing again.").into());
    }

    let ticket = ctx
        .db
        .queue_ticket()
        .try_insert(QueueTicket {
            id: 0,
            identity,
            account_id: account_id.clone(),
            character_id: character_id.clone(),
            character_name,
            definition_id,
            level,
            gear_score,
            loadout,
            talents,
            talent_points,
            dungeon_id: dungeon_id.clone(),
            role,
            key_dungeon_id,
            key_level,
            // Made safe once, here: what the party and the fight read has only modifiers the
            // catalog knows, within the limits and the key level's slots.
            key_modifiers: engine_realm::modifiers::encode_modifiers(
                &catalog.modifiers().sanitize(
                    key_level,
                    &engine_realm::modifiers::decode_modifiers(&key_modifiers),
                ),
            ),
            enqueued_at: ctx.timestamp,
            status: TicketStatus::Searching,
            party_id: None,
        })
        .map_err(|_| RealmError::matchmaking("You are already in the queue.").to_wire())?;
    notify(
        ctx,
        identity,
        "queue_joined",
        NoticeLevel::Info,
        format!("Queued for {} as {}.", dungeon.name, role_rule.name()),
    );
    log::info!(
        "event=queue_join account_id={account_id} character_id={character_id} dungeon_id={dungeon_id} role={} gear_score={gear_score} ticket_id={}",
        role_rule.key(),
        ticket.id
    );
    Ok(())
}

/// Hands the bridge every result not yet acknowledged (at most 100 per call). Read-only: a
/// result leaves the outbox only when [`svc_ack_results`] says it was persisted, so a crash
/// between the two re-delivers rather than loses it, and the API's idempotency key makes
/// re-delivery harmless. Anyone but the bridge receives an empty list.
#[spacetimedb::procedure]
pub fn svc_take_results(ctx: &mut spacetimedb::ProcedureContext) -> Vec<PendingResult> {
    ctx.with_tx(|tx| match require_service(tx) {
        Ok(_) => tx.db.pending_result().iter().take(100).collect(),
        Err(_) => Vec::new(),
    })
}

/// Whether an account is engaged with the realm, for the API's lock on talents and equipment.
#[derive(spacetimedb::SpacetimeType, Clone, Debug)]
pub struct Engagement {
    /// Queued, in a party or in an instance.
    pub engaged: bool,
    /// `"none"`, `"queued"`, `"party"` or `"instance"`; `"refused"` to anyone but the bridge.
    pub state: String,
}

/// Whether the account has a character in the queue, in a party (ready check, key selection)
/// or in an instance - disconnected-in-grace included, because a disconnected member keeps
/// their row until the grace runs out. The API asks before it changes talents or equipment:
/// the realm holds a snapshot of both for the run.
///
/// Fails closed: anyone but the bridge is told the account is engaged.
#[spacetimedb::procedure]
pub fn svc_engagement(ctx: &mut spacetimedb::ProcedureContext, account_id: String) -> Engagement {
    ctx.with_tx(|tx| {
        let Ok(config) = require_service(tx) else {
            return Engagement {
                engaged: true,
                state: "refused".to_owned(),
            };
        };
        let identity = player_identity(&config, &account_id);
        let state = if tx.db.instance_player().identity().find(identity).is_some() {
            "instance"
        } else if tx.db.party_member().identity().find(identity).is_some() {
            "party"
        } else if tx.db.queue_ticket().identity().find(identity).is_some() {
            "queued"
        } else {
            "none"
        };
        Engagement {
            engaged: state != "none",
            state: state.to_owned(),
        }
    })
}

/// Removes results the bridge has persisted.
#[spacetimedb::reducer]
pub fn svc_ack_results(ctx: &ReducerContext, result_ids: Vec<String>) -> Result<(), String> {
    require_service(ctx)?;
    for id in &result_ids {
        if ctx.db.pending_result().result_id().delete(id) {
            log::info!("event=result_persisted result_id={id}");
        }
    }
    Ok(())
}
