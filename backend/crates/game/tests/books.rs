//! Integration tests for books against a live Postgres.
//!
//! Requires `DATABASE_URL` with the migrations applied. Marked `#[ignore]` so default
//! `cargo test` runs stay hermetic. Run explicitly:
//!     cargo test -p realm-game -- --ignored

use engine_game::hero::Hero;
use engine_realm::api::{CharacterDetail, UseBookResponse};
use engine_realm::books::books_of;
use engine_realm::loadout::{BagItem, SlotRef};
use generic_auth_core::AppError;
use realm_game::{characters, store};
use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    // The shipped catalogs: installs the talent and book tables the tests read.
    let _ = realm_game::catalog();
    PgPool::connect(&url).await.expect("connect")
}

async fn new_account(pool: &PgPool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO auth.users (email, role_id)
         SELECT $1, id FROM auth.roles WHERE name = 'user' RETURNING id",
    )
    .bind(format!("books-{}@test.local", Uuid::new_v4()))
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

fn bag(cell: u32) -> SlotRef {
    SlotRef::Inventory { index: cell }
}

fn book_item(cell: u32, id: &str) -> BagItem {
    BagItem {
        index: cell,
        item: id.to_owned(),
        quantity: 1,
        quality: 100,
        sharpening: 0,
        broken: false,
        gems: Vec::new(),
    }
}

/// The account's character of `hero`, with exactly these books in the bag (cell, book id).
async fn stocked(pool: &PgPool, account: Uuid, hero: Hero, books: &[(u32, &str)]) -> Uuid {
    engine_realm::loot::ensure_items_registered();
    let rows = store::list_or_provision(pool, account)
        .await
        .expect("provision");
    let id = rows
        .into_iter()
        .find(|row| characters::hero(row) == hero)
        .expect("a starter of that hero")
        .id;
    store::update_loadout(pool, account, id, |current| {
        let mut loadout = current.clone();
        loadout.inventory.clear();
        for (cell, book) in books {
            loadout.inventory.push(book_item(*cell, book));
        }
        Ok(loadout)
    })
    .await
    .expect("stock")
    .expect("owned");
    id
}

async fn row(pool: &PgPool, account: Uuid, id: Uuid) -> store::CharacterRow {
    store::get(pool, account, id)
        .await
        .expect("get")
        .expect("exists")
}

fn bag_cells(row: &store::CharacterRow) -> Vec<u32> {
    row.loadout.0.inventory.iter().map(|b| b.index).collect()
}

async fn read(
    pool: &PgPool,
    account: Uuid,
    id: Uuid,
    request: Uuid,
    cell: u32,
) -> Result<UseBookResponse, AppError> {
    store::use_book_character(pool, account, id, request, &bag(cell))
        .await
        .map(|response| response.expect("the account's character"))
}

async fn choose(
    pool: &PgPool,
    account: Uuid,
    id: Uuid,
    request: Uuid,
    book: &str,
    option: &str,
    engaged: bool,
) -> Result<CharacterDetail, AppError> {
    store::set_active_book(pool, account, id, request, book, option, || async move {
        if engaged {
            Err(AppError::Conflict("Leave the dungeon first.".into()))
        } else {
            Ok(())
        }
    })
    .await
    .map(|detail| detail.expect("the account's character"))
}

