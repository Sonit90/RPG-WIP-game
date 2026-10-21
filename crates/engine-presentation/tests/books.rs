//! Books in the interface: the item tooltip and the bag's right-click.
//!
//! An integration test of its own because it needs the shipped talent catalog installed before
//! anything reads the talent tables, and each test binary is one process.

use engine_game::arena::{self, ArenaConfig};
use engine_game::character::{ItemStack, item_by_key};
use engine_game::hero::Hero;
use engine_game::talent::books::book_bit;
use engine_presentation::snapshot::{CharacterView, UiSnapshot};
use engine_presentation::ui::character_menu::{CharacterMenu, CharacterMenuOutcome};
use engine_presentation::ui::input::UiInput;
use engine_presentation::ui::tooltip::{
    ItemContext, ItemPlace, SectionKind, TooltipModel, fill_item,
};

const VIEWPORT: (u32, u32) = (1600, 900);

fn snapshot() -> UiSnapshot {
    engine_realm::catalog::Catalog::builtin().expect("the shipped catalogs");
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    UiSnapshot::build(&simulation)
}

fn character() -> CharacterView {
    snapshot().character
}

#[test]
fn a_book_tooltip_names_its_kind_class_and_whether_it_is_learned() {
    let mut character = character();
    let hero = character.hero;
    let books = &engine_game::talent::catalog().books[hero.index()];
    let talent = item_by_key(&books.talent[0].id).expect("registered");
    let enhancement = item_by_key(&books.enhancement[0].id).expect("registered");
    let mut model = TooltipModel::new();

    let context = ItemContext::loose(&character, ItemStack::one(talent.id), ItemPlace::Bag);
    fill_item(&mut model, ItemStack::one(talent.id), hero, &context);
    let body = model.find(SectionKind::Description).expect("text");
    assert!(body.text.contains("one talent point"), "{}", body.text);
    assert_eq!(
        model.find(SectionKind::Requirements).expect("state").text,
        "Not learned yet"
    );
    assert!(model.footer.starts_with("Right-click to read"));

    let context = ItemContext::loose(&character, ItemStack::one(enhancement.id), ItemPlace::Bag);
    fill_item(&mut model, ItemStack::one(enhancement.id), hero, &context);
    let options = model.find(SectionKind::Details).expect("options");
    assert_eq!(options.text.lines().count(), 3, "{}", options.text);
    assert!(
        model
            .find(SectionKind::Description)
            .expect("text")
            .text
            .contains("three options")
    );

    character.books_learned |= 1 << book_bit(&books.talent[0].id).expect("bit");
    let context = ItemContext::loose(&character, ItemStack::one(talent.id), ItemPlace::Bag);
    fill_item(&mut model, ItemStack::one(talent.id), hero, &context);
    assert_eq!(
        model.find(SectionKind::Requirements).expect("state").text,
        "Already learned"
    );
    assert!(!model.footer.contains("read"), "{}", model.footer);

    // Another class's book says whose it is.
    let other = Hero::ALL
        .into_iter()
        .find(|candidate| *candidate != hero)
        .expect("another hero");
    let foreign = item_by_key(&engine_game::talent::catalog().books[other.index()].talent[0].id)
        .expect("registered");
    let context = ItemContext::loose(&character, ItemStack::one(foreign.id), ItemPlace::Bag);
    fill_item(&mut model, ItemStack::one(foreign.id), hero, &context);
    let state = model.find(SectionKind::Requirements).expect("state");
    assert!(state.text.starts_with("Written for the"), "{}", state.text);
}

#[test]
fn right_clicking_a_book_asks_to_read_it_unless_it_is_already_learned() {
    let mut snapshot = snapshot();
    let hero = snapshot.character.hero;
    let book = &engine_game::talent::catalog().books[hero.index()].talent[0];
    let def = item_by_key(&book.id).expect("registered");
    let cell = snapshot
        .character
        .inventory
        .iter()
        .position(Option::is_none)
        .expect("a free cell");
    snapshot.character.inventory[cell] = Some(ItemStack::one(def.id));
    let mut menu = CharacterMenu::new();
    menu.open();
    let rect = menu.cell_rect(cell, VIEWPORT);
    let right_click = UiInput {
        secondary_pressed: true,
        ..UiInput::new().with_pointer(rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    };
    assert_eq!(
        menu.update(&right_click, &snapshot, VIEWPORT),
        CharacterMenuOutcome::Use { cell }
    );
    snapshot.character.books_learned |= 1 << book_bit(&book.id).expect("bit");
    assert_eq!(
        menu.update(&right_click, &snapshot, VIEWPORT),
        CharacterMenuOutcome::Idle
    );
}
