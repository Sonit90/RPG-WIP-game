//! The `game` schema: queries only, no HTTP. Every character query is scoped by account, so
//! a caller can never read or write a character that is not theirs — the WHERE clause, not a
//! check after the fact, is the ownership rule.

use engine_realm::api::{CharacterDetail, DungeonCompletion, DungeonResult, LootDelivery};
use engine_realm::economy::{add_resources, debit_resources, spend_resource};
use engine_realm::sharpen::{sharpen, SharpenError, SharpenOutcome};
use engine_realm::loadout::{
    add_stack_to_bag, add_stack_to_mail, migrate_removed_slots, starting_loadout, BagError,
    Loadout, SlotRef, MAIL_BAG_FULL, MAIL_DUNGEON_DROP, MAIL_UNCLAIMED,
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
    let HubStart::Locked(mut hub) =
        HubTx::begin(pool, account_id, character_id, None, "key_downgrade").await?
    else {
        return Ok(None);
    };
    let key = hub.downgrade_key().await?;
    hub.release().await?;
    Ok(key)
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
    let HubStart::Locked(mut hub) =
        HubTx::begin(pool, account_id, character_id, None, "key_cleanse").await?
    else {
        return Ok(None);
    };
    let key = hub.cleanse_key().await?;
    hub.release().await?;
    Ok(key)
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

/// An open transaction.
pub type Tx = sqlx::Transaction<'static, sqlx::Postgres>;

/// What [`HubTx::begin`] found.
pub enum HubStart {
    /// The character does not exist or is not the account's.
    Missing,
    /// This request id was already served: the response it got, to be returned unchanged. Nothing
    /// was changed and nothing is locked any more.
    Replay(serde_json::Value),
    /// The character row is locked; the write may go ahead.
    Locked(Box<HubTx>),
}

/// One hub write: a transaction holding the character row's `FOR UPDATE` lock.
///
/// The order is fixed, and every hub route follows it: [`HubTx::begin`] takes the lock (and
/// answers a replayed request id from what the first attempt stored), the route then asks the
/// realm whether the character is engaged - with the lock held, so a queue join cannot slip in
/// between the question and the write - then mutates, builds its response with
/// [`HubTx::detail`], and [`HubTx::commit`]s, which records the response under the request id in
/// the same transaction. Dropping a `HubTx` rolls everything back.
pub struct HubTx {
    tx: Tx,
    account_id: Uuid,
    /// The character as stored under the lock (a loadout saved with removed slots is rehomed),
    /// kept current by the write methods. Validate against this, not a read from before the lock.
    pub row: CharacterRow,
    request: Option<(Uuid, &'static str)>,
}

impl HubTx {
    /// Locks the character's row for the account, or answers a replayed `request_id`.
    ///
    /// `kind` names the action; a request id first seen for another account, character or kind
    /// is a conflict, and the response stored under it is never shown to the other caller.
    ///
    /// # Errors
    ///
    /// A conflict for a request id that belongs to something else, or a database failure.
    pub async fn begin(
        pool: &PgPool,
        account_id: Uuid,
        character_id: Uuid,
        request_id: Option<Uuid>,
        kind: &'static str,
    ) -> Result<HubStart, AppError> {
        let mut tx = pool.begin().await.map_err(db)?;
        let row = sqlx::query_as::<_, CharacterRow>(&format!(
            "SELECT {COLUMNS} FROM game.characters WHERE id = $1 AND account_id = $2 FOR UPDATE"
        ))
        .bind(character_id)
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        let Some(mut row) = row else {
            return Ok(HubStart::Missing);
        };
        if let Some(request_id) = request_id {
            let seen: Option<(Uuid, Uuid, String, Json<serde_json::Value>)> = sqlx::query_as(
                "SELECT account_id, character_id, kind, response FROM game.hub_requests
                 WHERE request_id = $1",
            )
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if let Some((owner, character, seen_kind, Json(response))) = seen {
                if owner != account_id || character != character_id || seen_kind != kind {
                    tracing::warn!(account_id = %account_id, request_id = %request_id, kind, "request id reused for another action");
                    return Err(AppError::Conflict(
                        "That request id was already used for another action.".into(),
                    ));
                }
                tracing::info!(account_id = %account_id, character_id = %character_id, request_id = %request_id, kind, "request replayed");
                return Ok(HubStart::Replay(response));
            }
        }
        // A character saved while there were four trinket slots: rehome those items first.
        migrate_removed_slots(&mut row.loadout.0, unix_now());
        Ok(HubStart::Locked(Box::new(Self {
            tx,
            account_id,
            row,
            request: request_id.map(|id| (id, kind)),
        })))
    }

    /// Stores `loadout` as the character's.
    pub async fn save_loadout(&mut self, loadout: &Loadout) -> Result<(), AppError> {
        sqlx::query("UPDATE game.characters SET loadout = $2, updated_at = now() WHERE id = $1")
            .bind(self.row.id)
            .bind(Json(loadout))
            .execute(&mut *self.tx)
            .await
            .map_err(db)?;
        self.row.loadout = Json(loadout.clone());
        Ok(())
    }

    /// Stores `talents` as the character's.
    pub async fn save_talents(&mut self, talents: &[String]) -> Result<(), AppError> {
        sqlx::query("UPDATE game.characters SET talents = $2, updated_at = now() WHERE id = $1")
            .bind(self.row.id)
            .bind(Json(talents))
            .execute(&mut *self.tx)
            .await
            .map_err(db)?;
        self.row.talents = Json(talents.to_vec());
        Ok(())
    }

    /// The character's key as this transaction sees it (locked; one never rolled is rolled and
    /// stored now). `None` if it has none.
    pub async fn key(&mut self) -> Result<Option<KeyRow>, AppError> {
        let row: Option<KeyColumns> = sqlx::query_as(&format!(
            "SELECT {KEY_COLUMNS} FROM game.dungeon_keys
             WHERE character_id = $1 AND account_id = $2 FOR UPDATE"
        ))
        .bind(self.row.id)
        .bind(self.account_id)
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(db)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut key = key_row(row);
        if !key.modifiers_rolled {
            key.modifiers = roll_modifiers(key.level);
            key.modifiers_rolled = true;
            self.store_key(&key).await?;
        }
        Ok(Some(key))
    }

    async fn store_key(&mut self, key: &KeyRow) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE game.dungeon_keys SET level = $3, modifiers = $4, modifiers_rolled = true,
                updated_at = now() WHERE character_id = $1 AND account_id = $2",
        )
        .bind(self.row.id)
        .bind(self.account_id)
        .bind(i32::try_from(key.level).unwrap_or(i32::MAX))
        .bind(Json(&key.modifiers))
        .execute(&mut *self.tx)
        .await
        .map_err(db)?;
        Ok(())
    }

    /// The character in full as it stands inside this transaction: what a route answers with,
    /// and what a replay of it will answer with.
    pub async fn detail(&mut self) -> Result<CharacterDetail, AppError> {
        let completions = completions(&mut *self.tx, self.row.id).await?;
        let key = self.key().await?;
        Ok(crate::characters::detail(
            &self.row,
            completions,
            key.as_ref(),
        ))
    }

    /// Lowers the key one level, dropping the modifiers the lower level has no slot for. Free.
    /// `Ok(None)` if the character has no key.
    ///
    /// # Errors
    ///
    /// A conflict when the key is already at its dungeon's lowest level.
    pub async fn downgrade_key(&mut self) -> Result<Option<KeyRow>, AppError> {
        // A key never rolled is rolled by `key()` first, so the downgrade keeps the modifiers
        // the player saw.
        let Some(mut key) = self.key().await? else {
            return Ok(None);
        };
        let min_level = crate::catalog()
            .dungeon(&key.dungeon_id)
            .map_or(1, |dungeon| dungeon.keys.min_level);
        let (level, kept) = plan_downgrade(min_level, key.level, &key.modifiers)?;
        let from = key.level;
        key.level = level;
        key.modifiers = kept;
        self.store_key(&key).await?;
        tracing::info!(account_id = %self.account_id, character_id = %self.row.id, from, to = level, "key downgraded");
        Ok(Some(key))
    }

    /// Strips every modifier from the key for gold: all or nothing. `Ok(None)` if the character
    /// has no key.
    ///
    /// # Errors
    ///
    /// A conflict when the key has no modifiers (nothing is charged) or the character holds too
    /// little gold.
    pub async fn cleanse_key(&mut self) -> Result<Option<KeyRow>, AppError> {
        let Some(mut key) = self.key().await? else {
            return Ok(None);
        };
        let mut loadout = self.row.loadout.0.clone();
        let rules = crate::catalog().modifiers();
        let gold = loadout.resources.get(GOLD).copied().unwrap_or(0);
        rules
            .check_cleanse(&key.modifiers, gold)
            .map_err(|e| AppError::Conflict(e.message))?;
        spend_resource(
            &mut loadout,
            GOLD,
            u64::from(rules.config().cleanse_gold_cost),
            "gold",
        )
        .map_err(|e| AppError::Conflict(e.to_string()))?;
        self.save_loadout(&loadout).await?;
        key.modifiers = engine_realm::modifiers::cleanse(&key.modifiers);
        self.store_key(&key).await?;
        tracing::info!(account_id = %self.account_id, character_id = %self.row.id, cost = rules.config().cleanse_gold_cost, "key cleansed");
        Ok(Some(key))
    }

    /// Commits, recording `response` under the request id (if the request had one) in the same
    /// transaction.
    ///
    /// # Errors
    ///
    /// A conflict if another request recorded the same id first, or a database failure; nothing
    /// is changed then.
    pub async fn commit<R: serde::Serialize>(mut self, response: &R) -> Result<(), AppError> {
        if let Some((request_id, kind)) = self.request {
            let response = serde_json::to_value(response)
                .map_err(|_| AppError::internal("The response could not be recorded."))?;
            let recorded = sqlx::query(
                "INSERT INTO game.hub_requests (request_id, account_id, character_id, kind, response)
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT (request_id) DO NOTHING",
            )
            .bind(request_id)
            .bind(self.account_id)
            .bind(self.row.id)
            .bind(kind)
            .bind(Json(&response))
            .execute(&mut *self.tx)
            .await
            .map_err(db)?;
            if recorded.rows_affected() == 0 {
                return Err(AppError::Conflict(
                    "That request id was already used for another action.".into(),
                ));
            }
        }
        self.tx.commit().await.map_err(db)
    }

    /// Commits a write that has no response to remember (the queue join holds the row only to
    /// serialise with the hub writes).
    pub async fn release(self) -> Result<(), AppError> {
        self.tx.commit().await.map_err(db)
    }
}

