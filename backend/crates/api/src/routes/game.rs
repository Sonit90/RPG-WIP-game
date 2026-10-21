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
//! | `POST /characters/{id}/reforge` | re-roll an item's base quality from the published table: the server rolls, the locked row pays |
//! | `POST /characters/{id}/books/use` | read a book from the bag: the item is consumed, the book learned (409 for a duplicate or another class's book) |
//! | `PUT  /characters/{id}/books/active` | choose the active option of an enhancement book (hub only: 409 while engaged) |
//! | `GET  /economy` | the economy catalog: resources, salvage rule, exchanges |
//! | `POST /characters/{id}/key/downgrade` | key one level lower, extra modifiers dropped (hub only) |
//! | `POST /characters/{id}/key/cleanse` | strip a key's modifiers for gold (hub only) |
//! | `PUT  /characters/{id}/talents` | replace talents: keys, points and the engagement lock checked |
//! | `GET  /player/settings` | the account's synced settings |
//! | `PUT  /player/settings` | save them, with optimistic versioning |
//! | `GET  /characters/{id}/history?before=&limit=` | the character's runs, newest first (owner only) |
//! | `GET  /runs/{run_uid}` | one run the caller took part in |
//! | `GET  /leaderboards?dungeon=&key=&season=&board=&page=` | ranked completed runs (any signed-in player) |
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
//! **Request ids**: the progression-changing routes (sharpen, reforge, salvage, exchange, equipment move,
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
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
    Json, Router,
};
use engine_realm::api::{
    CharacterDetail, CharacterSummary, HistoryPage, LeaderboardPage, RunDetail, EconomyInfo, SharpenOutcomeName, SharpenRequest,
    SharpenResponse, ExchangeInfo, ExchangeRequest, MoveItemRequest, QueueRequest, ResourceInfo,
    ReforgeRequest, ReforgeResponse, RequestIdBody, SalvageInfo, SalvageRequest, SettingsDocument, SetActiveBookRequest, TalentsRequest,
    UseBookRequest, UseBookResponse,
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
        .route("/characters/{id}/reforge", post(reforge))
        .route("/characters/{id}/books/use", post(use_book))
        .route("/characters/{id}/books/active", put(set_active_book))
        .route("/economy", get(get_economy))
        .route("/characters/{id}/key/downgrade", post(downgrade_key))
        .route("/characters/{id}/key/cleanse", post(cleanse_key))
        .route("/characters/{id}/talents", put(save_talents))
        .route("/player/settings", get(get_settings).put(put_settings))
        .route("/characters/{id}/history", get(character_history))
        .route("/runs/{run_uid}", get(get_run))
        .route("/leaderboards", get(get_leaderboard))
        .route("/realm/queue", post(queue))
}

fn read_limit(state: &AppState, user: &AuthUser) -> Result<(), ApiError> {
    state
        .limits
        .check(
            &format!("reads:{}", user.user_id),
            rate_limit::READS_PER_ACCOUNT,
        )
        .map_err(ApiError)
}

#[derive(serde::Deserialize)]
struct HistoryQuery {
    before: Option<i64>,
    limit: Option<u32>,
}

/// The character's runs, newest first. Owner only: a character id that is not the caller's is
/// a 404.
async fn character_history(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Json<HistoryPage>> {
    read_limit(&state, &user)?;
    let row = owned(&state, &user, &id).await?;
    let limit = query.limit.unwrap_or(engine_realm::api::HISTORY_MAX_LIMIT);
    let page = realm_game::history::history(&state.db, row.id, query.before, limit)
        .await
        .map_err(ApiError)?;
    Ok(Json(page))
}

/// One run, if the caller took part in it. Anyone else gets the same 404 as for a run that
/// does not exist.
async fn get_run(
    State(state): State<AppState>,
    user: AuthUser,
    Path(run_uid): Path<String>,
) -> ApiResult<Json<RunDetail>> {
    read_limit(&state, &user)?;
    if run_uid.is_empty() || run_uid.len() > 64 || !run_uid.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(ApiError(AppError::NotFound));
    }
    realm_game::history::run_detail(&state.db, user.user_id, &run_uid.to_ascii_lowercase())
        .await
        .map_err(ApiError)?
        .map(Json)
        .ok_or(ApiError(AppError::NotFound))
}

#[derive(serde::Deserialize)]
struct BoardParams {
    dungeon: String,
    key: Option<u32>,
    season: Option<String>,
    board: Option<String>,
    page: Option<u32>,
}

