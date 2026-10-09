//! The `game` schema: queries only, no HTTP. Every character query is scoped by account, so
//! a caller can never read or write a character that is not theirs — the WHERE clause, not a
//! check after the fact, is the ownership rule.

use engine_realm::api::{DungeonCompletion, DungeonResult, LootDelivery};
use engine_realm::loadout::{
    add_to_bag, add_to_mail, migrate_removed_slots, starting_loadout, BagError, Loadout,
    MAIL_BAG_FULL, MAIL_UNCLAIMED,
};
use generic_auth_core::AppError;
use sqlx::types::Json;
use sqlx::PgPool;
use uuid::Uuid;

/// One character row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CharacterRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub definition_id: String,
    pub name: String,
    pub level: i32,
    pub experience: i64,
    pub loadout: Json<Loadout>,
    pub talents: Json<Vec<String>>,
}

fn db(e: sqlx::Error) -> AppError {
    AppError::Database(e.to_string())
}

const COLUMNS: &str = "id, account_id, definition_id, name, level, experience, loadout, talents";

/// The account's characters, creating one of each starter definition it does not have yet.
pub async fn list_or_provision(
    pool: &PgPool,
    account_id: Uuid,
) -> Result<Vec<CharacterRow>, AppError> {
    let catalog = crate::catalog();
    for definition in catalog.starters() {
        let loadout = starting_loadout(definition.hero);
        let inserted = sqlx::query(
            "INSERT INTO game.characters (account_id, definition_id, name, loadout)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (account_id, definition_id) DO NOTHING",
        )
        .bind(account_id)
        .bind(&definition.id)
        .bind(&definition.name)
        .bind(Json(&loadout))
        .execute(pool)
        .await
        .map_err(db)?;
        if inserted.rows_affected() > 0 {
            tracing::info!(account_id = %account_id, definition_id = %definition.id, "character provisioned");
        }
    }
    sqlx::query_as::<_, CharacterRow>(&format!(
        "SELECT {COLUMNS} FROM game.characters WHERE account_id = $1 ORDER BY created_at, definition_id"
    ))
    .bind(account_id)
    .fetch_all(pool)
    .await
    .map_err(db)
}

/// One of the account's characters. `None` if it does not exist *or is someone else's*.
pub async fn get(
    pool: &PgPool,
    account_id: Uuid,
    id: Uuid,
) -> Result<Option<CharacterRow>, AppError> {
    sqlx::query_as::<_, CharacterRow>(&format!(
        "SELECT {COLUMNS} FROM game.characters WHERE id = $1 AND account_id = $2"
    ))
    .bind(id)
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .map_err(db)
}