/// A refused sharpening as the HTTP error: the request named nothing sharpenable (400), or
/// the item or purse is not in a state that allows it (409). The message is the sentence the
/// player reads.
#[must_use]
pub fn sharpen_error(error: SharpenError) -> AppError {
    let message = error.to_string();
    match error {
        SharpenError::EmptySlot | SharpenError::NotEquipment => AppError::BadRequest(message),
        SharpenError::CannotSharpenBroken
        | SharpenError::MaxLevel
        | SharpenError::Insufficient(_)
        | SharpenError::Overflow => AppError::Conflict(message),
        // The server drew a roll outside `[0, 1)`: its own bug, not the player's request.
        SharpenError::InvalidRoll => AppError::internal("The sharpening could not be rolled."),
    }
}

/// One sharpening attempt on `loadout`, with the server's `roll` in `[0, 1)`.
///
/// # Errors
///
/// The refusals of [`engine_realm::sharpen::sharpen`] as [`sharpen_error`] maps them; the
/// loadout is unchanged then.
pub fn sharpen_loadout(
    loadout: &Loadout,
    slot: &SlotRef,
    roll: f64,
) -> Result<(Loadout, SharpenOutcome), AppError> {
    let mut changed = loadout.clone();
    let outcome = sharpen(&mut changed, slot, roll, crate::catalog().economy())
        .map_err(sharpen_error)?;
    Ok((changed, outcome))
}

