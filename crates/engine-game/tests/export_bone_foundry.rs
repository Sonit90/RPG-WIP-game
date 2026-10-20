//! The Bone Foundry's level generator: a room graph in, a plain level file out.
//!
//! The same tool as the Drowned Catacombs' generator, drawn as a different shape: a **ring**
//! round a central furnace, with a yard hung off each arm. The dungeon is drawn once, as text,
//! below ([`MAP`]): one character per 8 m cell. This turns that drawing into walls, piers,
//! paving, props, lights, packs and the dungeon block. The output is an ordinary
//! [`EnvironmentScene`]: the editor opens it, edits it and saves it, and from then on the file
//! is the source of truth.
//!
//! Run on demand, never in CI, because it rewrites a checked-in asset:
//! `cargo test -p engine-game --test export_bone_foundry -- --ignored`
//!
//! The other tests are ordinary: they keep the checked-in file honest against this generator
//! and hold the layout to its own rules.
//!
//! ```text
//!   r23..28  B  FOUNDRY FLOOR   the Overseer (boss, behind the gate at z = -184)
//!   r22      a  antechamber
//!   r19..21  H  CASTING HALL    the last pack
//!   r18      c  corridor
//!   r16..17  R  north arm       a foreman walks his rounds
//!   r10..15  R  west / east arms (islands of furnace between them), yards W / X off the arms
//!   r8..9    R  south arm       a slag hauler walks the arm
//!   r7       c  corridor
//!   r4..6    F  forecourt
//!   r3       c  corridor, start barrier at z = -24
//!   r0..2    E  entrance hall (spawn)
//! ```

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use std::collections::HashMap;

use engine_core::math::Vec3;
use engine_environment::asset::{
    BANNER, BONE_PILE, BRAZIER, CRYPT_FLOOR, CRYPT_WALL, EnvironmentAsset, GROUND_PLAIN, IRON_FENCE,
    MAUSOLEUM, PILLAR, ROCK_MEDIUM, ROCK_SMALL, ROCK_SPIRE, SARCOPHAGUS, SKY_DOME, SUPPLY_CRATE,
    WALL_PIER,
};
use engine_environment::object::default_collision;
use engine_environment::{
    BossGate, DungeonSettings, EnvironmentCollision, EnvironmentLighting, EnvironmentObject,
    EnvironmentScene, MonsterPlacement, SceneLight, SpawnPoint, SpawnPoints, hash01,
};
use engine_game::enemy::EnemyKind;

const FILE: &str = "scenes/bone_foundry.json";

fn path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(FILE)
}

// ------------------------------------------------------------------------------------------
// The drawing
// ------------------------------------------------------------------------------------------

/// Metres along one side of a cell: one wall segment.
const CELL: f32 = 8.0;

/// The column whose cell is centred on x = 0.
const ORIGIN_COL: i32 = 10;

/// How many rows the drawing has, north (top line) to south (bottom line).
const ROWS: usize = 29;

/// How many columns every row has.
const COLS: usize = 21;

/// The level, one character per cell. `.` is solid; every other character is floor.
///
/// ```text
/// E entrance hall   F forecourt   R the ring   W bone-sorting yard   X slag works
/// c corridor        H casting hall   a antechamber   B foundry floor (boss arena)
/// ```
const MAP: [&str; ROWS] = [
    ".......BBBBBBB.......", // 28
    ".......BBBBBBB.......", // 27
    ".......BBBBBBB.......", // 26
    ".......BBBBBBB.......", // 25
    ".......BBBBBBB.......", // 24
    ".......BBBBBBB.......", // 23
    "..........a..........", // 22
    "........HHHHH........", // 21
    "........HHHHH........", // 20
    "........HHHHH........", // 19
    "..........c..........", // 18
    ".....RRRRRRRRRRR.....", // 17
    ".....RRRRRRRRRRR.....", // 16
    "WWWW.RR.......RR.XXXX", // 15
    "WWWW.RR.......RR.XXXX", // 14
    "WWWW.RR.......RR.XXXX", // 13
    "WWWWcRR.......RRcXXXX", // 12
    "WWWW.RR.......RR.XXXX", // 11
    "WWWW.RR.......RR.XXXX", // 10
    ".....RRRRRRRRRRR.....", // 9
    ".....RRRRRRRRRRR.....", // 8
    "..........c..........", // 7
    "........FFFFF........", // 6
    "........FFFFF........", // 5
    "........FFFFF........", // 4
    "..........c..........", // 3
    ".........EEE.........", // 2
    ".........EEE.........", // 1
    ".........EEE.........", // 0
];

