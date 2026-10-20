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
//! | `POST /characters/{id}/sharpen` | one sharpening attempt: the server rolls, the locked row pays |
//! | `POST /characters/{id}/enhance` | alias of `/sharpen` (P1 name), kept for one release |
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
//! **Lock order** (every hub write, and the queue join): `SELECT ... FOR UPDATE` on the
//! character row first (`store::HubTx::begin`), *then* the engagement question, *then* the
//! mutation, then commit. The queue join holds the same row lock while it snapshots the
//! character and enqueues, so a hub write and a queue join are serialised: whichever takes the
//! row second sees the other's effect.
//!
//! **Request ids**: the progression-changing routes (sharpen, salvage, exchange, equipment move,
//! taking mail, key downgrade and cleanse, talents) take an optional `request_id` (a UUID the
//! client makes per user action and reuses on retry; in the body, which may be otherwise empty
//! for the routes without one). The first request records its response in the transaction that
//! changes the character; a replay returns that response and changes nothing. Without a
//! request id a route behaves as before.
//!
//! Every route takes the account from the verified token and scopes every query by it. A
//! character id in a path is a request, never a proof of ownership.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
    Json, Router,
};
use engine_realm::api::{
    CharacterDetail, CharacterSummary, EconomyInfo, SharpenOutcomeName, SharpenRequest,
    SharpenResponse, ExchangeInfo, ExchangeRequest, MoveItemRequest, QueueRequest, ResourceInfo,
    RequestIdBody, SalvageInfo, SalvageRequest, SettingsDocument, TalentsRequest,
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
        .route("/characters/{id}/sharpen", post(sharpen))
        // P1 name, kept as an alias for one release.
        .route("/characters/{id}/enhance", post(sharpen))
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

/// What a hub route starts from: a replayed request, or the character locked for writing.
enum Hub {
    /// The request id was already served; this is what it answered.
    Replay(serde_json::Value),
    /// The character row is locked.
    Go(Box<store::HubTx>),
}

/// The request id a body carried, if it parses as a UUID.
fn request_id_of(raw: Option<&str>) -> Result<Option<Uuid>, ApiError> {
    raw.map(|text| {
        Uuid::parse_str(text.trim())
            .map_err(|_| ApiError(AppError::BadRequest("The request id is not valid.".into())))
    })
    .transpose()
}

/// The request id of a body that carries nothing else (or no body at all): `None` for anything
/// that is not `{"request_id": ...}`, so the clients that send `{}` or `null` keep working.
fn body_request_id(body: &Bytes) -> Option<String> {
    serde_json::from_slice::<RequestIdBody>(body)
        .ok()
        .and_then(|body| body.request_id)
}

/// Rate limits a hub write, then takes the character row lock (see the module docs for the
/// order). `request_id` and `kind` make the write replayable.
async fn hub_begin(
    state: &AppState,
    user: &AuthUser,
    id: &str,
    request_id: Option<&str>,
    kind: &'static str,
) -> Result<Hub, ApiError> {
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let character = character_id(id)?;
    let request_id = request_id_of(request_id)?;
    match store::HubTx::begin(&state.db, user.user_id, character, request_id, kind)
        .await
        .map_err(ApiError)?
    {
        store::HubStart::Missing => {
            tracing::warn!(account_id = %user.user_id, character_id = %character, "character access refused");
            Err(ApiError(AppError::NotFound))
        }
        store::HubStart::Replay(response) => Ok(Hub::Replay(response)),
        store::HubStart::Locked(hub) => Ok(Hub::Go(hub)),
    }
}

/// A stored response, answered again.
fn replayed<T: serde::de::DeserializeOwned>(response: serde_json::Value) -> ApiResult<Json<T>> {
    serde_json::from_value(response).map(Json).map_err(|_| {
        ApiError(AppError::internal(
            "The earlier answer to that request could not be read.",
        ))
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
    let mut hub = match hub_begin(&state, &user, &id, body.request_id.as_deref(), "equipment_move")
        .await?
    {
        Hub::Replay(response) => return replayed(response),
        Hub::Go(hub) => hub,
    };
    ensure_unlocked(&state, &user).await?;
    let hero = characters::hero(&hub.row);
    // Under the row lock, from the stored loadout: a read from before the lock may predate a
    // chest drop.
    let moved = engine_realm::loadout::move_item(hero, &hub.row.loadout.0, &body.from, &body.to)
        .map_err(|e| AppError::BadRequest(e.message))?;
    hub.save_loadout(&moved).await?;
    let detail = hub.detail().await?;
    hub.commit(&detail).await?;
    tracing::info!(account_id = %user.user_id, character_id = %id, "equipment changed");
    Ok(Json(detail))
}

/// The mail, salvage and exchange routes: a locked read-modify-write of the loadout, then the
/// character in full. `kind` and `request_id` make the change replayable.
async fn mail_change(
    state: &AppState,
    user: &AuthUser,
    id: &str,
    locked_while_engaged: bool,
    kind: &'static str,
    request_id: Option<&str>,
    change: impl FnOnce(&mut engine_realm::loadout::Loadout) -> Result<(), AppError>,
) -> ApiResult<Json<CharacterDetail>> {
    let mut hub = match hub_begin(state, user, id, request_id, kind).await? {
        Hub::Replay(response) => return replayed(response),
        Hub::Go(hub) => hub,
    };
    if locked_while_engaged {
        ensure_unlocked(state, user).await?;
    }
    let mut loadout = hub.row.loadout.0.clone();
    change(&mut loadout)?;
    hub.save_loadout(&loadout).await?;
    let detail = hub.detail().await?;
    hub.commit(&detail).await?;
    Ok(Json(detail))
}

async fn take_mail(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, mail_id)): Path<(String, u64)>,
    body: Bytes,
) -> ApiResult<Json<CharacterDetail>> {
    use engine_realm::loadout::{take_mail, TakeMailError};
    let account = user.user_id;
    let request_id = body_request_id(&body);
    let detail = mail_change(
        &state,
        &user,
        &id,
        true,
        "mail_take",
        request_id.as_deref(),
        |loadout| {
            take_mail(loadout, mail_id)
                .map(|_| ())
                .map_err(|error| match error {
                    TakeMailError::BagFull => AppError::Conflict("Your bag is full.".into()),
                    TakeMailError::NotFound => AppError::NotFound,
                })
        },
    )
    .await?;
    tracing::info!(account_id = %account, character_id = %id, mail_id, "mail taken");
    Ok(detail)
}

async fn read_mail(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, mail_id)): Path<(String, u64)>,
) -> ApiResult<Json<CharacterDetail>> {
    mail_change(&state, &user, &id, false, "mail_read", None, |loadout| {
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
    let detail = mail_change(
        &state,
        &user,
        &id,
        true,
        "salvage",
        body.request_id.as_deref(),
        |loadout| {
            engine_realm::economy::salvage_many(loadout, &body.indices, economy)
                .map(|_| ())
                .map_err(economy_error)
        },
    )
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
    let detail = mail_change(
        &state,
        &user,
        &id,
        false,
        "exchange",
        body.request_id.as_deref(),
        |loadout| {
            engine_realm::economy::exchange(loadout, &body.exchange_id, body.times, economy)
                .map_err(economy_error)
        },
    )
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

/// A random number in `[0, 1)` from the operating system's secure source: the only place a
/// sharpening's fate is decided. Never derived from the request.
fn secure_roll() -> f64 {
    use rand::Rng;
    rand::rngs::OsRng.gen_range(0.0..1.0)
}

async fn sharpen(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<SharpenRequest>,
) -> ApiResult<Json<SharpenResponse>> {
    sharpen_with(&state, &user, &id, &body, secure_roll()).await
}

/// One sharpening attempt with the server's `roll`. Rate limited like every character
/// mutation, refused (409) while the character is engaged, then priced, rolled and written in
/// one locked read-modify-write of the character row. Split from [`sharpen`] so a test can
/// inject the roll.
async fn sharpen_with(
    state: &AppState,
    user: &AuthUser,
    id: &str,
    body: &SharpenRequest,
    roll: f64,
) -> ApiResult<Json<SharpenResponse>> {
    let mut hub = match hub_begin(state, user, id, body.request_id.as_deref(), "sharpen").await? {
        Hub::Replay(response) => return replayed(response),
        Hub::Go(hub) => hub,
    };
    ensure_unlocked(state, user).await?;
    let (changed, outcome) = store::sharpen_loadout(&hub.row.loadout.0, &body.slot, roll)?;
    hub.save_loadout(&changed).await?;
    let (name, new_level) = match outcome {
        engine_realm::sharpen::SharpenOutcome::Success { new_level } => {
            (SharpenOutcomeName::Success, new_level)
        }
        engine_realm::sharpen::SharpenOutcome::Broken => (SharpenOutcomeName::Broken, 0),
    };
    tracing::info!(
        account_id = %user.user_id,
        character_id = %hub.row.id,
        outcome = ?name,
        new_level,
        "sharpening attempted"
    );
    let response = SharpenResponse {
        outcome: name,
        new_level,
        character: hub.detail().await?,
    };
    hub.commit(&response).await?;
    Ok(Json(response))
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
    body: Bytes,
) -> ApiResult<Json<CharacterDetail>> {
    let request_id = body_request_id(&body);
    let mut hub = match hub_begin(&state, &user, &id, request_id.as_deref(), "key_downgrade")
        .await?
    {
        Hub::Replay(response) => return replayed(response),
        Hub::Go(hub) => hub,
    };
    ensure_unlocked(&state, &user).await?;
    hub.downgrade_key()
        .await?
        .ok_or(ApiError(AppError::NotFound))?;
    let detail = hub.detail().await?;
    hub.commit(&detail).await?;
    Ok(Json(detail))
}

/// Strips the key's modifiers for gold: all or nothing, hub only. Answers with the character.
async fn cleanse_key(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<CharacterDetail>> {
    let request_id = body_request_id(&body);
    let mut hub = match hub_begin(&state, &user, &id, request_id.as_deref(), "key_cleanse").await?
    {
        Hub::Replay(response) => return replayed(response),
        Hub::Go(hub) => hub,
    };
    ensure_unlocked(&state, &user).await?;
    hub.cleanse_key()
        .await?
        .ok_or(ApiError(AppError::NotFound))?;
    let detail = hub.detail().await?;
    hub.commit(&detail).await?;
    Ok(Json(detail))
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
    let mut hub = match hub_begin(&state, &user, &id, body.request_id.as_deref(), "talents").await?
    {
        Hub::Replay(response) => return replayed(response),
        Hub::Go(hub) => hub,
    };
    ensure_unlocked(&state, &user).await?;
    // Validated against the row as it is under the lock, not a read from before it.
    let hero = characters::hero(&hub.row);
    let mut talents: Vec<String> = Vec::new();
    for key in body.talents {
        if !talents.contains(&key) {
            talents.push(key);
        }
    }
    engine_realm::talents::validate(hero, &talents, characters::earned_points(&hub.row))
        .map_err(talent_error)?;
    hub.save_talents(&talents).await?;
    tracing::info!(account_id = %user.user_id, character_id = %hub.row.id, count = talents.len(), "talents changed");
    let detail = hub.detail().await?;
    hub.commit(&detail).await?;
    Ok(Json(detail))
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

/// The queue join. It holds the character row lock from the snapshot until the realm has
/// answered, so a hub write (which takes the same lock, then asks the realm) cannot change the
/// character between the snapshot and the enqueue.
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
    let character = character_id(&body.character_id)?;
    let mut hub =
        match store::HubTx::begin(&state.db, user.user_id, character, None, "queue")
            .await
            .map_err(ApiError)?
        {
            store::HubStart::Locked(hub) => hub,
            store::HubStart::Missing | store::HubStart::Replay(_) => {
                tracing::warn!(account_id = %user.user_id, character_id = %character, "character access refused");
                return Err(ApiError(AppError::NotFound));
            }
        };
    let row = hub.row.clone();
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
    let key = hub.key().await?;
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
            // The key may have been rolled for the first time; keep that, release the row.
            hub.release().await?;
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

    /// The text of the handler `name`, up to the next top-level item.
    fn handler(name: &str) -> &'static str {
        let source = include_str!("game.rs");
        let start = source
            .find(&format!("async fn {name}("))
            .unwrap_or_else(|| panic!("no handler {name}"));
        let rest = &source[start..];
        let end = rest[1..]
            .find("\nasync fn ")
            .map_or(rest.len(), |at| at + 1);
        &rest[..end]
    }

    /// Every route that changes what a run snapshots (equipment, talents, the key) asks the
    /// realm whether the character is engaged before it writes. The decision itself is
    /// `lock_decision` (tested in `realm-game`); this keeps a new or edited route from skipping
    /// the question. Exchange and mail-read touch only resources and flags, and stay open.
    #[test]
    fn every_route_that_changes_the_snapshot_checks_the_lock() {
        for direct in [
            "move_item",
            "sharpen_with",
            "downgrade_key",
            "cleanse_key",
            "save_talents",
        ] {
            assert!(
                handler(direct).contains("ensure_unlocked("),
                "{direct} must call ensure_unlocked"
            );
        }
        for through_mail_change in ["take_mail", "salvage"] {
            assert!(
                squashed(through_mail_change).contains("mail_change(&state,&user,&id,true,"),
                "{through_mail_change} must lock through mail_change"
            );
        }
        assert!(
            handler("mail_change").contains("ensure_unlocked("),
            "mail_change must check the lock when asked to"
        );
        for open in ["exchange", "read_mail"] {
            assert!(
                squashed(open).contains("mail_change(&state,&user,&id,false,"),
                "{open} is a resources/flags change and stays open"
            );
        }
    }

    /// [`handler`] without whitespace, so a call matches however rustfmt wrapped it.
    fn squashed(name: &str) -> String {
        handler(name).split_whitespace().collect()
    }

    /// The lock order of risks 2 and 3: the character row is locked (`hub_begin`) before the
    /// engagement question, and the question comes before any write. The queue join takes the
    /// same row lock, and `save_talents` validates on the locked row.
    #[test]
    fn hub_writes_lock_the_row_before_asking_the_realm_and_writing() {
        for route in [
            "move_item",
            "sharpen_with",
            "downgrade_key",
            "cleanse_key",
            "save_talents",
        ] {
            let text = handler(route);
            let begin = text.find("hub_begin(").expect("takes the row lock");
            let ask = text.find("ensure_unlocked(").expect("asks the realm");
            let write = text
                .find("hub.save_")
                .or_else(|| text.find("hub.downgrade_key"))
                .or_else(|| text.find("hub.cleanse_key"))
                .expect("writes");
            assert!(begin < ask && ask < write, "{route}: lock, ask, then write");
        }
        let mail = handler("mail_change");
        assert!(mail.find("hub_begin(") < mail.find("ensure_unlocked("));
        assert!(mail.find("ensure_unlocked(") < mail.find("hub.save_loadout"));
        let queue = handler("queue");
        assert!(
            queue.find("HubTx::begin(") < queue.find(".enqueue("),
            "the queue join holds the row lock while it snapshots and enqueues"
        );
        assert!(
            !handler("save_talents").contains("store::save_talents"),
            "talents are written under the lock, validated on the locked row"
        );
    }

    #[test]
    fn a_request_id_is_a_uuid_and_a_bodyless_route_reads_it_leniently() {
        let id = Uuid::new_v4().to_string();
        assert!(
            matches!(request_id_of(Some(&id)), Ok(Some(found)) if found.to_string() == id)
        );
        assert!(matches!(request_id_of(None), Ok(None)));
        assert!(request_id_of(Some("not-a-uuid")).is_err());

        let with = Bytes::from(format!(r#"{{"request_id":"{id}"}}"#));
        assert_eq!(body_request_id(&with).as_deref(), Some(id.as_str()));
        for without in ["", "{}", "null", "[1]", "not json"] {
            assert_eq!(body_request_id(&Bytes::from(without)), None, "{without:?}");
        }
    }

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
