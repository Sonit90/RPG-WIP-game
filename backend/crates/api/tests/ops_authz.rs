//! Who may read `/admin/ops/*`, without a database.
//!
//! The real routes are generic over the state, so the real handlers and the real `OpsUser`
//! extractor run here against a mock that holds canned data:
//!   * no token                      -> 401
//!   * a token, but not `ops.view`   -> 403 (even for `users.manage`)
//!   * `ops.view`                    -> 200 with the data
//!   * the realm down                -> 503, not a panic
//!   * the page at `/admin`          -> public, with its security headers

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    routing::get,
    Router,
};
use engine_realm::ops::AdminSnapshot;
use generic_auth_api::middleware::auth::{AdminUserStore, FindUserById, HasJwt};
use generic_auth_api::ops::{
    AnomalyReport, ApiInfo, DbInfo, InstanceDetail, OpsError, OpsProvider, RealmInfo, StatusReport,
    SystemInfo,
};
use generic_auth_api::routes::ops;
use generic_auth_auth::JwtCodec;
use generic_auth_core::{
    models::{Role, User},
    AppError,
};
use http_body_util::BodyExt as _;
use time::OffsetDateTime;
use tower::ServiceExt as _;
use uuid::Uuid;

#[derive(Clone)]
struct Mock {
    jwt: Arc<JwtCodec>,
    perms: HashSet<String>,
    realm_up: bool,
}

impl HasJwt for Mock {
    fn jwt_codec(&self) -> &JwtCodec {
        &self.jwt
    }
}

#[async_trait::async_trait]
impl FindUserById for Mock {
    async fn find_user_by_id(&self, id: Uuid) -> Result<Option<User>, AppError> {
        Ok(Some(User {
            id,
            email: Some("ops@example.com".into()),
            display_name: None,
            role: Role::Admin,
            is_active: true,
            email_verified: true,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }))
    }
}

#[async_trait::async_trait]
impl AdminUserStore for Mock {
    async fn user_permissions(&self, _: Uuid) -> Result<HashSet<String>, AppError> {
        Ok(self.perms.clone())
    }
    async fn delete_user(&self, _: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }
}

#[async_trait::async_trait]
impl OpsProvider for Mock {
    async fn status(&self) -> Result<StatusReport, OpsError> {
        Ok(StatusReport {
            api: ApiInfo {
                version: "test",
                environment: "test".into(),
                uptime_seconds: 1,
                started_at: 0,
            },
            db: DbInfo {
                ok: true,
                latency_ms: Some(1.0),
                error: None,
            },
            realm: RealmInfo {
                enabled: true,
                reachable: self.realm_up,
                snapshot_latency_ms: None,
                error: None,
                online_accounts: None,
                active_instances: None,
                max_ticks_behind: None,
            },
            system: SystemInfo {
                cpu_cores: 1,
                process_cpu_percent: 0.0,
                process_rss_bytes: 0,
                system_cpu_percent: 0.0,
                system_memory_used_bytes: 0,
                system_memory_total_bytes: 0,
            },
            bridge: Default::default(),
            errors: Vec::new(),
        })
    }

    async fn realm_snapshot(&self) -> Result<Arc<AdminSnapshot>, OpsError> {
        if !self.realm_up {
            return Err(OpsError::Realm("connection refused".into()));
        }
        Ok(Arc::new(AdminSnapshot {
            online_accounts: 4,
            ..AdminSnapshot::default()
        }))
    }

    async fn instance_detail(&self, _: u64) -> Result<InstanceDetail, OpsError> {
        Err(OpsError::App(AppError::NotFound))
    }

    async fn anomalies(&self) -> Result<AnomalyReport, OpsError> {
        Ok(AnomalyReport {
            generated_at: 0,
            thresholds: Default::default(),
            runs: Default::default(),
            damage: Default::default(),
        })
    }
}

fn codec() -> JwtCodec {
    generic_auth_auth::jwt::test_codec()
}

fn mock(perms: &[&str], realm_up: bool) -> Mock {
    Mock {
        jwt: Arc::new(codec()),
        perms: perms.iter().map(|p| (*p).to_owned()).collect(),
        realm_up,
    }
}

fn app(state: Mock) -> Router {
    Router::new()
        .nest("/api/v1/admin", ops::router::<Mock>())
        .route("/admin", get(ops::dashboard))
        .with_state(state)
}

async fn get_status(state: Mock, path: &str, with_token: bool) -> (StatusCode, String) {
    let mut request = Request::builder().uri(path);
    if with_token {
        let token = codec().issue(Uuid::new_v4(), Role::Admin).unwrap();
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app(state)
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

const PATHS: [&str; 4] = [
    "/api/v1/admin/ops/status",
    "/api/v1/admin/ops/realm",
    "/api/v1/admin/ops/instances/7",
    "/api/v1/admin/ops/anomalies",
];

#[tokio::test]
async fn no_token_is_unauthorized_everywhere() {
    for path in PATHS {
        let (status, _) = get_status(mock(&["ops.view"], true), path, false).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
}

#[tokio::test]
async fn a_user_without_ops_view_is_forbidden_everywhere() {
    // users.manage is not enough: the dashboard has its own permission.
    for perms in [&[][..], &["users.manage", "users.read"][..]] {
        for path in PATHS {
            let (status, _) = get_status(mock(perms, true), path, true).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path} with {perms:?}");
        }
    }
}

#[tokio::test]
async fn ops_view_reads_status_realm_and_anomalies() {
    let state = mock(&["ops.view"], true);
    let (status, body) = get_status(state.clone(), PATHS[0], true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"uptime_seconds\":1"), "{body}");

    let (status, body) = get_status(state.clone(), PATHS[1], true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"online_accounts\":4"), "{body}");

    let (status, _) = get_status(state, PATHS[3], true).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_missing_instance_is_not_found() {
    let (status, _) = get_status(mock(&["ops.view"], true), PATHS[2], true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_realm_that_is_down_is_service_unavailable() {
    let (status, body) = get_status(mock(&["ops.view"], false), PATHS[1], true).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("realm_unavailable"), "{body}");
}

#[tokio::test]
async fn the_dashboard_page_is_public_and_locked_down() {
    let response = app(mock(&[], true))
        .oneshot(
            Request::builder()
                .uri("/admin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert!(headers[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert!(headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .contains("default-src 'none'"));
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("Realm operator dashboard"));
    // No external scripts or styles: the page must work on a locked-down box.
    assert!(!html.contains("src=\"http"));
    assert!(!html.contains("href=\"http"));
}
