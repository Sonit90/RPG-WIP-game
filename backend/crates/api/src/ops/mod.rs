//! The operator dashboard's data: server health, the live realm, one instance in detail, and
//! run anomalies. `routes::ops` is the HTTP face; `OpsProvider` is the seam tests replace.
//!
//! Everything here is read-only. The realm is asked on demand through `svc_admin_snapshot`
//! and the answer is cached for a second, so a dashboard open in ten browsers costs the realm
//! one walk of its tables per second.

pub mod anomalies;
pub mod errlog;
pub mod gear;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use engine_realm::ops::{AdminSnapshot, InstanceView};
use generic_auth_core::AppError;
use serde::Serialize;
use serde_json::json;
use sqlx::types::Json as SqlJson;
use sysinfo::{Pid, ProcessesToUpdate, System};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;
use anomalies::{DamageAnomalies, DamageRow, RunAnomalies, RunRow};
use errlog::ErrorEntry;
use gear::{GearItem, TalentView};

/// How long a realm snapshot is reused.
const SNAPSHOT_TTL: Duration = Duration::from_secs(1);
/// Rows read for the anomaly report.
const ANOMALY_ROW_CAP: i64 = 5000;

/// Why an ops request failed.
#[derive(Debug)]
pub enum OpsError {
    /// The realm could not be asked.
    Realm(String),
    /// Anything else.
    App(AppError),
}

impl From<AppError> for OpsError {
    fn from(e: AppError) -> Self {
        Self::App(e)
    }
}

impl IntoResponse for OpsError {
    fn into_response(self) -> Response {
        match self {
            Self::Realm(message) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": {"code": "realm_unavailable", "message": message}})),
            )
                .into_response(),
            Self::App(e) => ApiError(e).into_response(),
        }
    }
}

fn db_error(e: sqlx::Error) -> OpsError {
    OpsError::App(AppError::Database(e.to_string()))
}

// --- bridge bookkeeping ----------------------------------------------------------------

/// What the result collector has seen.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BridgeStats {
    /// Whether the collector runs at all.
    pub enabled: bool,
    /// Unix seconds of the last pass that reached the realm.
    pub last_success_at: Option<i64>,
    /// Unix seconds of the last pass that did not.
    pub last_failure_at: Option<i64>,
    pub last_error: Option<String>,
    /// Results the realm handed over in the last pass (not yet acknowledged before it).
    pub pending_results: usize,
    /// Loot deliveries handed over in the last pass.
    pub pending_loot: usize,
    pub passes: u64,
    pub failures: u64,
}

struct Cached {
    at: Instant,
    snapshot: Arc<AdminSnapshot>,
    latency_ms: f64,
}

/// Process-wide ops state, built once with the app state.
pub struct OpsState {
    started: Instant,
    started_at: i64,
    system: Mutex<System>,
    bridge: Mutex<BridgeStats>,
    cache: tokio::sync::Mutex<Option<Cached>>,
    snapshot_error: Mutex<Option<(i64, String)>>,
}

fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

impl OpsState {
    pub fn new(bridge_enabled: bool) -> Self {
        let mut system = System::new();
        // A first sample now, so the first status call has a CPU delta to report.
        system.refresh_cpu_usage();
        Self {
            started: Instant::now(),
            started_at: now_unix(),
            system: Mutex::new(system),
            bridge: Mutex::new(BridgeStats {
                enabled: bridge_enabled,
                ..BridgeStats::default()
            }),
            cache: tokio::sync::Mutex::new(None),
            snapshot_error: Mutex::new(None),
        }
    }

    /// A collection pass reached the realm.
    pub fn bridge_pass_ok(&self, pending_results: usize) {
        if let Ok(mut b) = self.bridge.lock() {
            b.passes += 1;
            b.last_success_at = Some(now_unix());
            b.pending_results = pending_results;
        }
    }

    /// Loot collected in the pass.
    pub fn bridge_loot_pending(&self, pending_loot: usize) {
        if let Ok(mut b) = self.bridge.lock() {
            b.pending_loot = pending_loot;
        }
    }

