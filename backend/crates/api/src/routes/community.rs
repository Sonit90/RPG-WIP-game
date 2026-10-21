//! Community maps: upload, validate, publish, browse, moderate.
//!
//! | Route | What |
//! | --- | --- |
//! | `GET  /community/maps?q=&author=&mine=&sort=&difficulty=&status=&page=` | published maps (the caller's own with `mine`) |
//! | `POST /community/maps` | create a draft from a map file; validated, report returned |
//! | `GET  /community/maps/{id}` | one map with its latest report (published, or the author's, or a moderator's) |
//! | `PUT  /community/maps/{id}` | save a new version (author only); a published map goes back to draft |
//! | `GET  /community/maps/{id}/versions/{v}` | the stored map file of one version |
//! | `POST /community/maps/{id}/publish` | publish the current version (author only; zero errors required) |
//! | `POST /community/maps/{id}/unpublish` | back to draft (author only) |
//! | `POST /community/validate` | dry run: the report, nothing stored |
//! | `POST /community/maps/{id}/hide` `/reject` `/restore` | moderation (`community.moderate`) |
//!
//! **Data only.** A request carries `{ "request_id": ..., "data": <map file> }`; the file is
//! validated by `engine_realm::community::check` (the same mapcheck the realm and the
//! `map_check` tool run) on every create, update and publish. Nothing the client says about a
//! map's validity is read.
//!
//! **Size.** A write body over [`community::MAX_BODY_BYTES`] is refused (413) before it is
//! parsed; the mapcheck limits (objects, monsters, lights, extent) apply on top.
//!
//! **Moderation** is the `community.moderate` permission (granted to the admin role by
//! migration 0015; grant it to a single user with a `user_permissions` row).
//!
//! **The realm.** Publishing hands the validated text to the realm (`svc_community_put`);
//! unpublishing, hiding, rejecting and editing a published map take it out. Those calls are
//! best-effort here: the bridge's reconciler (`realm_bridge::sync_community`) repeats them until
//! the realm agrees with the database, and the queue route re-puts a map the realm lost.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use engine_realm::community::{
    self, MapActionRequest, MapDetail, MapPage, MapSort, MapStatus, SaveMapRequest,
    SaveMapResponse,
};
use generic_auth_auth::permissions::Permission;
use generic_auth_core::AppError;
use realm_game::community as store;
use realm_game::community::{Moderation, Published, Viewer};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::middleware::auth::{AdminUserStore, AuthUser};
use crate::rate_limit;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    let body_limit = DefaultBodyLimit::max(community::MAX_BODY_BYTES);
    Router::new()
        .route("/community/maps", get(list_maps).post(create_map))
        .route("/community/maps/{id}", get(get_map).put(update_map))
        .route("/community/maps/{id}/versions/{version}", get(get_version))
        .route("/community/maps/{id}/publish", post(publish_map))
        .route("/community/maps/{id}/unpublish", post(unpublish_map))
        .route("/community/maps/{id}/hide", post(hide_map))
        .route("/community/maps/{id}/reject", post(reject_map))
        .route("/community/maps/{id}/restore", post(restore_map))
        .route("/community/validate", post(validate_map))
        .layer(body_limit)
}

fn bad(message: &str) -> ApiError {
    ApiError(AppError::BadRequest(message.into()))
}

fn map_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| ApiError(AppError::NotFound))
}

fn request_id(raw: Option<&str>) -> Result<Option<Uuid>, ApiError> {
    raw.map(|text| {
        Uuid::parse_str(text.trim()).map_err(|_| bad("The request id is not valid."))
    })
    .transpose()
}

async fn viewer(state: &AppState, user: &AuthUser) -> Result<Viewer, ApiError> {
    let permissions = state.user_permissions(user.user_id).await.map_err(ApiError)?;
    Ok(Viewer {
        account: user.user_id,
        moderator: permissions.contains(Permission::CommunityModerate.as_str()),
    })
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

fn write_limit(state: &AppState, user: &AuthUser) -> Result<(), ApiError> {
    state
        .limits
        .check(
            &format!("community:{}", user.user_id),
            rate_limit::COMMUNITY_WRITES_PER_ACCOUNT,
        )
        .map_err(ApiError)
}

/// The text of the map file a write carried, after the size and shape checks.
fn map_text<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.len() > community::MAX_BODY_BYTES {
        return Err(ApiError(AppError::BadRequest("That map is too large.".into())));
    }
    serde_json::from_slice(body).map_err(|e| bad(&format!("The request is not valid JSON: {e}")))
}

fn save_body(body: &Bytes) -> Result<(Option<Uuid>, String), ApiError> {
    let request: SaveMapRequest = map_text(body)?;
    let id = request_id(request.request_id.as_deref())?;
    let text = serde_json::to_string(&request.data)
        .map_err(|_| bad("The map could not be read."))?;
    Ok((id, text))
}

