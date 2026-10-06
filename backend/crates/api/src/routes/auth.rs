//! Email/password registration & login, OAuth (Google),
//! refresh-token rotation, and logout.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{get, post},
    Json, Router,
};
use generic_auth_auth::{
    generate_email_code, generate_refresh_token, hash_email_code, hash_password,
    hash_refresh_token, verify_password,
};
use generic_auth_core::{
    models::{Role, User},
    AppError,
};
use generic_auth_db::users as user_db;
use serde::{Deserialize, Serialize};
use time::{Duration as TDuration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::email::{send_password_reset_email, send_verification_email};
use crate::error::{ApiError, ApiResult};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
        .route("/verify-email-code", post(verify_email_code))
        .route("/resend-verification", post(resend_verification))
        .route("/forgot-password", post(forgot_password))
        .route("/reset-password", post(reset_password))
        .route("/change-password", post(change_password))
        .route("/oauth/google", get(google_start))
        .route("/oauth/google/callback", get(google_callback))
}

// -------- email/password ---------------------------------------------------

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RegisterReq {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 8))]
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_in: i64,
}

/// Operations the `register_user` flow needs. Lets us mock the DB + JWT
/// codec in unit tests without a Postgres connection.
#[async_trait::async_trait]
pub trait RegisterPort: Send + Sync {
    async fn create_user(
        &self,
        email: &str,
        password_hash: &str,
        display_name: Option<&str>,
    ) -> Result<User, AppError>;

    async fn store_refresh_token(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: time::OffsetDateTime,
    ) -> Result<(), AppError>;

    fn issue_access_token(&self, user_id: Uuid, role: Role) -> Result<String, AppError>;
}

#[async_trait::async_trait]
impl RegisterPort for AppState {
    async fn create_user(
        &self,
        email: &str,
        password_hash: &str,
        display_name: Option<&str>,
    ) -> Result<User, AppError> {
        user_db::create_user(
            &self.db,
            user_db::NewUser {
                email: Some(email),
                password_hash: Some(password_hash),
                display_name,
                role: Role::User,
                email_verified: false,
            },
        )
        .await
        .map_err(Into::into)
    }

    async fn store_refresh_token(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: time::OffsetDateTime,
    ) -> Result<(), AppError> {
        user_db::store_refresh_token(&self.db, user_id, token_hash, expires_at, None)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }

    fn issue_access_token(&self, user_id: Uuid, role: Role) -> Result<String, AppError> {
        self.jwt.issue(user_id, role)
    }
}

/// Pure registration logic — testable without a DB.
pub async fn register_user(
    port: &dyn RegisterPort,
    body: RegisterReq,
    access_ttl_min: i64,
    refresh_ttl_days: i64,
) -> Result<TokenPair, AppError> {
    body.validate().map_err(AppError::from_validation_errors)?;
    let phc = hash_password(&body.password)?;

    let user = port
        .create_user(&body.email, &phc, body.display_name.as_deref())
        .await?;
    let access = port.issue_access_token(user.id, user.role)?;

    let refresh = generate_refresh_token();
    let hash = hash_refresh_token(&refresh);
    let expires_at = OffsetDateTime::now_utc() + TDuration::days(refresh_ttl_days);
    port.store_refresh_token(user.id, &hash, expires_at).await?;

    Ok(TokenPair {
        access_token: access,
        refresh_token: refresh,
        access_expires_in: access_ttl_min * 60,
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/register",
    request_body = RegisterReq,
    responses(
        (status = 201, description = "Registered", body = TokenPair),
        (status = 409, description = "Email already taken"),
        (status = 422, description = "Validation error"),
    ),
    tag = "auth",
)]
pub(crate) async fn register(
    State(state): State<AppState>,
    address: crate::rate_limit::ClientAddress,
    Json(body): Json<RegisterReq>,
) -> ApiResult<(StatusCode, Json<TokenPair>)> {
    state
        .limits
        .check(
            &format!("register:{}", address.0),
            crate::rate_limit::REGISTER_PER_ADDRESS,
        )
        .map_err(ApiError)?;
    let email = body.email.clone();
    let pair = register_user(
        &state,
        body,
        state.settings.auth.jwt_access_ttl_min,
        state.settings.auth.jwt_refresh_ttl_days,
    )
    .await
    .map_err(ApiError)?;

    // Issue a verification code and email it (non-blocking).
    if let Ok(Some(u)) = user_db::find_by_email(&state.db, &email).await {
        let _ = spawn_verification_code(state.clone(), u.user.id, email).await;
    }

    Ok((StatusCode::CREATED, Json(pair)))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginReq {
    pub email: String,
    pub password: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    request_body = LoginReq,
    responses(
        (status = 200, description = "Logged in", body = TokenPair),
        (status = 401, description = "Invalid credentials"),
    ),
    tag = "auth",
)]
pub(crate) async fn login(
    State(state): State<AppState>,
    address: crate::rate_limit::ClientAddress,
    Json(body): Json<LoginReq>,
) -> ApiResult<Json<TokenPair>> {
    // Both: one address guessing many accounts, and many addresses guessing one account.
    state
        .limits
        .check(
            &format!("login-address:{}", address.0),
            crate::rate_limit::LOGIN_PER_ADDRESS,
        )
        .map_err(ApiError)?;
    state
        .limits
        .check(
            &format!("login-email:{}", body.email.trim().to_lowercase()),
            crate::rate_limit::LOGIN_PER_EMAIL,
        )
        .map_err(ApiError)?;
    let row = user_db::find_by_email(&state.db, &body.email)
        .await
        .map_err(|e| ApiError(e.into()))?
        .ok_or_else(|| ApiError(AppError::Auth("Invalid email or password".into())))?;

    let phc = row
        .password_hash
        .as_deref()
        .ok_or_else(|| ApiError(AppError::Auth("password login disabled".into())))?;

    if !verify_password(&body.password, phc).map_err(ApiError)? {
        return Err(ApiError(AppError::Auth("Invalid email or password".into())));
    }
    if !row.user.is_active {
        return Err(ApiError(AppError::Forbidden));
    }

    let pair = issue_tokens(&state, row.user.id, row.user.role, None).await?;
    Ok(Json(pair))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RefreshReq {
    pub refresh_token: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    request_body = RefreshReq,
    responses(
        (status = 200, description = "Tokens rotated", body = TokenPair),
        (status = 401, description = "Token invalid or expired"),
    ),
    tag = "auth",
)]
pub(crate) async fn refresh(
    State(state): State<AppState>,
    address: crate::rate_limit::ClientAddress,
    Json(body): Json<RefreshReq>,
) -> ApiResult<Json<TokenPair>> {
    state
        .limits
        .check(
            &format!("refresh:{}", address.0),
            crate::rate_limit::REFRESH_PER_ADDRESS,
        )
        .map_err(ApiError)?;
    let hash = hash_refresh_token(&body.refresh_token);
    let (user_id, expires_at, revoked_at) = user_db::lookup_refresh_token(&state.db, &hash)
        .await
        .map_err(|e| ApiError(e.into()))?
        .ok_or(ApiError(AppError::Unauthorized))?;
    if revoked_at.is_some() {
        // A spent refresh token presented again: either the client retried a lost response or
        // someone else holds a copy. Assume the worst and end every session of the account.
        tracing::warn!(account_id = %user_id, "refresh token reuse detected; revoking all sessions");
        user_db::revoke_all_refresh_tokens(&state.db, user_id)
            .await
            .map_err(|e| ApiError(e.into()))?;
        return Err(ApiError(AppError::Unauthorized));
    }
    if expires_at <= OffsetDateTime::now_utc() {
        return Err(ApiError(AppError::Unauthorized));
    }
    // Rotate.
    user_db::revoke_refresh_token(&state.db, &hash)
        .await
        .map_err(|e| ApiError(e.into()))?;
    let user = user_db::find_by_id(&state.db, user_id)
        .await
        .map_err(|e| ApiError(e.into()))?
        .ok_or(ApiError(AppError::Unauthorized))?;
    if !user.is_active {
        return Err(ApiError(AppError::Forbidden));
    }

    Ok(Json(issue_tokens(&state, user.id, user.role, None).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LogoutReq {
    pub refresh_token: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    request_body = LogoutReq,
    responses(
        (status = 204, description = "Logged out"),
        (status = 401),
    ),
    tag = "auth",
)]
pub(crate) async fn logout(
    State(state): State<AppState>,
    Json(body): Json<LogoutReq>,
) -> ApiResult<StatusCode> {
    let hash = hash_refresh_token(&body.refresh_token);
    user_db::revoke_refresh_token(&state.db, &hash)
        .await
        .map_err(|e| ApiError(e.into()))?;
    Ok(StatusCode::NO_CONTENT)
}

// -------- Google OAuth -----------------------------------------------------

#[utoipa::path(
    get,
    path = "/api/v1/auth/oauth/google",
    responses(
        (status = 307, description = "Redirect to Google"),
        (status = 400, description = "Google OAuth disabled"),
    ),
    tag = "auth",
)]
pub(crate) async fn google_start(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let cfg = generic_auth_auth::oauth::GoogleConfig {
        client_id: state.settings.auth.google.client_id.clone(),
        client_secret: state.settings.auth.google.client_secret.clone(),
        redirect_url: state.settings.auth.google.redirect_url.clone(),
    };
    if cfg.client_id.is_empty() {
        return Err(ApiError(AppError::BadRequest(
            "google oauth disabled".into(),
        )));
    }
    let client = generic_auth_auth::oauth::google_client(&cfg).map_err(ApiError)?;
    let (url, _csrf) = generic_auth_auth::oauth::google_authorize_url(&client);
    // TODO: persist `_csrf` in a signed cookie and check on callback.
    Ok(Redirect::temporary(url.as_str()))
}

#[derive(Debug, Deserialize, ToSchema, utoipa::IntoParams)]
pub struct OAuthCallbackQuery {
    /// OAuth authorization code
    pub code: String,
    pub state: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/oauth/google/callback",
    params(OAuthCallbackQuery),
    responses((status = 200, body = TokenPair)),
    tag = "auth",
)]
pub(crate) async fn google_callback(
    State(state): State<AppState>,
    Query(q): Query<OAuthCallbackQuery>,
) -> ApiResult<Json<TokenPair>> {
    let cfg = generic_auth_auth::oauth::GoogleConfig {
        client_id: state.settings.auth.google.client_id.clone(),
        client_secret: state.settings.auth.google.client_secret.clone(),
        redirect_url: state.settings.auth.google.redirect_url.clone(),
    };
    let client = generic_auth_auth::oauth::google_client(&cfg).map_err(ApiError)?;
    let profile = generic_auth_auth::oauth::google_exchange_code(&client, q.code, &state.http)
        .await
        .map_err(ApiError)?;

    let user = upsert_oauth_user(
        &state,
        "google",
        &profile.sub,
        profile.email.as_deref(),
        profile.name.as_deref(),
        &serde_json::to_value(&profile).unwrap(),
    )
    .await?;

    Ok(Json(issue_tokens(&state, user.id, user.role, None).await?))
}

