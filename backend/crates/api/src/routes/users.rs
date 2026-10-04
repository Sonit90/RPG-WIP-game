use axum::{extract::State, routing::get, Json, Router};
use generic_auth_core::models::User;
use generic_auth_db::users as user_db;

use crate::error::{ApiError, ApiResult};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me",          get(me))
        .route("/me/permissions", get(my_permissions))
}

#[utoipa::path(
    get,
    path = "/api/v1/users/me",
    responses(
        (status = 200, description = "Current user", body = User),
        (status = 401),
        (status = 404),
    ),
    security(("bearer_auth" = [])),
    tag = "users",
)]
pub(crate) async fn me(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<User>> {
    let u = user_db::find_by_id(&state.db, user.user_id)
        .await.map_err(|e| ApiError(e.into()))?
        .ok_or_else(|| ApiError(generic_auth_core::AppError::NotFound))?;
    Ok(Json(u))
}

#[utoipa::path(
    get,
    path = "/api/v1/users/me/permissions",
    responses(
        (status = 200, description = "List of permission names", body = Vec<String>),
        (status = 401),
    ),
    security(("bearer_auth" = [])),
    tag = "users",
)]
pub(crate) async fn my_permissions(
    State(state): State<AppState>, user: AuthUser,
) -> ApiResult<Json<Vec<String>>> {
    let perms = user_db::permissions_for(&state.db, user.user_id)
        .await.map_err(|e| ApiError(e.into()))?;
    Ok(Json(perms))
}