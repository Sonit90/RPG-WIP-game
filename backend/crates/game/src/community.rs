//! Community maps: the `game.community_maps` / `community_map_versions` queries behind
//! `/community/*`. No HTTP here.
//!
//! ```text
//!  text ─► engine_realm::community::check (parse + mapcheck, every write) ─► version row
//!  publish ─► check again on the stored text ─► status = published ─► (route) realm push
//! ```
//!
//! Rules that live in this file, not in the routes:
//!
//! * **The validator is the gate.** `create`, `update` and `publish` all run
//!   `community::check`; nothing the client says about validity is read. A text that is not a
//!   map at all is refused without storing anything; a map with errors is stored as a draft
//!   with its report, and can never be published while it has any.
//! * **Versions are immutable.** An update inserts version `n + 1`. Updating a published map
//!   returns it to `draft` (its older version leaves the realm until it is published again), so
//!   a published `cm:<id>:<version>` is always exactly what was validated.
//! * **Author only.** Updating, publishing and unpublishing look the map up by
//!   `(id, author_account)`; anyone else gets a 404, as for a map that does not exist.
//! * **Moderation** (`hide`, `reject`, `restore`) takes any map; the routes check the
//!   `community.moderate` permission. A hidden or rejected map cannot be published by its author.
//! * **Request ids.** Every write may carry a `request_id`; the first answer is stored in
//!   `game.community_requests` in the same transaction and a replay returns it unchanged. The
//!   transaction takes an advisory lock on the request id first, so two concurrent retries
//!   serialise instead of both writing.
//! * **Visibility.** Only `published` maps are listed or fetched by others; the author sees all
//!   of theirs; a moderator can list by status.

use engine_realm::community::{
    self, Listing, MapActionRequest, MapDetail, MapPage, MapSort, MapStatus,
    MapSummary, ReportInfo, SaveMapResponse, VersionInfo, MAX_DIFFICULTY, MAX_MAPS_PER_AUTHOR,
    MAX_VERSIONS,
};
use generic_auth_core::AppError;
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::store::{db, Tx};

/// Maps per list page.
pub const PAGE_SIZE: u32 = 12;

/// Who is asking.
#[derive(Debug, Clone, Copy)]
pub struct Viewer {
    /// The account.
    pub account: Uuid,
    /// Whether the account holds `community.moderate`.
    pub moderator: bool,
}

// ---------------------------------------------------------------------------------------
// Request ids
// ---------------------------------------------------------------------------------------