// -------- email verification (6-digit code) --------------------------------

/// Operations the email-verification flow needs. A port so the verify/resend
/// logic can be unit-tested without a database or SMTP server.
#[async_trait::async_trait]
pub trait EmailVerifyPort: Send + Sync {
    /// `(user_id, already_verified)` for the email, or `None` if unknown.
    async fn find_user_for_verification(
        &self,
        email: &str,
    ) -> Result<Option<(Uuid, bool)>, AppError>;

    // --- verify path ---
    /// Latest active code for a user: `(code_id, code_hash, attempt_count)`.
    async fn active_code(&self, user_id: Uuid) -> Result<Option<(Uuid, String, i32)>, AppError>;
    async fn increment_attempt(&self, code_id: Uuid) -> Result<i32, AppError>;
    async fn consume_code(&self, code_id: Uuid) -> Result<(), AppError>;
    async fn mark_verified(&self, user_id: Uuid) -> Result<(), AppError>;
    /// Constant-time check of a code against its stored hash.
    fn code_matches(&self, code: &str, stored_hash: &str) -> bool;

    // --- resend path ---
    async fn count_codes_since(
        &self,
        user_id: Uuid,
        since: OffsetDateTime,
    ) -> Result<i64, AppError>;
    /// Invalidate prior codes, generate + store a fresh one, return the raw code.
    async fn issue_code(&self, user_id: Uuid) -> Result<String, AppError>;
}

#[async_trait::async_trait]
impl EmailVerifyPort for AppState {
    async fn find_user_for_verification(
        &self,
        email: &str,
    ) -> Result<Option<(Uuid, bool)>, AppError> {
        Ok(user_db::find_by_email(&self.db, email)
            .await?
            .map(|row| (row.user.id, row.user.email_verified)))
    }

    async fn active_code(&self, user_id: Uuid) -> Result<Option<(Uuid, String, i32)>, AppError> {
        user_db::find_active_email_code(&self.db, user_id)
            .await
            .map_err(Into::into)
    }

    async fn increment_attempt(&self, code_id: Uuid) -> Result<i32, AppError> {
        user_db::increment_email_code_attempt(&self.db, code_id)
            .await
            .map_err(Into::into)
    }

    async fn consume_code(&self, code_id: Uuid) -> Result<(), AppError> {
        user_db::consume_email_code(&self.db, code_id)
            .await
            .map_err(Into::into)
    }

    async fn mark_verified(&self, user_id: Uuid) -> Result<(), AppError> {
        user_db::mark_email_verified(&self.db, user_id)
            .await
            .map_err(Into::into)
    }

    fn code_matches(&self, code: &str, stored_hash: &str) -> bool {
        generic_auth_auth::verify_email_code(&self.settings.auth.jwt_secret, code, stored_hash)
    }

    async fn count_codes_since(
        &self,
        user_id: Uuid,
        since: OffsetDateTime,
    ) -> Result<i64, AppError> {
        user_db::count_recent_email_codes(&self.db, user_id, since)
            .await
            .map_err(Into::into)
    }

