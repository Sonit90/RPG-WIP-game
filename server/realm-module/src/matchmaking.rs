//! The queue: leaving it, and the once-a-second pass that turns tickets into parties.
//!
//! The pass reads every searching ticket, hands them to `engine_realm::matchmaking::
//! find_matches` (the whole algorithm, tested natively) and writes each proposed party as a
//! `ReadyCheck` party. Reducers run one at a time in a transaction, so two passes — or a pass
//! and a `queue_leave` — can never both claim the same ticket.

use std::collections::BTreeMap;

use engine_realm::matchmaking::{QueueTicket as RuleTicket, find_matches};
use engine_realm::rate_limit::Operation;
use engine_realm::{RealmError, Role};
use spacetimedb::{ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::schema::*;
use crate::util::{config, matchmaking_config, micros, notify, rate_limit, timestamp};

/// Drives [`matchmaking_tick`].
#[spacetimedb::table(accessor = matchmaking_timer, scheduled(matchmaking_tick))]
pub struct MatchmakingTimer {
    /// Row id.
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    /// Every second.
    pub scheduled_at: ScheduleAt,
}

/// Starts the once-a-second pass.
pub fn schedule(ctx: &ReducerContext) {
    ctx.db.matchmaking_timer().insert(MatchmakingTimer {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(engine_realm::SECOND)),
    });
}

/// Leaves the queue. A matched ticket (in a ready check) is left by declining instead.
#[spacetimedb::reducer]
pub fn queue_leave(ctx: &ReducerContext) -> Result<(), String> {
    rate_limit(ctx, Operation::Queue)?;
    let ticket = ctx
        .db
        .queue_ticket()
        .identity()
        .find(ctx.sender())
        .ok_or_else(|| RealmError::matchmaking("You are not in the queue.").to_wire())?;
    if ticket.status == TicketStatus::Matched {
        return Err(
            RealmError::matchmaking("A dungeon was found. Accept or decline it instead.").into(),
        );
    }
    ctx.db.queue_ticket().id().delete(ticket.id);
    log::info!(
        "event=queue_leave account_id={} dungeon_id={} ticket_id={}",
        ticket.account_id,
        ticket.dungeon_id,
        ticket.id
    );
    Ok(())
}

/// Forms every party the queue allows.
#[spacetimedb::reducer]
pub fn matchmaking_tick(ctx: &ReducerContext, _timer: MatchmakingTimer) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err(RealmError::unauthorized("That operation is not available.").into());
    }
    let config = config(ctx)?;
    let now = micros(ctx.timestamp);

    let searching: Vec<QueueTicket> = ctx
        .db
        .queue_ticket()
        .iter()
        .filter(|ticket| ticket.status == TicketStatus::Searching)
        .collect();
    if searching.len() < Role::ALL.len() {
        return Ok(());
    }
    let mut by_id: BTreeMap<u64, QueueTicket> = BTreeMap::new();
    let rule_tickets: Vec<RuleTicket> = searching
        .into_iter()
        .map(|ticket| {
            let connected = ctx
                .db
                .account()
                .identity()
                .find(ticket.identity)
                .is_some_and(|account| account.connections > 0);
            let rule = RuleTicket {
                id: ticket.id,
                account: ticket.account_id.clone(),
                dungeon_id: ticket.dungeon_id.clone(),
                role: ticket.role.into(),
                gear_score: ticket.gear_score,
                enqueued_at: micros(ticket.enqueued_at),
                eligible: connected,
            };
            by_id.insert(ticket.id, ticket);
            rule
        })
        .collect();

    for proposal in find_matches(&rule_tickets, now, &matchmaking_config(&config)) {
        let tickets: Vec<QueueTicket> = proposal
            .tickets
            .iter()
            .filter_map(|id| by_id.get(id).cloned())
            .collect();
        if tickets.len() != Role::ALL.len() {
            continue;
        }
        let deadline = now + crate::util::party_config(&config).ready_check_timeout;
        let party = ctx.db.party().insert(Party {
            id: 0,
            dungeon_id: proposal.dungeon_id.clone(),
            state: PartyStateTag::ReadyCheck,
            leader: None,
            key_owner: None,
            key_level: 0,
            ready_deadline: timestamp(deadline),
            instance_id: None,
            created_at: ctx.timestamp,
        });
        let dungeon_name = crate::level::definition(ctx, &proposal.dungeon_id)
            .map_or_else(|_| proposal.dungeon_id.clone(), |definition| definition.name);
        for (slot, ticket) in tickets.iter().enumerate() {
            let display_name = ctx
                .db
                .account()
                .identity()
                .find(ticket.identity)
                .map_or_else(|| ticket.character_name.clone(), |a| a.display_name);
            ctx.db.party_member().insert(PartyMember {
                identity: ticket.identity,
                party_id: party.id,
                account_id: ticket.account_id.clone(),
                display_name,
                character_id: ticket.character_id.clone(),
                character_name: ticket.character_name.clone(),
                definition_id: ticket.definition_id.clone(),
                role: ticket.role,
                level: ticket.level,
                gear_score: ticket.gear_score,
                loadout: ticket.loadout.clone(),
                talents: ticket.talents.clone(),
                talent_points: ticket.talent_points,
                key_dungeon_id: ticket.key_dungeon_id.clone(),
                key_level: ticket.key_level,
                key_modifiers: ticket.key_modifiers.clone(),
                ready: ReadyTag::Pending,
                presence: PresenceTag::Present,
                disconnected_at: None,
                reconnect_grace_us: config.reconnect_grace_us,
                enqueued_at: ticket.enqueued_at,
                slot: u8::try_from(slot).unwrap_or(u8::MAX),
            });
            ctx.db.queue_ticket().id().update(QueueTicket {
                status: TicketStatus::Matched,
                party_id: Some(party.id),
                ..ticket.clone()
            });
            notify(
                ctx,
                ticket.identity,
                "dungeon_found",
                NoticeLevel::Info,
                format!("Dungeon found: {dungeon_name}."),
            );
        }
        crate::party::schedule_ready_check(ctx, party.id, deadline);
        log::info!(
            "event=match_formed party_id={} dungeon_id={} tank={} healer={} dps={}",
            party.id,
            proposal.dungeon_id,
            tickets[0].account_id,
            tickets[1].account_id,
            tickets[2].account_id
        );
    }
    Ok(())
}
