//! The background task that moves dungeon results from the realm into the database.
//!
//! ```text
//! every N seconds: take results ─► apply each (idempotent, one transaction) ─► acknowledge
//! ```
//!
//! A result is acknowledged only after it is committed, so a crash anywhere re-delivers it on
//! the next pass, and `apply_result`'s idempotency key turns the re-delivery into a no-op.

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
        let token = match state.jwt.issue_service(&state.settings.realm.service_subject) {
            Ok(token) => token,
            Err(e) => {
                tracing::error!(error = %e, "realm bridge: cannot sign a service token");
                continue;
            }
        };
        let results = match state.realm.take_results(&token).await {
            Ok(results) => {
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
                continue;
            }
        };
        if results.is_empty() {
            continue;
        }
        let mut persisted = Vec::with_capacity(results.len());
        for result in &results {
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
            if let Err(e) = state.realm.ack_results(&token, &persisted).await {
                tracing::warn!(error = %e, count = persisted.len(), "realm bridge: acknowledgement failed; results will be re-delivered");
            }
        }
    }
}
