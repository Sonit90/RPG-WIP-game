//! Books: what a character has learned, and the two hub actions on it.
//!
//! ```text
//! boss chest ─► Book item in the bag ─► use_book ─► BookState.unlocked   (item consumed)
//!                                      set_active ─► BookState.active    (enhancement books)
//!   BookState ─► point_bonus      ─► the talent budget (API validation, client total)
//!             └► active_nodes     ─► hidden talent nodes ─► TalentGrid ─► <Hero>TalentEffects
//! ```
//!
//! The catalog (ids, options, nodes) lives in `talents.json` and is read by [`crate::talents`];
//! the effect rules live in `engine-game` as hidden talent nodes. This module is the state and the
//! pure transitions. The API runs them inside one locked read-modify-write of the character row,
//! so a duplicate use, a race of two copies or a replay cannot double-spend anything.
//!
//! # Gates
//!
//! A book's `min_key_level` gates the **drop** (the boss chest rolls it only at or above that key
//! level; see `engine_game::loot::roll_book`), not the use. The item can only exist because the gate
//! was met, and a key level is a moving thing (keys are downgraded, cleansed) that must not strand a
//! book already held.

use std::collections::BTreeMap;
use std::fmt;

use engine_game::character::item_by_key;
use engine_game::hero::Hero;
use engine_game::talent::books::{BookKind, HeroBooks};
use engine_game::talent::catalog;
use serde::{Deserialize, Serialize};

use crate::loadout::{Loadout, SlotRef};
use crate::loot::ensure_items_registered;

/// What a character has learned. Stored as `characters.books` (JSONB); the default is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookState {
    /// Ids of the books learned, in the order they were learned.
    #[serde(default)]
    pub unlocked: Vec<String>,
    /// For each enhancement book, the option in force. Absent: none chosen yet.
    #[serde(default)]
    pub active: BTreeMap<String, String>,
}

/// Why a book action was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookError {
    /// The cell holds no book (empty, or an ordinary item), or the slot is not a bag cell.
    NotABook,
    /// The book belongs to another class.
    WrongClass,
    /// The character already learned it.
    AlreadyLearned,
    /// The book is not learned.
    NotLearned,
    /// The book is a talent-point book, which has no options.
    NotEnhancement,
    /// The book has no such option.
    UnknownOption,
}

impl BookError {
    /// A sentence for the player.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::NotABook => "That is not a book.",
            Self::WrongClass => "That book is written for another class.",
            Self::AlreadyLearned => "You have already learned that book.",
            Self::NotLearned => "You have not learned that book.",
            Self::NotEnhancement => "That book has no options to choose.",
            Self::UnknownOption => "That book has no such option.",
        }
    }
}

impl fmt::Display for BookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for BookError {}

/// `hero`'s books in force.
#[must_use]
pub fn books_of(hero: Hero) -> &'static HeroBooks {
    &catalog().books[hero.index()]
}

/// The hero a book is written for, by its id.
#[must_use]
pub fn hero_of(book: &str) -> Option<Hero> {
    Hero::ALL
        .into_iter()
        .find(|hero| books_of(*hero).kind_of(book).is_some())
}

/// What a use of a book did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Learned {
    /// The book's id.
    pub book: String,
    /// Its kind.
    pub kind: BookKind,
}

impl BookState {
    /// Whether `book` is learned.
    #[must_use]
    pub fn has(&self, book: &str) -> bool {
        self.unlocked.iter().any(|id| id == book)
    }

    /// Talent points the learned talent books of `hero` are worth. Unknown ids count nothing, so
    /// a retired book can neither crash nor inflate a budget.
    #[must_use]
    pub fn point_bonus(&self, hero: Hero) -> u32 {
        let books = books_of(hero);
        let mut counted: Vec<&str> = Vec::new();
        for id in &self.unlocked {
            if books.talent_book(id).is_some() && !counted.contains(&id.as_str()) {
                counted.push(id);
            }
        }
        u32::try_from(counted.len()).unwrap_or(u32::MAX)
    }

    /// The `(book, option)` pairs in force for `hero`: learned enhancement books with a valid
    /// chosen option. Anything else in `active` is ignored.
    #[must_use]
    pub fn active_options(&self, hero: Hero) -> Vec<(&str, &str)> {
        let books = books_of(hero);
        books
            .enhancement
            .iter()
            .filter(|book| self.has(&book.id))
            .filter_map(|book| {
                let option = self.active.get(&book.id)?;
                book.options
                    .iter()
                    .any(|candidate| &candidate.id == option)
                    .then_some((book.id.as_str(), option.as_str()))
            })
            .collect()
    }

    /// The hidden talent node keys in force for `hero`: what the snapshot of a character appends
    /// to its bought talents so the realm's grid switches them on.
    #[must_use]
    pub fn active_nodes(&self, hero: Hero) -> Vec<String> {
        let books = books_of(hero);
        self.active_options(hero)
            .into_iter()
            .filter_map(|(book, option)| books.node_of(book, option).map(str::to_owned))
            .collect()
    }

