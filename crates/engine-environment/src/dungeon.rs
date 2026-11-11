//! The Ashen Reliquary: the first dungeon's layout, as data.
//!
//! ```text
//!    +Z   ENTRANCE (arch in a closed wall)
//!          ┌───────────────┐  z −2
//!          │ 0 ENTRANCE    │   no pack
//!          └──────┬────────┘  z −26
//!                 │ corridor
//!          ┌──────┴────────┐  z −42
//!          │ 1 GATEHOUSE   │   packs 1, 2
//!          └──────┬────────┘  z −74
//!                 │ corridor
//!          ┌──────┴────────┐  z −90
//!          │ 2 OSSUARY     │   packs 3, 4
//!          └──────┬────────┘  z −122
//!                 │ corridor
//!          ┌──────┴────────┐  z −138
//!          │ 3 CINDER      │   packs 5, 6
//!          └──────┬────────┘  z −170
//!                 │ corridor
//!          ┌──────┴────────┐  z −186
//!          │ 4 VAULT       │   packs 7, 8
//!          └──────┬────────┘  z −218
//!                 │ corridor
//!     ┌───────────┴────────────┐  z −242
//!     │    5 ASHEN GATE        │   the boss
//!     └────────────────────────┘  z −290
//!    −Z
//! ```
//!
//! # Why this is a function and not a file
//!
//! The same answer [`crate::arena`] gives: a scene file needs a parser, a schema and an
//! error path for a malformed one, and none of that buys anything until scenes are authored
//! by somebody who is not editing the source anyway. What matters is that the composition is
//! **data** — a `Vec<EnvironmentObject>` three crates read without any of them knowing how it
//! was produced.
//!
//! # Collision is the geometry
//!
//! Every wall segment, every pillar and every doorway here carries its own collision box, and
//! [`crate::arena`]-style invisible walls are used for nothing except the outer bound the
//! character controller needs. What the player sees is what stops them.
//!
//! # No new assets
//!
//! Wall segments, pillars, arches and braziers, all from the existing sixteen-asset library.
//! A dungeon phase that also needed an art phase would be two phases.

use engine_core::math::Vec3;
use engine_core::transform::Transform;

use crate::asset::{ARCH, BRAZIER, GROUND_PLAIN, PILLAR, SKY_DOME, WALL_SEGMENT};
use crate::lighting::{EnvironmentLighting, SceneLight};
use crate::object::{EnvironmentCollision, EnvironmentObject};
use crate::scene::{EnvironmentScene, SpawnPoints};

/// Length of one wall segment, in metres. The mesh is authored at this length.
const SEGMENT: f32 = 8.0;

/// Half the width of every doorway and every corridor, in metres.
///
/// One number for both, deliberately: a corridor narrower than the door it leaves through
/// leaves a gap at the junction the player can walk out of, and a corridor wider than the
/// door reads as a room. Sixteen metres of opening is two wall segments' worth, which is what
/// makes a doorway a whole number of segments rather than a scaled one.
const DOOR_HALF: f32 = 8.0;

/// How far the boundary the character controller is clamped to stands from the origin.
///
/// Far outside the dungeon, because the dungeon's own walls are what confine the player. It
/// exists only so a character that somehow left the geometry is still in a bounded world.
pub const DUNGEON_HALF_EXTENT: f32 = 320.0;

/// Where the player starts, in the entrance hall.
pub const ENTRANCE: Vec3 = Vec3::new(0.0, 0.0, -8.0);

/// One room of the dungeon, as the numbers every other part of the phase reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DungeonArea {
    /// What it is called, for the readout and the design document.
    pub name: &'static str,
    /// Its centre on the floor.
    pub center: Vec3,
    /// Half its width along X, in metres.
    pub half_x: f32,
    /// Half its depth along Z, in metres.
    pub half_z: f32,
}

impl DungeonArea {
    /// The z of its wall nearer the entrance.
    #[must_use]
    pub fn south_edge(&self) -> f32 {
        self.center.z + self.half_z
    }

    /// The z of its wall further from the entrance.
    #[must_use]
    pub fn north_edge(&self) -> f32 {
        self.center.z - self.half_z
    }

    /// Whether `point` is inside the room, ignoring height.
    #[must_use]
    pub fn contains(&self, point: Vec3) -> bool {
        (point.x - self.center.x).abs() <= self.half_x
            && (point.z - self.center.z).abs() <= self.half_z
    }

