//! Integration tests for dungeon keys, result application and salvage against a live Postgres.
//!
//! Requires `DATABASE_URL` with the migrations applied. Marked `#[ignore]` so default
//! `cargo test` runs stay hermetic. Run explicitly:
//!     cargo test -p realm-game -- --ignored

use engine_realm::api::{DungeonResult, LootDelivery};
use engine_realm::loadout::add_to_bag;
use realm_game::store;
use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    PgPool::connect(&url).await.expect("connect")
}

async fn new_account(pool: &PgPool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO auth.users (email, role_id)
         SELECT $1, id FROM auth.roles WHERE name = 'user' RETURNING id",
    )
    .bind(format!("keys-{}@test.local", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .expect("create account")
}

/// Deleting the account cascades to its characters, keys and completions.
async fn drop_account(pool: &PgPool, account: Uuid) {
    sqlx::query("DELETE FROM auth.users WHERE id = $1")
        .bind(account)
        .execute(pool)
        .await
        .expect("delete account");
}

/// The account's first character and its key.
async fn first_character(pool: &PgPool, account: Uuid) -> (store::CharacterRow, store::KeyRow) {
    let rows = store::list_or_provision(pool, account)
        .await
        .expect("provision");
    let row = rows.into_iter().next().expect("a starter character");
    let key = store::get_key(pool, account, row.id)
        .await
        .expect("get key")
        .expect("a provisioned key");
    (row, key)
}

/// A winning run by the key owner, paying 100 experience and 9 dust, one key level up.
fn winning(account: Uuid, character: Uuid, key: &store::KeyRow, tag: &str) -> DungeonResult {
    DungeonResult {
        result_id: format!("{tag}:{}", Uuid::new_v4()),
        account_id: account.to_string(),
        character_id: character.to_string(),
        dungeon_id: key.dungeon_id.clone(),
        success: true,
        duration_us: 1_000_000,
        experience: 100,
        key_level: key.level,
        key_owner: true,
        key_level_after: key.level + 1,
        resources: vec![("crypt_dust".into(), 9)],
    }
}

/// `(experience, crypt dust held, key level)` as stored right now.
async fn stored(pool: &PgPool, account: Uuid, character: Uuid) -> (i64, u64, u32) {
    let row = store::get(pool, account, character)
        .await
        .expect("get")
        .expect("exists");
    let key = store::get_key(pool, account, character)
        .await
        .expect("key")
        .expect("has key");
    (
        row.experience,
        row.loadout
            .0
            .resources
            .get("crypt_dust")
            .copied()
            .unwrap_or(0),
        key.level,
    )
}

#[tokio::test]
#[ignore]
async fn a_result_applied_twice_counts_once() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, key) = first_character(&pool, account).await;
    let run = winning(account, row.id, &key, "twice");

    assert!(store::apply_result(&pool, &run).await.expect("first"));
    let once = stored(&pool, account, row.id).await;
    assert!(!store::apply_result(&pool, &run).await.expect("second"));
    assert_eq!(stored(&pool, account, row.id).await, once);
    assert_eq!(once, (100, 9, key.level + 1));
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn the_key_advances_for_the_owner_on_success_only() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, key) = first_character(&pool, account).await;

    // Not the owner: the run pays, the key stays.
    let mut guest = winning(account, row.id, &key, "guest");
    guest.key_owner = false;
    guest.key_level_after = 0;
    assert!(store::apply_result(&pool, &guest).await.expect("guest"));
    assert_eq!(stored(&pool, account, row.id).await, (100, 9, key.level));

    // A failure: the realm reports nothing earned; the key stays.
    let mut failed = winning(account, row.id, &key, "fail");
    failed.success = false;
    failed.experience = 0;
    failed.resources = vec![];
    failed.key_level_after = key.level;
    assert!(store::apply_result(&pool, &failed).await.expect("fail"));
    assert_eq!(stored(&pool, account, row.id).await, (100, 9, key.level));
    // Even a failure that claims a higher level cannot move the key.
    let mut lying = winning(account, row.id, &key, "lying");
    lying.success = false;
    lying.experience = 0;
    lying.resources = vec![];
    assert!(store::apply_result(&pool, &lying).await.expect("lying"));
    assert_eq!(stored(&pool, account, row.id).await.2, key.level);

    // The owner on success: up one level.
    let win = winning(account, row.id, &key, "win");
    assert!(store::apply_result(&pool, &win).await.expect("win"));
    assert_eq!(
        stored(&pool, account, row.id).await,
        (200, 18, key.level + 1)
    );
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn a_stale_result_does_not_move_the_key() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, key) = first_character(&pool, account).await;
    // Played at a level the key is no longer at.
    let mut stale = winning(account, row.id, &key, "stale");
    stale.key_level = key.level + 5;
    stale.key_level_after = key.level + 6;
    assert!(store::apply_result(&pool, &stale).await.expect("stale"));
    assert_eq!(stored(&pool, account, row.id).await.2, key.level);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn listing_provisions_one_key_per_character_once() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let first = store::list_or_provision(&pool, account)
        .await
        .expect("list");
    let again = store::list_or_provision(&pool, account)
        .await
        .expect("relist");
    assert_eq!(first.len(), again.len());
    let keys = store::keys_of(&pool, account).await.expect("keys");
    assert_eq!(keys.len(), first.len());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM game.dungeon_keys WHERE account_id = $1")
            .bind(account)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(usize::try_from(count).expect("small"), first.len());
    let dungeon = realm_game::catalog().dungeons().first().expect("a dungeon");
    for key in keys.values() {
        assert_eq!(key.dungeon_id, dungeon.id);
        assert_eq!(key.level, dungeon.keys.min_level.max(1));
    }
    // Another account cannot read this account's key.
    let other = new_account(&pool).await;
    assert!(store::get_key(&pool, other, first[0].id)
        .await
        .expect("get")
        .is_none());
    drop_account(&pool, other).await;
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn salvage_persists_resources_and_removes_the_item() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, _) = first_character(&pool, account).await;
    let item = engine_game::character::loot_pool()[0].key;
    let cell = std::cell::Cell::new(0);
    store::update_loadout(&pool, account, row.id, |current| {
        let mut loadout = current.clone();
        cell.set(add_to_bag(&mut loadout, item).expect("room"));
        Ok(loadout)
    })
    .await
    .expect("stock")
    .expect("owned");

    let economy = realm_game::catalog().economy();
    store::update_loadout(&pool, account, row.id, |current| {
        let mut loadout = current.clone();
        engine_realm::economy::salvage_many(&mut loadout, &[cell.get()], economy)
            .map_err(|e| generic_auth_core::AppError::BadRequest(e.to_string()))?;
        Ok(loadout)
    })
    .await
    .expect("salvage")
    .expect("owned");

    let after = store::get(&pool, account, row.id)
        .await
        .expect("get")
        .expect("row");
    assert!(after
        .loadout
        .0
        .inventory
        .iter()
        .all(|entry| entry.item != item));
    let held = after
        .loadout
        .0
        .resources
        .get(&economy.salvage.resource)
        .copied()
        .unwrap_or(0);
    assert!(held > 0);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn character_key_and_loot_survive_a_refetch() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, key) = first_character(&pool, account).await;
    let run = winning(account, row.id, &key, "refetch");
    store::apply_result(&pool, &run).await.expect("result");
    let item = engine_game::character::loot_pool()[0].key;
    let delivery = LootDelivery {
        loot_id: format!("loot:{}", Uuid::new_v4()),
        account_id: account.to_string(),
        character_id: row.id.to_string(),
        item: item.into(),
        kind: "bag".into(),
        reason: String::new(),
    };
    assert!(store::apply_loot(&pool, &delivery).await.expect("loot"));

    // Reconnect = refetch everything from the database.
    let again = store::get(&pool, account, row.id)
        .await
        .expect("get")
        .expect("row");
    let key_again = store::get_key(&pool, account, row.id)
        .await
        .expect("key")
        .expect("key");
    assert_eq!(key_again.level, key.level + 1);
    assert_eq!(again.loadout.0.resources.get("crypt_dust"), Some(&9));
    assert!(again
        .loadout
        .0
        .inventory
        .iter()
        .any(|entry| entry.item == item));
    drop_account(&pool, account).await;
}