/// The character of the cell at `(col, row)`, row zero being the southernmost.
fn glyph(col: i32, row: i32) -> char {
    if !(0..ROWS as i32).contains(&row) || col < 0 {
        return '.';
    }
    MAP[ROWS - 1 - row as usize]
        .chars()
        .nth(col as usize)
        .unwrap_or('.')
}

fn is_floor(col: i32, row: i32) -> bool {
    glyph(col, row) != '.'
}

/// The middle of a cell on the floor.
fn centre(col: i32, row: i32) -> (f32, f32) {
    (
        (col - ORIGIN_COL) as f32 * CELL,
        -(row as f32) * CELL - CELL * 0.5,
    )
}

/// A rectangle of floor, as `x0 <= x <= x1`, `z0 <= z <= z1` with `z0` the more northern.
#[derive(Clone, Copy)]
struct Rect {
    x0: f32,
    x1: f32,
    z0: f32,
    z1: f32,
}

impl Rect {
    /// The rectangle covering columns `c0..=c1` and rows `r0..=r1`.
    fn cells(c0: i32, c1: i32, r0: i32, r1: i32) -> Self {
        let (west, _) = centre(c0, r0);
        let (east, _) = centre(c1, r0);
        let (_, south) = centre(c0, r0);
        let (_, north) = centre(c0, r1);
        Self {
            x0: west - CELL * 0.5,
            x1: east + CELL * 0.5,
            z0: north - CELL * 0.5,
            z1: south + CELL * 0.5,
        }
    }
}

const HALL: fn() -> Rect = || Rect::cells(9, 11, 0, 2);
const FORECOURT: fn() -> Rect = || Rect::cells(8, 12, 4, 6);
const WEST_YARD: fn() -> Rect = || Rect::cells(0, 3, 10, 15);
const EAST_YARD: fn() -> Rect = || Rect::cells(17, 20, 10, 15);
const CASTING_HALL: fn() -> Rect = || Rect::cells(8, 12, 19, 21);
const FOUNDRY_FLOOR: fn() -> Rect = || Rect::cells(7, 13, 23, 28);

/// Where the boss gate stands: across the antechamber's north end.
const GATE_Z: f32 = -184.0;

// ------------------------------------------------------------------------------------------
// The builder
// ------------------------------------------------------------------------------------------

struct Builder {
    scene: EnvironmentScene,
}

/// The colour of slag: a deep orange glow.
const SLAG: Vec3 = Vec3::new(1.0, 0.38, 0.10);

impl Builder {
    fn new() -> Self {
        let mut scene = EnvironmentScene::with_capacity("bone_foundry", 1300);
        scene.half_extent = 260.0;
        // Indoors and hot: a dull red haze through the roof vents, a brown-red fill, and the
        // braziers and slag channels doing the real lighting.
        scene.lighting = EnvironmentLighting {
            sun_direction: Vec3::new(0.30, -0.74, -0.55),
            sun_color: Vec3::new(0.92, 0.58, 0.40),
            sun_intensity: 0.8,
            ambient_color: Vec3::new(0.50, 0.34, 0.28),
            ambient_intensity: 0.46,
            sky_zenith: Vec3::new(0.10, 0.04, 0.03),
            sky_horizon: Vec3::new(0.30, 0.14, 0.08),
        };
        scene.spawns = SpawnPoints {
            player: Vec3::new(0.0, 0.0, -8.0),
            featured_dummy: Vec3::new(6.0, 0.0, -16.0),
            group_center: Vec3::new(6.0, 0.0, -16.0),
            group_spacing: 3.0,
        };
        scene.push_spawn(SpawnPoint::new(Vec3::new(0.0, 0.0, -8.0)));
        Self { scene }
    }

    fn place(&mut self, asset: EnvironmentAsset, x: f32, z: f32, yaw: f32, scale: f32) {
        let mut object = EnvironmentObject::decoration(asset, Vec3::new(x, 0.0, z)).with_yaw(yaw);
        object.transform.scale = Vec3::splat(scale);
        object.collision = default_collision(asset);
        self.scene.push(object);
    }

