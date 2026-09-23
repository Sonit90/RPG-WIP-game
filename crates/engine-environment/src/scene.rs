//! A whole environment: its objects, its spawn points, its light and its bounds.
//!
//! ```text
//! EnvironmentScene
//!  ├ name            "combat_arena"
//!  ├ objects[]       every placement, in a deterministic order
//!  ├ spawns          player, featured dummy, dummy group centre and spacing
//!  ├ lighting        sun, ambient, sky colours
//!  ├ half_extent     how far from the origin the playable square reaches
//!  ├ ground_height   what y the floor is at
//!  └ water[]         the bodies of water, if the level has any
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
    ///
    /// The flat floor under everything, and what a level with no terrain stands on. A level
    /// *with* terrain keeps it: the terrain covers part of the playable square and this is
    /// what holds up whatever is standing on the rest of it.
    pub ground_height: f32,
    /// The compound collision shapes placements refer to by index.
    ///
    /// Empty in a level whose objects each block one box or nothing, which is most of them.
    /// A cliff overhang or a cave piece names an entry here — see
    /// [`EnvironmentCollision::Compound`](crate::EnvironmentCollision::Compound) for why the
    /// boxes are here rather than in the object.
    pub collision_shapes: Vec<crate::object::CollisionShape>,
    /// The shape of the ground, if this level has been given one.
    ///
    /// `None` is a flat level, which is every level built before terrain existed and every
    /// new one until somebody raises a hill. The broad shape only — hills, valleys, slopes,
    /// plateaus. Anything that overhangs is a placed object, and the terrain does not know
    /// those exist; see [`crate::terrain`].
    pub terrain: Option<crate::terrain::Terrain>,
    /// The bodies of water this level has.
    ///
    /// Empty in every level nobody has poured water into, which is most of them. Like the
    /// terrain and unlike a placement: a body has no asset and no mesh of its own until
    /// something builds one for it, and the question it answers is
    /// [`EnvironmentScene::is_submerged`]. Water blocks nothing — see [`crate::water`].
    pub water: Vec<crate::water::WaterBody>,
    /// Where a player can start, and which way they face when they do.
    ///
    /// Empty in every level authored before spawn points existed and in every built-in
    /// layout, which is why [`SpawnPoints::player`] is still here: see
    /// [`EnvironmentScene::player_spawn`], which is the one question with the one answer.
    /// A *list* because a spawn point is a thing an author places rather than a field of the
    /// level — see [`crate::spawn`].
    pub spawn_points: Vec<crate::spawn::SpawnPoint>,
}

