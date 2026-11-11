//! The background task that moves dungeon results and chest loot from the realm into the
//! database.
//!
//! ```text
//! every N seconds: take results ─► apply each (idempotent, one transaction) ─► acknowledge
//!                  take loot    ─► deliver each (idempotent, one transaction) ─► acknowledge
//! ```
//!
//! A result (or an item) is acknowledged only after it is committed, so a crash anywhere
//! re-delivers it on the next pass, and the idempotency key turns the re-delivery into a no-op.

use std::time::Duration;

use realm_game::store;

use crate::state::AppState;

pub async fn collect_results(state: AppState) {
    let period = Duration::from_secs(state.settings.realm.result_poll_secs.max(1));
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut unavailable = false;
    let mut last_sync = std::time::Instant::now() - COMMUNITY_SYNC_EVERY;
    loop {
        interval.tick().await;
        let token = match state
            .jwt
            .issue_service(&state.settings.realm.service_subject)
        {
            Ok(token) => token,
            Err(e) => {
                tracing::error!(error = %e, "realm bridge: cannot sign a service token");
                continue;
            }
        };
        let results = match state.realm.take_results(&token).await {
            Ok(results) => {
                state.ops.bridge_pass_ok(results.len());
                if unavailable {
                    tracing::info!("realm bridge: realm reachable again");
                    unavailable = false;
                }
                results
            }
            Err(e) => {
                if !unavailable {
                    tracing::warn!(error = %e, "realm bridge: cannot collect results");
                    unavailable = true;
                }
                state
                    .ops
                    .bridge_pass_failed(&format!("cannot collect results: {e}"));
                continue;
            }
        };
        persist_results(&state, &token, &results).await;
        collect_loot(&state, &token).await;
        if last_sync.elapsed() >= COMMUNITY_SYNC_EVERY {
            last_sync = std::time::Instant::now();
            sync_community(&state, &token).await;
        }
    }
}

/// How often the realm's community maps are compared with the database's published ones.
const COMMUNITY_SYNC_EVERY: Duration = Duration::from_secs(30);

/// Makes the realm's community maps match the database: publishes what the realm lost (a schema
/// publish wipes its data) and drops what is no longer published (a withdraw that failed).
async fn sync_community(state: &AppState, token: &str) {
    use std::collections::BTreeSet;
    let live = match state.realm.community_list(token).await {
        Ok(live) => live,
        // A realm module without community maps yet, or a blip: the next pass tries again.
        Err(e) => {
            tracing::debug!(error = %e, "community sync: cannot list the realm's maps");
            return;
        }
    };
    let published = match realm_game::community::published(&state.db).await {
        Ok(published) => published,
        Err(e) => {
            tracing::warn!(error = %e, "community sync: cannot read the published maps");
            return;
        }
    };
    let wanted: BTreeSet<String> = published
        .iter()
        .map(|(id, version)| engine_realm::community::dungeon_id(&id.to_string(), *version))
        .collect();
    let mut present: BTreeSet<String> = live.iter().cloned().collect();
    // Stale entries (a version that is no longer the published one, or a map that is not
    // published at all) go first, so a newer version can then be put.
    let mut dropped = BTreeSet::new();
    for key in live.iter().filter(|key| !wanted.contains(*key)) {
        let Some((id, _)) = engine_realm::community::parse_dungeon_id(key) else {
            continue;
        };
        if dropped.insert(id.to_owned()) {
            if let Err(e) = state.realm.community_remove(token, id).await {
                tracing::warn!(map_id = id, error = %e, "community sync: cannot drop a map");
            }
        }
    }
    present.retain(|key| {
        engine_realm::community::parse_dungeon_id(key).is_none_or(|(id, _)| !dropped.contains(id))
    });
    for (id, version) in &published {
        let key = engine_realm::community::dungeon_id(&id.to_string(), *version);
        if present.contains(&key) {
            continue;
        }
        match realm_game::community::playable(&state.db, *id, *version).await {
            Ok(Some(playable)) => {
                if let Err(e) = state.realm.community_put(token, &key, &playable.data).await {
                    tracing::warn!(%key, error = %e, "community sync: the realm did not take a map");
                } else {
                    tracing::info!(%key, "community sync: map put in the realm");
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(%key, error = %e, "community sync: cannot read a map"),
        }
    }
}

async fn persist_results(
    state: &AppState,
    token: &str,
    results: &[engine_realm::api::DungeonResult],
) {
    if results.is_empty() {
        return;
    }
    let mut persisted = Vec::with_capacity(results.len());
    for result in results {
        match store::apply_result(&state.db, result).await {
            Ok(_) => persisted.push(result.result_id.clone()),
            Err(e) => tracing::error!(
                error = %e,
                result_id = %result.result_id,
                account_id = %result.account_id,
                "persistence failure: dungeon result not applied; will retry"
            ),
        }
    }
    if !persisted.is_empty() {
        if let Err(e) = state.realm.ack_results(token, &persisted).await {
            crate::ops::errlog::error_log()
                .push("bridge", &format!("result acknowledgement failed: {e}"));
            tracing::warn!(error = %e, count = persisted.len(), "realm bridge: acknowledgement failed; results will be re-delivered");
        }
    }
}

/// Chest items: applied once each, then acknowledged.
async fn collect_loot(state: &AppState, token: &str) {
    let deliveries = match state.realm.take_loot(token).await {
        Ok(deliveries) => deliveries,
        // A realm module without the loot outbox yet, or a blip: the next pass tries again.
        Err(e) => {
            tracing::debug!(error = %e, "realm bridge: cannot collect loot");
            return;
        }
    };
    state.ops.bridge_loot_pending(deliveries.len());
    let mut persisted = Vec::with_capacity(deliveries.len());
    for delivery in &deliveries {
        match store::apply_loot(&state.db, delivery).await {
            Ok(_) => persisted.push(delivery.loot_id.clone()),
            Err(e) => tracing::error!(
                error = %e,
                loot_id = %delivery.loot_id,
                "persistence failure: dungeon loot not applied; will retry"
            ),
        }
    }
    if !persisted.is_empty() {
        if let Err(e) = state.realm.ack_loot(token, &persisted).await {
            crate::ops::errlog::error_log()
                .push("bridge", &format!("loot acknowledgement failed: {e}"));
            tracing::warn!(error = %e, count = persisted.len(), "realm bridge: loot acknowledgement failed; it will be re-delivered");
        }
    }
}
