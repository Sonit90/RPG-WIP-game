//! Books: the data behind the two book kinds, and the rules that tie them to hidden talent nodes.
//!
//! ```text
//! talents.json books -> HeroBooks (this file) -> option.node -> hidden row of the hero's table
//!                                                   '-> TalentGrid::grant_index -> <Hero>TalentEffects
//! ```
//!
//! A **talent book** is worth one talent point once learned. An **enhancement book** changes one
//! signature ability; it offers three options and the player keeps one active. Each option is a
//! hidden talent node (tier [`super::HIDDEN_TIER`], cost 0): switching an option on is
//! [`TalentGrid::grant_index`], so the effects are resolved by the same `from_grid` as every other
//! talent. There is no second effect path. This crate keeps no serde; `engine-realm` reads the file
//! and keeps what a character has learned.

use super::{HeroTalents, TalentGrid};
use crate::ability::FireAbility;
use crate::hero::Hero;

/// How many options an enhancement book offers.
pub const OPTIONS_PER_BOOK: usize = 3;

/// A book worth one permanent talent point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TalentBook {
    /// Stable id; also the key of the book item.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The lowest key level at which a boss can drop it.
    pub min_key_level: u32,
}

/// One option of an enhancement book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookOption {
    /// Stable id within its book.
    pub id: String,
    /// Display name.
    pub name: String,
    /// What it does, for the book panel.
    pub description: String,
    /// The key of the hidden talent node it switches on.
    pub node: String,
}

/// A book that changes one ability, three ways.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnhancementBook {
    /// Stable id; also the key of the book item.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The ability it is about, by config name (`"war_stomp"`).
    pub ability: String,
    /// The lowest key level at which a boss can drop it.
    pub min_key_level: u32,
    /// The options, exactly [`OPTIONS_PER_BOOK`].
    pub options: Vec<BookOption>,
}

/// Which kind a book is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookKind {
    /// +1 talent point.
    TalentPoint,
    /// Three options for one ability.
    Enhancement,
}

/// One hero's books.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeroBooks {
    /// The talent-point books.
    pub talent: Vec<TalentBook>,
    /// The enhancement books.
    pub enhancement: Vec<EnhancementBook>,
}

impl HeroBooks {
    /// The talent book with this id.
    #[must_use]
    pub fn talent_book(&self, id: &str) -> Option<&TalentBook> {
        self.talent.iter().find(|book| book.id == id)
    }

    /// The enhancement book with this id.
    #[must_use]
    pub fn enhancement_book(&self, id: &str) -> Option<&EnhancementBook> {
        self.enhancement.iter().find(|book| book.id == id)
    }

    /// Which kind of book `id` is, if it is this hero's.
    #[must_use]
    pub fn kind_of(&self, id: &str) -> Option<BookKind> {
        if self.talent_book(id).is_some() {
            Some(BookKind::TalentPoint)
        } else if self.enhancement_book(id).is_some() {
            Some(BookKind::Enhancement)
        } else {
            None
        }
    }

    /// Every book id, talent books first.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.talent
            .iter()
            .map(|book| book.id.as_str())
            .chain(self.enhancement.iter().map(|book| book.id.as_str()))
    }

    /// The hidden node key option `option` of book `book` switches on.
    #[must_use]
    pub fn node_of(&self, book: &str, option: &str) -> Option<&str> {
        self.enhancement_book(book)?
            .options
            .iter()
            .find(|candidate| candidate.id == option)
            .map(|candidate| candidate.node.as_str())
    }
}

/// The bit a book takes in a "which books are read" mask: its position among every hero's books
/// (heroes in [`Hero::ALL`] order, talent books then enhancement books). `None` for an unknown id.
/// How the interface's `Copy` character view carries which books are learned.
#[must_use]
pub fn book_bit(id: &str) -> Option<u32> {
    let catalog = super::catalog();
    let mut bit = 0;
    for hero in Hero::ALL {
        for candidate in catalog.books[hero.index()].ids() {
            if candidate == id {
                return Some(bit);
            }
            bit += 1;
        }
    }
    None
}

