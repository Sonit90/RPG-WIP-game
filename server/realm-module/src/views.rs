//! What a client can read: only its own slice of the realm.
//!
//! Every table is private. These views are the whole public surface, and each starts from
//! `ctx.sender()`: my account, my ticket, my party, my instance. A client subscribing to
//! `my_instance_players` receives the players of *its* instance and nothing else — there is
//! no parameter to point it at someone else's.

use spacetimedb::{AnonymousViewContext, SpacetimeType, ViewContext, view};

use crate::schema::*;

#[view(accessor = my_account, public)]
fn my_account(ctx: &ViewContext) -> Option<Account> {
    ctx.db.account().identity().find(ctx.sender())
}

#[view(accessor = my_ticket, public)]
fn my_ticket(ctx: &ViewContext) -> Option<QueueTicket> {
    ctx.db.queue_ticket().identity().find(ctx.sender())
}

#[view(accessor = my_party, public)]
fn my_party(ctx: &ViewContext) -> Option<Party> {
    let member = ctx.db.party_member().identity().find(ctx.sender())?;
    ctx.db.party().id().find(member.party_id)
}

#[view(accessor = my_party_members, public, primary_key = identity)]
fn my_party_members(ctx: &ViewContext) -> Vec<PartyMember> {
    let Some(member) = ctx.db.party_member().identity().find(ctx.sender()) else {
        return Vec::new();
    };
    ctx.db
        .party_member()
        .party_id()
        .filter(member.party_id)
        .collect()
}

#[view(accessor = my_instance, public)]
fn my_instance(ctx: &ViewContext) -> Option<DungeonInstance> {
    let me = ctx.db.instance_player().identity().find(ctx.sender())?;
    ctx.db.dungeon_instance().id().find(me.instance_id)
}

#[view(accessor = my_instance_players, public, primary_key = identity)]
fn my_instance_players(ctx: &ViewContext) -> Vec<InstancePlayer> {
    let Some(me) = ctx.db.instance_player().identity().find(ctx.sender()) else {
        return Vec::new();
    };
    ctx.db
        .instance_player()
        .instance_id()
        .filter(me.instance_id)
        .collect()
}

#[view(accessor = my_instance_monsters, public, primary_key = id)]
fn my_instance_monsters(ctx: &ViewContext) -> Vec<InstanceMonster> {
    let Some(me) = ctx.db.instance_player().identity().find(ctx.sender()) else {
        return Vec::new();
    };
    ctx.db
        .instance_monster()
        .instance_id()
        .filter(me.instance_id)
        .collect()
}

#[view(accessor = my_notices, public, primary_key = id)]
fn my_notices(ctx: &ViewContext) -> Vec<PlayerNotice> {
    ctx.db
        .player_notice()
        .identity()
        .filter(ctx.sender())
        .collect()
}

/// How many players are searching for one dungeon, per role.
#[derive(SpacetimeType, Debug, Clone)]
pub struct QueuePopulation {
    /// The dungeon.
    pub dungeon_id: String,
    /// Tanks searching.
    pub tanks: u32,
    /// Healers searching.
    pub healers: u32,
    /// DPS searching.
    pub dps: u32,
}

/// Queue counts per dungeon, for the dungeon selection screen. Counts only — no names.
#[view(accessor = queue_population, public, primary_key = dungeon_id)]
fn queue_population(ctx: &AnonymousViewContext) -> Vec<QueuePopulation> {
    let Ok(catalog) = crate::level::catalog() else {
        return Vec::new();
    };
    catalog
        .dungeons()
        .iter()
        .map(|dungeon| {
            let mut population = QueuePopulation {
                dungeon_id: dungeon.id.clone(),
                tanks: 0,
                healers: 0,
                dps: 0,
            };
            for ticket in ctx.db.queue_ticket().dungeon_id().filter(&dungeon.id) {
                if ticket.status != TicketStatus::Searching {
                    continue;
                }
                match ticket.role {
                    RoleTag::Tank => population.tanks += 1,
                    RoleTag::Healer => population.healers += 1,
                    RoleTag::Dps => population.dps += 1,
                }
            }
            population
        })
        .collect()
}

#[view(accessor = my_player_combat, public, primary_key = identity)]
fn my_player_combat(ctx: &ViewContext) -> Vec<PlayerCombat> {
    let Some(me) = ctx.db.instance_player().identity().find(ctx.sender()) else {
        return Vec::new();
    };
    ctx.db
        .player_combat()
        .instance_id()
        .filter(me.instance_id)
        .collect()
}

#[view(accessor = my_combat_batches, public, primary_key = id)]
fn my_combat_batches(ctx: &ViewContext) -> Vec<CombatBatch> {
    let Some(me) = ctx.db.instance_player().identity().find(ctx.sender()) else {
        return Vec::new();
    };
    ctx.db
        .combat_batch()
        .instance_id()
        .filter(me.instance_id)
        .collect()
}
