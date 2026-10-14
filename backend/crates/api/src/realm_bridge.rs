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