    /// A collection pass failed: remembered here and in the error list.
    pub fn bridge_pass_failed(&self, message: &str) {
        if let Ok(mut b) = self.bridge.lock() {
            b.passes += 1;
            b.failures += 1;
            b.last_failure_at = Some(now_unix());
            b.last_error = Some(message.to_owned());
        }
        errlog::error_log().push("bridge", message);
    }

    fn bridge_stats(&self) -> BridgeStats {
        self.bridge.lock().map(|b| b.clone()).unwrap_or_default()
    }
}

// --- reports ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ApiInfo {
    pub version: &'static str,
    pub environment: String,
    pub uptime_seconds: u64,
    pub started_at: i64,
}

#[derive(Debug, Serialize)]
pub struct DbInfo {
    pub ok: bool,
    pub latency_ms: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RealmInfo {
    pub enabled: bool,
    pub reachable: bool,
    pub snapshot_latency_ms: Option<f64>,
    pub error: Option<String>,
    pub online_accounts: Option<u32>,
    pub active_instances: Option<usize>,
    pub max_ticks_behind: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct SystemInfo {
    pub cpu_cores: usize,
    /// This process, percent of the whole machine (0-100).
    pub process_cpu_percent: f32,
    pub process_rss_bytes: u64,
    pub system_cpu_percent: f32,
    pub system_memory_used_bytes: u64,
    pub system_memory_total_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub api: ApiInfo,
    pub db: DbInfo,
    pub realm: RealmInfo,
    pub system: SystemInfo,
    pub bridge: BridgeStats,
    /// The last errors, newest first.
    pub errors: Vec<ErrorEntry>,
}

#[derive(Debug, Serialize)]
pub struct PlayerDetail {
    pub account_id: String,
    pub name: String,
    pub class: String,
    pub gear: Vec<GearItem>,
    /// Talents bought, from the character's row; empty when it has none or the row is gone.
    pub talents: Vec<TalentView>,
}

#[derive(Debug, Serialize)]
pub struct InstanceDetail {
    pub dungeon_name: String,
    /// Where the party starts: X, Z. A hint for the map's bounds.
    pub spawn_points: Vec<[f32; 2]>,
    pub instance: InstanceView,
    pub players: Vec<PlayerDetail>,
    pub snapshot_at_us: i64,
}

#[derive(Debug, Serialize)]
pub struct AnomalyReport {
    pub generated_at: i64,
    pub thresholds: crate::config::Anomaly,
    pub runs: RunAnomalies,
    pub damage: DamageAnomalies,
}

// --- the seam --------------------------------------------------------------------------

/// What the ops endpoints need. `AppState` implements it; tests substitute canned data.
#[async_trait::async_trait]
pub trait OpsProvider: Send + Sync {
    async fn status(&self) -> Result<StatusReport, OpsError>;
    async fn realm_snapshot(&self) -> Result<Arc<AdminSnapshot>, OpsError>;
    async fn instance_detail(&self, id: u64) -> Result<InstanceDetail, OpsError>;
    async fn anomalies(&self) -> Result<AnomalyReport, OpsError>;
}

impl AppState {
    /// The realm's snapshot and how long fetching it took, reused for a second.
    async fn cached_snapshot(&self) -> Result<(Arc<AdminSnapshot>, f64), OpsError> {
        if !self.settings.realm.enabled {
            return Err(OpsError::Realm("the realm bridge is disabled".into()));
        }
        let mut cache = self.ops.cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            if cached.at.elapsed() < SNAPSHOT_TTL {
                return Ok((cached.snapshot.clone(), cached.latency_ms));
            }
        }
        let token = self
            .jwt
            .issue_service(&self.settings.realm.service_subject)
            .map_err(|e| OpsError::Realm(format!("cannot sign a service token: {e}")))?;
        let started = Instant::now();
        let result = self.realm.admin_snapshot(&token).await;
        let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
        match result {
            Ok(snapshot) => {
                let snapshot = Arc::new(snapshot);
                if let Ok(mut err) = self.ops.snapshot_error.lock() {
                    *err = None;
                }
                *cache = Some(Cached {
                    at: Instant::now(),
                    snapshot: snapshot.clone(),
                    latency_ms,
                });
                Ok((snapshot, latency_ms))
            }
            Err(e) => {
                let message = e.to_string();
                if let Ok(mut err) = self.ops.snapshot_error.lock() {
                    *err = Some((now_unix(), message.clone()));
                }
                *cache = None;
                Err(OpsError::Realm(message))
            }
        }
    }

    fn system_info(&self) -> SystemInfo {
        let Ok(mut sys) = self.ops.system.lock() else {
            return SystemInfo {
                cpu_cores: 0,
                process_cpu_percent: 0.0,
                process_rss_bytes: 0,
                system_cpu_percent: 0.0,
                system_memory_used_bytes: 0,
                system_memory_total_bytes: 0,
            };
        };
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let pid = sysinfo::get_current_pid().ok();
        if let Some(pid) = pid {
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        }
        let cores = sys.cpus().len().max(1);
        let process = pid.and_then(|pid: Pid| sys.process(pid));
        SystemInfo {
            cpu_cores: cores,
            // sysinfo reports a process as a percent of one core.
            process_cpu_percent: process.map_or(0.0, |p| p.cpu_usage() / cores as f32),
            process_rss_bytes: process.map_or(0, |p| p.memory()),
            system_cpu_percent: sys.global_cpu_usage(),
            system_memory_used_bytes: sys.used_memory(),
            system_memory_total_bytes: sys.total_memory(),
        }
    }

    async fn db_info(&self) -> DbInfo {
        let started = Instant::now();
        let ping = tokio::time::timeout(
            Duration::from_secs(3),
            sqlx::query("SELECT 1").execute(&self.db),
        )
        .await;
        match ping {
            Ok(Ok(_)) => DbInfo {
                ok: true,
                latency_ms: Some(started.elapsed().as_secs_f64() * 1000.0),
                error: None,
            },
            Ok(Err(e)) => DbInfo {
                ok: false,
                latency_ms: None,
                error: Some(e.to_string()),
            },
            Err(_) => DbInfo {
                ok: false,
                latency_ms: None,
                error: Some("ping timed out after 3s".into()),
            },
        }
    }
}

#[async_trait::async_trait]
impl OpsProvider for AppState {
    async fn status(&self) -> Result<StatusReport, OpsError> {
        let db = self.db_info().await;
        let realm = match self.cached_snapshot().await {
            Ok((snapshot, latency_ms)) => RealmInfo {
                enabled: true,
                reachable: true,
                snapshot_latency_ms: Some(latency_ms),
                error: None,
                online_accounts: Some(snapshot.online_accounts),
                active_instances: Some(snapshot.instances.len()),
                max_ticks_behind: Some(snapshot.max_ticks_behind),
            },
            Err(OpsError::Realm(message)) => RealmInfo {
                enabled: self.settings.realm.enabled,
                reachable: false,
                snapshot_latency_ms: None,
                error: Some(message),
                online_accounts: None,
                active_instances: None,
                max_ticks_behind: None,
            },
            Err(e) => return Err(e),
        };
        Ok(StatusReport {
            api: ApiInfo {
                version: env!("CARGO_PKG_VERSION"),
                environment: self.settings.app_env.clone(),
                uptime_seconds: self.ops.started.elapsed().as_secs(),
                started_at: self.ops.started_at,
            },
            db,
            realm,
            system: self.system_info(),
            bridge: self.ops.bridge_stats(),
            errors: errlog::error_log().recent(),
        })
    }