    /// Placed without any collision, whatever the asset usually blocks.
    fn place_soft(&mut self, asset: EnvironmentAsset, x: f32, z: f32, yaw: f32, scale: f32) {
        let mut object = EnvironmentObject::decoration(asset, Vec3::new(x, 0.0, z)).with_yaw(yaw);
        object.transform.scale = Vec3::splat(scale);
        object.collision = EnvironmentCollision::None;
        self.scene.push(object);
    }

    /// A pillar scaled in height only, so a broken one is a low obstacle.
    fn pillar(&mut self, x: f32, z: f32, height: f32) {
        let mut object = EnvironmentObject::decoration(PILLAR, Vec3::new(x, 0.0, z));
        object.transform.scale = Vec3::new(1.0, height, 1.0);
        object.collision = default_collision(PILLAR);
        self.scene.push(object);
    }

    /// A brazier and the light it throws.
    fn brazier(&mut self, x: f32, z: f32, shadows: bool) {
        self.place_soft(BRAZIER, x, z, 0.0, 1.0);
        let mut light = SceneLight::brazier(Vec3::new(x, 1.45, z), 15.0);
        light.intensity = 4.2;
        if shadows {
            light = light.casting_shadows(2);
        }
        self.scene.push_light(light);
    }

    /// The glow of a slag channel: a low orange light, and a brazier's flame to be its source.
    fn slag(&mut self, x: f32, z: f32, range: f32) {
        self.place_soft(BRAZIER, x, z, 0.0, 1.4);
        let light = SceneLight::brazier(Vec3::new(x, 0.9, z), range).with_color(SLAG, 4.6);
        self.scene.push_light(light);
    }