/// Sharpens the item in `slot` of one of the account's characters: the price, the roll's
/// consequence and the new item state are written in one locked read-modify-write of the
/// character row, so two requests can neither both pay from the same purse nor both sharpen
/// the same item. `roll` comes from the server's own random source, never from the request.
/// `Ok(None)` if the character is not the account's.
pub async fn sharpen_character(
    pool: &PgPool,
    account_id: Uuid,
    id: Uuid,
    slot: &SlotRef,
    roll: f64,
) -> Result<Option<SharpenOutcome>, AppError> {
    let HubStart::Locked(mut hub) = HubTx::begin(pool, account_id, id, None, "sharpen").await?
    else {
        return Ok(None);
    };
    let (changed, outcome) = sharpen_loadout(&hub.row.loadout.0, slot, roll)?;
    hub.save_loadout(&changed).await?;
    hub.release().await?;
    Ok(Some(outcome))
}

/// Replaces a character's talents under the character's row lock. `false` if it is not the
/// account's.
///
/// The caller has validated the set against the character's points (see
/// [`engine_realm::talents::validate`]); a route that must validate against the stored row
/// does it on [`HubTx::row`] instead.
pub async fn save_talents(
    pool: &PgPool,
    account_id: Uuid,
    id: Uuid,
    talents: &[String],
) -> Result<bool, AppError> {
    let HubStart::Locked(mut hub) = HubTx::begin(pool, account_id, id, None, "talents").await?
    else {
        return Ok(false);
    };
    hub.save_talents(talents).await?;
    hub.release().await?;
    Ok(true)
}

