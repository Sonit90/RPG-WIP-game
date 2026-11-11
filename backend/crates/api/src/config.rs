use config::{Config, Environment, File};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    #[serde(skip)]
    pub app_env: String,
    pub http: Http,
    pub db: Db,
    pub auth: Auth,
    pub cors: Cors,
    pub email: Email,
    #[serde(default)]
    pub realm: Realm,
    #[serde(default)]
    pub ops: Ops,
}

/// The SpacetimeDB realm this service bridges to.
#[derive(Debug, Clone, Deserialize)]
pub struct Realm {
    /// Whether the bridge runs at all (queueing and result collection need it).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The SpacetimeDB server, as this service reaches it (`SPACETIMEDB_URL`).
    #[serde(default = "default_spacetimedb_url")]
    pub spacetimedb_url: String,
    /// The realm database's name (`SPACETIMEDB_DATABASE`).
    #[serde(default = "default_spacetimedb_database")]
    pub spacetimedb_database: String,
    /// The `sub` of the bridge's own token. The module derives the bridge's identity from it.
    #[serde(default = "default_service_subject")]
    pub service_subject: String,
    /// Seconds between result collections.
    #[serde(default = "default_poll_secs")]
    pub result_poll_secs: u64,
}

impl Default for Realm {
    fn default() -> Self {
        Self {
            enabled: true,
            spacetimedb_url: default_spacetimedb_url(),
            spacetimedb_database: default_spacetimedb_database(),
            service_subject: default_service_subject(),
            result_poll_secs: default_poll_secs(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_spacetimedb_url() -> String {
    "http://127.0.0.1:3000".into()
}
fn default_spacetimedb_database() -> String {
    "realm".into()
}
fn default_service_subject() -> String {
    "service:realm-bridge".into()
}
fn default_poll_secs() -> u64 {
    5
}

#[derive(Debug, Clone, Deserialize)]
pub struct Http {
    pub bind: String,
    pub public_url: String,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Db {
    pub url: String,
    pub max_connections: u32,
    #[serde(default = "default_min")]
    pub min_connections: u32,
    #[serde(default = "default_acquire")]
    pub acquire_timeout_secs: u64,
    #[serde(default)]
    pub run_migrations_on_start: bool,
}
fn default_min() -> u32 {
    2
}
fn default_acquire() -> u64 {
    10
}

#[derive(Debug, Clone, Deserialize)]
pub struct Auth {
    /// HMAC key for email verification codes. Not used for access tokens any more.
    pub jwt_secret: String,
    /// The access-token issuer. Empty means `http.public_url`. Must be the URL SpacetimeDB
    /// can fetch `/.well-known/openid-configuration` from.
    #[serde(default)]
    pub jwt_issuer: String,
    /// The audience every access token carries.
    #[serde(default = "default_audience")]
    pub jwt_audience: String,
    /// The key id published in the JWKS.
    #[serde(default = "default_key_id")]
    pub jwt_key_id: String,
    /// The ES256 private key, PKCS#8 PEM, inline (`APP__AUTH__JWT_PRIVATE_KEY_PEM`).
    #[serde(default)]
    pub jwt_private_key_pem: Option<String>,
    /// Or a path to it (`APP__AUTH__JWT_PRIVATE_KEY_PATH`). In development a missing file
    /// is created; in production a missing key is a startup error.
    #[serde(default)]
    pub jwt_private_key_path: Option<String>,
    pub jwt_access_ttl_min: i64,
    pub jwt_refresh_ttl_days: i64,
    pub password_min_length: usize,
    /// Minutes a 6-digit email verification code stays valid.
    #[serde(default = "default_email_code_ttl_min")]
    pub email_code_ttl_min: i64,
    /// Max wrong guesses before a code is invalidated (online brute-force cap).
    #[serde(default = "default_email_code_max_attempts")]
    pub email_code_max_attempts: i32,
    /// Minimum seconds between resend requests for the same account.
    #[serde(default = "default_email_code_resend_cooldown_secs")]
    pub email_code_resend_cooldown_secs: i64,
    /// Max codes issued per account per rolling hour.
    #[serde(default = "default_email_code_max_per_hour")]
    pub email_code_max_per_hour: i64,
    pub google: GoogleAuth,
}

fn default_audience() -> String {
    "mygame-realm".into()
}
fn default_key_id() -> String {
    "realm-1".into()
}
fn default_email_code_ttl_min() -> i64 {
    15
}
fn default_email_code_max_attempts() -> i32 {
    5
}
fn default_email_code_resend_cooldown_secs() -> i64 {
    15
}
fn default_email_code_max_per_hour() -> i64 {
    5
}

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleAuth {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cors {
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Email {
    /// SMTP hostname (e.g. smtp.mailjet.com, smtp.sendgrid.net).
    /// Leave blank to disable sending and log the verification link instead.
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    pub from_email: String,
    pub from_name: String,
}

impl Settings {
    /// Load `config/default.toml`, then `config/<APP_ENV>.toml`, then env vars
    /// of the form `APP__SECTION__KEY`. `DATABASE_URL` overrides `db.url`.
    pub fn load() -> anyhow::Result<Self> {
        let app_env = std::env::var("APP_ENV").unwrap_or_else(|_| "development".into());

        // Resolve the config dir relative to the binary's CWD.
        let mut builder = Config::builder()
            .add_source(File::with_name("config/default").required(false))
            .add_source(File::with_name(&format!("config/{app_env}")).required(false))
            .add_source(
                Environment::with_prefix("APP")
                    .separator("__")
                    .try_parsing(true)
                    .list_separator(",")
                    .with_list_parse_key("cors.allowed_origins"),
            );

        if let Ok(url) = std::env::var("DATABASE_URL") {
            builder = builder.set_override("db.url", url)?;
        }
        // The deployment-wide names the game client uses too.
        if let Ok(url) = std::env::var("SPACETIMEDB_URL") {
            builder = builder.set_override("realm.spacetimedb_url", url)?;
        }
        if let Ok(name) = std::env::var("SPACETIMEDB_DATABASE") {
            builder = builder.set_override("realm.spacetimedb_database", name)?;
        }

        let mut s: Settings = builder.build()?.try_deserialize()?;
        s.app_env = app_env;
        if s.auth.jwt_issuer.trim().is_empty() {
            s.auth.jwt_issuer = s.http.public_url.clone();
        }
        s.auth.jwt_issuer = s.auth.jwt_issuer.trim_end_matches('/').to_string();
        Ok(s)
    }
}

/// The operator dashboard's settings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Ops {
    #[serde(default)]
    pub anomaly: Anomaly,
}

/// What counts as suspicious (`[ops.anomaly]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anomaly {
    /// A completed run faster than this many seconds is flagged. 0 turns the check off.
    #[serde(default = "default_min_completion_seconds")]
    pub min_completion_seconds: f64,
    /// ...or faster than this percent of its dungeon's median completed run.
    #[serde(default = "default_relative_fast_percent")]
    pub relative_fast_percent: f64,
    /// Completed runs a dungeon needs before the relative check applies.
    #[serde(default = "default_relative_min_samples")]
    pub relative_min_samples: usize,
    /// A player's DPS above this is flagged.
    #[serde(default = "default_max_dps")]
    pub max_dps: f64,
    /// A player's total damage in one run above this is flagged.
    #[serde(default = "default_max_total_damage")]
    pub max_total_damage: i64,
    /// A boss kill faster than this many seconds is flagged.
    #[serde(default = "default_min_boss_kill_seconds")]
    pub min_boss_kill_seconds: f64,
    /// Rows per list in the report.
    #[serde(default = "default_list_limit")]
    pub list_limit: usize,
}

impl Default for Anomaly {
    fn default() -> Self {
        Self {
            min_completion_seconds: default_min_completion_seconds(),
            relative_fast_percent: default_relative_fast_percent(),
            relative_min_samples: default_relative_min_samples(),
            max_dps: default_max_dps(),
            max_total_damage: default_max_total_damage(),
            min_boss_kill_seconds: default_min_boss_kill_seconds(),
            list_limit: default_list_limit(),
        }
    }
}

fn default_min_completion_seconds() -> f64 {
    60.0
}
fn default_relative_fast_percent() -> f64 {
    50.0
}
fn default_relative_min_samples() -> usize {
    5
}
fn default_max_dps() -> f64 {
    5000.0
}
fn default_max_total_damage() -> i64 {
    5_000_000
}
fn default_min_boss_kill_seconds() -> f64 {
    10.0
}
fn default_list_limit() -> usize {
    20
}