    /// A point `x` metres right of centre and `z` metres in front of the room's centre.
    #[must_use]
    pub fn at(&self, x: f32, z: f32) -> Vec3 {
        Vec3::new(self.center.x + x, 0.0, self.center.z + z)
    }
}

/// How many areas the dungeon has.
pub const AREA_COUNT: usize = 6;

/// A combat room's half-extent, in metres. Four wall segments each way.
///
/// Sized from the content rather than from taste: two packs stand at opposite corners of a
/// room, and the roster's widest aggro radius is 20 m. A 32 m room puts them 28 m apart,
/// which is the smallest room in which two packs are two packs.
const ROOM_HALF: f32 = 16.0;

/// The dungeon's rooms, entrance first.
///
/// Every extent is a multiple of the wall segment's own length, so a room is a whole number
/// of segments and a doorway is exactly two of them left out. A room with a fractional span
/// would need scaled segments, and a scaled segment's collision box and its mesh are two
/// numbers that drift.
pub static AREAS: [DungeonArea; AREA_COUNT] = [
    // No pack stands here. It is where the player arrives, and it exists so that arriving is
    // not the same event as being noticed.
    DungeonArea {
        name: "ENTRANCE HALL",
        center: Vec3::new(0.0, 0.0, -14.0),
        half_x: 12.0,
        half_z: 12.0,
    },
    DungeonArea {
        name: "GATEHOUSE",
        center: Vec3::new(0.0, 0.0, -58.0),
        half_x: ROOM_HALF,
        half_z: ROOM_HALF,
    },
    DungeonArea {
        name: "OSSUARY HALL",
        center: Vec3::new(0.0, 0.0, -106.0),
        half_x: ROOM_HALF,
        half_z: ROOM_HALF,
    },
    DungeonArea {
        name: "CINDER GALLERY",
        center: Vec3::new(0.0, 0.0, -154.0),
        half_x: ROOM_HALF,
        half_z: ROOM_HALF,
    },
    DungeonArea {
        name: "RELIQUARY VAULT",
        center: Vec3::new(0.0, 0.0, -202.0),
        half_x: ROOM_HALF,
        half_z: ROOM_HALF,
    },
    // The boss arena. Larger than every other room because one of its mechanics reaches 14 m
    // and the answer to it is running further than that.
    DungeonArea {
        name: "ASHEN GATE",
        center: Vec3::new(0.0, 0.0, -266.0),
        half_x: 24.0,
        half_z: 24.0,
    },
];

/// The shipping dungeon.
///
/// Deterministic: two calls produce byte-identical scenes, which
/// [`EnvironmentScene::signature`] makes checkable in one line.
#[must_use]
pub fn ashen_reliquary() -> EnvironmentScene {
    let mut scene = EnvironmentScene::with_capacity("ashen_reliquary", 256);
    scene.half_extent = DUNGEON_HALF_EXTENT;
    // Underground, and lit by what is burning in it: the arena's afternoon sun would make a
    // reliquary read as a courtyard.
    scene.lighting = EnvironmentLighting::arena_afternoon();
    scene.spawns = SpawnPoints {
        player: ENTRANCE,
        // Nothing in the dungeon spawns dummies. The points still have to be somewhere legal,
        // because `ArenaConfig::from_environment` reads them, so they are put where the first
        // pack stands — the one place in the dungeon known to have room for a body.
        featured_dummy: AREAS[0].at(0.0, -8.0),
        group_center: AREAS[0].at(0.0, -8.0),
        group_spacing: 3.0,
    };

    push_backdrop(&mut scene);
    for (index, area) in AREAS.iter().enumerate() {
        // The first room's south wall is the dungeon's entrance and is closed; every other
        // opening joins two rooms.
        push_room(&mut scene, area, index > 0, index + 1 < AREA_COUNT);
        if let Some(next) = AREAS.get(index + 1) {
            push_corridor(&mut scene, area.north_edge(), next.south_edge());
        }
    }
    push_boss_pillars(&mut scene);
    scene
}

