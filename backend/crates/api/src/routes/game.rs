//! The game's persistent-data endpoints and the queue entry point.
//!
//! | Route | What |
//! | --- | --- |
//! | `GET  /characters` | the account's characters (starters created on first call) |
//! | `GET  /characters/{id}` | one character in full |
//! | `POST /characters/{id}/equipment/move` | move an item, validated server-side |
//! | `POST /characters/{id}/mail/{mail_id}/take` | move a mail's item into the bag (409 if full) |
//! | `POST /characters/{id}/mail/{mail_id}/read` | mark mail read |
//! | `POST /characters/{id}/salvage` | break one bag item down into a resource |
//! | `POST /characters/{id}/exchange` | trade resources at a catalog rate |
//! | `GET  /economy` | the economy catalog: resources, salvage rule, exchanges |
//! | `PUT  /characters/{id}/talents` | replace talents, validated against the catalog |
//! | `GET  /player/settings` | the account's synced settings |
//! | `PUT  /player/settings` | save them, with optimistic versioning |
//! | `POST /realm/queue` | queue a character: ownership, role and gear checked here |
//!
//! Every route takes the account from the verified token and scopes every query by it. A
//! character id in a path is a request, never a proof of ownership.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
    Json, Router,
};
use engine_realm::api::{
    CharacterDetail, CharacterSummary, EconomyInfo, ExchangeInfo, ExchangeRequest, MoveItemRequest,
    QueueRequest, ResourceInfo, SalvageInfo, SalvageRequest, SettingsDocument, TalentsRequest,
};
use generic_auth_core::AppError;
use realm_game::bridge::{BridgeError, Snapshot};
use realm_game::{characters, store};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::middleware::auth::AuthUser;
use crate::rate_limit;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/characters", get(list_characters))
        .route("/characters/{id}", get(get_character))
        .route("/characters/{id}/equipment/move", post(move_item))
        .route("/characters/{id}/mail/{mail_id}/take", post(take_mail))
        .route("/characters/{id}/mail/{mail_id}/read", post(read_mail))
        .route("/characters/{id}/salvage", post(salvage))
        .route("/characters/{id}/exchange", post(exchange))
        .route("/economy", get(get_economy))
        .route("/characters/{id}/talents", put(save_talents))
        .route("/player/settings", get(get_settings).put(put_settings))
        .route("/realm/queue", post(queue))
}

fn character_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| ApiError(AppError::NotFound))
}

async fn owned(
    state: &AppState,
    user: &AuthUser,
    id: &str,
) -> Result<store::CharacterRow, ApiError> {
    let id = character_id(id)?;
    store::get(&state.db, user.user_id, id)
        .await
        .map_err(ApiError)?
        .ok_or_else(|| {
            tracing::warn!(account_id = %user.user_id, character_id = %id, "character access refused");
            ApiError(AppError::NotFound)
        })
}

/// One character in full: completions and key read fresh next to the row.
async fn detail_of(
    state: &AppState,
    user: &AuthUser,
    row: &store::CharacterRow,
) -> ApiResult<Json<CharacterDetail>> {
    let completions = store::completions(&state.db, row.id)
        .await
        .map_err(ApiError)?;
    let key = store::get_key(&state.db, user.user_id, row.id)
        .await
        .map_err(ApiError)?;
    Ok(Json(characters::detail(row, completions, key.as_ref())))
}

