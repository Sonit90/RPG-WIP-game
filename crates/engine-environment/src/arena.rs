//! The layouts: the shipping combat arena, the inspection scene, and the stress scene.
//!
//! ```text
//!                        distant ridges (6)
//!            ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//!                treeline: pines, rocks, ferns (r 46..110)
//!
//!            P---W--W--W--P--W--W--W--P---W--W--W--P        z = -39, gate side
//!            |              [ A R C H ]              |
//!            W                    D                  W      D  featured dummy
//!            W                                       W
//!            P              (P)          b b b       P      b  dummy group
//!            W                                       W
//!            W        ^                              W      ^  player spawn
//!            P---W--W--W--W--P--W--W--W--W--W--W--P--P      z = +39
//!
//!            x = -39                              x = +39
//! ```
//!
//! # Why the layout is a function and not a file
//!
//! A scene file needs a parser, a schema, an error path for a malformed one, and a decision
//! about what happens when it names an asset that does not exist. All four are real work
//! that buys nothing until scenes are authored by someone who is not editing the source
//! anyway. What the phase brief actually asks for is that the composition is **data** rather
//! than being hardcoded into the renderer, and that is what this is: a function that
//! produces a `Vec<EnvironmentObject>`, consumed by three crates that each read it as data.
//! Replacing it with a loader is a change to this module and nothing else, which is the
//! property that matters — see the "second arena" section of `phase_environment.md`.
//!
//! # The one composition rule
//!
//! [`CLEAR_RADIUS`] metres around the origin holds nothing but the floor. That is the
//! combat space: the player, the dummies and every area ability live inside it, and an
//! ability indicator that clipped through a boulder would be lying about what it hits. It
//! is asserted, not intended.

use engine_core::math::{Quat, Vec3};
use engine_core::transform::Transform;

use crate::asset::{
    ARCH, ARENA_FLOOR, BANNER, BRAZIER, EnvironmentAsset, FERN, GROUND_PLAIN, LIBRARY, PILLAR,
    PINE, RIDGE, ROCK_MEDIUM, ROCK_SMALL, ROCK_SPIRE, SHRUB, SKY_DOME, SUPPLY_CRATE, WALL_SEGMENT,
};
use crate::lighting::EnvironmentLighting;
use crate::object::{EnvironmentCollision, EnvironmentObject};
use crate::scene::{EnvironmentScene, SpawnPoints};
use crate::{hash_range, hash01};

/// Half-width of the square the player is confined to, in metres.
///
/// The same 40 the training arena has always used, so this phase changes what the arena
/// *looks* like without changing where the player can go.
pub const ARENA_HALF_EXTENT: f32 = 40.0;

/// How far from the wall the decorative kerb stands, in metres.
///
/// Inside the collision wall rather than on it: the wall at ±40 is what stops the player,
/// and geometry standing exactly on a collision plane means a character pressed against the
/// wall has its camera inside a pillar.
const KERB_INSET: f32 = 1.4;

/// Radius around the origin that holds nothing but the floor, in metres.
///
/// The combat space. Sized from the arena's own content: the dummy group's furthest member
/// sits about 24 m out, and the largest area ability reaches past it.
pub const CLEAR_RADIUS: f32 = 30.0;

/// Length of one kerb wall segment, in metres. The mesh is authored at this length.
pub const WALL_SEGMENT_LENGTH: f32 = 8.0;

/// How high above the ground plain the arena floor is drawn, in metres.
///
/// Two centimetres. Coplanar surfaces are a depth-buffer tie and a tie flickers across the
/// whole floor; two centimetres is below anything the camera can resolve at gameplay
/// distance and far above the depth buffer's precision at these ranges.
pub const FLOOR_LIFT: f32 = 0.02;

/// Radius of the drawn arena floor disc, in metres. The mesh is authored at this radius.
pub const FLOOR_RADIUS: f32 = 41.0;

/// The shipping combat arena.
///
/// Deterministic: two calls produce byte-identical scenes, which
/// [`EnvironmentScene::signature`] makes checkable in one line.
#[must_use]
pub fn combat_arena() -> EnvironmentScene {
    let mut scene = EnvironmentScene::with_capacity("combat_arena", 192);
    scene.spawns = SpawnPoints::default();
    scene.lighting = EnvironmentLighting::arena_afternoon();
    scene.half_extent = ARENA_HALF_EXTENT;
    scene.ground_height = 0.0;

    push_backdrop(&mut scene);
    push_boundary(&mut scene);
    push_arena_props(&mut scene);
    push_scatter(&mut scene, 1.0);
    scene
}

