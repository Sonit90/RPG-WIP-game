//! Integration tests for run history and leaderboards against a live Postgres.
//!
//! Requires `DATABASE_URL` with the migrations applied. Marked `#[ignore]` so default
//! `cargo test` runs stay hermetic. Run explicitly:
//!     cargo test -p realm-game -- --ignored

use engine_realm::api::{DungeonResult, LeaderboardRow, LootDelivery, RunMember};
use engine_realm::leaderboard::{Board, LeaderboardConfig, RunScore};
use engine_realm::outcome::RunOutcome;
use realm_game::{history, store};
use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    PgPool::connect(&url).await.expect("connect")
}

struct Player {
    account: Uuid,
    character: Uuid,
}

async fn player(pool: &PgPool) -> Player {
    let account: Uuid = sqlx::query_scalar(
        "INSERT INTO auth.users (email, role_id)
         SELECT $1, id FROM auth.roles WHERE name = 'user' RETURNING id",
    )
    .bind(format!("history-{}@test.local", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .expect("create account");
    let rows = store::list_or_provision(pool, account)
        .await
        .expect("provision");
    Player {
        account,
        character: rows.into_iter().next().expect("a character").id,
    }
}

/// A dungeon id no other test uses, so boards read only this test's runs.
fn private_dungeon() -> String {
    format!("history-{}", Uuid::new_v4())
}

fn run_uid() -> String {
    Uuid::new_v4().simple().to_string()
}

/// One player's result of the run `uid`, carrying `party`.
fn run_result(
    uid: &str,
    who: &Player,
    party: &[&Player],
    dungeon: &str,
    key: u32,
    outcome: RunOutcome,
    duration_us: i64,
) -> DungeonResult {
    DungeonResult {
        result_id: format!("{uid}:{}", who.account),
        run_uid: uid.to_owned(),
        account_id: who.account.to_string(),
        character_id: who.character.to_string(),
        dungeon_id: dungeon.to_owned(),
        success: outcome.is_success(),
        duration_us,
        experience: 0,
        key_level: key,
        key_owner: false,
        key_level_after: 0,
        resources: vec![],
        damage_report: Some(engine_realm::meter::DamageReport {
            party: vec![],
            total_damage: 1000,
            dps: 12.5,
            duration_seconds: 80.0,
            boss_kill_seconds: None,
            abilities: vec![],
            healing: 40,
            damage_taken: 300,
            deaths: 2,
        }),
        talent_points: 0,
        looted: vec![],
        currency_spent: vec![],
        participant_key_bump: false,
        own_key_level: 0,
        outcome: Some(outcome),
        timer_us: 2_000_000,
        party: party
            .iter()
            .map(|p| RunMember {
                character_id: p.character.to_string(),
                name: "Ash".into(),
                hero: "Fire Mage".into(),
                role: "dps".into(),
            })
            .collect(),
        modifiers: vec![],
    }
}

async fn apply_run(
    pool: &PgPool,
    uid: &str,
    players: &[&Player],
    dungeon: &str,
    key: u32,
    outcome: RunOutcome,
    duration_us: i64,
) {
    for who in players {
        let result = run_result(uid, who, players, dungeon, key, outcome, duration_us);
        store::apply_result(pool, &result).await.expect("apply");
    }
}

async fn runs_named(pool: &PgPool, uid: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM game.dungeon_runs WHERE run_uid = $1")
        .bind(uid)
        .fetch_one(pool)
        .await
        .expect("count")
}

async fn clean(pool: &PgPool, players: &[&Player], uids: &[String]) {
    for who in players {
        sqlx::query("DELETE FROM auth.users WHERE id = $1")
            .bind(who.account)
            .execute(pool)
            .await
            .expect("delete account");
    }
    for uid in uids {
        sqlx::query("DELETE FROM game.dungeon_runs WHERE run_uid = $1")
            .bind(uid)
            .execute(pool)
            .await
            .expect("delete run");
    }
}

#[tokio::test]
#[ignore]
async fn three_results_of_one_run_write_one_run_row_and_each_a_history_entry() {
    let pool = pool().await;
    let (a, b, c) = (player(&pool).await, player(&pool).await, player(&pool).await);
    let (uid, dungeon) = (run_uid(), private_dungeon());
    apply_run(&pool, &uid, &[&a, &b, &c], &dungeon, 4, RunOutcome::Timed, 1_000_000).await;
    assert_eq!(runs_named(&pool, &uid).await, 1);
    let results: i64 =
        sqlx::query_scalar("SELECT count(*) FROM game.dungeon_results WHERE run_uid = $1")
            .bind(&uid)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(results, 3);
    let page = history::history(&pool, b.character, None, 50)
        .await
        .expect("history");
    assert_eq!(page.runs.len(), 1);
    let entry = &page.runs[0];
    assert_eq!((entry.run_uid.as_str(), entry.key_level), (uid.as_str(), 4));
    assert_eq!(entry.outcome, RunOutcome::Timed);
    assert_eq!(entry.party.len(), 3);
    assert_eq!(
        (entry.deaths, entry.damage_taken, entry.healing),
        (2, 300, 40)
    );
    assert!((entry.dps - 12.5).abs() < 0.01);
    // A participant reads the run; a stranger gets nothing.
    let detail = history::run_detail(&pool, c.account, &uid)
        .await
        .expect("detail");
    assert_eq!(detail.expect("a participant reads it").party.len(), 3);
    let stranger = player(&pool).await;
    assert!(history::run_detail(&pool, stranger.account, &uid)
        .await
        .expect("detail")
        .is_none());
    clean(&pool, &[&a, &b, &c, &stranger], &[uid]).await;
}

#[tokio::test]
#[ignore]
async fn re_applying_a_result_changes_nothing() {
    let pool = pool().await;
    let a = player(&pool).await;
    let (uid, dungeon) = (run_uid(), private_dungeon());
    let result = run_result(&uid, &a, &[&a], &dungeon, 2, RunOutcome::Overtime, 3_000_000);
    assert!(store::apply_result(&pool, &result).await.expect("first"));
    assert!(!store::apply_result(&pool, &result).await.expect("second"));
    assert_eq!(runs_named(&pool, &uid).await, 1);
    let page = history::history(&pool, a.character, None, 50)
        .await
        .expect("history");
    assert_eq!(page.runs.len(), 1);
    clean(&pool, &[&a], &[uid]).await;
}

#[tokio::test]
#[ignore]
async fn history_pages_newest_first_with_a_cursor() {
    let pool = pool().await;
    let a = player(&pool).await;
    let dungeon = private_dungeon();
    let mut uids = Vec::new();
    for key in 1..=5 {
        let uid = run_uid();
        apply_run(&pool, &uid, &[&a], &dungeon, key, RunOutcome::Timed, 1_000_000).await;
        uids.push(uid);
    }
    let keys = |page: &engine_realm::api::HistoryPage| {
        page.runs.iter().map(|r| r.key_level).collect::<Vec<_>>()
    };
    let first = history::history(&pool, a.character, None, 2).await.expect("1");
    assert_eq!(keys(&first), [5, 4]);
    let second = history::history(&pool, a.character, first.next_before, 2)
        .await
        .expect("2");
    assert_eq!(keys(&second), [3, 2]);
    let third = history::history(&pool, a.character, second.next_before, 2)
        .await
        .expect("3");
    assert_eq!(keys(&third), [1]);
    assert_eq!(third.next_before, None);
    clean(&pool, &[&a], &uids).await;
}

#[tokio::test]
#[ignore]
async fn a_leaver_writes_no_run_and_the_party_result_still_does() {
    let pool = pool().await;
    let (a, b) = (player(&pool).await, player(&pool).await);
    let (uid, dungeon) = (run_uid(), private_dungeon());
    let mut leaver = run_result(&uid, &b, &[&a, &b], &dungeon, 3, RunOutcome::Abandoned, 500_000);
    leaver.party.clear();
    store::apply_result(&pool, &leaver).await.expect("leaver");
    assert_eq!(runs_named(&pool, &uid).await, 0);
    let stayer = run_result(&uid, &a, &[&a], &dungeon, 3, RunOutcome::Timed, 1_000_000);
    store::apply_result(&pool, &stayer).await.expect("stayer");
    assert_eq!(runs_named(&pool, &uid).await, 1);
    let mine = history::history(&pool, b.character, None, 10)
        .await
        .expect("history");
    assert_eq!(mine.runs[0].outcome, RunOutcome::Abandoned);
    assert_eq!(mine.runs[0].party.len(), 1, "the run's party, joined on read");
    clean(&pool, &[&a, &b], &[uid]).await;
}

#[tokio::test]
#[ignore]
async fn boards_rank_only_completions_in_the_documented_order() {
    let pool = pool().await;
    let a = player(&pool).await;
    let dungeon = private_dungeon();
    let config = LeaderboardConfig::builtin().expect("config");
    let runs = [
        (5, RunOutcome::Overtime, 900_000),
        (7, RunOutcome::Failed, 100_000),
        (7, RunOutcome::Abandoned, 100_000),
        (5, RunOutcome::Timed, 800_000),
        (5, RunOutcome::Timed, 700_000),
        (6, RunOutcome::Overtime, 950_000),
    ];
    let mut uids = Vec::new();
    for (key, outcome, duration) in runs {
        let uid = run_uid();
        apply_run(&pool, &uid, &[&a], &dungeon, key, outcome, duration).await;
        uids.push(uid);
    }
    let read = |board, key_level| {
        let (pool, config, dungeon) = (pool.clone(), config.clone(), dungeon.clone());
        async move {
            history::leaderboard(
                &pool,
                &config,
                &history::BoardQuery {
                    board,
                    dungeon_id: &dungeon,
                    key_level,
                    season_id: "season-0",
                    page: 0,
                },
            )
            .await
            .expect("board")
        }
    };
    let highest = read(Board::Highest, None).await;
    let shape: Vec<(u32, RunOutcome, i64)> = highest
        .rows
        .iter()
        .map(|r| (r.key_level, r.outcome, r.duration_us))
        .collect();
    assert_eq!(
        shape,
        [
            (6, RunOutcome::Overtime, 950_000),
            (5, RunOutcome::Timed, 700_000),
            (5, RunOutcome::Timed, 800_000),
            (5, RunOutcome::Overtime, 900_000),
        ],
        "failed and abandoned runs never rank"
    );
    assert_eq!(
        highest.rows.iter().map(|r| r.rank).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    let fastest = read(Board::Fastest, Some(5)).await;
    assert_eq!(
        fastest.rows.iter().map(|r| r.duration_us).collect::<Vec<_>>(),
        [700_000, 800_000, 900_000]
    );
    // The SQL order agrees with the rule's own comparator.
    let score = |r: &LeaderboardRow| RunScore {
        key_level: r.key_level,
        outcome: r.outcome,
        duration_us: r.duration_us,
        finished_at_us: r.finished_at_us,
        run_uid: r.run_uid.clone(),
    };
    let mut by_rule = highest.rows.clone();
    by_rule.sort_by(|x, y| config.score().compare(Board::Highest, &score(x), &score(y)));
    assert_eq!(by_rule, highest.rows);
    clean(&pool, &[&a], &uids).await;
}

#[tokio::test]
#[ignore]
async fn delivered_loot_lands_on_its_result_whichever_arrives_first() {
    let pool = pool().await;
    let (a, b) = (player(&pool).await, player(&pool).await);
    let (uid, dungeon) = (run_uid(), private_dungeon());
    let delivery = |who: &Player| LootDelivery {
        loot_id: format!("{uid}:{}:loot:0", who.account),
        account_id: who.account.to_string(),
        character_id: who.character.to_string(),
        item: "ashgraves_seal".into(),
        kind: "bag".into(),
        reason: String::new(),
        quality: 104,
        enhancement: 2,
        broken: false,
    };
    engine_realm::loot::ensure_items_registered();
    let result = |who: &Player| run_result(&uid, who, &[&a, &b], &dungeon, 1, RunOutcome::Timed, 1);
    // `a`: result then loot. `b`: loot then result.
    store::apply_result(&pool, &result(&a)).await.expect("a result");
    store::apply_loot(&pool, &delivery(&a)).await.expect("a loot");
    store::apply_loot(&pool, &delivery(&b)).await.expect("b loot");
    store::apply_result(&pool, &result(&b)).await.expect("b result");
    for who in [&a, &b] {
        let page = history::history(&pool, who.character, None, 10)
            .await
            .expect("history");
        let loot = &page.runs[0].loot;
        assert_eq!(loot.len(), 1, "exactly once");
        assert_eq!(
            (loot[0].item.as_str(), loot[0].quality, loot[0].sharpening),
            ("ashgraves_seal", 104, 2)
        );
    }
    clean(&pool, &[&a, &b], &[uid]).await;
}
