//! `GET /api/v1/admin/ops/*`: the operator dashboard's data, and the page itself at `/admin`.
//!
//! | Route | What |
//! | --- | --- |
//! | `GET /ops/status` | API, database and realm health, CPU and memory, bridge state, recent errors |
//! | `GET /ops/realm` | the realm snapshot (cached for a second) |
//! | `GET /ops/instances/{id}` | one instance with resolved gear and talents |
//! | `GET /ops/anomalies` | fastest/slowest runs, too-fast runs, damage outliers |
//!
//! Every route needs the `ops.view` permission (the admin role has it). The handlers are
//! generic over the state so the permission checks run in tests without a database.

use axum::{
    extract::{FromRequestParts, Path, State},
    http::{header, request::Parts},
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use engine_realm::ops::AdminSnapshot;
use generic_auth_auth::permissions::{require, Permission};

use crate::error::ApiError;
use crate::middleware::auth::{AdminUserStore, FindUserById, HasJwt, VerifiedUser};
use crate::ops::{AnomalyReport, InstanceDetail, OpsError, OpsProvider, StatusReport};

/// The caller, verified and holding `ops.view`.
#[derive(Debug, Clone)]
pub struct OpsUser;

impl<S> FromRequestParts<S> for OpsUser
where
    S: HasJwt + FindUserById + AdminUserStore + Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = VerifiedUser::from_request_parts(parts, state).await?;
        let perms = state
            .user_permissions(user.user_id)
            .await
            .map_err(ApiError)?;
        require(&perms, Permission::OpsView).map_err(ApiError)?;
        Ok(Self)
    }
}

pub fn router<S>() -> Router<S>
where
    S: OpsProvider + HasJwt + FindUserById + AdminUserStore + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/ops/status", get(status::<S>))
        .route("/ops/realm", get(realm::<S>))
        .route("/ops/instances/{id}", get(instance::<S>))
        .route("/ops/anomalies", get(anomalies::<S>))
}

async fn status<S>(_: OpsUser, State(state): State<S>) -> Result<Json<StatusReport>, OpsError>
where
    S: OpsProvider + HasJwt + FindUserById + AdminUserStore + Clone + Send + Sync + 'static,
{
    state.status().await.map(Json)
}

async fn realm<S>(_: OpsUser, State(state): State<S>) -> Result<Json<AdminSnapshot>, OpsError>
where
    S: OpsProvider + HasJwt + FindUserById + AdminUserStore + Clone + Send + Sync + 'static,
{
    state
        .realm_snapshot()
        .await
        .map(|snapshot| Json((*snapshot).clone()))
}

async fn instance<S>(
    _: OpsUser,
    State(state): State<S>,
    Path(id): Path<u64>,
) -> Result<Json<InstanceDetail>, OpsError>
where
    S: OpsProvider + HasJwt + FindUserById + AdminUserStore + Clone + Send + Sync + 'static,
{
    state.instance_detail(id).await.map(Json)
}

async fn anomalies<S>(_: OpsUser, State(state): State<S>) -> Result<Json<AnomalyReport>, OpsError>
where
    S: OpsProvider + HasJwt + FindUserById + AdminUserStore + Clone + Send + Sync + 'static,
{
    state.anomalies().await.map(Json)
}

/// The dashboard page. Static and public: it holds no data, and every call it makes carries
/// the operator's own token.
pub async fn dashboard() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::REFERRER_POLICY, "no-referrer"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
                 connect-src 'self'; img-src data:; base-uri 'none'; form-action 'none'",
            ),
        ],
        Html(include_str!("../ops/dashboard.html")),
    )
}