async fn list_characters(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Vec<CharacterSummary>>> {
    let rows = store::list_or_provision(&state.db, user.user_id)
        .await
        .map_err(ApiError)?;
    let keys = store::keys_of(&state.db, user.user_id)
        .await
        .map_err(ApiError)?;
    Ok(Json(
        rows.iter()
            .map(|row| characters::summary(row, keys.get(&row.id)))
            .collect(),
    ))
}

async fn get_character(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<CharacterDetail>> {
    let row = owned(&state, &user, &id).await?;
    detail_of(&state, &user, &row).await
}

async fn move_item(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<MoveItemRequest>,
) -> ApiResult<Json<CharacterDetail>> {
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let row = owned(&state, &user, &id).await?;
    let hero = realm_game::catalog()
        .character(&row.definition_id)
        .map(|definition| definition.hero)
        .unwrap_or_default();
    // Under the row lock, from the stored loadout: `row` above may predate a chest drop.
    store::update_loadout(&state.db, user.user_id, row.id, |stored| {
        engine_realm::loadout::move_item(hero, stored, &body.from, &body.to)
            .map_err(|e| AppError::BadRequest(e.message))
    })
    .await
    .map_err(ApiError)?
    .ok_or(ApiError(AppError::NotFound))?;
    tracing::info!(account_id = %user.user_id, character_id = %row.id, "equipment changed");
    let row = owned(&state, &user, &id).await?;
    detail_of(&state, &user, &row).await
}

/// The mail, salvage and exchange routes: a locked read-modify-write of the loadout, then the character in full.
async fn mail_change(
    state: &AppState,
    user: &AuthUser,
    id: &str,
    change: impl FnOnce(&mut engine_realm::loadout::Loadout) -> Result<(), AppError>,
) -> ApiResult<Json<CharacterDetail>> {
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let character = character_id(id)?;
    store::update_loadout(&state.db, user.user_id, character, |stored| {
        let mut loadout = stored.clone();
        change(&mut loadout)?;
        Ok(loadout)
    })
    .await
    .map_err(ApiError)?
    .ok_or(ApiError(AppError::NotFound))?;
    let row = owned(state, user, id).await?;
    detail_of(state, user, &row).await
}

async fn take_mail(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, mail_id)): Path<(String, u64)>,
) -> ApiResult<Json<CharacterDetail>> {
    use engine_realm::loadout::{take_mail, TakeMailError};
    let account = user.user_id;
    let detail = mail_change(&state, &user, &id, |loadout| {
        take_mail(loadout, mail_id)
            .map(|_| ())
            .map_err(|error| match error {
                TakeMailError::BagFull => AppError::Conflict("Your bag is full.".into()),
                TakeMailError::NotFound => AppError::NotFound,
            })
    })
    .await?;
    tracing::info!(account_id = %account, character_id = %id, mail_id, "mail taken");
    Ok(detail)
}

async fn read_mail(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, mail_id)): Path<(String, u64)>,
) -> ApiResult<Json<CharacterDetail>> {
    mail_change(&state, &user, &id, |loadout| {
        if engine_realm::loadout::mark_mail_read(loadout, mail_id) {
            Ok(())
        } else {
            Err(AppError::NotFound)
        }
    })
    .await
}

async fn salvage(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<SalvageRequest>,
) -> ApiResult<Json<CharacterDetail>> {
    let economy = realm_game::catalog().economy();
    let account = user.user_id;
    let detail = mail_change(&state, &user, &id, |loadout| {
        engine_realm::economy::salvage(loadout, body.index, economy)
            .map(|_| ())
            .map_err(economy_error)
    })
    .await?;
    tracing::info!(account_id = %account, character_id = %id, index = body.index, "item salvaged");
    Ok(detail)
}

async fn exchange(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ExchangeRequest>,
) -> ApiResult<Json<CharacterDetail>> {
    let economy = realm_game::catalog().economy();
    let account = user.user_id;
    let detail = mail_change(&state, &user, &id, |loadout| {
        engine_realm::economy::exchange(loadout, &body.exchange_id, body.times, economy)
            .map_err(economy_error)
    })
    .await?;
    tracing::info!(
        account_id = %account,
        character_id = %id,
        exchange_id = %body.exchange_id,
        times = body.times,
        "resources exchanged"
    );
    Ok(detail)
}

/// A refused salvage or exchange as the HTTP error: the request was wrong (400) or the state
/// it needs is not there (409); the message is the player-facing sentence.
fn economy_error(error: engine_realm::economy::EconomyError) -> AppError {
    use engine_realm::economy::EconomyError;
    let message = error.to_string();
    match error {
        EconomyError::EmptyCell
        | EconomyError::UnknownItem
        | EconomyError::UnknownExchange
        | EconomyError::InvalidTimes => AppError::BadRequest(message),
        EconomyError::Insufficient(_) | EconomyError::Overflow => AppError::Conflict(message),
    }
}

async fn get_economy(_user: AuthUser) -> Json<EconomyInfo> {
    let economy = realm_game::catalog().economy();
    Json(EconomyInfo {
        resources: economy
            .resources
            .iter()
            .map(|resource| ResourceInfo {
                id: resource.id.clone(),
                name: resource.name.clone(),
                description: resource.description.clone(),
            })
            .collect(),
        salvage: SalvageInfo {
            resource: economy.salvage.resource.clone(),
            base: economy.salvage.base,
            per_item_level: economy.salvage.per_item_level,
        },
        exchanges: economy
            .exchanges
            .iter()
            .map(|exchange| ExchangeInfo {
                id: exchange.id.clone(),
                from: exchange.from.clone(),
                from_amount: exchange.from_amount,
                to: exchange.to.clone(),
                to_amount: exchange.to_amount,
            })
            .collect(),
    })
}