    async fn realm_snapshot(&self) -> Result<Arc<AdminSnapshot>, OpsError> {
        self.cached_snapshot().await.map(|(snapshot, _)| snapshot)
    }

    async fn instance_detail(&self, id: u64) -> Result<InstanceDetail, OpsError> {
        let (snapshot, _) = self.cached_snapshot().await?;
        let instance = snapshot
            .instances
            .iter()
            .find(|i| i.id == id)
            .cloned()
            .ok_or(OpsError::App(AppError::NotFound))?;

        let character_ids: Vec<Uuid> = instance
            .players
            .iter()
            .filter_map(|p| Uuid::parse_str(&p.character_id).ok())
            .collect();
        let rows: Vec<(Uuid, SqlJson<Vec<String>>)> = if character_ids.is_empty() {
            Vec::new()
        } else {
            sqlx::query_as("SELECT id, talents FROM game.characters WHERE id = ANY($1)")
                .bind(&character_ids)
                .fetch_all(&self.db)
                .await
                .map_err(db_error)?
        };
        let players = instance
            .players
            .iter()
            .map(|p| {
                let talents = Uuid::parse_str(&p.character_id)
                    .ok()
                    .and_then(|id| rows.iter().find(|(row, _)| *row == id))
                    .map(|(_, SqlJson(keys))| gear::resolve_talents(&p.class, keys))
                    .unwrap_or_default();
                PlayerDetail {
                    account_id: p.account_id.clone(),
                    name: p.name.clone(),
                    class: p.class.clone(),
                    gear: gear::resolve_gear(&p.gear),
                    talents,
                }
            })
            .collect();

        let dungeon = realm_game::catalog().dungeon(&instance.dungeon_id);
        Ok(InstanceDetail {
            dungeon_name: dungeon.map_or_else(|| instance.dungeon_id.clone(), |d| d.name.clone()),
            spawn_points: dungeon
                .map(|d| {
                    d.spawn_points
                        .iter()
                        .map(|s| [s.position[0], s.position[2]])
                        .collect()
                })
                .unwrap_or_default(),
            snapshot_at_us: snapshot.generated_at_us,
            instance,
            players,
        })
    }