    async fn issue_code(&self, user_id: Uuid) -> Result<String, AppError> {
        user_db::invalidate_active_email_codes(&self.db, user_id).await?;
        let code = generate_email_code();
        let hash = hash_email_code(&self.settings.auth.jwt_secret, &code);
        let expires_at =
            OffsetDateTime::now_utc() + TDuration::minutes(self.settings.auth.email_code_ttl_min);
        user_db::store_email_verification_code(&self.db, user_id, &hash, expires_at).await?;
        Ok(code)
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct VerifyEmailCodeReq {
    #[validate(email)]
    pub email: String,
    /// 6-digit numeric code from the verification email.
    #[validate(length(equal = 6))]
    pub code: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VerifyEmailCodeResp {
    pub verified: bool,
}

/// Pure verification logic — testable without a DB.
///
/// Brute-force defenses: one active code per user, `max_attempts` wrong guesses
/// before the code is invalidated, constant-time hash comparison.
pub async fn verify_email_with_code(
    port: &dyn EmailVerifyPort,
    body: VerifyEmailCodeReq,
    max_attempts: i32,
) -> Result<(), AppError> {
    body.validate().map_err(AppError::from_validation_errors)?;
    if !body.code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::BadRequest("code must be 6 digits".into()));
    }

    // Unknown email → same response as a bad code (no account enumeration).
    let Some((user_id, already_verified)) = port.find_user_for_verification(&body.email).await?
    else {
        return Err(AppError::BadRequest("invalid or expired code".into()));
    };
    if already_verified {
        return Ok(()); // idempotent
    }

    let Some((code_id, hash, attempts)) = port.active_code(user_id).await? else {
        return Err(AppError::BadRequest(
            "code expired — request a new one".into(),
        ));
    };
    if attempts >= max_attempts {
        port.consume_code(code_id).await?;
        return Err(AppError::TooManyRequests(
            "too many attempts — request a new code".into(),
        ));
    }

    if port.code_matches(&body.code, &hash) {
        port.consume_code(code_id).await?;
        port.mark_verified(user_id).await?;
        Ok(())
    } else {
        let now = port.increment_attempt(code_id).await?;
        if now >= max_attempts {
            port.consume_code(code_id).await?; // burn it once the cap is hit
        }
        Err(AppError::BadRequest("invalid or expired code".into()))
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/verify-email-code",
    request_body = VerifyEmailCodeReq,
    responses(
        (status = 200, description = "Email verified", body = VerifyEmailCodeResp),
        (status = 400, description = "Invalid or expired code"),
        (status = 429, description = "Too many attempts"),
    ),
    tag = "auth",
)]
pub(crate) async fn verify_email_code(
    State(state): State<AppState>,
    Json(body): Json<VerifyEmailCodeReq>,
) -> ApiResult<Json<VerifyEmailCodeResp>> {
    verify_email_with_code(&state, body, state.settings.auth.email_code_max_attempts)
        .await
        .map_err(ApiError)?;
    Ok(Json(VerifyEmailCodeResp { verified: true }))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ResendVerificationReq {
    #[validate(email)]
    pub email: String,
}

/// Pure resend logic. Returns `Some((user_id, raw_code))` to email, or `None`
/// when there's nothing to do (unknown email or already verified) — the caller
/// returns the same generic response either way. Errors with `TooManyRequests`
/// when the per-account rate limit is exceeded.
pub async fn resend_verification_code(
    port: &dyn EmailVerifyPort,
    email: &str,
    cooldown_secs: i64,
    max_per_hour: i64,
) -> Result<Option<(Uuid, String)>, AppError> {
    let Some((user_id, already_verified)) = port.find_user_for_verification(email).await? else {
        return Ok(None);
    };
    if already_verified {
        return Ok(None);
    }

    let now = OffsetDateTime::now_utc();
    if port
        .count_codes_since(user_id, now - TDuration::hours(1))
        .await?
        >= max_per_hour
    {
        return Err(AppError::TooManyRequests(
            "too many requests — try again later".into(),
        ));
    }

    // The first resend is always allowed immediately; only repeat resends are
    // throttled by the cooldown. Registration issues exactly one code, so a
    // user with at most one existing code is making their first resend.
    let total_codes = port
        .count_codes_since(user_id, OffsetDateTime::UNIX_EPOCH)
        .await?;
    if total_codes > 1
        && port
            .count_codes_since(user_id, now - TDuration::seconds(cooldown_secs))
            .await?
            > 0
    {
        return Err(AppError::TooManyRequests(
            "please wait before requesting another code".into(),
        ));
    }

    let code = port.issue_code(user_id).await?;
    Ok(Some((user_id, code)))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/resend-verification",
    request_body = ResendVerificationReq,
    responses(
        (status = 200, description = "Verification code sent if the account needs it"),
        (status = 429, description = "Rate limited"),
    ),
    tag = "auth",
)]
pub(crate) async fn resend_verification(
    State(state): State<AppState>,
    Json(body): Json<ResendVerificationReq>,
) -> ApiResult<StatusCode> {
    body.validate()
        .map_err(|e| ApiError(AppError::from_validation_errors(e)))?;

    let outcome = resend_verification_code(
        &state,
        &body.email,
        state.settings.auth.email_code_resend_cooldown_secs,
        state.settings.auth.email_code_max_per_hour,
    )
    .await
    .map_err(ApiError)?;

    if let Some((_user_id, code)) = outcome {
        spawn_code_email(state, body.email, code);
    }

    // Generic response — never reveal whether the account exists.
    Ok(StatusCode::OK)
}

// -------- password recovery -----------------------------------------------

#[async_trait::async_trait]
pub trait PasswordResetPort: Send + Sync {
    /// Returns `(user_id, has_password_hash)`, or `None` if the email is not found.
    async fn find_by_email_for_reset(&self, email: &str) -> Result<Option<(Uuid, bool)>, AppError>;

    async fn store_reset_token(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: OffsetDateTime,
    ) -> Result<(), AppError>;

    async fn consume_reset_token(&self, token_hash: &str) -> Result<Option<Uuid>, AppError>;

    async fn update_password(&self, user_id: Uuid, phc: &str) -> Result<(), AppError>;

    async fn revoke_sessions(&self, user_id: Uuid) -> Result<(), AppError>;
}

#[async_trait::async_trait]
impl PasswordResetPort for AppState {
    async fn find_by_email_for_reset(&self, email: &str) -> Result<Option<(Uuid, bool)>, AppError> {
        Ok(user_db::find_by_email(&self.db, email)
            .await?
            .map(|row| (row.user.id, row.password_hash.is_some())))
    }

    async fn store_reset_token(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: OffsetDateTime,
    ) -> Result<(), AppError> {
        user_db::store_password_reset_token(&self.db, user_id, token_hash, expires_at)
            .await
            .map_err(Into::into)
    }

    async fn consume_reset_token(&self, token_hash: &str) -> Result<Option<Uuid>, AppError> {
        user_db::consume_password_reset_token(&self.db, token_hash)
            .await
            .map_err(Into::into)
    }

    async fn update_password(&self, user_id: Uuid, phc: &str) -> Result<(), AppError> {
        user_db::update_password_hash(&self.db, user_id, phc)
            .await
            .map_err(Into::into)
    }