    /// Learns `book` for a character of `hero`.
    ///
    /// # Errors
    ///
    /// [`BookError::NotABook`] for an id no hero has, [`BookError::WrongClass`],
    /// [`BookError::AlreadyLearned`]. The state is unchanged on an error.
    pub fn learn(&mut self, hero: Hero, book: &str) -> Result<Learned, BookError> {
        let owner = hero_of(book).ok_or(BookError::NotABook)?;
        if owner != hero {
            return Err(BookError::WrongClass);
        }
        if self.has(book) {
            return Err(BookError::AlreadyLearned);
        }
        let kind = books_of(hero).kind_of(book).ok_or(BookError::NotABook)?;
        self.unlocked.push(book.to_owned());
        Ok(Learned {
            book: book.to_owned(),
            kind,
        })
    }

    /// Makes `option` the one in force for enhancement book `book`.
    ///
    /// # Errors
    ///
    /// [`BookError::NotLearned`], [`BookError::NotEnhancement`], [`BookError::UnknownOption`] (a
    /// book of another class is "not learned": it cannot have been).
    pub fn set_active(&mut self, hero: Hero, book: &str, option: &str) -> Result<(), BookError> {
        let books = books_of(hero);
        if !self.has(book) || books.kind_of(book).is_none() {
            return Err(BookError::NotLearned);
        }
        let Some(enhancement) = books.enhancement_book(book) else {
            return Err(BookError::NotEnhancement);
        };
        if !enhancement.options.iter().any(|candidate| candidate.id == option) {
            return Err(BookError::UnknownOption);
        }
        self.active.insert(book.to_owned(), option.to_owned());
        Ok(())
    }
}

