//! OIDC discovery and the JWKS, at the issuer's root.
//!
//! SpacetimeDB (and any other verifier) resolves `{iss}/.well-known/openid-configuration`,
//! follows its `jwks_uri`, and checks token signatures against the published key. Both
//! documents are public by design: they contain only the *public* half of the signing key.

use axum::{extract::State, http::header, response::IntoResponse, routing::get, Json, Router};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/.well-known/jwks.json", get(jwks))
}

async fn discovery(State(state): State<AppState>) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "public, max-age=300")], Json(state.jwt.discovery()))
}

async fn jwks(State(state): State<AppState>) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "public, max-age=300")], Json(state.jwt.jwks()))
}