impl EnvironmentScene {
    /// A level with nothing in it.
    ///
    /// What the editor opens when a scene file does not exist yet. It is a real scene rather
    /// than an absence of one: it has a floor height, a boundary and a sun, so a camera has
    /// somewhere to stand and the first object placed in it lands on the same ground every
    /// other object will. Nothing is placed, because a "new level" that quietly contained a
    /// ground plane would be a level whose first object an author cannot delete.
    #[must_use]
    pub fn empty(name: impl Into<Cow<'static, str>>) -> Self {
        Self::with_capacity(name, 0)
    }

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
            collision_shapes: Vec::new(),
            terrain: None,
            water: Vec::new(),
            spawn_points: Vec::new(),
        }
    }

    /// The height of the ground under a world position.
    ///
    /// The terrain's surface where there is terrain, and [`EnvironmentScene::ground_height`]
    /// everywhere else. One question with one answer, so nothing outside this crate has to
    /// know whether the level has a heightmap — which is what lets a spawn point, a placement
    /// and a debug marker all sit on the ground without three different rules.
    #[must_use]
    pub fn ground_at(&self, position: Vec3) -> f32 {
        match &self.terrain {
            Some(terrain) if terrain.contains(position) => terrain.height_at(position),
            _ => self.ground_height,
        }
    }

    /// Where the player starts, and which way they look.
    ///
    /// The level's first spawn point if it has one, and [`SpawnPoints::player`] if it does
    /// not — so a level authored before spawn points existed still starts a player in the
    /// place it always did, and nothing outside this crate has to know which of the two it
    /// got. The height is lifted onto the terrain, for the reason
    /// [`EnvironmentScene::ground_at`] exists: a spawn authored at `y = 0` on a level whose
    /// ground was raised afterwards would otherwise start the player inside a hill.
    ///
    /// The *first* rather than a random one, because a level has to start the same way twice.
    /// When something needs more than one — a team, a checkpoint, a dungeon's entrance — it
    /// reads [`EnvironmentScene::spawn_points`] directly and decides for itself.
    #[must_use]
    pub fn player_spawn(&self) -> crate::spawn::SpawnPoint {
        let authored = self
            .spawn_points
            .iter()
            .copied()
            .find(crate::spawn::SpawnPoint::is_valid);
        let spawn = authored.unwrap_or_else(|| crate::spawn::SpawnPoint::new(self.spawns.player));
        crate::spawn::SpawnPoint {
            position: Vec3::new(
                spawn.position.x,
                spawn.position.y.max(self.ground_at(spawn.position)),
                spawn.position.z,
            ),
            yaw: spawn.yaw,
        }
    }

    /// Adds a spawn point.
    pub fn push_spawn(&mut self, spawn: crate::spawn::SpawnPoint) {
        self.spawn_points.push(spawn);
    }

    /// Which body of water a point is inside, if any.
    ///
    /// The first one that contains it, in the order the level holds them. Two bodies that
    /// overlap are an authoring accident rather than a case with a right answer, and picking
    /// the first is at least the same answer every time.
    #[must_use]
    pub fn water_at(&self, point: Vec3) -> Option<usize> {
        self.water.iter().position(|body| body.contains(point))
    }

    /// Whether a point is under water.
    ///
    /// The one question with one answer, in the same spirit as
    /// [`EnvironmentScene::ground_at`]: a caller must not have to know how many bodies a
    /// level has or how each of them is shaped.
    #[must_use]
    pub fn is_submerged(&self, point: Vec3) -> bool {
        self.water.iter().any(|body| body.contains(point))
    }

    /// Adds a body of water.
    pub fn push_water(&mut self, body: crate::water::WaterBody) {
        self.water.push(body);
    }

    /// Adds a compound collision shape and returns the collision that names it.
    ///
    /// Shapes are shared: placing forty of the same cliff piece adds one entry and forty
    /// references to it. A shape with no boxes in it blocks nothing and is answered with
    /// [`EnvironmentCollision::None`] rather than with an entry nothing can be done with.
    ///
    /// Beyond `u16::MAX` distinct shapes the level gets `None` and a log line. A level with
    /// sixty-five thousand *different* collision shapes is not a level anybody built by hand.
    pub fn intern_collision(
        &mut self,
        shape: crate::object::CollisionShape,
    ) -> crate::object::EnvironmentCollision {
        use crate::object::EnvironmentCollision;
        if shape.is_empty() {
            return EnvironmentCollision::None;
        }
        if let Some(index) = self
            .collision_shapes
            .iter()
            .position(|known| *known == shape)
        {
            return EnvironmentCollision::Compound(index as u16);
        }
        let Ok(index) = u16::try_from(self.collision_shapes.len()) else {
            log::error!(
                "scene '{}': more than {} distinct collision shapes; this one blocks nothing",
                self.name,
                u16::MAX
            );
            return EnvironmentCollision::None;
        };
        self.collision_shapes.push(shape);
        EnvironmentCollision::Compound(index)
    }

    /// Drops every compound shape no placement names, renumbering the ones that are left.
    ///
    /// Deleting a cave piece leaves its shape in the table; interning keeps the table from
    /// growing per placement but nothing ever shrinks it. What the editor writes goes through
    /// this, so a level file carries the shapes its placements use and no others. Returns how
    /// many were dropped.
    pub fn prune_collision_shapes(&mut self) -> usize {
        use crate::object::EnvironmentCollision;
        let mut used = vec![false; self.collision_shapes.len()];
        for object in &self.objects {
            if let EnvironmentCollision::Compound(index) = object.collision
                && let Some(slot) = used.get_mut(index as usize)
            {
                *slot = true;
            }
        }
        if used.iter().all(|used| *used) {
            return 0;
        }
        // Old index -> new index, for the shapes that survive.
        let mut remap = vec![u16::MAX; used.len()];
        let mut kept = Vec::with_capacity(used.len());
        for (index, shape) in std::mem::take(&mut self.collision_shapes)
            .into_iter()
            .enumerate()
        {
            if used[index] {
                remap[index] = kept.len() as u16;
                kept.push(shape);
            }
        }
        let dropped = used.len() - kept.len();
        self.collision_shapes = kept;
        for object in &mut self.objects {
            if let EnvironmentCollision::Compound(index) = &mut object.collision
                && let Some(new) = remap.get(*index as usize)
            {
                *index = *new;
            }
        }
        dropped
    }

    /// Every world-space box one placement blocks.
    ///
    /// The one place the three kinds of collision become the same thing: nothing, one box, or
    /// the boxes of a shape this scene holds. Whatever builds a collision world walks this and
    /// never asks which kind it was — which is what let compound shapes arrive without a
    /// second code path in gameplay.
    #[must_use]
    pub fn world_boxes(&self, object: &EnvironmentObject) -> Vec<crate::object::WorldBox> {
        use crate::object::EnvironmentCollision;
        match object.collision {
            EnvironmentCollision::None => Vec::new(),
            EnvironmentCollision::Box { .. } => object.world_collision().into_iter().collect(),
            EnvironmentCollision::Compound(index) => {
                let Some(shape) = self.collision_shapes.get(index as usize) else {
                    log::error!(
                        "scene '{}': {} names collision shape {index}, which this level does                          not have; it blocks nothing",
                        self.name,
                        object.asset
                    );
                    return Vec::new();
                };
                shape
                    .boxes
                    .iter()
                    .map(|shape| object.place_box(*shape))
                    .collect()
            }
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
    fn a_level_answers_for_every_body_of_water_in_it_at_once() {
        use crate::water::WaterBody;
        use engine_core::math::Vec2;

        let mut scene = scene();
        // A pond at ground level and a reservoir up a hill, so "under water" cannot be
        // answered by one height for the whole level.
        scene.push_water(WaterBody::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec2::new(5.0, 5.0),
            2.0,
        ));
        scene.push_water(WaterBody::new(
            Vec3::new(40.0, 12.0, 0.0),
            Vec2::new(3.0, 3.0),
            1.0,
        ));

        assert_eq!(scene.water_at(Vec3::new(0.0, -1.0, 0.0)), Some(0));
        assert_eq!(scene.water_at(Vec3::new(40.0, 11.5, 0.0)), Some(1));
        assert!(scene.is_submerged(Vec3::new(40.0, 11.5, 0.0)));

        // The height of the second body over the position of the first, and the reverse:
        // neither is in any water, and a query that only compared heights would say both are.
        assert_eq!(scene.water_at(Vec3::new(0.0, 11.5, 0.0)), None);
        assert_eq!(scene.water_at(Vec3::new(40.0, -1.0, 0.0)), None);
        assert!(!scene.is_submerged(Vec3::new(100.0, -1.0, 0.0)));
    }

    #[test]
    fn a_level_with_no_spawn_point_still_starts_the_player_where_it_always_did() {
        let mut scene = scene();
        scene.spawns.player = Vec3::new(5.0, 0.0, -7.0);
        let fallback = scene.player_spawn();
        assert_eq!(fallback.position, Vec3::new(5.0, 0.0, -7.0));
        assert_eq!(fallback.yaw, 0.0);

        // An authored one wins, and brings its facing with it.
        scene.push_spawn(crate::spawn::SpawnPoint::new(Vec3::new(-2.0, 0.0, 3.0)).with_yaw(1.5));
        let authored = scene.player_spawn();
        assert_eq!(authored.position, Vec3::new(-2.0, 0.0, 3.0));
        assert!((authored.yaw - 1.5).abs() < 1e-6);

        // The first of several, so a level starts the same way twice.
        scene.push_spawn(crate::spawn::SpawnPoint::new(Vec3::new(40.0, 0.0, 40.0)));
        assert_eq!(scene.player_spawn().position, Vec3::new(-2.0, 0.0, 3.0));
    }

    #[test]
    fn a_spawn_point_is_lifted_onto_the_ground_under_it() {
        // A spawn authored at zero on a level whose ground was raised afterwards would start
        // the player inside the hill. The same rule `ground_at` exists for.
        let mut scene = scene();
        let mut terrain = crate::terrain::Terrain::flat(17, 2.0, 0.0);
        terrain.apply_brush(Vec3::ZERO, 10.0, 4.0, crate::BrushMode::Raise);
        scene.terrain = Some(terrain);
        scene.push_spawn(crate::spawn::SpawnPoint::new(Vec3::ZERO));

        let spawn = scene.player_spawn();
        assert!(spawn.position.y > 0.5, "{:?}", spawn.position);
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