/// The combat arena plus one of every library asset, laid out in two rows to be looked at.
///
/// The environment demo scene. It is the *same* arena rather than a bespoke one, for the
/// same reason the character test bed is the arena with the fight taken out: the thing being
/// inspected has to be the thing the game ships, or inspecting it proves nothing. The rows
/// stand inside the north-east quarter, past the combat space, where the camera can be
/// walked down them.
#[must_use]
pub fn environment_demo() -> EnvironmentScene {
    let mut scene = combat_arena();
    scene.name = "environment_demo".into();

    // Two rows of eight. Spaced far enough apart that a large asset does not overlap the
    // one beside it, and placed past `CLEAR_RADIUS` so the demo row is not in the arena.
    const SPACING: f32 = 7.0;
    const PER_ROW: usize = 8;
    for (index, asset) in LIBRARY.iter().enumerate() {
        // The sky dome and the ground plain are the scene, not exhibits: a second one would
        // be a shell inside the shell and a second floor two centimetres above the first.
        if *asset == SKY_DOME || *asset == GROUND_PLAIN || *asset == ARENA_FLOOR {
            continue;
        }
        let row = index / PER_ROW;
        let column = index % PER_ROW;
        let x = (column as f32 - (PER_ROW as f32 - 1.0) * 0.5) * SPACING;
        let z = 27.0 + row as f32 * 7.5;
        scene.push(exhibit(*asset, Vec3::new(x, 0.0, z)));
    }
    scene
}

/// A scene with far more environment in it than the arena, for measuring how it scales.
///
/// `density` multiplies the scatter counts. The goal is not to make the game scene this
/// dense; it is to find out whether the cost of the environment is linear in the number of
/// objects, sub-linear because culling and batching absorb it, or super-linear because
/// something in the path is quadratic.
#[must_use]
pub fn stress_arena(density: f32) -> EnvironmentScene {
    let mut scene = EnvironmentScene::with_capacity("stress", 2_048);
    scene.spawns = SpawnPoints::default();
    scene.lighting = EnvironmentLighting::arena_afternoon();
    scene.half_extent = ARENA_HALF_EXTENT;
    scene.ground_height = 0.0;

    push_backdrop(&mut scene);
    push_boundary(&mut scene);
    push_arena_props(&mut scene);
    push_scatter(&mut scene, density.max(0.0));
    scene
}

/// The sky, the plain and the arena floor: the three things everything else stands on.
fn push_backdrop(scene: &mut EnvironmentScene) {
    // The shell. Authored inside-out at a radius comfortably beyond the far plane's useful
    // range but inside it, so it is never clipped and never occludes anything.
    scene.push(EnvironmentObject::decoration(SKY_DOME, Vec3::ZERO));

    // The plain. The one object with a non-uniform scale in the library: it is a unit quad
    // and it has to cover the horizon, so its transform is set directly rather than through
    // `with_scale`, which is uniform by design.
    scene.push(EnvironmentObject {
        asset: GROUND_PLAIN,
        transform: Transform::from_translation(Vec3::ZERO).with_scale(Vec3::new(640.0, 1.0, 640.0)),
        collision: EnvironmentCollision::None,
    });

    // The arena's own floor, two centimetres above the plain.
    scene.push(EnvironmentObject::decoration(
        ARENA_FLOOR,
        Vec3::new(0.0, FLOOR_LIFT, 0.0),
    ));
}

