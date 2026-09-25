//! Naming the pieces an environment is built from.
//!
//! ```text
//!  terrain        ground_plain   arena_floor
//!  architecture   wall_segment   pillar        arch          dome
//!  rocks          rock_small     rock_medium   rock_spire
//!  vegetation     shrub          fern          pine
//!  props          brazier        crate         banner
//!  background     ridge          sky_dome
//! ```
//!
//! # Why a `&'static str` and not an enum
//!
//! An enum would make the library a closed set that every `match` in three crates has to be
//! extended for. A name is what a scene file will carry when scenes stop being Rust
//! functions, and it is what the model library already uses to resolve a character
//! ([`engine_assets::ModelId`] is the same idea). A `&'static str` keeps it allocation-free
//! and `Copy`, which matters because an id is compared once per placement at load and
//! stored in every object.
//!
//! # Why a newtype and not a bare `&'static str`
//!
//! So a placement cannot be given a material name, a debug label or an entity name by
//! mistake. The wrapper costs nothing at run time and the constants below are the only
//! sanctioned way to make one outside a test.

/// The name of one environment asset.
///
/// Resolved to a model by `engine_assets::procedural::environment::model` and to a
/// [`ModelBinding`](engine_render_data) by the application's upload pass. This crate never
/// resolves it to anything: it does not know what a model is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnvironmentAsset(&'static str);

impl EnvironmentAsset {
    /// Names an asset.
    ///
    /// `const` so the library below is a compile-time table rather than something built at
    /// startup.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// The name as text. What a log line, a debug label and a future scene file all use.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.0
    }
}

/// Every name that has been interned, so one string is one id.
///
/// A `Mutex` around a `Vec` rather than a lock-free structure: interning happens when a level
/// is loaded and when the editor scans a directory, which is a few hundred times per session
/// and never in a frame.
static INTERNED: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

impl EnvironmentAsset {
    /// The id for a name that is not known at compile time.
    ///
    /// A scene file carries names, and so does a directory of `.glb` files that nobody wrote
    /// a constant for. Both have to become an [`EnvironmentAsset`], and an
    /// [`EnvironmentAsset`] is a `&'static str` because it is `Copy`, compared once per
    /// placement and stored in every object.
    ///
    /// So a name is *interned*: looked up in a table of names already seen, and leaked once
    /// if it is new. The leak is bounded by the number of distinct assets a run touches — a
    /// few hundred strings of a few dozen bytes — and it buys an id that costs nothing to
    /// copy and nothing to compare.
    ///
    /// A name that already has a constant returns that constant's id, so `interned("pillar")`
    /// and [`PILLAR`] are the same asset and compare equal.
    ///
    /// # Panics
    ///
    /// Never. A poisoned intern table is recovered from rather than propagated: the table is
    /// append-only and a panic while holding it cannot leave it inconsistent.
    #[must_use]
    pub fn interned(name: &str) -> Self {
        if let Some(known) = LIBRARY.into_iter().find(|asset| asset.name() == name) {
            return known;
        }
        let mut table = INTERNED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(known) = table.iter().find(|known| **known == name) {
            return Self(known);
        }
        let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
        table.push(leaked);
        Self(leaked)
    }

