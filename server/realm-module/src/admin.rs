//! Operator entry points: configuration after a deploy, and the clock procedure every client
//! uses to agree with the server about when a countdown ends.
//!
//! `admin_*` reducers accept only the identity that published the module (recorded by `init`),
//! so they are reachable with `spacetime call` by whoever holds the deploy credentials and by
//! nobody else.

use engine_realm::RealmError;
use spacetimedb::ReducerContext;

use crate::schema::*;
use crate::util::config;

/// Refuses anyone but the owner.
fn require_owner(ctx: &ReducerContext) -> Result<RealmConfig, RealmError> {
    let config = config(ctx)?;
    if ctx.sender() != config.owner {
        log::warn!(
            "event=admin_call_refused identity={}",
            ctx.sender().to_abbreviated_hex()
        );
        return Err(RealmError::unauthorized("That operation is not available."));
    }
    Ok(config)
}

/// Points the realm at an issuer: which tokens it accepts and who the bridge is.
#[spacetimedb::reducer]
pub fn admin_configure(
    ctx: &ReducerContext,
    issuer: String,
    audience: String,
    service_subject: String,
) -> Result<(), String> {
    let config = require_owner(ctx)?;
    if issuer.is_empty() || audience.is_empty() || service_subject.is_empty() {
        return Err(RealmError::protocol("Issuer, audience and service subject are required.").into());
    }
    log::info!("event=realm_configured issuer={issuer} audience={audience}");
    ctx.db.realm_config().id().update(RealmConfig {
        issuer,
        audience,
        service_subject,
        ..config
    });
    Ok(())
}

/// Tunes matchmaking on a live realm.
#[spacetimedb::reducer]
pub fn admin_set_matchmaking(
    ctx: &ReducerContext,
    min_tolerance: f64,
    max_tolerance: f64,
    expand_start_seconds: u32,
    expand_end_seconds: u32,
) -> Result<(), String> {
    let config = require_owner(ctx)?;
    let candidate = engine_realm::matchmaking::MatchmakingConfig {
        min_tolerance,
        max_tolerance,
        expand_start: i64::from(expand_start_seconds) * engine_realm::SECOND,
        expand_end: i64::from(expand_end_seconds) * engine_realm::SECOND,
        ..Default::default()
    };
    if !candidate.is_valid() {
        return Err(RealmError::protocol("That matchmaking configuration is not valid.").into());
    }
    log::info!(
        "event=matchmaking_configured min={min_tolerance} max={max_tolerance} start_s={expand_start_seconds} end_s={expand_end_seconds}"
    );
    ctx.db.realm_config().id().update(RealmConfig {
        mm_min_tolerance: min_tolerance,
        mm_max_tolerance: max_tolerance,
        mm_expand_start_us: candidate.expand_start,
        mm_expand_end_us: candidate.expand_end,
        ..config
    });
    Ok(())
}

/// Tunes the timeouts on a live realm.
#[spacetimedb::reducer]
pub fn admin_set_timeouts(
    ctx: &ReducerContext,
    ready_check_seconds: u32,
    reconnect_grace_seconds: u32,
    queue_disconnect_grace_seconds: u32,
) -> Result<(), String> {
    let config = require_owner(ctx)?;
    if ready_check_seconds == 0 {
        return Err(RealmError::protocol("A ready check needs some time.").into());
    }
    let second = engine_realm::SECOND;
    ctx.db.realm_config().id().update(RealmConfig {
        ready_check_timeout_us: i64::from(ready_check_seconds) * second,
        reconnect_grace_us: i64::from(reconnect_grace_seconds) * second,
        queue_disconnect_grace_us: i64::from(queue_disconnect_grace_seconds) * second,
        ..config
    });
    log::info!(
        "event=timeouts_configured ready_check_s={ready_check_seconds} reconnect_grace_s={reconnect_grace_seconds} queue_grace_s={queue_disconnect_grace_seconds}"
    );
    Ok(())
}

/// Ends an instance as completed or failed. An operator tool, and the integration tests' way
/// to exercise the result pipeline without clearing the dungeon; a real run completes from
/// `combat_tick` when the fight's dungeon run does.
#[spacetimedb::reducer]
pub fn admin_finish_instance(ctx: &ReducerContext, instance_id: u64, success: bool) -> Result<(), String> {
    require_owner(ctx)?;
    crate::instance::finish(ctx, instance_id, success)?;
    Ok(())
}

/// The server's clock, in microseconds since the Unix epoch.
///
/// A client calls this a few times, keeps the round trip with the least delay, and from it
/// estimates the offset between its clock and the server's — which is how three clients show
/// the same countdown number at the same moment.
#[spacetimedb::procedure]
pub fn server_time(ctx: &mut spacetimedb::ProcedureContext) -> i64 {
    ctx.timestamp.to_micros_since_unix_epoch()
}

/// Dismisses one of the caller's notices.
#[spacetimedb::reducer]
pub fn dismiss_notice(ctx: &ReducerContext, notice_id: u64) -> Result<(), String> {
    if let Some(notice) = ctx.db.player_notice().id().find(notice_id)
        && notice.identity == ctx.sender()
    {
        ctx.db.player_notice().id().delete(notice_id);
    }
    Ok(())
}
