//! Access tokens: ES256-signed, OIDC-shaped, verifiable by anyone holding the JWKS.
//!
//! The same token authenticates a player to this API and to the SpacetimeDB realm. SpacetimeDB
//! verifies it by fetching `{iss}/.well-known/openid-configuration` and the JWKS it names, so
//! the token must carry a URL issuer, an audience, a `kid`, and be signed with a published
//! asymmetric key. No shared secret leaves this service.
//!
//! ```text
//! private key (PEM, this service only) ── signs ──► token { iss, sub, aud, exp, jti, role }
//! public key ── /.well-known/jwks.json ──► SpacetimeDB, other services
//! ```
//!
//! Two kinds of subject share the issuer: a player (`sub` = account UUID) and the realm
//! bridge (`sub` = `service:realm-bridge`). [`JwtCodec::verify`] only ever accepts the first,
//! so a service token cannot call player endpoints.

use base64::Engine as _;
use generic_auth_core::{models::Role, AppError};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct JwtConfig {
    /// The token issuer: this service's public base URL (no trailing slash).
    pub issuer: String,
    /// The audience every token carries and every verifier checks.
    pub audience: String,
    /// The key id published in the JWKS and stamped on every token.
    pub key_id: String,
    /// The ES256 (P-256) private key, PKCS#8 PEM.
    pub private_key_pem: String,
    pub access_ttl_min: i64,
    pub refresh_ttl_days: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub role: String,
    pub iss: String,
    pub aud: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: Uuid,
}

/// A verified player token.
#[derive(Debug, Clone)]
pub struct PlayerClaims {
    pub user_id: Uuid,
    pub role: String,
}

pub struct JwtCodec {
    enc: EncodingKey,
    dec: DecodingKey,
    cfg: JwtConfig,
    x: String,
    y: String,
}

impl std::fmt::Debug for JwtCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtCodec")
            .field("issuer", &self.cfg.issuer)
            .field("key_id", &self.cfg.key_id)
            .finish_non_exhaustive()
    }
}

/// A fixed P-256 key for unit tests in this workspace. It signs nothing outside tests and must
/// never be configured as a deployment's key.
#[doc(hidden)]
pub const TEST_SIGNING_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgzGQyqwJuA5eQQZN6
AMgC7nVaQxX8KjqdIGfvnQrrA2WhRANCAATbCZ49UG0KV6T83kt2NmdhDQCX9xot
wOOrsw06Oz+XCAodt5tmNYhPlDAbvDLqXz27I6J/6oCJyJOmBKHiAd+p
-----END PRIVATE KEY-----
";

/// A codec over [`TEST_SIGNING_KEY_PEM`], for unit tests.
#[doc(hidden)]
pub fn test_codec() -> JwtCodec {
    match JwtCodec::new(JwtConfig {
        issuer: "http://test.invalid".into(),
        audience: "mygame-realm".into(),
        key_id: "test".into(),
        private_key_pem: TEST_SIGNING_KEY_PEM.into(),
        access_ttl_min: 60,
        refresh_ttl_days: 14,
    }) {
        Ok(codec) => codec,
        Err(e) => panic!("test key: {e}"),
    }
}

/// A fresh P-256 private key as PKCS#8 PEM. Used to bootstrap a development key file; a
/// deployment generates its key once (see SPACETIMEDB_DEPLOYMENT.md) and keeps it.
pub fn generate_signing_key_pem() -> Result<String, AppError> {
    let secret = p256::SecretKey::random(&mut rand::rngs::OsRng);
    secret
        .to_pkcs8_pem(LineEnding::LF)
        .map(|pem| pem.to_string())
        .map_err(|e| AppError::internal(format!("encode signing key: {e}")))
}

impl JwtCodec {
    pub fn new(cfg: JwtConfig) -> Result<Self, AppError> {
        let secret = p256::SecretKey::from_pkcs8_pem(&cfg.private_key_pem)
            .map_err(|e| AppError::internal(format!("signing key is not a P-256 PKCS#8 PEM: {e}")))?;
        let point = p256::EncodedPoint::from(secret.public_key());
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let x = point
            .x()
            .map(|x| b64.encode(x))
            .ok_or_else(|| AppError::internal("signing key has no x coordinate"))?;
        let y = point
            .y()
            .map(|y| b64.encode(y))
            .ok_or_else(|| AppError::internal("signing key has no y coordinate"))?;
        let enc = EncodingKey::from_ec_pem(cfg.private_key_pem.as_bytes())
            .map_err(|e| AppError::internal(format!("signing key: {e}")))?;
        let dec = DecodingKey::from_ec_components(&x, &y)
            .map_err(|e| AppError::internal(format!("verification key: {e}")))?;
        Ok(Self { enc, dec, cfg, x, y })
    }

    pub fn issuer(&self) -> &str {
        &self.cfg.issuer
    }

    pub fn audience(&self) -> &str {
        &self.cfg.audience
    }

