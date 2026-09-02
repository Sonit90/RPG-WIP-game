//! The showcase level: one small map that has one of everything the editor can author.
//!
//! ```text
//!                            cliff run
//!                    ┌──────────────────────┐
//!                    │       plateau        │
//!                    └──────┐        ┌──────┘
//!                     cave  │        │  cliff
//!                   entrance│        │
//!        ▲ spawn            │        │
//!   ─────┴──────────────────┴────────┴──────────  ground, sculpted into hills
//!
//!              ~~~~~~~~~~~~~~~~~~
//!                    lake
//! ```
//!
//! # Why this is generated rather than hand-written
//!
//! A level file is mostly numbers. Its terrain alone is sixteen thousand heights, and the
//! collision of the seven cave pieces in it is eighty-odd boxes — neither is something a
//! person writes by hand, and a hand-written file would go stale the moment a piece's shape
//! changed. So the level is described in code here, the same way the arena layouts in
//! `engine_environment::arena` are, and written out as an ordinary scene file that the game
//! loads like any other.
//!
//! It lives in this crate for the same reason the starter assets do: this is the content
//! generator, it already depends on the asset library, and none of it is linked into the game.
//! What ships is the JSON.
//!
//! # What it is for
//!
//! Verifying, in one place, that terrain, cliffs, an overhang, caves, water, placed assets and
//! a spawn point all work together — and that a level using every one of them survives being
//! written and read back. It is a test fixture that happens to be playable, not a designed
//! level, and nothing in the engine depends on it.

use engine_assets::procedural::starter::{MODULE, starter_collision};
use engine_core::math::{Vec2, Vec3};
use engine_core::transform::Transform;
use engine_environment::spawn::SpawnPoint;
use engine_environment::terrain::{BrushMode, Terrain};
use engine_environment::water::WaterBody;
use engine_environment::{EnvironmentAsset, EnvironmentObject, EnvironmentScene};

/// Where the cave mouth sits, and the line the tunnel runs along.
const CAVE_MOUTH: Vec3 = Vec3::new(-18.0, 0.0, -22.0);

/// The whole level.
#[must_use]
pub fn showcase() -> EnvironmentScene {
    let mut scene = EnvironmentScene::empty("showcase");
    scene.half_extent = 60.0;

    ground(&mut scene);
    cliffs(&mut scene);
    cave(&mut scene);
    lake(&mut scene);
    scatter(&mut scene);

    // Out in the open, facing the cliff line, so somebody who loads the level is looking at
    // what it is for. Yaw zero looks along -Z, which is where all of it is.
    scene.push_spawn(SpawnPoint::new(Vec3::new(0.0, 0.0, 18.0)));
    scene
}

/// The ground: a plain, a plateau to the north, and a hollow for the lake.
fn ground(scene: &mut EnvironmentScene) {
    // 2 m cells over the playable square: 61 points a side, which is 3 700 heights and about
    // seven thousand triangles. A blockout does not need finer than that, and the editor's
    // brush is six metres wide by default.
    let mut terrain = Terrain::flat(61, 2.0, 0.0);

    // The plateau the cliff line stands on, flattened after it is raised so its top is level
    // and something can be built on it.
    terrain.apply_brush(Vec3::new(0.0, 0.0, -34.0), 26.0, 6.0, BrushMode::Raise);
    terrain.apply_brush(Vec3::new(0.0, 0.0, -34.0), 18.0, 1.0, BrushMode::Flatten);

    // Two hills either side, so the plain is not a table.
    terrain.apply_brush(Vec3::new(34.0, 0.0, -6.0), 14.0, 3.0, BrushMode::Raise);
    terrain.apply_brush(Vec3::new(-36.0, 0.0, 6.0), 12.0, 2.0, BrushMode::Raise);

    // The hollow the lake sits in. Lowered rather than left flat, so the water has a bed and
    // "under water" is somewhere a player can actually stand.
    terrain.apply_brush(Vec3::new(20.0, 0.0, 26.0), 16.0, 2.5, BrushMode::Lower);
    terrain.apply_brush(Vec3::new(20.0, 0.0, 26.0), 16.0, 1.0, BrushMode::Smooth);

    // And the shelf the cave mouth is cut into: flat, so the pieces sit on it squarely.
    terrain.apply_brush(CAVE_MOUTH, 12.0, 1.0, BrushMode::Flatten);

    scene.terrain = Some(terrain);
}

