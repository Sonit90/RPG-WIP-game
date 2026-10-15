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

use engine_realm::api::{DungeonResult, Engagement, LootDelivery};
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
    /// The dungeon the character's key opens; empty when it has no key.
    pub key_dungeon_id: String,
    /// The key's level; 0 when it has no key.
    pub key_level: u32,
    /// The character's talents, by key: the realm builds the fight from these.
    pub talents: Vec<String>,
    /// Every talent point the character has (starting plus earned): what the keys must fit in.
    pub talent_points: u32,
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
    key_level: u32,
    key_owner: bool,
    key_level_after: u32,
    /// JSON text: `[["crypt_dust", 9], ...]`.
    resources: String,
    talent_points: u32,
}

/// What a realm answer means for a change to a character's talents or equipment.
///
/// Engaged is a 409 with the player-facing sentence. An unreachable realm also refuses (a 500):
/// the lock fails closed, because an unchecked change could slip in while the player is queued.
///
/// # Errors
///
/// The change must not happen.
pub fn lock_decision(
    answer: Result<Engagement, BridgeError>,
) -> Result<(), generic_auth_core::AppError> {
    match answer {
        Ok(Engagement { engaged: false, .. }) => Ok(()),
        Ok(_) => Err(generic_auth_core::AppError::Conflict(
            engine_realm::api::ENGAGED_MESSAGE.into(),
        )),
        Err(BridgeError::Refused(message)) => {
            tracing::warn!(%message, "realm refused the engagement check");
            Err(generic_auth_core::AppError::internal(
                "The game server is unavailable.",
            ))
        }
        Err(BridgeError::Unavailable(reason)) => {
            tracing::error!(%reason, "realm unavailable for the engagement check");
            Err(generic_auth_core::AppError::internal(
                "The game server is unavailable.",
            ))
        }
    }
}

/// The resources a result row carries. Malformed text pays nothing (and says so).
fn parse_resources(text: &str, result_id: &str) -> Vec<(String, u32)> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str(text).unwrap_or_else(|e| {
        tracing::warn!(result_id, error = %e, "result resources are malformed; paying none");
        Vec::new()
    })
}

