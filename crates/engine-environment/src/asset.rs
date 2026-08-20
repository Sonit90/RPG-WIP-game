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
/// A smooth stone dome: a hemisphere the size of a small hill.
///
/// The one piece of architecture that is round in every direction, which is why the
/// line-of-sight course is built from it: a wall answers "is a wall a wall", and a dome
/// answers the harder question of what happens to a target standing part of the way up
/// something that curves away from the caster.
pub const DOME: EnvironmentAsset = EnvironmentAsset::new("dome");

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

/// Every asset the environment library contains, in category order.
///
/// The list an uploader iterates: one `create_model` per entry, and every placement of that
/// asset then shares the mesh and material handles it returned. That sharing is the whole
/// of the instancing requirement — see the crate docs of
/// `engine_presentation::environment`.
pub const LIBRARY: [EnvironmentAsset; 17] = [
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
    BRAZIER,
    SUPPLY_CRATE,
    BANNER,
    RIDGE,
    SKY_DOME,
];

#[cfg(test)]
mod tests {
    use super::*;

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
        for asset in [SHRUB, FERN, PINE] {
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
    fn ids_compare_by_name_and_print_as_their_name() {
        assert_eq!(EnvironmentAsset::new("pillar"), PILLAR);
        assert_ne!(PILLAR, ARCH);
        assert_eq!(PILLAR.to_string(), "pillar");
    }
}