/// The cliff run along the front of the plateau, and the overhang a player walks under.
fn cliffs(scene: &mut EnvironmentScene) {
    let front = -24.0;
    for (index, x) in [-8.0_f32, -4.0, 8.0, 12.0].into_iter().enumerate() {
        let piece = if index == 1 { "cliff_corner" } else { "cliff_straight" };
        place(scene, "cliffs", piece, Vec3::new(x, 0.0, front), 0.0);
    }
    // The overhang, in the gap in the run: the thing the whole cliff system exists for.
    place(
        scene,
        "cliffs",
        "cliff_overhang",
        Vec3::new(2.0, 0.0, front),
        0.0,
    );
    // A way up onto the plateau, so it is somewhere a player can get to.
    place(
        scene,
        "cliffs",
        "cliff_slope",
        Vec3::new(20.0, 0.0, front),
        0.0,
    );
    place(scene, "props", "arch", Vec3::new(0.0, 0.0, -6.0), 0.0);
}

/// A cave the player can walk in one end of and out of the other.
///
/// Laid on the module grid along -Z, which is what the editor's module snap produces: the
/// entrance at the mouth, a length of tunnel, a junction, a room, and an exit that comes back
/// out onto the plain.
fn cave(scene: &mut EnvironmentScene) {
    let (x, z) = (CAVE_MOUTH.x, CAVE_MOUTH.z);
    place(scene, "caves", "cave_entrance", Vec3::new(x, 0.0, z), 0.0);
    place(
        scene,
        "caves",
        "cave_tunnel_straight",
        Vec3::new(x, 0.0, z - MODULE),
        0.0,
    );
    // The junction is centred on its origin, so it fills the cell behind that tunnel.
    place(
        scene,
        "caves",
        "cave_junction",
        Vec3::new(x, 0.0, z - MODULE * 2.5),
        0.0,
    );
    // A dead end off the side of the junction, so the crossing is a crossing.
    place(
        scene,
        "caves",
        "cave_tunnel_straight",
        Vec3::new(x - MODULE * 0.5, 0.0, z - MODULE * 2.5),
        std::f32::consts::FRAC_PI_2,
    );
    place(
        scene,
        "caves",
        "cave_tunnel_straight",
        Vec3::new(x, 0.0, z - MODULE * 3.0),
        0.0,
    );
    // The room, then the way back out.
    place(
        scene,
        "caves",
        "cave_room_small",
        Vec3::new(x, 0.0, z - MODULE * 4.0 - 6.5),
        0.0,
    );
    place(
        scene,
        "caves",
        "cave_exit",
        Vec3::new(x, 0.0, z - MODULE * 4.0 - 13.0),
        0.0,
    );
}

/// The lake, in the hollow the terrain was lowered for.
fn lake(scene: &mut EnvironmentScene) {
    // Its surface a little under the plain, so the shore is a shore rather than a wall of
    // water, and deep enough that standing on the bed is under the surface.
    scene.push_water(WaterBody::new(
        Vec3::new(20.0, -0.6, 26.0),
        Vec2::new(13.0, 10.0),
        3.0,
    ));
}

/// Rocks, vegetation and a couple of props, so the level reads as somewhere rather than as a
/// list of systems.
fn scatter(scene: &mut EnvironmentScene) {
    let rocks = [
        ("rock_small", Vec3::new(6.0, 0.0, 8.0)),
        ("rock_medium", Vec3::new(-10.0, 0.0, 2.0)),
        ("boulder_large", Vec3::new(14.0, 0.0, -12.0)),
        ("rock_spire", Vec3::new(-26.0, 0.0, -10.0)),
        ("rock_small", Vec3::new(28.0, 0.0, 14.0)),
    ];
    for (index, (piece, at)) in rocks.into_iter().enumerate() {
        // A yaw off the hash of the index, so the scatter is varied and identical on every
        // run — the determinism the rest of this project's generated content keeps.
        let yaw = engine_environment::hash_range(index as u32, 7, 0.0, std::f32::consts::TAU);
        place(scene, "rocks", piece, at, yaw);
    }

    let plants = [
        ("tree_simple", Vec3::new(-4.0, 0.0, 14.0)),
        ("tree_simple", Vec3::new(9.0, 0.0, 20.0)),
        ("tree_simple", Vec3::new(-22.0, 0.0, 16.0)),
        ("bush_round", Vec3::new(2.0, 0.0, 11.0)),
        ("bush_round", Vec3::new(-14.0, 0.0, 9.0)),
        ("bush_round", Vec3::new(24.0, 0.0, 6.0)),
    ];
    for (index, (piece, at)) in plants.into_iter().enumerate() {
        let yaw = engine_environment::hash_range(index as u32, 19, 0.0, std::f32::consts::TAU);
        place(scene, "vegetation", piece, at, yaw);
    }

    place(scene, "props", "crate_wooden", Vec3::new(4.0, 0.0, 15.0), 0.4);
    place(scene, "props", "pillar", Vec3::new(-6.0, 0.0, -14.0), 0.0);
}

