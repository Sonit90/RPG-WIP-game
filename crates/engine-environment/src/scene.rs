//! A whole environment: its objects, its spawn points, its light and its bounds.
//!
//! ```text
//! EnvironmentScene
//!  ├ name            "combat_arena"
//!  ├ objects[]       every placement, in a deterministic order
//!  ├ spawns          player, featured dummy, dummy group centre and spacing
//!  ├ lighting        sun, ambient, sky colours
//!  ├ half_extent     how far from the origin the playable square reaches
//!  └ ground_height   what y the floor is at
//! ```
//!
//! # Determinism
//!
//! The object list is built in one pass with no iteration order that depends on a hash map,
//! no random number generator and no clock. Two calls to the same layout function produce
//! byte-identical scenes, and [`EnvironmentScene::signature`] exists so a test can assert
//! that in one line rather than by comparing 141 transforms by hand.
//!
//! That is not a nicety: a benchmark comparing two runs has to know the scene did not move
//! between them, and a screenshot compared against last week's has to be of the same arena.

use std::borrow::Cow;

use engine_core::math::Vec3;

use crate::asset::EnvironmentAsset;
use crate::lighting::{EnvironmentLighting, SceneLight};
use crate::object::EnvironmentObject;

/// Where the scene says the fight happens.
///
/// Gameplay owns spawning; the environment owns *where there is room to spawn*. A layout
/// that puts a boulder where the group stands is a layout bug, and it is caught by having
/// the two facts in one place — see the tests in [`crate::arena`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnPoints {
    /// Where the player starts.
    pub player: Vec3,
    /// Where the single-target dummy stands.
    pub featured_dummy: Vec3,
    /// Centre of the dummy group.
    pub group_center: Vec3,
    /// Metres between dummies in the group.
    pub group_spacing: f32,
}

impl Default for SpawnPoints {
    fn default() -> Self {
        Self {
            player: Vec3::ZERO,
            featured_dummy: Vec3::new(0.0, 0.0, -12.0),
            group_center: Vec3::new(14.0, 0.0, -18.0),
            group_spacing: 3.0,
        }
    }
}

/// One complete environment.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentScene {
    /// What it is: `"combat_arena"`, `"environment_demo"`, `"stress"`.
    ///
    /// Borrowed for a layout function, which names itself with a literal, and owned for a
    /// scene that came out of a file, which learns its name at run time. A `Cow` rather than
    /// a `String` so the built-in layouts still cost no allocation.
    pub name: Cow<'static, str>,
    /// Every placement.
    pub objects: Vec<EnvironmentObject>,
    /// Where the fight happens.
    pub spawns: SpawnPoints,
    /// The sun, the fill and the sky.
    pub lighting: EnvironmentLighting,
    /// Lights that belong to places in the scene: braziers, torches, shafts of light.
    ///
    /// Empty for an outdoor scene lit by the sun alone, which is most of them. A light here
    /// is *placed* rather than *rendered*: which of them get shadow maps is the renderer's
    /// decision and depends on where the camera is.
    pub lights: Vec<SceneLight>,
    /// Half-width of the square a character is confined to, in metres.
    pub half_extent: f32,
    /// The y the floor is at.
    pub ground_height: f32,
}

impl EnvironmentScene {
    /// An empty scene with room for `capacity` objects.
    #[must_use]
    pub fn with_capacity(name: impl Into<Cow<'static, str>>, capacity: usize) -> Self {
        Self {
            name: name.into(),
            objects: Vec::with_capacity(capacity),
            spawns: SpawnPoints::default(),
            lighting: EnvironmentLighting::arena_afternoon(),
            lights: Vec::new(),
            half_extent: 40.0,
            ground_height: 0.0,
        }
    }

    /// Adds a placement.
    pub fn push(&mut self, object: EnvironmentObject) {
        self.objects.push(object);
    }

    /// Adds a local light.
    pub fn push_light(&mut self, light: SceneLight) {
        self.lights.push(light);
    }

    /// How many objects the scene holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the scene holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// How many placements name `asset`.
    #[must_use]
    pub fn count_of(&self, asset: EnvironmentAsset) -> usize {
        self.objects
            .iter()
            .filter(|object| object.asset == asset)
            .count()
    }