fn action_body(body: &Bytes) -> Result<MapActionRequest, ApiError> {
    if body.is_empty() {
        return Ok(MapActionRequest::default());
    }
    map_text(body)
}

/// A save's answer: 200 with the map, or 422 with the report when the text was not a map at all
/// (nothing was stored).
fn saved_response(saved: store::Saved) -> Response {
    if saved.response.map.is_none() {
        (StatusCode::UNPROCESSABLE_ENTITY, Json(saved.response)).into_response()
    } else {
        Json(saved.response).into_response()
    }
}

// --- the realm -------------------------------------------------------------------------

fn realm_enabled(state: &AppState) -> bool {
    state.settings.realm.enabled
}

fn service_token(state: &AppState) -> Option<String> {
    match state
        .jwt
        .issue_service(&state.settings.realm.service_subject)
    {
        Ok(token) => Some(token),
        Err(e) => {
            tracing::error!(error = %e, "community: cannot sign a service token");
            None
        }
    }
}

/// Hands the realm the live version of a published map. Best-effort (see the module docs).
pub async fn push(state: &AppState, id: Uuid, version: u32, data: &str) {
    if !realm_enabled(state) {
        return;
    }
    let Some(token) = service_token(state) else {
        return;
    };
    let key = community::dungeon_id(&id.to_string(), version);
    if let Err(e) = state.realm.community_put(&token, &key, data).await {
        crate::ops::errlog::error_log().push("community", &format!("realm refused {key}: {e}"));
        tracing::warn!(map_id = %id, error = %e, "community: the realm did not take the map; the reconciler will retry");
    }
}

/// Makes sure the realm holds `cm:<id>:<version>` (a schema publish wipes its data); puts it if
/// not. Best-effort: the queue that follows fails with the realm's own message if it is still
/// missing.
pub async fn ensure_live(state: &AppState, id: Uuid, version: u32, data: &str) {
    if !realm_enabled(state) {
        return;
    }
    let Some(token) = service_token(state) else {
        return;
    };
    let key = community::dungeon_id(&id.to_string(), version);
    match state.realm.community_list(&token).await {
        Ok(live) if live.contains(&key) => {}
        _ => push(state, id, version, data).await,
    }
}

/// Takes a map out of the realm. Best-effort.
pub async fn withdraw(state: &AppState, id: Uuid) {
    if !realm_enabled(state) {
        return;
    }
    let Some(token) = service_token(state) else {
        return;
    };
    if let Err(e) = state
        .realm
        .community_remove(&token, &id.to_string())
        .await
    {
        tracing::warn!(map_id = %id, error = %e, "community: the realm did not drop the map; the reconciler will retry");
    }
}

// --- handlers --------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct ListParams {
    q: Option<String>,
    author: Option<String>,
    #[serde(default)]
    mine: bool,
    sort: Option<String>,
    difficulty: Option<u8>,
    status: Option<String>,
    page: Option<u32>,
}

async fn list_maps(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<MapPage>> {
    read_limit(&state, &user)?;
    let viewer = viewer(&state, &user).await?;
    let sort = match params.sort.as_deref() {
        None => MapSort::Newest,
        Some(key) => MapSort::from_key(key).ok_or_else(|| bad("Unknown sort."))?,
    };
    let author = params
        .author
        .as_deref()
        .map(|raw| Uuid::parse_str(raw).map_err(|_| bad("The author id is not valid.")))
        .transpose()?;
    let status = params
        .status
        .as_deref()
        .map(|raw| MapStatus::from_key(raw).ok_or_else(|| bad("Unknown status.")))
        .transpose()?;
    if params.q.as_ref().is_some_and(|q| q.chars().count() > community::SEARCH_MAX) {
        return Err(bad("That search is too long."));
    }
    if params.difficulty.is_some_and(|d| !(1..=5).contains(&d)) {
        return Err(bad("Difficulty is 1 to 5."));
    }
    let page = params.page.unwrap_or(0);
    if page > 10_000 {
        return Err(bad("That page is out of range."));
    }
    let page = store::list(
        &state.db,
        viewer,
        &store::ListQuery {
            search: params.q,
            author,
            mine: params.mine,
            sort,
            difficulty: params.difficulty,
            status,
            page,
        },
    )
    .await
    .map_err(ApiError)?;
    Ok(Json(page))
}

async fn create_map(
    State(state): State<AppState>,
    user: AuthUser,
    body: Bytes,
) -> ApiResult<Response> {
    write_limit(&state, &user)?;
    let (request_id, text) = save_body(&body)?;
    let saved = store::create(&state.db, user.user_id, request_id, &text)
        .await
        .map_err(ApiError)?;
    Ok(saved_response(saved))
}

async fn get_map(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<MapDetail>> {
    read_limit(&state, &user)?;
    let viewer = viewer(&state, &user).await?;
    store::get(&state.db, viewer, map_id(&id)?)
        .await
        .map_err(ApiError)?
        .map(Json)
        .ok_or(ApiError(AppError::NotFound))
}

async fn update_map(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Response> {
    write_limit(&state, &user)?;
    let id = map_id(&id)?;
    let (request_id, text) = save_body(&body)?;
    let saved = store::update(&state.db, user.user_id, id, request_id, &text)
        .await
        .map_err(ApiError)?;
    if saved.unpublished {
        withdraw(&state, id).await;
    }
    Ok(saved_response(saved))
}

async fn get_version(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, version)): Path<(String, u32)>,
) -> ApiResult<Response> {
    read_limit(&state, &user)?;
    let viewer = viewer(&state, &user).await?;
    let text = store::version_data(&state.db, viewer, map_id(&id)?, version)
        .await
        .map_err(ApiError)?
        .ok_or(ApiError(AppError::NotFound))?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        text,
    )
        .into_response())
}

