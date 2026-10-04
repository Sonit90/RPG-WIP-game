use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, PasswordVerifier, SaltString},
    Argon2, PasswordHash,
};
use generic_auth_core::AppError;

/// Hash a password with Argon2id and a random per-password salt.
pub fn hash_password(plain: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon = Argon2::default();
    argon
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Auth(format!("hash: {e}")))
}

/// Verify `plain` against the encoded `phc` hash.
pub fn verify_password(plain: &str, phc: &str) -> Result<bool, AppError> {
    let parsed = PasswordHash::new(phc).map_err(|e| AppError::Auth(format!("parse hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok())
}