    /// Every distinct asset the scene uses, in first-placement order.
    ///
    /// What the uploader iterates: one `create_model` per entry, so a scene with 141
    /// placements of 16 assets costs 16 uploads. A scene that named an asset the library
    /// does not contain fails here, loudly, at load — not as a missing object at run time.
    #[must_use]
    pub fn distinct_assets(&self) -> Vec<EnvironmentAsset> {
        let mut seen: Vec<EnvironmentAsset> = Vec::with_capacity(16);
        for object in &self.objects {
            if !seen.contains(&object.asset) {
                seen.push(object.asset);
            }
        }
        seen
    }

    /// How many objects block a character.
    #[must_use]
    pub fn blocking_count(&self) -> usize {
        self.objects
            .iter()
            .filter(|object| object.collision.blocks())
            .count()
    }

    /// A number that changes if any placement does.
    ///
    /// A cheap order-sensitive hash over every asset name and every transform component. It
    /// exists so "the scene is reproducible" is a one-line assertion rather than a
    /// field-by-field comparison, and so a benchmark can record *which* scene it measured.
    ///
    /// Not a cryptographic hash and not stable across a change to this function — it is
    /// compared against itself within a run, never persisted.
    #[must_use]
    pub fn signature(&self) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |value: u64| {
            hash ^= value;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        };
        for object in &self.objects {
            for byte in object.asset.name().as_bytes() {
                feed(u64::from(*byte));
            }
            let transform = &object.transform;
            for component in [
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
                transform.rotation.x,
                transform.rotation.y,
                transform.rotation.z,
                transform.rotation.w,
                transform.scale.x,
                transform.scale.y,
                transform.scale.z,
            ] {
                feed(u64::from(component.to_bits()));
            }
            feed(u64::from(object.collision.blocks()));
        }
        hash
    }

    /// The horizontal distance from the origin to the furthest object.
    ///
    /// What a camera's far plane and a sky dome's radius are checked against.
    #[must_use]
    pub fn horizontal_reach(&self) -> f32 {
        self.objects
            .iter()
            .map(|object| {
                let position = object.position();
                position.x.hypot(position.z)
            })
            .fold(0.0_f32, f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{PILLAR, ROCK_SMALL};
    use crate::object::EnvironmentCollision;

    fn scene() -> EnvironmentScene {
        let mut scene = EnvironmentScene::with_capacity("test", 4);
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::new(10.0, 0.0, 0.0))
                .with_collision(EnvironmentCollision::standing(0.5, 4.0, 0.5)),
        );
        scene.push(EnvironmentObject::decoration(
            ROCK_SMALL,
            Vec3::new(-4.0, 0.0, 6.0),
        ));
        scene.push(EnvironmentObject::decoration(
            ROCK_SMALL,
            Vec3::new(2.0, 0.0, -30.0),
        ));
        scene
    }

    #[test]
    fn a_scene_reports_what_is_in_it() {
        let scene = scene();
        assert_eq!(scene.len(), 3);
        assert!(!scene.is_empty());
        assert_eq!(scene.count_of(ROCK_SMALL), 2);
        assert_eq!(scene.count_of(PILLAR), 1);
        assert_eq!(scene.blocking_count(), 1);
        assert_eq!(scene.distinct_assets(), vec![PILLAR, ROCK_SMALL]);
        assert!((scene.horizontal_reach() - 30.066).abs() < 0.01);
    }

    #[test]
    fn the_signature_changes_when_a_placement_does_and_not_otherwise() {
        let first = scene();
        let second = scene();
        assert_eq!(first.signature(), second.signature());

        let mut moved = scene();
        moved.objects[1].transform.translation.x += 0.001;
        assert_ne!(first.signature(), moved.signature());

        let mut restyled = scene();
        restyled.objects[1].collision = EnvironmentCollision::standing(1.0, 1.0, 1.0);
        assert_ne!(first.signature(), restyled.signature());

        // Order matters: two scenes holding the same objects in a different order are two
        // different scenes, because the render entities are spawned in this order.
        let mut swapped = scene();
        swapped.objects.swap(0, 1);
        assert_ne!(first.signature(), swapped.signature());
    }

    #[test]
    fn an_empty_scene_reaches_nowhere_and_hashes_to_the_seed() {
        let empty = EnvironmentScene::with_capacity("empty", 0);
        assert!(empty.is_empty());
        assert_eq!(empty.horizontal_reach(), 0.0);
        assert_eq!(empty.distinct_assets(), Vec::new());
        assert_eq!(empty.signature(), 0xcbf2_9ce4_8422_2325);
    }
}