/// The hero a book is written for, by its id.
#[must_use]
pub fn book_hero(id: &str) -> Option<Hero> {
    let catalog = super::catalog();
    Hero::ALL
        .into_iter()
        .find(|hero| catalog.books[hero.index()].kind_of(id).is_some())
}

/// Switches on the nodes of the active options (`(book id, option id)` pairs) in `grid`. A pair
/// that names no book or option of the grid's hero is ignored.
pub fn grant_active(grid: &mut TalentGrid, books: &HeroBooks, active: &[(&str, &str)]) {
    let table = super::hero_talents(grid.hero());
    for (book, option) in active {
        if let Some(index) = books
            .node_of(book, option)
            .and_then(|node| table.index_of(node))
        {
            grid.grant_index(index);
        }
    }
}

/// Whether one hero's books and hidden rows agree.
///
/// # Errors
///
/// A sentence naming the first problem: a hidden row before a visible one, a duplicate id, a book
/// without exactly [`OPTIONS_PER_BOOK`] options, an option whose node is missing, visible or shared,
/// a hidden row no option uses, an ability that is not the hero's, or a key-level gate of zero.
pub fn validate_hero(hero: Hero, table: &HeroTalents, books: &HeroBooks) -> Result<(), String> {
    let name = hero.name();
    let visible = table.visible().len();
    if table.talents[visible..]
        .iter()
        .any(|spec| !spec.is_hidden())
    {
        return Err(format!(
            "{name}: a hidden node comes before a visible talent"
        ));
    }
    let mut ids: Vec<&str> = Vec::new();
    for id in books.ids() {
        if id.trim().is_empty() || ids.contains(&id) {
            return Err(format!("{name}: book id {id:?} is empty or listed twice"));
        }
        ids.push(id);
    }
    if books.talent.iter().any(|book| book.min_key_level == 0)
        || books.enhancement.iter().any(|book| book.min_key_level == 0)
    {
        return Err(format!("{name}: a book has a key-level gate of zero"));
    }
    let mut used: Vec<&str> = Vec::new();
    for book in &books.enhancement {
        let on_class = FireAbility::from_config_name(&book.ability)
            .is_some_and(|ability| ability.hero() == hero);
        if !on_class {
            return Err(format!(
                "{name}: book {:?} names ability {:?}, which is not the hero's",
                book.id, book.ability
            ));
        }
        if book.options.len() != OPTIONS_PER_BOOK {
            return Err(format!(
                "{name}: book {:?} has {} options, it needs {OPTIONS_PER_BOOK}",
                book.id,
                book.options.len()
            ));
        }
        for (index, option) in book.options.iter().enumerate() {
            if option.id.trim().is_empty()
                || book.options[..index]
                    .iter()
                    .any(|other| other.id == option.id)
            {
                return Err(format!(
                    "{name}: book {:?} option {:?} is empty or listed twice",
                    book.id, option.id
                ));
            }
            let Some(row) = table.index_of(&option.node) else {
                return Err(format!(
                    "{name}: option {:?} names node {:?}, which does not exist",
                    option.id, option.node
                ));
            };
            if !table.talents[row].is_hidden() {
                return Err(format!(
                    "{name}: option {:?} names {:?}, which is a visible talent",
                    option.id, option.node
                ));
            }
            if used.contains(&option.node.as_str()) {
                return Err(format!("{name}: node {:?} serves two options", option.node));
            }
            used.push(&option.node);
        }
    }
    if !books.enhancement.is_empty()
        && let Some(orphan) = table.talents[visible..]
            .iter()
            .find(|spec| !used.contains(&spec.key.as_str()))
    {
        return Err(format!(
            "{name}: hidden node {:?} belongs to no option",
            orphan.key
        ));
    }
    Ok(())
}
