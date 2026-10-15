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
        damage_report: None,
        talent_points: 2,
        looted: Vec::new(),
        currency_spent: vec![],
        participant_key_bump: false,
        own_key_level: 0,
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
        quality: 100,
        enhancement: 0,
        broken: false,
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

/// A damage report for a run by `account`.
fn report_for(account: Uuid) -> engine_realm::meter::DamageReport {
    use engine_realm::meter::{DamageReport, ReportAbility, ReportMember};
    DamageReport {
        party: vec![ReportMember {
            account_id: account.to_string(),
            character_name: "Ash".into(),
            class: "Fire Mage".into(),
            role: "dps".into(),
        }],
        total_damage: 12_345,
        dps: 61.7,
        duration_seconds: 200.0,
        boss_kill_seconds: Some(33.0),
        abilities: vec![ReportAbility {
            ability: "Firebolt".into(),
            damage: 12_345,
            casts: 50,
        }],
    }
}

async fn reports_of(pool: &PgPool, result_id: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM game.dungeon_damage_reports WHERE result_id = $1")
        .bind(result_id)
        .fetch_one(pool)
        .await
        .expect("count reports")
}

/// Talent points the character has earned, as stored.
async fn earned(pool: &PgPool, account: Uuid, character: Uuid) -> i32 {
    store::get(pool, account, character)
        .await
        .expect("get")
        .expect("exists")
        .talent_points_earned
}

#[tokio::test]
#[ignore]
async fn a_damage_report_is_stored_once_per_result_id() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, key) = first_character(&pool, account).await;
    let mut run = winning(account, row.id, &key, "report");
    run.damage_report = Some(report_for(account));

    assert!(store::apply_result(&pool, &run).await.expect("first"));
    assert_eq!(reports_of(&pool, &run.result_id).await, 1);
    // The bridge re-delivers after a crash: the result is a no-op, so is its report.
    assert!(!store::apply_result(&pool, &run).await.expect("second"));
    assert_eq!(reports_of(&pool, &run.result_id).await, 1);
    let (class, total): (String, i64) = sqlx::query_as(
        "SELECT class, total_damage FROM game.dungeon_damage_reports WHERE result_id = $1",
    )
    .bind(&run.result_id)
    .fetch_one(&pool)
    .await
    .expect("report row");
    assert_eq!((class.as_str(), total), ("Fire Mage", 12_345));
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn a_result_without_a_report_stores_none() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, key) = first_character(&pool, account).await;
    let run = winning(account, row.id, &key, "noreport");
    assert!(store::apply_result(&pool, &run).await.expect("applies"));
    assert_eq!(reports_of(&pool, &run.result_id).await, 0);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn talent_points_are_granted_once_per_result_and_only_for_a_success() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, key) = first_character(&pool, account).await;
    assert_eq!(earned(&pool, account, row.id).await, 0);

    let run = winning(account, row.id, &key, "points");
    assert!(store::apply_result(&pool, &run).await.expect("first"));
    assert_eq!(earned(&pool, account, row.id).await, 2);
    // Delivered again: recorded already, so nothing is granted twice.
    assert!(!store::apply_result(&pool, &run).await.expect("second"));
    assert_eq!(earned(&pool, account, row.id).await, 2);

    // A failure that claims points grants none.
    let mut failed = winning(account, row.id, &key, "points-fail");
    failed.success = false;
    assert!(store::apply_result(&pool, &failed).await.expect("failed"));
    assert_eq!(earned(&pool, account, row.id).await, 2);

    // A second, distinct result grants again.
    let another = winning(account, row.id, &key, "points-again");
    assert!(store::apply_result(&pool, &another).await.expect("another"));
    assert_eq!(earned(&pool, account, row.id).await, 4);
    drop_account(&pool, account).await;
}

// --- key modifiers, downgrade, cleanse, currency and participant keys -------------------

/// Puts the key at `level` with `modifiers` already rolled (or not), as an older build left it.
async fn set_key(
    pool: &PgPool,
    character: Uuid,
    level: u32,
    modifiers: &[engine_realm::modifiers::KeyModifier],
    rolled: bool,
) {
    sqlx::query(
        "UPDATE game.dungeon_keys SET level = $2, modifiers = $3, modifiers_rolled = $4
         WHERE character_id = $1",
    )
    .bind(character)
    .bind(i32::try_from(level).expect("small"))
    .bind(sqlx::types::Json(modifiers))
    .bind(rolled)
    .execute(pool)
    .await
    .expect("set key");
}

