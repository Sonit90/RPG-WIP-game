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
use engine_realm::meter::DamageReport;
use engine_realm::modifiers::{encode_modifiers, KeyModifier};
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
    /// The key's modifiers, as stored: every party member sees them before the key goes in.
    pub key_modifiers: Vec<KeyModifier>,
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
    /// JSON text of an `engine_realm::meter::DamageReport`; empty when there is none.
    damage_report: String,
    talent_points: u32,
    /// JSON text: what the player picked up from monsters, `[["gold", 40], ...]`.
    looted: String,
    /// JSON text: `[["aether_shard", 1], ...]`, what the character spent during the run.
    currency_spent: String,
    participant_key_bump: bool,
    own_key_level: u32,
    /// The run's uid (hex). Absent from a module older than the uid.
    #[serde(default)]
    run_uid: String,
    /// `RunOutcome` key. Absent from a module older than outcomes.
    #[serde(default)]
    outcome: String,
    #[serde(default)]
    timer_us: i64,
    /// JSON text: `[RunMember]`; empty for a result that must not write the run.
    #[serde(default)]
    party: String,
    /// JSON text: the key's modifiers.
    #[serde(default)]
    modifiers: String,
}

/// Reads a JSON-text column leniently: empty or malformed text is the default.
fn parse_text<T: serde::de::DeserializeOwned + Default>(text: &str, result_id: &str) -> T {
    if text.trim().is_empty() {
        return T::default();
    }
    serde_json::from_str(text).unwrap_or_else(|e| {
        tracing::warn!(result_id, error = %e, "result column is malformed; using none");
        T::default()
    })
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

/// The damage report a result row carries. Empty text is no report; malformed text is dropped
/// (and says so) rather than failing the result, which still pays.
fn parse_report(text: &str, result_id: &str) -> Option<DamageReport> {
    if text.trim().is_empty() {
        return None;
    }
    let report = DamageReport::from_json(text);
    if report.is_none() {
        tracing::warn!(result_id, "damage report is malformed; dropping it");
    }
    report
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
    quality: u8,
    enhancement: u8,
    broken: bool,
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
                encode_modifiers(&snapshot.key_modifiers),
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
                currency_spent: parse_resources(&row.currency_spent, &row.result_id),
                damage_report: parse_report(&row.damage_report, &row.result_id),
                looted: parse_resources(&row.looted, &row.result_id),
                outcome: engine_realm::outcome::RunOutcome::from_key(&row.outcome),
                timer_us: row.timer_us,
                party: parse_text(&row.party, &row.result_id),
                modifiers: parse_text(&row.modifiers, &row.result_id),
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
                participant_key_bump: row.participant_key_bump,
                own_key_level: row.own_key_level,
                run_uid: row.run_uid,
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
                quality: row.quality,
                sharpening: row.enhancement,
                broken: row.broken,
            })
            .collect())
    }

    /// The operator's snapshot of the live realm (`svc_admin_snapshot`). The procedure returns
    /// the document as JSON text, which SpacetimeDB wraps in a JSON string.
    pub async fn admin_snapshot(
        &self,
        token: &str,
    ) -> Result<engine_realm::ops::AdminSnapshot, BridgeError> {
        let body = self.call(token, "svc_admin_snapshot", json!([])).await?;
        parse_snapshot(&body)
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

/// Reads the snapshot procedure's reply: a JSON string holding the document, or (leniently)
/// the document itself. An empty string is what the module returns to a caller it refuses.
fn parse_snapshot(body: &str) -> Result<engine_realm::ops::AdminSnapshot, BridgeError> {
    let unreadable =
        |e: serde_json::Error| BridgeError::Unavailable(format!("unreadable snapshot: {e}"));
    let value: Value = serde_json::from_str(body).map_err(unreadable)?;
    let document = match value {
        Value::String(text) if text.is_empty() => {
            return Err(BridgeError::Refused(
                "the realm refused the snapshot".into(),
            ));
        }
        Value::String(text) => serde_json::from_str(&text).map_err(unreadable)?,
        other => other,
    };
    serde_json::from_value(document).map_err(unreadable)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_rows_read_positionally() {
        let body = r#"[["12:acct","acct","char","graveyard_crypt",12,true,5000000,250,[1791124476846797],3,true,4,"[[\"crypt_dust\",9]]","",2,"[[\"gold\",12],[\"aether_shard\",1]]","[[\"aether_shard\",1]]",false,0,"0000000000000000000000000000000c"]]"#;
        let rows: Vec<PendingRow> = serde_json::from_str(body).unwrap();
        assert_eq!(rows[0].result_id, "12:acct");
        assert_eq!(rows[0].run_uid, "0000000000000000000000000000000c");
        assert!(rows[0].success);
        assert_eq!(rows[0].experience, 250);
        assert_eq!(rows[0].key_level, 3);
        assert!(rows[0].key_owner);
        assert_eq!(rows[0].key_level_after, 4);
        assert_eq!(rows[0].talent_points, 2);
        assert_eq!(
            parse_resources(&rows[0].looted, "12:acct"),
            vec![("gold".to_string(), 12), ("aether_shard".to_string(), 1)]
        );
        assert_eq!(
            parse_resources(&rows[0].resources, "12:acct"),
            vec![("crypt_dust".to_string(), 9)]
        );
        assert_eq!(
            parse_resources(&rows[0].currency_spent, "12:acct"),
            vec![("aether_shard".to_string(), 1)]
        );
        assert!(!rows[0].participant_key_bump);
        assert_eq!(rows[0].own_key_level, 0);
    }

    #[test]
    fn outcome_timer_party_and_modifiers_read_positionally_and_default() {
        let body = r#"[["u:a","a","c","graveyard_crypt",12,true,5000000,250,[1],3,true,4,"[]","",2,"[]","[]",false,0,"uu","overtime",1500000000,"[{\"character_id\":\"c\",\"name\":\"Ash\",\"hero\":\"Fire Mage\",\"role\":\"dps\"}]","[{\"id\":\"rotating_beams\"}]"]]"#;
        let rows: Vec<PendingRow> = serde_json::from_str(body).unwrap();
        assert_eq!(rows[0].outcome, "overtime");
        assert_eq!(rows[0].timer_us, 1_500_000_000);
        let party: Vec<engine_realm::api::RunMember> = parse_text(&rows[0].party, "r");
        assert_eq!(party[0].name, "Ash");
        let mods: Vec<engine_realm::modifiers::KeyModifier> = parse_text(&rows[0].modifiers, "r");
        assert_eq!(mods[0].id, "rotating_beams");
        let bad: Vec<engine_realm::api::RunMember> = parse_text("{nope", "r");
        assert!(bad.is_empty());
    }

    #[test]
    fn a_row_from_a_module_without_the_uid_still_reads() {
        let body = r#"[["12:b","acct","char","graveyard_crypt",12,false,5000000,0,[1],0,false,0,"[]","",0,"[]","[]",true,2]]"#;
        let rows: Vec<PendingRow> = serde_json::from_str(body).unwrap();
        assert_eq!(rows[0].run_uid, "");
    }

    #[test]
    fn a_row_carries_its_damage_report() {
        let report = r#"{"party":[{"account_id":"acct","character_name":"Ash","class":"Fire Mage","role":"dps"}],"total_damage":1200,"dps":40.0,"duration_seconds":30.0,"boss_kill_seconds":12.5,"abilities":[{"ability":"Firebolt","damage":1200,"casts":10}]}"#;
        let body = serde_json::json!([[
            "12:acct", "acct", "char", "graveyard_crypt", 12, true, 5_000_000, 250, [1], 3, true,
            4, "[]", report, 0, "[]", "[]", false, 0
        ]])
        .to_string();
        let rows: Vec<PendingRow> = serde_json::from_str(&body).unwrap();
        let parsed = parse_report(&rows[0].damage_report, "12:acct").expect("a report");
        assert_eq!(parsed.total_damage, 1200);
        assert_eq!(parsed.boss_kill_seconds, Some(12.5));
        assert_eq!(parsed.abilities[0].ability, "Firebolt");
        assert!(parse_report("", "r").is_none());
        assert!(parse_report("{broken", "r").is_none());
    }

    #[test]
    fn a_non_owner_row_without_resources_reads() {
        let body =
            r#"[["12:b","acct","char","graveyard_crypt",12,false,5000000,0,[1],0,false,0,"[]","",0,"[]","[]",true,2]]"#;
        let rows: Vec<PendingRow> = serde_json::from_str(body).unwrap();
        assert!(!rows[0].key_owner);
        assert!(parse_resources(&rows[0].resources, "12:b").is_empty());
        assert!(rows[0].participant_key_bump);
        assert_eq!(rows[0].own_key_level, 2);
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
        let body = r#"[["12:acct:loot:1","acct","char",12,"head_haste_critical_strike","mail","unclaimed",[1791124476846797],106,3,false]]"#;
        let rows: Vec<LootRow> = serde_json::from_str(body).unwrap();
        assert_eq!(rows[0].loot_id, "12:acct:loot:1");
        assert_eq!(rows[0].state, "mail");
        assert_eq!(rows[0].reason, "unclaimed");
        assert_eq!((rows[0].quality, rows[0].enhancement, rows[0].broken), (106, 3, false));
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

    #[test]
    fn snapshots_read_from_a_json_string_or_a_bare_document() {
        let document = r#"{"generated_at_us":5,"online_accounts":2,"queue":[],"parties":{"ready":1},"instances":[],"max_ticks_behind":0}"#;
        let wrapped = serde_json::to_string(document).unwrap();
        for body in [wrapped.as_str(), document] {
            let snapshot = parse_snapshot(body).unwrap();
            assert_eq!(snapshot.online_accounts, 2);
            assert_eq!(snapshot.parties["ready"], 1);
        }
        assert!(matches!(
            parse_snapshot(r#""""#),
            Err(BridgeError::Refused(_))
        ));
        assert!(matches!(
            parse_snapshot("not json"),
            Err(BridgeError::Unavailable(_))
        ));
    }
}
