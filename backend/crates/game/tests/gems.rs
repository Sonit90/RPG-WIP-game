//! Integration tests for gem socketing, unsocketing and upgrading against a live Postgres.
//!
//! Requires `DATABASE_URL` with the migrations applied. Marked `#[ignore]` so default
//! `cargo test` runs stay hermetic. Run explicitly:
//!     cargo test -p realm-game -- --ignored

use engine_realm::api::{
    GemSlotResponse, GemSocketRequest, GemUnsocketRequest, GemUpgradeResponse,
};
use engine_realm::loadout::{BagItem, EquippedItem, Loadout, SlotRef};
use generic_auth_core::AppError;
use realm_game::store;
use sqlx::PgPool;
use uuid::Uuid;

const RING: &str = "ashgraves_seal";

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
    .bind(format!("gems-{}@test.local", Uuid::new_v4()))
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

fn ring() -> SlotRef {
    SlotRef::Equipment {
        slot: "ring1".into(),
    }
}

/// A character wearing the ring (socket 0 holding `socketed`), carrying `bag` as
/// `(cell, item, quantity)` and exactly `gold`.
async fn stocked(
    pool: &PgPool,
    account: Uuid,
    socketed: Option<&str>,
    bag: &[(u32, &str, u16)],
    gold: u64,
) -> Uuid {
    let rows = store::list_or_provision(pool, account)
        .await
        .expect("provision");
    let id = rows.into_iter().next().expect("a starter").id;
    engine_realm::loot::ensure_items_registered();
    let bag: Vec<(u32, String, u16)> = bag.iter().map(|(c, i, q)| (*c, (*i).into(), *q)).collect();
    let socketed = socketed.map(str::to_owned);
    store::update_loadout(pool, account, id, move |current| {
        let mut loadout: Loadout = current.clone();
        loadout.equipment.retain(|e| e.slot != "ring1");
        let mut worn = EquippedItem::new("ring1", RING);
        if let Some(gem) = &socketed {
            worn.gems = vec![gem.clone()];
        }
        loadout.equipment.push(worn);
        loadout.inventory.clear();
        for (cell, item, quantity) in &bag {
            loadout
                .inventory
                .push(BagItem::new(*cell, item, *quantity));
        }
        loadout.resources.remove("gold");
        if gold > 0 {
            loadout.resources.insert("gold".into(), gold);
        }
        Ok(loadout)
    })
    .await
    .expect("stock")
    .expect("owned");
    id
}

async fn loadout(pool: &PgPool, account: Uuid, id: Uuid) -> Loadout {
    store::get(pool, account, id)
        .await
        .expect("get")
        .expect("exists")
        .loadout
        .0
}

fn gold(loadout: &Loadout) -> u64 {
    loadout.resources.get("gold").copied().unwrap_or(0)
}

fn count(loadout: &Loadout, key: &str) -> u32 {
    loadout
        .inventory
        .iter()
        .filter(|e| e.item == key)
        .map(|e| u32::from(e.quantity))
        .sum()
}

async fn unlocked() -> Result<(), AppError> {
    Ok(())
}

async fn engaged() -> Result<(), AppError> {
    Err(AppError::Conflict(engine_realm::api::ENGAGED_MESSAGE.into()))
}

fn socket_request(request: Uuid, socket: u8, gem_slot: u32) -> GemSocketRequest {
    GemSocketRequest {
        request_id: request.to_string(),
        item_slot: ring(),
        socket,
        gem_slot,
    }
}

fn unsocket_request(request: Uuid, socket: u8) -> GemUnsocketRequest {
    GemUnsocketRequest {
        request_id: request.to_string(),
        item_slot: ring(),
        socket,
    }
}

async fn socket(
    pool: &PgPool,
    account: Uuid,
    id: Uuid,
    request: Uuid,
    socket_index: u8,
    gem_slot: u32,
) -> Result<GemSlotResponse, AppError> {
    let body = socket_request(request, socket_index, gem_slot);
    store::socket_gem(pool, account, id, request, &body, unlocked)
        .await
        .map(|r| r.expect("the account's character"))
}

async fn unsocket(
    pool: &PgPool,
    account: Uuid,
    id: Uuid,
    request: Uuid,
    socket_index: u8,
) -> Result<GemSlotResponse, AppError> {
    let body = unsocket_request(request, socket_index);
    store::unsocket_gem(pool, account, id, request, &body, unlocked)
        .await
        .map(|r| r.expect("the account's character"))
}

/// One upgrade rolling the lowest bits (always a success) or the highest (a failure).
async fn upgrade(
    pool: &PgPool,
    account: Uuid,
    id: Uuid,
    request: Uuid,
    gem_slot: u32,
    success: bool,
) -> Result<GemUpgradeResponse, AppError> {
    let bits = if success { 0 } else { u64::MAX };
    store::upgrade_gem(pool, account, id, request, gem_slot, &mut || bits, unlocked)
        .await
        .map(|r| r.expect("the account's character"))
}

async fn recorded(pool: &PgPool, request: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM game.hub_requests WHERE request_id = $1")
        .bind(request)
        .fetch_one(pool)
        .await
        .expect("count")
}

