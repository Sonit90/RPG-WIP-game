//! Authentication & authorization.
//!
//!  * Email + password (Argon2 hashing)
//!  * Google OAuth2 (auth code flow)
//!  * JWT access tokens + opaque refresh tokens (stored hashed)
//!  * Permission checks against `users.role` and `user_permissions`

pub mod jwt;
pub mod oauth;
pub mod password;
pub mod permissions;
pub mod tokens;

pub use jwt::{generate_signing_key_pem, Claims, JwtCodec, JwtConfig, PlayerClaims};
pub use password::{hash_password, verify_password};
pub use permissions::Permission;
pub use tokens::{
    generate_email_code, generate_refresh_token, hash_email_code, hash_refresh_token,
    verify_email_code,
};
