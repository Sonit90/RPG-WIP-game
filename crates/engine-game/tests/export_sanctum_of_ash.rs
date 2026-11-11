//! The Sanctum of Ash's level generator: a room graph in, a plain level file out.
//!
//! The same tool as the other production dungeons' generators, drawn as a fifth shape: a **hub
//! with three shrines**. The nave leads to the cinder rotunda; the rotunda has a shrine off its
//! west door, one off its east door, and the road north runs *through* the third. Nothing is
//! optional: all three shrines, the censer warden in the gallery behind the third, and the
//! packs before them are one hundred per cent, and the pontiff's gate stays shut until they are
//! done. The dungeon is drawn once, as text, below ([`MAP`]): one character per 8 m cell. This
//! turns that drawing into walls, piers, paving, props, lights, packs and the dungeon block.
//! The output is an ordinary [`EnvironmentScene`]: the editor opens it, edits it and saves it,
//! and from then on the file is the source of truth.
//!
//! Run on demand, never in CI, because it rewrites a checked-in asset:
//! `cargo test -p engine-game --test export_sanctum_of_ash -- --ignored`
//!
//! The other tests are ordinary: they keep the checked-in file honest against this generator
//! and hold the layout to its own rules.
//!
//! ```text
//!   r25..31  B  SANCTUM FLOOR    the Ashen Pontiff (boss, behind the gate at z = -200)
//!   r23..24  a  antechamber
//!   r22      c  corridor
//!   r19..21  G  GALLERY OF CENSERS   the Censer Warden (mid-dungeon guardian) and two cantors
//!   r18      c  corridor
//!   r14..17  S  SHRINE OF THE HUNT   the road north runs through it
//!   r13      c  corridor
//!   r8..12   H  CINDER ROTUNDA   the hub; an immolator walks its rounds
//!            W  SHRINE OF MIRRORS (west door)      X  SHRINE OF EMBERS (east door)
//!   r7       c  corridor
//!   r4..6    N  ASHEN NAVE
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
    ARCH, BANNER, BONE_PILE, BRAZIER, CRYPT_FLOOR, CRYPT_WALL, EnvironmentAsset, GROUND_PLAIN,
    PILLAR, ROCK_SMALL, ROCK_SPIRE, SARCOPHAGUS, SKY_DOME, SUPPLY_CRATE, WALL_PIER,
};
use engine_environment::object::default_collision;
use engine_environment::{
    BossGate, DungeonSettings, EnvironmentCollision, EnvironmentLighting, EnvironmentObject,
    EnvironmentScene, MonsterPlacement, SceneLight, SpawnPoint, SpawnPoints, hash01,
};
use engine_game::enemy::EnemyKind;

const FILE: &str = "scenes/sanctum_of_ash.json";

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
const ORIGIN_COL: i32 = 12;

/// How many rows the drawing has, north (top line) to south (bottom line).
const ROWS: usize = 32;

/// How many columns every row has.
const COLS: usize = 25;

