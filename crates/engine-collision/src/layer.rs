//! Which things collide with which, as two bitmasks.
//!
//! ```text
//! A collides with B  <=>  A.mask contains B.layer  AND  B.mask contains A.layer
//! ```
//!
//! Both directions are required, so "the player does not block the dummy" cannot be
//! expressed on one side only and then silently disagreed with by the other. The rule is
//! pure arithmetic over `u32`s and knows nothing about players, dummies or walls — a
//! gameplay crate assigns the meanings, and this crate never learns them.

/// One collision category, as a single bit.
///
/// A layer, not a set: an object is exactly one kind of thing. What it *cares about* is a
/// [`CollisionMask`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CollisionLayer(u32);

impl CollisionLayer {
    /// The character the player controls.
    pub const PLAYER: Self = Self(1 << 0);
    /// Static world geometry: floors, walls, ramps, steps.
    pub const WORLD: Self = Self(1 << 1);
    /// A training dummy or other non-player character.
    pub const DUMMY: Self = Self(1 << 2);

    /// A layer from a bit index in `0..32`.
    ///
    /// Returns `None` above 31, because a 33rd layer silently wrapping to the first is the
    /// kind of bug that only shows up as "the player fell through the floor once".
    #[must_use]
    pub const fn from_index(index: u32) -> Option<Self> {
        if index < 32 {
            Some(Self(1 << index))
        } else {
            None
        }
    }

    /// The raw bit.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// This layer on its own, as a mask.
    #[must_use]
    pub const fn as_mask(self) -> CollisionMask {
        CollisionMask(self.0)
    }
}

/// The set of layers something collides with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollisionMask(u32);

impl Default for CollisionMask {
    fn default() -> Self {
        Self::NONE
    }
}

impl CollisionMask {
    /// Collides with nothing.
    pub const NONE: Self = Self(0);
    /// Collides with everything.
    pub const ALL: Self = Self(u32::MAX);
    /// What a character controller collides with by default: static geometry only.
    pub const WORLD: Self = CollisionLayer::WORLD.as_mask();

    /// A mask from raw bits.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The raw bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// This mask with `layer` added.
    #[must_use]
    pub const fn with(self, layer: CollisionLayer) -> Self {
        Self(self.0 | layer.0)
    }

    /// This mask with `layer` removed.
    #[must_use]
    pub const fn without(self, layer: CollisionLayer) -> Self {
        Self(self.0 & !layer.0)
    }

    /// Whether `layer` is in the mask.
    #[must_use]
    pub const fn contains(self, layer: CollisionLayer) -> bool {
        self.0 & layer.0 != 0
    }

    /// Whether the two masks share any layer.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether the mask is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// What layer something is on and what it collides with.
///
/// Carried by every static object and by the character controller, so the filter question
/// is answered from two of these and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollisionFilter {
    /// What this object is.
    pub layer: CollisionLayer,
    /// What this object collides with.
    pub mask: CollisionMask,
}

impl CollisionFilter {
    /// A filter on `layer` colliding with `mask`.
    #[must_use]
    pub const fn new(layer: CollisionLayer, mask: CollisionMask) -> Self {
        Self { layer, mask }
    }

    /// Static world geometry: collides with everything that asks to collide with it.
    #[must_use]
    pub const fn world() -> Self {
        Self::new(CollisionLayer::WORLD, CollisionMask::ALL)
    }

    /// The player character: collides with static world geometry only.
    ///
    /// Not with dummies — see the decision record in `collision.md`. A dummy that blocks
    /// movement in a combat prototype is a dummy the player gets stuck on.
    #[must_use]
    pub const fn player() -> Self {
        Self::new(CollisionLayer::PLAYER, CollisionMask::WORLD)
    }

    /// A training dummy: present in the world, blocking nothing.
    #[must_use]
    pub const fn dummy() -> Self {
        Self::new(CollisionLayer::DUMMY, CollisionMask::NONE)
    }

    /// Whether these two objects collide.
    ///
    /// Symmetric by construction: both sides have to agree, so a caller cannot get a
    /// different answer by asking in the other order.
    #[must_use]
    pub const fn collides_with(self, other: Self) -> bool {
        self.mask.contains(other.layer) && other.mask.contains(self.layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layer_is_one_bit_and_the_indices_do_not_overlap() {
        assert_eq!(CollisionLayer::PLAYER.bits().count_ones(), 1);
        assert_eq!(CollisionLayer::WORLD.bits().count_ones(), 1);
        assert_eq!(CollisionLayer::DUMMY.bits().count_ones(), 1);
        let combined = CollisionLayer::PLAYER.bits()
            | CollisionLayer::WORLD.bits()
            | CollisionLayer::DUMMY.bits();
        assert_eq!(combined.count_ones(), 3, "two layers share a bit");
    }

    #[test]
    fn a_layer_index_past_the_word_is_refused_rather_than_wrapping() {
        assert_eq!(CollisionLayer::from_index(0), Some(CollisionLayer::PLAYER));
        assert!(CollisionLayer::from_index(31).is_some());
        assert_eq!(CollisionLayer::from_index(32), None);
        assert_eq!(CollisionLayer::from_index(u32::MAX), None);
    }

    #[test]
    fn masks_add_and_remove_layers() {
        let mask = CollisionMask::NONE
            .with(CollisionLayer::WORLD)
            .with(CollisionLayer::DUMMY);
        assert!(mask.contains(CollisionLayer::WORLD));
        assert!(mask.contains(CollisionLayer::DUMMY));
        assert!(!mask.contains(CollisionLayer::PLAYER));

        let narrowed = mask.without(CollisionLayer::DUMMY);
        assert!(!narrowed.contains(CollisionLayer::DUMMY));
        assert!(narrowed.contains(CollisionLayer::WORLD));
        assert!(CollisionMask::NONE.is_empty());
        assert!(!CollisionMask::ALL.is_empty());
    }

    #[test]
    fn the_player_collides_with_the_world_and_not_with_a_dummy() {
        let player = CollisionFilter::player();
        let world = CollisionFilter::world();
        let dummy = CollisionFilter::dummy();

        assert!(player.collides_with(world));
        assert!(world.collides_with(player));
        assert!(!player.collides_with(dummy));
        assert!(!dummy.collides_with(player));
    }

    #[test]
    fn filtering_is_symmetric_even_when_only_one_side_opts_out() {
        // The reason both directions are required: a one-sided mask would let the player
        // be blocked by something that is not blocked by the player.
        let one_sided = CollisionFilter::new(CollisionLayer::DUMMY, CollisionMask::ALL);
        let player = CollisionFilter::player();
        assert_eq!(
            player.collides_with(one_sided),
            one_sided.collides_with(player)
        );
        assert!(!player.collides_with(one_sided));
    }

    #[test]
    fn masks_intersect_when_they_share_a_layer() {
        let a = CollisionMask::NONE.with(CollisionLayer::WORLD);
        let b = CollisionMask::NONE.with(CollisionLayer::WORLD);
        let c = CollisionMask::NONE.with(CollisionLayer::PLAYER);
        assert!(a.intersects(b));
        assert!(!a.intersects(c));
        assert_eq!(CollisionMask::from_bits(a.bits()), a);
    }
}