    async fn anomalies(&self) -> Result<AnomalyReport, OpsError> {
        let config = &self.settings.ops.anomaly;
        let run_rows: Vec<(String, String, i32, i64, f64)> = sqlx::query_as(
            "SELECT result_id, dungeon_id, key_level, duration_us,
                    EXTRACT(EPOCH FROM applied_at)::float8
             FROM (
                 SELECT DISTINCT ON (split_part(result_id, ':', 1))
                        result_id, dungeon_id, key_level, duration_us, applied_at
                 FROM game.dungeon_results
                 WHERE success
                 ORDER BY split_part(result_id, ':', 1), applied_at DESC
             ) runs
             ORDER BY applied_at DESC
             LIMIT $1",
        )
        .bind(ANOMALY_ROW_CAP)
        .fetch_all(&self.db)
        .await
        .map_err(db_error)?;
        let runs: Vec<RunRow> = run_rows
            .into_iter()
            .map(
                |(result_id, dungeon_id, key_level, duration_us, completed_at)| RunRow {
                    result_id,
                    dungeon_id,
                    key_level,
                    duration_seconds: duration_us as f64 / 1_000_000.0,
                    completed_at: completed_at as i64,
                },
            )
            .collect();

        type DamageColumns = (
            String,
            String,
            i32,
            String,
            Option<String>,
            String,
            i64,
            f64,
            f64,
            Option<f64>,
            f64,
        );
        let damage_rows: Result<Vec<DamageColumns>, sqlx::Error> = sqlx::query_as(
            "SELECT instance_id, dungeon_id, key_level, account_id::text, character_id::text,
                    class, total_damage, dps::float8, duration_seconds::float8,
                    boss_kill_seconds::float8, EXTRACT(EPOCH FROM completed_at)::float8
             FROM game.dungeon_damage_reports
             ORDER BY completed_at DESC
             LIMIT $1",
        )
        .bind(ANOMALY_ROW_CAP)
        .fetch_all(&self.db)
        .await;
        let damage = match damage_rows {
            Ok(rows) => {
                let rows: Vec<DamageRow> = rows
                    .into_iter()
                    .map(|r| DamageRow {
                        instance_id: r.0,
                        dungeon_id: r.1,
                        key_level: r.2,
                        account_id: r.3,
                        character_id: r.4,
                        class: r.5,
                        total_damage: r.6,
                        dps: r.7,
                        duration_seconds: r.8,
                        boss_kill_seconds: r.9,
                        completed_at: r.10 as i64,
                    })
                    .collect();
                anomalies::damage_anomalies(&rows, config)
            }
            // A database that predates migration 0006: report it, do not fail the panel.
            Err(e) => DamageAnomalies {
                available: false,
                note: Some(format!("damage reports unavailable: {e}")),
                ..DamageAnomalies::default()
            },
        };

        Ok(AnomalyReport {
            generated_at: now_unix(),
            thresholds: config.clone(),
            runs: anomalies::run_anomalies(&runs, config),
            damage,
        })
    }
}
