//! What identifies an ability in an event.
//!
//! The class itself - what the abilities are, what they cost, what they do - lives in
//! [`crate::firemage`]. What is left here is the identity an event carries: a small integer
//! that damage, casts and the combat meter can bucket by without knowing anything about fire.
//!
//! Keeping the two apart is what lets telemetry hold a fixed array instead of a map, and what
//! would let a second class exist without the event stream learning a second ability enum.

use crate::firemage::ability::{ABILITY_COUNT, FireAbility};

/// How many distinct abilities can be named. The ten-slot bar plus the Ultimate.
pub const ABILITY_SLOTS: usize = ABILITY_COUNT;

/// Which ability an event came from.
///
/// A dense index rather than a name, so telemetry can bucket by ability with an array and a
/// future class can reassign what lives behind an index without changing the event shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AbilityId(u8);

impl AbilityId {
    /// The id used by damage that came from no ability - a fall, or a future hazard.
    pub const NONE: Self = Self(u8::MAX);

    /// The ability at `index`, or `None` if there is no such ability.
    #[must_use]
    pub const fn from_slot(index: usize) -> Option<Self> {
        // The cast cannot truncate: the branch bounds the index by ABILITY_SLOTS.
        if index < ABILITY_SLOTS {
            Some(Self(index as u8))
        } else {
            None
        }
    }

    /// The identity of a FireMage ability.
    #[must_use]
    pub fn from_fire(ability: FireAbility) -> Self {
        Self(ability.index() as u8)
    }

    /// Which FireMage ability this names, if it names one.
    #[must_use]
    pub fn fire(self) -> Option<FireAbility> {
        FireAbility::ALL.get(self.slot()).copied()
    }

    /// The index this id names.
    #[must_use]
    pub const fn slot(self) -> usize {
        self.0 as usize
    }

    /// Whether this names a real ability.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        (self.0 as usize) < ABILITY_SLOTS
    }

    /// The name of the ability, or `UNKNOWN` for [`AbilityId::NONE`].
    #[must_use]
    pub fn name(self) -> &'static str {
        match self.fire() {
            Some(ability) => ability.definition().name,
            None => "UNKNOWN",
        }
    }

    /// The key that casts it: `1` to `0` across the bar, `R` for the Ultimate.
    #[must_use]
    pub fn key_hint(self) -> &'static str {
        match self.fire() {
            Some(ability) => ability.key_hint(),
            None => "",
        }
    }
}

impl From<FireAbility> for AbilityId {
    fn from(ability: FireAbility) -> Self {
        Self::from_fire(ability)
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
    fn every_firemage_ability_has_an_identity_and_round_trips() {
        for ability in FireAbility::ALL {
            let id = AbilityId::from_fire(ability);
            assert!(id.is_valid());
            assert_eq!(id.fire(), Some(ability));
            assert_eq!(id.name(), ability.definition().name);
        }
    }

    #[test]
    fn the_ultimate_has_an_identity_of_its_own() {
        // It is not on the bar, but its damage still has to reach the meter.
        let id = AbilityId::from_fire(FireAbility::Cataclysm);
        assert!(id.is_valid());
        assert_eq!(id.slot(), ABILITY_SLOTS - 1);
        assert_eq!(id.key_hint(), "R");
    }

    #[test]
    fn nothing_is_a_valid_slot_and_names_no_ability() {
        assert!(!AbilityId::NONE.is_valid());
        assert_eq!(AbilityId::NONE.fire(), None);
        assert_eq!(AbilityId::NONE.name(), "UNKNOWN");
        assert_eq!(AbilityId::from_slot(ABILITY_SLOTS), None);
    }

    #[test]
    fn slots_are_dense_and_ordered_like_the_bar() {
        for (index, ability) in FireAbility::ALL.into_iter().enumerate() {
            assert_eq!(
                AbilityId::from_slot(index).and_then(AbilityId::fire),
                Some(ability)
            );
        }
    }
}