/// Replaces a character's loadout. `false` if it is not the account's.
pub async fn save_loadout(
    pool: &PgPool,
    account_id: Uuid,
    id: Uuid,
    loadout: &Loadout,
) -> Result<bool, AppError> {
    let done = sqlx::query(
        "UPDATE game.characters SET loadout = $3, updated_at = now() WHERE id = $1 AND account_id = $2",
    )
    .bind(id)
    .bind(account_id)
    .bind(Json(loadout))
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(done.rows_affected() == 1)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Read-modify-write of a character's loadout under a row lock, so a concurrent write (a
/// chest drop applied by the bridge) cannot be overwritten with a stale copy. `change` gets
/// the stored loadout and returns the new one, or the error to report; nothing is saved then.
/// `Ok(None)` if the character is not the account's.
pub async fn update_loadout<F>(
    pool: &PgPool,
    account_id: Uuid,
    id: Uuid,
    change: F,
) -> Result<Option<Loadout>, AppError>
where
    F: FnOnce(&Loadout) -> Result<Loadout, AppError>,
{
    let mut tx = pool.begin().await.map_err(db)?;
    let stored: Option<Json<Loadout>> = sqlx::query_scalar(
        "SELECT loadout FROM game.characters WHERE id = $1 AND account_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(account_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    let Some(Json(mut current)) = stored else {
        return Ok(None);
    };
    // A character saved while there were four trinket slots: rehome those items first.
    migrate_removed_slots(&mut current, unix_now());
    let changed = change(&current)?;
    sqlx::query("UPDATE game.characters SET loadout = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(Json(&changed))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Some(changed))
}

/// Replaces a character's talents. `false` if it is not the account's.
pub async fn save_talents(
    pool: &PgPool,
    account_id: Uuid,
    id: Uuid,
    talents: &[String],
) -> Result<bool, AppError> {
    let done = sqlx::query(
        "UPDATE game.characters SET talents = $3, updated_at = now() WHERE id = $1 AND account_id = $2",
    )
    .bind(id)
    .bind(account_id)
    .bind(Json(talents))
    .execute(pool)
    .await
    .map_err(db)?;
    Ok(done.rows_affected() == 1)
}

/// How often a character finished each dungeon.
pub async fn completions(
    pool: &PgPool,
    character_id: Uuid,
) -> Result<Vec<DungeonCompletion>, AppError> {
    let rows: Vec<(String, i32)> = sqlx::query_as(
        "SELECT dungeon_id, count FROM game.dungeon_completions WHERE character_id = $1 ORDER BY dungeon_id",
    )
    .bind(character_id)
    .fetch_all(pool)
    .await
    .map_err(db)?;
    Ok(rows
        .into_iter()
        .map(|(dungeon_id, count)| DungeonCompletion {
            dungeon_id,
            count: u32::try_from(count).unwrap_or(0),
        })
        .collect())
}

/// The account's settings document and its version, if any were saved.
pub async fn settings(
    pool: &PgPool,
    account_id: Uuid,
) -> Result<Option<(i64, serde_json::Value)>, AppError> {
    sqlx::query_as::<_, (i64, Json<serde_json::Value>)>(
        "SELECT version, settings FROM game.account_settings WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .map(|row| row.map(|(version, Json(value))| (version, value)))
    .map_err(db)
}

/// Saves settings if `expected` is the stored version (0 for "none stored yet"). Returns the
/// new version, or a conflict if another machine saved first.
pub async fn save_settings(
    pool: &PgPool,
    account_id: Uuid,
    expected: i64,
    value: &serde_json::Value,
) -> Result<i64, AppError> {
    let saved: Option<i64> = if expected == 0 {
        sqlx::query_scalar(
            "INSERT INTO game.account_settings (account_id, version, settings) VALUES ($1, 1, $2)
             ON CONFLICT (account_id) DO NOTHING RETURNING version",
        )
        .bind(account_id)
        .bind(Json(value))
        .fetch_optional(pool)
        .await
        .map_err(db)?
    } else {
        sqlx::query_scalar(
            "UPDATE game.account_settings SET settings = $3, version = version + 1, updated_at = now()
             WHERE account_id = $1 AND version = $2 RETURNING version",
        )
        .bind(account_id)
        .bind(expected)
        .bind(Json(value))
        .fetch_optional(pool)
        .await
        .map_err(db)?
    };
    saved.ok_or_else(|| AppError::Conflict("Settings were changed on another machine.".into()))
}

/// Applies one realm result exactly once. Returns whether it was new.
///
/// Inside one transaction: record the result under its idempotency key; if that key was
/// already recorded, stop. Otherwise credit the completion and the experience — computed by
/// the realm, applied here with the shared progression curve. Nothing in it came from a client.
pub async fn apply_result(pool: &PgPool, result: &DungeonResult) -> Result<bool, AppError> {
    let account_id = Uuid::parse_str(&result.account_id)
        .map_err(|_| AppError::validation("result has a malformed account id"))?;
    let character_id = Uuid::parse_str(&result.character_id).ok();
    let mut tx = pool.begin().await.map_err(db)?;
    let recorded = sqlx::query(
        "INSERT INTO game.dungeon_results
            (result_id, account_id, character_id, dungeon_id, success, duration_us, experience)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (result_id) DO NOTHING",
    )
    .bind(&result.result_id)
    .bind(account_id)
    .bind(character_id)
    .bind(&result.dungeon_id)
    .bind(result.success)
    .bind(result.duration_us)
    .bind(i32::try_from(result.experience).unwrap_or(i32::MAX))
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    if recorded.rows_affected() == 0 {
        tx.commit().await.map_err(db)?;
        return Ok(false);
    }
    if let Some(character_id) = character_id {
        let row: Option<(i32, i64)> = sqlx::query_as(
            "SELECT level, experience FROM game.characters WHERE id = $1 AND account_id = $2 FOR UPDATE",
        )
        .bind(character_id)
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        if let Some((level, experience)) = row {
            let (level, experience) = engine_realm::progression::gain(
                u32::try_from(level).unwrap_or(1),
                u64::try_from(experience).unwrap_or(0),
                u64::from(result.experience),
            );
            sqlx::query("UPDATE game.characters SET level = $2, experience = $3, updated_at = now() WHERE id = $1")
                .bind(character_id)
                .bind(i32::try_from(level).unwrap_or(1))
                .bind(i64::try_from(experience).unwrap_or(0))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            if result.success {
                sqlx::query(
                    "INSERT INTO game.dungeon_completions (character_id, dungeon_id, count, best_duration_us, last_completed)
                     VALUES ($1, $2, 1, $3, now())
                     ON CONFLICT (character_id, dungeon_id) DO UPDATE SET
                        count = game.dungeon_completions.count + 1,
                        best_duration_us = LEAST(game.dungeon_completions.best_duration_us, EXCLUDED.best_duration_us),
                        last_completed = now()",
                )
                .bind(character_id)
                .bind(&result.dungeon_id)
                .bind(result.duration_us)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }
        }
    }
    tx.commit().await.map_err(db)?;
    tracing::info!(
        result_id = %result.result_id,
        account_id = %result.account_id,
        character_id = %result.character_id,
        dungeon_id = %result.dungeon_id,
        success = result.success,
        experience = result.experience,
        "dungeon result applied"
    );
    Ok(true)
}

/// Delivers one chest item exactly once. Returns whether it was new.
///
/// One transaction: record the delivery under its idempotency key (stop if seen), lock the
/// character, then put the item in the first free bag cell (`bag`, when there is room) or in
/// the mailbox (`mail`, or a `bag` delivery that found the bag full). Nothing is ever dropped
/// silently: when it cannot be given, the row says why.
pub async fn apply_loot(pool: &PgPool, delivery: &LootDelivery) -> Result<bool, AppError> {
    let account_id = Uuid::parse_str(&delivery.account_id)
        .map_err(|_| AppError::validation("loot has a malformed account id"))?;
    let character_id = Uuid::parse_str(&delivery.character_id).ok();
    let kind = if delivery.kind == "bag" { "bag" } else { "mail" };
    let mut tx = pool.begin().await.map_err(db)?;
    let recorded = sqlx::query(
        "INSERT INTO game.loot_deliveries (loot_id, account_id, character_id, item, kind, reason, outcome)
         VALUES ($1, $2, $3, $4, $5, $6, 'pending')
         ON CONFLICT (loot_id) DO NOTHING",
    )
    .bind(&delivery.loot_id)
    .bind(account_id)
    .bind(character_id)
    .bind(&delivery.item)
    .bind(kind)
    .bind(&delivery.reason)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    if recorded.rows_affected() == 0 {
        tx.commit().await.map_err(db)?;
        return Ok(false);
    }
    let stored: Option<Json<Loadout>> = match character_id {
        Some(id) => sqlx::query_scalar(
            "SELECT loadout FROM game.characters WHERE id = $1 AND account_id = $2 FOR UPDATE",
        )
        .bind(id)
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?,
        None => None,
    };
    let outcome = match (character_id, stored) {
        (Some(id), Some(Json(mut loadout))) => {
            let now = unix_now();
            migrate_removed_slots(&mut loadout, now);
            let outcome = if kind == "bag" {
                match add_to_bag(&mut loadout, &delivery.item) {
                    Ok(_) => Ok("bag"),
                    Err(BagError::Full) => {
                        add_to_mail(&mut loadout, &delivery.item, MAIL_BAG_FULL, now)
                            .map(|_| "mail_bag_full")
                    }
                    Err(error) => Err(error),
                }
            } else {
                let reason = if delivery.reason == MAIL_BAG_FULL {
                    MAIL_BAG_FULL
                } else {
                    MAIL_UNCLAIMED
                };
                add_to_mail(&mut loadout, &delivery.item, reason, now).map(|_| "mail")
            };
            match outcome {
                Ok(outcome) => {
                    sqlx::query(
                        "UPDATE game.characters SET loadout = $2, updated_at = now() WHERE id = $1",
                    )
                    .bind(id)
                    .bind(Json(&loadout))
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                    outcome
                }
                Err(_) => "unknown_item",
            }
        }
        _ => "no_character",
    };
    sqlx::query("UPDATE game.loot_deliveries SET outcome = $2 WHERE loot_id = $1")
        .bind(&delivery.loot_id)
        .bind(outcome)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;
    if outcome == "bag" || outcome.starts_with("mail") {
        tracing::info!(loot_id = %delivery.loot_id, item = %delivery.item, outcome, "dungeon loot delivered");
    } else {
        tracing::warn!(loot_id = %delivery.loot_id, item = %delivery.item, outcome, "dungeon loot not delivered");
    }
    Ok(true)
}