/// One `PendingLoot` row, positional like [`PendingRow`]: `server/realm-module/src/schema.rs`.
#[derive(Deserialize)]
struct LootRow {
    loot_id: String,
    account_id: String,
    character_id: String,
    #[allow(dead_code)]
    instance_id: u64,
    item: String,
    state: String,
    reason: String,
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
        let url = format!(
            "{}/v1/database/{}/call/{}",
            self.base, self.database, function
        );
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
            Err(BridgeError::Unavailable(format!(
                "{status}: {}",
                body.trim()
            )))
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
                snapshot.key_dungeon_id,
                snapshot.key_level,
                engine_realm::talents::encode_keys(&snapshot.talents),
                snapshot.talent_points,
            ]),
        )
        .await
        .map(|_| ())
    }

    /// Whether the account has a character queued, in a party or in an instance.
    pub async fn engagement(
        &self,
        token: &str,
        account_id: &str,
    ) -> Result<Engagement, BridgeError> {
        let body = self
            .call(token, "svc_engagement", json!([account_id]))
            .await?;
        // A product value comes back as an array in field order: `[true, "queued"]`.
        let (engaged, state): (bool, String) = serde_json::from_str(&body)
            .map_err(|e| BridgeError::Unavailable(format!("unreadable engagement: {e}")))?;
        Ok(Engagement { engaged, state })
    }

    /// Results waiting in the realm's outbox.
    pub async fn take_results(&self, token: &str) -> Result<Vec<DungeonResult>, BridgeError> {
        let body = self.call(token, "svc_take_results", json!([])).await?;
        let rows: Vec<PendingRow> = serde_json::from_str(&body)
            .map_err(|e| BridgeError::Unavailable(format!("unreadable results: {e}")))?;
        Ok(rows
            .into_iter()
            .map(|row| DungeonResult {
                resources: parse_resources(&row.resources, &row.result_id),
                result_id: row.result_id,
                account_id: row.account_id,
                character_id: row.character_id,
                dungeon_id: row.dungeon_id,
                success: row.success,
                duration_us: row.duration_us,
                experience: row.experience,
                key_level: row.key_level,
                key_owner: row.key_owner,
                key_level_after: row.key_level_after,
                talent_points: row.talent_points,
            })
            .collect())
    }

    /// Chest items waiting in the realm's outbox.
    pub async fn take_loot(&self, token: &str) -> Result<Vec<LootDelivery>, BridgeError> {
        let body = self.call(token, "svc_take_loot", json!([])).await?;
        let rows: Vec<LootRow> = serde_json::from_str(&body)
            .map_err(|e| BridgeError::Unavailable(format!("unreadable loot: {e}")))?;
        Ok(rows
            .into_iter()
            .map(|row| LootDelivery {
                loot_id: row.loot_id,
                account_id: row.account_id,
                character_id: row.character_id,
                item: row.item,
                kind: row.state,
                reason: row.reason,
            })
            .collect())
    }

    /// Clears chest items that were persisted.
    pub async fn ack_loot(&self, token: &str, ids: &[String]) -> Result<(), BridgeError> {
        self.call(token, "svc_ack_loot", json!([ids]))
            .await
            .map(|_| ())
    }

    /// Clears results that were persisted.
    pub async fn ack_results(&self, token: &str, ids: &[String]) -> Result<(), BridgeError> {
        self.call(token, "svc_ack_results", json!([ids]))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_rows_read_positionally() {
        let body = r#"[["12:acct","acct","char","graveyard_crypt",12,true,5000000,250,[1791124476846797],3,true,4,"[[\"crypt_dust\",9]]",2]]"#;
        let rows: Vec<PendingRow> = serde_json::from_str(body).unwrap();
        assert_eq!(rows[0].result_id, "12:acct");
        assert!(rows[0].success);
        assert_eq!(rows[0].experience, 250);
        assert_eq!(rows[0].key_level, 3);
        assert!(rows[0].key_owner);
        assert_eq!(rows[0].key_level_after, 4);
        assert_eq!(rows[0].talent_points, 2);
        assert_eq!(
            parse_resources(&rows[0].resources, "12:acct"),
            vec![("crypt_dust".to_string(), 9)]
        );
    }

    #[test]
    fn a_non_owner_row_without_resources_reads() {
        let body =
            r#"[["12:b","acct","char","graveyard_crypt",12,false,5000000,0,[1],0,false,0,"[]",0]]"#;
        let rows: Vec<PendingRow> = serde_json::from_str(body).unwrap();
        assert!(!rows[0].key_owner);
        assert!(parse_resources(&rows[0].resources, "12:b").is_empty());
    }

    #[test]
    fn malformed_or_empty_resources_pay_nothing() {
        assert!(parse_resources("not json", "r").is_empty());
        assert!(parse_resources("", "r").is_empty());
        assert!(parse_resources(r#"[["dust","many"]]"#, "r").is_empty());
        assert_eq!(
            parse_resources(r#"[["a",1],["b",2]]"#, "r"),
            vec![("a".to_string(), 1), ("b".to_string(), 2)]
        );
    }

    #[test]
    fn loot_rows_read_positionally() {
        let body = r#"[["12:acct:loot","acct","char",12,"head_haste_critical_strike","mail","unclaimed",[1791124476846797]]]"#;
        let rows: Vec<LootRow> = serde_json::from_str(body).unwrap();
        assert_eq!(rows[0].loot_id, "12:acct:loot");
        assert_eq!(rows[0].state, "mail");
        assert_eq!(rows[0].reason, "unclaimed");
    }

    fn engaged(state: &str) -> Result<Engagement, BridgeError> {
        Ok(Engagement {
            engaged: state != "none",
            state: state.to_owned(),
        })
    }

    #[test]
    fn an_engaged_account_is_locked_out_with_the_players_sentence() {
        assert!(lock_decision(engaged("none")).is_ok());
        for state in ["queued", "party", "instance"] {
            match lock_decision(engaged(state)) {
                Err(generic_auth_core::AppError::Conflict(message)) => {
                    assert_eq!(message, engine_realm::api::ENGAGED_MESSAGE);
                }
                other => panic!("{state}: {other:?}"),
            }
        }
    }

    #[test]
    fn the_lock_fails_closed_when_the_realm_cannot_answer() {
        assert!(lock_decision(Err(BridgeError::Unavailable("down".into()))).is_err());
        assert!(lock_decision(Err(BridgeError::Refused("no".into()))).is_err());
    }

    #[test]
    fn roles_encode_as_spacetimedb_sum_types() {
        assert_eq!(role_json(Role::Tank), json!({"tank": []}));
        assert_eq!(role_json(Role::Dps), json!({"dps": []}));
    }
}
