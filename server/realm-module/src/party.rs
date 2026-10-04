//! Parties: the ready check, the leader, and every member operation.
//!
//! Each reducer builds a `PartyView` (plain data) from the rows and asks
//! `engine_realm::party::authorize` whether the caller may do what they asked. The answer is
//! the whole permission model; this file only applies what it allows.

use engine_realm::RealmError;
use engine_realm::party::{
    DepartureEffect, MemberView, PartyAction, PartyState, PartyView, ReadyCheckOutcome,
    ReadyResponse, apply_ready_response, authorize, departure_effect, evaluate_ready_check,
    pick_leader, successor,
};
use engine_realm::rate_limit::Operation;
use spacetimedb::{Identity, ReducerContext, ScheduleAt, Table};

use crate::schema::*;
use crate::util::{micros, notify, rate_limit, timestamp};

/// Fires when a ready check's time is up.
#[spacetimedb::table(accessor = ready_check_timer, scheduled(ready_check_expire))]
pub struct ReadyCheckTimer {
    /// Row id.
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    /// The deadline.
    pub scheduled_at: ScheduleAt,
    /// Which party.
    pub party_id: u64,
}

/// Arms the ready check's timeout.
pub fn schedule_ready_check(ctx: &ReducerContext, party_id: u64, deadline: engine_realm::Micros) {
    ctx.db.ready_check_timer().insert(ReadyCheckTimer {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(timestamp(deadline)),
        party_id,
    });
}

/// A party's rows and the rules' view of them.
pub struct Loaded {
    /// The party row.
    pub party: Party,
    /// Its members, in slot order.
    pub members: Vec<PartyMember>,
}

impl Loaded {
    /// The rules' view.
    pub fn view(&self, ctx: &ReducerContext) -> PartyView {
        PartyView {
            state: self.party.state.into(),
            leader: self.party.leader.and_then(|leader| {
                self.members
                    .iter()
                    .find(|member| member.identity == leader)
                    .map(|member| member.account_id.clone())
            }),
            members: self
                .members
                .iter()
                .map(|member| MemberView {
                    account: member.account_id.clone(),
                    role: member.role.into(),
                    ready: member.ready.into(),
                    presence: present(ctx, member),
                })
                .collect(),
        }
    }

    /// The member with this account.
    pub fn member(&self, account_id: &str) -> Option<&PartyMember> {
        self.members
            .iter()
            .find(|member| member.account_id == account_id)
    }
}

/// A member's presence, from the account's live connection count.
fn present(ctx: &ReducerContext, member: &PartyMember) -> engine_realm::party::Presence {
    let connected = ctx
        .db
        .account()
        .identity()
        .find(member.identity)
        .is_some_and(|account| account.connections > 0);
    if connected {
        engine_realm::party::Presence::Present
    } else {
        engine_realm::party::Presence::TemporarilyDisconnected
    }
}

/// Loads a party.
pub fn load(ctx: &ReducerContext, party_id: u64) -> Option<Loaded> {
    let party = ctx.db.party().id().find(party_id)?;
    let mut members: Vec<PartyMember> = ctx.db.party_member().party_id().filter(party_id).collect();
    members.sort_by_key(|member| member.slot);
    Some(Loaded { party, members })
}

/// The caller's party, or an error saying they have none.
fn callers_party(ctx: &ReducerContext) -> Result<(PartyMember, Loaded), RealmError> {
    let member = ctx
        .db
        .party_member()
        .identity()
        .find(ctx.sender())
        .ok_or_else(|| RealmError::party("You are not in a party."))?;
    let loaded =
        load(ctx, member.party_id).ok_or_else(|| RealmError::party("That party no longer exists."))?;
    Ok((member, loaded))
}

/// Moves a party to `next`, refusing a transition the state machine forbids.
pub fn transition(ctx: &ReducerContext, party: &Party, next: PartyState) -> Result<Party, RealmError> {
    let current: PartyState = party.state.into();
    if !current.can_become(next) {
        log::error!(
            "event=party_transition_refused party_id={} from={} to={}",
            party.id,
            current.key(),
            next.key()
        );
        return Err(RealmError::party("The party cannot do that right now."));
    }
    let updated = Party {
        state: next.into(),
        ..party.clone()
    };
    ctx.db.party().id().update(updated.clone());
    Ok(updated)
}

/// Answers the ready check.
#[spacetimedb::reducer]
pub fn ready_check_respond(ctx: &ReducerContext, accept: bool) -> Result<(), String> {
    rate_limit(ctx, Operation::ReadyCheck)?;
    let (member, loaded) = callers_party(ctx)?;
    authorize(&loaded.view(ctx), &member.account_id, &PartyAction::RespondReady)?;
    let answer = if accept {
        ReadyResponse::Accepted
    } else {
        ReadyResponse::Declined
    };
    let ready = apply_ready_response(member.ready.into(), answer)?;
    let party_id = member.party_id;
    log::info!(
        "event=ready_check_response party_id={party_id} account_id={} accept={accept}",
        member.account_id
    );
    ctx.db.party_member().identity().update(PartyMember {
        ready: ready.into(),
        ..member
    });
    resolve_ready_check(ctx, party_id);
    Ok(())
}