fn socketed(loadout: &Loadout) -> Vec<String> {
    loadout
        .equipment
        .iter()
        .find(|e| e.slot == "ring1")
        .expect("the ring")
        .gems
        .clone()
}

#[tokio::test]
#[ignore]
async fn socketing_consumes_one_gem_and_fills_the_socket() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let id = stocked(&pool, account, None, &[(3, "gem_haste_2", 2)], 0).await;
    let request = Uuid::new_v4();
    let response = socket(&pool, account, id, request, 0, 3).await.expect("ok");
    assert_eq!((response.item.as_str(), response.gem.as_str()), (RING, "gem_haste_2"));
    let after = loadout(&pool, account, id).await;
    assert_eq!(socketed(&after), vec!["gem_haste_2".to_owned()]);
    assert_eq!(count(&after, "gem_haste_2"), 1);
    assert_eq!(recorded(&pool, request).await, 1);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn an_occupied_or_missing_socket_or_a_non_gem_is_refused_and_records_nothing() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let id = stocked(
        &pool,
        account,
        Some("gem_haste_1"),
        &[(0, "gem_haste_2", 2), (1, "gravewardens_cowl", 1)],
        0,
    )
    .await;
    let before = loadout(&pool, account, id).await;
    for (socket_index, cell, expect_conflict) in [(0, 0, true), (1, 0, false), (0, 1, false), (0, 8, false)] {
        let request = Uuid::new_v4();
        let error = socket(&pool, account, id, request, socket_index, cell)
            .await
            .expect_err("refused");
        if expect_conflict {
            assert!(matches!(error, AppError::Conflict(_)), "{error:?}");
        } else {
            assert!(matches!(error, AppError::BadRequest(_)), "{error:?}");
        }
        assert_eq!(recorded(&pool, request).await, 0);
    }
    assert_eq!(loadout(&pool, account, id).await, before);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn unsocketing_pays_gold_and_returns_the_gem_and_replays_identically() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let cost = engine_realm::gems::builtin().expect("gems").unsocket_cost(3);
    let id = stocked(&pool, account, Some("gem_haste_3"), &[], cost + 9).await;
    let request = Uuid::new_v4();
    let first = unsocket(&pool, account, id, request, 0).await.expect("ok");
    assert_eq!((first.cost, first.gold_left), (cost, 9));
    let after = loadout(&pool, account, id).await;
    assert!(socketed(&after).is_empty());
    assert_eq!((count(&after, "gem_haste_3"), gold(&after)), (1, 9));
    let again = unsocket(&pool, account, id, request, 0).await.expect("replay");
    assert_eq!(first, again);
    assert_eq!(loadout(&pool, account, id).await, after, "one charge");
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn unsocketing_refuses_without_gold_and_with_a_full_bag() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let id = stocked(&pool, account, Some("gem_haste_5"), &[], 1).await;
    let request = Uuid::new_v4();
    let error = unsocket(&pool, account, id, request, 0).await.expect_err("poor");
    assert!(matches!(error, AppError::Conflict(_)), "{error:?}");
    assert_eq!(socketed(&loadout(&pool, account, id).await).len(), 1);
    assert_eq!(recorded(&pool, request).await, 0);

    let full: Vec<(u32, &str, u16)> = (0..u32::try_from(engine_game::character::INVENTORY_SLOTS).expect("small"))
        .map(|cell| (cell, "gravewardens_cowl", 1))
        .collect();
    let id = stocked(&pool, account, Some("gem_haste_1"), &full, 1000).await;
    let before = loadout(&pool, account, id).await;
    let error = unsocket(&pool, account, id, Uuid::new_v4(), 0).await.expect_err("full");
    assert!(matches!(error, AppError::Conflict(_)), "{error:?}");
    assert_eq!(loadout(&pool, account, id).await, before);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn an_upgrade_success_consumes_the_gems_and_gold_and_a_failure_keeps_one() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let catalog = engine_realm::gems::builtin().expect("gems");
    let step = catalog.upgrade.steps[0].gold;
    let id = stocked(&pool, account, None, &[(0, "gem_haste_1", 5)], step * 2 + 4).await;

    let win = upgrade(&pool, account, id, Uuid::new_v4(), 0, true).await.expect("rolled");
    assert!(win.success);
    assert_eq!((win.consumed, win.returned, win.cost), (2, 0, step));
    let after = loadout(&pool, account, id).await;
    assert_eq!((count(&after, "gem_haste_1"), count(&after, "gem_haste_2")), (3, 1));
    assert_eq!(gold(&after), step + 4);

    let lose = upgrade(&pool, account, id, Uuid::new_v4(), 0, false).await.expect("rolled");
    assert!(!lose.success);
    assert_eq!((lose.consumed, lose.returned), (2, 1));
    let after = loadout(&pool, account, id).await;
    assert_eq!((count(&after, "gem_haste_1"), count(&after, "gem_haste_2")), (2, 1));
    assert_eq!(gold(&after), 4);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn an_upgrade_without_gems_or_gold_is_refused_and_nothing_is_recorded() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let id = stocked(&pool, account, None, &[(0, "gem_haste_1", 1)], 1000).await;
    let request = Uuid::new_v4();
    let error = upgrade(&pool, account, id, request, 0, true).await.expect_err("one gem");
    assert!(matches!(error, AppError::Conflict(_)), "{error:?}");
    assert_eq!(recorded(&pool, request).await, 0);

    let id = stocked(&pool, account, None, &[(0, "gem_haste_1", 4)], 1).await;
    let before = loadout(&pool, account, id).await;
    let error = upgrade(&pool, account, id, Uuid::new_v4(), 0, true).await.expect_err("poor");
    assert!(matches!(error, AppError::Conflict(_)), "{error:?}");
    assert_eq!(loadout(&pool, account, id).await, before);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn the_same_request_id_twice_is_one_roll_one_charge_and_the_identical_response() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let step = engine_realm::gems::builtin().expect("gems").upgrade.steps[0].gold;
    let id = stocked(&pool, account, None, &[(0, "gem_haste_1", 8)], step * 3).await;
    let request = Uuid::new_v4();
    let first = upgrade(&pool, account, id, request, 0, false).await.expect("first");
    // A retry whose bits would have succeeded: the stored answer comes back.
    let again = upgrade(&pool, account, id, request, 0, true).await.expect("replay");
    assert_eq!(first, again);
    assert!(!again.success);
    let after = loadout(&pool, account, id).await;
    assert_eq!(gold(&after), step * 2, "one charge");
    assert_eq!(count(&after, "gem_haste_1"), 7, "one failed roll: 8 - 2 + 1");

    // Two copies in flight at once: the row lock makes the second a replay.
    let racing = Uuid::new_v4();
    let (a, b) = tokio::join!(
        upgrade(&pool, account, id, racing, 0, true),
        upgrade(&pool, account, id, racing, 0, false),
    );
    assert_eq!(a.expect("a"), b.expect("b"));
    assert_eq!(gold(&loadout(&pool, account, id).await), step);

    // The id belongs to this action: another kind cannot reuse it.
    let other = store::HubTx::begin(&pool, account, id, Some(request), "reforge").await;
    assert!(matches!(other, Err(AppError::Conflict(_))));
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn an_engaged_character_is_refused_before_any_change() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let id = stocked(&pool, account, Some("gem_haste_1"), &[(0, "gem_haste_1", 4)], 1000).await;
    let before = loadout(&pool, account, id).await;
    let request = Uuid::new_v4();
    let body = socket_request(request, 0, 0);
    let error = store::socket_gem(&pool, account, id, request, &body, engaged)
        .await
        .expect_err("engaged");
    assert!(matches!(error, AppError::Conflict(_)));
    let error = store::unsocket_gem(&pool, account, id, request, &unsocket_request(request, 0), engaged)
        .await
        .expect_err("engaged");
    assert!(matches!(error, AppError::Conflict(_)));
    let error = store::upgrade_gem(&pool, account, id, request, 0, &mut || 0, engaged)
        .await
        .expect_err("engaged");
    assert!(matches!(error, AppError::Conflict(_)));
    assert_eq!(loadout(&pool, account, id).await, before);
    assert_eq!(recorded(&pool, request).await, 0);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn another_accounts_character_is_not_found() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let other = new_account(&pool).await;
    let id = stocked(&pool, account, None, &[(0, "gem_haste_1", 4)], 1000).await;
    let before = loadout(&pool, account, id).await;
    let result = store::upgrade_gem(&pool, other, id, Uuid::new_v4(), 0, &mut || 0, unlocked)
        .await
        .expect("no error");
    assert!(result.is_none());
    assert_eq!(loadout(&pool, account, id).await, before);
    drop_account(&pool, other).await;
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn racing_upgrades_with_different_ids_never_spend_gold_or_gems_twice() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let step = engine_realm::gems::builtin().expect("gems").upgrade.steps[0].gold;
    // Gold for one attempt only: exactly one is charged, the other refused.
    let id = stocked(&pool, account, None, &[(0, "gem_haste_1", 6)], step + step / 2).await;
    let (a, b) = tokio::join!(
        upgrade(&pool, account, id, Uuid::new_v4(), 0, true),
        upgrade(&pool, account, id, Uuid::new_v4(), 0, true),
    );
    assert_eq!([a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(), 1);
    let after = loadout(&pool, account, id).await;
    assert_eq!(gold(&after), step / 2);
    assert_eq!((count(&after, "gem_haste_1"), count(&after, "gem_haste_2")), (4, 1));

    // Gems for one socketing only (a single gem, one socket): one wins.
    let id = stocked(&pool, account, None, &[(0, "gem_haste_1", 1)], 0).await;
    let (a, b) = tokio::join!(
        socket(&pool, account, id, Uuid::new_v4(), 0, 0),
        socket(&pool, account, id, Uuid::new_v4(), 0, 0),
    );
    assert_eq!([a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(), 1);
    let after = loadout(&pool, account, id).await;
    assert_eq!(socketed(&after).len(), 1);
    assert_eq!(count(&after, "gem_haste_1"), 0);
    drop_account(&pool, account).await;
}