    fn sign(&self, sub: String, role: &str, ttl_seconds: i64) -> Result<String, AppError> {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let claims = Claims {
            sub,
            role: role.to_string(),
            iss: self.cfg.issuer.clone(),
            aud: self.cfg.audience.clone(),
            iat: now,
            exp: now + ttl_seconds,
            jti: Uuid::new_v4(),
        };
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.cfg.key_id.clone());
        encode(&header, &claims, &self.enc).map_err(|e| AppError::Auth(format!("encode jwt: {e}")))
    }

    /// A player's access token.
    pub fn issue(&self, user_id: Uuid, role: Role) -> Result<String, AppError> {
        self.sign(user_id.to_string(), role.as_str(), self.cfg.access_ttl_min * 60)
    }

    /// A token for an internal service identity (the realm bridge). Short-lived; the bridge
    /// asks for a new one before every batch.
    pub fn issue_service(&self, subject: &str) -> Result<String, AppError> {
        self.sign(subject.to_string(), "service", 10 * 60)
    }

    /// Verifies a token and returns its claims, whatever the subject.
    pub fn verify_any(&self, token: &str) -> Result<Claims, AppError> {
        let mut v = Validation::new(Algorithm::ES256);
        v.set_issuer(&[&self.cfg.issuer]);
        v.set_audience(&[&self.cfg.audience]);
        // We deliberately don't surface JWT internals to callers.
        decode::<Claims>(token, &self.dec, &v)
            .map(|d| d.claims)
            .map_err(|_| AppError::Unauthorized)
    }

    /// Verifies a *player* token. Service tokens are refused.
    pub fn verify(&self, token: &str) -> Result<PlayerClaims, AppError> {
        let claims = self.verify_any(token)?;
        let user_id = Uuid::parse_str(&claims.sub).map_err(|_| AppError::Unauthorized)?;
        Ok(PlayerClaims { user_id, role: claims.role })
    }

    /// The JWKS document: the one public key, by id.
    pub fn jwks(&self) -> serde_json::Value {
        serde_json::json!({
            "keys": [{
                "kty": "EC",
                "crv": "P-256",
                "x": self.x,
                "y": self.y,
                "kid": self.cfg.key_id,
                "alg": "ES256",
                "use": "sig",
            }]
        })
    }

    /// The OIDC discovery document.
    pub fn discovery(&self) -> serde_json::Value {
        serde_json::json!({
            "issuer": self.cfg.issuer,
            "jwks_uri": format!("{}/.well-known/jwks.json", self.cfg.issuer),
            "id_token_signing_alg_values_supported": ["ES256"],
            "subject_types_supported": ["public"],
            "response_types_supported": ["id_token"],
            "token_endpoint": format!("{}/api/v1/auth/login", self.cfg.issuer),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec() -> JwtCodec {
        JwtCodec::new(JwtConfig {
            issuer: "http://127.0.0.1:8080".into(),
            audience: "mygame-realm".into(),
            key_id: "test".into(),
            private_key_pem: generate_signing_key_pem().unwrap(),
            access_ttl_min: 30,
            refresh_ttl_days: 14,
        })
        .unwrap()
    }

    #[test]
    fn player_tokens_round_trip_with_issuer_audience_and_kid() {
        let codec = codec();
        let id = Uuid::new_v4();
        let token = codec.issue(id, Role::User).unwrap();
        let header = jsonwebtoken::decode_header(&token).unwrap();
        assert_eq!(header.alg, Algorithm::ES256);
        assert_eq!(header.kid.as_deref(), Some("test"));
        let claims = codec.verify_any(&token).unwrap();
        assert_eq!(claims.aud, "mygame-realm");
        assert_eq!(claims.iss, "http://127.0.0.1:8080");
        assert_eq!(codec.verify(&token).unwrap().user_id, id);
    }

    #[test]
    fn service_tokens_cannot_act_as_players() {
        let codec = codec();
        let token = codec.issue_service("service:realm-bridge").unwrap();
        assert!(codec.verify_any(&token).is_ok());
        assert!(codec.verify(&token).is_err());
    }

    #[test]
    fn tokens_from_another_key_are_refused() {
        let ours = codec();
        let theirs = codec();
        let forged = theirs.issue(Uuid::new_v4(), Role::Admin).unwrap();
        assert!(ours.verify(&forged).is_err());
    }

    #[test]
    fn tampered_tokens_are_refused() {
        let codec = codec();
        let token = codec.issue(Uuid::new_v4(), Role::User).unwrap();
        let mut parts: Vec<String> = token.split('.').map(str::to_owned).collect();
        parts[1] = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"sub":"00000000-0000-0000-0000-000000000000","role":"admin"}"#);
        assert!(codec.verify(&parts.join(".")).is_err());
    }

    #[test]
    fn expired_tokens_are_refused() {
        let codec = JwtCodec::new(JwtConfig {
            access_ttl_min: -10,
            ..codec().cfg
        })
        .unwrap();
        let token = codec.issue(Uuid::new_v4(), Role::User).unwrap();
        assert!(codec.verify(&token).is_err());
    }

    #[test]
    fn jwks_publishes_the_verification_key() {
        let codec = codec();
        let jwks = codec.jwks();
        let key = &jwks["keys"][0];
        assert_eq!(key["kty"], "EC");
        assert_eq!(key["kid"], "test");
        let dec = DecodingKey::from_ec_components(
            key["x"].as_str().unwrap(),
            key["y"].as_str().unwrap(),
        )
        .unwrap();
        let token = codec.issue(Uuid::new_v4(), Role::User).unwrap();
        let mut v = Validation::new(Algorithm::ES256);
        v.set_audience(&["mygame-realm"]);
        assert!(decode::<Claims>(&token, &dec, &v).is_ok());
        assert_eq!(
            codec.discovery()["jwks_uri"],
            "http://127.0.0.1:8080/.well-known/jwks.json"
        );
    }
}
