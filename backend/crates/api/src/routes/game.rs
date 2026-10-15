//! The game's persistent-data endpoints and the queue entry point.
//!
//! | Route | What |
//! | --- | --- |
//! | `GET  /characters` | the account's characters (starters created on first call) |
//! | `GET  /characters/{id}` | one character in full |
//! | `POST /characters/{id}/equipment/move` | move an item, validated server-side |
//! | `POST /characters/{id}/mail/{mail_id}/take` | move a mail's item into the bag (409 if full) |
//! | `POST /characters/{id}/mail/{mail_id}/read` | mark mail read |
//! | `POST /characters/{id}/salvage` | break bag items down into a resource |
//! | `POST /characters/{id}/exchange` | trade resources at a catalog rate |
//! | `POST /characters/{id}/enhance` | one enhancement attempt: the server rolls, the locked row pays |
//! | `GET  /economy` | the economy catalog: resources, salvage rule, exchanges |
//! | `POST /characters/{id}/key/downgrade` | key one level lower, extra modifiers dropped (hub only) |
//! | `POST /characters/{id}/key/cleanse` | strip a key's modifiers for gold (hub only) |
//! | `PUT  /characters/{id}/talents` | replace talents: keys, points and the engagement lock checked |
//! | `GET  /player/settings` | the account's synced settings |
//! | `PUT  /player/settings` | save them, with optimistic versioning |
//! | `POST /realm/queue` | queue a character: ownership, role and gear checked here |
//!
//! Talents, equipment (move, salvage, taking mail) and the key (downgrade, cleanse) are **locked
//! while the character is engaged** - queued, in a party, in an instance - because the realm holds a snapshot of
//! them for the run. The API asks the realm (`svc_engagement`) before each such change and
//! answers 409 when it is engaged; if the realm cannot be asked, the change is refused too.
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
    CharacterDetail, CharacterSummary, EconomyInfo, EnhanceOutcomeName, EnhanceRequest,
    EnhanceResponse, ExchangeInfo, ExchangeRequest, MoveItemRequest, QueueRequest, ResourceInfo,
    SalvageInfo, SalvageRequest, SettingsDocument, TalentsRequest,
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
        .route("/characters/{id}/enhance", post(enhance))
        .route("/economy", get(get_economy))
        .route("/characters/{id}/key/downgrade", post(downgrade_key))
        .route("/characters/{id}/key/cleanse", post(cleanse_key))
        .route("/characters/{id}/talents", put(save_talents))
        .route("/player/settings", get(get_settings).put(put_settings))
        .route("/realm/queue", post(queue))
}