/// Gives the character `amount` of `resource`.
async fn give(pool: &PgPool, account: Uuid, character: Uuid, resource: &str, amount: u64) {
    store::update_loadout(pool, account, character, |current| {
        let mut loadout = current.clone();
        engine_realm::economy::add_resources(&mut loadout, &[(resource.to_owned(), amount)]);
        Ok(loadout)
    })
    .await
    .expect("give")
    .expect("owned");
}

/// How much of `resource` the character holds.
async fn held(pool: &PgPool, account: Uuid, character: Uuid, resource: &str) -> u64 {
    store::get(pool, account, character)
        .await
        .expect("get")
        .expect("exists")
        .loadout
        .0
        .resources
        .get(resource)
        .copied()
        .unwrap_or(0)
}

/// Gold held, as stored.
async fn gold(pool: &PgPool, account: Uuid, character: Uuid) -> u64 {
    held(pool, account, character, "gold").await
}

#[tokio::test]
#[ignore]
async fn looted_currency_is_paid_once_on_success_and_on_failure() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, key) = first_character(&pool, account).await;

    let mut won = winning(account, row.id, &key, "loot-won");
    won.looted = vec![("gold".into(), 40), ("aether_shard".into(), 2)];
    assert!(store::apply_result(&pool, &won).await.expect("won"));
    assert_eq!(gold(&pool, account, row.id).await, 40);
    // Delivered again: recorded already, so nothing is paid twice.
    assert!(!store::apply_result(&pool, &won).await.expect("again"));
    assert_eq!(gold(&pool, account, row.id).await, 40);

    // A failed run pays no run resources but what was picked up is kept.
    let mut lost = winning(account, row.id, &key, "loot-lost");
    lost.success = false;
    lost.looted = vec![("gold".into(), 15)];
    assert!(store::apply_result(&pool, &lost).await.expect("lost"));
    assert_eq!(gold(&pool, account, row.id).await, 55);
    assert_eq!(stored(&pool, account, row.id).await.1, 9, "only the win paid dust");
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn a_key_that_was_never_rolled_is_rolled_once_when_read() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, _) = first_character(&pool, account).await;
    // Level 1 has no slots: nothing to roll.
    let key = store::get_key(&pool, account, row.id)
        .await
        .expect("key")
        .expect("has");
    assert!(key.modifiers.is_empty());
    // An older key at a level with two slots and nothing stored.
    set_key(&pool, row.id, 5, &[], false).await;
    let first = store::get_key(&pool, account, row.id)
        .await
        .expect("key")
        .expect("has");
    assert_eq!(first.modifiers.len(), 2);
    assert!(first.modifiers_rolled);
    // Read again, and through the list: the same modifiers, not a new roll.
    let again = store::get_key(&pool, account, row.id)
        .await
        .expect("key")
        .expect("has");
    assert_eq!(again.modifiers, first.modifiers);
    let keys = store::keys_of(&pool, account).await.expect("keys");
    assert_eq!(keys[&row.id].modifiers, first.modifiers);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn a_key_that_rises_is_rolled_afresh_for_its_new_level() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, key) = first_character(&pool, account).await;
    set_key(&pool, row.id, 2, &[], true).await;
    let mut win = winning(account, row.id, &key, "rise");
    win.key_level = 2;
    win.key_level_after = 3;
    assert!(store::apply_result(&pool, &win).await.expect("win"));
    let after = store::get_key(&pool, account, row.id)
        .await
        .expect("key")
        .expect("has");
    assert_eq!(after.level, 3);
    assert_eq!(after.modifiers.len(), 1, "level 3 has its first slot");
    assert!(after.modifiers_rolled);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn loot_with_props_is_delivered_once_to_the_bag_or_the_mail() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, _) = first_character(&pool, account).await;
    engine_realm::loot::ensure_items_registered();
    let item = "ashgraves_seal";
    let chest = LootDelivery {
        loot_id: format!("i:{account}:loot:0"),
        account_id: account.to_string(),
        character_id: row.id.to_string(),
        item: item.into(),
        kind: "bag".into(),
        reason: String::new(),
        quality: 106,
        enhancement: 0,
        broken: false,
    };
    let drop = LootDelivery {
        loot_id: format!("i:{account}:mob:0"),
        kind: "mail".into(),
        reason: "dungeon_drop".into(),
        quality: 71,
        ..chest.clone()
    };
    for delivery in [&chest, &drop] {
        assert!(store::apply_loot(&pool, delivery).await.expect("first"));
        assert!(!store::apply_loot(&pool, delivery).await.expect("again"));
    }
    let after = store::get(&pool, account, row.id)
        .await
        .expect("get")
        .expect("row");
    let bag: Vec<_> = after.loadout.0.inventory.iter().filter(|e| e.item == item).collect();
    assert_eq!(bag.len(), 1, "delivered once");
    assert_eq!(bag[0].quality, 106);
    let mail: Vec<_> = after.loadout.0.mail.iter().filter(|m| m.item == item).collect();
    assert_eq!(mail.len(), 1);
    assert_eq!((mail[0].quality, mail[0].reason.as_str()), (71, "dungeon_drop"));
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn an_enhancement_pays_and_persists_under_the_row_lock() {
    use engine_realm::enhance::EnhanceOutcome;
    use engine_realm::loadout::SlotRef;
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, _) = first_character(&pool, account).await;
    engine_realm::loot::ensure_items_registered();
    store::update_loadout(&pool, account, row.id, |current| {
        let mut loadout = current.clone();
        loadout.equipment.retain(|e| e.slot != "ring1");
        loadout
            .equipment
            .push(engine_realm::loadout::EquippedItem::new("ring1", "ashgraves_seal"));
        engine_realm::economy::add_resources(&mut loadout, &[("crypt_dust".into(), 30)]);
        Ok(loadout)
    })
    .await
    .expect("stock")
    .expect("owned");
    let slot = SlotRef::Equipment {
        slot: "ring1".into(),
    };
    let outcome = store::enhance_character(&pool, account, row.id, &slot, 0.5)
        .await
        .expect("allowed")
        .expect("owned");
    assert_eq!(outcome, EnhanceOutcome::Success { new_level: 1 });
    let after = store::get(&pool, account, row.id).await.expect("get").expect("row");
    let ring = after.loadout.0.equipment.iter().find(|e| e.slot == "ring1").expect("ring");
    assert_eq!(ring.enhancement, 1);
    assert_eq!(after.loadout.0.resources.get("crypt_dust"), Some(&20));
    // Another account's character is not found, and nothing is paid.
    let other = new_account(&pool).await;
    assert!(store::enhance_character(&pool, other, row.id, &slot, 0.5)
        .await
        .expect("no error")
        .is_none());
    // Broken items are refused.
    store::update_loadout(&pool, account, row.id, |current| {
        let mut loadout = current.clone();
        loadout.equipment[0].broken = true;
        Ok(loadout)
    })
    .await
    .expect("break")
    .expect("owned");
    assert!(store::enhance_character(&pool, account, row.id, &slot, 0.5)
        .await
        .is_err());
    drop_account(&pool, other).await;
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn a_downgrade_lowers_one_level_keeps_the_key_and_stops_at_the_minimum() {
    use engine_realm::modifiers::KeyModifier;
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, _) = first_character(&pool, account).await;
    let mods = vec![
        KeyModifier::base("rotating_beams"),
        KeyModifier::base("plague_explosion"),
    ];
    set_key(&pool, row.id, 5, &mods, true).await;
    let key = store::downgrade_key(&pool, account, row.id)
        .await
        .expect("downgrade")
        .expect("has a key");
    assert_eq!(key.level, 4);
    assert_eq!(
        key.modifiers,
        vec![mods[0].clone()],
        "the extra modifier is gone"
    );
    let stored = store::get_key(&pool, account, row.id)
        .await
        .expect("key")
        .expect("has");
    assert_eq!(stored, key);

    // Down to the minimum, then refused; the key is still there.
    set_key(&pool, row.id, 2, &[], true).await;
    let key = store::downgrade_key(&pool, account, row.id)
        .await
        .expect("2 to 1")
        .expect("key");
    assert_eq!(key.level, 1);
    assert!(store::downgrade_key(&pool, account, row.id).await.is_err());
    let stored = store::get_key(&pool, account, row.id)
        .await
        .expect("key")
        .expect("still has one");
    assert_eq!(stored.level, 1);

    // Someone else's character has no key to the caller.
    let other = new_account(&pool).await;
    assert!(store::downgrade_key(&pool, other, row.id)
        .await
        .expect("other")
        .is_none());
    drop_account(&pool, other).await;
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn a_cleanse_costs_gold_all_or_nothing() {
    use engine_realm::modifiers::KeyModifier;
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, _) = first_character(&pool, account).await;
    let mods = vec![KeyModifier::base("rotating_beams")];
    set_key(&pool, row.id, 3, &mods, true).await;

    // 99 gold: refused, nothing changes.
    give(&pool, account, row.id, "gold", 99).await;
    assert!(store::cleanse_key(&pool, account, row.id).await.is_err());
    assert_eq!(held(&pool, account, row.id, "gold").await, 99);
    let key = store::get_key(&pool, account, row.id)
        .await
        .expect("key")
        .expect("has");
    assert_eq!(key.modifiers, mods);

    // 100 more is enough: modifiers gone, 100 gold taken, and they stay gone when read again.
    give(&pool, account, row.id, "gold", 51).await;
    let key = store::cleanse_key(&pool, account, row.id)
        .await
        .expect("cleanse")
        .expect("key");
    assert!(key.modifiers.is_empty());
    assert_eq!(held(&pool, account, row.id, "gold").await, 50);
    let again = store::get_key(&pool, account, row.id)
        .await
        .expect("key")
        .expect("has");
    assert!(
        again.modifiers.is_empty(),
        "a cleansed key is not rolled again"
    );
    assert_eq!(again.level, 3);

    // A clean key is not charged for.
    assert!(store::cleanse_key(&pool, account, row.id).await.is_err());
    assert_eq!(held(&pool, account, row.id, "gold").await, 50);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn spent_currency_is_debited_once_and_never_below_zero() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, key) = first_character(&pool, account).await;
    give(&pool, account, row.id, "aether_shard", 3).await;

    let mut run = winning(account, row.id, &key, "spend");
    run.currency_spent = vec![("aether_shard".into(), 2)];
    assert!(store::apply_result(&pool, &run).await.expect("first"));
    assert_eq!(held(&pool, account, row.id, "aether_shard").await, 1);
    // Re-delivered: not debited again.
    assert!(!store::apply_result(&pool, &run).await.expect("again"));
    assert_eq!(held(&pool, account, row.id, "aether_shard").await, 1);

    // A failed run still costs what was spent, and a claim above the balance stops at zero.
    let mut failed = winning(account, row.id, &key, "spend-fail");
    failed.success = false;
    failed.experience = 0;
    failed.resources = vec![];
    failed.key_level_after = 0;
    failed.key_owner = false;
    failed.currency_spent = vec![("aether_shard".into(), 50)];
    assert!(store::apply_result(&pool, &failed).await.expect("failed"));
    assert_eq!(held(&pool, account, row.id, "aether_shard").await, 0);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn a_participants_key_rises_once_and_only_from_the_level_it_had() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let (row, _) = first_character(&pool, account).await;
    set_key(&pool, row.id, 2, &[], true).await;
    let participant = |tag: &str, own: u32, after: u32| DungeonResult {
        result_id: format!("{tag}:{}", Uuid::new_v4()),
        account_id: account.to_string(),
        character_id: row.id.to_string(),
        dungeon_id: realm_game::catalog().dungeons()[0].id.clone(),
        success: true,
        duration_us: 1,
        experience: 10,
        key_level: 6,
        key_owner: false,
        key_level_after: after,
        resources: vec![],
        damage_report: None,
        talent_points: 0,
        looted: vec![],
        currency_spent: vec![],
        participant_key_bump: true,
        own_key_level: own,
    };
    // The realm snapshot said 2: the key rises to 3, rolled for its new level.
    let run = participant("bump", 2, 3);
    assert!(store::apply_result(&pool, &run).await.expect("bump"));
    let key = store::get_key(&pool, account, row.id)
        .await
        .expect("key")
        .expect("has");
    assert_eq!(key.level, 3);
    assert_eq!(key.modifiers.len(), 1);
    // Re-delivered: nothing.
    assert!(!store::apply_result(&pool, &run).await.expect("again"));
    assert_eq!(
        store::get_key(&pool, account, row.id)
            .await
            .expect("k")
            .expect("h")
            .level,
        3
    );
    // Another run that also saw 2 (a stale snapshot) cannot lift it again.
    assert!(store::apply_result(&pool, &participant("stale", 2, 3))
        .await
        .expect("stale"));
    assert_eq!(
        store::get_key(&pool, account, row.id)
            .await
            .expect("k")
            .expect("h")
            .level,
        3
    );
    // Without the flag nothing moves.
    let mut unflagged = participant("flag", 3, 4);
    unflagged.participant_key_bump = false;
    assert!(store::apply_result(&pool, &unflagged)
        .await
        .expect("unflagged"));
    assert_eq!(
        store::get_key(&pool, account, row.id)
            .await
            .expect("k")
            .expect("h")
            .level,
        3
    );
    drop_account(&pool, account).await;
}