/// The kerb, the pillars, the gate arch and the banners: what makes the arena an arena.
///
/// Four sides of eight-metre segments with a gap on the −Z side for the gate. The gap is
/// where the arch stands, which is what gives the arena a front and therefore a composition
/// — a symmetric ring of identical wall has no direction and reads as a container.
fn push_boundary(scene: &mut EnvironmentScene) {
    let edge = ARENA_HALF_EXTENT - KERB_INSET;
    // Centres at ±(4, 12, 20, 28, 36): five segments each way, ten per side.
    let offsets: [f32; 10] = [
        -36.0, -28.0, -20.0, -12.0, -4.0, 4.0, 12.0, 20.0, 28.0, 36.0,
    ];
    // The gate: the two segments either side of the −Z side's centre are left out.
    const GATE_HALF_WIDTH: f32 = 13.0;

    let wall_collision = EnvironmentCollision::standing(WALL_SEGMENT_LENGTH * 0.5, 1.15, 0.45);
    for offset in offsets {
        // −Z and +Z sides run along X.
        for side in [-1.0_f32, 1.0] {
            let z = side * edge;
            if side < 0.0 && offset.abs() < GATE_HALF_WIDTH {
                continue;
            }
            scene.push(
                EnvironmentObject::decoration(WALL_SEGMENT, Vec3::new(offset, 0.0, z))
                    .with_collision(wall_collision),
            );
        }
        // −X and +X sides run along Z, so the segment is turned a quarter turn.
        for side in [-1.0_f32, 1.0] {
            scene.push(
                EnvironmentObject::decoration(WALL_SEGMENT, Vec3::new(side * edge, 0.0, offset))
                    .with_collision(wall_collision)
                    .with_yaw(std::f32::consts::FRAC_PI_2),
            );
        }
    }

    // Pillars: the four corners, the midpoint of each side, and the two flanking the gate.
    let pillar_collision = EnvironmentCollision::standing(0.62, 4.6, 0.62);
    let mut pillars: Vec<Vec3> = Vec::with_capacity(16);
    for x in [-edge, edge] {
        for z in [-edge, edge] {
            pillars.push(Vec3::new(x, 0.0, z));
        }
    }
    pillars.push(Vec3::new(0.0, 0.0, edge));
    pillars.push(Vec3::new(-edge, 0.0, 0.0));
    pillars.push(Vec3::new(edge, 0.0, 0.0));
    pillars.push(Vec3::new(-GATE_HALF_WIDTH, 0.0, -edge));
    pillars.push(Vec3::new(GATE_HALF_WIDTH, 0.0, -edge));
    for (index, position) in pillars.iter().enumerate() {
        scene.push(
            EnvironmentObject::decoration(PILLAR, *position)
                .with_collision(pillar_collision)
                // A little variation in height, so a row of pillars is not a row of clones.
                // Deterministic, and small enough that they still read as a set.
                .with_scale(hash_range(index as u32, 41, 0.94, 1.08)),
        );
    }

    // The gate arch spans the two gate pillars, resting on their capitals. Decoration: the
    // legs it appears to stand on *are* the pillars, which already block, and the span
    // itself is five metres overhead where nothing can reach it.
    scene.push(EnvironmentObject::decoration(
        ARCH,
        Vec3::new(0.0, 0.0, -edge),
    ));

    // Banners on the four corner pillars and the two gate pillars, facing inwards.
    let banners: [(Vec3, f32); 6] = [
        (Vec3::new(-edge, 0.0, -edge), -std::f32::consts::FRAC_PI_4),
        (Vec3::new(edge, 0.0, -edge), std::f32::consts::FRAC_PI_4),
        (
            Vec3::new(-edge, 0.0, edge),
            -3.0 * std::f32::consts::FRAC_PI_4,
        ),
        (
            Vec3::new(edge, 0.0, edge),
            3.0 * std::f32::consts::FRAC_PI_4,
        ),
        (Vec3::new(-GATE_HALF_WIDTH, 0.0, -edge), 0.0),
        (Vec3::new(GATE_HALF_WIDTH, 0.0, -edge), 0.0),
    ];
    for (position, yaw) in banners {
        // Offset a little towards the arena so the cloth hangs clear of the pillar's face.
        let inward = Vec3::new(-position.x, 0.0, -position.z);
        let length = inward.x.hypot(inward.z).max(1e-3);
        let offset = inward * (0.95 / length);
        scene.push(EnvironmentObject::decoration(BANNER, position + offset).with_yaw(yaw));
    }
}

