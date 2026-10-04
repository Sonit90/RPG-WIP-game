use generic_auth_auth::hash_password;
use generic_auth_core::{models::User, AppError};
use validator::Validate;

#[derive(Debug, Validate)]
pub struct CreateAdminArgs {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 8))]
    pub password: String,
}

#[async_trait::async_trait]
pub trait CreateAdminPort: Send + Sync {
    async fn create_user(&self, email: &str, password_hash: &str) -> Result<User, AppError>;
}

pub async fn create_admin_user(
    port: &dyn CreateAdminPort,
    args: CreateAdminArgs,
) -> Result<User, AppError> {
    args.validate().map_err(AppError::from_validation_errors)?;
    let hash = hash_password(&args.password)?;
    port.create_user(&args.email, &hash).await
}

// ---------------------------------------------------------------------------
// Real DB implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl CreateAdminPort for sqlx::PgPool {
    async fn create_user(&self, email: &str, password_hash: &str) -> Result<User, AppError> {
        generic_auth_db::users::create_user(self, generic_auth_db::users::NewUser {
            email: Some(email),
            password_hash: Some(password_hash),
            display_name: None,
            role: generic_auth_core::models::Role::Admin,
            email_verified: true,
        }).await.map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use generic_auth_core::models::Role;
    use std::sync::Mutex;
    use time::OffsetDateTime;
    use uuid::Uuid;

    #[derive(Default)]
    struct MockPort {
        response: Mutex<Option<Result<User, AppError>>>,
        calls: Mutex<Vec<(String, String)>>,
    }

    impl MockPort {
        fn returns(self, result: Result<User, AppError>) -> Self {
            *self.response.lock().unwrap() = Some(result);
            self
        }
    }

    #[async_trait::async_trait]
    impl CreateAdminPort for MockPort {
        async fn create_user(&self, email: &str, password_hash: &str) -> Result<User, AppError> {
            self.calls.lock().unwrap().push((email.into(), password_hash.into()));
            self.response.lock().unwrap().take().expect("response not set")
        }
    }

    fn sample_user(email: &str) -> User {
        let now = OffsetDateTime::now_utc();
        User {
            id: Uuid::new_v4(),
            email: Some(email.into()),
            display_name: None,
            role: Role::Admin,
            is_active: true,
            email_verified: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn args(email: &str, password: &str) -> CreateAdminArgs {
        CreateAdminArgs { email: email.into(), password: password.into() }
    }

    // ---- happy path -------------------------------------------------------

    #[tokio::test]
    async fn creates_admin_with_hashed_password() {
        let user = sample_user("admin@example.com");
        let port = MockPort::default().returns(Ok(user.clone()));

        let result = create_admin_user(&port, args("admin@example.com", "securepass"))
            .await
            .expect("should succeed");

        assert_eq!(result.id, user.id);
        assert_eq!(result.role, Role::Admin);

        let calls = port.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "admin@example.com");
        // password must be hashed, not raw
        assert!(calls[0].1.starts_with("$argon2"), "expected Argon2 hash, got: {}", calls[0].1);
        assert_ne!(calls[0].1, "securepass");
    }

    // ---- email validation -------------------------------------------------

    #[tokio::test]
    async fn rejects_invalid_email() {
        let port = MockPort::default();

        let err = create_admin_user(&port, args("not-an-email", "securepass"))
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::ValidationFields { ref fields, .. } if fields.contains_key("email")));
        assert!(port.calls.lock().unwrap().is_empty(), "DB must not be called for invalid input");
    }

    #[tokio::test]
    async fn rejects_empty_email() {
        let port = MockPort::default();

        let err = create_admin_user(&port, args("", "securepass"))
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::ValidationFields { .. }));
        assert!(port.calls.lock().unwrap().is_empty());
    }

    // ---- password validation ----------------------------------------------

    #[tokio::test]
    async fn rejects_password_shorter_than_8_chars() {
        let port = MockPort::default();

        let err = create_admin_user(&port, args("admin@example.com", "short"))
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::ValidationFields { ref fields, .. } if fields.contains_key("password")));
        assert!(port.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn accepts_password_exactly_8_chars() {
        let user = sample_user("admin@example.com");
        let port = MockPort::default().returns(Ok(user));

        create_admin_user(&port, args("admin@example.com", "exactly8"))
            .await
            .expect("8-char password should be valid");

        assert_eq!(port.calls.lock().unwrap().len(), 1);
    }

    // ---- DB error propagation --------------------------------------------

    #[tokio::test]
    async fn propagates_db_conflict() {
        let port = MockPort::default().returns(Err(AppError::Conflict("email taken".into())));

        let err = create_admin_user(&port, args("admin@example.com", "securepass"))
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::Conflict(_)));
    }

    #[tokio::test]
    async fn propagates_db_internal_error() {
        let port = MockPort::default().returns(Err(AppError::Database("connection lost".into())));

        let err = create_admin_user(&port, args("admin@example.com", "securepass"))
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::Database(_)));
    }
}