/// A write in progress: the transaction, and the request id to record the answer under.
struct Write {
    tx: Tx,
    account: Uuid,
    request: Option<(Uuid, &'static str)>,
}

enum Begin {
    /// The request id was served before: its answer.
    Replay(serde_json::Value),
    Go(Box<Write>),
}

async fn begin(
    pool: &PgPool,
    account: Uuid,
    request_id: Option<Uuid>,
    kind: &'static str,
) -> Result<Begin, AppError> {
    let mut tx = pool.begin().await.map_err(db)?;
    if let Some(request_id) = request_id {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(request_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let seen: Option<(Uuid, String, Json<serde_json::Value>)> = sqlx::query_as(
            "SELECT account_id, kind, response FROM game.community_requests WHERE request_id = $1",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        if let Some((owner, seen_kind, Json(response))) = seen {
            if owner != account || seen_kind != kind {
                tracing::warn!(account_id = %account, %request_id, kind, "request id reused for another action");
                return Err(AppError::Conflict(
                    "That request id was already used for another action.".into(),
                ));
            }
            tracing::info!(account_id = %account, %request_id, kind, "community request replayed");
            return Ok(Begin::Replay(response));
        }
    }
    Ok(Begin::Go(Box::new(Write {
        tx,
        account,
        request: request_id.map(|id| (id, kind)),
    })))
}

impl Write {
    /// Records `response` under the request id (if any) and commits.
    async fn commit<R: serde::Serialize>(mut self, response: &R) -> Result<(), AppError> {
        if let Some((request_id, kind)) = self.request {
            let response = serde_json::to_value(response)
                .map_err(|_| AppError::internal("The response could not be recorded."))?;
            sqlx::query(
                "INSERT INTO game.community_requests (request_id, account_id, kind, response)
                 VALUES ($1, $2, $3, $4) ON CONFLICT (request_id) DO NOTHING",
            )
            .bind(request_id)
            .bind(self.account)
            .bind(kind)
            .bind(Json(&response))
            .execute(&mut *self.tx)
            .await
            .map_err(db)?;
        }
        self.tx.commit().await.map_err(db)
    }
}

fn replayed<T: serde::de::DeserializeOwned>(response: serde_json::Value) -> Result<T, AppError> {
    serde_json::from_value(response).map_err(|_| {
        AppError::internal("The earlier answer to that request could not be read.")
    })
}

// ---------------------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------------------

const SUMMARY_SELECT: &str = "SELECT m.id, m.title, m.author_account,
        COALESCE(NULLIF(u.display_name, ''), 'Player ' || left(u.id::text, 6)) AS author,
        m.description, m.difficulty, m.tags, m.status, m.current_version, m.plays,
        (SELECT min(r.duration_us) FROM game.dungeon_runs r
          WHERE r.dungeon_id = 'cm:' || m.id::text AND r.outcome IN ('timed', 'overtime'))
            AS fastest_us,
        (SELECT count(*) FROM game.dungeon_runs r
          WHERE r.dungeon_id = 'cm:' || m.id::text AND r.outcome IN ('timed', 'overtime'))
            AS completions,
        v.valid,
        (EXTRACT(EPOCH FROM m.created_at) * 1000000)::bigint AS created_at_us,
        (EXTRACT(EPOCH FROM m.updated_at) * 1000000)::bigint AS updated_at_us
    FROM game.community_maps m
    JOIN auth.users u ON u.id = m.author_account
    JOIN game.community_map_versions v ON v.map_id = m.id AND v.version = m.current_version";

#[derive(sqlx::FromRow)]
struct SummaryRow {
    id: Uuid,
    title: String,
    author_account: Uuid,
    author: String,
    description: String,
    difficulty: i16,
    tags: Vec<String>,
    status: String,
    current_version: i32,
    plays: i64,
    fastest_us: Option<i64>,
    completions: i64,
    valid: bool,
    created_at_us: i64,
    updated_at_us: i64,
}

impl SummaryRow {
    fn into_summary(self) -> MapSummary {
        MapSummary {
            id: self.id.to_string(),
            title: self.title,
            author_id: self.author_account.to_string(),
            author: self.author,
            description: self.description,
            difficulty: u8::try_from(self.difficulty).unwrap_or(1),
            tags: self.tags,
            status: MapStatus::from_key(&self.status).unwrap_or(MapStatus::Draft),
            version: u32::try_from(self.current_version).unwrap_or(1),
            plays: self.plays,
            fastest_us: self.fastest_us,
            completions: self.completions,
            valid: self.valid,
            created_at_us: self.created_at_us,
            updated_at_us: self.updated_at_us,
        }
    }
}

async fn summary_of(conn: &mut PgConnection, id: Uuid) -> Result<Option<MapSummary>, AppError> {
    let row: Option<SummaryRow> = sqlx::query_as(&format!("{SUMMARY_SELECT} WHERE m.id = $1"))
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db)?;
    Ok(row.map(SummaryRow::into_summary))
}

/// The map in full, unchecked for visibility (callers decide who may see it).
async fn detail_of(conn: &mut PgConnection, id: Uuid) -> Result<Option<MapDetail>, AppError> {
    let Some(summary) = summary_of(&mut *conn, id).await? else {
        return Ok(None);
    };
    let report: Json<serde_json::Value> = sqlx::query_scalar(
        "SELECT report FROM game.community_map_versions WHERE map_id = $1 AND version = $2",
    )
    .bind(id)
    .bind(i32::try_from(summary.version).unwrap_or(1))
    .fetch_one(&mut *conn)
    .await
    .map_err(db)?;
    let report: ReportInfo = serde_json::from_value(report.0).unwrap_or_default();
    let versions: Vec<(i32, i64, bool, i32, i32)> = sqlx::query_as(
        "SELECT version, (EXTRACT(EPOCH FROM created_at) * 1000000)::bigint, valid,
                COALESCE(jsonb_array_length(report->'errors'), 0),
                COALESCE(jsonb_array_length(report->'warnings'), 0)
         FROM game.community_map_versions WHERE map_id = $1 ORDER BY version DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let dungeon_id = if summary.status == MapStatus::Published {
        community::dungeon_id(&summary.id, summary.version)
    } else {
        String::new()
    };
    Ok(Some(MapDetail {
        summary,
        report,
        versions: versions
            .into_iter()
            .map(|(version, created, valid, errors, warnings)| VersionInfo {
                version: u32::try_from(version).unwrap_or(0),
                created_at_us: created,
                valid,
                errors: u32::try_from(errors).unwrap_or(0),
                warnings: u32::try_from(warnings).unwrap_or(0),
            })
            .collect(),
        dungeon_id,
        max_difficulty: MAX_DIFFICULTY,
    }))
}

/// One map, if `viewer` may see it: anyone sees a published map, its author and moderators see
/// every status. Anything else is `None` (the same answer as for a map that does not exist).
///
/// # Errors
///
/// The database failed.
pub async fn get(pool: &PgPool, viewer: Viewer, id: Uuid) -> Result<Option<MapDetail>, AppError> {
    let mut conn = pool.acquire().await.map_err(db)?;
    let Some(detail) = detail_of(&mut conn, id).await? else {
        return Ok(None);
    };
    Ok(visible(viewer, &detail.summary).then_some(detail))
}

fn visible(viewer: Viewer, summary: &MapSummary) -> bool {
    summary.status == MapStatus::Published
        || viewer.moderator
        || summary.author_id == viewer.account.to_string()
}

/// The stored text of one version, if `viewer` may see the map. A published map's versions up
/// to its current one are public (a party that started a run reads the version it queued for).
///
/// # Errors
///
/// The database failed.
pub async fn version_data(
    pool: &PgPool,
    viewer: Viewer,
    id: Uuid,
    version: u32,
) -> Result<Option<String>, AppError> {
    let mut conn = pool.acquire().await.map_err(db)?;
    let Some(summary) = summary_of(&mut conn, id).await? else {
        return Ok(None);
    };
    if !visible(viewer, &summary) {
        return Ok(None);
    }
    version_text(&mut conn, id, version).await
}

async fn version_text(
    conn: &mut PgConnection,
    id: Uuid,
    version: u32,
) -> Result<Option<String>, AppError> {
    sqlx::query_scalar(
        "SELECT data::text FROM game.community_map_versions WHERE map_id = $1 AND version = $2",
    )
    .bind(id)
    .bind(i32::try_from(version).unwrap_or(i32::MAX))
    .fetch_optional(conn)
    .await
    .map_err(db)
}

/// What a list asks for.
#[derive(Debug, Clone)]
pub struct ListQuery {
    /// A title substring, case-insensitive.
    pub search: Option<String>,
    /// Only this author's maps. For anyone but the author (or a moderator) that means their
    /// published maps.
    pub author: Option<Uuid>,
    /// The caller's own maps, every status.
    pub mine: bool,
    /// The order.
    pub sort: MapSort,
    /// The author-claimed difficulty, 1 to 5.
    pub difficulty: Option<u8>,
    /// A status filter; honoured for moderators only.
    pub status: Option<MapStatus>,
    /// From 0.
    pub page: u32,
}

/// A page of maps. Published maps only, unless the caller asks for their own (or is a
/// moderator filtering by status).
///
/// # Errors
///
/// The database failed.
pub async fn list(pool: &PgPool, viewer: Viewer, query: &ListQuery) -> Result<MapPage, AppError> {
    let own = query.mine || query.author == Some(viewer.account);
    let (author, status): (Option<Uuid>, Option<&str>) = if own {
        (Some(viewer.account), None)
    } else if viewer.moderator && query.status.is_some() {
        (query.author, query.status.map(MapStatus::key))
    } else {
        (query.author, Some(MapStatus::Published.key()))
    };
    // A status filter on one's own list is honoured too.
    let status = if own { query.status.map(MapStatus::key) } else { status };
    let order = match (query.sort, query.mine) {
        (MapSort::Newest, true) => "m.updated_at DESC, m.id",
        (MapSort::Newest, false) => "m.created_at DESC, m.id",
        (MapSort::Plays, _) => "m.plays DESC, m.created_at DESC, m.id",
        (MapSort::Fastest, _) => "fastest_us ASC NULLS LAST, m.created_at DESC, m.id",
        (MapSort::Difficulty, _) => "m.difficulty ASC, m.created_at DESC, m.id",
        (MapSort::Hardest, _) => "m.difficulty DESC, m.created_at DESC, m.id",
    };
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let sql = format!(
        "{SUMMARY_SELECT}
         WHERE ($1::uuid IS NULL OR m.author_account = $1)
           AND ($2::text IS NULL OR m.status = $2)
           AND ($3::text IS NULL OR position(lower($3) in lower(m.title)) > 0)
           AND ($4::smallint IS NULL OR m.difficulty = $4)
         ORDER BY {order}
         LIMIT $5 OFFSET $6"
    );
    let offset = i64::from(query.page) * i64::from(PAGE_SIZE);
    let rows: Vec<SummaryRow> = sqlx::query_as(&sql)
        .bind(author)
        .bind(status)
        .bind(search)
        .bind(query.difficulty.map(i16::from))
        .bind(i64::from(PAGE_SIZE) + 1)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(db)?;
    let has_more = rows.len() > PAGE_SIZE as usize;
    Ok(MapPage {
        maps: rows
            .into_iter()
            .take(PAGE_SIZE as usize)
            .map(SummaryRow::into_summary)
            .collect(),
        page: query.page,
        page_size: PAGE_SIZE,
        has_more,
    })
}

// ---------------------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------------------

/// What a save did, for the route to act on (the realm push).
#[derive(Debug, Clone)]
pub struct Saved {
    /// The answer.
    pub response: SaveMapResponse,
    /// The map was published and this update returned it to draft: its entries leave the realm.
    pub unpublished: bool,
}

fn report_response(checked: &community::Checked) -> SaveMapResponse {
    SaveMapResponse {
        report: ReportInfo::from(&checked.report),
        map: None,
    }
}

/// Creates a draft from `text`. Returns `map: None` (nothing stored) when the text is not a
/// map at all; a map with validation errors is stored as a draft with its report.
///
/// # Errors
///
/// A conflict for a request id used elsewhere, or for an account at its map limit; a database
/// failure.
pub async fn create(
    pool: &PgPool,
    author: Uuid,
    request_id: Option<Uuid>,
    text: &str,
) -> Result<Saved, AppError> {
    let checked = community::check(text);
    let Some(map) = checked.map.as_ref() else {
        return Ok(Saved {
            response: report_response(&checked),
            unpublished: false,
        });
    };
    let mut write = match begin(pool, author, request_id, "community_create").await? {
        Begin::Replay(response) => {
            return Ok(Saved {
                response: replayed(response)?,
                unpublished: false,
            });
        }
        Begin::Go(write) => write,
    };
    let owned: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM game.community_maps WHERE author_account = $1 AND status <> 'rejected'",
    )
    .bind(author)
    .fetch_one(&mut *write.tx)
    .await
    .map_err(db)?;
    if owned >= MAX_MAPS_PER_AUTHOR {
        return Err(AppError::Conflict(format!(
            "You already have {MAX_MAPS_PER_AUTHOR} maps. Delete or unpublish one first."
        )));
    }
    let listing = Listing::of(map);
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO game.community_maps (author_account, title, description, difficulty, tags)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(author)
    .bind(&listing.title)
    .bind(&listing.description)
    .bind(i16::from(listing.difficulty))
    .bind(&listing.tags)
    .fetch_one(&mut *write.tx)
    .await
    .map_err(db)?;
    insert_version(&mut write.tx, id, 1, text, &checked).await?;
    let detail = detail_of(&mut write.tx, id).await?;
    let response = SaveMapResponse {
        report: ReportInfo::from(&checked.report),
        map: detail,
    };
    write.commit(&response).await?;
    tracing::info!(account_id = %author, map_id = %id, valid = checked.report.is_valid(), "community map created");
    Ok(Saved {
        response,
        unpublished: false,
    })
}

async fn insert_version(
    tx: &mut Tx,
    id: Uuid,
    version: i32,
    text: &str,
    checked: &community::Checked,
) -> Result<(), AppError> {
    let data: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| AppError::validation("The map is not valid JSON."))?;
    let report = serde_json::to_value(ReportInfo::from(&checked.report))
        .map_err(|_| AppError::internal("The report could not be stored."))?;
    sqlx::query(
        "INSERT INTO game.community_map_versions (map_id, version, data, report, valid)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(version)
    .bind(Json(&data))
    .bind(Json(&report))
    .bind(checked.report.is_valid())
    .execute(&mut **tx)
    .await
    .map_err(db)?;
    Ok(())
}

/// The map row locked for its author: `(status, current_version)`; `None` when there is no
/// such map or it is not `author`'s.
async fn lock_own(
    tx: &mut Tx,
    author: Uuid,
    id: Uuid,
) -> Result<Option<(MapStatus, i32)>, AppError> {
    let row: Option<(String, i32)> = sqlx::query_as(
        "SELECT status, current_version FROM game.community_maps
         WHERE id = $1 AND author_account = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(author)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db)?;
    Ok(row.map(|(status, version)| {
        (
            MapStatus::from_key(&status).unwrap_or(MapStatus::Draft),
            version,
        )
    }))
}

/// Saves `text` as the next version of the author's map `id`, re-validated. A published map
/// goes back to draft. `map: None` (nothing stored) when the text is not a map at all.
///
/// # Errors
///
/// A 404 for a map that is not the author's; a conflict for a rejected map, the version limit,
/// or a request id used elsewhere; a database failure.
pub async fn update(
    pool: &PgPool,
    author: Uuid,
    id: Uuid,
    request_id: Option<Uuid>,
    text: &str,
) -> Result<Saved, AppError> {
    let checked = community::check(text);
    let Some(map) = checked.map.as_ref() else {
        // Not stored, but still the author's map or nobody's.
        let mut conn = pool.acquire().await.map_err(db)?;
        let owned: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM game.community_maps WHERE id = $1 AND author_account = $2",
        )
        .bind(id)
        .bind(author)
        .fetch_optional(&mut *conn)
        .await
        .map_err(db)?;
        return owned.map_or(Err(AppError::NotFound), |_| {
            Ok(Saved {
                response: report_response(&checked),
                unpublished: false,
            })
        });
    };
    let mut write = match begin(pool, author, request_id, "community_update").await? {
        Begin::Replay(response) => {
            return Ok(Saved {
                response: replayed(response)?,
                unpublished: false,
            });
        }
        Begin::Go(write) => write,
    };
    let (status, current) = lock_own(&mut write.tx, author, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if status == MapStatus::Rejected {
        return Err(AppError::Conflict(
            "A moderator rejected this map; it cannot be edited.".into(),
        ));
    }
    if u32::try_from(current).unwrap_or(u32::MAX) >= MAX_VERSIONS {
        return Err(AppError::Conflict(format!(
            "A map can have {MAX_VERSIONS} versions at most."
        )));
    }
    let next = current + 1;
    insert_version(&mut write.tx, id, next, text, &checked).await?;
    let listing = Listing::of(map);
    let unpublished = status == MapStatus::Published;
    sqlx::query(
        "UPDATE game.community_maps SET title = $2, description = $3, difficulty = $4,
            tags = $5, current_version = $6,
            status = CASE WHEN status = 'published' THEN 'draft' ELSE status END,
            updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(&listing.title)
    .bind(&listing.description)
    .bind(i16::from(listing.difficulty))
    .bind(&listing.tags)
    .bind(next)
    .execute(&mut *write.tx)
    .await
    .map_err(db)?;
    let detail = detail_of(&mut write.tx, id).await?;
    let response = SaveMapResponse {
        report: ReportInfo::from(&checked.report),
        map: detail,
    };
    write.commit(&response).await?;
    tracing::info!(account_id = %author, map_id = %id, version = next, valid = checked.report.is_valid(), "community map updated");
    Ok(Saved {
        response,
        unpublished,
    })
}

/// The dry run: the report for `text`, nothing stored.
#[must_use]
pub fn validate(text: &str) -> SaveMapResponse {
    report_response(&community::check(text))
}

/// What publishing did.
#[derive(Debug, Clone)]
pub enum Published {
    /// Now (or already) published: the map, and the text of the live version for the realm.
    Live { detail: Box<MapDetail>, data: String },
    /// The stored version does not validate; it was not published.
    Invalid(ReportInfo),
}

/// Publishes the author's map: the current version is validated again from the stored text and
/// must have no errors. Publishing a published map is a no-op that returns it.
///
/// # Errors
///
/// A 404 for a map that is not the author's; a conflict for a hidden or rejected map or a
/// request id used elsewhere; a database failure.
pub async fn publish(
    pool: &PgPool,
    author: Uuid,
    id: Uuid,
    request_id: Option<Uuid>,
) -> Result<Published, AppError> {
    let mut write = match begin(pool, author, request_id, "community_publish").await? {
        Begin::Replay(response) => {
            let detail: MapDetail = replayed(response)?;
            let mut conn = pool.acquire().await.map_err(db)?;
            let data = version_text(&mut conn, id, detail.summary.version)
                .await?
                .unwrap_or_default();
            return Ok(Published::Live {
                detail: Box::new(detail),
                data,
            });
        }
        Begin::Go(write) => write,
    };
    let (status, current) = lock_own(&mut write.tx, author, id)
        .await?
        .ok_or(AppError::NotFound)?;
    match status {
        MapStatus::Hidden | MapStatus::Rejected => {
            return Err(AppError::Conflict(
                "A moderator has taken this map down; it cannot be published.".into(),
            ));
        }
        MapStatus::Draft | MapStatus::Published => {}
    }
    let version = u32::try_from(current).unwrap_or(1);
    let text = version_text(&mut write.tx, id, version)
        .await?
        .ok_or(AppError::NotFound)?;
    // Never trust the stored verdict: check the text again.
    let checked = community::check(&text);
    if !checked.report.is_valid() || checked.map.is_none() {
        return Ok(Published::Invalid(ReportInfo::from(&checked.report)));
    }
    if status == MapStatus::Draft {
        sqlx::query(
            "UPDATE game.community_maps SET status = 'published', updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .execute(&mut *write.tx)
        .await
        .map_err(db)?;
        tracing::info!(account_id = %author, map_id = %id, version, "community map published");
    }
    let detail = detail_of(&mut write.tx, id)
        .await?
        .ok_or(AppError::NotFound)?;
    write.commit(&detail).await?;
    Ok(Published::Live {
        detail: Box::new(detail),
        data: text,
    })
}

/// Takes the author's published map back to draft. Unpublishing a draft is a no-op.
///
/// # Errors
///
/// A 404 for a map that is not the author's; a conflict for a request id used elsewhere; a
/// database failure.
pub async fn unpublish(
    pool: &PgPool,
    author: Uuid,
    id: Uuid,
    request_id: Option<Uuid>,
) -> Result<MapDetail, AppError> {
    let mut write = match begin(pool, author, request_id, "community_unpublish").await? {
        Begin::Replay(response) => return replayed(response),
        Begin::Go(write) => write,
    };
    let (status, _) = lock_own(&mut write.tx, author, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if status == MapStatus::Published {
        sqlx::query(
            "UPDATE game.community_maps SET status = 'draft', updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .execute(&mut *write.tx)
        .await
        .map_err(db)?;
        tracing::info!(account_id = %author, map_id = %id, "community map unpublished");
    }
    let detail = detail_of(&mut write.tx, id)
        .await?
        .ok_or(AppError::NotFound)?;
    write.commit(&detail).await?;
    Ok(detail)
}

/// A moderator's action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moderation {
    /// Take a map down: it stays editable by its author but cannot be published.
    Hide,
    /// Refuse a map for good: no edits, no publishing.
    Reject,
    /// Return a hidden or rejected map to draft.
    Restore,
}

impl Moderation {
    fn kind(self) -> &'static str {
        match self {
            Self::Hide => "community_hide",
            Self::Reject => "community_reject",
            Self::Restore => "community_restore",
        }
    }
}

/// What a moderation call did: the map after, and whether it was live before (its entries must
/// leave the realm).
#[derive(Debug, Clone)]
pub struct Moderated {
    /// The map after the action.
    pub detail: MapDetail,
    /// It was published before the action.
    pub was_live: bool,
}

/// Applies a moderator's `action`. The caller has checked the `community.moderate` permission.
///
/// # Errors
///
/// A 404 for no such map; a conflict for an action the map's status does not allow or a
/// request id used elsewhere; a database failure.
pub async fn moderate(
    pool: &PgPool,
    moderator: Uuid,
    id: Uuid,
    action: Moderation,
    request: &MapActionRequest,
    request_id: Option<Uuid>,
) -> Result<Moderated, AppError> {
    let mut write = match begin(pool, moderator, request_id, action.kind()).await? {
        Begin::Replay(response) => {
            return Ok(Moderated {
                detail: replayed(response)?,
                was_live: false,
            });
        }
        Begin::Go(write) => write,
    };
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM game.community_maps WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *write.tx)
            .await
            .map_err(db)?;
    let status = status
        .and_then(|s| MapStatus::from_key(&s))
        .ok_or(AppError::NotFound)?;
    let next = match (action, status) {
        (Moderation::Hide, MapStatus::Draft | MapStatus::Published) => MapStatus::Hidden,
        (Moderation::Hide, MapStatus::Hidden) => MapStatus::Hidden,
        (Moderation::Reject, MapStatus::Rejected) => MapStatus::Rejected,
        (Moderation::Reject, _) => MapStatus::Rejected,
        (Moderation::Restore, MapStatus::Hidden | MapStatus::Rejected) => MapStatus::Draft,
        (Moderation::Hide, MapStatus::Rejected) => {
            return Err(AppError::Conflict("That map is already rejected.".into()));
        }
        (Moderation::Restore, _) => {
            return Err(AppError::Conflict(
                "Only a hidden or rejected map can be restored.".into(),
            ));
        }
    };
    let note = request
        .reason
        .as_deref()
        .map(|r| r.chars().filter(|c| !c.is_control()).take(300).collect::<String>());
    sqlx::query(
        "UPDATE game.community_maps SET status = $2, moderation_note = $3, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(next.key())
    .bind(note)
    .execute(&mut *write.tx)
    .await
    .map_err(db)?;
    let detail = detail_of(&mut write.tx, id)
        .await?
        .ok_or(AppError::NotFound)?;
    write.commit(&detail).await?;
    tracing::info!(moderator = %moderator, map_id = %id, status = next.key(), "community map moderated");
    Ok(Moderated {
        detail,
        was_live: status == MapStatus::Published,
    })
}

// ---------------------------------------------------------------------------------------
// Playing
// ---------------------------------------------------------------------------------------

/// A published map's live version, for the realm.
#[derive(Debug, Clone)]
pub struct Playable {
    /// The map.
    pub summary: MapSummary,
    /// The version text the realm is given.
    pub data: String,
}

/// The map `id`'s live version, if it is published and `version` is that version.
///
/// # Errors
///
/// The database failed.
pub async fn playable(pool: &PgPool, id: Uuid, version: u32) -> Result<Option<Playable>, AppError> {
    let mut conn = pool.acquire().await.map_err(db)?;
    let Some(summary) = summary_of(&mut conn, id).await? else {
        return Ok(None);
    };
    if summary.status != MapStatus::Published || summary.version != version || !summary.valid {
        return Ok(None);
    }
    let Some(data) = version_text(&mut conn, id, version).await? else {
        return Ok(None);
    };
    Ok(Some(Playable { summary, data }))
}

/// Every published map and its live version: what the realm should hold.
///
/// # Errors
///
/// The database failed.
pub async fn published(pool: &PgPool) -> Result<Vec<(Uuid, u32)>, AppError> {
    let rows: Vec<(Uuid, i32)> = sqlx::query_as(
        "SELECT id, current_version FROM game.community_maps WHERE status = 'published'",
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;
    Ok(rows
        .into_iter()
        .map(|(id, version)| (id, u32::try_from(version).unwrap_or(1)))
        .collect())
}

/// Counts one play on the map behind board id `cm:<id>`; called with the transaction that
/// first wrote the run's row, so a redelivered result counts nothing.
pub(crate) async fn count_play(tx: &mut Tx, board_id: &str) -> Result<(), AppError> {
    let Some(id) = community::map_of_board(board_id).and_then(|id| Uuid::parse_str(id).ok())
    else {
        return Ok(());
    };
    sqlx::query("UPDATE game.community_maps SET plays = plays + 1 WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(())
}