/// The sky and the floor everything stands on.
fn push_backdrop(scene: &mut EnvironmentScene) {
    scene.push(EnvironmentObject::decoration(SKY_DOME, Vec3::ZERO));
    // One plain under the whole dungeon. Non-uniform scale, so its transform is written
    // directly: `with_scale` is uniform by design.
    scene.push(EnvironmentObject {
        asset: GROUND_PLAIN,
        transform: Transform::from_translation(Vec3::new(0.0, 0.0, -145.0))
            .with_scale(Vec3::new(640.0, 1.0, 640.0)),
        collision: EnvironmentCollision::None,
    });
}

/// The collision box one wall segment carries.
fn wall_collision() -> EnvironmentCollision {
    EnvironmentCollision::standing(SEGMENT * 0.5, 2.4, 0.45)
}

/// Four walls, four corner pillars, and a brazier beside each opening.
///
/// `open_south` and `open_north` say which of the two ends has a doorway. The doorway is the
/// two centre segments left out, which is why every span here is a multiple of the segment
/// length: the opening is then exactly [`DOOR_HALF`] either side of centre and the corridor
/// that meets it lines up with no gap at the junction.
fn push_room(scene: &mut EnvironmentScene, area: &DungeonArea, open_south: bool, open_north: bool) {
    let collision = wall_collision();

    // The two walls that run along X, at each end of the room.
    for (edge, open) in [
        (area.south_edge(), open_south),
        (area.north_edge(), open_north),
    ] {
        for x in span_centers(area.center.x, area.half_x) {
            if open && (x - area.center.x).abs() < DOOR_HALF {
                continue;
            }
            scene.push(
                EnvironmentObject::decoration(WALL_SEGMENT, Vec3::new(x, 0.0, edge))
                    .with_collision(collision),
            );
        }
        // The arch stands in the opening, and in a closed end it is the dungeon's entrance.
        scene.push(EnvironmentObject::decoration(
            ARCH,
            Vec3::new(area.center.x, 0.0, edge),
        ));
    }

    // The two walls that run along Z. A quarter turn, so the segment's long axis follows the
    // wall — an unturned segment here would be a fence of posts.
    for side in [-1.0_f32, 1.0] {
        let x = area.center.x + side * area.half_x;
        for z in span_centers(area.center.z, area.half_z) {
            scene.push(
                EnvironmentObject::decoration(WALL_SEGMENT, Vec3::new(x, 0.0, z))
                    .with_collision(collision)
                    .with_yaw(std::f32::consts::FRAC_PI_2),
            );
        }
    }

    // Corner pillars: what makes a box of walls read as a room.
    let pillar = EnvironmentCollision::standing(0.62, 4.6, 0.62);
    for sx in [-1.0_f32, 1.0] {
        for sz in [-1.0_f32, 1.0] {
            scene.push(
                EnvironmentObject::decoration(
                    PILLAR,
                    Vec3::new(
                        area.center.x + sx * area.half_x,
                        0.0,
                        area.center.z + sz * area.half_z,
                    ),
                )
                .with_collision(pillar),
            );
        }
    }

    // A brazier either side of each opening. They are the only light source that moves with
    // the layout, so they are what tells the player which end of the room is forward.
    for (edge, open) in [
        (area.south_edge(), open_south),
        (area.north_edge(), open_north),
    ] {
        if !open {
            continue;
        }
        for side in [-1.0_f32, 1.0] {
            let position = Vec3::new(area.center.x + side * (DOOR_HALF + 1.5), 0.0, edge);
            scene.push(EnvironmentObject::decoration(BRAZIER, position));
            // Indoors, the braziers are not decoration: they are what the room is lit by.
            // Every one of them asks for a shadow map and the renderer hands out the few it
            // has by distance, so the pair the player is standing between casts and the pair
            // three rooms away does not — which is the whole point of §21's budget.
            scene.push_light(
                SceneLight::brazier(position + Vec3::new(0.0, 1.45, 0.0), 12.0).casting_shadows(2),
            );
        }
    }
}

/// The passage between two rooms: two walls, and nothing in between.
fn push_corridor(scene: &mut EnvironmentScene, from_z: f32, to_z: f32) {
    let collision = wall_collision();
    let center = (from_z + to_z) * 0.5;
    let half = (from_z - to_z).abs() * 0.5;
    for side in [-1.0_f32, 1.0] {
        let x = side * DOOR_HALF;
        for z in span_centers(center, half) {
            scene.push(
                EnvironmentObject::decoration(WALL_SEGMENT, Vec3::new(x, 0.0, z))
                    .with_collision(collision)
                    .with_yaw(std::f32::consts::FRAC_PI_2),
            );
        }
    }
}

