//! Community maps in the realm: the bridge's entry points and the janitor's cleanup.
//!
//! ```text
//! API publish ─► svc_community_put(key, text) ─► prepare() = mapcheck again ─► community_map row
//! API unpublish/hide/reject/edit ─► svc_community_remove(map id) ─► inactive (or gone)
//! queue / party / instance for "cm:<id>:<ver>" ─► level::dungeon(ctx, id) ─► prepare(row.data)
//! ```
//!
//! The realm trusts nothing it is handed: `svc_community_put` runs the same parse and validate
//! as the API did, and instance creation runs it again on the stored text. Only the bridge's
//! identity may call these; the table is private.

use engine_realm::RealmError;
use engine_realm::community;
use spacetimedb::{ReducerContext, Table};

use crate::schema::*;
use crate::service::require_service;

/// Stores a validated community map. Putting a map it already holds with the same text only
/// makes it active again; a different text under the same key is refused (versions never
/// change).
#[spacetimedb::reducer]
pub fn svc_community_put(ctx: &ReducerContext, key: String, data: String) -> Result<(), String> {
    require_service(ctx)?;
    if community::parse_dungeon_id(&key).is_none() {
        return Err(RealmError::dungeon("That is not a community map id.").into());
    }
    if data.len() > community::MAX_BODY_BYTES {
        return Err(RealmError::dungeon("That map is too large.").into());
    }
    let catalog = crate::level::catalog()?;
    let template = catalog.dungeon(community::TEMPLATE_DUNGEON).ok_or_else(|| {
        RealmError::protocol("The realm's catalog has no template dungeon.").to_wire()
    })?;
    if let Err(report) = community::prepare(&data, &key, template) {
        let first = report
            .errors
            .first()
            .map_or_else(String::new, |issue| format!(" ({issue})"));
        log::warn!("event=community_put_refused key={key} errors={}", report.errors.len());
        return Err(
            RealmError::dungeon(format!("That map does not validate{first}.")).to_wire()
        );
    }
    match ctx.db.community_map().id().find(key.clone()) {
        Some(existing) if existing.data == data => {
            if !existing.active {
                ctx.db.community_map().id().update(CommunityMapRow {
                    active: true,
                    ..existing
                });
            }
        }
        Some(_) => {
            log::error!("event=community_put_conflict key={key}");
            return Err(RealmError::dungeon(
                "That map version already exists with different content.",
            )
            .into());
        }
        None => {
            ctx.db.community_map().insert(CommunityMapRow {
                id: key.clone(),
                data,
                active: true,
                created_at: ctx.timestamp,
            });
            log::info!("event=community_put key={key}");
        }
    }
    Ok(())
}

/// Takes every version of a map out of play: rows nothing refers to are deleted, the rest are
/// made inactive (a running party keeps its level) and removed by the janitor later.
#[spacetimedb::reducer]
pub fn svc_community_remove(ctx: &ReducerContext, map_id: String) -> Result<(), String> {
    require_service(ctx)?;
    let prefix = format!("{}{map_id}:", community::PREFIX);
    let rows: Vec<CommunityMapRow> = ctx
        .db
        .community_map()
        .iter()
        .filter(|row| row.id.starts_with(&prefix))
        .collect();
    for row in rows {
        if in_use(ctx, &row.id) {
            if row.active {
                ctx.db.community_map().id().update(CommunityMapRow {
                    active: false,
                    ..row
                });
            }
        } else {
            log::info!("event=community_removed key={}", row.id);
            ctx.db.community_map().id().delete(row.id);
        }
    }
    Ok(())
}

/// The ids of the maps new queues may name.
#[spacetimedb::procedure]
pub fn svc_community_list(ctx: &mut spacetimedb::ProcedureContext) -> Vec<String> {
    ctx.with_tx(|tx| match require_service(tx) {
        Ok(_) => tx
            .db
            .community_map()
            .iter()
            .filter(|row| row.active)
            .map(|row| row.id)
            .collect(),
        Err(_) => Vec::new(),
    })
}

/// Whether a ticket, party or instance still refers to the map.
fn in_use(ctx: &ReducerContext, id: &str) -> bool {
    ctx.db.queue_ticket().dungeon_id().filter(id).next().is_some()
        || ctx.db.party().iter().any(|party| party.dungeon_id == id)
        || ctx
            .db
            .dungeon_instance()
            .iter()
            .any(|instance| instance.dungeon_id == id)
}

/// Whether new queues may name `id`: the map exists and is active.
pub fn is_active(ctx: &ReducerContext, id: &str) -> bool {
    ctx.db
        .community_map()
        .id()
        .find(id.to_owned())
        .is_some_and(|row| row.active)
}

/// The janitor's pass: inactive maps nothing refers to any more are deleted.
pub fn prune(ctx: &ReducerContext) {
    let stale: Vec<String> = ctx
        .db
        .community_map()
        .iter()
        .filter(|row| !row.active && !in_use(ctx, &row.id))
        .map(|row| row.id)
        .collect();
    for id in stale {
        log::info!("event=community_pruned key={id}");
        ctx.db.community_map().id().delete(id);
    }
}
