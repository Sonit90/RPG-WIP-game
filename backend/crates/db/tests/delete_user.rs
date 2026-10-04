//! Integration test: `delete_user` removes a user and reports existence via
//! its boolean return, so the API layer can map a missing user to 404.
//!
//! Requires a live Postgres reachable via `DATABASE_URL` with the migrations
//! already applied. Marked `#[ignore]` so default `cargo test` runs stay
//! hermetic. Run explicitly:
//!     cargo test -p generic-auth-db -- --ignored

use generic_auth_core::models::Role;
use generic_auth_db::{
    connect,
    users::{create_user, delete_user, find_by_id, NewUser},
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
    format!("del-{}@test.local", uuid::Uuid::new_v4())
}

#[tokio::test]
#[ignore]
async fn delete_user_removes_row_and_reports_existence() {
    let pool = pool().await;

    let user = create_user(
        &pool,
        NewUser {
            email: Some(&unique_email()),
            password_hash: Some("$argon2id$fake"),
            display_name: None,
            role: Role::User,
            email_verified: false,
        },
    )
    .await
    .expect("create user");

    // First delete hits an existing row.
    let deleted = delete_user(&pool, user.id).await.expect("delete");
    assert!(deleted, "deleting an existing user must return true");

    // Row is actually gone.
    let found = find_by_id(&pool, user.id).await.expect("find_by_id");
    assert!(found.is_none(), "user must not exist after deletion");

    // Second delete finds nothing — drives the API's 404 path.
    let deleted_again = delete_user(&pool, user.id).await.expect("delete again");
    assert!(
        !deleted_again,
        "deleting a missing user must return false"
    );
}