/// A leaderboard page. Built only from `dungeon_runs`, which `apply_result` writes from the
/// realm's results.
async fn get_leaderboard(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<BoardParams>,
) -> ApiResult<Json<LeaderboardPage>> {
    use engine_realm::leaderboard::Board;
    read_limit(&state, &user)?;
    let bad = |message: &str| ApiError(AppError::BadRequest(message.into()));
    let config = realm_game::leaderboards();
    let board = Board::from_key(params.board.as_deref().unwrap_or("highest"))
        .filter(|board| config.offers(*board))
        .ok_or_else(|| bad("Unknown leaderboard."))?;
    if params.dungeon.is_empty() || params.dungeon.len() > 64 {
        return Err(bad("A dungeon is required."));
    }
    if board == Board::Fastest && params.key.is_none() {
        return Err(bad("The fastest board needs a key level."));
    }
    let page = params.page.unwrap_or(0);
    if page > 10_000 {
        return Err(bad("That page is out of range."));
    }
    let season = match params.season {
        Some(season) => season,
        None => store::active_season(&state.db)
            .await
            .map_err(ApiError)?
            .map_or_else(|| realm_game::history::FALLBACK_SEASON.to_owned(), |s| s.id),
    };
    if !realm_game::history::season_exists(&state.db, &season)
        .await
        .map_err(ApiError)?
    {
        return Err(ApiError(AppError::NotFound));
    }
    let board_page = realm_game::history::leaderboard(
        &state.db,
        config,
        &realm_game::history::BoardQuery {
            board,
            dungeon_id: &params.dungeon,
            key_level: params.key,
            season_id: &season,
            page,
        },
    )
    .await
    .map_err(ApiError)?;
    Ok(Json(board_page))
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

/// One reforge: pay gold, re-roll the item's base quality from the published table.
///
/// Rate limited like every character mutation. The request id is required (a gamble must never
/// be repeated by a retry). Everything else - the row lock, the engagement question (409 while
/// the character is queued, in a party or in a dungeon), the price, the roll from the
/// operating system's random source, the write and the recorded response - happens in
/// [`store::reforge_character`], in that order, in one transaction.
async fn reforge(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<ReforgeRequest>,
) -> ApiResult<Json<ReforgeResponse>> {
    use rand::RngCore;
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let character = character_id(&id)?;
    let request_id = request_id_of(Some(&body.request_id))?.ok_or_else(|| {
        ApiError(AppError::BadRequest("A reforge needs a request id.".into()))
    })?;
    // The only source of a reforge's fate. Never derived from the request.
    let mut draw = || rand::rngs::OsRng.next_u64();
    let response = store::reforge_character(
        &state.db,
        user.user_id,
        character,
        request_id,
        &body.slot,
        &mut draw,
        || async { ensure_unlocked(&state, &user).await.map_err(|e| e.0) },
    )
    .await
    .map_err(ApiError)?
    .ok_or(ApiError(AppError::NotFound))?;
    Ok(Json(response))
}

/// Reads a book from the bag: the item is consumed and the book learned, in one transaction
/// ([`store::use_book_character`]). The request id is required. A duplicate or another class's
/// book is a 409 and the item stays. Which key level a book needs is decided at the drop, not here.
async fn use_book(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UseBookRequest>,
) -> ApiResult<Json<UseBookResponse>> {
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let character = character_id(&id)?;
    let request_id = request_id_of(Some(&body.request_id))?.ok_or_else(|| {
        ApiError(AppError::BadRequest("Reading a book needs a request id.".into()))
    })?;
    let response =
        store::use_book_character(&state.db, user.user_id, character, request_id, &body.slot)
            .await
            .map_err(ApiError)?
            .ok_or(ApiError(AppError::NotFound))?;
    Ok(Json(response))
}

/// Chooses the option in force for an enhancement book. Hub only: the realm holds a snapshot of
/// the talents, so this is a 409 while the character is queued, in a party or in a dungeon
/// ([`store::set_active_book`] asks with the row locked, before it writes).
async fn set_active_book(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<SetActiveBookRequest>,
) -> ApiResult<Json<CharacterDetail>> {
    state
        .limits
        .check(
            &format!("character:{}", user.user_id),
            rate_limit::CHARACTER_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let character = character_id(&id)?;
    let request_id = request_id_of(Some(&body.request_id))?.ok_or_else(|| {
        ApiError(AppError::BadRequest("Choosing an option needs a request id.".into()))
    })?;
    let detail = store::set_active_book(
        &state.db,
        user.user_id,
        character,
        request_id,
        &body.book_id,
        &body.option_id,
        || async { ensure_unlocked(&state, &user).await.map_err(|e| e.0) },
    )
    .await
    .map_err(ApiError)?
    .ok_or(ApiError(AppError::NotFound))?;
    Ok(Json(detail))
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
        talents: characters::snapshot_talents(&row),
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

    /// The reforge route hands the whole transaction to `store::reforge_character`, which asks
    /// the realm (with the row locked) before it writes, and feeds it the operating system's
    /// random source - nothing from the request.
    #[test]
    fn reforge_asks_the_realm_inside_the_store_and_rolls_from_the_os() {
        let text = handler("reforge");
        assert!(text.contains("store::reforge_character("));
        assert!(text.contains("ensure_unlocked(&state, &user)"));
        assert!(text.contains("OsRng"));
        assert!(!text.contains("body.roll") && !text.contains("seed"));
        let source = include_str!("../../../game/src/store.rs");
        let start = source
            .find("pub async fn reforge_character")
            .expect("the store function");
        let body = &source[start..];
        let begin = body.find("HubTx::begin(").expect("takes the row lock");
        let ask = body.find("ensure_unlocked()").expect("asks the realm");
        let write = body.find("hub.save_loadout").expect("writes");
        assert!(begin < ask && ask < write, "lock, ask, then write");
    }

    /// Choosing an option asks the realm inside the store, after the row lock and before the
    /// write; reading a book does not ask (it changes no combat number until the next queue).
    #[test]
    fn choosing_a_book_option_asks_the_realm_with_the_row_locked() {
        let text = handler("set_active_book");
        assert!(text.contains("store::set_active_book("));
        assert!(text.contains("ensure_unlocked(&state, &user)"));
        let source = include_str!("../../../game/src/store.rs");
        let start = source.find("pub async fn set_active_book").expect("the store function");
        let body = &source[start..];
        let begin = body.find("HubTx::begin(").expect("takes the row lock");
        let ask = body.find("ensure_unlocked()").expect("asks the realm");
        let write = body.find("hub.save_books").expect("writes");
        assert!(begin < ask && ask < write, "lock, ask, then write");
        assert!(!handler("use_book").contains("ensure_unlocked"));
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
