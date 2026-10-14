//! Which playable character the player is, and which abilities that gives them.
//!
//! ```text
//! Hero ──► bar(): [Option<FireAbility>; ABILITY_SLOTS] ──► the action bar's defaults
//!      └─► every ability's `AbilityDef::hero` says which hero owns it
//! ```
//!
//! # Why a hero is an index into the one ability table
//!
//! There is exactly one ability table in the project ([`crate::firemage::ABILITIES`]) and
//! exactly one executor that reads it. A second class implemented as a second table would be
//! a second set of cooldown rules, a second global cooldown, a second tooltip generator and a
//! second thing for the damage meter to learn — four opportunities for two classes to
//! disagree about what a cast is.
//!
//! So a hero is not a new system: it is a *slice* of the table. Every row names its owner,
//! and a hero's bar is the rows that name it, in bar order. Everything downstream — the
//! executor, the meter, the tooltips, the action bar, the simulator — reads a row and never
//! asks who is casting it.
//!
//! # What a hero is not
//!
//! It is not a stat block and it is not a resource. Heat, Ignite and the talent grid belong
//! to [`crate::firemage::FireMage`] and are the FireMage's; the Warrior and the Druid
//! generate no Heat and apply no burn because their rows say zero, not because anything
//! branches on who they are.

use crate::character::stat::{Stat, StatBlock};
use crate::character::{
    BASE_MAIN_STAT, BASE_STAMINA, WARRIOR_BASE_ARMOR, WARRIOR_BASE_AVOIDANCE_RATING,
};
use crate::firemage::{ABILITY_SLOTS, FireAbility};

/// Which playable character the player is.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hero {
    /// Fire, Heat, Ignite and the payoff. The class the game was built around.
    #[default]
    FireMage,
    /// The tank. Block, Parry and Dodge pay a resource; the resource pays the kit.
    Warrior,
    /// In human form: the moon, the stars, and a heal that is not a cooldown.
    Druid,
}

impl Hero {
    /// Every hero, in selection order.
    pub const ALL: [Self; 3] = [Self::FireMage, Self::Warrior, Self::Druid];

