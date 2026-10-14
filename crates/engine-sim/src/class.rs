//! The three classes the simulator can play, and what is particular to each.
//!
//! A [`Class`] is a thin name for an [`engine_game::hero::Hero`]: it exists so the command
//! line can say `mage` (the game says `FireMage`) and so a class can carry the two facts the
//! simulator needs and the game does not state in one place - how close it fights, and which
//! resources its report should show.

use engine_game::hero::Hero;

/// One of the three playable classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    /// The tank, played here as a damage dealer.
    Warrior,
    /// The healer, played here as a damage dealer.
    Druid,
    /// The FireMage.
    Mage,
}

impl Class {
    /// Every class, in report order.
    pub const ALL: [Self; 3] = [Self::Warrior, Self::Druid, Self::Mage];

    /// The game's hero for this class.
    #[must_use]
    pub fn hero(self) -> Hero {
        match self {
            Self::Warrior => Hero::Warrior,
            Self::Druid => Hero::Druid,
            Self::Mage => Hero::FireMage,
        }
    }

    /// The name the command line and the report use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warrior => "warrior",
            Self::Druid => "druid",
            Self::Mage => "mage",
        }
    }

    /// The name with a capital, for a heading.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Warrior => "Warrior",
            Self::Druid => "Druid",
            Self::Mage => "Mage",
        }
    }

    /// Parses `warrior`, `druid` or `mage`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.as_str() == text)
    }

    /// How far from the pack the class starts the fight, in metres.
    ///
    /// A caster opens at range; the Warrior's abilities reach five to eight metres, so it
    /// starts at the edge of melee and the rotation walks the rest.
    #[must_use]
    pub fn engage_distance(self) -> f32 {
        match self {
            Self::Warrior => 3.0,
            Self::Druid | Self::Mage => 12.0,
        }
    }

    /// The name of the class's primary stat, for the report.
    #[must_use]
    pub fn primary_stat(self) -> &'static str {
        self.hero().main_stat().name()
    }
}