/// Places one starter piece, standing on whatever the ground under it turned out to be.
///
/// The collision comes from [`starter_collision`] — the same authored shapes the sidecar files
/// beside the models carry — and is interned in the scene, so the level file holds the boxes
/// and the game needs neither this crate nor those sidecars to load it.
fn place(scene: &mut EnvironmentScene, folder: &str, piece: &str, at: Vec3, yaw: f32) {
    let collision = match starter_collision(piece) {
        Some(shape) => scene.intern_collision(shape),
        // Everything without an authored shape keeps the automatic box the loader gives it,
        // which is the right answer for a rock and is what the editor would place too.
        None => engine_environment::object::default_collision(EnvironmentAsset::interned(piece)),
    };
    let position = Vec3::new(at.x, scene.ground_at(at), at.z);
    scene.push(EnvironmentObject {
        asset: EnvironmentAsset::interned(&format!("generated/environment/{folder}/{piece}")),
        transform: Transform::from_translation(position)
            .with_rotation(engine_core::math::Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0)),
        collision,
    });
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn the_showcase_has_one_of_everything_the_editor_can_author() {
        let scene = showcase();
        assert!(scene.terrain.is_some(), "no ground");
        assert!(!scene.water.is_empty(), "no water");
        assert!(!scene.spawn_points.is_empty(), "no spawn point");
        assert!(!scene.collision_shapes.is_empty(), "no authored collision");

        let names: Vec<&str> = scene
            .objects
            .iter()
            .map(|object| object.asset.name())
            .collect();
        for wanted in [
            "cliff_overhang",
            "cave_entrance",
            "cave_junction",
            "cave_room_small",
            "cave_exit",
            "tree_simple",
            "rock_small",
        ] {
            assert!(
                names.iter().any(|name| name.ends_with(wanted)),
                "the level has no {wanted}"
            );
        }
    }

    #[test]
    fn everything_in_it_stands_on_the_ground_rather_than_in_it() {
        // A level whose rocks are buried to the waist is a level nobody wants to look at, and
        // it is the mistake that happens when a layout is written before the terrain is.
        let scene = showcase();
        for object in &scene.objects {
            let position = object.position();
            let ground = scene.ground_at(position);
            assert!(
                (position.y - ground).abs() < 0.01,
                "{} stands at {:.2} and the ground there is {:.2}",
                object.asset.name(),
                position.y,
                ground
            );
        }
    }

    #[test]
    fn the_lake_has_a_bed_under_it_and_water_over_it() {
        // The two halves of a body of water actually working: the terrain was lowered where it
        // sits, and a point on that bed is inside the volume.
        let scene = showcase();
        let middle = Vec3::new(20.0, 0.0, 26.0);
        let bed = scene.ground_at(middle);
        assert!(bed < -0.5, "the lake bed is at {bed:.2}, not a hollow");

        let standing = Vec3::new(middle.x, bed + 0.9, middle.z);
        assert!(
            scene.is_submerged(standing),
            "a character standing on the lake bed at {standing:?} is not in the water"
        );
        // And the shore is dry.
        assert!(!scene.is_submerged(Vec3::new(0.0, 0.5, 18.0)));
    }

    #[test]
    fn the_level_survives_being_written_and_read_back() {
        // The whole point of generating it: what ships is the file, so the file has to be the
        // level. Terrain heights, interned collision shapes, water and the spawn point all
        // through the same round trip.
        let scene = showcase();
        let document = engine_environment::scene_file::SceneDocument::from_scene(&scene);
        let text = serde_json::to_string(&document).expect("the level is describable");
        let restored: engine_environment::scene_file::SceneDocument =
            serde_json::from_str(&text).expect("and readable");
        let restored = restored.to_scene("memory").expect("and valid");

        assert_eq!(restored.terrain, scene.terrain);
        assert_eq!(restored.water, scene.water);
        assert_eq!(restored.spawn_points, scene.spawn_points);
        assert_eq!(restored.collision_shapes, scene.collision_shapes);
        assert_eq!(restored.objects.len(), scene.objects.len());
    }
}