    /// Dense index, for an array or a settings file.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::FireMage => 0,
            Self::Warrior => 1,
            Self::Druid => 2,
        }
    }

    /// The hero at `index`, if there is one.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// Uppercase display name. The UI font has no lowercase glyphs.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::FireMage => "FIREMAGE",
            Self::Warrior => "WARRIOR",
            Self::Druid => "DRUID",
        }
    }

    /// One line of what the hero is, for the swap menu.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::FireMage => "RANGED FIRE. BANK A BURN, THEN SPEND IT",
            Self::Warrior => "TANK. BUILD VALOR INTO BATTLE FERVOR",
            Self::Druid => "HUMAN FORM. MOON, STARS AND A HEAL",
        }
    }

    /// The stable written name a configuration file stores.
    ///
    /// Lowercase and stable across builds, and deliberately not [`Hero::name`]: renaming a
    /// hero on screen must not silently reset which hero a player was.
    #[must_use]
    pub fn config_name(self) -> &'static str {
        match self {
            Self::FireMage => "firemage",
            Self::Warrior => "warrior",
            Self::Druid => "druid",
        }
    }

    /// The hero with this written name, if there is one.
    #[must_use]
    pub fn from_config_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|hero| hero.config_name() == name)
    }

    /// The name of the model this hero is drawn with.
    ///
    /// A *name*, not a mesh: `engine-game` depends on `engine-core` and nothing else, and it
    /// must not learn that a triangle exists. The visual layer looks the name up in its model
    /// library, exactly as it already does for the FireMage.
    #[must_use]
    pub fn model_name(self) -> &'static str {
        match self {
            Self::FireMage => "fire_mage",
            Self::Warrior => "warrior",
            Self::Druid => "druid",
        }
    }

    /// The abilities this hero can cast, in bar order.
    #[must_use]
    pub fn abilities(self) -> &'static [FireAbility] {
        match self {
            Self::FireMage => &[
                FireAbility::Firebolt,
                FireAbility::FlameLance,
                FireAbility::FlameCone,
                FireAbility::Meteor,
                FireAbility::Pyroblast,
                FireAbility::Firestorm,
                FireAbility::Detonate,
                FireAbility::FireMastery,
                FireAbility::FlameShield,
                FireAbility::Blink,
            ],
            Self::Warrior => &[
                FireAbility::Recuperate,
                FireAbility::WarStomp,
                FireAbility::ColossusStrike,
                FireAbility::ShieldCharge,
                FireAbility::ShieldBash,
                FireAbility::ShieldBlock,
                FireAbility::PunctureBlood,
                FireAbility::Adrenaline,
                FireAbility::BattleTrance,
                FireAbility::SpellReflect,
                // Twelve abilities on a ten-key bar. Taunt sits on the button beside it — see
                // `Hero::extras` — and the twelfth is the Ultimate on `R`.
                FireAbility::Taunt,
                FireAbility::LastStand,
            ],
            Self::Druid => &[
                FireAbility::Moonfire,
                FireAbility::Sunfire,
                FireAbility::Wrath,
                FireAbility::Rejuvenation,
                FireAbility::Regrowth,
                FireAbility::Lifebloom,
                FireAbility::WildGrowth,
                FireAbility::Swiftmend,
                FireAbility::Nourish,
                FireAbility::NaturesCleanse,
                // Thirteen abilities on a ten-key bar. Two of them sit on the buttons beside
                // it — see `Hero::extras` — and the thirteenth is the Ultimate on `R`.
                FireAbility::WildRush,
                FireAbility::AstralPower,
                FireAbility::Tranquility,
            ],
        }
    }

    /// The hero's bar: its abilities on the digits, and nothing in the slots it does not
    /// fill.
    ///
    /// A fixed-width array rather than a slice so that "slot 4" means the same key for every
    /// hero. A hero with three abilities leaves seven slots empty; it does not renumber the
    /// keyboard.
    ///
    /// A hero with *more* than ten does not renumber it either: the extra ability is the
    /// hero's Ultimate, sits on `R`, and is excluded here by the `take` — see
    /// [`Hero::ultimate`].
    #[must_use]
    pub fn bar(self) -> [Option<FireAbility>; ABILITY_SLOTS] {
        let mut bar = [None; ABILITY_SLOTS];
        for (slot, ability) in self.abilities().iter().enumerate().take(ABILITY_SLOTS) {
            bar[slot] = Some(*ability);
        }
        bar
    }

    /// The ability on this hero's bar in `slot`, if there is one.
    #[must_use]
    pub fn ability_at(self, slot: usize) -> Option<FireAbility> {
        if slot >= ABILITY_SLOTS {
            return None;
        }
        self.abilities().get(slot).copied()
    }

    /// Whether this hero can cast `ability`.
    #[must_use]
    pub fn has(self, ability: FireAbility) -> bool {
        ability.hero() == self
    }

    /// The furthest anything this hero has can reach, in metres.
    ///
    /// What "in range" means for *selection*: a target further away than this cannot be hit
    /// by anything on the bar, so putting it in the target cycle is offering the player
    /// something to select and then be refused for. Derived from the hero's own rows, which
    /// is why a Warrior's cycle is five metres deep and a mage's is thirty-five.
    #[must_use]
    pub fn longest_range(self) -> f32 {
        self.abilities()
            .iter()
            .map(|ability| ability.definition().range)
            .fold(0.0, f32::max)
    }

    /// Which stat this hero's damage and healing scale from.
    ///
    /// A property of the hero rather than of the character sheet, so a sheet, a tooltip and
    /// a damage calculation cannot disagree about which of the two primary stats matters.
    #[must_use]
    pub fn main_stat(self) -> Stat {
        match self {
            Self::FireMage | Self::Druid => Stat::Intellect,
            Self::Warrior => Stat::Strength,
        }
    }

    /// What this hero is worth with nothing equipped.
    ///
    /// The main stat and Stamina, and zero of everything else. Every hero has the same base
    /// numbers on purpose: what separates the three is their abilities, and a base block
    /// that differed per class would be three balance passes before there is one.
    #[must_use]
    pub fn base_stats(self) -> StatBlock {
        let mut stats = StatBlock::of(self.main_stat(), BASE_MAIN_STAT);
        stats.set(Stat::Stamina, BASE_STAMINA);
        // The one exception to "every hero has the same base numbers", and it is the tank's:
        // Armour, Block, Parry and Dodge are what the Warrior's whole resource is generated
        // by, and a class whose resource cannot move at base gear is a class that cannot be
        // played until it is geared. See `crate::character::WARRIOR_BASE_ARMOR`.
        if matches!(self, Self::Warrior) {
            stats.set(Stat::Armor, WARRIOR_BASE_ARMOR);
            stats.set(Stat::Block, WARRIOR_BASE_AVOIDANCE_RATING);
            stats.set(Stat::Parry, WARRIOR_BASE_AVOIDANCE_RATING);
            stats.set(Stat::Dodge, WARRIOR_BASE_AVOIDANCE_RATING);
        }
        stats
    }

    /// The abilities this hero keeps **beside** the bar, other than the Ultimate.
    ///
    /// A hero with more abilities than the keyboard has digits does not renumber the
    /// keyboard: the overflow goes on the two buttons next to the Ultimate, on `F` and `G`.
    /// The Ultimate is excluded here because it already has a place of its own — which is
    /// what lets the Warrior's eleventh ability and the Druid's twelfth and thirteenth be
    /// three different things without three rules.
    pub fn extras(self) -> impl Iterator<Item = FireAbility> {
        let ultimate = self.ultimate();
        self.abilities()
            .iter()
            .copied()
            .skip(ABILITY_SLOTS)
            .filter(move |ability| Some(*ability) != ultimate)
    }

    /// Which of this hero's extra buttons `ability` sits on, if it sits on one.
    #[must_use]
    pub fn extra_index(self, ability: FireAbility) -> Option<usize> {
        self.extras().position(|extra| extra == ability)
    }

    /// **The command that casts `ability` for this hero**, wherever its button lives.
    ///
    /// The one answer to "which command is this button", for every place a press can come
    /// from — a key, a click on a slot, a confirmed targeting circle. A bar ability is its
    /// slot; a button beside the bar is its extra index; the Ultimate carries a position.
    ///
    /// `aim` is where a placed ability was put. An ability that is not placed has none, and
    /// the Ultimate is then cast at `caster` — where the player stands — so a self-cast
    /// Ultimate does not turn the character towards an arbitrary point.
    ///
    /// This is what was wrong with Taunt on `F`: the application sent every ability without
    /// a bar slot through the Ultimate's placement circle, so the key opened a circle and the
    /// click that followed cast the *Ultimate* rather than Taunt. `None` for an ability this
    /// hero does not have.
    #[must_use]
    pub fn command_for(
        self,
        ability: FireAbility,
        aim: Option<engine_core::math::Vec3>,
        caster: engine_core::math::Vec3,
    ) -> Option<crate::command::PlayerCommand> {
        use crate::command::PlayerCommand;
        if !self.has(ability) {
            return None;
        }
        if let Some(slot) = ability.bar_slot() {
            return Some(match aim {
                Some(position) => PlayerCommand::UseAbilityAt { slot, position },
                None => PlayerCommand::UseAbility(slot),
            });
        }
        if let Some(index) = self.extra_index(ability) {
            return Some(PlayerCommand::UseExtra(index));
        }
        (self.ultimate() == Some(ability)).then(|| PlayerCommand::UseUltimate {
            position: aim.unwrap_or(caster),
        })
    }

    /// This hero's Ultimate: the ability on `R`, beside the bar.
    ///
    /// The FireMage's is Cataclysm, which has no design yet and is switched off — see
    /// [`crate::firemage::ULTIMATE_ENABLED`]. The Warrior's is Last Stand and the Druid's is
    /// Tranquility, and both are real buttons: a class with more abilities than the bar has
    /// keys puts its largest one in the slot beside the bar, which already exists rather
    /// than being invented for it.
    #[must_use]
    pub fn ultimate(self) -> Option<FireAbility> {
        match self {
            Self::FireMage => Some(FireAbility::Cataclysm),
            Self::Warrior => Some(FireAbility::LastStand),
            Self::Druid => Some(FireAbility::Tranquility),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn every_ability_maps_to_the_command_that_casts_it_and_no_other() {
        use crate::command::PlayerCommand;
        let caster = engine_core::math::Vec3::new(1.0, 0.0, 2.0);
        for hero in Hero::ALL {
            for &ability in hero.abilities() {
                let command = hero
                    .command_for(ability, None, caster)
                    .expect("every ability of a hero has a command");
                let cast = match command {
                    PlayerCommand::UseAbility(slot) => hero.ability_at(slot),
                    PlayerCommand::UseExtra(index) => hero.extras().nth(index),
                    PlayerCommand::UseUltimate { position } => {
                        assert_eq!(position, caster, "{ability:?} was aimed somewhere");
                        hero.ultimate()
                    }
                    other => panic!("{ability:?} became {other:?}"),
                };
                assert_eq!(cast, Some(ability), "{hero:?} {ability:?} -> {command:?}");
            }
            // Another hero's ability is nobody's button here.
            for other in Hero::ALL.into_iter().filter(|other| *other != hero) {
                for &ability in other.abilities() {
                    assert_eq!(hero.command_for(ability, None, caster), None);
                }
            }
        }
    }
    use crate::firemage::{ABILITIES, ABILITY_SLOTS, FireMageRow};

    #[test]
    fn every_hero_round_trips_through_its_index_and_its_written_name() {
        for hero in Hero::ALL {
            assert_eq!(Hero::from_index(hero.index()), Some(hero));
            assert_eq!(Hero::from_config_name(hero.config_name()), Some(hero));
        }
        assert_eq!(Hero::from_index(Hero::ALL.len()), None);
        assert_eq!(Hero::from_config_name("necromancer"), None);
    }

    #[test]
    fn every_castable_ability_belongs_to_exactly_one_heros_bar() {
        // The property the whole design rests on: an ability's bar slot can be derived from
        // the ability alone, because no two heroes share one.
        for ability in FireAbility::ALL {
            if !ability.is_assignable() {
                continue;
            }
            let owners: Vec<Hero> = Hero::ALL
                .into_iter()
                .filter(|hero| hero.abilities().contains(&ability))
                .collect();
            assert_eq!(owners.len(), 1, "{ability:?} is owned by {owners:?}");
            assert_eq!(owners[0], ability.hero(), "{ability:?}");
        }
    }

    #[test]
    fn a_heros_bar_is_its_abilities_and_then_nothing() {
        for hero in Hero::ALL {
            let bar = hero.bar();
            let filled = bar.iter().filter(|slot| slot.is_some()).count();
            assert_eq!(filled, hero.abilities().len().min(ABILITY_SLOTS));
            for (slot, ability) in hero.abilities().iter().enumerate().take(ABILITY_SLOTS) {
                assert_eq!(bar[slot], Some(*ability));
                assert_eq!(hero.ability_at(slot), Some(*ability));
                assert_eq!(ability.bar_slot(), Some(slot), "{ability:?}");
            }
        }
    }

    #[test]
    fn the_other_two_heroes_generate_no_heat_and_no_ignite() {
        // What makes them different classes rather than a mage with different names: their
        // rows carry no FireMage columns at all, so they cannot generate Heat, apply Ignite or
        // cost Energy. The type holds that; this pins each hero's rows to its own class.
        for hero in [Hero::Warrior, Hero::Druid] {
            for ability in hero.abilities() {
                let definition = &ABILITIES[ability.index()];
                assert_eq!(definition.hero(), hero, "{ability:?}");
                assert_eq!(*definition.fire(), FireMageRow::NONE, "{ability:?}");
            }
        }
        assert_eq!(Hero::Druid.abilities().len(), 13);
        assert_eq!(Hero::Druid.ultimate(), Some(FireAbility::Tranquility));
        // Ten on the digits, two beside the bar, and the Ultimate on `R`.
        assert_eq!(
            Hero::Druid.extras().collect::<Vec<_>>(),
            [FireAbility::WildRush, FireAbility::AstralPower]
        );
        assert_eq!(Hero::Druid.extra_index(FireAbility::AstralPower), Some(1));
        assert_eq!(FireAbility::WildRush.key_hint(), "F");
        assert_eq!(FireAbility::AstralPower.key_hint(), "G");
        assert_eq!(FireAbility::Tranquility.key_hint(), "R");
        // The Warrior has one button beside the bar: Taunt, on `F`.
        assert_eq!(
            Hero::Warrior.extras().collect::<Vec<_>>(),
            [FireAbility::Taunt]
        );
        assert_eq!(FireAbility::Taunt.key_hint(), "F");
        // And pressing it is Taunt's own command, not the Ultimate's placement circle.
        let here = engine_core::math::Vec3::ZERO;
        assert_eq!(
            Hero::Warrior.command_for(FireAbility::Taunt, None, here),
            Some(crate::command::PlayerCommand::UseExtra(0))
        );
    }

    #[test]
    fn the_warrior_is_twelve_abilities_on_ten_keys_an_extra_and_an_ultimate() {
        assert_eq!(Hero::Warrior.abilities().len(), 12);
        let bar = Hero::Warrior.bar();
        assert_eq!(bar.iter().filter(|slot| slot.is_some()).count(), 10);
        assert_eq!(Hero::Warrior.ultimate(), Some(FireAbility::LastStand));
        // The last two are not on a digit, and asking for an eleventh slot answers nothing
        // rather than an ability that lives beside the bar.
        assert_eq!(Hero::Warrior.ability_at(ABILITY_SLOTS), None);
        assert!(FireAbility::LastStand.is_ultimate());
        assert!(!FireAbility::LastStand.is_disabled());
    }

    #[test]
    fn only_the_warrior_starts_with_defences() {
        use crate::character::stat::Stat;
        for hero in Hero::ALL {
            let stats = hero.base_stats();
            let expected = f32::from(u8::from(matches!(hero, Hero::Warrior)));
            for stat in [Stat::Armor, Stat::Block, Stat::Parry, Stat::Dodge] {
                let value = stats.get(stat);
                assert_eq!(
                    value > 0.0,
                    expected > 0.0,
                    "{hero:?} {}: {value}",
                    stat.name()
                );
            }
        }
    }
}