/// Braziers and crates: the props inside the wall that give the space a scale reference.
///
/// All of them outside [`CLEAR_RADIUS`], which is the point of that constant.
fn push_arena_props(scene: &mut EnvironmentScene) {
    let brazier_collision = EnvironmentCollision::standing(0.45, 1.35, 0.45);
    for (index, (x, z)) in [(-26.0, -26.0), (26.0, -26.0), (-26.0, 26.0), (26.0, 26.0)]
        .into_iter()
        .enumerate()
    {
        scene.push(
            EnvironmentObject::decoration(BRAZIER, Vec3::new(x, 0.0, z))
                .with_collision(brazier_collision)
                .with_yaw(hash_range(index as u32, 7, 0.0, std::f32::consts::TAU)),
        );
    }

    let crate_collision = EnvironmentCollision::standing(0.46, 0.92, 0.46);
    let crates: [(f32, f32); 6] = [
        (-31.5, -20.0),
        (-30.6, -21.4),
        (32.0, 18.0),
        (30.8, 19.6),
        (-18.0, 33.0),
        (21.0, -32.0),
    ];
    for (index, (x, z)) in crates.into_iter().enumerate() {
        scene.push(
            EnvironmentObject::decoration(SUPPLY_CRATE, Vec3::new(x, 0.0, z))
                .with_collision(crate_collision)
                .with_yaw(hash_range(index as u32, 19, 0.0, std::f32::consts::TAU))
                .with_scale(hash_range(index as u32, 23, 0.85, 1.15)),
        );
    }
}

/// The natural scatter: rocks, pines, shrubs and ferns, and the distant ridges.
///
/// Placed on rings rather than in a square, so density does not pile up in the corners, and
/// with a deterministic hash rather than a random number generator, so the arena is the same
/// arena on every run.
///
/// `density` scales every count. 1.0 is the shipping arena.
fn push_scatter(scene: &mut EnvironmentScene, density: f32) {
    let scaled = |base: usize| ((base as f32) * density).round().max(0.0) as usize;

    // Rocks, mostly outside the wall with a few inside the corners. Nothing placed by
    // `ring_scatter` blocks — see the function's own docs for why unreachable geometry does
    // not get a collision shape.
    ring_scatter(
        scene,
        &[ROCK_SMALL, ROCK_SMALL, ROCK_MEDIUM, ROCK_SPIRE],
        scaled(30),
        101,
        46.0,
        104.0,
        0.75,
        1.5,
    );
    inside_scatter(scene, &[ROCK_SMALL, ROCK_MEDIUM], scaled(7), 211, 0.7, 1.15);

    // The treeline. Pines are the tallest scatter piece and the only thing that gives the
    // horizon a silhouette between the wall and the ridges.
    ring_scatter(
        scene,
        &[PINE, PINE, PINE],
        scaled(34),
        307,
        50.0,
        118.0,
        0.8,
        1.45,
    );

    // Ground cover, both sides of the wall. Never blocks: knee-high geometry a player can
    // trip on without being able to see why is worse than none.
    ring_scatter(
        scene,
        &[SHRUB, FERN, FERN],
        scaled(38),
        409,
        43.0,
        96.0,
        0.8,
        1.35,
    );
    inside_scatter(scene, &[SHRUB, FERN], scaled(16), 503, 0.7, 1.1);

    // The far background. Six silhouettes, deliberately few and deliberately huge: they are
    // there to put something behind the treeline, not to be looked at.
    for index in 0..6_u32 {
        let angle = (index as f32 / 6.0) * std::f32::consts::TAU + 0.4;
        let distance = hash_range(index, 601, 235.0, 300.0);
        scene.push(
            EnvironmentObject::decoration(
                RIDGE,
                Vec3::new(angle.cos() * distance, 0.0, angle.sin() * distance),
            )
            // Facing the origin, so the silhouette is seen broadside from the arena.
            .with_yaw(-angle)
            .with_scale(hash_range(index, 607, 0.85, 1.35)),
        );
    }
}

