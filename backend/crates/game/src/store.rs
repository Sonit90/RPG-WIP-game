//! The `game` schema: queries only, no HTTP. Every character query is scoped by account, so
//! a caller can never read or write a character that is not theirs — the WHERE clause, not a
//! check after the fact, is the ownership rule.

use engine_realm::api::{DungeonCompletion, DungeonResult, LootDelivery};
use engine_realm::economy::{add_resources, debit_resources, spend_resource};
use engine_realm::loadout::{
    add_to_bag, add_to_mail, migrate_removed_slots, starting_loadout, BagError, Loadout,
    MAIL_BAG_FULL, MAIL_UNCLAIMED,
};
use engine_realm::modifiers::KeyModifier;
use generic_auth_core::AppError;
use sqlx::types::Json;
use sqlx::PgPool;
use std::collections::HashMap;
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
    /// Talent points earned from runs; the starting points come from the catalog.
    pub talent_points_earned: i32,
}

/// A character's dungeon key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRow {
    pub dungeon_id: String,
    pub level: u32,
    /// The modifiers the key carries, rolled when it was created or its level last changed.
    pub modifiers: Vec<KeyModifier>,
    /// Whether the modifiers were ever rolled. A key that was not (an older key, or one created
    /// for a level with slots) is rolled once, the next time it is read.
    pub modifiers_rolled: bool,
}

fn db(e: sqlx::Error) -> AppError {
    AppError::Database(e.to_string())
}

