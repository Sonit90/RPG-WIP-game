//! Who the player is grouped with.
//!
//! ```text
//! Party  ->  UiSnapshot::party  ->  the party frames
//! ```
//!
//! A roster of entity ids and nothing else. The party owns no health, no name and no
//! position: those live in [`GameWorld`](crate::world::GameWorld) beside every other
//! entity's, addressed by the same [`EntityId`]. A roster that carried its own copy would
//! be a second truth about how much health a member has, and the interface would have to be
//! told which one to believe.
//!
//! ## Why this is small
//!
//! There are no ally entities in the game yet, so a session starts as a party of one: the
//! local player. That is the honest answer, and it is what the party frames show. The type
//! exists so the interface reads a real roster that grows the moment something calls
//! [`Party::join`] — the alternative, a frame that invents four teammates, would be a mock
//! with nothing behind it.
//!
//! The local player is always index zero, so "the party" and "the player and their party"
//! are the same list and no caller has to remember to prepend anyone.

use crate::entity::EntityId;

/// How many characters a party holds, including the local player.
///
/// Five is the size the frames are laid out for. It is a constant rather than a literal so
/// the roster, the layout and the tests cannot disagree about it.
pub const MAX_PARTY: usize = 5;

/// The player's group, in display order, with the local player first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Party {
    members: Vec<EntityId>,
}

impl Party {
    /// An empty party. No player has been named yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            members: Vec::with_capacity(MAX_PARTY),
        }
    }

    /// Makes `entity` the local player: index zero, joining if it is not a member yet.
    ///
    /// Idempotent, and it moves an existing member to the front rather than duplicating it,
    /// because the local player changing is a re-anchoring of the same list.
    pub fn set_leader(&mut self, entity: EntityId) {
        self.members.retain(|member| *member != entity);
        self.members.insert(0, entity);
        self.members.truncate(MAX_PARTY);
    }

    /// Adds a member, if there is room and it is not already in.
    ///
    /// Returns whether the roster changed, so a caller can tell "joined" from "was already
    /// here" and from "the party is full" without comparing lengths itself.
    pub fn join(&mut self, entity: EntityId) -> bool {
        if self.members.len() >= MAX_PARTY || self.contains(entity) {
            return false;
        }
        self.members.push(entity);
        true
    }

    /// Removes a member. Returns whether it was in the party.
    ///
    /// Order is preserved: a member leaving must not shuffle the frames of the ones who
    /// stayed, which is what a swap-remove would do.
    pub fn leave(&mut self, entity: EntityId) -> bool {
        let Some(index) = self.members.iter().position(|member| *member == entity) else {
            return false;
        };
        self.members.remove(index);
        true
    }

    /// Empties the roster.
    pub fn clear(&mut self) {
        self.members.clear();
    }

    /// Whether `entity` is in the party.
    #[must_use]
    pub fn contains(&self, entity: EntityId) -> bool {
        self.members.contains(&entity)
    }

    /// How many members there are, including the local player.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether nobody is in the party, which is only true before a player is named.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The members, in display order, local player first.
    #[must_use]
    pub fn members(&self) -> &[EntityId] {
        &self.members
    }

    /// The member in display position `index`.
    #[must_use]
    pub fn member(&self, index: usize) -> Option<EntityId> {
        self.members.get(index).copied()
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn entity(index: u32) -> EntityId {
        EntityId::new(index, 1)
    }

    #[test]
    fn a_fresh_party_is_empty_and_the_leader_is_the_first_member() {
        let mut party = Party::new();
        assert!(party.is_empty());

        party.set_leader(entity(7));
        assert_eq!(party.len(), 1);
        assert_eq!(party.member(0), Some(entity(7)));
        assert!(party.contains(entity(7)));
    }

    #[test]
    fn naming_the_leader_twice_does_not_duplicate_them() {
        let mut party = Party::new();
        party.set_leader(entity(1));
        party.set_leader(entity(1));
        assert_eq!(party.len(), 1);
    }

    #[test]
    fn an_existing_member_promoted_to_leader_moves_to_the_front() {
        let mut party = Party::new();
        party.set_leader(entity(1));
        party.join(entity(2));
        party.set_leader(entity(2));

        assert_eq!(party.members(), [entity(2), entity(1)]);
        assert_eq!(party.len(), 2, "promotion duplicated a member");
    }

    #[test]
    fn members_join_and_leave_without_disturbing_the_order() {
        let mut party = Party::new();
        party.set_leader(entity(1));
        assert!(party.join(entity(2)));
        assert!(party.join(entity(3)));
        assert!(party.join(entity(4)));

        assert!(party.leave(entity(3)));
        // Two stays where it was: a member leaving must not shuffle everyone else's frame.
        assert_eq!(party.members(), [entity(1), entity(2), entity(4)]);
        assert!(!party.leave(entity(3)), "left twice");
    }

    #[test]
    fn a_party_never_grows_past_its_maximum_and_never_holds_a_duplicate() {
        let mut party = Party::new();
        party.set_leader(entity(0));
        for index in 1..MAX_PARTY as u32 {
            assert!(party.join(entity(index)), "member {index} was refused");
        }
        assert_eq!(party.len(), MAX_PARTY);

        assert!(!party.join(entity(99)), "a full party accepted a member");
        assert!(!party.join(entity(1)), "a duplicate was accepted");
        assert_eq!(party.len(), MAX_PARTY);
    }

    #[test]
    fn clearing_empties_the_roster() {
        let mut party = Party::new();
        party.set_leader(entity(1));
        party.join(entity(2));
        party.clear();
        assert!(party.is_empty());
        assert!(!party.contains(entity(1)));
    }
}
