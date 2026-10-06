use std::sync::Arc;

use generic_auth_auth::JwtCodec;
use generic_auth_core::{models::User, AppError};
use generic_auth_db::{connect, run_migrations, users as user_db, DbConfig};
use sqlx::PgPool;
use uuid::Uuid;

use std::collections::HashSet;

use crate::config::Settings;
use crate::middleware::auth::{AdminUserStore, FindUserById, HasJwt};

#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub db: PgPool,
    pub jwt: Arc<JwtCodec>,
    pub http: reqwest::Client,
    /// Login, registration and refresh throttles.
    pub limits: Arc<crate::rate_limit::Limits>,
    /// The bridge to the SpacetimeDB realm.
    pub realm: realm_game::bridge::RealmBridge,
}

/// The access-token signing key: inline PEM, else the configured file. Development creates
/// the file on first start; anywhere else a missing key stops the service.
fn load_signing_key(settings: &Settings) -> anyhow::Result<String> {
    if let Some(pem) = settings
        .auth
        .jwt_private_key_pem
        .as_ref()
        .filter(|p| !p.trim().is_empty())
    {
        // An env var cannot hold newlines everywhere: accept a PEM written with literal `\n`.
        return Ok(pem.replace("\\n", "\n"));
    }
    let path = settings.auth.jwt_private_key_path.clone().ok_or_else(|| {
        anyhow::anyhow!("no signing key: set APP__AUTH__JWT_PRIVATE_KEY_PEM or _PATH")
    })?;
    match std::fs::read_to_string(&path) {
        Ok(pem) => Ok(pem),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && settings.app_env == "development" => {
            let pem = generic_auth_auth::generate_signing_key_pem()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if let Some(dir) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, &pem)?;
            tracing::warn!(path = %path, "created a development signing key");
            Ok(pem)
        }
        Err(e) => Err(anyhow::anyhow!("signing key {path}: {e}")),
    }
}

impl AppState {
    pub async fn build(settings: Arc<Settings>) -> anyhow::Result<Self> {
        let db = connect(&DbConfig {
            url: settings.db.url.clone(),
            max_connections: settings.db.max_connections,
            min_connections: settings.db.min_connections,
            acquire_timeout_secs: settings.db.acquire_timeout_secs,
        })
        .await?;

        if settings.db.run_migrations_on_start {
            run_migrations(&db).await?;
        }

        let jwt = JwtCodec::new(generic_auth_auth::JwtConfig {
            issuer: settings.auth.jwt_issuer.clone(),
            audience: settings.auth.jwt_audience.clone(),
            key_id: settings.auth.jwt_key_id.clone(),
            private_key_pem: load_signing_key(&settings)?,
            access_ttl_min: settings.auth.jwt_access_ttl_min,
            refresh_ttl_days: settings.auth.jwt_refresh_ttl_days,
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        tracing::info!(issuer = %jwt.issuer(), audience = %jwt.audience(), "access tokens: ES256");

        let http = reqwest::Client::builder()
            .user_agent("mygame-realm-api/0.1")
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        let limit_scale = if settings.app_env == "development" {
            100.0
        } else {
            1.0
        };
        let realm = realm_game::bridge::RealmBridge::new(
            http.clone(),
            &settings.realm.spacetimedb_url,
            &settings.realm.spacetimedb_database,
        );
        Ok(Self {
            settings,
            db,
            jwt: Arc::new(jwt),
            http,
            limits: Arc::new(crate::rate_limit::Limits::scaled(limit_scale)),
            realm,
        })
    }
}

impl HasJwt for AppState {
    fn jwt_codec(&self) -> &JwtCodec {
        &self.jwt
    }
}

#[async_trait::async_trait]
impl FindUserById for AppState {
    async fn find_user_by_id(&self, id: Uuid) -> Result<Option<User>, AppError> {
        user_db::find_by_id(&self.db, id).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl AdminUserStore for AppState {
    async fn user_permissions(&self, user_id: Uuid) -> Result<HashSet<String>, AppError> {
        generic_auth_auth::permissions::load_permissions(&self.db, user_id).await
    }

    async fn delete_user(&self, id: Uuid) -> Result<bool, AppError> {
        user_db::delete_user(&self.db, id).await.map_err(Into::into)
    }
}