    async fn revoke_sessions(&self, user_id: Uuid) -> Result<(), AppError> {
        user_db::revoke_all_refresh_tokens(&self.db, user_id)
            .await
            .map_err(Into::into)
    }
}

/// Generates and stores a reset token. Returns the raw token so the caller can send the email.
/// Returns `None` when no action is needed (unknown email or OAuth-only account).
pub async fn request_password_reset(
    port: &dyn PasswordResetPort,
    email: &str,
    reset_ttl_hours: i64,
) -> Result<Option<String>, AppError> {
    let Some((user_id, has_password)) = port.find_by_email_for_reset(email).await? else {
        return Ok(None);
    };
    if !has_password {
        return Ok(None);
    }
    let token = generate_verification_token();
    let hash = sha256_hex(&token);
    let expires_at = OffsetDateTime::now_utc() + TDuration::hours(reset_ttl_hours);
    port.store_reset_token(user_id, &hash, expires_at).await?;
    Ok(Some(token))
}

/// Validates, consumes the token, updates the password hash, and revokes all sessions.
pub async fn perform_password_reset(
    port: &dyn PasswordResetPort,
    body: ResetPasswordReq,
) -> Result<(), AppError> {
    body.validate().map_err(AppError::from_validation_errors)?;
    let token_hash = sha256_hex(&body.token);
    let user_id = port
        .consume_reset_token(&token_hash)
        .await?
        .ok_or_else(|| AppError::BadRequest("invalid or expired token".into()))?;
    let phc = hash_password(&body.new_password)?;
    port.update_password(user_id, &phc).await?;
    port.revoke_sessions(user_id).await?;
    Ok(())
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ForgotPasswordReq {
    pub email: String,
}

#[utoipa::path(
    post,
    operation_id = "forgot_password",
    path = "/api/v1/auth/forgot-password",
    request_body = ForgotPasswordReq,
    responses(
        (status = 204, description = "Reset email sent if account exists"),
    ),
    tag = "auth",
)]
pub(crate) async fn forgot_password(
    State(state): State<AppState>,
    Json(body): Json<ForgotPasswordReq>,
) -> ApiResult<StatusCode> {
    // Always 204 — don't reveal whether the email exists.
    if let Ok(Some(token)) = request_password_reset(&state, &body.email, 1).await {
        let url = format!(
            "{}/reset-password?token={}",
            state.settings.http.public_url, token
        );
        let email = body.email.clone();
        tokio::spawn(async move {
            if let Err(e) = send_password_reset_email(&state.settings.email, &email, &url).await {
                tracing::error!(error = %e, "failed to send password reset email");
            }
        });
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ResetPasswordReq {
    pub token: String,
    #[validate(length(min = 8))]
    pub new_password: String,
}

#[utoipa::path(
    post,
    operation_id = "reset_password",
    path = "/api/v1/auth/reset-password",
    request_body = ResetPasswordReq,
    responses(
        (status = 204, description = "Password updated"),
        (status = 400, description = "Token invalid or expired, or password too short"),
    ),
    tag = "auth",
)]
pub(crate) async fn reset_password(
    State(state): State<AppState>,
    Json(body): Json<ResetPasswordReq>,
) -> ApiResult<StatusCode> {
    perform_password_reset(&state, body)
        .await
        .map_err(ApiError)?;
    Ok(StatusCode::NO_CONTENT)
}

// -------- change password (authenticated) ----------------------------------

/// Operations the authenticated change-password flow needs. A port so the
/// logic can be unit-tested without a database.
#[async_trait::async_trait]
pub trait ChangePasswordPort: Send + Sync {
    /// Whether the account's email is verified. Errors `Unauthorized` if the
    /// user no longer exists.
    async fn email_verified(&self, user_id: Uuid) -> Result<bool, AppError>;
    /// Current Argon2 hash, or `None` for an unknown / OAuth-only account.
    async fn password_hash_for(&self, user_id: Uuid) -> Result<Option<String>, AppError>;
    async fn update_password(&self, user_id: Uuid, phc: &str) -> Result<(), AppError>;
    async fn revoke_sessions(&self, user_id: Uuid) -> Result<(), AppError>;
}