/// The level, one character per cell. `.` is solid; every other character is floor.
///
/// ```text
/// E entrance hall   N nave   H rotunda (hub)   W shrine of mirrors   X shrine of embers
/// S shrine of the hunt   G gallery   a antechamber   c corridor   B sanctum floor (boss arena)
/// ```
const MAP: [&str; ROWS] = [
    "........BBBBBBBBB........", // 31
    "........BBBBBBBBB........", // 30
    "........BBBBBBBBB........", // 29
    "........BBBBBBBBB........", // 28
    "........BBBBBBBBB........", // 27
    "........BBBBBBBBB........", // 26
    "........BBBBBBBBB........", // 25
    "............a............", // 24
    "............a............", // 23
    "............c............", // 22
    "..........GGGGG..........", // 21
    "..........GGGGG..........", // 20
    "..........GGGGG..........", // 19
    "............c............", // 18
    "..........SSSSS..........", // 17
    "..........SSSSS..........", // 16
    "..........SSSSS..........", // 15
    "..........SSSSS..........", // 14
    "............c............", // 13
    "WWWWW....HHHHHHH....XXXXX", // 12
    "WWWWW....HHHHHHH....XXXXX", // 11
    "WWWWWccccHHHHHHHccccXXXXX", // 10
    "WWWWW....HHHHHHH....XXXXX", // 9
    "WWWWW....HHHHHHH....XXXXX", // 8
    "............c............", // 7
    "..........NNNNN..........", // 6
    "..........NNNNN..........", // 5
    "..........NNNNN..........", // 4
    "............c............", // 3
    "...........EEE...........", // 2
    "...........EEE...........", // 1
    "...........EEE...........", // 0
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

const HALL: fn() -> Rect = || Rect::cells(11, 13, 0, 2);
const NAVE: fn() -> Rect = || Rect::cells(10, 14, 4, 6);
const ROTUNDA: fn() -> Rect = || Rect::cells(9, 15, 8, 12);
const WEST_SHRINE: fn() -> Rect = || Rect::cells(0, 4, 8, 12);
const EAST_SHRINE: fn() -> Rect = || Rect::cells(20, 24, 8, 12);
const HUNT_SHRINE: fn() -> Rect = || Rect::cells(10, 14, 14, 17);
const GALLERY: fn() -> Rect = || Rect::cells(10, 14, 19, 21);
const SANCTUM_FLOOR: fn() -> Rect = || Rect::cells(8, 16, 25, 31);

/// Where the boss gate stands: across the antechamber's north end.
const GATE_Z: f32 = -200.0;

/// The great pillars of the sanctum floor: where to stand when the rite is cast. Each is a
/// column three metres across, so a party can shelter behind one; the floor has six.
const COVER: [(f32, f32); 6] = [
    (-14.0, -216.0),
    (14.0, -216.0),
    (-24.0, -228.0),
    (24.0, -228.0),
    (-14.0, -242.0),
    (14.0, -242.0),
];

/// How much wider than an ordinary pillar a great pillar is.
const GREAT: f32 = 2.4;

// ------------------------------------------------------------------------------------------
// The builder
// ------------------------------------------------------------------------------------------

struct Builder {
    scene: EnvironmentScene,
}

/// The colour of a censer's coals: a bright, pale orange.
const COALS: Vec3 = Vec3::new(1.0, 0.52, 0.20);

impl Builder {
    fn new() -> Self {
        let mut scene = EnvironmentScene::with_capacity("sanctum_of_ash", 1500);
        scene.half_extent = 280.0;
        // Indoors and smouldering: a pale grey-orange shaft through the broken roof, a cold
        // ash-grey fill, and the censers and braziers doing the real lighting.
        scene.lighting = EnvironmentLighting {
            sun_direction: Vec3::new(-0.34, -0.72, -0.60),
            sun_color: Vec3::new(0.96, 0.70, 0.52),
            sun_intensity: 0.75,
            ambient_color: Vec3::new(0.44, 0.40, 0.42),
            ambient_intensity: 0.44,
            sky_zenith: Vec3::new(0.07, 0.05, 0.05),
            sky_horizon: Vec3::new(0.26, 0.16, 0.12),
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

    /// A great pillar: [`GREAT`] times as wide, full height, and as solid to sight as to feet.
    fn great_pillar(&mut self, x: f32, z: f32) {
        let mut object = EnvironmentObject::decoration(PILLAR, Vec3::new(x, 0.0, z));
        object.transform.scale = Vec3::new(GREAT, 1.0, GREAT);
        object.collision = default_collision(PILLAR);
        self.scene.push(object);
    }

    /// A brazier and the light it throws.
    fn brazier(&mut self, x: f32, z: f32, shadows: bool) {
        self.place_soft(BRAZIER, x, z, 0.0, 1.0);
        let mut light = SceneLight::brazier(Vec3::new(x, 1.45, z), 15.0);
        light.intensity = 4.0;
        if shadows {
            light = light.casting_shadows(2);
        }
        self.scene.push_light(light);
    }

    /// A censer: a big brazier on a plinth of rock, burning coals instead of a flame.
    fn censer(&mut self, x: f32, z: f32, range: f32) {
        self.place_soft(BRAZIER, x, z, 0.0, 1.5);
        let light = SceneLight::brazier(Vec3::new(x, 1.2, z), range).with_color(COALS, 4.4);
        self.scene.push_light(light);
    }

    /// A line of censers from `(x0, z0)` to `(x1, z1)`, one every `every` metres.
    fn aisle(&mut self, from: (f32, f32), to: (f32, f32), every: f32, range: f32) {
        let length = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        let steps = (length / every).floor().max(1.0) as u32;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            self.censer(
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
                range,
            );
        }
    }

    /// Anything scattered by the hash, `count` of them inside `rect`.
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
        let mut ground = EnvironmentObject::decoration(GROUND_PLAIN, Vec3::new(0.0, 0.0, -128.0));
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
        self.scatter(BONE_PILE, hall, 3, 2.0, 11);
        self.place_soft(BANNER, -8.0, -0.6, 0.0, 1.0);
        self.place_soft(BANNER, 8.0, -0.6, 0.0, 1.0);
        // The doorway to the nave.
        self.place_soft(ARCH, 0.0, -24.0, 0.0, 1.0);
        self.brazier(-3.2, -27.0, false);
        self.brazier(3.2, -30.0, false);
    }

    /// The nave: two rows of pillars down its length, a censer between each pair.
    fn nave(&mut self) {
        let nave = NAVE();
        for z in [-34.0, -44.0, -54.0] {
            self.pillar(-14.0, z, 1.0);
            self.pillar(14.0, z, 1.0);
        }
        self.aisle((-8.0, -36.0), (-8.0, -52.0), 8.0, 9.0);
        self.aisle((8.0, -36.0), (8.0, -52.0), 8.0, 9.0);
        self.scatter(ROCK_SMALL, nave, 4, 3.0, 21);
        self.scatter(BONE_PILE, nave, 6, 3.0, 22);
        self.brazier(0.0, -33.0, true);
        self.place_soft(ARCH, 0.0, -56.0, 0.0, 1.0);
        self.brazier(-3.2, -58.0, false);
        self.brazier(3.2, -61.0, false);
    }

    /// The rotunda: a ring of pillars round a great censer, a doorway on each of three sides.
    fn rotunda(&mut self) {
        let hub = ROTUNDA();
        // The great censer: a pale light on a plinth in the middle of the floor.
        self.censer(0.0, -84.0, 18.0);
        self.place(SARCOPHAGUS, 0.0, -84.0, std::f32::consts::FRAC_PI_2, 1.0);
        for (x, z) in [(-20.0, -72.0), (20.0, -72.0), (-20.0, -96.0), (20.0, -96.0)] {
            self.pillar(x, z, 1.0);
        }
        self.scatter(ROCK_SMALL, hub, 6, 3.0, 31);
        self.scatter(BONE_PILE, hub, 6, 3.0, 32);
        self.brazier(-24.0, -66.0, true);
        self.brazier(24.0, -66.0, true);
        self.brazier(-24.0, -102.0, false);
        self.brazier(24.0, -102.0, false);
        // The three doors: west, east, north.
        self.place_soft(ARCH, -28.0, -84.0, std::f32::consts::FRAC_PI_2, 1.0);
        self.place_soft(ARCH, 28.0, -84.0, std::f32::consts::FRAC_PI_2, 1.0);
        self.place_soft(ARCH, 0.0, -104.0, 0.0, 1.0);
        self.brazier(-3.2, -107.0, false);
        self.brazier(3.2, -110.0, false);
    }

    /// The shrine of mirrors: polished plinths, tall pillars, nothing to hide behind.
    fn west_shrine(&mut self) {
        let shrine = WEST_SHRINE();
        for z in [-72.0, -96.0] {
            self.pillar(-68.0, z, 1.0);
        }
        self.pillar(-92.0, -66.0, 0.5);
        self.pillar(-92.0, -102.0, 0.5);
        self.place(SARCOPHAGUS, -96.0, -84.0, std::f32::consts::FRAC_PI_2, 1.0);
        self.aisle((-62.0, -68.0), (-62.0, -100.0), 8.0, 9.0);
        self.scatter(BONE_PILE, shrine, 8, 3.0, 41);
        self.brazier(-96.0, -66.0, true);
        self.brazier(-96.0, -102.0, false);
        self.place_soft(BANNER, -99.0, -84.0, std::f32::consts::FRAC_PI_2, 1.0);
    }

    /// The shrine of embers: braziers on every side and crates of fuel.
    fn east_shrine(&mut self) {
        let shrine = EAST_SHRINE();
        self.aisle((62.0, -68.0), (62.0, -100.0), 5.0, 10.0);
        for (x, z) in [(76.0, -70.0), (92.0, -70.0), (76.0, -98.0), (92.0, -98.0)] {
            self.place(SUPPLY_CRATE, x, z, hash01(53, (x + z) as u32) * 3.0, 1.0);
        }
        self.scatter(ROCK_SPIRE, shrine, 5, 3.0, 51);
        self.scatter(BONE_PILE, shrine, 6, 3.0, 52);
        self.pillar(70.0, -84.0, 0.5);
        self.brazier(96.0, -66.0, true);
        self.brazier(96.0, -102.0, false);
        self.place_soft(BANNER, 99.0, -84.0, -std::f32::consts::FRAC_PI_2, 1.0);
    }

    /// The shrine of the hunt: the road north. Narrow aisles between rows of pillars, so a
    /// stalker's leap has somewhere to land and somewhere to come from.
    fn hunt_shrine(&mut self) {
        let shrine = HUNT_SHRINE();
        for z in [-120.0, -132.0] {
            self.pillar(-14.0, z, 1.0);
            self.pillar(14.0, z, 1.0);
        }
        self.aisle((-18.0, -114.0), (-18.0, -142.0), 7.0, 9.0);
        self.aisle((18.0, -114.0), (18.0, -142.0), 7.0, 9.0);
        self.scatter(ROCK_SMALL, shrine, 5, 3.0, 61);
        self.scatter(BONE_PILE, shrine, 6, 3.0, 62);
        self.place_soft(ARCH, 0.0, -144.0, 0.0, 1.0);
        self.brazier(-3.2, -147.0, false);
        self.brazier(3.2, -150.0, false);
    }

    /// The gallery of censers: a long hall of them, the warden at its far end.
    fn gallery(&mut self) {
        let hall = GALLERY();
        self.aisle((-14.0, -154.0), (-14.0, -174.0), 5.0, 9.0);
        self.aisle((14.0, -154.0), (14.0, -174.0), 5.0, 9.0);
        for z in [-158.0, -170.0] {
            self.pillar(-8.0, z, 1.0);
            self.pillar(8.0, z, 1.0);
        }
        self.scatter(BONE_PILE, hall, 5, 3.0, 71);
        self.brazier(0.0, -153.0, true);
        // The antechamber before the gate.
        self.place_soft(ARCH, 0.0, -176.0, 0.0, 1.0);
        self.brazier(-3.2, -183.0, true);
        self.brazier(3.2, -183.0, true);
        self.place_soft(BANNER, -3.4, -198.4, 0.0, 1.0);
        self.place_soft(BANNER, 3.4, -198.4, 0.0, 1.0);
    }

    /// The sanctum floor: the boss arena. Open in the middle, six great pillars standing about
    /// it for the rite, and the pontiff's altar behind.
    fn sanctum_floor(&mut self) {
        let floor = SANCTUM_FLOOR();
        for (x, z) in COVER {
            self.great_pillar(x, z);
        }
        // Three plinths where the pylons rise.
        for (x, z) in [(-26.0, -248.0), (26.0, -248.0), (0.0, -252.0)] {
            self.place_soft(BRAZIER, x, z, 0.0, 1.8);
            let light = SceneLight::brazier(Vec3::new(x, 1.4, z), 11.0).with_color(COALS, 5.0);
            self.scene.push_light(light);
        }
        self.place(SARCOPHAGUS, 0.0, -254.0, 0.0, 1.6);
        self.aisle((-30.0, -204.0), (-30.0, -250.0), 7.0, 10.0);
        self.aisle((30.0, -204.0), (30.0, -250.0), 7.0, 10.0);
        for (x, z) in [
            (-30.0, -202.0),
            (30.0, -202.0),
            (-30.0, -254.0),
            (30.0, -254.0),
        ] {
            self.brazier(x, z, true);
        }
        self.scatter(ROCK_SMALL, floor, 6, 4.0, 81);
        self.scatter(BONE_PILE, floor, 8, 4.0, 82);
        self.place_soft(BANNER, -6.0, -255.0, 0.0, 1.0);
        self.place_soft(BANNER, 6.0, -255.0, 0.0, 1.0);
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

const PACKS: [Pack; 7] = [
    Pack {
        name: "ASHEN NAVE",
        members: &[
            ("mirror_penitent", 0.0, -46.0),
            ("skeleton_warrior", -8.0, -42.0),
            ("skeleton_warrior", 8.0, -42.0),
            ("ash_cantor", 0.0, -54.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "CINDER WATCH",
        members: &[("pyre_immolator", -16.0, -92.0)],
        patrol: &[[-18.0, -92.0], [18.0, -92.0]],
    },
    Pack {
        name: "SHRINE OF MIRRORS",
        members: &[
            ("mirror_penitent", -76.0, -80.0),
            ("mirror_penitent", -76.0, -92.0),
            ("ash_cantor", -88.0, -86.0),
            ("skeleton_bonecaster", -84.0, -72.0),
            ("skeleton_bonecaster", -84.0, -100.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "SHRINE OF EMBERS",
        members: &[
            ("pyre_immolator", 76.0, -80.0),
            ("pyre_immolator", 76.0, -92.0),
            ("ash_cantor", 88.0, -86.0),
            ("grave_brute", 84.0, -72.0),
            ("crypt_archer", 84.0, -100.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "SHRINE OF THE HUNT",
        members: &[
            ("ash_stalker", -10.0, -124.0),
            ("ash_stalker", 10.0, -124.0),
            ("ash_stalker", 0.0, -132.0),
            ("pyre_immolator", 0.0, -118.0),
            ("crypt_archer", -12.0, -138.0),
            ("crypt_archer", 12.0, -138.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "GALLERY OF CENSERS",
        members: &[
            ("censer_warden", 0.0, -166.0),
            ("ash_cantor", -12.0, -172.0),
            ("ash_cantor", 12.0, -172.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "THE ASHEN PONTIFF",
        members: &[("ashen_pontiff", 0.0, -236.0)],
        patrol: &[],
    },
];

/// The three shrines the party must clear: the completion rule names them.
const SHRINES: [&str; 3] = [
    "SHRINE OF MIRRORS",
    "SHRINE OF EMBERS",
    "SHRINE OF THE HUNT",
];

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

fn pack_named(name: &str) -> &'static Pack {
    PACKS
        .iter()
        .find(|pack| pack.name == name)
        .unwrap_or_else(|| panic!("no pack {name}"))
}

/// What counts as a hundred per cent: everything the boss does not guard. Nothing is optional.
fn required_completion() -> f32 {
    PACKS[..PACKS.len() - 1].iter().map(completion_of).sum()
}

fn exported() -> EnvironmentScene {
    let mut builder = Builder::new();
    builder.shell();
    builder.entrance_hall();
    builder.nave();
    builder.rotunda();
    builder.west_shrine();
    builder.east_shrine();
    builder.hunt_shrine();
    builder.gallery();
    builder.sanctum_floor();
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

    let mut dungeon = DungeonSettings::new("SANCTUM OF ASH", required_completion());
    dungeon.boss_pack = Some("THE ASHEN PONTIFF".to_owned());
    dungeon.boss_gate = Some(BossGate {
        center: Vec3::new(0.0, 0.0, GATE_Z),
        half_extents: Vec3::new(4.0, 3.0, 0.6),
        yaw: 0.0,
    });
    // The pylons and the immolators he calls rise from the back of the floor: a plinth in each
    // far corner and one under the altar.
    dungeon.add_spawns = vec![
        SpawnPoint::new(Vec3::new(-26.0, 0.0, -248.0)).with_yaw(0.0),
        SpawnPoint::new(Vec3::new(26.0, 0.0, -248.0)).with_yaw(0.0),
        SpawnPoint::new(Vec3::new(0.0, 0.0, -252.0)).with_yaw(0.0),
    ];
    dungeon.add_count = 0;
    scene.dungeon = Some(dungeon);
    scene
}

#[test]
#[ignore = "rewrites assets/scenes/sanctum_of_ash.json"]
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
        (HALL(), 'E', (11, 13), (0, 2)),
        (NAVE(), 'N', (10, 14), (4, 6)),
        (ROTUNDA(), 'H', (9, 15), (8, 12)),
        (WEST_SHRINE(), 'W', (0, 4), (8, 12)),
        (EAST_SHRINE(), 'X', (20, 24), (8, 12)),
        (HUNT_SHRINE(), 'S', (10, 14), (14, 17)),
        (GALLERY(), 'G', (10, 14), (19, 21)),
        (SANCTUM_FLOOR(), 'B', (8, 16), (25, 31)),
    ] {
        for col in cols.0..=cols.1 {
            for row in rows.0..=rows.1 {
                assert_eq!(glyph(col, row), glyph_wanted, "({col}, {row})");
            }
        }
        assert!(rect.x0 < rect.x1 && rect.z0 < rect.z1);
    }
    // Three doors off the rotunda, and the only way on runs north through the third shrine.
    assert!(is_floor(ORIGIN_COL - 4, 10) && is_floor(ORIGIN_COL + 4, 10));
    assert!(is_floor(ORIGIN_COL, 13) && is_floor(ORIGIN_COL, 18));
    // The shrines are dead ends: only their one door is open.
    for (cols, rows) in [((0, 4), (8, 12)), ((20, 24), (8, 12))] {
        for col in cols.0..=cols.1 {
            for row in [rows.0 - 1, rows.1 + 1] {
                assert!(!is_floor(col, row), "({col}, {row}) is a way out");
            }
        }
    }
    let hall = HALL();
    assert!(hall.x0 < 0.0 && hall.x1 > 0.0 && hall.z1 > -8.0);
    assert!((SANCTUM_FLOOR().z1 - GATE_Z).abs() < 1e-3);
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
fn nothing_is_optional_and_every_shrine_is_needed() {
    let total: f32 = PACKS[..PACKS.len() - 1].iter().map(completion_of).sum();
    let required = required_completion();
    assert!(
        (required - total).abs() < 1e-3,
        "everything before the gate is required"
    );
    for shrine in SHRINES {
        let value = completion_of(pack_named(shrine));
        assert!(value > 0.0, "{shrine}");
        assert!(total - value < required, "skipping {shrine} is enough");
    }
    // The warden is worth a pack of its own, and skipping it is not enough either.
    let warden = completion_of(pack_named("GALLERY OF CENSERS"));
    assert!(warden >= 25.0 && total - warden < required);
}

#[test]
fn the_roster_has_a_patrol_a_mid_dungeon_guardian_and_a_boss_behind_the_gate() {
    let patrols = PACKS.iter().filter(|pack| !pack.patrol.is_empty()).count();
    assert_eq!(patrols, 1);
    assert_eq!(PACKS.len(), 7, "six packs and the boss");
    let boss = PACKS.last().expect("a boss pack");
    assert!(boss.members[0].2 < GATE_Z, "the boss is behind the gate");
    for pack in &PACKS[..PACKS.len() - 1] {
        for (_, _, member_z) in pack.members {
            assert!(*member_z > GATE_Z, "{} is behind the gate", pack.name);
        }
    }
    // The guardian stands between the third shrine and the gate.
    let warden = pack_named("GALLERY OF CENSERS");
    let (id, _, z) = warden.members[0];
    assert_eq!(id, "censer_warden");
    let hunt_z = pack_named("SHRINE OF THE HUNT")
        .members
        .iter()
        .map(|(_, _, z)| *z)
        .fold(f32::MAX, f32::min);
    assert!(z < hunt_z && z > GATE_Z);
    // The boss floor has its six great pillars, all of them on it and none in the gate's way.
    let floor = SANCTUM_FLOOR();
    assert_eq!(COVER.len(), 6);
    for (x, z) in COVER {
        assert!(x > floor.x0 && x < floor.x1 && z > floor.z0 && z < floor.z1);
        assert!(x.abs() > 4.0 + GREAT, "({x}, {z}) blocks the way in");
    }
}
