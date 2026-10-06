//! `Authorization: Bearer <jwt>` extractor + a guard middleware.

use std::collections::HashSet;

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use generic_auth_auth::JwtCodec;
use generic_auth_core::{
    models::{Role, User},
    AppError,
};
use uuid::Uuid;

use crate::error::ApiError;

/// Provides JWT verification to extractors. Implemented for `AppState`.
pub trait HasJwt: Send + Sync {
    fn jwt_codec(&self) -> &JwtCodec;
}

/// Provides user lookup by id to extractors. Implemented for `AppState`.
#[async_trait::async_trait]
pub trait FindUserById: Send + Sync {
    async fn find_user_by_id(&self, id: Uuid) -> Result<Option<User>, AppError>;
}

/// Admin user-management capabilities used by admin handlers. Implemented for
/// `AppState`; mocked in tests so handler logic runs without a database.
#[async_trait::async_trait]
pub trait AdminUserStore: Send + Sync {
    /// Effective permission names for a user (role + grants − revokes).
    async fn user_permissions(&self, user_id: Uuid) -> Result<HashSet<String>, AppError>;
    /// Permanently delete a user. `true` if a row was removed, `false` if absent.
    async fn delete_user(&self, id: Uuid) -> Result<bool, AppError>;
}

/// Authenticated user — JWT valid. Does NOT enforce email verification.
/// Use on routes that must be accessible before email is confirmed.
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub role: Role,
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: HasJwt + Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .ok_or(ApiError(AppError::Unauthorized))?;

        let token = header
            .strip_prefix("Bearer ")
            .ok_or(ApiError(AppError::Unauthorized))?
            .trim();

        let claims = state.jwt_codec().verify(token).map_err(ApiError)?;
        let role = Role::parse(&claims.role).unwrap_or(Role::User);
        Ok(AuthUser {
            user_id: claims.user_id,
            role,
        })
    }
}

/// Authenticated user with confirmed email. Returns 403 if email not verified.
#[derive(Clone, Debug)]
pub struct VerifiedUser {
    pub user_id: Uuid,
    pub role: Role,
}

impl<S> FromRequestParts<S> for VerifiedUser
where
    S: HasJwt + FindUserById + Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = AuthUser::from_request_parts(parts, state).await?;

        let user = state
            .find_user_by_id(auth.user_id)
            .await
            .map_err(ApiError)?
            .ok_or(ApiError(AppError::Unauthorized))?;

        if !user.email_verified {
            return Err(ApiError(AppError::EmailNotVerified));
        }

        Ok(VerifiedUser {
            user_id: auth.user_id,
            role: auth.role,
        })
    }
}
