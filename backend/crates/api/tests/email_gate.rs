//! Verifies the email-verification gate is wired correctly.
//!
//! `AuthUser` routes   → accessible before email confirmation
//! `VerifiedUser` routes → return 403 `email_not_verified` for unconfirmed users
//!
//! No database required — `MockState` returns controlled user data.

use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    routing::get,
    Router,
};
use generic_auth_api::middleware::auth::{AuthUser, FindUserById, HasJwt, VerifiedUser};
use generic_auth_auth::JwtCodec;
use generic_auth_core::{
    models::{Role, User},
    AppError,
};
use time::OffsetDateTime;
use tower::ServiceExt as _;
use uuid::Uuid;

// ---- mock state ----------------------------------------------------------

#[derive(Clone)]
struct MockState {
    jwt: Arc<JwtCodec>,
    /// User returned by `find_user_by_id`. `None` simulates "not found".
    user: Option<User>,
}

impl MockState {
    fn with_unverified_user(user_id: Uuid) -> Self {
        Self {
            jwt: Arc::new(test_jwt()),
            user: Some(User {
                id: user_id,
                email: Some("test@example.com".into()),
                display_name: None,
                role: Role::User,
                is_active: true,
                email_verified: false,
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

// ---- helpers -------------------------------------------------------------

fn test_jwt() -> JwtCodec {
    generic_auth_auth::jwt::test_codec()
}

fn bearer_token(user_id: Uuid) -> String {
    test_jwt().issue(user_id, Role::User).expect("issue jwt")
}

fn get_req(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .method("GET")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// Stub router: routes mirror which extractor the real routes use.
///   /me, /me/permissions  → AuthUser (no email check)
///   /files                → VerifiedUser (email check)
fn test_app(state: MockState) -> Router {
    async fn auth_stub(_: AuthUser) -> StatusCode {
        StatusCode::OK
    }
    async fn verified_stub(_: VerifiedUser) -> StatusCode {
        StatusCode::OK
    }

    Router::new()
        .route("/me", get(auth_stub))
        .route("/me/permissions", get(auth_stub))
        .route("/files", get(verified_stub))
        .with_state(state)
}

// ---- tests ---------------------------------------------------------------

#[tokio::test]
async fn unverified_user_can_access_me() {
    let user_id = Uuid::new_v4();
    let app = test_app(MockState::with_unverified_user(user_id));
    let token = bearer_token(user_id);

    let status = app.oneshot(get_req("/me", &token)).await.unwrap().status();

    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "/users/me must not enforce email verification"
    );
}

#[tokio::test]
async fn unverified_user_can_access_permissions() {
    let user_id = Uuid::new_v4();
    let app = test_app(MockState::with_unverified_user(user_id));
    let token = bearer_token(user_id);

    let status = app
        .oneshot(get_req("/me/permissions", &token))
        .await
        .unwrap()
        .status();

    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "/users/me/permissions must not enforce email verification"
    );
}

#[tokio::test]
async fn unverified_user_cannot_access_files() {
    let user_id = Uuid::new_v4();
    let app = test_app(MockState::with_unverified_user(user_id));
    let token = bearer_token(user_id);

    let resp = app.oneshot(get_req("/files", &token)).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "/files must return 403 for unverified users"
    );

    let body = to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["error"]["code"], "email_not_verified",
        "error code must be email_not_verified, got: {json}"
    );
}