async fn save_talents(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<TalentsRequest>,
) -> ApiResult<StatusCode> {
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let row = owned(&state, &user, &id).await?;
    let hero = realm_game::catalog()
        .character(&row.definition_id)
        .map(|definition| definition.hero)
        .unwrap_or_default();
    let mut talents: Vec<String> = Vec::new();
    for key in body.talents {
        if !engine_realm::talents::is_valid(hero, &key) {
            return Err(ApiError(AppError::BadRequest(format!(
                "{key} is not a talent this character has."
            ))));
        }
        if !talents.contains(&key) {
            talents.push(key);
        }
    }
    store::save_talents(&state.db, user.user_id, row.id, &talents)
        .await
        .map_err(ApiError)?;
    tracing::info!(account_id = %user.user_id, character_id = %row.id, count = talents.len(), "talents changed");
    Ok(StatusCode::NO_CONTENT)
}

async fn get_settings(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<SettingsDocument>> {
    let stored = store::settings(&state.db, user.user_id)
        .await
        .map_err(ApiError)?;
    Ok(Json(match stored {
        Some((version, settings)) => SettingsDocument { version, settings },
        None => SettingsDocument {
            version: 0,
            settings: serde_json::Value::Null,
        },
    }))
}

/// The largest settings document accepted.
const MAX_SETTINGS_BYTES: usize = 64 * 1024;

async fn put_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<SettingsDocument>,
) -> ApiResult<Json<SettingsDocument>> {
    state
        .limits
        .check(
            &format!("settings:{}", user.user_id),
            rate_limit::SETTINGS_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    if !body.settings.is_object() {
        return Err(ApiError(AppError::BadRequest(
            "Settings must be an object.".into(),
        )));
    }
    if serde_json::to_vec(&body.settings).map_or(usize::MAX, |bytes| bytes.len())
        > MAX_SETTINGS_BYTES
    {
        return Err(ApiError(AppError::BadRequest(
            "Settings are too large.".into(),
        )));
    }
    let version = store::save_settings(&state.db, user.user_id, body.version, &body.settings)
        .await
        .map_err(ApiError)?;
    Ok(Json(SettingsDocument {
        version,
        settings: body.settings,
    }))
}

async fn queue(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<QueueRequest>,
) -> ApiResult<StatusCode> {
    let _ = headers;
    state
        .limits
        .check(
            &format!("queue:{}", user.user_id),
            rate_limit::QUEUE_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let row = owned(&state, &user, &body.character_id).await?;
    let catalog = realm_game::catalog();
    let definition = catalog
        .character(&row.definition_id)
        .ok_or_else(|| ApiError(AppError::BadRequest("That character cannot queue.".into())))?;
    if !definition.can_play(body.role) {
        return Err(ApiError(AppError::BadRequest(format!(
            "{} cannot queue as {}.",
            definition.name,
            body.role.name()
        ))));
    }
    let dungeon = catalog
        .dungeon(&body.dungeon_id)
        .ok_or_else(|| ApiError(AppError::BadRequest("That dungeon does not exist.".into())))?;
    let key = store::get_key(&state.db, user.user_id, row.id)
        .await
        .map_err(ApiError)?;
    let summary = characters::summary(&row, key.as_ref());
    dungeon
        .admits(summary.level, summary.gear_score)
        .map_err(|e| ApiError(AppError::BadRequest(e.message)))?;

    let token = state
        .jwt
        .issue_service(&state.settings.realm.service_subject)
        .map_err(ApiError)?;
    let snapshot = Snapshot {
        account_id: user.user_id.to_string(),
        character_id: row.id.to_string(),
        character_name: row.name.clone(),
        definition_id: row.definition_id.clone(),
        level: summary.level,
        gear_score: summary.gear_score,
        loadout: serde_json::to_string(&row.loadout.0).unwrap_or_default(),
        key_dungeon_id: key
            .as_ref()
            .map(|key| key.dungeon_id.clone())
            .unwrap_or_default(),
        key_level: key.as_ref().map_or(0, |key| key.level),
    };
    match state
        .realm
        .enqueue(&token, &snapshot, &dungeon.id, body.role)
        .await
    {
        Ok(()) => {
            tracing::info!(
                account_id = %user.user_id,
                character_id = %row.id,
                dungeon_id = %dungeon.id,
                role = body.role.key(),
                gear_score = summary.gear_score,
                "queue join relayed"
            );
            Ok(StatusCode::NO_CONTENT)
        }
        Err(BridgeError::Refused(message)) => {
            let error = engine_realm::RealmError::from_wire(&message);
            Err(ApiError(AppError::Conflict(error.message)))
        }
        Err(BridgeError::Unavailable(reason)) => {
            tracing::error!(reason = %reason, "realm unavailable for queue join");
            Err(ApiError(AppError::internal(
                "The game server is unavailable.",
            )))
        }
    }
}
