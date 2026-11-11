//! Authorization wiring for `DELETE /admin/users/{id}`.
//!
//! The real handler guards on `VerifiedUser` + the `users.manage` permission.
//! Permission/self-delete/404 logic lives in the handler body and needs a live
//! DB (see `generic-auth-db`'s `delete_user` test). Here we verify, hermetically,
//! the guard layer that gates the route:
//!   * no token            → 401 Unauthorized
//!   * valid token, verified → reaches handler (200 from stub)
//!   * valid token, unverified → 403 (email gate)
//!
//! No database required — `MockState` returns controlled user data, mirroring
//! `email_gate.rs`.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    routing::delete,
    Router,
};
use generic_auth_api::middleware::auth::{FindUserById, HasJwt, VerifiedUser};
use generic_auth_auth::JwtCodec;
use generic_auth_core::{
    models::{Role, User},
    AppError,
};
use time::OffsetDateTime;
use tower::ServiceExt as _;
use uuid::Uuid;

#[derive(Clone)]
struct MockState {
    jwt: Arc<JwtCodec>,
    user: Option<User>,
}

impl MockState {
    fn with_user(user_id: Uuid, email_verified: bool) -> Self {
        Self {
            jwt: Arc::new(test_jwt()),
            user: Some(User {
                id: user_id,
                email: Some("admin@example.com".into()),
                display_name: None,
                role: Role::Admin,
                is_active: true,
                email_verified,
                created_at: OffsetDateTime::now_utc(),
                updated_at: OffsetDateTime::now_utc(),
            }),
        }
    }
}

impl HasJwt for MockState {
    fn jwt_codec(&self) -> &JwtCodec {
        &self.jwt
    }
}

#[async_trait::async_trait]
impl FindUserById for MockState {
    async fn find_user_by_id(&self, _id: Uuid) -> Result<Option<User>, AppError> {
        Ok(self.user.clone())
    }
}

fn test_jwt() -> JwtCodec {
    generic_auth_auth::jwt::test_codec()
}

fn bearer_token(user_id: Uuid) -> String {
    test_jwt().issue(user_id, Role::Admin).expect("issue jwt")
}

/// Stub route mirroring the real one: same method + `VerifiedUser` guard.
fn test_app(state: MockState) -> Router {
    async fn delete_stub(_: VerifiedUser) -> StatusCode {
        StatusCode::OK
    }
    Router::new()
        .route("/admin/users/{id}", delete(delete_stub))
        .with_state(state)
}

fn delete_req(path: &str, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(path).method("DELETE");
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn delete_without_token_is_unauthorized() {
    let app = test_app(MockState::with_user(Uuid::new_v4(), true));
    let id = Uuid::new_v4();

    let status = app
        .oneshot(delete_req(&format!("/admin/users/{id}"), None))
        .await
        .unwrap()
        .status();

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delete_with_verified_admin_passes_guard() {
    let user_id = Uuid::new_v4();
    let app = test_app(MockState::with_user(user_id, true));
    let token = bearer_token(user_id);

    let status = app
        .oneshot(delete_req(
            &format!("/admin/users/{}", Uuid::new_v4()),
            Some(&token),
        ))
        .await
        .unwrap()
        .status();

    assert_eq!(
        status,
        StatusCode::OK,
        "verified admin must clear the guard"
    );
}

#[tokio::test]
async fn delete_with_unverified_user_is_forbidden() {
    let user_id = Uuid::new_v4();
    let app = test_app(MockState::with_user(user_id, false));
    let token = bearer_token(user_id);

    let status = app
        .oneshot(delete_req(
            &format!("/admin/users/{}", Uuid::new_v4()),
            Some(&token),
        ))
        .await
        .unwrap()
        .status();

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "unverified user must be blocked"
    );
}