    /// How many distinct names have been interned this run. For tests and diagnostics.
    ///
    /// # Panics
    ///
    /// Never; see [`EnvironmentAsset::interned`].
    #[must_use]
    pub fn interned_count() -> usize {
        INTERNED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

impl std::fmt::Display for EnvironmentAsset {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

/// The flat plain the arena sits on. One quad, tiled.
pub const GROUND_PLAIN: EnvironmentAsset = EnvironmentAsset::new("ground_plain");
/// The arena's own flagstone floor: a wide low-polygon disc.
pub const ARENA_FLOOR: EnvironmentAsset = EnvironmentAsset::new("arena_floor");

/// One length of the arena's boundary kerb.
pub const WALL_SEGMENT: EnvironmentAsset = EnvironmentAsset::new("wall_segment");
/// A standing stone pillar. Punctuates the kerb and carries a banner.
pub const PILLAR: EnvironmentAsset = EnvironmentAsset::new("pillar");
/// A broken arch: the arena's entrance and its one tall landmark.
pub const ARCH: EnvironmentAsset = EnvironmentAsset::new("arch");
/// A smooth stone dome: a round hill, wider than it is tall, that a character can walk up.
///
/// The one piece of architecture that is round in every direction, which is why the
/// line-of-sight course is built from it: a wall answers "is a wall a wall", and a dome
/// answers the harder question of what happens to a target standing part of the way up
/// something that curves away from the caster. Walking up it is the other half of that
/// question — see [`dome_profile`].
pub const DOME: EnvironmentAsset = EnvironmentAsset::new("dome");

/// How tall a dome is, as a fraction of its radius.
///
/// Not a hemisphere. A hemisphere's surface is vertical where it meets the ground, so the
/// only way onto one is to be placed on it — and a hill a player cannot walk up is a hill
/// that can only be looked at. At this ratio the steepest part of the surface is 35°, which
/// is inside the character controller's 50° limit with room for the approximation the
/// collision world makes of it.
pub const DOME_HEIGHT: f32 = 0.45;

/// The dome's profile: how high the surface is at `t`, the distance from the middle as a
/// fraction of the radius.
///
/// A raised cosine, on a unit dome, so a placement's scale is its radius and its height
/// follows. The curve is flat at the crest and flat again where it meets the ground, which
/// is what makes both ends walkable: a profile with a kink at the rim presents a lip the
/// controller has to step over to get onto a hill that is otherwise gentle.
///
/// ```text
///  y
///  |    ___
///  |  /     \        y = DOME_HEIGHT * (1 + cos(pi * t)) / 2
///  | /       ///  |/         \_
///  +-------------- t
///  0            1
/// ```
///
/// One function rather than one curve in the mesh and another in the level, because the
/// dome is the one asset whose *shape* is gameplay: a dummy stands on it, a ray is stopped
/// by it and a player walks up it. Two descriptions of it would be two hills.
#[must_use]
pub fn dome_profile(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    DOME_HEIGHT * 0.5 * (1.0 + (std::f32::consts::PI * t).cos())
}

/// The slope of [`dome_profile`] at `t`: `dy/dt` on a unit dome.
///
/// Negative, because the surface descends as it goes out. Steepest at the half-way point,
/// where it is `DOME_HEIGHT * pi / 2`.
#[must_use]
pub fn dome_slope(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    -DOME_HEIGHT * 0.5 * std::f32::consts::PI * (std::f32::consts::PI * t).sin()
}

/// The steepest angle anywhere on a dome, in degrees.
#[must_use]
pub fn dome_max_slope_degrees() -> f32 {
    (DOME_HEIGHT * std::f32::consts::FRAC_PI_2)
        .atan()
        .to_degrees()
}

/// A knee-high boulder. The cheapest scatter piece, and the most numerous.
pub const ROCK_SMALL: EnvironmentAsset = EnvironmentAsset::new("rock_small");
/// A waist-high boulder with a second lobe.
pub const ROCK_MEDIUM: EnvironmentAsset = EnvironmentAsset::new("rock_medium");
/// A leaning spire of rock, twice a character's height.
pub const ROCK_SPIRE: EnvironmentAsset = EnvironmentAsset::new("rock_spire");

/// A low round bush.
pub const SHRUB: EnvironmentAsset = EnvironmentAsset::new("shrub");
/// A fan of blades. Ground cover.
pub const FERN: EnvironmentAsset = EnvironmentAsset::new("fern");
/// A conifer. The treeline outside the arena is made of these.
pub const PINE: EnvironmentAsset = EnvironmentAsset::new("pine");
/// A screen of broad leaves on crossed cards: the one asset whose silhouette lives in its
/// texture's alpha rather than in its geometry.
///
/// Every other plant here is solid, so every other plant casts the shadow of the box it is
/// made of. This one exists because an alpha-tested shadow caster is a different code path
/// end to end — the depth pass has to run a pixel shader and `discard`, instead of running
/// no pixel shader at all — and a path with no content that uses it is a path that quietly
/// rots. Its shadow is a scatter of leaf shapes or it is broken.
pub const LEAF_SCREEN: EnvironmentAsset = EnvironmentAsset::new("leaf_screen");

/// A lit brazier. The arena's warm accent and its only emissive prop.
pub const BRAZIER: EnvironmentAsset = EnvironmentAsset::new("brazier");
/// A banded wooden crate.
pub const SUPPLY_CRATE: EnvironmentAsset = EnvironmentAsset::new("supply_crate");
/// A hanging banner on a pole.
pub const BANNER: EnvironmentAsset = EnvironmentAsset::new("banner");

/// A distant mountain silhouette. Placed a few hundred metres out.
pub const RIDGE: EnvironmentAsset = EnvironmentAsset::new("ridge");
/// The inverted gradient shell the whole scene sits inside.
pub const SKY_DOME: EnvironmentAsset = EnvironmentAsset::new("sky_dome");

/// A full-height crypt wall: eight metres long and five tall, the graveyard's room boundary.
///
/// The same length as [`WALL_SEGMENT`], so a room laid out in kerb segments can be walled in
/// place, and tall enough that a room reads as a room from the gameplay camera — the kerb is
/// a metre high and a level built from it reads as a field with fences.
pub const CRYPT_WALL: EnvironmentAsset = EnvironmentAsset::new("crypt_wall");
/// A square stone pier with a pyramid cap. Stands at every corner and joint of a crypt wall.
pub const WALL_PIER: EnvironmentAsset = EnvironmentAsset::new("wall_pier");
/// A pointed arch that springs from two piers eight metres apart: a doorway's head.
///
/// Carries no collision of its own. The piers it stands on are what block, and the opening
/// under it must stay open.
pub const CRYPT_GATE: EnvironmentAsset = EnvironmentAsset::new("crypt_gate");
/// Metres between the centres of the two piers a [`CRYPT_GATE`] stands on.
///
/// Two wall segments: the width of every doorway in a room laid out in segments. The arch's
/// feet are built into piers this far apart, so a gate placed anywhere else is an arch
/// standing on air — the level's tests hold every gate in the crypt to it.
pub const CRYPT_GATE_SPAN: f32 = 16.0;
/// A weathered headstone with a rounded shoulder.
pub const GRAVESTONE: EnvironmentAsset = EnvironmentAsset::new("gravestone");
/// A stone grave cross on a stepped base.
pub const GRAVE_CROSS: EnvironmentAsset = EnvironmentAsset::new("grave_cross");
/// A lidded stone coffin on a plinth.
pub const SARCOPHAGUS: EnvironmentAsset = EnvironmentAsset::new("sarcophagus");
/// A bare, crooked tree: the graveyard's vegetation.
pub const DEAD_TREE: EnvironmentAsset = EnvironmentAsset::new("dead_tree");
/// Four metres of wrought-iron railing between two posts.
pub const IRON_FENCE: EnvironmentAsset = EnvironmentAsset::new("iron_fence");
/// An eight-metre square of worn paving laid over the ground: a crypt's floor.
pub const CRYPT_FLOOR: EnvironmentAsset = EnvironmentAsset::new("crypt_floor");
/// A small stone tomb-house with a pediment and a sealed door. A graveyard's landmark.
pub const MAUSOLEUM: EnvironmentAsset = EnvironmentAsset::new("mausoleum");
/// A heap of bones and skulls. Ossuary dressing, walked through.
pub const BONE_PILE: EnvironmentAsset = EnvironmentAsset::new("bone_pile");

/// Every asset the environment library contains, in category order.
///
/// The list an uploader iterates: one `create_model` per entry, and every placement of that
/// asset then shares the mesh and material handles it returned. That sharing is the whole
/// of the instancing requirement — see the crate docs of
/// `engine_presentation::environment`.
pub const LIBRARY: [EnvironmentAsset; 29] = [
    GROUND_PLAIN,
    ARENA_FLOOR,
    WALL_SEGMENT,
    PILLAR,
    ARCH,
    DOME,
    ROCK_SMALL,
    ROCK_MEDIUM,
    ROCK_SPIRE,
    SHRUB,
    FERN,
    PINE,
    LEAF_SCREEN,
    BRAZIER,
    SUPPLY_CRATE,
    BANNER,
    RIDGE,
    SKY_DOME,
    CRYPT_WALL,
    WALL_PIER,
    CRYPT_GATE,
    GRAVESTONE,
    GRAVE_CROSS,
    SARCOPHAGUS,
    DEAD_TREE,
    IRON_FENCE,
    CRYPT_FLOOR,
    MAUSOLEUM,
    BONE_PILE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interned_name_is_the_same_id_every_time_it_is_asked_for() {
        let first = EnvironmentAsset::interned("generated/environment/rocks/rock_small");
        let second = EnvironmentAsset::interned("generated/environment/rocks/rock_small");
        assert_eq!(first, second);
        assert_eq!(first.name(), "generated/environment/rocks/rock_small");
        assert_eq!(
            first.name().as_ptr(),
            second.name().as_ptr(),
            "the name was leaked twice"
        );
    }

    #[test]
    fn interning_a_name_the_library_already_has_returns_the_librarys_own_id() {
        assert_eq!(EnvironmentAsset::interned("pillar"), PILLAR);
        assert_eq!(EnvironmentAsset::interned("sky_dome"), SKY_DOME);
    }

    #[test]
    fn two_different_names_are_two_different_ids() {
        let rock = EnvironmentAsset::interned("generated/environment/rocks/boulder_large");
        let cliff = EnvironmentAsset::interned("generated/environment/cliffs/cliff_overhang");
        assert_ne!(rock, cliff);
        // Interning does not grow the table for a name it has already seen.
        let before = EnvironmentAsset::interned_count();
        let _ = EnvironmentAsset::interned("generated/environment/rocks/boulder_large");
        assert_eq!(EnvironmentAsset::interned_count(), before);
    }

    #[test]
    fn every_asset_in_the_library_is_named_once() {
        let mut names: Vec<&str> = LIBRARY.iter().map(|asset| asset.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "an asset is listed twice");
        assert!(names.iter().all(|name| !name.is_empty()));
    }

    #[test]
    fn the_library_covers_every_category_the_phase_asks_for() {
        // The brief's minimum: 2-3 rocks, 2-3 vegetation, 2-3 architecture, 2-3 props.
        let has = |asset: EnvironmentAsset| LIBRARY.contains(&asset);
        for asset in [ROCK_SMALL, ROCK_MEDIUM, ROCK_SPIRE] {
            assert!(has(asset));
        }
        for asset in [SHRUB, FERN, PINE, LEAF_SCREEN] {
            assert!(has(asset));
        }
        for asset in [WALL_SEGMENT, PILLAR, ARCH, DOME] {
            assert!(has(asset));
        }
        for asset in [BRAZIER, SUPPLY_CRATE, BANNER] {
            assert!(has(asset));
        }
        for asset in [GROUND_PLAIN, ARENA_FLOOR, RIDGE, SKY_DOME] {
            assert!(has(asset));
        }
    }

    #[test]
    fn a_dome_is_flat_at_both_ends_and_never_steeper_than_a_character_can_walk() {
        assert!((dome_profile(0.0) - DOME_HEIGHT).abs() < 1e-6);
        assert!(dome_profile(1.0).abs() < 1e-6);
        // Outside the footprint there is no dome, and asking for it gets the ground.
        assert_eq!(dome_profile(1.4), 0.0);

        // Flat at the crest and flat at the rim: both are places a character walks onto the
        // dome from, and a kink at either is a lip to step over.
        assert!(dome_slope(0.0).abs() < 1e-6);
        assert!(dome_slope(1.0).abs() < 1e-4);

        // And monotone in between, or the hill has a dip in it.
        let mut previous = dome_profile(0.0);
        for step in 1..=100 {
            let height = dome_profile(step as f32 / 100.0);
            assert!(height <= previous + 1e-6, "the profile rises at t = {step}");
            previous = height;
        }

        // The controller's limit is 50 degrees. The margin is what the collision world's
        // approximation of the curve is allowed to cost.
        let steepest = dome_max_slope_degrees();
        assert!(
            (30.0..40.0).contains(&steepest),
            "the steepest part of a dome is {steepest} degrees"
        );
    }

    #[test]
    fn ids_compare_by_name_and_print_as_their_name() {
        assert_eq!(EnvironmentAsset::new("pillar"), PILLAR);
        assert_ne!(PILLAR, ARCH);
        assert_eq!(PILLAR.to_string(), "pillar");
    }
}
