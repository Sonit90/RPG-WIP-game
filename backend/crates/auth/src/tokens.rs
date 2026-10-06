use hmac::{Hmac, Mac};
use rand::{Rng, RngCore};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Generate a 256-bit cryptographically random refresh token, URL-safe base64.
pub fn generate_refresh_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    // No external base64 dep — use hex which is fine for an opaque token.
    hex::encode(bytes)
}

/// SHA-256 hash of the token, hex-encoded — never store the raw token.
pub fn hash_refresh_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

/// Generate a zero-padded 6-digit email verification code (`"000000"`..=`"999999"`)
/// using a cryptographically secure RNG with a uniform distribution.
pub fn generate_email_code() -> String {
    let n: u32 = rand::thread_rng().gen_range(0..1_000_000);
    format!("{n:06}")
}

/// HMAC-SHA256 of a verification code, keyed with the server secret, hex-encoded.
///
/// The keyed MAC (rather than a bare hash) is what makes a leaked database
/// safe: the code space is only 1,000,000, so an unkeyed hash would be trivial
/// to brute-force offline. Without the secret an attacker cannot precompute or
/// reverse the stored value.
pub fn hash_email_code(secret: &str, code: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(code.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Constant-time check that `code` matches the stored hex-encoded HMAC.
///
/// Returns `false` on any mismatch, including a malformed `stored_hex`.
pub fn verify_email_code(secret: &str, code: &str, stored_hex: &str) -> bool {
    let Ok(expected) = hex::decode(stored_hex) else {
        return false;
    };
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(code.as_bytes());
    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_code_is_six_digits() {
        for _ in 0..1_000 {
            let code = generate_email_code();
            assert_eq!(code.len(), 6, "code must be 6 chars: {code}");
            assert!(
                code.chars().all(|c| c.is_ascii_digit()),
                "non-digit in {code}"
            );
        }
    }

    #[test]
    fn hash_is_not_the_raw_code() {
        let h = hash_email_code("server-secret", "123456");
        assert_ne!(h, "123456");
        assert_eq!(h.len(), 64, "hex of 32-byte HMAC");
    }

    #[test]
    fn verify_accepts_matching_code_and_secret() {
        let h = hash_email_code("server-secret", "042042");
        assert!(verify_email_code("server-secret", "042042", &h));
    }

    #[test]
    fn verify_rejects_wrong_code() {
        let h = hash_email_code("server-secret", "042042");
        assert!(!verify_email_code("server-secret", "042043", &h));
    }

    #[test]
    fn verify_rejects_wrong_secret() {
        let h = hash_email_code("server-secret", "042042");
        assert!(!verify_email_code("other-secret", "042042", &h));
    }

    #[test]
    fn verify_rejects_malformed_stored_hash() {
        assert!(!verify_email_code("server-secret", "042042", "not-hex"));
    }
}