#[tokio::test]
#[ignore]
async fn reading_a_talent_book_consumes_it_and_adds_a_point_the_server_enforces() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let book = books_of(Hero::Warrior).talent[0].id.clone();
    let id = stocked(&pool, account, Hero::Warrior, &[(2, &book)]).await;
    let before = characters::detail(&row(&pool, account, id).await, vec![], None);

    let response = read(&pool, account, id, Uuid::new_v4(), 2)
        .await
        .expect("read");
    assert_eq!(response.book, book);
    assert_eq!(
        response.character.talent_points_total,
        before.talent_points_total + 1
    );
    assert_eq!(
        response.character.talent_points_available,
        before.talent_points_available + 1
    );
    assert_eq!(response.character.books.unlocked, std::slice::from_ref(&book));
    let stored = row(&pool, account, id).await;
    assert!(bag_cells(&stored).is_empty(), "the item is consumed");
    assert_eq!(stored.books.0.unlocked, [book]);

    // The extra point is real, and so is the limit: one point more than the budget is refused.
    let hero = characters::hero(&stored);
    let budget = engine_realm::talents::total_points(characters::earned_points(&stored));
    assert_eq!(budget, before.talent_points_total + 1);
    // Warrior talents cost 3, 3, 4, 4, ...: the first two cost 6 = the starting 5 + the book.
    let keys = vec!["unbroken_momentum".to_owned(), "shockwave".to_owned()];
    assert!(
        engine_realm::talents::validate(hero, &keys, characters::earned_points(&stored)).is_ok()
    );
    let without =
        engine_realm::talents::validate(hero, &keys, characters::earned_points(&stored) - 1);
    assert!(
        without.is_err(),
        "without the book the same set does not fit"
    );
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn a_duplicate_is_a_conflict_and_the_item_is_kept() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let book = books_of(Hero::Druid).talent[0].id.clone();
    let id = stocked(&pool, account, Hero::Druid, &[(0, &book), (1, &book)]).await;
    read(&pool, account, id, Uuid::new_v4(), 0)
        .await
        .expect("first");

    let second = read(&pool, account, id, Uuid::new_v4(), 1).await;
    assert!(matches!(second, Err(AppError::Conflict(_))), "{second:?}");
    let stored = row(&pool, account, id).await;
    assert_eq!(bag_cells(&stored), [1], "the second copy stays in the bag");
    assert_eq!(stored.books.0.unlocked.len(), 1);
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn another_classs_book_and_a_plain_item_are_refused_with_nothing_changed() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let warrior_book = books_of(Hero::Warrior).talent[0].id.clone();
    let id = stocked(
        &pool,
        account,
        Hero::Druid,
        &[(0, &warrior_book), (1, "ashgraves_seal")],
    )
    .await;
    let wrong = read(&pool, account, id, Uuid::new_v4(), 0).await;
    assert!(matches!(wrong, Err(AppError::Conflict(_))), "{wrong:?}");
    let plain = read(&pool, account, id, Uuid::new_v4(), 1).await;
    assert!(matches!(plain, Err(AppError::BadRequest(_))), "{plain:?}");
    let stored = row(&pool, account, id).await;
    assert_eq!(bag_cells(&stored), [0, 1]);
    assert!(stored.books.0.unlocked.is_empty());
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn an_enhancement_option_is_set_switched_and_refused_while_engaged() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let book = &books_of(Hero::FireMage).enhancement[1];
    let id = stocked(&pool, account, Hero::FireMage, &[(4, &book.id)]).await;

    // Not learned yet.
    let early = choose(
        &pool,
        account,
        id,
        Uuid::new_v4(),
        &book.id,
        &book.options[0].id,
        false,
    )
    .await;
    assert!(matches!(early, Err(AppError::Conflict(_))), "{early:?}");

    read(&pool, account, id, Uuid::new_v4(), 4)
        .await
        .expect("learn");
    let detail = choose(
        &pool,
        account,
        id,
        Uuid::new_v4(),
        &book.id,
        &book.options[0].id,
        false,
    )
    .await
    .expect("choose");
    assert_eq!(detail.books.active[&book.id], book.options[0].id);
    let switched = choose(
        &pool,
        account,
        id,
        Uuid::new_v4(),
        &book.id,
        &book.options[2].id,
        false,
    )
    .await
    .expect("switch");
    assert_eq!(switched.books.active[&book.id], book.options[2].id);
    // The run snapshot carries the hidden node, not a bought talent.
    let stored = row(&pool, account, id).await;
    assert_eq!(
        characters::snapshot_talents(&stored),
        [book.options[2].node.clone()]
    );
    assert!(stored.talents.0.is_empty());

    // Engaged: refused, and the choice stays.
    let engaged = choose(
        &pool,
        account,
        id,
        Uuid::new_v4(),
        &book.id,
        &book.options[1].id,
        true,
    )
    .await;
    assert!(matches!(engaged, Err(AppError::Conflict(_))), "{engaged:?}");
    assert_eq!(
        row(&pool, account, id).await.books.0.active[&book.id],
        book.options[2].id
    );
    // An unknown option or book.
    let unknown = choose(&pool, account, id, Uuid::new_v4(), &book.id, "nope", false).await;
    assert!(
        matches!(unknown, Err(AppError::BadRequest(_))),
        "{unknown:?}"
    );
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn a_replayed_request_returns_the_first_answer_and_consumes_nothing_more() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let book = books_of(Hero::Warrior).talent[1].id.clone();
    let other = books_of(Hero::Warrior).talent[2].id.clone();
    let id = stocked(&pool, account, Hero::Warrior, &[(0, &book), (1, &other)]).await;
    let request = Uuid::new_v4();
    let first = read(&pool, account, id, request, 0).await.expect("first");
    // The retry names the same cell, which now holds nothing: it must not touch the other book.
    let again = read(&pool, account, id, request, 0).await.expect("replay");
    assert_eq!(first, again);
    let stored = row(&pool, account, id).await;
    assert_eq!(bag_cells(&stored), [1], "the other book is untouched");
    assert_eq!(stored.books.0.unlocked, [book]);

    // The same request id for another action is a conflict, not a replay.
    let book_def = &books_of(Hero::Warrior).enhancement[0];
    let mixed = choose(
        &pool,
        account,
        id,
        request,
        &book_def.id,
        &book_def.options[0].id,
        false,
    )
    .await;
    assert!(matches!(mixed, Err(AppError::Conflict(_))), "{mixed:?}");
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn two_uses_of_two_copies_at_once_learn_it_exactly_once() {
    let pool = pool().await;
    let account = new_account(&pool).await;
    let book = books_of(Hero::Warrior).talent[0].id.clone();
    let id = stocked(&pool, account, Hero::Warrior, &[(0, &book), (1, &book)]).await;
    let (a, b) = tokio::join!(
        read(&pool, account, id, Uuid::new_v4(), 0),
        read(&pool, account, id, Uuid::new_v4(), 1),
    );
    let (wins, losses): (Vec<_>, Vec<_>) = [a, b].into_iter().partition(Result::is_ok);
    assert_eq!(wins.len(), 1, "exactly one use wins");
    assert!(matches!(losses[0], Err(AppError::Conflict(_))));
    let stored = row(&pool, account, id).await;
    assert_eq!(stored.books.0.unlocked, [book]);
    assert_eq!(bag_cells(&stored).len(), 1, "one copy consumed, one kept");
    assert_eq!(
        characters::detail(&stored, vec![], None).talent_points_total,
        5 + 1
    );
    drop_account(&pool, account).await;
}

#[tokio::test]
#[ignore]
async fn a_book_from_a_boss_chest_is_delivered_once_and_can_then_be_read() {
    use engine_realm::api::LootDelivery;
    let pool = pool().await;
    let account = new_account(&pool).await;
    let book = books_of(Hero::Warrior).talent[2].id.clone();
    let id = stocked(&pool, account, Hero::Warrior, &[]).await;
    let delivery = LootDelivery {
        loot_id: format!("test-{}:{account}:loot:0", Uuid::new_v4()),
        account_id: account.to_string(),
        character_id: id.to_string(),
        item: book.clone(),
        kind: "bag".into(),
        reason: String::new(),
        quality: 100,
        sharpening: 0,
        broken: false,
    };
    assert!(store::apply_loot(&pool, &delivery).await.expect("applied"));
    assert!(!store::apply_loot(&pool, &delivery).await.expect("replayed"));
    let stored = row(&pool, account, id).await;
    assert_eq!(bag_cells(&stored).len(), 1, "delivered once");
    let cell = bag_cells(&stored)[0];
    let response = read(&pool, account, id, Uuid::new_v4(), cell)
        .await
        .expect("read");
    assert_eq!(response.book, book);
    drop_account(&pool, account).await;
}
