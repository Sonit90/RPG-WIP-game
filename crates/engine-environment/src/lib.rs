//! The level as data: what an arena is made of, where each piece stands, and what of it
//! blocks a character.
//!
//! ```text
//!                        EnvironmentScene
//!                               │
//!                 ┌─────────────┼─────────────┐
//!                 ▼             ▼             ▼
//!           engine-game   engine-assets  engine-presentation
//!           builds the    builds the     spawns the
//!           collision     ModelAssets    render entities
//!           world
//! ```
//!
//! # Why this is its own crate
//!
//! Three crates have to read the same placement list, and they are deliberately forbidden
//! from knowing about each other. `engine-game` may not name a mesh — it does not depend on
//! `engine-assets` and could not compile a reference to one. `engine-assets` may not name a
//! collision world. `engine-presentation` may name both, but it is the crate that decides
//! *how* things are drawn, not *what is in the level*; putting the layout there would put
//! content in the same file as the mechanism that draws it.
//!
//! So the description sits below all three, in a crate whose only dependency is
//! `engine-core`. That absence is the point of the crate and it is checkable:
//!
//! ```text
//! cargo tree -p engine-environment
//! engine-environment
//! └── engine-core
//!     └── log
//! ```
//!
//! # What is here and what is not
//!
//! Here: an [`EnvironmentAsset`] id, a [`Transform`](engine_core::transform::Transform), an
//! [`EnvironmentCollision`] primitive, an [`EnvironmentLighting`] preset, and the
//! [`EnvironmentScene`] that collects them.
//!
//! Not here: meshes, materials, textures, handles, `CollisionWorld`, `RenderWorld`,
//! entities, health. An [`EnvironmentObject`] says *"a medium rock, here, rotated this far,
//! and it blocks a box this big"*. What a medium rock looks like is
//! `engine_assets::procedural::environment`'s answer; what blocking means is
//! `engine_collision`'s.
//!
//! # Cost model
//!
//! Everything here is built once, when a scene is loaded, and read once per placement.
//! Nothing in this crate runs per frame. A scene is a `Vec` of 40-byte records; the shipping
//! arena is 141 of them.

#![deny(missing_docs)]

pub mod arena;
pub mod asset;
pub mod dungeon;
pub mod lighting;
pub mod object;
pub mod scene;

pub use asset::{EnvironmentAsset, LIBRARY};
pub use lighting::EnvironmentLighting;
pub use object::{EnvironmentCollision, EnvironmentObject};
pub use scene::{EnvironmentScene, SpawnPoints};

/// Deterministic value in `0..1` from a pair of integers.
///
/// The scatter in [`arena`] is placed with this rather than with a random number generator,
/// so the arena is the same arena on every run, in every process, on every machine — which
/// is what makes a screenshot comparable to the last one and a benchmark comparable to the
/// run before it. The same hash the training dummy's textures use.
#[must_use]
pub fn hash01(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x27D4_EB2D) ^ y.wrapping_mul(0x1656_67B1);
    value ^= value >> 15;
    value = value.wrapping_mul(0x2545_F491);
    value ^= value >> 13;
    (value & 0xFFFF) as f32 / 65535.0
}

/// Deterministic value in `low..high`.
#[must_use]
pub fn hash_range(x: u32, y: u32, low: f32, high: f32) -> f32 {
    low + (high - low) * hash01(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_the_same_every_time_and_spread_across_its_range() {
        assert_eq!(hash01(7, 11), hash01(7, 11));
        assert_ne!(hash01(7, 11), hash01(11, 7));

        let mut low = f32::MAX;
        let mut high = f32::MIN;
        let mut sum = 0.0;
        for index in 0..1_000 {
            let value = hash01(index, index * 3 + 1);
            assert!((0.0..=1.0).contains(&value), "{value}");
            low = low.min(value);
            high = high.max(value);
            sum += value;
        }
        assert!(low < 0.05 && high > 0.95, "{low} {high}");
        let mean = sum / 1_000.0;
        assert!((mean - 0.5).abs() < 0.05, "{mean}");
    }

    #[test]
    fn a_range_stays_inside_itself() {
        for index in 0..200 {
            let value = hash_range(index, 5, -3.0, 9.0);
            assert!((-3.0..=9.0).contains(&value), "{value}");
        }
    }
}
