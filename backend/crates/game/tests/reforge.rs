//! Integration tests for the reforge gamble against a live Postgres.
//!
//! Requires `DATABASE_URL` with the migrations applied. Marked `#[ignore]` so default
//! `cargo test` runs stay hermetic. Run explicitly:
//!     cargo test -p realm-game -- --ignored

use engine_realm::api::ReforgeResponse;
use engine_realm::loadout::{EquippedItem, SlotRef};
use generic_auth_core::AppError;
use realm_game::store;
use sqlx::PgPool;
use uuid::Uuid;

const KEY: &str = "ashgraves_seal";

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
    .bind(format!("reforge-{}@test.local", Uuid::new_v4()))
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

fn slot() -> SlotRef {
    SlotRef::Equipment {
        slot: "ring1".into(),
    }
}

/// A character wearing a ring of this quality and sharpening, holding exactly `gold`.
async fn stocked(pool: &PgPool, account: Uuid, quality: u8, sharpening: u8, gold: u64) -> Uuid {
    let rows = store::list_or_provision(pool, account)
        .await
        .expect("provision");
    let id = rows.into_iter().next().expect("a starter").id;
    engine_realm::loot::ensure_items_registered();
    store::update_loadout(pool, account, id, |current| {
        let mut loadout = current.clone();
        loadout.equipment.retain(|e| e.slot != "ring1");
        let mut ring = EquippedItem::new("ring1", KEY);
        ring.quality = quality;
        ring.sharpening = sharpening;
        loadout.equipment.push(ring);
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

/// The ring's (quality, sharpening) and the gold held, as stored.
async fn state(pool: &PgPool, account: Uuid, id: Uuid) -> (u8, u8, u64) {
    let row = store::get(pool, account, id)
        .await
        .expect("get")
        .expect("exists");
    let ring = row
        .loadout
        .0
        .equipment
        .iter()
        .find(|e| e.slot == "ring1")
        .expect("the ring");
    (
        ring.quality,
        ring.sharpening,
        row.loadout.0.resources.get("gold").copied().unwrap_or(0),
    )
}

/// What a reforge of this ring costs now.
fn price(sharpening: u8) -> u64 {
    engine_realm::loot::ensure_items_registered();
    let mut loadout = engine_realm::loadout::Loadout::default();
    let mut ring = EquippedItem::new("ring1", KEY);
    ring.sharpening = sharpening;
    loadout.equipment.push(ring);
    engine_realm::reforge::quote(&loadout, &slot(), realm_game::catalog().economy())
        .expect("allowed")
        .gold
}

async fn unlocked() -> Result<(), AppError> {
    Ok(())
}

/// One reforge the way the route does it, rolling the lowest quality (`draw` 0) or the highest.
async fn reforge(
    pool: &PgPool,
    account: Uuid,
    id: Uuid,
    request: Uuid,
    highest: bool,
) -> Result<ReforgeResponse, AppError> {
    let bits = if highest { u64::MAX } else { 0 };
    store::reforge_character(pool, account, id, request, &slot(), &mut || bits, unlocked)
        .await
        .map(|response| response.expect("the account's character"))
}

async fn recorded(pool: &PgPool, request: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM game.hub_requests WHERE request_id = $1")
        .bind(request)
        .fetch_one(pool)
        .await
        .expect("count")
}

#[tokio::test]
#[ignore]
async fn a_reforge_charges_gold_replaces_the_quality_and_keeps_the_sharpening() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let cost = price(3);
    let id = stocked(&pool, account, 108, 3, cost + 40).await;

    let request = Uuid::new_v4();
    let response = reforge(&pool, account, id, request, false)
        .await
        .expect("allowed");
    assert_eq!(response.item, KEY);
    // 108 -> 50: the new roll replaces the old one even when it is worse.
    assert_eq!((response.old_quality, response.new_quality), (108, 50));
    assert_eq!((response.cost, response.gold_left), (cost, 40));
    assert_eq!(state(&pool, account, id).await, (50, 3, 40));
    assert_eq!(recorded(&pool, request).await, 1);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn too_little_gold_is_refused_and_nothing_changes_or_is_recorded() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let cost = price(0);
    let id = stocked(&pool, account, 77, 0, cost - 1).await;

    let request = Uuid::new_v4();
    let error = reforge(&pool, account, id, request, true)
        .await
        .expect_err("short of gold");
    assert!(matches!(error, AppError::Conflict(_)), "{error:?}");
    assert_eq!(state(&pool, account, id).await, (77, 0, cost - 1));
    assert_eq!(
        recorded(&pool, request).await,
        0,
        "a refusal records nothing"
    );

    // The same id succeeds once the purse can pay.
    store::update_loadout(&pool, account, id, |current| {
        let mut loadout = current.clone();
        engine_realm::economy::add_resources(&mut loadout, &[("gold".into(), 1)]);
        Ok(loadout)
    })
    .await
    .expect("top up")
    .expect("owned");
    reforge(&pool, account, id, request, true)
        .await
        .expect("now affordable");
    assert_eq!(state(&pool, account, id).await, (110, 0, 0));
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn the_same_request_id_twice_is_one_charge_and_the_identical_response() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let cost = price(0);
    let id = stocked(&pool, account, 90, 0, cost * 3).await;

    let request = Uuid::new_v4();
    let first = reforge(&pool, account, id, request, false)
        .await
        .expect("first");
    // A retry whose random bits would have rolled the top: the stored answer comes back.
    let again = reforge(&pool, account, id, request, true)
        .await
        .expect("replay");
    assert_eq!(first, again, "identical response");
    assert_eq!(
        state(&pool, account, id).await,
        (50, 0, cost * 2),
        "one charge, one roll"
    );

    // Two copies in flight at once: the row lock makes the second a replay.
    let racing = Uuid::new_v4();
    let (a, b) = tokio::join!(
        reforge(&pool, account, id, racing, false),
        reforge(&pool, account, id, racing, true),
    );
    assert_eq!(a.expect("a"), b.expect("b"));
    assert_eq!(state(&pool, account, id).await.2, cost);

    // The id belongs to this action: another kind of action cannot reuse it.
    let other = store::HubTx::begin(&pool, account, id, Some(request), "sharpen").await;
    assert!(matches!(other, Err(AppError::Conflict(_))));
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn an_engaged_character_is_refused_before_any_charge() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let cost = price(0);
    let id = stocked(&pool, account, 90, 0, cost * 2).await;

    let request = Uuid::new_v4();
    let engaged = || async {
        Err::<(), _>(AppError::Conflict(
            engine_realm::api::ENGAGED_MESSAGE.into(),
        ))
    };
    let error = store::reforge_character(&pool, account, id, request, &slot(), &mut || 0, engaged)
        .await
        .expect_err("engaged");
    assert!(matches!(error, AppError::Conflict(_)));
    assert_eq!(state(&pool, account, id).await, (90, 0, cost * 2));
    assert_eq!(recorded(&pool, request).await, 0);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn another_accounts_character_is_not_found_and_nothing_is_charged() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let other = new_account(&pool).await;
    let cost = price(0);
    let id = stocked(&pool, account, 90, 0, cost).await;

    let result = store::reforge_character(
        &pool,
        other,
        id,
        Uuid::new_v4(),
        &slot(),
        &mut || 0,
        unlocked,
    )
    .await
    .expect("no error");
    assert!(result.is_none());
    assert_eq!(state(&pool, account, id).await, (90, 0, cost));
    drop_account(&pool, other).await;
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn racing_requests_with_different_ids_leave_consistent_gold() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let cost = price(0);

    // Gold for exactly one: of two racing reforges exactly one is charged and the other
    // is refused, never both paid from the same purse.
    let id = stocked(&pool, account, 90, 0, cost + cost / 2).await;
    let (a, b) = tokio::join!(
        reforge(&pool, account, id, Uuid::new_v4(), false),
        reforge(&pool, account, id, Uuid::new_v4(), false),
    );
    assert_eq!(
        [a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(),
        1,
        "one paid, one refused"
    );
    assert_eq!(state(&pool, account, id).await, (50, 0, cost / 2));

    // Gold for both: both are charged, nothing lost or doubled.
    let id = stocked(&pool, account, 90, 0, cost * 2 + 7).await;
    let (a, b) = tokio::join!(
        reforge(&pool, account, id, Uuid::new_v4(), true),
        reforge(&pool, account, id, Uuid::new_v4(), true),
    );
    a.expect("a");
    b.expect("b");
    assert_eq!(state(&pool, account, id).await, (110, 0, 7));
    drop_account(&pool, account).await;
}