/// Refuses with 409 while the realm has the account queued, in a party or in an instance.
async fn ensure_unlocked(state: &AppState, user: &AuthUser) -> Result<(), ApiError> {
    let token = state
        .jwt
        .issue_service(&state.settings.realm.service_subject)
        .map_err(ApiError)?;
    let answer = state
        .realm
        .engagement(&token, &user.user_id.to_string())
        .await;
    realm_game::bridge::lock_decision(answer).map_err(ApiError)
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
    ensure_unlocked(&state, &user).await?;
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
    locked_while_engaged: bool,
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
    if locked_while_engaged {
        ensure_unlocked(state, user).await?;
    }
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
    let detail = mail_change(&state, &user, &id, true, |loadout| {
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
    mail_change(&state, &user, &id, false, |loadout| {
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
    let detail = mail_change(&state, &user, &id, true, |loadout| {
        engine_realm::economy::salvage_many(loadout, &body.indices, economy)
            .map(|_| ())
            .map_err(economy_error)
    })
    .await?;
    tracing::info!(account_id = %account, character_id = %id, items = body.indices.len(), "items salvaged");
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
    let detail = mail_change(&state, &user, &id, false, |loadout| {
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

/// A random number in `[0, 1)` from the operating system's secure source: the only place an
/// enhancement's fate is decided. Never derived from the request.
fn secure_roll() -> f64 {
    use rand::Rng;
    rand::rngs::OsRng.gen_range(0.0..1.0)
}

async fn enhance(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<EnhanceRequest>,
) -> ApiResult<Json<EnhanceResponse>> {
    enhance_with(&state, &user, &id, &body, secure_roll()).await
}

/// One enhancement attempt with the server's `roll`. Rate limited like every character
/// mutation, refused (409) while the character is engaged, then priced, rolled and written in
/// one locked read-modify-write of the character row. Split from [`enhance`] so a test can
/// inject the roll.
async fn enhance_with(
    state: &AppState,
    user: &AuthUser,
    id: &str,
    body: &EnhanceRequest,
    roll: f64,
) -> ApiResult<Json<EnhanceResponse>> {
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let character = character_id(id)?;
    ensure_unlocked(state, user).await?;
    let outcome = store::enhance_character(&state.db, user.user_id, character, &body.slot, roll)
        .await
        .map_err(ApiError)?
        .ok_or(ApiError(AppError::NotFound))?;
    let (name, new_level) = match outcome {
        engine_realm::enhance::EnhanceOutcome::Success { new_level } => {
            (EnhanceOutcomeName::Success, new_level)
        }
        engine_realm::enhance::EnhanceOutcome::Broken => (EnhanceOutcomeName::Broken, 0),
    };
    tracing::info!(
        account_id = %user.user_id,
        character_id = %character,
        outcome = ?name,
        new_level,
        "enhancement attempted"
    );
    let row = owned(state, user, id).await?;
    let Json(detail) = detail_of(state, user, &row).await?;
    Ok(Json(EnhanceResponse {
        outcome: name,
        new_level,
        character: detail,
    }))
}

/// A refused salvage or exchange as the HTTP error: the request was wrong (400) or the state
/// it needs is not there (409); the message is the player-facing sentence.
fn economy_error(error: engine_realm::economy::EconomyError) -> AppError {
    use engine_realm::economy::EconomyError;
    let message = error.to_string();
    match error {
        EconomyError::EmptyCell
        | EconomyError::UnknownItem
        | EconomyError::NothingToSalvage
        | EconomyError::TooManyCells(_)
        | EconomyError::DuplicateCell
        | EconomyError::UnknownExchange
        | EconomyError::InvalidTimes => AppError::BadRequest(message),
        EconomyError::Insufficient(_) | EconomyError::Overflow => AppError::Conflict(message),
    }
}

/// Downgrades the character's key one level (free), dropping modifiers the lower level has no
/// slot for. Hub only: refused while the character is queued, in a party or in a dungeon, because
/// the realm holds a snapshot of the key. Answers with the character.
async fn downgrade_key(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<CharacterDetail>> {
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let row = owned(&state, &user, &id).await?;
    ensure_unlocked(&state, &user).await?;
    store::downgrade_key(&state.db, user.user_id, row.id)
        .await
        .map_err(ApiError)?
        .ok_or(ApiError(AppError::NotFound))?;
    let row = owned(&state, &user, &id).await?;
    detail_of(&state, &user, &row).await
}

/// Strips the key's modifiers for gold: all or nothing, hub only. Answers with the character.
async fn cleanse_key(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<CharacterDetail>> {
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let row = owned(&state, &user, &id).await?;
    ensure_unlocked(&state, &user).await?;
    store::cleanse_key(&state.db, user.user_id, row.id)
        .await
        .map_err(ApiError)?
        .ok_or(ApiError(AppError::NotFound))?;
    let row = owned(&state, &user, &id).await?;
    detail_of(&state, &user, &row).await
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
            slots: economy.salvage.slots,
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
) -> ApiResult<Json<CharacterDetail>> {
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let row = owned(&state, &user, &id).await?;
    ensure_unlocked(&state, &user).await?;
    let hero = characters::hero(&row);
    let mut talents: Vec<String> = Vec::new();
    for key in body.talents {
        if !talents.contains(&key) {
            talents.push(key);
        }
    }
    engine_realm::talents::validate(hero, &talents, characters::earned_points(&row))
        .map_err(talent_error)
        .map_err(ApiError)?;
    store::save_talents(&state.db, user.user_id, row.id, &talents)
        .await
        .map_err(ApiError)?;
    tracing::info!(account_id = %user.user_id, character_id = %row.id, count = talents.len(), "talents changed");
    let row = owned(&state, &user, &id).await?;
    detail_of(&state, &user, &row).await
}

/// A refused talent set as the HTTP error: an unknown key is a bad request, too few points
/// a conflict with what the character has.
fn talent_error(error: engine_realm::talents::TalentError) -> AppError {
    use engine_realm::talents::TalentError;
    match error {
        TalentError::UnknownTalent => {
            AppError::BadRequest("That is not a talent this character has.".into())
        }
        TalentError::NotEnoughPoints | TalentError::AlreadyOwned => {
            AppError::Conflict("You do not have enough talent points for that.".into())
        }
    }
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
    // The snapshot is checked once more here: a set that no longer fits (the starting points
    // changed, or a talent was retired) is refused before it reaches the realm.
    let hero = characters::hero(&row);
    engine_realm::talents::validate(hero, &row.talents.0, characters::earned_points(&row))
        .map_err(|_| {
            ApiError(AppError::BadRequest(
                "Your talents are not valid. Reset them in the talent window.".into(),
            ))
        })?;
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
        key_modifiers: key
            .as_ref()
            .map(|key| key.modifiers.clone())
            .unwrap_or_default(),
        talents: row.talents.0.clone(),
        talent_points: engine_realm::talents::total_points(characters::earned_points(&row)),
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn druid() -> engine_game::hero::Hero {
        realm_game::catalog()
            .character("druid")
            .expect("a druid")
            .hero
    }

    #[test]
    fn a_talent_set_above_the_characters_points_is_a_conflict() {
        let keys = vec!["swift_wrath".to_owned(), "verdant_sunfire".to_owned()];
        // 5 starting points hold one 3-point talent, not two.
        let error = engine_realm::talents::validate(druid(), &keys, 0).expect_err("too dear");
        assert!(matches!(talent_error(error), AppError::Conflict(_)));
        // One earned point later they fit.
        assert!(engine_realm::talents::validate(druid(), &keys, 1).is_ok());
    }

    #[test]
    fn a_key_the_hero_lacks_is_a_bad_request() {
        let error = engine_realm::talents::validate(druid(), &["burning_embers".to_owned()], 9)
            .expect_err("a mage talent");
        assert!(matches!(talent_error(error), AppError::BadRequest(_)));
    }
}
