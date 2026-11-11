//! Integration tests for community maps against a live Postgres.
//!
//! Requires `DATABASE_URL` with the migrations applied. Marked `#[ignore]` so default
//! `cargo test` runs stay hermetic. Run explicitly:
//!     cargo test -p realm-game -- --ignored

use engine_realm::api::{DungeonResult, LootDelivery, RunMember};
use engine_realm::community::{
    self, MapActionRequest, MapSort, MapStatus, ReportInfo, SaveMapResponse,
};
use engine_realm::leaderboard::{Board, LeaderboardConfig};
use engine_realm::outcome::RunOutcome;
use generic_auth_core::AppError;
use realm_game::community::{
    create, get, list, moderate, playable, publish, unpublish, update, version_data, ListQuery,
    Moderation, Published, Viewer,
};
use realm_game::{history, store};
use sqlx::PgPool;
use uuid::Uuid;

const FIXTURE: &str = include_str!("../../../../crates/engine-realm/tests/fixtures/community/valid_small.json");

async fn pool() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    PgPool::connect(&url).await.expect("connect")
}

async fn account(pool: &PgPool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO auth.users (email, role_id)
         SELECT $1, id FROM auth.roles WHERE name = 'user' RETURNING id",
    )
    .bind(format!("community-{}@test.local", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .expect("create account")
}

async fn drop_account(pool: &PgPool, account: Uuid) {
    sqlx::query("DELETE FROM auth.users WHERE id = $1")
        .bind(account)
        .execute(pool)
        .await
        .expect("delete account");
}

fn viewer(account: Uuid) -> Viewer {
    Viewer {
        account,
        moderator: false,
    }
}

fn moderator(account: Uuid) -> Viewer {
    Viewer {
        account,
        moderator: true,
    }
}

/// The fixture with a distinct title (and optionally a claimed difficulty).
fn map_text(title: &str, difficulty: u8) -> String {
    let mut value: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture");
    value["meta"]["title"] = title.into();
    value["meta"]["difficulty"] = difficulty.into();
    value.to_string()
}

fn invalid_text(title: &str) -> String {
    map_text(title, 2).replace("crypt_floor", "no_such_asset")
}

fn query(author: Option<Uuid>, mine: bool, sort: MapSort) -> ListQuery {
    ListQuery {
        search: None,
        author,
        mine,
        sort,
        difficulty: None,
        status: None,
        page: 0,
    }
}

async fn make(pool: &PgPool, author: Uuid, title: &str) -> engine_realm::community::MapDetail {
    create(pool, author, None, &map_text(title, 2))
        .await
        .expect("create")
        .response
        .map
        .expect("stored")
}

fn id_of(detail: &engine_realm::community::MapDetail) -> Uuid {
    Uuid::parse_str(&detail.summary.id).expect("uuid")
}

#[tokio::test]
#[ignore]
async fn create_update_and_versions() {
    let pool = pool().await;
    let author = account(&pool).await;
    let saved = create(&pool, author, None, &map_text("First Crypt", 2))
        .await
        .expect("create");
    let detail = saved.response.map.expect("stored");
    assert_eq!(detail.summary.status, MapStatus::Draft);
    assert_eq!(detail.summary.version, 1);
    assert!(detail.summary.valid && detail.report.is_valid());
    assert_eq!(detail.summary.title, "First Crypt");
    let id = id_of(&detail);

    // An update is a new version, with the listing from the new meta.
    let saved = update(&pool, author, id, None, &map_text("Second Crypt", 4))
        .await
        .expect("update");
    let detail = saved.response.map.expect("stored");
    assert_eq!((detail.summary.version, detail.versions.len()), (2, 2));
    assert_eq!((detail.summary.title.as_str(), detail.summary.difficulty), ("Second Crypt", 4));
    // Both versions are kept, and version 1 still reads as it was.
    let v1 = version_data(&pool, viewer(author), id, 1)
        .await
        .expect("v1")
        .expect("exists");
    assert!(v1.contains("First Crypt"));
    assert!(version_data(&pool, viewer(author), id, 3).await.expect("v3").is_none());

    // An invalid map is stored as a draft with its report; it is not an error.
    let saved = update(&pool, author, id, None, &invalid_text("Broken Crypt"))
        .await
        .expect("update invalid");
    let detail = saved.response.map.expect("stored as a draft");
    assert!(!detail.summary.valid);
    assert!(detail.report.errors.iter().any(|i| i.code == "missing_asset"));
    assert_eq!(detail.summary.version, 3);

    // Text that is not a map at all stores nothing.
    let saved = create(&pool, author, None, "{ nope").await.expect("not a map");
    assert!(saved.response.map.is_none());
    assert!(saved.response.report.errors.iter().any(|i| i.code == "malformed_json"));
    let saved = create(&pool, author, None, &map_text("Water", 2).replacen("\"objects\"", "\"water\": [], \"objects\"", 1))
        .await
        .expect("water");
    let detail = saved.response.map.expect("typed parse works; water is an error in the report");
    assert!(!detail.summary.valid);
    assert!(detail.report.errors.iter().any(|i| i.code == "unsupported_feature"));
    drop_account(&pool, author).await;
}

#[tokio::test]
#[ignore]
async fn publish_is_refused_while_the_map_is_invalid_and_never_trusts_the_stored_verdict() {
    let pool = pool().await;
    let author = account(&pool).await;
    let bad = create(&pool, author, None, &invalid_text("Bad Crypt"))
        .await
        .expect("create")
        .response
        .map
        .expect("stored");
    match publish(&pool, author, id_of(&bad), None).await.expect("publish") {
        Published::Invalid(report) => {
            assert!(report.errors.iter().any(|i| i.code == "missing_asset"));
        }
        Published::Live { .. } => panic!("an invalid map must not publish"),
    }
    assert_eq!(
        get(&pool, viewer(author), id_of(&bad)).await.expect("get").expect("exists").summary.status,
        MapStatus::Draft
    );
    // Forge the stored verdict: the text is checked again.
    sqlx::query("UPDATE game.community_map_versions SET valid = true, report = '{}'::jsonb WHERE map_id = $1")
        .bind(id_of(&bad))
        .execute(&pool)
        .await
        .expect("forge");
    assert!(matches!(
        publish(&pool, author, id_of(&bad), None).await.expect("publish"),
        Published::Invalid(_)
    ));

    let good = make(&pool, author, "Good Crypt").await;
    match publish(&pool, author, id_of(&good), None).await.expect("publish") {
        Published::Live { detail, data } => {
            assert_eq!(detail.summary.status, MapStatus::Published);
            assert_eq!(detail.dungeon_id, community::dungeon_id(&good.summary.id, 1));
            assert!(community::check(&data).report.is_valid());
        }
        Published::Invalid(report) => panic!("{report:?}"),
    }
    // Publishing twice is a no-op, and the live version is playable.
    assert!(matches!(
        publish(&pool, author, id_of(&good), None).await.expect("again"),
        Published::Live { .. }
    ));
    assert!(playable(&pool, id_of(&good), 1).await.expect("playable").is_some());
    assert!(playable(&pool, id_of(&good), 2).await.expect("other version").is_none());
    // Editing a published map returns it to draft: it is no longer playable.
    let saved = update(&pool, author, id_of(&good), None, &map_text("Good Crypt 2", 2))
        .await
        .expect("update");
    assert!(saved.unpublished);
    assert!(playable(&pool, id_of(&good), 1).await.expect("gone").is_none());
    drop_account(&pool, author).await;
}

#[tokio::test]
#[ignore]
async fn only_the_author_can_edit_or_publish() {
    let pool = pool().await;
    let (author, other) = (account(&pool).await, account(&pool).await);
    let map = make(&pool, author, "Mine Alone").await;
    let id = id_of(&map);
    let text = map_text("Hijacked", 3);
    assert!(matches!(update(&pool, other, id, None, &text).await, Err(AppError::NotFound)));
    assert!(matches!(publish(&pool, other, id, None).await, Err(AppError::NotFound)));
    assert!(matches!(unpublish(&pool, other, id, None).await, Err(AppError::NotFound)));
    // A non-map from a stranger does not reveal that the map exists, nor store anything.
    assert!(matches!(update(&pool, other, id, None, "{").await, Err(AppError::NotFound)));
    // A draft is invisible to others, visible to the author and to a moderator.
    assert!(get(&pool, viewer(other), id).await.expect("get").is_none());
    assert!(get(&pool, viewer(author), id).await.expect("get").is_some());
    assert!(get(&pool, moderator(other), id).await.expect("get").is_some());
    assert!(version_data(&pool, viewer(other), id, 1).await.expect("data").is_none());
    // After publishing, everyone reads it.
    publish(&pool, author, id, None).await.expect("publish");
    assert!(get(&pool, viewer(other), id).await.expect("get").is_some());
    assert!(version_data(&pool, viewer(other), id, 1).await.expect("data").is_some());
    // Still not editable by the stranger.
    assert!(matches!(unpublish(&pool, other, id, None).await, Err(AppError::NotFound)));
    drop_account(&pool, author).await;
    drop_account(&pool, other).await;
}

#[tokio::test]
#[ignore]
async fn moderation_hides_rejects_and_restores() {
    let pool = pool().await;
    let (author, staff) = (account(&pool).await, account(&pool).await);
    let map = make(&pool, author, "Moderated").await;
    let id = id_of(&map);
    publish(&pool, author, id, None).await.expect("publish");
    let note = MapActionRequest {
        request_id: None,
        reason: Some("spam".into()),
    };

    let hidden = moderate(&pool, staff, id, Moderation::Hide, &note, None).await.expect("hide");
    assert!(hidden.was_live);
    assert_eq!(hidden.detail.summary.status, MapStatus::Hidden);
    assert!(hidden.detail.dungeon_id.is_empty());
    assert!(playable(&pool, id, 1).await.expect("playable").is_none());
    // Others cannot see it; the author can; the author cannot publish it.
    assert!(get(&pool, viewer(staff), id).await.expect("get").is_none());
    assert!(get(&pool, viewer(author), id).await.expect("get").is_some());
    assert!(matches!(publish(&pool, author, id, None).await, Err(AppError::Conflict(_))));
    // The author can still fix it (a new version), and a moderator restores it to draft.
    update(&pool, author, id, None, &map_text("Moderated Fixed", 2)).await.expect("fix");
    let restored = moderate(&pool, staff, id, Moderation::Restore, &note, None).await.expect("restore");
    assert!(!restored.was_live);
    assert_eq!(restored.detail.summary.status, MapStatus::Draft);
    assert!(matches!(publish(&pool, author, id, None).await.expect("publish"), Published::Live { .. }));

    // Rejection is final for the author.
    let rejected = moderate(&pool, staff, id, Moderation::Reject, &note, None).await.expect("reject");
    assert!(rejected.was_live);
    assert_eq!(rejected.detail.summary.status, MapStatus::Rejected);
    assert!(matches!(update(&pool, author, id, None, &map_text("Nope", 2)).await, Err(AppError::Conflict(_))));
    assert!(matches!(publish(&pool, author, id, None).await, Err(AppError::Conflict(_))));
    // Only a hidden or rejected map restores.
    let draft = make(&pool, author, "Plain Draft").await;
    assert!(matches!(
        moderate(&pool, staff, id_of(&draft), Moderation::Restore, &note, None).await,
        Err(AppError::Conflict(_))
    ));
    // A moderator can list hidden/rejected maps by status.
    let mut q = query(Some(author), false, MapSort::Newest);
    q.status = Some(MapStatus::Rejected);
    let page = list(&pool, moderator(staff), &q).await.expect("list");
    assert!(page.maps.iter().any(|m| m.id == map.summary.id));
    // A non-moderator's status filter is ignored: they only ever get published maps.
    let page = list(&pool, viewer(account(&pool).await), &q).await.expect("list");
    assert!(page.maps.is_empty());
    drop_account(&pool, author).await;
    drop_account(&pool, staff).await;
}

#[tokio::test]
#[ignore]
async fn list_visibility_filters_and_sorts() {
    let pool = pool().await;
    let (author, reader) = (account(&pool).await, account(&pool).await);
    let tag = Uuid::new_v4().simple().to_string();
    let titles: Vec<String> = ["Alpha", "Bravo", "Charlie", "Delta"]
        .iter()
        .map(|n| format!("{n} {}", &tag[..8]))
        .collect();
    let mut ids = Vec::new();
    for (index, title) in titles.iter().enumerate() {
        let difficulty = [3, 1, 5, 2][index];
        let saved = create(&pool, author, None, &map_text(title, difficulty)).await.expect("create");
        ids.push(id_of(&saved.response.map.expect("stored")));
    }
    // Publish three of the four; the fourth stays a draft.
    for id in &ids[..3] {
        publish(&pool, author, *id, None).await.expect("publish");
    }
    // The reader sees the published three, filtered to this test's author.
    let page = list(&pool, viewer(reader), &query(Some(author), false, MapSort::Newest)).await.expect("list");
    assert_eq!(page.maps.len(), 3);
    assert!(page.maps.iter().all(|m| m.status == MapStatus::Published));
    // The author's own list has all four.
    let mine = list(&pool, viewer(author), &query(None, true, MapSort::Newest)).await.expect("mine");
    assert_eq!(mine.maps.len(), 4);
    // Newest first (created order reversed), among the published three.
    let order: Vec<&str> = page.maps.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(order, [titles[2].as_str(), titles[1].as_str(), titles[0].as_str()]);
    // Easiest and hardest.
    let easy = list(&pool, viewer(reader), &query(Some(author), false, MapSort::Difficulty)).await.expect("easy");
    assert_eq!(easy.maps.iter().map(|m| m.difficulty).collect::<Vec<_>>(), [1, 3, 5]);
    let hard = list(&pool, viewer(reader), &query(Some(author), false, MapSort::Hardest)).await.expect("hard");
    assert_eq!(hard.maps.iter().map(|m| m.difficulty).collect::<Vec<_>>(), [5, 3, 1]);
    // Search by title is case-insensitive and substring.
    let mut q = query(Some(author), false, MapSort::Newest);
    q.search = Some(format!("bRaVo {}", &tag[..8]));
    let found = list(&pool, viewer(reader), &q).await.expect("search");
    assert_eq!(found.maps.len(), 1);
    assert_eq!(found.maps[0].title, titles[1]);
    // LIKE wildcards are plain characters.
    q.search = Some("%".into());
    assert!(list(&pool, viewer(reader), &q).await.expect("percent").maps.is_empty());
    // Difficulty filter.
    q.search = None;
    q.difficulty = Some(5);
    assert_eq!(list(&pool, viewer(reader), &q).await.expect("d5").maps.len(), 1);

    // Plays and fastest come from the runs: give the maps plays and times.
    let board = |i: usize| format!("cm:{}", ids[i]);
    for (i, plays, best) in [(0usize, 1usize, 900_000_000_i64), (1, 3, 300_000_000), (2, 2, 600_000_000)] {
        for n in 0..plays {
            sqlx::query(
                "INSERT INTO game.dungeon_runs (run_uid, season_id, dungeon_id, key_level, outcome, duration_us)
                 VALUES ($1, 'season-0', $2, 1, 'timed', $3)",
            )
            .bind(Uuid::new_v4().simple().to_string())
            .bind(board(i))
            .bind(best + n as i64)
            .execute(&pool)
            .await
            .expect("run");
        }
        sqlx::query("UPDATE game.community_maps SET plays = $2 WHERE id = $1")
            .bind(ids[i])
            .bind(plays as i64)
            .execute(&pool)
            .await
            .expect("plays");
    }
    let by_plays = list(&pool, viewer(reader), &query(Some(author), false, MapSort::Plays)).await.expect("plays");
    assert_eq!(by_plays.maps.iter().map(|m| m.plays).collect::<Vec<_>>(), [3, 2, 1]);
    let by_speed = list(&pool, viewer(reader), &query(Some(author), false, MapSort::Fastest)).await.expect("fastest");
    assert_eq!(
        by_speed.maps.iter().map(|m| m.fastest_us).collect::<Vec<_>>(),
        [Some(300_000_000), Some(600_000_000), Some(900_000_000)]
    );
    assert_eq!(by_speed.maps[0].completions, 3);

    // Paging: twelve per page, has_more.
    assert!(!page.has_more);
    drop_account(&pool, author).await;
    drop_account(&pool, reader).await;
    for i in 0..4 {
        sqlx::query("DELETE FROM game.dungeon_runs WHERE dungeon_id = $1")
            .bind(board(i))
            .execute(&pool)
            .await
            .expect("clean runs");
    }
}

#[tokio::test]
#[ignore]
async fn request_ids_make_retries_idempotent() {
    let pool = pool().await;
    let (author, other) = (account(&pool).await, account(&pool).await);
    let rid = Uuid::new_v4();
    let text = map_text("Retried", 2);
    let first = create(&pool, author, Some(rid), &text).await.expect("create");
    let second = create(&pool, author, Some(rid), &text).await.expect("replay");
    let (a, b) = (first.response.map.expect("map"), second.response.map.expect("map"));
    assert_eq!(a.summary.id, b.summary.id, "a replayed create returns the first map");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM game.community_maps WHERE author_account = $1")
        .bind(author)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);

    // A retried update is one version.
    let id = id_of(&a);
    let rid2 = Uuid::new_v4();
    let text2 = map_text("Retried 2", 3);
    update(&pool, author, id, Some(rid2), &text2).await.expect("update");
    let replay = update(&pool, author, id, Some(rid2), &text2).await.expect("replay");
    assert_eq!(replay.response.map.expect("map").summary.version, 2);
    let versions: i64 = sqlx::query_scalar("SELECT count(*) FROM game.community_map_versions WHERE map_id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(versions, 2);

    // Publish and unpublish replay too: the replay returns the first answer and changes nothing.
    let rid3 = Uuid::new_v4();
    publish(&pool, author, id, Some(rid3)).await.expect("publish");
    unpublish(&pool, author, id, None).await.expect("unpublish");
    let replay = publish(&pool, author, id, Some(rid3)).await.expect("replayed publish");
    assert!(matches!(replay, Published::Live { .. }));
    assert_eq!(
        get(&pool, viewer(author), id).await.expect("get").expect("exists").summary.status,
        MapStatus::Draft,
        "the replay must not publish again"
    );

    // A request id is not shared across accounts or actions.
    assert!(matches!(
        create(&pool, other, Some(rid), &text).await,
        Err(AppError::Conflict(_))
    ));
    assert!(matches!(
        update(&pool, author, id, Some(rid), &text).await,
        Err(AppError::Conflict(_))
    ));
    drop_account(&pool, author).await;
    drop_account(&pool, other).await;
}

fn run_result(uid: &str, account: Uuid, character: Uuid, dungeon: &str, party: &[(Uuid, Uuid)]) -> DungeonResult {
    DungeonResult {
        result_id: format!("{uid}:{account}"),
        run_uid: uid.to_owned(),
        account_id: account.to_string(),
        character_id: character.to_string(),
        dungeon_id: dungeon.to_owned(),
        success: true,
        duration_us: 123_000_000,
        // Everything a normal run would pay, to prove none of it is applied.
        experience: 5000,
        key_level: 4,
        key_owner: true,
        key_level_after: 5,
        resources: vec![("crypt_dust".into(), 99)],
        damage_report: None,
        talent_points: 3,
        looted: vec![("gold".into(), 400), ("aether_shard".into(), 5)],
        currency_spent: vec![("aether_shard".into(), 1)],
        participant_key_bump: true,
        own_key_level: 2,
        outcome: Some(RunOutcome::Timed),
        timer_us: 0,
        party: party
            .iter()
            .map(|(_, c)| RunMember {
                character_id: c.to_string(),
                name: "Ash".into(),
                hero: "Fire Mage".into(),
                role: "dps".into(),
            })
            .collect(),
        modifiers: vec![],
    }
}

fn new_uid() -> String {
    Uuid::new_v4().simple().to_string()
}

#[tokio::test]
#[ignore]
async fn plays_count_once_per_run_however_many_results_or_redeliveries() {
    let pool = pool().await;
    let author = account(&pool).await;
    let (a, b) = (account(&pool).await, account(&pool).await);
    let (ca, cb) = (
        store::list_or_provision(&pool, a).await.expect("a")[0].id,
        store::list_or_provision(&pool, b).await.expect("b")[0].id,
    );
    let map = make(&pool, author, "Played").await;
    let dungeon = community::dungeon_id(&map.summary.id, 1);
    let party = [(a, ca), (b, cb)];
    let uid = new_uid();
    // Both members' results of one run, then both again (the bridge re-delivers).
    for _ in 0..2 {
        store::apply_result(&pool, &run_result(&uid, a, ca, &dungeon, &party)).await.expect("a");
        store::apply_result(&pool, &run_result(&uid, b, cb, &dungeon, &party)).await.expect("b");
    }
    let map_id = id_of(&map);
    let plays = |pool: PgPool| async move {
        get(&pool, viewer(author), map_id).await.expect("get").expect("exists").summary.plays
    };
    assert_eq!(plays(pool.clone()).await, 1);
    // A second run counts again; a leaver's result (no party) does not write a run row.
    let uid2 = new_uid();
    let solo = [(a, ca)];
    store::apply_result(&pool, &run_result(&uid2, a, ca, &dungeon, &solo)).await.expect("run 2");
    let leaver = DungeonResult { party: Vec::new(), ..run_result(&uid2, b, cb, &dungeon, &solo) };
    store::apply_result(&pool, &leaver).await.expect("leaver");
    assert_eq!(plays(pool.clone()).await, 2);

    // The run is on the map's board, keyed cm:<id> (not the versioned id), and the completions
    // board counts each character.
    let board = format!("cm:{}", map.summary.id);
    let config = LeaderboardConfig::builtin().expect("config");
    let highest = history::leaderboard(
        &pool,
        &config,
        &history::BoardQuery { board: Board::Highest, dungeon_id: &board, key_level: None, season_id: "season-0", page: 0 },
    )
    .await
    .expect("highest");
    assert_eq!(highest.rows.len(), 2);
    assert!(highest.rows.iter().all(|r| r.key_level == 4));
    let completions = history::leaderboard(
        &pool,
        &config,
        &history::BoardQuery { board: Board::Completions, dungeon_id: &board, key_level: None, season_id: "season-0", page: 0 },
    )
    .await
    .expect("completions");
    assert_eq!(completions.completions.len(), 2);
    assert_eq!(completions.completions[0].completions, 2);
    assert_eq!(completions.completions[0].rank, 1);
    assert_eq!(completions.completions[1].completions, 1);
    assert!(completions.rows.is_empty());
    let page = get(&pool, viewer(author), id_of(&map)).await.expect("get").expect("exists");
    assert_eq!(page.summary.completions, 2);
    assert_eq!(page.summary.fastest_us, Some(123_000_000));

    sqlx::query("DELETE FROM game.dungeon_runs WHERE dungeon_id = $1")
        .bind(&board)
        .execute(&pool)
        .await
        .expect("clean");
    for who in [author, a, b] {
        drop_account(&pool, who).await;
    }
}

#[tokio::test]
#[ignore]
async fn a_community_result_grants_no_loot_currency_experience_or_key_progress() {
    let pool = pool().await;
    let author = account(&pool).await;
    let player = account(&pool).await;
    let rows = store::list_or_provision(&pool, player).await.expect("provision");
    let character = rows[0].id;
    let key_before = store::get_key(&pool, player, character).await.expect("key").expect("a key");
    let before = store::get(&pool, player, character).await.expect("get").expect("exists");
    let map = make(&pool, author, "Grants Nothing").await;
    let dungeon = community::dungeon_id(&map.summary.id, 1);
    let uid = new_uid();

    let result = run_result(&uid, player, character, &dungeon, &[(player, character)]);
    assert!(store::apply_result(&pool, &result).await.expect("applied"));
    // A duplicate is a no-op.
    assert!(!store::apply_result(&pool, &result).await.expect("duplicate"));

    let after = store::get(&pool, player, character).await.expect("get").expect("exists");
    assert_eq!(after.level, before.level);
    assert_eq!(after.experience, before.experience, "no experience");
    assert_eq!(after.loadout.0.resources, before.loadout.0.resources, "no gold, dust or shards, and no debit");
    assert_eq!(
        serde_json::to_value(&after.loadout.0).expect("json"),
        serde_json::to_value(&before.loadout.0).expect("json"),
        "the loadout is untouched"
    );
    assert_eq!(after.talent_points_earned, before.talent_points_earned, "no talent points");
    let key_after = store::get_key(&pool, player, character).await.expect("key").expect("a key");
    assert_eq!(key_after.level, key_before.level, "no key progress");
    let counted: i64 = sqlx::query_scalar("SELECT count(*) FROM game.dungeon_completions WHERE character_id = $1")
        .bind(character)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(counted, 0, "no completion count");
    // The result row exists, zeroed, and the history lists the run.
    let (experience, resources): (i32, serde_json::Value) = sqlx::query_as(
        "SELECT experience, resources FROM game.dungeon_results WHERE result_id = $1",
    )
    .bind(&result.result_id)
    .fetch_one(&pool)
    .await
    .map(|(e, r): (i32, sqlx::types::Json<serde_json::Value>)| (e, r.0))
    .expect("row");
    assert_eq!((experience, resources), (0, serde_json::json!([])));
    let page = history::history(&pool, character, None, 10).await.expect("history");
    assert_eq!(page.runs.len(), 1);
    assert_eq!(page.runs[0].dungeon_id, dungeon);

    // Loot for the run is refused: a delivery naming the community result is not applied.
    let delivery = LootDelivery {
        loot_id: format!("{}:loot:0", result.result_id),
        account_id: player.to_string(),
        character_id: character.to_string(),
        item: "head_haste_critical_strike".into(),
        kind: "bag".into(),
        reason: String::new(),
        quality: 100,
        sharpening: 0,
        broken: false,
    };
    assert!(!store::apply_loot(&pool, &delivery).await.expect("loot"), "the delivery is refused");
    let final_row = store::get(&pool, player, character).await.expect("get").expect("exists");
    assert_eq!(
        serde_json::to_value(&final_row.loadout.0).expect("json"),
        serde_json::to_value(&before.loadout.0).expect("json"),
        "no item arrived"
    );

    // The same result for an official dungeon does grant (the control for this test).
    let mut official = run_result(&new_uid(), player, character, &key_before.dungeon_id, &[(player, character)]);
    official.key_level = key_before.level;
    official.key_level_after = key_before.level + 1;
    official.looted = vec![];
    official.currency_spent = vec![];
    official.participant_key_bump = false;
    assert!(store::apply_result(&pool, &official).await.expect("official"));
    let granted = store::get(&pool, player, character).await.expect("get").expect("exists");
    assert!(granted.experience > before.experience || granted.level > before.level);

    sqlx::query("DELETE FROM game.dungeon_runs WHERE dungeon_id = $1")
        .bind(format!("cm:{}", map.summary.id))
        .execute(&pool)
        .await
        .expect("clean");
    sqlx::query("DELETE FROM game.dungeon_runs WHERE run_uid = $1")
        .bind(&official.run_uid)
        .execute(&pool)
        .await
        .expect("clean");
    drop_account(&pool, author).await;
    drop_account(&pool, player).await;
}

#[tokio::test]
#[ignore]
async fn the_report_and_response_types_round_trip_through_the_stored_replay() {
    let pool = pool().await;
    let author = account(&pool).await;
    let rid = Uuid::new_v4();
    let saved = create(&pool, author, Some(rid), &invalid_text("Replay Report")).await.expect("create");
    let replay = create(&pool, author, Some(rid), &invalid_text("Replay Report")).await.expect("replay");
    let first: &SaveMapResponse = &saved.response;
    assert_eq!(first, &replay.response);
    let report: &ReportInfo = &first.report;
    assert!(!report.is_valid());
    drop_account(&pool, author).await;
}