    /// A line of slag glow from `(x0, z0)` to `(x1, z1)`, one source every `every` metres.
    fn channel(&mut self, from: (f32, f32), to: (f32, f32), every: f32, range: f32) {
        let length = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        let steps = (length / every).floor().max(1.0) as u32;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            self.slag(
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
                range,
            );
        }
    }

    /// Rails: soft iron fences along a line, for the conveyor bridges.
    fn rail(&mut self, from: (f32, f32), to: (f32, f32), yaw: f32) {
        let length = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        let steps = (length / 4.0).floor().max(1.0) as u32;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            self.place_soft(
                IRON_FENCE,
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
                yaw,
                1.0,
            );
        }
    }

    /// Bones (or anything) scattered by the hash, `count` of them inside `rect`.
    fn scatter(&mut self, asset: EnvironmentAsset, rect: Rect, count: u32, margin: f32, seed: u32) {
        for index in 0..count {
            let u = hash01(seed, index * 2 + 1);
            let v = hash01(seed, index * 2 + 2);
            let x = rect.x0 + margin + (rect.x1 - rect.x0 - 2.0 * margin) * u;
            let z = rect.z0 + margin + (rect.z1 - rect.z0 - 2.0 * margin) * v;
            let yaw = hash01(seed + 7, index) * std::f32::consts::TAU;
            let scale = 0.9 + 0.3 * hash01(seed + 13, index);
            self.place_soft(asset, x, z, yaw, scale);
        }
    }

    // ---------------------------------------------------------------------- the shell

    fn shell(&mut self) {
        use std::f32::consts::FRAC_PI_2;
        self.scene
            .push(EnvironmentObject::decoration(SKY_DOME, Vec3::ZERO));
        let mut ground = EnvironmentObject::decoration(GROUND_PLAIN, Vec3::new(0.0, 0.0, -116.0));
        ground.transform.scale = Vec3::new(640.0, 1.0, 640.0);
        ground.collision = EnvironmentCollision::None;
        self.scene.push(ground);

        // Paving under every cell, then the walls on every floor edge.
        let mut vertices: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
        for row in 0..ROWS as i32 {
            for col in 0..COLS as i32 {
                if !is_floor(col, row) {
                    continue;
                }
                let (x, z) = centre(col, row);
                self.place_soft(CRYPT_FLOOR, x, z, 0.0, 1.0);
                // North, south, east, west.
                for (dc, dr) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
                    if is_floor(col + dc, row + dr) {
                        continue;
                    }
                    let (wx, wz, yaw) = match (dc, dr) {
                        (0, 1) => (x, z - CELL * 0.5, 0.0),
                        (0, -1) => (x, z + CELL * 0.5, 0.0),
                        (1, 0) => (x + CELL * 0.5, z, FRAC_PI_2),
                        _ => (x - CELL * 0.5, z, FRAC_PI_2),
                    };
                    let mut wall =
                        EnvironmentObject::decoration(CRYPT_WALL, Vec3::new(wx, 0.0, wz))
                            .with_yaw(yaw);
                    wall.collision = default_collision(CRYPT_WALL);
                    self.scene.push(wall);
                    let along = if dc == 0 { (1, 0) } else { (0, 1) };
                    for sign in [-1, 1] {
                        let vx = wx + sign as f32 * along.0 as f32 * CELL * 0.5;
                        let vz = wz + sign as f32 * along.1 as f32 * CELL * 0.5;
                        vertices
                            .entry(((vx * 2.0).round() as i32, (vz * 2.0).round() as i32))
                            .or_default()
                            .push((-sign * along.0, -sign * along.1));
                    }
                }
            }
        }

        // A pier at every corner and jamb, and at every second joint of a straight run.
        let mut keys: Vec<_> = vertices.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let runs = &vertices[&key];
            let straight = runs.len() == 2 && runs[0].0 == -runs[1].0 && runs[0].1 == -runs[1].1;
            let (x, z) = (key.0 as f32 * 0.5, key.1 as f32 * 0.5);
            let every_other = ((x / CELL).round() as i32 + (z / CELL).round() as i32) % 2 == 0;
            if straight && !every_other {
                continue;
            }
            self.place(WALL_PIER, x, z, 0.0, 1.0);
        }
    }

    // ---------------------------------------------------------------------- the rooms

    fn entrance_hall(&mut self) {
        let hall = HALL();
        self.brazier(-10.5, -2.2, false);
        self.brazier(10.5, -2.2, false);
        self.brazier(-10.5, -22.0, true);
        self.brazier(10.5, -22.0, true);
        for z in [-6.0, -14.0] {
            self.place(SUPPLY_CRATE, -10.0, z, 0.3, 1.0);
            self.place(SUPPLY_CRATE, 10.0, z - 2.0, 1.1, 1.0);
        }
        self.scatter(BONE_PILE, hall, 4, 2.0, 11);
        self.place_soft(BANNER, -8.0, -0.6, 0.0, 1.0);
        self.place_soft(BANNER, 8.0, -0.6, 0.0, 1.0);
        // The corridor to the forecourt.
        self.brazier(-3.2, -27.0, false);
        self.brazier(3.2, -30.0, false);
    }

    /// The forecourt: scrap sorted into heaps, a slag channel down each side.
    fn forecourt(&mut self) {
        let yard = FORECOURT();
        self.channel((-17.0, -34.0), (-17.0, -54.0), 5.0, 9.0);
        self.channel((17.0, -34.0), (17.0, -54.0), 5.0, 9.0);
        for (x, z) in [(-14.0, -40.0), (14.0, -40.0), (-14.0, -52.0), (14.0, -52.0)] {
            self.pillar(x, z, 1.0);
        }
        self.scatter(ROCK_SMALL, yard, 6, 2.0, 21);
        self.scatter(BONE_PILE, yard, 8, 3.0, 22);
        self.place(SUPPLY_CRATE, -9.0, -37.0, 0.5, 1.0);
        self.place(SUPPLY_CRATE, 9.0, -37.0, 2.0, 1.0);
        self.brazier(0.0, -34.5, true);
        // The corridor to the ring.
        self.brazier(-3.2, -58.0, false);
        self.brazier(3.2, -61.0, false);
    }

    /// The ring: slag channels along the furnace's foot, pillars on the outer wall, and the
    /// furnace itself in the middle.
    fn ring(&mut self) {
        // The furnace island: its four faces glow, and the great furnace sits inside.
        self.place_soft(MAUSOLEUM, 0.0, -104.0, 0.0, 1.8);
        self.channel((-26.0, -82.0), (26.0, -82.0), 6.0, 10.0);
        self.channel((-26.0, -126.0), (26.0, -126.0), 6.0, 10.0);
        self.channel((-26.0, -82.0), (-26.0, -126.0), 6.0, 10.0);
        self.channel((26.0, -82.0), (26.0, -126.0), 6.0, 10.0);
        // Outer wall: a pillar every 16 m on each arm.
        for z in [-92.0, -104.0, -116.0] {
            self.pillar(-43.0, z, 1.0);
            self.pillar(43.0, z, 1.0);
        }
        for x in [-36.0, -18.0, 18.0, 36.0] {
            self.pillar(x, -66.0, 1.0);
            self.pillar(x, -142.0, 1.0);
        }
        // Conveyor bridges across the arms: rails, soft so they never pin a body.
        self.rail((-44.0, -72.0), (-28.0, -72.0), 0.0);
        self.rail((28.0, -72.0), (44.0, -72.0), 0.0);
        self.brazier(-42.0, -68.0, true);
        self.brazier(42.0, -68.0, true);
        self.brazier(-42.0, -140.0, false);
        self.brazier(42.0, -140.0, false);
        self.scatter(ROCK_SMALL, Rect::cells(5, 15, 8, 9), 6, 2.0, 31);
        self.scatter(BONE_PILE, Rect::cells(5, 15, 16, 17), 6, 2.0, 32);
        self.scatter(BONE_PILE, Rect::cells(5, 6, 10, 15), 4, 2.0, 33);
        self.scatter(ROCK_SMALL, Rect::cells(14, 15, 10, 15), 4, 2.0, 34);
        // The corridor to the casting hall.
        self.brazier(-3.2, -146.0, false);
        self.brazier(3.2, -150.0, false);
    }

    /// The bone-sorting yard: heaps of bones, coffins on racks.
    fn west_yard(&mut self) {
        let yard = WEST_YARD();
        self.scatter(BONE_PILE, yard, 22, 3.0, 41);
        for z in [-88.0, -118.0] {
            self.place(SARCOPHAGUS, -80.0, z, std::f32::consts::FRAC_PI_2, 1.0);
        }
        self.place(SUPPLY_CRATE, -56.0, -86.0, 0.4, 1.0);
        self.place(SUPPLY_CRATE, -56.0, -122.0, 2.1, 1.0);
        self.pillar(-60.0, -84.0, 0.5);
        self.pillar(-60.0, -124.0, 0.5);
        self.brazier(-82.0, -82.0, true);
        self.brazier(-82.0, -126.0, false);
        self.slag(-58.0, -104.0, 9.0);
        self.place_soft(BANNER, -83.0, -104.0, std::f32::consts::FRAC_PI_2, 1.0);
    }

    /// The slag works: casting moulds, spires of cooled slag, braziers.
    fn east_yard(&mut self) {
        let yard = EAST_YARD();
        self.channel((58.0, -84.0), (58.0, -124.0), 5.0, 9.0);
        self.scatter(ROCK_SPIRE, yard, 6, 3.0, 51);
        self.scatter(ROCK_MEDIUM, yard, 4, 3.0, 52);
        for (x, z) in [(64.0, -86.0), (78.0, -86.0), (64.0, -122.0), (78.0, -122.0)] {
            self.place(SUPPLY_CRATE, x, z, hash01(53, (x + z) as u32) * 3.0, 1.0);
        }
        self.pillar(70.0, -96.0, 0.5);
        self.pillar(70.0, -112.0, 0.5);
        self.brazier(82.0, -82.0, true);
        self.brazier(82.0, -126.0, false);
    }

    fn casting_hall(&mut self) {
        let hall = CASTING_HALL();
        for (x, z) in [(-14.0, -156.0), (14.0, -156.0), (-14.0, -172.0), (14.0, -172.0)] {
            self.pillar(x, z, 1.0);
        }
        self.channel((-18.0, -154.0), (-18.0, -174.0), 5.0, 9.0);
        self.channel((18.0, -154.0), (18.0, -174.0), 5.0, 9.0);
        self.brazier(0.0, -153.0, true);
        self.scatter(BONE_PILE, hall, 6, 3.0, 61);
        self.place(SUPPLY_CRATE, -8.0, -174.0, 0.2, 1.0);
        self.place(SUPPLY_CRATE, 8.0, -174.0, 1.7, 1.0);
        // The antechamber before the gate.
        self.brazier(-3.2, -179.0, true);
        self.brazier(3.2, -179.0, true);
        self.place_soft(BANNER, -3.4, -182.4, 0.0, 1.0);
        self.place_soft(BANNER, 3.4, -182.4, 0.0, 1.0);
    }

    /// The foundry floor: the boss arena. Open in the middle for the soak and the lane, with
    /// pillars at the edges for cover and the great furnace behind the Overseer.
    fn foundry_floor(&mut self) {
        let floor = FOUNDRY_FLOOR();
        for (x, z) in [
            (-22.0, -196.0),
            (22.0, -196.0),
            (-22.0, -220.0),
            (22.0, -220.0),
        ] {
            self.pillar(x, z, 1.0);
        }
        self.place(MAUSOLEUM, 0.0, -229.4, 0.0, 1.4);
        // The conveyor bridge: rails down both sides of the middle, soft so they never catch.
        self.rail((-10.0, -190.0), (-10.0, -226.0), std::f32::consts::FRAC_PI_2);
        self.rail((10.0, -190.0), (10.0, -226.0), std::f32::consts::FRAC_PI_2);
        self.channel((-26.0, -188.0), (-26.0, -228.0), 5.0, 10.0);
        self.channel((26.0, -188.0), (26.0, -228.0), 5.0, 10.0);
        for (x, z) in [(-26.0, -186.0), (26.0, -186.0), (-26.0, -230.0), (26.0, -230.0)] {
            self.brazier(x, z, true);
        }
        self.scatter(ROCK_SMALL, floor, 8, 4.0, 71);
        self.scatter(BONE_PILE, floor, 8, 4.0, 72);
        self.place_soft(BANNER, -6.0, -231.0, 0.0, 1.0);
        self.place_soft(BANNER, 6.0, -231.0, 0.0, 1.0);
    }
}

