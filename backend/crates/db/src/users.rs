//! User and identity queries.
//!
//! Notes:
//!  * Passwords are stored only as hashes (Argon2 — see `auth` crate).
//!  * OAuth users may have a NULL password_hash.
//!  * Permission resolution is `role_permissions ∪ user_permissions(granted)`
//!    minus `user_permissions(revoked)`.

use generic_auth_core::models::{Role, User};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::DbResult;

#[derive(Debug, Clone)]
pub struct NewUser<'a> {
    pub email: Option<&'a str>,
    pub password_hash: Option<&'a str>,
    pub display_name: Option<&'a str>,
    pub role: Role,
    pub email_verified: bool,
}

pub async fn create_user(pool: &PgPool, new: NewUser<'_>) -> DbResult<User> {
    let role_id = role_id_for(pool, new.role).await?;
    let row = sqlx::query!(
        r#"
        INSERT INTO users (email, password_hash, display_name, role_id, email_verified)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, email::text AS "email", display_name, role_id,
                  is_active, email_verified, created_at, updated_at
        "#,
        new.email,
        new.password_hash,
        new.display_name,
        role_id,
        new.email_verified,
    )
    .fetch_one(pool)
    .await?;

    Ok(User {
        id: row.id,
        email: row.email,
        display_name: row.display_name,
        role: new.role,
        is_active: row.is_active,
        email_verified: row.email_verified,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

pub async fn find_by_email(pool: &PgPool, email: &str) -> DbResult<Option<UserWithSecret>> {
    let row = sqlx::query!(
        r#"
        SELECT u.id, u.email::text AS "email", u.password_hash, u.display_name,
               r.name AS role_name,
               u.is_active, u.email_verified, u.created_at, u.updated_at
        FROM users u JOIN roles r ON r.id = u.role_id
        WHERE u.email = $1
        "#,
        email,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| UserWithSecret {
        user: User {
            id: r.id,
            email: r.email,
            display_name: r.display_name,
            role: Role::parse(&r.role_name).unwrap_or(Role::User),
            is_active: r.is_active,
            email_verified: r.email_verified,
            created_at: r.created_at,
            updated_at: r.updated_at,
        },
        password_hash: r.password_hash,
    }))
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> DbResult<Option<User>> {
    let row = sqlx::query!(
        r#"
        SELECT u.id, u.email::text AS "email", u.display_name, r.name AS role_name,
               u.is_active, u.email_verified, u.created_at, u.updated_at
        FROM users u JOIN roles r ON r.id = u.role_id
        WHERE u.id = $1
        "#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| User {
        id: r.id,
        email: r.email,
        display_name: r.display_name,
        role: Role::parse(&r.role_name).unwrap_or(Role::User),
        is_active: r.is_active,
        email_verified: r.email_verified,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

pub async fn list_users(pool: &PgPool, limit: i64, offset: i64) -> DbResult<Vec<User>> {
    let rows = sqlx::query!(
        r#"
        SELECT u.id, u.email::text AS "email", u.display_name, r.name AS role_name,
               u.is_active, u.email_verified, u.created_at, u.updated_at
        FROM users u JOIN roles r ON r.id = u.role_id
        ORDER BY u.created_at DESC
        LIMIT $1 OFFSET $2
        "#,
        limit, offset,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| User {
        id: r.id,
        email: r.email,
        display_name: r.display_name,
        role: Role::parse(&r.role_name).unwrap_or(Role::User),
        is_active: r.is_active,
        email_verified: r.email_verified,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }).collect())
}

pub async fn set_role(pool: &PgPool, user_id: Uuid, role: Role) -> DbResult<()> {
    let role_id = role_id_for(pool, role).await?;
    sqlx::query!("UPDATE users SET role_id = $1, updated_at = now() WHERE id = $2",
        role_id, user_id)
        .execute(pool).await?;
    Ok(())
}

pub async fn set_active(pool: &PgPool, user_id: Uuid, active: bool) -> DbResult<()> {
    sqlx::query!("UPDATE users SET is_active = $1, updated_at = now() WHERE id = $2",
        active, user_id)
        .execute(pool).await?;
    Ok(())
}

/// Permanently delete a user. Dependent rows (oauth identities, tokens,
/// user_permissions) cascade via `ON DELETE CASCADE`.
/// Returns `true` if a row was deleted, `false` if no such user existed.
pub async fn delete_user(pool: &PgPool, user_id: Uuid) -> DbResult<bool> {
    let res = sqlx::query!("DELETE FROM users WHERE id = $1", user_id)
        .execute(pool).await?;
    Ok(res.rows_affected() > 0)
}

pub async fn permissions_for(pool: &PgPool, user_id: Uuid) -> DbResult<Vec<String>> {
    let rows = sqlx::query!(
        r#"
        WITH role_perms AS (
            SELECT p.name
            FROM users u
            JOIN role_permissions rp ON rp.role_id = u.role_id
            JOIN permissions p ON p.id = rp.permission_id
            WHERE u.id = $1
        ),
        granted AS (
            SELECT p.name
            FROM user_permissions up
            JOIN permissions p ON p.id = up.permission_id
            WHERE up.user_id = $1 AND up.granted = TRUE
        ),
        revoked AS (
            SELECT p.name
            FROM user_permissions up
            JOIN permissions p ON p.id = up.permission_id
            WHERE up.user_id = $1 AND up.granted = FALSE
        )
        SELECT name FROM (SELECT * FROM role_perms UNION SELECT * FROM granted) u
        WHERE name NOT IN (SELECT name FROM revoked)
        "#,
        user_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().filter_map(|r| r.name).collect())
}

pub async fn upsert_oauth_identity(
    pool: &PgPool,
    user_id: Uuid,
    provider: &str,
    subject: &str,
    email: Option<&str>,
    raw_profile: &serde_json::Value,
) -> DbResult<()> {
    sqlx::query!(
        r#"
        INSERT INTO oauth_identities (user_id, provider, subject, email, raw_profile)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (provider, subject) DO UPDATE
            SET email = EXCLUDED.email,
                raw_profile = EXCLUDED.raw_profile
        "#,
        user_id, provider, subject, email, raw_profile,
    ).execute(pool).await?;
    Ok(())
}

pub async fn find_user_by_oauth(
    pool: &PgPool, provider: &str, subject: &str,
) -> DbResult<Option<User>> {
    let row = sqlx::query!(
        r#"
        SELECT u.id, u.email::text AS "email", u.display_name, r.name AS role_name,
               u.is_active, u.email_verified, u.created_at, u.updated_at
        FROM oauth_identities oi
        JOIN users u ON u.id = oi.user_id
        JOIN roles r ON r.id = u.role_id
        WHERE oi.provider = $1 AND oi.subject = $2
        "#,
        provider, subject,
    ).fetch_optional(pool).await?;
    Ok(row.map(|r| User {
        id: r.id,
        email: r.email,
        display_name: r.display_name,
        role: Role::parse(&r.role_name).unwrap_or(Role::User),
        is_active: r.is_active,
        email_verified: r.email_verified,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

// ---- refresh tokens -------------------------------------------------------

pub async fn store_refresh_token(
    pool: &PgPool,
    user_id: Uuid,
    token_hash: &str,
    expires_at: OffsetDateTime,
    user_agent: Option<&str>,
) -> DbResult<Uuid> {
    let id = sqlx::query_scalar!(
        "INSERT INTO refresh_tokens (user_id, token_hash, expires_at, user_agent)
         VALUES ($1, $2, $3, $4) RETURNING id",
        user_id, token_hash, expires_at, user_agent,
    ).fetch_one(pool).await?;
    Ok(id)
}

pub async fn revoke_refresh_token(pool: &PgPool, token_hash: &str) -> DbResult<()> {
    sqlx::query!(
        "UPDATE refresh_tokens SET revoked_at = now() WHERE token_hash = $1",
        token_hash,
    ).execute(pool).await?;
    Ok(())
}

pub async fn lookup_refresh_token(
    pool: &PgPool, token_hash: &str,
) -> DbResult<Option<(Uuid, OffsetDateTime, Option<OffsetDateTime>)>> {
    let row = sqlx::query!(
        "SELECT user_id, expires_at, revoked_at FROM refresh_tokens WHERE token_hash = $1",
        token_hash,
    ).fetch_optional(pool).await?;
    Ok(row.map(|r| (r.user_id, r.expires_at, r.revoked_at)))
}

// ---- helpers -------------------------------------------------------------

pub struct UserWithSecret {
    pub user: User,
    pub password_hash: Option<String>,
}

// ---- email verification codes ---------------------------------------------
//
// `email_verification_tokens.token_hash` holds the HMAC of a 6-digit code.
// Codes are looked up by `user_id` (not unique) and capped by `attempt_count`.

/// One active verification code per user: mark any prior unused codes as used.
pub async fn invalidate_active_email_codes(pool: &PgPool, user_id: Uuid) -> DbResult<()> {
    sqlx::query(
        "UPDATE email_verification_tokens SET used_at = now()
         WHERE user_id = $1 AND used_at IS NULL",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn store_email_verification_code(
    pool: &PgPool,
    user_id: Uuid,
    code_hash: &str,
    expires_at: OffsetDateTime,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO email_verification_tokens (user_id, token_hash, expires_at)
         VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(code_hash)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Number of codes created for a user since `since` — backs resend rate limiting.
pub async fn count_recent_email_codes(
    pool: &PgPool,
    user_id: Uuid,
    since: OffsetDateTime,
) -> DbResult<i64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM email_verification_tokens
         WHERE user_id = $1 AND created_at > $2",
    )
    .bind(user_id)
    .bind(since)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// The latest unused, unexpired code for a user: `(id, code_hash, attempt_count)`.
pub async fn find_active_email_code(
    pool: &PgPool,
    user_id: Uuid,
) -> DbResult<Option<(Uuid, String, i32)>> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT id, token_hash, attempt_count
         FROM email_verification_tokens
         WHERE user_id = $1 AND used_at IS NULL AND expires_at > now()
         ORDER BY created_at DESC
         LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| (r.get("id"), r.get("token_hash"), r.get("attempt_count"))))
}

/// Record a failed guess; returns the new attempt count.
pub async fn increment_email_code_attempt(pool: &PgPool, id: Uuid) -> DbResult<i32> {
    let count: i32 = sqlx::query_scalar(
        "UPDATE email_verification_tokens SET attempt_count = attempt_count + 1
         WHERE id = $1 RETURNING attempt_count",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Invalidate a code by id (after a successful verification or attempt-cap hit).
pub async fn consume_email_code(pool: &PgPool, id: Uuid) -> DbResult<()> {
    sqlx::query("UPDATE email_verification_tokens SET used_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn mark_email_verified(pool: &PgPool, user_id: Uuid) -> DbResult<()> {
    sqlx::query("UPDATE users SET email_verified = true, updated_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// The user's Argon2 password hash, or `None` if the user is unknown or has no
/// password set (OAuth-only account).
pub async fn password_hash_by_id(pool: &PgPool, user_id: Uuid) -> DbResult<Option<String>> {
    let hash: Option<Option<String>> =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(hash.flatten())
}

// ---- password reset -------------------------------------------------------

pub async fn store_password_reset_token(
    pool: &PgPool,
    user_id: Uuid,
    token_hash: &str,
    expires_at: OffsetDateTime,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO password_reset_tokens (user_id, token_hash, expires_at)
         VALUES ($1, $2, $3)
         ON CONFLICT (token_hash) DO NOTHING",
    )
    .bind(user_id)
    .bind(token_hash)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Consume a reset token and return the associated `user_id`.
/// Returns `None` if the token is unknown, already used, or expired.
pub async fn consume_password_reset_token(
    pool: &PgPool,
    token_hash: &str,
) -> DbResult<Option<Uuid>> {
    let user_id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE password_reset_tokens
         SET used_at = now()
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()
         RETURNING user_id",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    Ok(user_id)
}

pub async fn update_password_hash(
    pool: &PgPool,
    user_id: Uuid,
    password_hash: &str,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE users SET password_hash = $1, updated_at = now() WHERE id = $2",
    )
    .bind(password_hash)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn revoke_all_refresh_tokens(pool: &PgPool, user_id: Uuid) -> DbResult<()> {
    sqlx::query(
        "UPDATE refresh_tokens SET revoked_at = now()
         WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ---- helpers -------------------------------------------------------------

async fn role_id_for(pool: &PgPool, role: Role) -> DbResult<i16> {
    let id = sqlx::query_scalar!("SELECT id FROM roles WHERE name = $1", role.as_str())
        .fetch_one(pool).await?;
    Ok(id)
}
