//! The bridge to the SpacetimeDB realm.
//!
//! ```text
//! this service ── POST /v1/database/{db}/call/svc_enqueue     ──► realm (queue a snapshot)
//!              ── POST /v1/database/{db}/call/svc_take_results ──► realm (collect results)
//!              ── POST /v1/database/{db}/call/svc_ack_results  ──► realm (clear the outbox)
//! ```
//!
//! Every call carries a short-lived token this service signs for its own service subject.
//! SpacetimeDB verifies it against our JWKS and derives the bridge's identity from it; the
//! module accepts `svc_*` calls from that identity and nobody else. The realm never calls out,
//! so it needs no secret and no route to this service.

use engine_realm::api::DungeonResult;
use engine_realm::Role;
use serde::Deserialize;
use serde_json::{json, Value};

/// Why a realm call did not succeed.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// The realm refused (a reducer returned an error). Carries the realm's wire message.
    #[error("{0}")]
    Refused(String),
    /// The realm could not be reached or answered nonsense.
    #[error("realm unavailable: {0}")]
    Unavailable(String),
}

/// A client for the realm's service entry points.
#[derive(Clone)]
pub struct RealmBridge {
    http: reqwest::Client,
    base: String,
    database: String,
}

/// What the module stores for a character entering the queue: everything it needs, nothing
/// a client could have supplied.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub account_id: String,
    pub character_id: String,
    pub character_name: String,
    pub definition_id: String,
    pub level: u32,
    pub gear_score: u32,
    /// The stored loadout as JSON: what the realm dresses the character in for the fight.
    pub loadout: String,
}

/// SpacetimeDB's JSON for a unit enum variant: the variant's name in snake_case.
fn role_json(role: Role) -> Value {
    json!({ role.key(): [] })
}

/// One `PendingResult` row. SpacetimeDB's JSON encodes a product value returned by a
/// procedure as an array in field order, so this struct must list the module's fields in the
/// module's order (`server/realm-module/src/schema.rs`); serde reads a struct from a sequence
/// positionally. A field added there is added here, at the same position.
#[derive(Deserialize)]
struct PendingRow {
    result_id: String,
    account_id: String,
    character_id: String,
    dungeon_id: String,
    #[allow(dead_code)]
    instance_id: u64,
    success: bool,
    duration_us: i64,
    experience: u32,
    #[allow(dead_code)]
    created_at: Value,
}

impl RealmBridge {
    pub fn new(http: reqwest::Client, base: &str, database: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            database: database.to_string(),
        }
    }

    async fn call(&self, token: &str, function: &str, args: Value) -> Result<String, BridgeError> {
        let url = format!("{}/v1/database/{}/call/{}", self.base, self.database, function);
        let response = self
            .http
            .post(&url)
            .bearer_auth(token)
            .json(&args)
            .send()
            .await
            .map_err(|e| BridgeError::Unavailable(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| BridgeError::Unavailable(e.to_string()))?;
        if status.is_success() {
            Ok(body)
        } else if status.as_u16() == 530 || status.is_client_error() {
            // 530: the reducer ran and returned an error — a refusal, with the realm's message.
            Err(BridgeError::Refused(body.trim().to_string()))
        } else {
            Err(BridgeError::Unavailable(format!("{status}: {}", body.trim())))
        }
    }

    /// Queues a character for a dungeon as a role.
    pub async fn enqueue(
        &self,
        token: &str,
        snapshot: &Snapshot,
        dungeon_id: &str,
        role: Role,
    ) -> Result<(), BridgeError> {
        self.call(
            token,
            "svc_enqueue",
            json!([
                snapshot.account_id,
                snapshot.character_id,
                snapshot.character_name,
                snapshot.definition_id,
                snapshot.level,
                snapshot.gear_score,
                snapshot.loadout,
                dungeon_id,
                role_json(role),
            ]),
        )
        .await
        .map(|_| ())
    }

    /// Results waiting in the realm's outbox.
    pub async fn take_results(&self, token: &str) -> Result<Vec<DungeonResult>, BridgeError> {
        let body = self.call(token, "svc_take_results", json!([])).await?;
        let rows: Vec<PendingRow> = serde_json::from_str(&body)
            .map_err(|e| BridgeError::Unavailable(format!("unreadable results: {e}")))?;
        Ok(rows
            .into_iter()
            .map(|row| DungeonResult {
                result_id: row.result_id,
                account_id: row.account_id,
                character_id: row.character_id,
                dungeon_id: row.dungeon_id,
                success: row.success,
                duration_us: row.duration_us,
                experience: row.experience,
            })
            .collect())
    }

    /// Clears results that were persisted.
    pub async fn ack_results(&self, token: &str, ids: &[String]) -> Result<(), BridgeError> {
        self.call(token, "svc_ack_results", json!([ids])).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_rows_read_positionally() {
        let body = r#"[["12:acct","acct","char","graveyard_crypt",12,true,5000000,250,[1791124476846797]]]"#;
        let rows: Vec<PendingRow> = serde_json::from_str(body).unwrap();
        assert_eq!(rows[0].result_id, "12:acct");
        assert!(rows[0].success);
        assert_eq!(rows[0].experience, 250);
    }

    #[test]
    fn roles_encode_as_spacetimedb_sum_types() {
        assert_eq!(role_json(Role::Tank), json!({"tank": []}));
        assert_eq!(role_json(Role::Dps), json!({"dps": []}));
    }
}
