use axum::{routing::get, Router};

use crate::state::AppState;

pub mod admin;
pub mod auth;
pub mod community;
pub mod game;
pub mod oidc;
pub mod ops;
pub mod users;

pub fn router(state: AppState) -> Router {
    let api_v1 = Router::new()
        .nest("/auth", auth::router())
        .nest("/users", users::router())
        .nest("/admin", admin::router())
        .merge(game::router())
        .merge(community::router())
        .with_state(state.clone());

    Router::new()
        .route("/health", get(health))
        // The operator dashboard: a static page; its data calls need an `ops.view` token.
        .route("/admin", get(ops::dashboard))
        .route("/admin/", get(ops::dashboard))
        // Discovery and the JWKS live at the issuer's root, where verifiers look for them.
        .merge(oidc::router().with_state(state))
        .nest("/api/v1", api_v1)
}

async fn health() -> &'static str {
    "ok"
}