/// Scatters `count` objects on an annulus between `inner` and `outer` metres.
///
/// **Nothing placed here blocks.** Every annulus in this module starts past
/// [`ARENA_HALF_EXTENT`], and the arena's collision wall confines the character inside it —
/// so a collision shape out here is a broad-phase entry, a proxy, an AABB and a narrow-phase
/// candidate for geometry no capsule can ever reach. Thirty-four pine trunks nothing can
/// touch is thirty-four objects the grid has to hold and the query has to skip.
///
/// If the arena ever opens up, the rule to change is this comment and the one line below it,
/// not every call site.
#[allow(
    clippy::too_many_arguments,
    reason = "a placement rule has this many knobs"
)]
fn ring_scatter(
    scene: &mut EnvironmentScene,
    palette: &[EnvironmentAsset],
    count: usize,
    seed: u32,
    inner: f32,
    outer: f32,
    min_scale: f32,
    max_scale: f32,
) {
    if palette.is_empty() {
        return;
    }
    for index in 0..count as u32 {
        // The angle is jittered around an even spread rather than being wholly random: a
        // wholly random angle clumps, and a clump of pines against an empty arc reads as a
        // mistake rather than as nature.
        let spread = std::f32::consts::TAU / count.max(1) as f32;
        let angle = index as f32 * spread + hash_range(index, seed, -spread * 0.45, spread * 0.45);
        // Square-rooted so the annulus fills evenly by area rather than by radius, which
        // would crowd the inner edge.
        let t = hash01(index, seed + 1).sqrt();
        let distance = inner + (outer - inner) * t;
        let asset =
            palette[(hash01(index, seed + 2) * palette.len() as f32) as usize % palette.len()];
        scene.push(
            EnvironmentObject::decoration(
                asset,
                Vec3::new(angle.cos() * distance, 0.0, angle.sin() * distance),
            )
            .with_yaw(hash_range(index, seed + 3, 0.0, std::f32::consts::TAU))
            .with_scale(hash_range(index, seed + 4, min_scale, max_scale)),
        );
    }
}

/// Scatters `count` objects inside the wall but outside the combat space.
///
/// These *are* reachable, so [`natural_collision`] decides what each one blocks.
fn inside_scatter(
    scene: &mut EnvironmentScene,
    palette: &[EnvironmentAsset],
    count: usize,
    seed: u32,
    min_scale: f32,
    max_scale: f32,
) {
    if palette.is_empty() {
        return;
    }
    // Between the combat space and the kerb, with a metre of margin at each end so nothing
    // straddles either boundary.
    let inner = CLEAR_RADIUS + 1.5;
    let outer = ARENA_HALF_EXTENT - KERB_INSET - 3.0;
    for index in 0..count as u32 {
        let angle = hash_range(index, seed, 0.0, std::f32::consts::TAU);
        let t = hash01(index, seed + 1).sqrt();
        let distance = inner + (outer - inner) * t;
        let asset =
            palette[(hash01(index, seed + 2) * palette.len() as f32) as usize % palette.len()];
        let position = Vec3::new(angle.cos() * distance, 0.0, angle.sin() * distance);
        // The ring is a circle and the wall is a square, so a point on the ring can still be
        // outside the wall near a corner's diagonal. Rejected rather than clamped: clamping
        // would pile objects against the kerb.
        let limit = ARENA_HALF_EXTENT - KERB_INSET - 2.0;
        if position.x.abs() > limit || position.z.abs() > limit {
            continue;
        }
        scene.push(
            EnvironmentObject::decoration(asset, position)
                .with_yaw(hash_range(index, seed + 3, 0.0, std::f32::consts::TAU))
                .with_scale(hash_range(index, seed + 4, min_scale, max_scale))
                .with_collision(natural_collision(asset)),
        );
    }
}

/// What a *reachable* scattered natural asset blocks.
///
/// The phase brief's own table, as code: a large rock is a primitive, a tree is its trunk,
/// and ground cover is nothing at all. Only consulted for placements inside the arena wall.
fn natural_collision(asset: EnvironmentAsset) -> EnvironmentCollision {
    // The library-wide table, which was this function's numbers: a large rock is a primitive,
    // a tree is its trunk, ground cover is nothing. The scatter has no reason to disagree
    // with what an editor would place, and two copies of these numbers would drift.
    crate::object::default_collision(asset)
}

