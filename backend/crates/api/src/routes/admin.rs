//! Admin endpoints. All routes require the `users.manage` (or
//! `users.assign_roles`) permission, enforced inline.

use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post, patch},
    Json, Router,
};
use generic_auth_auth::permissions::{load_permissions, require, Permission};
use generic_auth_core::models::{Role, User};
use generic_auth_db::users as user_db;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::middleware::auth::{AdminUserStore, FindUserById, HasJwt, VerifiedUser};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users",                 get(list_users))
        .route("/users/{id}",             delete(delete_user::<AppState>))
        .route("/users/{id}/role",        patch(set_role))
        .route("/users/{id}/active",      patch(set_active))
        .route("/users/{id}/permissions", post(grant_permission))
}

#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct Page {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}
fn default_limit() -> i64 { 50 }

#[utoipa::path(
    get,
    path = "/api/v1/admin/users",
    params(Page),
    responses(
        (status = 200, body = Vec<User>),
        (status = 401),
        (status = 403),
    ),
    security(("bearer_auth" = [])),
    tag = "admin",
)]
pub(crate) async fn list_users(
    State(state): State<AppState>,
    user: VerifiedUser,
    Query(page): Query<Page>,
) -> ApiResult<Json<Vec<User>>> {
    let perms = load_permissions(&state.db, user.user_id).await.map_err(ApiError)?;
    require(&perms, Permission::UsersRead).map_err(ApiError)?;

    let users = user_db::list_users(&state.db, page.limit.min(200), page.offset)
        .await.map_err(|e| ApiError(e.into()))?;
    Ok(Json(users))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetRoleReq { pub role: Role }

#[utoipa::path(
    patch,
    path = "/api/v1/admin/users/{id}/role",
    params(("id" = Uuid, Path,)),
    request_body = SetRoleReq,
    responses(
        (status = 204, description = "Role updated"),
        (status = 401),
        (status = 403),
    ),
    security(("bearer_auth" = [])),
    tag = "admin",
)]
pub(crate) async fn set_role(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRoleReq>,
) -> ApiResult<axum::http::StatusCode> {
    let perms = load_permissions(&state.db, user.user_id).await.map_err(ApiError)?;
    require(&perms, Permission::UsersAssignRoles).map_err(ApiError)?;

    user_db::set_role(&state.db, id, body.role)
        .await.map_err(|e| ApiError(e.into()))?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetActiveReq { pub active: bool }

#[utoipa::path(
    patch,
    path = "/api/v1/admin/users/{id}/active",
    params(("id" = Uuid, Path,)),
    request_body = SetActiveReq,
    responses(
        (status = 204, description = "Active flag updated"),
        (status = 401),
        (status = 403),
    ),
    security(("bearer_auth" = [])),
    tag = "admin",
)]
pub(crate) async fn set_active(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetActiveReq>,
) -> ApiResult<axum::http::StatusCode> {
    let perms = load_permissions(&state.db, user.user_id).await.map_err(ApiError)?;
    require(&perms, Permission::UsersManage).map_err(ApiError)?;

    user_db::set_active(&state.db, id, body.active)
        .await.map_err(|e| ApiError(e.into()))?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GrantPermReq { pub permission: String, pub granted: bool }

#[utoipa::path(
    post,
    path = "/api/v1/admin/users/{id}/permissions",
    params(("id" = Uuid, Path,)),
    request_body = GrantPermReq,
    responses(
        (status = 204, description = "Permission updated"),
        (status = 401),
        (status = 403),
    ),
    security(("bearer_auth" = [])),
    tag = "admin",
)]
pub(crate) async fn grant_permission(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
    Json(body): Json<GrantPermReq>,
) -> ApiResult<axum::http::StatusCode> {
    let perms = load_permissions(&state.db, user.user_id).await.map_err(ApiError)?;
    require(&perms, Permission::UsersAssignRoles).map_err(ApiError)?;

    sqlx::query!(
        r#"
        INSERT INTO user_permissions (user_id, permission_id, granted)
        SELECT $1, p.id, $3 FROM permissions p WHERE p.name = $2
        ON CONFLICT (user_id, permission_id)
        DO UPDATE SET granted = EXCLUDED.granted
        "#,
        id, body.permission, body.granted,
    ).execute(&state.db).await
        .map_err(|e| ApiError(generic_auth_core::AppError::Database(e.to_string())))?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/v1/admin/users/{id}",
    params(("id" = Uuid, Path,)),
    responses(
        (status = 204, description = "User deleted"),
        (status = 400, description = "Cannot delete own account"),
        (status = 401),
        (status = 403),
        (status = 404),
    ),
    security(("bearer_auth" = [])),
    tag = "admin",
)]
pub(crate) async fn delete_user<S>(
    State(state): State<S>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
) -> ApiResult<axum::http::StatusCode>
where
    S: AdminUserStore + HasJwt + FindUserById + Clone + Send + Sync + 'static,
{
    let perms = state.user_permissions(user.user_id).await.map_err(ApiError)?;
    require(&perms, Permission::UsersManage).map_err(ApiError)?;

    if id == user.user_id {
        return Err(ApiError(generic_auth_core::AppError::BadRequest(
            "cannot delete your own account".into(),
        )));
    }

    let deleted = state.delete_user(id).await.map_err(ApiError)?;
    if !deleted {
        return Err(ApiError(generic_auth_core::AppError::NotFound));
    }

    tracing::info!(actor = %user.user_id, target = %id, "admin deleted user");
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    //! Full `delete_user` handler logic, no database. `MockState` implements the
    //! state-capability traits the handler and its extractors depend on.

    use std::collections::HashSet;
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
        routing::delete,
        Router,
    };
    use generic_auth_auth::permissions::Permission;
    use generic_auth_auth::JwtCodec;
    use generic_auth_core::{models::{Role, User}, AppError};
    use time::OffsetDateTime;
    use tower::ServiceExt as _;
    use uuid::Uuid;

    use crate::middleware::auth::{AdminUserStore, FindUserById, HasJwt};


    #[derive(Clone)]
    struct MockState {
        jwt: Arc<JwtCodec>,
        /// User returned by `find_user_by_id` (the authenticated caller).
        caller: Option<User>,
        /// Permissions the caller is granted.
        perms: HashSet<String>,
        /// What `delete_user` reports — `true` = row removed, `false` = absent.
        delete_result: bool,
    }

    impl HasJwt for MockState {
        fn jwt_codec(&self) -> &JwtCodec { &self.jwt }
    }

    #[async_trait::async_trait]
    impl FindUserById for MockState {
        async fn find_user_by_id(&self, _id: Uuid) -> Result<Option<User>, AppError> {
            Ok(self.caller.clone())
        }
    }

    #[async_trait::async_trait]
    impl AdminUserStore for MockState {
        async fn user_permissions(&self, _user_id: Uuid) -> Result<HashSet<String>, AppError> {
            Ok(self.perms.clone())
        }
        async fn delete_user(&self, _id: Uuid) -> Result<bool, AppError> {
            Ok(self.delete_result)
        }
    }

    fn test_jwt() -> JwtCodec {
        generic_auth_auth::jwt::test_codec()
    }

    fn caller(id: Uuid, email_verified: bool) -> User {
        User {
            id,
            email: Some("admin@example.com".into()),
            display_name: None,
            role: Role::Admin,
            is_active: true,
            email_verified,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    fn manage_perms() -> HashSet<String> {
        HashSet::from([Permission::UsersManage.as_str().to_string()])
    }

    fn app(state: MockState) -> Router {
        Router::new()
            .route("/admin/users/{id}", delete(super::delete_user::<MockState>))
            .with_state(state)
    }

    fn req(target: Uuid, token: Option<&str>) -> Request<Body> {
        let mut b = Request::builder()
            .uri(format!("/admin/users/{target}"))
            .method("DELETE");
        if let Some(t) = token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        b.body(Body::empty()).unwrap()
    }

    /// Issue a token whose subject matches the caller id.
    fn token_for(id: Uuid) -> String {
        test_jwt().issue(id, Role::Admin).expect("issue jwt")
    }

    async fn status_of(state: MockState, caller_id: Uuid, target: Uuid, with_token: bool) -> StatusCode {
        let token = with_token.then(|| token_for(caller_id));
        app(state)
            .oneshot(req(target, token.as_deref()))
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn no_token_is_unauthorized() {
        let id = Uuid::new_v4();
        let state = MockState {
            jwt: Arc::new(test_jwt()),
            caller: Some(caller(id, true)),
            perms: manage_perms(),
            delete_result: true,
        };
        assert_eq!(status_of(state, id, Uuid::new_v4(), false).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unverified_email_is_forbidden() {
        let id = Uuid::new_v4();
        let state = MockState {
            jwt: Arc::new(test_jwt()),
            caller: Some(caller(id, false)),
            perms: manage_perms(),
            delete_result: true,
        };
        assert_eq!(status_of(state, id, Uuid::new_v4(), true).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn missing_permission_is_forbidden() {
        let id = Uuid::new_v4();
        let state = MockState {
            jwt: Arc::new(test_jwt()),
            caller: Some(caller(id, true)),
            perms: HashSet::new(),
            delete_result: true,
        };
        assert_eq!(status_of(state, id, Uuid::new_v4(), true).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn self_delete_is_bad_request() {
        let id = Uuid::new_v4();
        let state = MockState {
            jwt: Arc::new(test_jwt()),
            caller: Some(caller(id, true)),
            perms: manage_perms(),
            delete_result: true,
        };
        // target == caller
        assert_eq!(status_of(state, id, id, true).await, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_target_is_not_found() {
        let id = Uuid::new_v4();
        let state = MockState {
            jwt: Arc::new(test_jwt()),
            caller: Some(caller(id, true)),
            perms: manage_perms(),
            delete_result: false, // row absent
        };
        assert_eq!(status_of(state, id, Uuid::new_v4(), true).await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn successful_delete_is_no_content() {
        let id = Uuid::new_v4();
        let state = MockState {
            jwt: Arc::new(test_jwt()),
            caller: Some(caller(id, true)),
            perms: manage_perms(),
            delete_result: true,
        };
        assert_eq!(status_of(state, id, Uuid::new_v4(), true).await, StatusCode::NO_CONTENT);
    }
}