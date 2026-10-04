//! Integration test: inserting a user with a duplicate email must surface as
//! `AppError::Conflict`, never `AppError::Database` (which the API maps to 500).
//!
//! Requires a live Postgres reachable via `DATABASE_URL` with the migrations
//! already applied. Marked `#[ignore]` so default `cargo test` runs stay
//! hermetic. Run explicitly:
//!     cargo test -p generic-auth-db -- --ignored

use generic_auth_core::{models::Role, AppError};
use generic_auth_db::{
    connect,
    users::{create_user, NewUser},
    DbConfig,
};

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for integration tests");
    connect(&DbConfig {
        url,
        max_connections: 2,
        min_connections: 1,
        acquire_timeout_secs: 5,
    })
    .await
    .expect("connect")
}

fn unique_email() -> String {
    format!("dup-{}@test.local", uuid::Uuid::new_v4())
}

#[tokio::test]
#[ignore]
async fn duplicate_email_returns_conflict_not_database() {
    let pool = pool().await;
    let email = unique_email();

    fn new_user(email: &str) -> NewUser<'_> {
        NewUser {
            email: Some(email),
            password_hash: Some("$argon2id$fake"),
            display_name: None,
            role: Role::User,
            email_verified: false,
        }
    }

    // First insert succeeds.
    let first = create_user(&pool, new_user(&email)).await;
    assert!(first.is_ok(), "first insert must succeed, got {first:?}");
    let user_id = first.unwrap().id;

    // Second insert with same email must conflict.
    let second = create_user(&pool, new_user(&email)).await;
    let err = second.expect_err("duplicate insert must error");

    let app_err: AppError = err.into();
    match app_err {
        AppError::Conflict(msg) => {
            assert!(
                msg.contains("email"),
                "conflict message should mention email, got {msg:?}"
            );
        }
        other => panic!("expected Conflict, got {other:?}"),
    }

    // Cleanup so the test is idempotent across runs.
    // Using untyped `query` to avoid adding another entry to `.sqlx/` offline cache.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .ok();
}