/// One asset placed on the demo row, at a sensible size and angle for looking at.
fn exhibit(asset: EnvironmentAsset, position: Vec3) -> EnvironmentObject {
    // Turned three-eighths of a turn so a flat-fronted piece is seen in three-quarter view
    // rather than face-on, which is what shows a silhouette.
    let object = EnvironmentObject::decoration(asset, position)
        .with_yaw(std::f32::consts::FRAC_PI_4 * 1.5)
        .with_collision(natural_collision(asset));
    match asset {
        a if a == WALL_SEGMENT => object.with_collision(EnvironmentCollision::standing(
            WALL_SEGMENT_LENGTH * 0.5,
            1.15,
            0.45,
        )),
        a if a == PILLAR => object.with_collision(EnvironmentCollision::standing(0.62, 4.6, 0.62)),
        a if a == BRAZIER => {
            object.with_collision(EnvironmentCollision::standing(0.45, 1.35, 0.45))
        }
        a if a == SUPPLY_CRATE => {
            object.with_collision(EnvironmentCollision::standing(0.46, 0.92, 0.46))
        }
        // A ridge is 200 metres of mountain; on the row it is a model of one.
        a if a == RIDGE => object.with_scale(0.05),
        _ => object,
    }
}

/// The rotation a yaw produces. Exposed for tests and for a consumer that wants to compare.
#[must_use]
pub fn yaw_rotation(yaw: f32) -> Quat {
    Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn the_arena_is_built_identically_every_time() {
        let first = combat_arena();
        let second = combat_arena();
        assert_eq!(first.signature(), second.signature());
        assert_eq!(first, second);
        assert_eq!(first.name, "combat_arena");
    }

    #[test]
    fn the_arena_contains_every_category_the_phase_asks_for() {
        let scene = combat_arena();
        for asset in [GROUND_PLAIN, ARENA_FLOOR, SKY_DOME, ARCH] {
            assert_eq!(scene.count_of(asset), 1, "{asset} should appear once");
        }
        assert!(scene.count_of(WALL_SEGMENT) >= 30);
        assert!(scene.count_of(PILLAR) >= 9);
        assert!(scene.count_of(BANNER) >= 4);
        assert_eq!(scene.count_of(BRAZIER), 4);
        assert!(scene.count_of(SUPPLY_CRATE) >= 4);
        assert!(scene.count_of(RIDGE) >= 4);

        let rocks =
            scene.count_of(ROCK_SMALL) + scene.count_of(ROCK_MEDIUM) + scene.count_of(ROCK_SPIRE);
        assert!(rocks >= 25, "{rocks} rocks");
        let plants = scene.count_of(PINE) + scene.count_of(SHRUB) + scene.count_of(FERN);
        assert!(plants >= 60, "{plants} plants");
    }

    #[test]
    fn the_combat_space_holds_nothing_but_the_floor() {
        // The one composition rule, asserted rather than intended. An ability indicator that
        // clipped through a boulder would be lying about what it covers.
        let scene = combat_arena();
        for object in &scene.objects {
            if matches!(
                object.asset,
                a if a == GROUND_PLAIN || a == ARENA_FLOOR || a == SKY_DOME
            ) {
                continue;
            }
            let position = object.position();
            let radius = position.x.hypot(position.z);
            assert!(
                radius >= CLEAR_RADIUS,
                "{} stands {radius:.1} m from the middle of the fight",
                object.asset
            );
        }
    }

    #[test]
    fn every_spawn_point_is_well_inside_the_clear_space() {
        let scene = combat_arena();
        let spawns = scene.spawns;
        // The group's furthest member, so the check covers the whole group rather than its
        // centre. Four dummies at three metres is a two-by-two grid 1.5 m from the middle.
        let furthest = spawns.group_center
            + Vec3::new(spawns.group_spacing * 1.5, 0.0, spawns.group_spacing * 1.5);
        for point in [spawns.player, spawns.featured_dummy, furthest] {
            let radius = point.x.hypot(point.z);
            assert!(radius < CLEAR_RADIUS, "{point:?} is {radius:.1} m out");
        }
    }

    #[test]
    fn nothing_is_placed_outside_the_arena_but_inside_the_wall_line() {
        // Every object is either comfortably inside the kerb or comfortably outside the
        // collision wall. One straddling the wall would poke through it.
        let scene = combat_arena();
        for object in &scene.objects {
            if matches!(
                object.asset,
                a if a == GROUND_PLAIN || a == ARENA_FLOOR || a == SKY_DOME
            ) {
                continue;
            }
            let position = object.position();
            let inside = position.x.abs() <= ARENA_HALF_EXTENT - KERB_INSET + 0.01
                && position.z.abs() <= ARENA_HALF_EXTENT - KERB_INSET + 0.01;
            let outside = position.x.hypot(position.z) >= ARENA_HALF_EXTENT + 2.0;
            assert!(
                inside || outside,
                "{} at {position:?} straddles the arena wall",
                object.asset
            );
        }
    }

    #[test]
    fn the_gate_is_a_gap_in_the_wall_with_an_arch_over_it() {
        let scene = combat_arena();
        let edge = ARENA_HALF_EXTENT - KERB_INSET;
        let gate_segments = scene
            .objects
            .iter()
            .filter(|object| object.asset == WALL_SEGMENT)
            .filter(|object| {
                let position = object.position();
                (position.z + edge).abs() < 0.01 && position.x.abs() < 13.0
            })
            .count();
        assert_eq!(gate_segments, 0, "the gate is walled up");

        let arch = scene
            .objects
            .iter()
            .find(|object| object.asset == ARCH)
            .expect("the arena has an arch");
        assert!((arch.position().z + edge).abs() < 0.01);
        assert!(
            !arch.collision.blocks(),
            "the span is five metres overhead; the pillars under it are what block"
        );
    }

    #[test]
    fn what_blocks_is_a_small_fraction_of_what_is_drawn() {
        // Collision complexity is chosen per object, not derived from the visual mesh.
        let scene = combat_arena();
        let blocking = scene.blocking_count();
        assert!(blocking > 40, "the arena has a boundary: {blocking}");
        assert!(
            blocking * 2 < scene.len(),
            "{blocking} of {} objects block, which is most of them",
            scene.len()
        );
    }

    #[test]
    fn ground_cover_never_blocks_and_large_obstacles_always_do() {
        let scene = combat_arena();
        for object in &scene.objects {
            let asset = object.asset;
            if asset == FERN || asset == SHRUB || asset == ROCK_SMALL || asset == BANNER {
                assert!(
                    !object.collision.blocks(),
                    "{asset} should be walked through"
                );
            }
            if asset == PILLAR || asset == WALL_SEGMENT || asset == BRAZIER {
                assert!(object.collision.blocks(), "{asset} should stop a character");
            }
        }
        // A reachable medium rock does block, so the rule is "ground cover never blocks"
        // rather than "nothing scattered blocks".
        assert!(
            scene
                .objects
                .iter()
                .any(|object| object.asset == ROCK_MEDIUM && object.collision.blocks()),
            "the boulders inside the arena are obstacles"
        );
    }

    #[test]
    fn nothing_a_character_cannot_reach_carries_a_collision_shape() {
        // A shape outside the wall is a broad-phase proxy, an AABB and a narrow-phase
        // candidate for geometry no capsule can ever touch. The arena has ninety-odd objects
        // out there.
        let scene = combat_arena();
        let mut unreachable = 0;
        for object in &scene.objects {
            let position = object.position();
            if position.x.abs() <= ARENA_HALF_EXTENT && position.z.abs() <= ARENA_HALF_EXTENT {
                continue;
            }
            unreachable += 1;
            assert!(
                !object.collision.blocks(),
                "{} at {position:?} is outside the wall and still blocks",
                object.asset
            );
        }
        assert!(unreachable > 60, "only {unreachable} objects are outside");
    }

    #[test]
    fn the_scene_uses_far_fewer_distinct_assets_than_it_has_objects() {
        // The instancing precondition: every placement of an asset must be able to share one
        // upload, so the number of uploads is the number of *assets*, not of objects.
        let scene = combat_arena();
        let distinct = scene.distinct_assets();
        assert!(distinct.len() <= LIBRARY.len());
        assert!(
            scene.len() > distinct.len() * 8,
            "{} objects over {} assets is not worth instancing",
            scene.len(),
            distinct.len()
        );
        for asset in &distinct {
            assert!(LIBRARY.contains(asset), "{asset} is not in the library");
        }
    }

    #[test]
    fn the_demo_scene_is_the_arena_plus_one_of_everything() {
        let arena = combat_arena();
        let demo = environment_demo();
        assert_eq!(demo.name, "environment_demo");
        assert!(demo.len() > arena.len());

        // Every asset that can be exhibited is, and the three that are the scene itself are
        // not duplicated.
        for asset in LIBRARY {
            if asset == SKY_DOME || asset == GROUND_PLAIN || asset == ARENA_FLOOR {
                assert_eq!(
                    demo.count_of(asset),
                    1,
                    "{asset} is the scene, not an exhibit"
                );
                continue;
            }
            assert!(
                demo.count_of(asset) > arena.count_of(asset),
                "{asset} is not on the demo row"
            );
        }
        assert_eq!(demo.signature(), environment_demo().signature());
    }

    #[test]
    fn the_demo_row_stands_inside_the_arena_where_it_can_be_walked_around() {
        let demo = environment_demo();
        let arena = combat_arena();
        let extra = demo.len() - arena.len();
        assert_eq!(extra, LIBRARY.len() - 3, "one of each exhibitable asset");

        for object in demo.objects.iter().skip(arena.len()) {
            let position = object.position();
            assert!(position.z > CLEAR_RADIUS - 4.0, "{position:?}");
            assert!(
                position.x.abs() < ARENA_HALF_EXTENT - KERB_INSET
                    && position.z.abs() < ARENA_HALF_EXTENT - KERB_INSET,
                "{position:?} is outside the wall"
            );
        }
    }

    #[test]
    fn the_stress_scene_scales_with_its_density_and_keeps_the_arena_intact() {
        let normal = stress_arena(1.0);
        let dense = stress_arena(4.0);
        let bare = stress_arena(0.0);

        assert!(
            dense.len() > normal.len() * 2,
            "{} vs {}",
            dense.len(),
            normal.len()
        );
        assert!(bare.len() < normal.len());
        // The boundary is not scatter and does not scale: a stress scene still has to be an
        // arena, or it measures a different scene rather than a denser one.
        for scene in [&normal, &dense, &bare] {
            assert_eq!(scene.count_of(ARENA_FLOOR), 1);
            assert!(scene.count_of(WALL_SEGMENT) >= 30);
            assert_eq!(scene.count_of(BRAZIER), 4);
        }
        assert_eq!(dense.signature(), stress_arena(4.0).signature());
    }

    #[test]
    fn the_stress_scene_reaches_the_object_counts_the_brief_names() {
        // 100 rocks, 100 vegetation, 50 architectural, 100 miscellaneous props.
        let scene = stress_arena(4.0);
        let rocks =
            scene.count_of(ROCK_SMALL) + scene.count_of(ROCK_MEDIUM) + scene.count_of(ROCK_SPIRE);
        let plants = scene.count_of(PINE) + scene.count_of(SHRUB) + scene.count_of(FERN);
        let architecture =
            scene.count_of(WALL_SEGMENT) + scene.count_of(PILLAR) + scene.count_of(ARCH);
        assert!(rocks >= 100, "{rocks} rocks");
        assert!(plants >= 200, "{plants} plants");
        // 46 rather than the brief's 50: the boundary is the arena's, not the stress
        // scene's, and multiplying it would measure a different level rather than a denser
        // one. The brief says to adjust the numbers to the scene, and this is the whole
        // boundary — 36 kerb segments, 9 pillars and the arch.
        assert!(architecture >= 46, "{architecture} architectural pieces");
    }

    #[test]
    fn every_placement_that_blocks_produces_a_box_standing_on_the_floor() {
        let scene = combat_arena();
        for object in &scene.objects {
            let Some(world) = object.world_collision() else {
                assert!(!object.collision.blocks());
                continue;
            };
            let (min, max) = world.min_max();
            assert!(
                min.y.abs() < 0.01,
                "{} floats or is sunk: {min:?}",
                object.asset
            );
            assert!(max.y > min.y, "{} has no height", object.asset);
            assert!(
                world.half_extents.x > 0.0 && world.half_extents.z > 0.0,
                "{} has no footprint",
                object.asset
            );
        }
    }

    #[test]
    fn a_yaw_of_a_quarter_turn_is_the_rotation_it_claims_to_be() {
        let turned = yaw_rotation(std::f32::consts::FRAC_PI_2);
        let sent = turned.rotate(Vec3::new(0.0, 0.0, -1.0));
        assert!((sent.x + 1.0).abs() < 1e-4, "{sent:?}");
    }
}