/// The four pillars in the boss arena.
///
/// On a 13 m radius at the diagonals, which is *inside* Immolation Nova's fourteen. A player
/// who runs to a pillar is still in the blast; running past them is the answer. That is a
/// gameplay decision expressed as level geometry, and it is why the radius is written here
/// next to the reason rather than being a number in a list.
fn push_boss_pillars(scene: &mut EnvironmentScene) {
    let arena = AREAS[AREA_COUNT - 1];
    let collision = EnvironmentCollision::standing(0.62, 4.6, 0.62);
    let radius = 13.0_f32;
    let diagonal = radius * std::f32::consts::FRAC_1_SQRT_2;
    for sx in [-1.0_f32, 1.0] {
        for sz in [-1.0_f32, 1.0] {
            scene.push(
                EnvironmentObject::decoration(PILLAR, arena.at(sx * diagonal, sz * diagonal))
                    .with_collision(collision),
            );
        }
    }
}

/// The centres of the wall segments covering `center ± half`.
///
/// The span is a whole number of segments by construction — see [`AREAS`] — so this covers it
/// exactly, with no overlap and no gap.
fn span_centers(center: f32, half: f32) -> impl Iterator<Item = f32> {
    let count = ((half * 2.0 / SEGMENT).round() as i32).max(1);
    (0..count).map(move |index| center - half + SEGMENT * (index as f32 + 0.5))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn the_dungeon_is_built_identically_every_time() {
        assert_eq!(ashen_reliquary().signature(), ashen_reliquary().signature());
    }

    #[test]
    fn every_room_is_a_whole_number_of_wall_segments() {
        // The property the doorway depends on: a fractional span would need a scaled segment,
        // and a scaled segment's mesh and its collision box are two numbers that drift.
        for area in &AREAS {
            for half in [area.half_x, area.half_z] {
                let segments = half * 2.0 / SEGMENT;
                assert!(
                    (segments - segments.round()).abs() < 1e-4,
                    "{} spans {segments} segments",
                    area.name
                );
                assert!(half >= DOOR_HALF, "{} is narrower than its door", area.name);
            }
        }
    }

    #[test]
    fn the_rooms_are_in_order_and_do_not_overlap() {
        for pair in AREAS.windows(2) {
            let (near, far) = (pair[0], pair[1]);
            assert!(
                far.south_edge() < near.north_edge(),
                "{} overlaps {}",
                far.name,
                near.name
            );
        }
    }

    #[test]
    fn the_player_starts_inside_the_first_room() {
        assert!(AREAS[0].contains(ENTRANCE));
        let scene = ashen_reliquary();
        assert_eq!(scene.spawns.player, ENTRANCE);
        assert_eq!(scene.half_extent, DUNGEON_HALF_EXTENT);
    }

    #[test]
    fn every_room_and_corridor_is_walled_and_most_of_it_blocks() {
        let scene = ashen_reliquary();
        // Five rooms and four corridors' worth of segments, plus pillars: a dungeon whose
        // walls stopped being built halfway would still pass every other test here.
        assert!(scene.count_of(WALL_SEGMENT) > 80, "{}", scene.len());
        assert_eq!(scene.count_of(PILLAR), AREA_COUNT * 4 + 4);
        assert!(scene.blocking_count() >= scene.count_of(WALL_SEGMENT));
    }

    #[test]
    fn the_boss_arena_is_large_enough_for_its_own_nova() {
        // Immolation Nova reaches 14 m from the centre. The room has to let the player get
        // further than that, or the mechanic is unavoidable rather than difficult.
        let arena = AREAS[AREA_COUNT - 1];
        assert!(
            arena.half_x > 14.0 + 4.0 && arena.half_z > 14.0 + 4.0,
            "the boss arena is {} x {}",
            arena.half_x,
            arena.half_z
        );
    }

    #[test]
    fn a_span_is_covered_exactly() {
        let centers: Vec<f32> = span_centers(0.0, 16.0).collect();
        assert_eq!(centers, vec![-12.0, -4.0, 4.0, 12.0]);
        // Each segment covers ±4 around its centre, so the four of them cover −16..16.
        assert!((centers[0] - SEGMENT * 0.5 + 16.0).abs() < 1e-4);
        assert!((centers[3] + SEGMENT * 0.5 - 16.0).abs() < 1e-4);
    }
}