// ------------------------------------------------------------------------------------------
// The monsters
// ------------------------------------------------------------------------------------------

/// One pack: its name, and who stands where. Everyone faces the way they came from (south)
/// unless the pack walks.
struct Pack {
    name: &'static str,
    members: &'static [(&'static str, f32, f32)],
    /// Waypoints `[x, z]` the pack's only member walks, if it patrols.
    patrol: &'static [[f32; 2]],
}

const PACKS: [Pack; 9] = [
    Pack {
        name: "SCRAP FORECOURT",
        members: &[
            ("skeleton_warrior", -6.0, -44.0),
            ("skeleton_warrior", 6.0, -44.0),
            ("cinder_bomber", -10.0, -51.0),
            ("cinder_bomber", 10.0, -51.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "SLAG HAULER",
        members: &[("slag_ironclad", -12.0, -72.0)],
        patrol: &[[-24.0, -72.0], [24.0, -72.0]],
    },
    Pack {
        name: "CRUCIBLE GANG",
        members: &[
            ("slag_ironclad", -36.0, -96.0),
            ("skeleton_warrior", -32.0, -104.0),
            ("slag_thrower", -38.0, -112.0),
            ("slag_thrower", -34.0, -118.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "LADLE CREW",
        members: &[
            ("grave_brute", 36.0, -96.0),
            ("cinder_bomber", 32.0, -104.0),
            ("slag_thrower", 38.0, -112.0),
            ("slag_thrower", 34.0, -118.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "BONE-SORTING YARD",
        members: &[
            ("foundry_foreman", -68.0, -100.0),
            ("crypt_ghoul", -76.0, -92.0),
            ("crypt_ghoul", -76.0, -108.0),
            ("skeleton_bonecaster", -72.0, -118.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "SLAG WORKS",
        members: &[
            ("slag_ironclad", 62.0, -100.0),
            ("cinder_bomber", 68.0, -92.0),
            ("cinder_bomber", 72.0, -100.0),
            ("cinder_bomber", 68.0, -108.0),
            ("slag_thrower", 76.0, -96.0),
            ("slag_thrower", 76.0, -112.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "FOREMAN ON ROUNDS",
        members: &[("foundry_foreman", -16.0, -136.0)],
        patrol: &[[-24.0, -136.0], [24.0, -136.0]],
    },
    Pack {
        name: "FURNACE CHOIR",
        members: &[
            ("slag_ironclad", 0.0, -160.0),
            ("skeleton_warrior", -8.0, -158.0),
            ("foundry_foreman", 0.0, -170.0),
            ("slag_thrower", -12.0, -168.0),
            ("slag_thrower", 12.0, -168.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "THE FOUNDRY OVERSEER",
        members: &[("foundry_overseer", 0.0, -208.0)],
        patrol: &[],
    },
];

/// The packs of each yard, for the completion rule: either yard is optional, not both.
const WEST: [&str; 1] = ["BONE-SORTING YARD"];
const EAST: [&str; 1] = ["SLAG WORKS"];

fn completion_of(pack: &Pack) -> f32 {
    pack.members
        .iter()
        .map(|(id, _, _)| {
            EnemyKind::from_id(id)
                .unwrap_or_else(|| panic!("{id} is not a monster"))
                .archetype()
                .completion
        })
        .sum()
}

fn yard_total(names: &[&str]) -> f32 {
    PACKS
        .iter()
        .filter(|pack| names.contains(&pack.name))
        .map(completion_of)
        .sum()
}

/// What counts as a hundred per cent: everything the boss does not guard, less the larger yard.
fn required_completion() -> f32 {
    let total: f32 = PACKS[..PACKS.len() - 1].iter().map(completion_of).sum();
    total - yard_total(&WEST).max(yard_total(&EAST))
}

fn exported() -> EnvironmentScene {
    let mut builder = Builder::new();
    builder.shell();
    builder.entrance_hall();
    builder.forecourt();
    builder.ring();
    builder.west_yard();
    builder.east_yard();
    builder.casting_hall();
    builder.foundry_floor();
    let mut scene = builder.scene;

    for pack in &PACKS {
        for (id, x, z) in pack.members {
            let mut placement = MonsterPlacement::new(*id, Vec3::new(*x, 0.0, *z))
                .with_yaw(std::f32::consts::PI)
                .in_pack(pack.name);
            if !pack.patrol.is_empty() {
                placement.patrol = pack.patrol.to_vec();
                placement.yaw = std::f32::consts::FRAC_PI_2;
            }
            scene.push_monster(placement);
        }
    }

    let mut dungeon = DungeonSettings::new("BONE FOUNDRY", required_completion());
    dungeon.boss_pack = Some("THE FOUNDRY OVERSEER".to_owned());
    dungeon.boss_gate = Some(BossGate {
        center: Vec3::new(0.0, 0.0, GATE_Z),
        half_extents: Vec3::new(4.0, 3.0, 0.6),
        yaw: 0.0,
    });
    // The bombers and throwers he calls arrive from the two north corners of the floor.
    dungeon.add_spawns = vec![
        SpawnPoint::new(Vec3::new(-22.0, 0.0, -226.0)).with_yaw(std::f32::consts::PI),
        SpawnPoint::new(Vec3::new(22.0, 0.0, -226.0)).with_yaw(std::f32::consts::PI),
        SpawnPoint::new(Vec3::new(0.0, 0.0, -222.0)).with_yaw(std::f32::consts::PI),
    ];
    dungeon.add_count = 0;
    scene.dungeon = Some(dungeon);
    scene
}

#[test]
#[ignore = "rewrites assets/scenes/bone_foundry.json"]
fn write_the_level_file() {
    engine_environment::scene_file::save(&exported(), path()).expect("the level file is written");
}

// ------------------------------------------------------------------------------------------
// The generator keeps its own promises
// ------------------------------------------------------------------------------------------

#[test]
fn the_checked_in_file_matches_the_generator() {
    let file = engine_environment::scene_file::load(path())
        .expect("run the ignored test to write the level file");
    let made = exported();
    assert_eq!(file.objects.len(), made.objects.len());
    assert_eq!(file.monsters.len(), made.monsters.len());
    assert_eq!(file.lights.len(), made.lights.len());
    assert_eq!(file.water.len(), made.water.len());
    assert_eq!(file.dungeon, made.dungeon);
}

#[test]
fn the_drawing_is_rectangular_and_the_rooms_are_where_the_rects_say() {
    for (index, line) in MAP.iter().enumerate() {
        let width = line.chars().count();
        assert_eq!(width, COLS, "row {} is {width} wide", ROWS - 1 - index);
    }
    for (rect, glyph_wanted, cols, rows) in [
        (HALL(), 'E', (9, 11), (0, 2)),
        (FORECOURT(), 'F', (8, 12), (4, 6)),
        (CASTING_HALL(), 'H', (8, 12), (19, 21)),
        (FOUNDRY_FLOOR(), 'B', (7, 13), (23, 28)),
        (WEST_YARD(), 'W', (0, 3), (10, 15)),
        (EAST_YARD(), 'X', (17, 20), (10, 15)),
    ] {
        for col in cols.0..=cols.1 {
            for row in rows.0..=rows.1 {
                assert_eq!(glyph(col, row), glyph_wanted, "({col}, {row})");
            }
        }
        assert!(rect.x0 < rect.x1 && rect.z0 < rect.z1);
    }
    // The ring is a ring: the furnace in its middle is solid, and every arm is floor.
    for col in 7..=13 {
        for row in 10..=15 {
            assert!(!is_floor(col, row), "the furnace island is solid ({col}, {row})");
        }
    }
    for col in 5..=15 {
        for row in [8, 9, 16, 17] {
            assert!(is_floor(col, row), "({col}, {row})");
        }
    }
    let hall = HALL();
    assert!(hall.x0 < 0.0 && hall.x1 > 0.0 && hall.z1 > -8.0);
    assert!((FOUNDRY_FLOOR().z1 - GATE_Z).abs() < 1e-3);
}

fn cell_of(x: f32, z: f32) -> (i32, i32) {
    (
        ((x + CELL * 0.5) / CELL).floor() as i32 + ORIGIN_COL,
        (-z / CELL).floor() as i32,
    )
}

#[test]
fn every_pack_stands_on_floor_and_out_of_reach_of_every_other() {
    let made = exported();
    let aggro = engine_game::enemy::AGGRO_RADIUS;
    for placement in &made.monsters {
        let (col, row) = cell_of(placement.position.x, placement.position.z);
        assert!(
            is_floor(col, row),
            "{} at {:?} is in a wall (cell {col}, {row})",
            placement.monster,
            placement.position
        );
    }
    for pack in &PACKS {
        for waypoint in pack.patrol {
            let (col, row) = cell_of(waypoint[0], waypoint[1]);
            assert!(is_floor(col, row), "{} walks into a wall", pack.name);
        }
    }
    for placement in &made.monsters {
        let spawn = Vec3::new(0.0, 0.0, -8.0);
        let distance = (placement.position - spawn).length();
        assert!(
            distance > aggro + 3.0,
            "{} is {distance} m from the spawn",
            placement.monster
        );
    }
    for (i, a) in PACKS.iter().enumerate() {
        for b in PACKS.iter().skip(i + 1) {
            let points = |pack: &Pack| -> Vec<(f32, f32)> {
                pack.members
                    .iter()
                    .map(|(_, x, z)| (*x, *z))
                    .chain(pack.patrol.iter().map(|p| (p[0], p[1])))
                    .collect()
            };
            let near = points(a).iter().any(|p| {
                points(b)
                    .iter()
                    .any(|q| ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt() < aggro + 2.0)
            });
            assert!(
                !near,
                "{} and {} are within a pull of each other",
                a.name, b.name
            );
        }
    }
}

#[test]
fn either_yard_is_enough_and_neither_is_not() {
    let total: f32 = PACKS[..PACKS.len() - 1].iter().map(completion_of).sum();
    let required = required_completion();
    let (west, east) = (yard_total(&WEST), yard_total(&EAST));
    assert!(total - west >= required, "skipping the west yard falls short");
    assert!(total - east >= required, "skipping the east yard falls short");
    assert!(total - west - east < required, "skipping both is enough");
    assert!(required > 0.0 && required < total);
}

#[test]
fn the_roster_has_two_patrols_and_a_boss_behind_the_gate() {
    let patrols = PACKS.iter().filter(|pack| !pack.patrol.is_empty()).count();
    assert_eq!(patrols, 2);
    assert_eq!(PACKS.len(), 9, "eight packs and the boss");
    let boss = PACKS.last().expect("a boss pack");
    let z = boss.members[0].2;
    assert!(z < GATE_Z, "the boss is behind the gate");
    for pack in &PACKS[..PACKS.len() - 1] {
        for (_, _, member_z) in pack.members {
            assert!(*member_z > GATE_Z, "{} is behind the gate", pack.name);
        }
    }
}