/// The ready check's deadline passed.
#[spacetimedb::reducer]
pub fn ready_check_expire(ctx: &ReducerContext, timer: ReadyCheckTimer) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err(RealmError::unauthorized("That operation is not available.").into());
    }
    resolve_ready_check(ctx, timer.party_id);
    Ok(())
}

/// Settles a ready check if its outcome is decided: everyone in, or cancelled.
pub fn resolve_ready_check(ctx: &ReducerContext, party_id: u64) {
    let Some(loaded) = load(ctx, party_id) else {
        return;
    };
    if loaded.party.state != PartyStateTag::ReadyCheck {
        return;
    }
    let view = loaded.view(ctx);
    let now = micros(ctx.timestamp);
    match evaluate_ready_check(&view.members, now, micros(loaded.party.ready_deadline)) {
        ReadyCheckOutcome::Waiting => {}
        ReadyCheckOutcome::AllAccepted => {
            let accounts: Vec<String> = loaded.members.iter().map(|m| m.account_id.clone()).collect();
            let roll: u64 = ctx.random();
            let leader_account = pick_leader(&accounts, roll);
            let leader = leader_account
                .as_deref()
                .and_then(|account| loaded.member(account))
                .map(|member| member.identity);
            let Ok(party) = transition(ctx, &loaded.party, PartyState::Ready) else {
                return;
            };
            ctx.db.party().id().update(Party { leader, ..party });
            for member in &loaded.members {
                ctx.db.queue_ticket().identity().delete(member.identity);
                notify(
                    ctx,
                    member.identity,
                    "party_ready",
                    NoticeLevel::Info,
                    if Some(member.identity) == leader {
                        "Everyone is ready. You lead the party: start when ready.".to_owned()
                    } else {
                        "Everyone is ready. Waiting for the leader to start.".to_owned()
                    },
                );
            }
            log::info!(
                "event=party_created party_id={party_id} dungeon_id={} leader={}",
                loaded.party.dungeon_id,
                leader_account.unwrap_or_default()
            );
        }
        ReadyCheckOutcome::Cancelled { requeue, remove } => {
            for member in &loaded.members {
                let removed = remove.contains(&member.account_id);
                if removed {
                    ctx.db.queue_ticket().identity().delete(member.identity);
                    notify(
                        ctx,
                        member.identity,
                        "ready_check_failed",
                        NoticeLevel::Warning,
                        "You did not confirm the dungeon and left the queue.",
                    );
                } else if let Some(ticket) = ctx.db.queue_ticket().identity().find(member.identity) {
                    // Back to searching, keeping the original queue time.
                    ctx.db.queue_ticket().id().update(QueueTicket {
                        status: TicketStatus::Searching,
                        party_id: None,
                        ..ticket
                    });
                    notify(
                        ctx,
                        member.identity,
                        "ready_check_cancelled",
                        NoticeLevel::Warning,
                        "A party member did not confirm. You are back in the queue.",
                    );
                }
                ctx.db.party_member().identity().delete(member.identity);
            }
            ctx.db.party().id().delete(party_id);
            log::info!(
                "event=ready_check_cancelled party_id={party_id} requeued={} removed={}",
                requeue.join(","),
                remove.join(",")
            );
        }
    }
}

/// Leaves the party (and its instance, if it has one).
#[spacetimedb::reducer]
pub fn party_leave(ctx: &ReducerContext) -> Result<(), String> {
    rate_limit(ctx, Operation::Party)?;
    let (member, loaded) = callers_party(ctx)?;
    authorize(&loaded.view(ctx), &member.account_id, &PartyAction::Leave)?;
    depart(ctx, &loaded, &member, "left");
    Ok(())
}

/// Removes another member. Leader only.
#[spacetimedb::reducer]
pub fn party_kick(ctx: &ReducerContext, target_account_id: String) -> Result<(), String> {
    rate_limit(ctx, Operation::Leader)?;
    let (member, loaded) = callers_party(ctx)?;
    authorize(
        &loaded.view(ctx),
        &member.account_id,
        &PartyAction::Kick {
            target: target_account_id.clone(),
        },
    )?;
    let target = loaded
        .member(&target_account_id)
        .cloned()
        .ok_or_else(|| RealmError::party("That player is not in your party.").to_wire())?;
    notify(
        ctx,
        target.identity,
        "kicked",
        NoticeLevel::Warning,
        "You were removed from the party.",
    );
    log::info!(
        "event=party_kick party_id={} leader={} target={target_account_id}",
        loaded.party.id,
        member.account_id
    );
    depart(ctx, &loaded, &target, "removed");
    Ok(())
}

/// Hands leadership to another member. Leader only.
#[spacetimedb::reducer]
pub fn party_transfer_leader(ctx: &ReducerContext, target_account_id: String) -> Result<(), String> {
    rate_limit(ctx, Operation::Leader)?;
    let (member, loaded) = callers_party(ctx)?;
    authorize(
        &loaded.view(ctx),
        &member.account_id,
        &PartyAction::TransferLeader {
            target: target_account_id.clone(),
        },
    )?;
    let target = loaded
        .member(&target_account_id)
        .cloned()
        .ok_or_else(|| RealmError::party("That player is not in your party.").to_wire())?;
    set_leader(ctx, &loaded, Some(target.identity));
    log::info!(
        "event=leader_transfer party_id={} from={} to={target_account_id}",
        loaded.party.id,
        member.account_id
    );
    Ok(())
}