const COLUMNS: &str =
    "id, account_id, definition_id, name, level, experience, loadout, talents, talent_points_earned";

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
    // One key per character, for the first dungeon at its lowest level; also back-fills the
    // characters that predate keys. A key that exists is never touched.
    if let Some(dungeon) = catalog.dungeons().first() {
        sqlx::query(
            "INSERT INTO game.dungeon_keys (character_id, account_id, dungeon_id, level)
             SELECT id, account_id, $2, $3 FROM game.characters WHERE account_id = $1
             ON CONFLICT DO NOTHING",
        )
        .bind(account_id)
        .bind(&dungeon.id)
        .bind(i32::try_from(dungeon.keys.min_level.max(1)).unwrap_or(1))
        .execute(pool)
        .await
        .map_err(db)?;
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

type KeyColumns = (Uuid, String, i32, Json<serde_json::Value>, bool);

const KEY_COLUMNS: &str = "character_id, dungeon_id, level, modifiers, modifiers_rolled";

fn key_row((_, dungeon_id, level, Json(modifiers), modifiers_rolled): KeyColumns) -> KeyRow {
    KeyRow {
        dungeon_id,
        level: u32::try_from(level).unwrap_or(1),
        modifiers: serde_json::from_value(modifiers).unwrap_or_default(),
        modifiers_rolled,
    }
}

/// A seed from the operating system's random source: a version 4 UUID is exactly that.
fn fresh_seed() -> u64 {
    // The low 64 bits of 122 random ones.
    (Uuid::new_v4().as_u128() & u128::from(u64::MAX)) as u64
}

/// The modifiers a key of `level` starts with, rolled from the OS random source.
#[must_use]
pub fn roll_modifiers(level: u32) -> Vec<KeyModifier> {
    crate::catalog()
        .modifiers()
        .roll_key_modifiers(level, fresh_seed())
}

/// Rolls a key's modifiers if they never were, once: the update only lands on a key still
/// marked unrolled, so two concurrent reads cannot both decide. Returns the key as stored.
async fn ensure_rolled(
    pool: &PgPool,
    account_id: Uuid,
    character_id: Uuid,
    mut key: KeyRow,
) -> Result<KeyRow, AppError> {
    if key.modifiers_rolled {
        return Ok(key);
    }
    let rolled = roll_modifiers(key.level);
    let done = sqlx::query(
        "UPDATE game.dungeon_keys SET modifiers = $3, modifiers_rolled = true, updated_at = now()
         WHERE character_id = $1 AND account_id = $2 AND modifiers_rolled = false AND level = $4",
    )
    .bind(character_id)
    .bind(account_id)
    .bind(Json(&rolled))
    .bind(i32::try_from(key.level).unwrap_or(i32::MAX))
    .execute(pool)
    .await
    .map_err(db)?;
    if done.rows_affected() == 1 {
        key.modifiers = rolled;
        key.modifiers_rolled = true;
        return Ok(key);
    }
    // Someone else rolled (or the level moved) first: take what is stored.
    let row: Option<KeyColumns> = sqlx::query_as(&format!(
        "SELECT {KEY_COLUMNS} FROM game.dungeon_keys WHERE character_id = $1 AND account_id = $2"
    ))
    .bind(character_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    Ok(row.map_or(key, key_row))
}

/// The key of one of the account's characters. `None` if it has none or is not the account's.
/// A key whose modifiers were never rolled is rolled here, once, and stored.
pub async fn get_key(
    pool: &PgPool,
    account_id: Uuid,
    character_id: Uuid,
) -> Result<Option<KeyRow>, AppError> {
    let row: Option<KeyColumns> = sqlx::query_as(&format!(
        "SELECT {KEY_COLUMNS} FROM game.dungeon_keys WHERE character_id = $1 AND account_id = $2"
    ))
    .bind(character_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    match row {
        Some(row) => Ok(Some(
            ensure_rolled(pool, account_id, character_id, key_row(row)).await?,
        )),
        None => Ok(None),
    }
}

/// Every key the account's characters hold, by character id (unrolled ones are rolled).
pub async fn keys_of(pool: &PgPool, account_id: Uuid) -> Result<HashMap<Uuid, KeyRow>, AppError> {
    let rows: Vec<KeyColumns> = sqlx::query_as(&format!(
        "SELECT {KEY_COLUMNS} FROM game.dungeon_keys WHERE account_id = $1"
    ))
    .bind(account_id)
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let mut keys = HashMap::with_capacity(rows.len());
    for row in rows {
        let character_id = row.0;
        let key = ensure_rolled(pool, account_id, character_id, key_row(row)).await?;
        keys.insert(character_id, key);
    }
    Ok(keys)
}

/// The key change a downgrade makes: one level lower, only as many modifiers as that level
/// carries.
///
/// # Errors
///
/// A conflict when the key is already at the dungeon's lowest level. A key is never
/// downgraded below it and never deleted.
pub fn plan_downgrade(
    min_level: u32,
    level: u32,
    modifiers: &[KeyModifier],
) -> Result<(u32, Vec<KeyModifier>), AppError> {
    if level <= min_level.max(1) {
        return Err(AppError::Conflict(
            "Your key is already at its lowest level.".into(),
        ));
    }
    Ok(crate::catalog()
        .modifiers()
        .downgrade(min_level, level, modifiers))
}

/// Downgrades a character's key one level, dropping the modifiers the lower level has no slot
/// for. Free. `Ok(None)` if the character has no key of the account's.
///
/// # Errors
///
/// A conflict when the key is already at its dungeon's lowest level.
pub async fn downgrade_key(
    pool: &PgPool,
    account_id: Uuid,
    character_id: Uuid,
) -> Result<Option<KeyRow>, AppError> {
    let mut tx = pool.begin().await.map_err(db)?;
    let row: Option<KeyColumns> = sqlx::query_as(&format!(
        "SELECT {KEY_COLUMNS} FROM game.dungeon_keys
         WHERE character_id = $1 AND account_id = $2 FOR UPDATE"
    ))
    .bind(character_id)
    .bind(account_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut key = key_row(row);
    let min_level = crate::catalog()
        .dungeon(&key.dungeon_id)
        .map_or(1, |dungeon| dungeon.keys.min_level);
    // A key never rolled would have been rolled when the player looked at it: roll it first, so
    // the downgrade keeps the modifiers they saw.
    let current = if key.modifiers_rolled {
        key.modifiers.clone()
    } else {
        roll_modifiers(key.level)
    };
    let (level, kept) = plan_downgrade(min_level, key.level, &current)?;
    sqlx::query(
        "UPDATE game.dungeon_keys SET level = $3, modifiers = $4, modifiers_rolled = true,
            updated_at = now() WHERE character_id = $1 AND account_id = $2",
    )
    .bind(character_id)
    .bind(account_id)
    .bind(i32::try_from(level).unwrap_or(i32::MAX))
    .bind(Json(&kept))
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    tracing::info!(account_id = %account_id, character_id = %character_id, from = key.level, to = level, "key downgraded");
    key.level = level;
    key.modifiers = kept;
    key.modifiers_rolled = true;
    Ok(Some(key))
}

/// Strips every modifier from a character's key for gold: all or nothing, in one transaction
/// with the character row locked. `Ok(None)` if the character or its key is not the account's.
///
/// # Errors
///
/// A conflict when the key has no modifiers (nothing is charged) or the character holds too
/// little gold.
pub async fn cleanse_key(
    pool: &PgPool,
    account_id: Uuid,
    character_id: Uuid,
) -> Result<Option<KeyRow>, AppError> {
    let mut tx = pool.begin().await.map_err(db)?;
    // The character first, then its key: the order `apply_result` locks them in.
    let stored: Option<Json<Loadout>> = sqlx::query_scalar(
        "SELECT loadout FROM game.characters WHERE id = $1 AND account_id = $2 FOR UPDATE",
    )
    .bind(character_id)
    .bind(account_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    let Some(Json(mut loadout)) = stored else {
        return Ok(None);
    };
    let row: Option<KeyColumns> = sqlx::query_as(&format!(
        "SELECT {KEY_COLUMNS} FROM game.dungeon_keys
         WHERE character_id = $1 AND account_id = $2 FOR UPDATE"
    ))
    .bind(character_id)
    .bind(account_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut key = key_row(row);
    let current = if key.modifiers_rolled {
        key.modifiers.clone()
    } else {
        roll_modifiers(key.level)
    };
    let rules = crate::catalog().modifiers();
    let gold = loadout.resources.get(GOLD).copied().unwrap_or(0);
    rules
        .check_cleanse(&current, gold)
        .map_err(|e| AppError::Conflict(e.message))?;
    spend_resource(
        &mut loadout,
        GOLD,
        u64::from(rules.config().cleanse_gold_cost),
        "gold",
    )
    .map_err(|e| AppError::Conflict(e.to_string()))?;
    sqlx::query("UPDATE game.characters SET loadout = $2, updated_at = now() WHERE id = $1")
        .bind(character_id)
        .bind(Json(&loadout))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    sqlx::query(
        "UPDATE game.dungeon_keys SET modifiers = '[]'::jsonb, modifiers_rolled = true,
            updated_at = now() WHERE character_id = $1 AND account_id = $2",
    )
    .bind(character_id)
    .bind(account_id)
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    tracing::info!(account_id = %account_id, character_id = %character_id, cost = rules.config().cleanse_gold_cost, "key cleansed");
    key.modifiers = engine_realm::modifiers::cleanse(&current);
    key.modifiers_rolled = true;
    Ok(Some(key))
}

/// The resource a cleanse is paid in.
const GOLD: &str = "gold";

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
///
/// The caller has validated the set against the character's points (see
/// [`engine_realm::talents::validate`]).
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

/// A damage report as `game.dungeon_damage_reports` stores it, validated.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageReportRow {
    pub instance_id: String,
    pub class: String,
    pub party: serde_json::Value,
    pub total_damage: i64,
    pub dps: f32,
    pub duration_seconds: f32,
    pub boss_kill_seconds: Option<f32>,
    pub abilities: serde_json::Value,
}

/// The row to store for `result`'s damage report, or `None` when there is nothing worth storing
/// or it would not satisfy the table's checks.
///
/// Only a completed run is stored (a failed run's duration and damage say nothing about the
/// dungeon's pace), and a report with a non-finite or negative figure is dropped: the insert
/// shares a transaction with the experience, and a constraint violation there would lose the
/// player's reward over a statistic.
#[must_use]
pub fn damage_report_row(result: &DungeonResult) -> Option<DamageReportRow> {
    let report = result.damage_report.as_ref()?;
    if !result.success {
        return None;
    }
    let sound = |value: f32| value.is_finite() && value >= 0.0;
    if !sound(report.dps)
        || !sound(report.duration_seconds)
        || !report.boss_kill_seconds.is_none_or(sound)
    {
        tracing::warn!(result_id = %result.result_id, "damage report has impossible figures; not stored");
        return None;
    }
    let instance_id = result
        .result_id
        .split(':')
        .next()
        .unwrap_or_default()
        .to_owned();
    let class = report
        .party
        .iter()
        .find(|member| member.account_id == result.account_id)
        .map_or_else(|| "unknown".to_owned(), |member| member.class.clone());
    Some(DamageReportRow {
        instance_id,
        class,
        party: serde_json::to_value(&report.party).ok()?,
        total_damage: i64::try_from(report.total_damage).unwrap_or(i64::MAX),
        dps: report.dps,
        duration_seconds: report.duration_seconds,
        boss_kill_seconds: report.boss_kill_seconds,
        abilities: serde_json::to_value(&report.abilities).ok()?,
    })
}

/// The talent points a result grants: what the realm reported, and only for a success.
///
/// A failure carries none, whatever the row says - the same rule resources follow.
#[must_use]
pub fn granted_talent_points(result: &DungeonResult) -> u32 {
    if result.success {
        result.talent_points.min(10)
    } else {
        0
    }
}

/// Applies one realm result exactly once. Returns whether it was new.
///
/// Inside one transaction: record the result under its idempotency key; if that key was
/// already recorded, stop. Otherwise credit the completion and the experience — computed by
/// the realm, applied here with the shared progression curve — then the resources into the
/// loadout and, for the key's owner on a success, the key one level up (guarded by the level
/// the run was played at, so a key never advances twice for one run). Nothing in it came from
/// a client.
pub async fn apply_result(pool: &PgPool, result: &DungeonResult) -> Result<bool, AppError> {
    let account_id = Uuid::parse_str(&result.account_id)
        .map_err(|_| AppError::validation("result has a malformed account id"))?;
    let character_id = Uuid::parse_str(&result.character_id).ok();
    let mut tx = pool.begin().await.map_err(db)?;
    let recorded = sqlx::query(
        "INSERT INTO game.dungeon_results
            (result_id, account_id, character_id, dungeon_id, success, duration_us, experience,
             key_level, key_owner, key_level_after, resources, talent_points,
             currency_spent, participant_key_bump, own_key_level)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         ON CONFLICT (result_id) DO NOTHING",
    )
    .bind(&result.result_id)
    .bind(account_id)
    .bind(character_id)
    .bind(&result.dungeon_id)
    .bind(result.success)
    .bind(result.duration_us)
    .bind(i32::try_from(result.experience).unwrap_or(i32::MAX))
    .bind(i32::try_from(result.key_level).unwrap_or(i32::MAX))
    .bind(result.key_owner)
    .bind(i32::try_from(result.key_level_after).unwrap_or(i32::MAX))
    .bind(Json(&result.resources))
    .bind(i32::try_from(granted_talent_points(result)).unwrap_or(i32::MAX))
    .bind(Json(&result.currency_spent))
    .bind(result.participant_key_bump)
    .bind(i32::try_from(result.own_key_level).unwrap_or(i32::MAX))
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    if recorded.rows_affected() == 0 {
        tx.commit().await.map_err(db)?;
        return Ok(false);
    }
    if let Some(character_id) = character_id {
        let row: Option<(i32, i64, Json<Loadout>)> = sqlx::query_as(
            "SELECT level, experience, loadout FROM game.characters WHERE id = $1 AND account_id = $2 FOR UPDATE",
        )
        .bind(character_id)
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        if let Some((level, experience, Json(mut loadout))) = row {
            let gains: Vec<(String, u64)> = result
                .resources
                .iter()
                .map(|(id, amount)| (id.clone(), u64::from(*amount)))
                .collect();
            // What the run cost the character (key rerolls) comes off first, whether or not the
            // run succeeded, and never takes the balance below zero.
            debit_resources(&mut loadout, &result.debits());
            // A failed run pays nothing, whatever the row carries.
            if result.success {
                add_resources(&mut loadout, &gains);
            }
            let (level, experience) = engine_realm::progression::gain(
                u32::try_from(level).unwrap_or(1),
                u64::try_from(experience).unwrap_or(0),
                u64::from(result.experience),
            );
            // Granted with the result, in the same transaction that recorded its id, so a
            // re-delivered result cannot grant twice.
            sqlx::query(
                "UPDATE game.characters SET level = $2, experience = $3, loadout = $4,
                    talent_points_earned = LEAST(talent_points_earned + $5, 1000000),
                    updated_at = now() WHERE id = $1",
            )
            .bind(character_id)
            .bind(i32::try_from(level).unwrap_or(1))
            .bind(i64::try_from(experience).unwrap_or(0))
            .bind(Json(&loadout))
            .bind(i32::try_from(granted_talent_points(result)).unwrap_or(0))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            // The owner's key rises from the level the run used, a participant's from the level
            // their own key had; the key moves only if it still has that level, so it can
            // never rise twice for one run. A key whose level changes is rolled afresh.
            if let Some(update) = result.key_update() {
                let max_level = crate::catalog()
                    .dungeon(&result.dungeon_id)
                    .map_or(u32::MAX, |dungeon| dungeon.keys.max_level);
                let new_level = update.new_level.min(max_level);
                if new_level > update.expected_level {
                    sqlx::query(
                        "UPDATE game.dungeon_keys SET level = $3, modifiers = $4,
                            modifiers_rolled = true, updated_at = now()
                         WHERE character_id = $1 AND account_id = $2 AND level = $5
                            AND dungeon_id = $6",
                    )
                    .bind(character_id)
                    .bind(account_id)
                    .bind(i32::try_from(new_level).unwrap_or(i32::MAX))
                    .bind(Json(roll_modifiers(new_level)))
                    .bind(i32::try_from(update.expected_level).unwrap_or(i32::MAX))
                    .bind(&result.dungeon_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                }
            }
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
    // The player's damage report, in the same transaction: it exists exactly when its result
    // was new, so a re-delivery cannot write it twice (the table also keys it by result id).
    if let Some(report) = damage_report_row(result) {
        sqlx::query(
            "INSERT INTO game.dungeon_damage_reports
                (result_id, instance_id, dungeon_id, key_level, character_id, account_id, class,
                 party, total_damage, dps, duration_seconds, boss_kill_seconds, abilities)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
             ON CONFLICT (result_id) DO NOTHING",
        )
        .bind(&result.result_id)
        .bind(&report.instance_id)
        .bind(&result.dungeon_id)
        .bind(i32::try_from(result.key_level).unwrap_or(i32::MAX))
        .bind(character_id)
        .bind(account_id)
        .bind(&report.class)
        .bind(Json(&report.party))
        .bind(report.total_damage)
        .bind(report.dps)
        .bind(report.duration_seconds)
        .bind(report.boss_kill_seconds)
        .bind(Json(&report.abilities))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
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
    let kind = if delivery.kind == "bag" {
        "bag"
    } else {
        "mail"
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use engine_realm::meter::{DamageReport, ReportAbility, ReportMember};

    fn result(success: bool, report: Option<DamageReport>) -> DungeonResult {
        DungeonResult {
            result_id: "12:acct".into(),
            account_id: "acct".into(),
            character_id: String::new(),
            dungeon_id: "graveyard_crypt".into(),
            success,
            duration_us: 1,
            experience: 0,
            key_level: 3,
            key_owner: false,
            key_level_after: 0,
            resources: Vec::new(),
            damage_report: report,
            talent_points: 0,
            currency_spent: Vec::new(),
            participant_key_bump: false,
            own_key_level: 0,
        }
    }

    fn result_with_points(success: bool, talent_points: u32) -> DungeonResult {
        DungeonResult {
            talent_points,
            ..result(success, None)
        }
    }

    fn report() -> DamageReport {
        DamageReport {
            party: vec![
                ReportMember {
                    account_id: "other".into(),
                    character_name: "Bo".into(),
                    class: "Warrior".into(),
                    role: "tank".into(),
                },
                ReportMember {
                    account_id: "acct".into(),
                    character_name: "Ash".into(),
                    class: "Fire Mage".into(),
                    role: "dps".into(),
                },
            ],
            total_damage: 5000,
            dps: 25.0,
            duration_seconds: 200.0,
            boss_kill_seconds: Some(20.0),
            abilities: vec![ReportAbility {
                ability: "Firebolt".into(),
                damage: 5000,
                casts: 40,
            }],
        }
    }

    #[test]
    fn a_completed_runs_report_becomes_a_row() {
        let row = damage_report_row(&result(true, Some(report()))).expect("a row");
        assert_eq!(row.instance_id, "12");
        assert_eq!(
            row.class, "Fire Mage",
            "the player's own class, not the party's first"
        );
        assert_eq!(row.total_damage, 5000);
        assert_eq!(row.boss_kill_seconds, Some(20.0));
        assert_eq!(row.party.as_array().map(Vec::len), Some(2));
        assert_eq!(row.abilities[0]["ability"], "Firebolt");
    }

    #[test]
    fn no_report_or_a_failed_run_stores_nothing() {
        assert!(damage_report_row(&result(true, None)).is_none());
        assert!(damage_report_row(&result(false, Some(report()))).is_none());
    }

    #[test]
    fn impossible_figures_are_dropped_not_stored() {
        for bad in [f32::NAN, -1.0, f32::INFINITY] {
            let mut r = report();
            r.dps = bad;
            assert!(damage_report_row(&result(true, Some(r))).is_none());
            let mut r = report();
            r.boss_kill_seconds = Some(bad);
            assert!(damage_report_row(&result(true, Some(r))).is_none());
        }
    }

    #[test]
    fn only_a_success_grants_talent_points_and_never_an_absurd_number() {
        assert_eq!(granted_talent_points(&result_with_points(true, 2)), 2);
        assert_eq!(granted_talent_points(&result_with_points(false, 2)), 0);
        assert_eq!(
            granted_talent_points(&result_with_points(true, u32::MAX)),
            10
        );
    }

    #[test]
    fn a_downgrade_is_refused_at_the_minimum_and_otherwise_drops_the_extras() {
        let mods = vec![
            KeyModifier::base("rotating_beams"),
            KeyModifier::base("plague_explosion"),
        ];
        let (level, kept) = plan_downgrade(1, 5, &mods).expect("5 to 4");
        assert_eq!((level, kept.len()), (4, 1));
        let (level, kept) = plan_downgrade(1, 3, &mods[..1]).expect("3 to 2");
        assert_eq!((level, kept.len()), (2, 0));
        for (min, level) in [(1, 1), (3, 3), (4, 3), (1, 0)] {
            assert!(
                matches!(
                    plan_downgrade(min, level, &mods),
                    Err(AppError::Conflict(_))
                ),
                "min {min} level {level}"
            );
        }
    }

    #[test]
    fn rolled_modifiers_fit_the_level_and_differ_between_rolls() {
        assert!(roll_modifiers(1).is_empty());
        assert!(roll_modifiers(2).is_empty());
        assert_eq!(roll_modifiers(3).len(), 1);
        assert_eq!(roll_modifiers(5).len(), 2);
        let seen: std::collections::HashSet<String> =
            (0..40).map(|_| roll_modifiers(3)[0].id.clone()).collect();
        assert!(
            seen.len() > 1,
            "an OS seed gives both modifiers over 40 rolls"
        );
    }
}