async fn publish_map(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Response> {
    write_limit(&state, &user)?;
    let id = map_id(&id)?;
    let action = action_body(&body)?;
    let rid = request_id(action.request_id.as_deref())?;
    match store::publish(&state.db, user.user_id, id, rid)
        .await
        .map_err(ApiError)?
    {
        Published::Live { detail, data } => {
            push(&state, id, detail.summary.version, &data).await;
            Ok(Json(detail).into_response())
        }
        Published::Invalid(report) => Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(SaveMapResponse { report, map: None }),
        )
            .into_response()),
    }
}

async fn unpublish_map(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<MapDetail>> {
    write_limit(&state, &user)?;
    let id = map_id(&id)?;
    let action = action_body(&body)?;
    let rid = request_id(action.request_id.as_deref())?;
    let detail = store::unpublish(&state.db, user.user_id, id, rid)
        .await
        .map_err(ApiError)?;
    withdraw(&state, id).await;
    Ok(Json(detail))
}

async fn validate_map(
    State(state): State<AppState>,
    user: AuthUser,
    body: Bytes,
) -> ApiResult<Json<SaveMapResponse>> {
    state
        .limits
        .check(
            &format!("community-validate:{}", user.user_id),
            rate_limit::COMMUNITY_VALIDATE_PER_ACCOUNT,
        )
        .map_err(ApiError)?;
    let (_, text) = save_body(&body)?;
    Ok(Json(store::validate(&text)))
}

async fn moderate(
    state: AppState,
    user: AuthUser,
    id: String,
    body: Bytes,
    action: Moderation,
) -> ApiResult<Json<MapDetail>> {
    let viewer = viewer(&state, &user).await?;
    if !viewer.moderator {
        return Err(ApiError(AppError::Forbidden));
    }
    write_limit(&state, &user)?;
    let id = map_id(&id)?;
    let request = action_body(&body)?;
    let rid = request_id(request.request_id.as_deref())?;
    let done = store::moderate(&state.db, user.user_id, id, action, &request, rid)
        .await
        .map_err(ApiError)?;
    if action != Moderation::Restore {
        withdraw(&state, id).await;
    }
    Ok(Json(done.detail))
}

async fn hide_map(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<MapDetail>> {
    moderate(state, user, id, body, Moderation::Hide).await
}

async fn reject_map(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<MapDetail>> {
    moderate(state, user, id, body, Moderation::Reject).await
}

async fn restore_map(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<MapDetail>> {
    moderate(state, user, id, body, Moderation::Restore).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_id_must_be_a_uuid() {
        assert!(request_id(None).expect("none is fine").is_none());
        assert!(request_id(Some("not-a-uuid")).is_err());
        assert!(request_id(Some("0b8f6d0e-1c2a-4d3b-9e4f-5a6b7c8d9e0f"))
            .expect("a uuid")
            .is_some());
    }

    #[test]
    fn an_empty_action_body_is_allowed_and_a_bad_one_is_not() {
        assert!(action_body(&Bytes::new()).is_ok());
        assert!(action_body(&Bytes::from_static(b"{\"reason\":\"spam\"}")).is_ok());
        assert!(action_body(&Bytes::from_static(b"nope")).is_err());
    }

    #[test]
    fn a_save_body_carries_the_map_as_text() {
        let body = Bytes::from_static(br#"{"request_id":null,"data":{"format":1}}"#);
        let (id, text) = save_body(&body).expect("reads");
        assert!(id.is_none());
        assert_eq!(text, r#"{"format":1}"#);
        assert!(save_body(&Bytes::from_static(b"[]")).is_err());
    }
}