/// Starts the dungeon: validates, builds the instance, begins the countdown. Leader only.
#[spacetimedb::reducer]
pub fn party_start(ctx: &ReducerContext) -> Result<(), String> {
    rate_limit(ctx, Operation::Leader)?;
    let (member, loaded) = callers_party(ctx)?;
    authorize(&loaded.view(ctx), &member.account_id, &PartyAction::Start)?;
    crate::instance::create(ctx, &loaded)?;
    Ok(())
}

/// Sets the leader and tells everyone.
fn set_leader(ctx: &ReducerContext, loaded: &Loaded, leader: Option<Identity>) {
    ctx.db.party().id().update(Party {
        leader,
        ..loaded.party.clone()
    });
    let name = leader
        .and_then(|identity| loaded.members.iter().find(|m| m.identity == identity))
        .map(|member| member.character_name.clone())
        .unwrap_or_default();
    for member in &loaded.members {
        if Some(member.identity) == leader {
            notify(ctx, member.identity, "leader_changed", NoticeLevel::Info, "You now lead the party.");
        } else {
            notify(
                ctx,
                member.identity,
                "leader_changed",
                NoticeLevel::Info,
                format!("{name} now leads the party."),
            );
        }
    }
}

/// Applies one member's departure, by choice, by kick or by expired grace.
pub fn depart(ctx: &ReducerContext, loaded: &Loaded, leaving: &PartyMember, how: &str) {
    let view = loaded.view(ctx);
    let party_id = loaded.party.id;
    match departure_effect(&view, &leaving.account_id) {
        DepartureEffect::DisbandAndRequeue => {
            // No party can start without a full roster, and roles are never replaced: the
            // others go back to the queue in their original place.
            for member in &loaded.members {
                ctx.db.party_member().identity().delete(member.identity);
                if member.identity == leaving.identity {
                    continue;
                }
                requeue(ctx, loaded, member);
                notify(
                    ctx,
                    member.identity,
                    "party_disbanded",
                    NoticeLevel::Warning,
                    format!(
                        "{} {how} the party. You are back in the queue.",
                        leaving.character_name
                    ),
                );
            }
            ctx.db.queue_ticket().identity().delete(leaving.identity);
            ctx.db.party().id().delete(party_id);
            log::info!(
                "event=party_disbanded party_id={party_id} reason={how} account_id={}",
                leaving.account_id
            );
        }
        DepartureEffect::ContinueWithout => {
            ctx.db.party_member().identity().delete(leaving.identity);
            ctx.db.instance_player().identity().delete(leaving.identity);
            let remaining: Vec<PartyMember> = loaded
                .members
                .iter()
                .filter(|member| member.identity != leaving.identity)
                .cloned()
                .collect();
            for member in &remaining {
                notify(
                    ctx,
                    member.identity,
                    "member_left",
                    NoticeLevel::Info,
                    format!("{} {how} the party.", leaving.character_name),
                );
            }
            if loaded.party.leader == Some(leaving.identity) {
                let roll: u64 = ctx.random();
                let next = successor(&view, &leaving.account_id, roll)
                    .and_then(|account| remaining.iter().find(|m| m.account_id == account))
                    .map(|member| member.identity);
                let reloaded = Loaded {
                    party: loaded.party.clone(),
                    members: remaining,
                };
                set_leader(ctx, &reloaded, next);
            }
            log::info!(
                "event=member_departed party_id={party_id} reason={how} account_id={}",
                leaving.account_id
            );
        }
        DepartureEffect::Disband => {
            ctx.db.party_member().identity().delete(leaving.identity);
            ctx.db.instance_player().identity().delete(leaving.identity);
            if let Some(instance_id) = loaded.party.instance_id {
                crate::instance::destroy(ctx, instance_id, "empty");
            }
            ctx.db.party().id().delete(party_id);
            log::info!("event=party_disbanded party_id={party_id} reason=empty");
        }
    }
}

/// Puts a former member back into the queue with their original queue time.
fn requeue(ctx: &ReducerContext, loaded: &Loaded, member: &PartyMember) {
    if ctx.db.queue_ticket().identity().find(member.identity).is_some() {
        ctx.db.queue_ticket().identity().delete(member.identity);
    }
    ctx.db.queue_ticket().insert(QueueTicket {
        id: 0,
        identity: member.identity,
        account_id: member.account_id.clone(),
        character_id: member.character_id.clone(),
        character_name: member.character_name.clone(),
        definition_id: member.definition_id.clone(),
        level: member.level,
        gear_score: member.gear_score,
        dungeon_id: loaded.party.dungeon_id.clone(),
        role: member.role,
        enqueued_at: member.enqueued_at,
        status: TicketStatus::Searching,
        party_id: None,
    });
}