/// A season: the period leaderboards and history are cut by.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Season {
    pub id: String,
    pub name: String,
    pub starts_at: time::OffsetDateTime,
    /// `None` for a season that never ends (`season-0`, "all time").
    pub ends_at: Option<time::OffsetDateTime>,
}

/// The active season: the latest one that has started and not ended. `None` only if the table
/// was emptied (the migration seeds `season-0`).
pub async fn active_season<'e>(
    executor: impl sqlx::PgExecutor<'e>,
) -> Result<Option<Season>, AppError> {
    sqlx::query_as::<_, Season>(
        "SELECT id, name, starts_at, ends_at FROM game.seasons
         WHERE starts_at <= now() AND (ends_at IS NULL OR ends_at > now())
         ORDER BY starts_at DESC, id LIMIT 1",
    )
    .fetch_optional(executor)
    .await
    .map_err(db)
}

/// How often a character finished each dungeon.
pub async fn completions<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    character_id: Uuid,
) -> Result<Vec<DungeonCompletion>, AppError> {
    let rows: Vec<(String, i32)> = sqlx::query_as(
        "SELECT dungeon_id, count FROM game.dungeon_completions WHERE character_id = $1 ORDER BY dungeon_id",
    )
    .bind(character_id)
    .fetch_all(executor)
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

/// The most of one looted resource a single result may carry. The realm rolled the numbers;
/// this only bounds a row nobody expected.
const MAX_LOOTED: u32 = 1_000_000;

/// What a result pays from monster drops: the realm's tally, restricted to resources the
/// economy defines and capped, whatever the outcome of the run.
#[must_use]
pub fn granted_looted(result: &DungeonResult) -> Vec<(String, u64)> {
    let economy = crate::catalog().economy();
    result
        .looted
        .iter()
        .filter(|(id, amount)| *amount > 0 && economy.resource(id).is_some())
        .map(|(id, amount)| (id.clone(), u64::from((*amount).min(MAX_LOOTED))))
        .collect()
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
    // The grants need a character. Without one (never existed, deleted, or someone else's) the
    // result must not be recorded or acknowledged: the transaction is dropped, so the realm keeps
    // the result and delivers it again, and support can see why it is stuck.
    let row: Option<(i32, i64, Json<Loadout>)> = match character_id {
        Some(character_id) => sqlx::query_as(
            "SELECT level, experience, loadout FROM game.characters WHERE id = $1 AND account_id = $2 FOR UPDATE",
        )
        .bind(character_id)
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?,
        None => None,
    };
    let (Some(character_id), Some((level, experience, Json(mut loadout)))) = (character_id, row)
    else {
        tracing::error!(
            result_id = %result.result_id,
            account_id = %result.account_id,
            character_id = %result.character_id,
            "dungeon result names no character of the account; not recorded, not acknowledged"
        );
        return Err(AppError::NotFound);
    };
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
    // What was picked up from monsters was looted whether or not the boss fell.
    add_resources(&mut loadout, &granted_looted(result));
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
        looted = ?result.looted,
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
            // The item arrives with the quality it was rolled with (clamped, so a forged row
            // cannot be stronger than the rules allow).
            let props = delivery.props();
            let outcome = if kind == "bag" {
                match add_stack_to_bag(&mut loadout, &delivery.item, props) {
                    Ok(_) => Ok("bag"),
                    Err(BagError::Full) => {
                        add_stack_to_mail(&mut loadout, &delivery.item, props, MAIL_BAG_FULL, now)
                            .map(|_| "mail_bag_full")
                    }
                    Err(error) => Err(error),
                }
            } else {
                let reason = match delivery.reason.as_str() {
                    MAIL_BAG_FULL => MAIL_BAG_FULL,
                    MAIL_DUNGEON_DROP => MAIL_DUNGEON_DROP,
                    _ => MAIL_UNCLAIMED,
                };
                add_stack_to_mail(&mut loadout, &delivery.item, props, reason, now)
                    .map(|_| "mail")
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
            looted: Vec::new(),
            currency_spent: Vec::new(),
            participant_key_bump: false,
            own_key_level: 0,
            run_uid: String::new(),
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

    #[test]
    fn monster_loot_is_paid_on_a_failed_run_too_but_only_known_resources_and_bounded() {
        let mut failed = result(false, None);
        failed.looted = vec![
            ("gold".into(), 40),
            ("aether_shard".into(), 2),
            ("moon_cheese".into(), 9),
            ("gold".into(), 0),
            ("crypt_dust".into(), u32::MAX),
        ];
        let paid = granted_looted(&failed);
        assert_eq!(
            paid,
            vec![
                ("gold".to_owned(), 40),
                ("aether_shard".to_owned(), 2),
                ("crypt_dust".to_owned(), u64::from(MAX_LOOTED)),
            ]
        );
        let mut loadout = Loadout::default();
        add_resources(&mut loadout, &paid);
        assert_eq!(loadout.resources.get("gold"), Some(&40));
    }

    mod sharpening {
        use super::*;
        use engine_game::character::ItemProps;

        const RING: &str = "ashgraves_seal";

        fn slot() -> SlotRef {
            SlotRef::Equipment {
                slot: "ring1".into(),
            }
        }

        fn loadout(props: ItemProps, dust: u64, ember: u64) -> Loadout {
            engine_realm::loot::ensure_items_registered();
            let mut loadout = Loadout::default();
            let mut entry = engine_realm::loadout::EquippedItem::new("ring1", RING);
            (entry.quality, entry.sharpening, entry.broken) =
                (props.quality, props.sharpening, props.broken);
            loadout.equipment.push(entry);
            add_resources(
                &mut loadout,
                &[("crypt_dust".into(), dust), ("steward_ember".into(), ember)],
            );
            loadout
        }

        fn props_of(loadout: &Loadout) -> ItemProps {
            loadout.equipment[0].props()
        }

        #[test]
        fn a_safe_level_succeeds_and_pays_the_price_whatever_the_roll() {
            let (after, outcome) =
                sharpen_loadout(&loadout(ItemProps::rolled(100), 50, 0), &slot(), 0.0)
                    .expect("allowed");
            assert_eq!(outcome, SharpenOutcome::Success { new_level: 1 });
            assert_eq!(props_of(&after).sharpening, 1);
            assert_eq!(after.resources.get("crypt_dust"), Some(&40), "10 dust paid");
        }

        #[test]
        fn a_risky_level_breaks_on_a_low_roll_and_holds_on_a_high_one() {
            let at_four = ItemProps::new(100, 4, false);
            let rich = loadout(at_four, 1_000, 5);
            let (broken, outcome) = sharpen_loadout(&rich, &slot(), 0.1).expect("allowed");
            assert_eq!(outcome, SharpenOutcome::Broken);
            assert!(props_of(&broken).broken);
            assert_eq!(props_of(&broken).sharpening, 0);
            let (held, outcome) = sharpen_loadout(&rich, &slot(), 0.99).expect("allowed");
            assert_eq!(outcome, SharpenOutcome::Success { new_level: 5 });
            assert_eq!(props_of(&held).sharpening, 5);
        }

        #[test]
        fn refusals_are_conflicts_or_bad_requests_and_change_nothing() {
            let conflict = |error: AppError| matches!(error, AppError::Conflict(_));
            // Broken.
            let broken = loadout(ItemProps::new(100, 0, true), 1_000, 5);
            assert!(conflict(sharpen_loadout(&broken, &slot(), 0.5).expect_err("broken")));
            // At the maximum (10).
            let maxed = loadout(ItemProps::new(100, 10, false), 10_000, 50);
            assert!(conflict(sharpen_loadout(&maxed, &slot(), 0.5).expect_err("max")));
            // Too poor.
            let poor = loadout(ItemProps::rolled(100), 5, 0);
            assert!(conflict(sharpen_loadout(&poor, &slot(), 0.5).expect_err("poor")));
            // Nothing there.
            let empty = SlotRef::Equipment {
                slot: "head".into(),
            };
            let rich = loadout(ItemProps::rolled(100), 1_000, 5);
            assert!(matches!(
                sharpen_loadout(&rich, &empty, 0.5),
                Err(AppError::BadRequest(_))
            ));
            // A roll outside [0, 1) is the server's bug, not a conflict.
            assert!(!conflict(sharpen_loadout(&rich, &slot(), 1.0).expect_err("roll")));
        }
    }
}