/// **Reads a book from the bag**: consumes the item and learns the book.
///
/// `slot` must be a bag cell holding a book of the character's class that is not yet learned. On
/// any error neither the loadout nor the state changes (a duplicate keeps its item).
///
/// # Errors
///
/// See [`BookError`].
pub fn use_book(
    loadout: &mut Loadout,
    state: &mut BookState,
    hero: Hero,
    slot: &SlotRef,
) -> Result<Learned, BookError> {
    ensure_items_registered();
    let SlotRef::Inventory { index } = slot else {
        return Err(BookError::NotABook);
    };
    let position = loadout
        .inventory
        .iter()
        .position(|bag| bag.index == *index)
        .ok_or(BookError::NotABook)?;
    let key = loadout.inventory[position].item.clone();
    if item_by_key(&key).is_none() {
        return Err(BookError::NotABook);
    }
    let learned = state.learn(hero, &key)?;
    let entry = &mut loadout.inventory[position];
    if entry.quantity > 1 {
        entry.quantity -= 1;
    } else {
        loadout.inventory.remove(position);
    }
    Ok(learned)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::loadout::BagItem;

    fn setup() {
        Catalog::builtin().expect("the shipped catalog parses");
    }

    fn bag_with(book: &str, cell: u32) -> Loadout {
        Loadout {
            inventory: vec![BagItem {
                index: cell,
                item: book.to_owned(),
                quantity: 1,
                quality: 100,
                sharpening: 0,
                broken: false,
            }],
            ..Loadout::default()
        }
    }

    fn slot(index: u32) -> SlotRef {
        SlotRef::Inventory { index }
    }

    #[test]
    fn using_a_talent_book_consumes_it_and_adds_a_point() {
        setup();
        let book = books_of(Hero::Warrior).talent[0].id.clone();
        let mut loadout = bag_with(&book, 3);
        let mut state = BookState::default();
        assert_eq!(state.point_bonus(Hero::Warrior), 0);
        let learned = use_book(&mut loadout, &mut state, Hero::Warrior, &slot(3)).expect("read");
        assert_eq!(learned.kind, BookKind::TalentPoint);
        assert!(loadout.inventory.is_empty());
        assert_eq!(state.point_bonus(Hero::Warrior), 1);
        assert_eq!(state.point_bonus(Hero::Druid), 0);
    }

    #[test]
    fn a_duplicate_is_refused_and_the_item_kept() {
        setup();
        let book = books_of(Hero::Druid).talent[0].id.clone();
        let mut loadout = bag_with(&book, 0);
        let mut state = BookState::default();
        use_book(&mut loadout, &mut state, Hero::Druid, &slot(0)).expect("first");
        let mut again = bag_with(&book, 5);
        assert_eq!(
            use_book(&mut again, &mut state, Hero::Druid, &slot(5)),
            Err(BookError::AlreadyLearned)
        );
        assert_eq!(again.inventory.len(), 1, "the copy stays in the bag");
        assert_eq!(state.unlocked.len(), 1);
    }

    #[test]
    fn another_classs_book_and_other_items_are_refused() {
        setup();
        let warrior_book = books_of(Hero::Warrior).talent[0].id.clone();
        let mut loadout = bag_with(&warrior_book, 0);
        let mut state = BookState::default();
        assert_eq!(
            use_book(&mut loadout, &mut state, Hero::Druid, &slot(0)),
            Err(BookError::WrongClass)
        );
        assert_eq!(loadout.inventory.len(), 1);
        let mut sword = bag_with("ashgraves_seal", 0);
        assert_eq!(
            use_book(&mut sword, &mut state, Hero::Druid, &slot(0)),
            Err(BookError::NotABook)
        );
        assert_eq!(
            use_book(&mut loadout, &mut state, Hero::Warrior, &slot(9)),
            Err(BookError::NotABook)
        );
        assert_eq!(
            use_book(
                &mut loadout,
                &mut state,
                Hero::Warrior,
                &SlotRef::Equipment { slot: "head".into() }
            ),
            Err(BookError::NotABook)
        );
        assert!(state.unlocked.is_empty());
    }

    #[test]
    fn options_are_chosen_per_learned_book_and_switchable() {
        setup();
        let book = &books_of(Hero::Warrior).enhancement[0];
        let mut state = BookState::default();
        assert_eq!(
            state.set_active(Hero::Warrior, &book.id, &book.options[0].id),
            Err(BookError::NotLearned)
        );
        state.learn(Hero::Warrior, &book.id).expect("learn");
        assert!(state.active_nodes(Hero::Warrior).is_empty(), "none chosen yet");
        assert_eq!(
            state.set_active(Hero::Warrior, &book.id, "nope"),
            Err(BookError::UnknownOption)
        );
        state
            .set_active(Hero::Warrior, &book.id, &book.options[0].id)
            .expect("choose");
        assert_eq!(state.active_nodes(Hero::Warrior), [book.options[0].node.clone()]);
        state
            .set_active(Hero::Warrior, &book.id, &book.options[2].id)
            .expect("switch");
        assert_eq!(state.active_nodes(Hero::Warrior), [book.options[2].node.clone()]);
        let talent = &books_of(Hero::Warrior).talent[0].id;
        state.learn(Hero::Warrior, talent).expect("learn");
        assert_eq!(
            state.set_active(Hero::Warrior, talent, "x"),
            Err(BookError::NotEnhancement)
        );
    }

    #[test]
    fn the_talent_budget_includes_learned_books_and_still_rejects_overspending() {
        use crate::talents::{TalentError, validate};
        setup();
        let keys = |names: &[&str]| names.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>();
        // Warrior: unbroken_momentum 3, shockwave 3, executioner 4. The start gives 5 points.
        let two = keys(&["unbroken_momentum", "shockwave"]);
        assert_eq!(validate(Hero::Warrior, &two, 0), Err(TalentError::NotEnoughPoints));
        let mut state = BookState::default();
        let books = books_of(Hero::Warrior);
        state.learn(Hero::Warrior, &books.talent[0].id).expect("learn");
        assert!(validate(Hero::Warrior, &two, state.point_bonus(Hero::Warrior)).is_ok());
        // Two books make 7 points: 3 + 4 fits, 3 + 3 + 4 = 10 does not, whatever the books.
        state.learn(Hero::Warrior, &books.talent[1].id).expect("learn");
        let bonus = state.point_bonus(Hero::Warrior);
        assert_eq!(bonus, 2);
        assert!(validate(Hero::Warrior, &keys(&["unbroken_momentum", "executioner"]), bonus).is_ok());
        assert_eq!(
            validate(
                Hero::Warrior,
                &keys(&["unbroken_momentum", "shockwave", "executioner"]),
                bonus
            ),
            Err(TalentError::NotEnoughPoints)
        );
        // An enhancement book adds no point.
        state.learn(Hero::Warrior, &books.enhancement[0].id).expect("learn");
        assert_eq!(state.point_bonus(Hero::Warrior), 2);
    }

    #[test]
    fn stale_state_is_harmless() {
        setup();
        let mut state = BookState {
            unlocked: vec!["retired_book".into(), "retired_book".into()],
            active: BTreeMap::from([("retired_book".into(), "x".into())]),
        };
        assert_eq!(state.point_bonus(Hero::Warrior), 0);
        assert!(state.active_nodes(Hero::Warrior).is_empty());
        let book = &books_of(Hero::Warrior).enhancement[0];
        state.unlocked.push(book.id.clone());
        state.active.insert(book.id.clone(), "retired_option".into());
        assert!(state.active_nodes(Hero::Warrior).is_empty());
        let json = serde_json::to_string(&BookState::default()).expect("json");
        assert_eq!(json, r#"{"unlocked":[],"active":{}}"#);
        assert_eq!(serde_json::from_str::<BookState>("{}").expect("empty"), BookState::default());
    }
}