#[async_trait::async_trait]
impl ChangePasswordPort for AppState {
    async fn email_verified(&self, user_id: Uuid) -> Result<bool, AppError> {
        let user = user_db::find_by_id(&self.db, user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;
        Ok(user.email_verified)
    }

    async fn password_hash_for(&self, user_id: Uuid) -> Result<Option<String>, AppError> {
        user_db::password_hash_by_id(&self.db, user_id)
            .await
            .map_err(Into::into)
    }

    async fn update_password(&self, user_id: Uuid, phc: &str) -> Result<(), AppError> {
        user_db::update_password_hash(&self.db, user_id, phc)
            .await
            .map_err(Into::into)
    }

    async fn revoke_sessions(&self, user_id: Uuid) -> Result<(), AppError> {
        user_db::revoke_all_refresh_tokens(&self.db, user_id)
            .await
            .map_err(Into::into)
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ChangePasswordReq {
    pub current_password: String,
    #[validate(length(min = 8))]
    pub new_password: String,
}

/// Pure change-password logic — testable without a DB.
///
/// Verifies the current password, stores the new hash, and revokes all refresh
/// tokens so existing sessions must re-authenticate.
pub async fn change_password_for_user(
    port: &dyn ChangePasswordPort,
    user_id: Uuid,
    body: ChangePasswordReq,
) -> Result<(), AppError> {
    body.validate().map_err(AppError::from_validation_errors)?;

    if !port.email_verified(user_id).await? {
        return Err(AppError::EmailNotVerified);
    }

    let phc = port.password_hash_for(user_id).await?.ok_or_else(|| {
        AppError::BadRequest("password login not enabled for this account".into())
    })?;

    if !verify_password(&body.current_password, &phc)? {
        return Err(AppError::Auth("current password is incorrect".into()));
    }
    if body.new_password == body.current_password {
        return Err(AppError::BadRequest(
            "new password must differ from current".into(),
        ));
    }

    let new_phc = hash_password(&body.new_password)?;
    port.update_password(user_id, &new_phc).await?;
    port.revoke_sessions(user_id).await?;
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/change-password",
    request_body = ChangePasswordReq,
    responses(
        (status = 204, description = "Password changed; existing sessions revoked"),
        (status = 400, description = "OAuth-only account or new password not different"),
        (status = 401, description = "Not authenticated or current password incorrect"),
        (status = 403, description = "Email not verified"),
        (status = 422, description = "New password too short"),
    ),
    security(("bearer_auth" = [])),
    tag = "auth",
)]
pub(crate) async fn change_password(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<ChangePasswordReq>,
) -> ApiResult<StatusCode> {
    change_password_for_user(&state, auth.user_id, body)
        .await
        .map_err(ApiError)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Issue a fresh verification code for a user and email it in the background.
/// Errors are swallowed (logged) — registration must not fail on email trouble.
async fn spawn_verification_code(state: AppState, user_id: Uuid, email: String) -> Result<(), ()> {
    let code = state.issue_code(user_id).await.map_err(|_| ())?;
    spawn_code_email(state, email, code);
    Ok(())
}

/// Build the verification link and dispatch the email on a background task.
fn spawn_code_email(state: AppState, email: String, code: String) {
    let query: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("email", &email)
        .append_pair("code", &code)
        .finish();
    let url = format!("{}/activate?{}", state.settings.http.public_url, query);
    let ttl_min = state.settings.auth.email_code_ttl_min;

    tokio::spawn(async move {
        if let Err(e) =
            send_verification_email(&state.settings.email, &email, &code, &url, ttl_min).await
        {
            tracing::error!(error = %e, "failed to send verification email");
        }
    });
}

fn generate_verification_token() -> String {
    // Two v4 UUIDs concatenated → 256-bit URL-safe token.
    format!(
        "{}{}",
        Uuid::new_v4().as_simple(),
        Uuid::new_v4().as_simple()
    )
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(input.as_bytes());
    hex::encode(hash)
}

// -------- helpers ----------------------------------------------------------

async fn upsert_oauth_user(
    state: &AppState,
    provider: &str,
    subject: &str,
    email: Option<&str>,
    display: Option<&str>,
    raw: &serde_json::Value,
) -> ApiResult<generic_auth_core::models::User> {
    if let Some(u) = user_db::find_user_by_oauth(&state.db, provider, subject)
        .await
        .map_err(|e| ApiError(e.into()))?
    {
        user_db::upsert_oauth_identity(&state.db, u.id, provider, subject, email, raw)
            .await
            .map_err(|e| ApiError(e.into()))?;
        return Ok(u);
    }
    let user = user_db::create_user(
        &state.db,
        user_db::NewUser {
            email,
            password_hash: None,
            display_name: display,
            role: Role::User,
            email_verified: email.is_some(),
        },
    )
    .await
    .map_err(|e| ApiError(e.into()))?;
    user_db::upsert_oauth_identity(&state.db, user.id, provider, subject, email, raw)
        .await
        .map_err(|e| ApiError(e.into()))?;
    Ok(user)
}

async fn issue_tokens(
    state: &AppState,
    user_id: Uuid,
    role: Role,
    user_agent: Option<&str>,
) -> ApiResult<TokenPair> {
    // The display name rides in the token so the realm can show it to other players.
    let name = user_db::find_by_id(&state.db, user_id)
        .await
        .ok()
        .flatten()
        .and_then(|user| user.display_name);
    let access = state
        .jwt
        .issue_named(user_id, role, name.as_deref())
        .map_err(ApiError)?;
    let refresh = generate_refresh_token();
    let hash = hash_refresh_token(&refresh);
    let exp = OffsetDateTime::now_utc() + TDuration::days(state.settings.auth.jwt_refresh_ttl_days);
    user_db::store_refresh_token(&state.db, user_id, &hash, exp, user_agent)
        .await
        .map_err(|e| ApiError(e.into()))?;
    Ok(TokenPair {
        access_token: access,
        refresh_token: refresh,
        access_expires_in: state.settings.auth.jwt_access_ttl_min * 60,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use time::OffsetDateTime;

    /// Mock implementation of `RegisterPort`. Records calls and returns
    /// canned responses. Errors aren't `Clone`, so each response slot is a
    /// `Mutex<Option<Result>>` that the trait method `take()`s.
    #[derive(Default)]
    struct MockPort {
        create_user_response: Mutex<Option<Result<User, AppError>>>,
        store_token_response: Mutex<Option<Result<(), AppError>>>,
        issue_token_response: Mutex<Option<Result<String, AppError>>>,

        create_user_calls: Mutex<Vec<(String, String, Option<String>)>>,
        store_token_calls: Mutex<Vec<(uuid::Uuid, String)>>,
    }

    impl MockPort {
        fn with_user(self, u: User) -> Self {
            *self.create_user_response.lock().unwrap() = Some(Ok(u));
            self
        }
        fn with_create_err(self, e: AppError) -> Self {
            *self.create_user_response.lock().unwrap() = Some(Err(e));
            self
        }
        fn with_access_token(self, tok: &str) -> Self {
            *self.issue_token_response.lock().unwrap() = Some(Ok(tok.into()));
            self
        }
        fn with_store_ok(self) -> Self {
            *self.store_token_response.lock().unwrap() = Some(Ok(()));
            self
        }
    }

    #[async_trait::async_trait]
    impl RegisterPort for MockPort {
        async fn create_user(
            &self,
            email: &str,
            phc: &str,
            display_name: Option<&str>,
        ) -> Result<User, AppError> {
            self.create_user_calls.lock().unwrap().push((
                email.into(),
                phc.into(),
                display_name.map(|s| s.into()),
            ));
            self.create_user_response
                .lock()
                .unwrap()
                .take()
                .expect("create_user_response not set")
        }

        async fn store_refresh_token(
            &self,
            user_id: uuid::Uuid,
            token_hash: &str,
            _expires_at: OffsetDateTime,
        ) -> Result<(), AppError> {
            self.store_token_calls
                .lock()
                .unwrap()
                .push((user_id, token_hash.into()));
            self.store_token_response
                .lock()
                .unwrap()
                .take()
                .expect("store_token_response not set")
        }

        fn issue_access_token(&self, _u: uuid::Uuid, _r: Role) -> Result<String, AppError> {
            self.issue_token_response
                .lock()
                .unwrap()
                .take()
                .expect("issue_token_response not set")
        }
    }

    fn sample_user(email: &str) -> User {
        let now = OffsetDateTime::now_utc();
        User {
            id: uuid::Uuid::new_v4(),
            email: Some(email.into()),
            display_name: None,
            role: Role::User,
            is_active: true,
            email_verified: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn body(email: &str, password: &str) -> RegisterReq {
        RegisterReq {
            email: email.into(),
            password: password.into(),
            display_name: None,
        }
    }

    // ---- happy path -------------------------------------------------------

    #[tokio::test]
    async fn happy_path_returns_token_pair_and_persists() {
        let user = sample_user("alice@example.com");
        let port = MockPort::default()
            .with_user(user.clone())
            .with_access_token("jwt.access.token")
            .with_store_ok();

        let pair = register_user(&port, body("alice@example.com", "longenoughpw"), 30, 14)
            .await
            .expect("register should succeed");

        assert_eq!(pair.access_token, "jwt.access.token");
        assert!(!pair.refresh_token.is_empty());
        assert_eq!(pair.access_expires_in, 30 * 60);

        // create_user called with the email and a non-empty Argon2 hash (not the raw password).
        let creates = port.create_user_calls.lock().unwrap();
        assert_eq!(creates.len(), 1);
        assert_eq!(creates[0].0, "alice@example.com");
        assert!(creates[0].1.starts_with("$argon2"));
        assert_ne!(creates[0].1, "longenoughpw");

        // refresh token stored as its SHA-256 hash, not the raw token.
        let stores = port.store_token_calls.lock().unwrap();
        assert_eq!(stores.len(), 1);
        assert_eq!(stores[0].0, user.id);
        assert_ne!(stores[0].1, pair.refresh_token);
        assert_eq!(stores[0].1, hash_refresh_token(&pair.refresh_token));
    }

    // ---- validation -------------------------------------------------------

    #[tokio::test]
    async fn rejects_invalid_email() {
        let port = MockPort::default(); // no DB calls expected
        let err = register_user(&port, body("not-an-email", "longenoughpw"), 30, 14)
            .await
            .unwrap_err();

        match err {
            AppError::ValidationFields { fields, .. } => {
                assert!(
                    fields.contains_key("email"),
                    "expected email field error, got {fields:?}"
                );
            }
            other => panic!("expected ValidationFields, got {other:?}"),
        }
        assert!(
            port.create_user_calls.lock().unwrap().is_empty(),
            "create_user must not be called for invalid input"
        );
    }

    #[tokio::test]
    async fn rejects_short_password() {
        let port = MockPort::default();
        let err = register_user(&port, body("alice@example.com", "short"), 30, 14)
            .await
            .unwrap_err();

        match err {
            AppError::ValidationFields { fields, .. } => {
                assert!(fields.contains_key("password"));
            }
            other => panic!("expected ValidationFields, got {other:?}"),
        }
    }

    // ---- DB error propagation --------------------------------------------

    #[tokio::test]
    async fn propagates_db_conflict_and_does_not_issue_tokens() {
        let port = MockPort::default().with_create_err(AppError::Conflict("email taken".into()));

        let err = register_user(&port, body("alice@example.com", "longenoughpw"), 30, 14)
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::Conflict(_)));
        // We never reach token storage. (issue_access_token would have panicked
        // since no canned response was set — that it didn't proves it wasn't called.)
        assert!(port.store_token_calls.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod password_reset_tests {
    use super::*;
    use std::sync::Mutex;

    /// A queued reply to `find_by_email_for_reset`: `(user_id, has_password_hash)`.
    type FindResponse = Result<Option<(Uuid, bool)>, AppError>;

    #[derive(Default)]
    struct MockResetPort {
        find_response: Mutex<Option<FindResponse>>,
        store_response: Mutex<Option<Result<(), AppError>>>,
        consume_response: Mutex<Option<Result<Option<Uuid>, AppError>>>,
        update_response: Mutex<Option<Result<(), AppError>>>,
        revoke_response: Mutex<Option<Result<(), AppError>>>,

        store_calls: Mutex<Vec<(Uuid, String)>>,
        update_calls: Mutex<Vec<(Uuid, String)>>,
        revoke_calls: Mutex<Vec<Uuid>>,
    }

    impl MockResetPort {
        fn with_user(self, user_id: Uuid) -> Self {
            *self.find_response.lock().unwrap() = Some(Ok(Some((user_id, true))));
            self
        }
        fn with_oauth_user(self, user_id: Uuid) -> Self {
            *self.find_response.lock().unwrap() = Some(Ok(Some((user_id, false))));
            self
        }
        fn with_no_user(self) -> Self {
            *self.find_response.lock().unwrap() = Some(Ok(None));
            self
        }
        fn with_store_ok(self) -> Self {
            *self.store_response.lock().unwrap() = Some(Ok(()));
            self
        }
        fn with_valid_token(self, user_id: Uuid) -> Self {
            *self.consume_response.lock().unwrap() = Some(Ok(Some(user_id)));
            self
        }
        fn with_invalid_token(self) -> Self {
            *self.consume_response.lock().unwrap() = Some(Ok(None));
            self
        }
        fn with_update_ok(self) -> Self {
            *self.update_response.lock().unwrap() = Some(Ok(()));
            self
        }
        fn with_revoke_ok(self) -> Self {
            *self.revoke_response.lock().unwrap() = Some(Ok(()));
            self
        }
    }

    #[async_trait::async_trait]
    impl PasswordResetPort for MockResetPort {
        async fn find_by_email_for_reset(
            &self,
            _email: &str,
        ) -> Result<Option<(Uuid, bool)>, AppError> {
            self.find_response
                .lock()
                .unwrap()
                .take()
                .expect("find_response not set")
        }

        async fn store_reset_token(
            &self,
            user_id: Uuid,
            token_hash: &str,
            _expires_at: OffsetDateTime,
        ) -> Result<(), AppError> {
            self.store_calls
                .lock()
                .unwrap()
                .push((user_id, token_hash.into()));
            self.store_response
                .lock()
                .unwrap()
                .take()
                .expect("store_response not set")
        }

        async fn consume_reset_token(&self, _token_hash: &str) -> Result<Option<Uuid>, AppError> {
            self.consume_response
                .lock()
                .unwrap()
                .take()
                .expect("consume_response not set")
        }

        async fn update_password(&self, user_id: Uuid, phc: &str) -> Result<(), AppError> {
            self.update_calls
                .lock()
                .unwrap()
                .push((user_id, phc.into()));
            self.update_response
                .lock()
                .unwrap()
                .take()
                .expect("update_response not set")
        }

        async fn revoke_sessions(&self, user_id: Uuid) -> Result<(), AppError> {
            self.revoke_calls.lock().unwrap().push(user_id);
            self.revoke_response
                .lock()
                .unwrap()
                .take()
                .expect("revoke_response not set")
        }
    }

    fn reset_req(token: &str, password: &str) -> ResetPasswordReq {
        ResetPasswordReq {
            token: token.into(),
            new_password: password.into(),
        }
    }

    // ---- request_password_reset -------------------------------------------

    #[tokio::test]
    async fn request_stores_token_for_password_user() {
        let user_id = Uuid::new_v4();
        let port = MockResetPort::default().with_user(user_id).with_store_ok();

        let token = request_password_reset(&port, "alice@example.com", 1)
            .await
            .expect("should succeed");

        let token = token.expect("should return token for password user");
        assert!(!token.is_empty());

        let stores = port.store_calls.lock().unwrap();
        assert_eq!(stores.len(), 1, "exactly one store call");
        assert_eq!(stores[0].0, user_id);
        // stored value must be the SHA-256 hash, not the raw token
        assert_ne!(stores[0].1, token, "must not store raw token");
        assert_eq!(
            stores[0].1,
            sha256_hex(&token),
            "must store SHA-256 of token"
        );
    }

    #[tokio::test]
    async fn request_no_op_for_unknown_email() {
        let port = MockResetPort::default().with_no_user();

        let result = request_password_reset(&port, "nobody@example.com", 1)
            .await
            .expect("should not error");

        assert!(result.is_none());
        assert!(
            port.store_calls.lock().unwrap().is_empty(),
            "must not store token"
        );
    }

    #[tokio::test]
    async fn request_no_op_for_oauth_only_account() {
        let user_id = Uuid::new_v4();
        let port = MockResetPort::default().with_oauth_user(user_id);

        let result = request_password_reset(&port, "oauth@example.com", 1)
            .await
            .expect("should not error");

        assert!(result.is_none());
        assert!(
            port.store_calls.lock().unwrap().is_empty(),
            "must not store token for OAuth-only account",
        );
    }

    // ---- perform_password_reset -------------------------------------------

    #[tokio::test]
    async fn reset_updates_password_and_revokes_sessions() {
        let user_id = Uuid::new_v4();
        let port = MockResetPort::default()
            .with_valid_token(user_id)
            .with_update_ok()
            .with_revoke_ok();

        perform_password_reset(&port, reset_req("some-token", "newpassword123"))
            .await
            .expect("should succeed");

        let updates = port.update_calls.lock().unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, user_id);
        assert!(
            updates[0].1.starts_with("$argon2"),
            "must store Argon2 hash"
        );
        assert_ne!(
            updates[0].1, "newpassword123",
            "must not store raw password"
        );

        let revokes = port.revoke_calls.lock().unwrap();
        assert_eq!(revokes.len(), 1, "must revoke sessions");
        assert_eq!(revokes[0], user_id);
    }

    #[tokio::test]
    async fn reset_rejects_short_password() {
        // No responses set — any DB call would panic, proving none are made.
        let port = MockResetPort::default();

        let err = perform_password_reset(&port, reset_req("some-token", "short"))
            .await
            .unwrap_err();

        match err {
            AppError::ValidationFields { fields, .. } => {
                assert!(
                    fields.contains_key("new_password"),
                    "expected new_password error, got {fields:?}"
                );
            }
            other => panic!("expected ValidationFields, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reset_rejects_invalid_token() {
        // consume returns None; update/revoke responses intentionally not set.
        let port = MockResetPort::default().with_invalid_token();

        let err = perform_password_reset(&port, reset_req("bad-token", "validpassword"))
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
        assert!(
            port.update_calls.lock().unwrap().is_empty(),
            "must not update password"
        );
        assert!(
            port.revoke_calls.lock().unwrap().is_empty(),
            "must not revoke sessions"
        );
    }
}

#[cfg(test)]
mod email_verify_tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Mock `EmailVerifyPort`. `code_matches` treats `correct_code` as the only
    /// valid guess; `counts` feeds `count_codes_since` responses in call order.
    #[derive(Default)]
    struct MockPort {
        user: Option<(Uuid, bool)>,
        active: Mutex<Option<(Uuid, String, i32)>>,
        correct_code: String,
        increment_to: i32,
        counts: Mutex<VecDeque<i64>>,
        issued: String,

        consume_calls: Mutex<Vec<Uuid>>,
        verified_calls: Mutex<Vec<Uuid>>,
        increment_calls: Mutex<Vec<Uuid>>,
        issue_calls: Mutex<Vec<Uuid>>,
    }

    impl MockPort {
        fn with_user(mut self, id: Uuid, verified: bool) -> Self {
            self.user = Some((id, verified));
            self
        }
        fn with_active_code(self, id: Uuid, hash: &str, attempts: i32) -> Self {
            *self.active.lock().unwrap() = Some((id, hash.into(), attempts));
            self
        }
        fn with_correct_code(mut self, code: &str) -> Self {
            self.correct_code = code.into();
            self
        }
        fn with_increment_to(mut self, n: i32) -> Self {
            self.increment_to = n;
            self
        }
        fn with_counts(self, counts: &[i64]) -> Self {
            *self.counts.lock().unwrap() = counts.iter().copied().collect();
            self
        }
        fn with_issued(mut self, code: &str) -> Self {
            self.issued = code.into();
            self
        }
    }

    #[async_trait::async_trait]
    impl EmailVerifyPort for MockPort {
        async fn find_user_for_verification(
            &self,
            _email: &str,
        ) -> Result<Option<(Uuid, bool)>, AppError> {
            Ok(self.user)
        }
        async fn active_code(
            &self,
            _user_id: Uuid,
        ) -> Result<Option<(Uuid, String, i32)>, AppError> {
            Ok(self.active.lock().unwrap().clone())
        }
        async fn increment_attempt(&self, code_id: Uuid) -> Result<i32, AppError> {
            self.increment_calls.lock().unwrap().push(code_id);
            Ok(self.increment_to)
        }
        async fn consume_code(&self, code_id: Uuid) -> Result<(), AppError> {
            self.consume_calls.lock().unwrap().push(code_id);
            Ok(())
        }
        async fn mark_verified(&self, user_id: Uuid) -> Result<(), AppError> {
            self.verified_calls.lock().unwrap().push(user_id);
            Ok(())
        }
        fn code_matches(&self, code: &str, _stored_hash: &str) -> bool {
            !self.correct_code.is_empty() && code == self.correct_code
        }
        async fn count_codes_since(
            &self,
            _user_id: Uuid,
            _since: OffsetDateTime,
        ) -> Result<i64, AppError> {
            Ok(self.counts.lock().unwrap().pop_front().unwrap_or(0))
        }
        async fn issue_code(&self, user_id: Uuid) -> Result<String, AppError> {
            self.issue_calls.lock().unwrap().push(user_id);
            Ok(self.issued.clone())
        }
    }

    fn req(code: &str) -> VerifyEmailCodeReq {
        VerifyEmailCodeReq {
            email: "alice@example.com".into(),
            code: code.into(),
        }
    }

    // ---- verify_email_with_code -------------------------------------------

    #[tokio::test]
    async fn correct_code_marks_verified_and_consumes() {
        let uid = Uuid::new_v4();
        let code_id = Uuid::new_v4();
        let port = MockPort::default()
            .with_user(uid, false)
            .with_active_code(code_id, "hash", 0)
            .with_correct_code("123456");

        verify_email_with_code(&port, req("123456"), 5)
            .await
            .expect("should verify");

        assert_eq!(
            *port.consume_calls.lock().unwrap(),
            vec![code_id],
            "code consumed"
        );
        assert_eq!(
            *port.verified_calls.lock().unwrap(),
            vec![uid],
            "user marked verified"
        );
        assert!(
            port.increment_calls.lock().unwrap().is_empty(),
            "no failed attempt"
        );
    }

    #[tokio::test]
    async fn wrong_code_increments_without_consuming() {
        let uid = Uuid::new_v4();
        let code_id = Uuid::new_v4();
        let port = MockPort::default()
            .with_user(uid, false)
            .with_active_code(code_id, "hash", 0)
            .with_correct_code("123456")
            .with_increment_to(1);

        let err = verify_email_with_code(&port, req("000000"), 5)
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
        assert_eq!(*port.increment_calls.lock().unwrap(), vec![code_id]);
        assert!(
            port.consume_calls.lock().unwrap().is_empty(),
            "code not burned yet"
        );
        assert!(port.verified_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn wrong_code_at_cap_burns_code() {
        let uid = Uuid::new_v4();
        let code_id = Uuid::new_v4();
        let port = MockPort::default()
            .with_user(uid, false)
            .with_active_code(code_id, "hash", 4)
            .with_correct_code("123456")
            .with_increment_to(5); // reaches max_attempts

        let err = verify_email_with_code(&port, req("000000"), 5)
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
        assert_eq!(
            *port.consume_calls.lock().unwrap(),
            vec![code_id],
            "burned at cap"
        );
    }

    #[tokio::test]
    async fn already_at_cap_rejects_before_guessing() {
        let uid = Uuid::new_v4();
        let code_id = Uuid::new_v4();
        let port = MockPort::default()
            .with_user(uid, false)
            .with_active_code(code_id, "hash", 5)
            .with_correct_code("123456");

        let err = verify_email_with_code(&port, req("123456"), 5)
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::TooManyRequests(_)));
        assert_eq!(*port.consume_calls.lock().unwrap(), vec![code_id]);
        assert!(port.increment_calls.lock().unwrap().is_empty());
        assert!(port.verified_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_active_code_reports_expired() {
        let uid = Uuid::new_v4();
        let port = MockPort::default().with_user(uid, false); // no active code

        let err = verify_email_with_code(&port, req("123456"), 5)
            .await
            .unwrap_err();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("expired"), "got: {m}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn already_verified_is_idempotent() {
        let uid = Uuid::new_v4();
        let port = MockPort::default().with_user(uid, true);

        verify_email_with_code(&port, req("123456"), 5)
            .await
            .expect("idempotent ok");
        assert!(
            port.verified_calls.lock().unwrap().is_empty(),
            "no re-verify"
        );
    }

    #[tokio::test]
    async fn unknown_email_is_indistinguishable_from_bad_code() {
        let port = MockPort::default(); // user = None

        let err = verify_email_with_code(&port, req("123456"), 5)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn rejects_non_numeric_code() {
        let uid = Uuid::new_v4();
        let port = MockPort::default().with_user(uid, false);

        let err = verify_email_with_code(&port, req("12ab56"), 5)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn rejects_wrong_length_code() {
        let port = MockPort::default();
        let err = verify_email_with_code(&port, req("12345"), 5)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::ValidationFields { .. }));
    }

    // ---- resend_verification_code -----------------------------------------

    // count_codes_since is called in order: [hourly, total, cooldown].
    // `total <= 1` means only the registration code exists → first resend → free.

    #[tokio::test]
    async fn first_resend_is_free_even_with_recent_registration_code() {
        let uid = Uuid::new_v4();
        let port = MockPort::default()
            .with_user(uid, false)
            // hourly=1 (the registration code), total=1 → first resend, no cooldown check
            .with_counts(&[1, 1])
            .with_issued("654321");

        let out = resend_verification_code(&port, "alice@example.com", 15, 5)
            .await
            .expect("should succeed");

        let (got_uid, code) = out.expect("should issue a code");
        assert_eq!(got_uid, uid);
        assert_eq!(code, "654321");
        assert_eq!(*port.issue_calls.lock().unwrap(), vec![uid]);
    }

    #[tokio::test]
    async fn second_resend_within_cooldown_is_rejected() {
        let uid = Uuid::new_v4();
        let port = MockPort::default()
            .with_user(uid, false)
            // hourly=2 (register + 1 resend), total=2 (>1), cooldown=1 (recent code)
            .with_counts(&[2, 2, 1]);

        let err = resend_verification_code(&port, "alice@example.com", 15, 5)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::TooManyRequests(_)));
        assert!(port.issue_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn resend_allowed_after_cooldown_elapses() {
        let uid = Uuid::new_v4();
        let port = MockPort::default()
            .with_user(uid, false)
            // hourly=2, total=2 (>1), cooldown=0 (no code in the last 15s)
            .with_counts(&[2, 2, 0])
            .with_issued("111222");

        let out = resend_verification_code(&port, "alice@example.com", 15, 5)
            .await
            .expect("should succeed");
        assert_eq!(out.expect("should issue").1, "111222");
        assert_eq!(*port.issue_calls.lock().unwrap(), vec![uid]);
    }

    #[tokio::test]
    async fn resend_noop_for_unknown_email() {
        let port = MockPort::default(); // user = None
        let out = resend_verification_code(&port, "nobody@example.com", 15, 5)
            .await
            .expect("no error");
        assert!(out.is_none());
        assert!(port.issue_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn resend_noop_for_already_verified() {
        let uid = Uuid::new_v4();
        let port = MockPort::default().with_user(uid, true);
        let out = resend_verification_code(&port, "alice@example.com", 15, 5)
            .await
            .expect("no error");
        assert!(out.is_none());
        assert!(port.issue_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn resend_rejected_over_hourly_cap() {
        let uid = Uuid::new_v4();
        let port = MockPort::default().with_user(uid, false).with_counts(&[5]); // first check (hourly) hits the cap

        let err = resend_verification_code(&port, "alice@example.com", 15, 5)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::TooManyRequests(_)));
        assert!(port.issue_calls.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod change_password_tests {
    use super::*;
    use std::sync::Mutex;

    /// Mock `ChangePasswordPort`. `hash` is the stored Argon2 hash returned by
    /// `password_hash_for` (`None` = OAuth-only / unknown).
    #[derive(Default)]
    struct MockPort {
        hash: Option<String>,
        verified: bool,
        update_calls: Mutex<Vec<(Uuid, String)>>,
        revoke_calls: Mutex<Vec<Uuid>>,
    }

    impl MockPort {
        fn with_password(mut self, plaintext: &str) -> Self {
            self.hash = Some(hash_password(plaintext).expect("hash"));
            self.verified = true; // password users in these tests are verified
            self
        }
        fn oauth_only(mut self) -> Self {
            self.verified = true; // OAuth users are created already verified
            self // hash stays None
        }
        fn unverified(mut self) -> Self {
            self.verified = false;
            self
        }
    }

    #[async_trait::async_trait]
    impl ChangePasswordPort for MockPort {
        async fn email_verified(&self, _user_id: Uuid) -> Result<bool, AppError> {
            Ok(self.verified)
        }
        async fn password_hash_for(&self, _user_id: Uuid) -> Result<Option<String>, AppError> {
            Ok(self.hash.clone())
        }
        async fn update_password(&self, user_id: Uuid, phc: &str) -> Result<(), AppError> {
            self.update_calls
                .lock()
                .unwrap()
                .push((user_id, phc.into()));
            Ok(())
        }
        async fn revoke_sessions(&self, user_id: Uuid) -> Result<(), AppError> {
            self.revoke_calls.lock().unwrap().push(user_id);
            Ok(())
        }
    }

    fn req(current: &str, new: &str) -> ChangePasswordReq {
        ChangePasswordReq {
            current_password: current.into(),
            new_password: new.into(),
        }
    }

    #[tokio::test]
    async fn changes_password_and_revokes_sessions() {
        let uid = Uuid::new_v4();
        let port = MockPort::default().with_password("oldpassword");

        change_password_for_user(&port, uid, req("oldpassword", "newpassword123"))
            .await
            .expect("should succeed");

        let updates = port.update_calls.lock().unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, uid);
        assert!(updates[0].1.starts_with("$argon2"), "stores Argon2 hash");
        assert_ne!(updates[0].1, "newpassword123", "never stores raw password");

        assert_eq!(
            *port.revoke_calls.lock().unwrap(),
            vec![uid],
            "sessions revoked"
        );
    }

    #[tokio::test]
    async fn rejects_unverified_email() {
        let uid = Uuid::new_v4();
        let port = MockPort::default()
            .with_password("oldpassword")
            .unverified();

        let err = change_password_for_user(&port, uid, req("oldpassword", "newpassword123"))
            .await
            .unwrap_err();

        assert!(
            matches!(err, AppError::EmailNotVerified),
            "unverified user must not change password"
        );
        assert!(
            port.update_calls.lock().unwrap().is_empty(),
            "must not update"
        );
        assert!(
            port.revoke_calls.lock().unwrap().is_empty(),
            "must not revoke"
        );
    }

    #[tokio::test]
    async fn rejects_wrong_current_password() {
        let uid = Uuid::new_v4();
        let port = MockPort::default().with_password("oldpassword");

        let err = change_password_for_user(&port, uid, req("wrongpassword", "newpassword123"))
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::Auth(_)));
        assert!(
            port.update_calls.lock().unwrap().is_empty(),
            "must not update"
        );
        assert!(
            port.revoke_calls.lock().unwrap().is_empty(),
            "must not revoke"
        );
    }

    #[tokio::test]
    async fn rejects_oauth_only_account() {
        let uid = Uuid::new_v4();
        let port = MockPort::default().oauth_only();

        let err = change_password_for_user(&port, uid, req("anything", "newpassword123"))
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
        assert!(port.update_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_new_password_same_as_current() {
        let uid = Uuid::new_v4();
        let port = MockPort::default().with_password("samepassword1");

        let err = change_password_for_user(&port, uid, req("samepassword1", "samepassword1"))
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
        assert!(
            port.update_calls.lock().unwrap().is_empty(),
            "must not update"
        );
        assert!(port.revoke_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_short_new_password() {
        // Validation runs before any port call.
        let port = MockPort::default().with_password("oldpassword");

        let err = change_password_for_user(&port, Uuid::new_v4(), req("oldpassword", "short"))
            .await
            .unwrap_err();

        match err {
            AppError::ValidationFields { fields, .. } => {
                assert!(fields.contains_key("new_password"), "got {fields:?}");
            }
            other => panic!("expected ValidationFields, got {other:?}"),
        }
    }
}
