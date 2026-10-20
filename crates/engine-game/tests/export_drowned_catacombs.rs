//! The Drowned Catacombs' level generator: a room graph in, a plain level file out.
//!
//! About a thousand placements are not a thing to type by hand, and a layout that is a picture
//! can be checked by eye. So the dungeon is drawn once, as text, below ([`MAP`]): one character
//! per 8 m cell, rooms and corridors by letter. This turns that drawing into walls (one
//! `crypt_wall` on every edge between floor and not-floor), piers at the joints, paving under
//! every cell, the props each room is dressed with, its lights and its water, then adds the
//! monsters and the dungeon block. The output is an ordinary [`EnvironmentScene`]: the editor
//! opens it, edits it and saves it, and from then on the file is the source of truth.
//!
//! Run on demand, never in CI, because it rewrites a checked-in asset:
//! `cargo test -p engine-game --test export_drowned_catacombs -- --ignored`
//!
//! The other tests are ordinary: they keep the checked-in file honest against this generator
//! (so a hand edit shows up as a diff somebody chose, not an accident) and hold the layout to
//! its own rules.
//!
//! ```text
//!   r22 ┌───────────────┐
//!       │  A  CHAPEL    │  the Abbess            (cols 5..11, rows 17..22)
//!   r17 └──────┬────────┘
//!   r16        a            boss gate z = -136
//!   r15 ┌──────┴────────┐
//!       │  O  OSSUARY   │  two packs             (rows 13..15)
//!   r13 └─c──────────c──┘  west / east approach
//!   r12   c            c
//!   r11 ┌───┐          ┌───┐
//!       │ W │  H hub   │ W │  wings: cloister (casters) / battery (archers)
//!   r7  └───┘g┌─────┐g └───┘  g: galleries, each walked by a lurker
//!             │  H  │
//!   r4 .. N nave, r3 corridor, r0..r2 E entrance hall
//! ```

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use std::collections::HashMap;

use engine_core::math::{Vec2, Vec3};
use engine_environment::asset::{
    BANNER, BONE_PILE, BRAZIER, CRYPT_FLOOR, CRYPT_WALL, EnvironmentAsset, GROUND_PLAIN, MAUSOLEUM,
    PILLAR, ROCK_MEDIUM, ROCK_SMALL, ROCK_SPIRE, SARCOPHAGUS, SKY_DOME, SUPPLY_CRATE, WALL_PIER,
};
use engine_environment::object::default_collision;
use engine_environment::{
    BossGate, DungeonSettings, EnvironmentCollision, EnvironmentLighting, EnvironmentObject,
    EnvironmentScene, MonsterPlacement, SceneLight, SpawnPoint, SpawnPoints, WaterBody, hash01,
};
use engine_game::enemy::EnemyKind;

const FILE: &str = "scenes/drowned_catacombs.json";

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
const ORIGIN_COL: i32 = 8;

/// How many rows the drawing has, north (top line) to south (bottom line).
const ROWS: usize = 23;

/// The level, one character per cell. `.` is solid; every other character is floor.
///
/// ```text
/// E entrance hall   N flooded nave   H the sluice (hub)   g gallery   W wing
/// c corridor        O ossuary        a antechamber        A chapel (boss arena)
/// ```
const MAP: [&str; ROWS] = [
    ".....AAAAAAA.....", // 22
    ".....AAAAAAA.....", // 21
    ".....AAAAAAA.....", // 20
    ".....AAAAAAA.....", // 19
    ".....AAAAAAA.....", // 18
    ".....AAAAAAA.....", // 17
    "........a........", // 16
    ".....OOOOOOO.....", // 15
    ".ccccOOOOOOOcccc.", // 14
    ".c...OOOOOOO...c.", // 13
    ".c.............c.", // 12
    "WWW...........WWW", // 11
    "WWW..HHHHHHH..WWW", // 10
    "WWWggHHHHHHHggWWW", // 9
    "WWW..HHHHHHH..WWW", // 8
    "WWW.....c.....WWW", // 7
    "......NNNNN......", // 6
    "......NNNNN......", // 5
    "......NNNNN......", // 4
    "........c........", // 3
    ".......EEE.......", // 2
    ".......EEE.......", // 1
    ".......EEE.......", // 0
];

/// The character of the cell at `(col, row)`, row zero being the southernmost.
fn glyph(col: i32, row: i32) -> char {
    if !(0..ROWS as i32).contains(&row) || col < 0 {
        return '.';
    }
    let line: String = MAP[ROWS - 1 - row as usize]
        .chars()
        .filter(|c| *c != ' ')
        .collect();
    line.chars().nth(col as usize).unwrap_or('.')
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

    fn mid_x(&self) -> f32 {
        (self.x0 + self.x1) * 0.5
    }

    fn mid_z(&self) -> f32 {
        (self.z0 + self.z1) * 0.5
    }
}

const HALL: fn() -> Rect = || Rect::cells(7, 9, 0, 2);
const NAVE: fn() -> Rect = || Rect::cells(6, 10, 4, 6);
const HUB: fn() -> Rect = || Rect::cells(5, 11, 8, 10);
const WEST_WING: fn() -> Rect = || Rect::cells(0, 2, 7, 11);
const EAST_WING: fn() -> Rect = || Rect::cells(14, 16, 7, 11);
const OSSUARY: fn() -> Rect = || Rect::cells(5, 11, 13, 15);
const CHAPEL: fn() -> Rect = || Rect::cells(5, 11, 17, 22);

/// Where the boss gate stands: across the antechamber's north end.
const GATE_Z: f32 = -136.0;

// ------------------------------------------------------------------------------------------
// The builder
// ------------------------------------------------------------------------------------------

struct Builder {
    scene: EnvironmentScene,
    lights: usize,
}

impl Builder {
    fn new() -> Self {
        let mut scene = EnvironmentScene::with_capacity("drowned_catacombs", 1100);
        scene.half_extent = 160.0;
        // Underground and wet: a cold moon through the roof, a green-blue fill, and the warm
        // braziers (and a few cold lamps) doing the real lighting.
        scene.lighting = EnvironmentLighting {
            sun_direction: Vec3::new(0.30, -0.74, -0.55),
            sun_color: Vec3::new(0.56, 0.74, 0.86),
            sun_intensity: 0.9,
            ambient_color: Vec3::new(0.30, 0.44, 0.56),
            ambient_intensity: 0.46,
            sky_zenith: Vec3::new(0.03, 0.07, 0.12),
            sky_horizon: Vec3::new(0.14, 0.24, 0.28),
        };
        scene.spawns = SpawnPoints {
            player: Vec3::new(0.0, 0.0, -8.0),
            featured_dummy: Vec3::new(6.0, 0.0, -16.0),
            group_center: Vec3::new(6.0, 0.0, -16.0),
            group_spacing: 3.0,
        };
        scene.push_spawn(SpawnPoint::new(Vec3::new(0.0, 0.0, -8.0)));
        Self { scene, lights: 0 }
    }

    fn place(&mut self, asset: EnvironmentAsset, x: f32, z: f32, yaw: f32, scale: f32) {
        let mut object =
            EnvironmentObject::decoration(asset, Vec3::new(x, 0.0, z)).with_yaw(yaw);
        object.transform.scale = Vec3::splat(scale);
        object.collision = default_collision(asset);
        self.scene.push(object);
    }

    /// Placed without any collision, whatever the asset usually blocks.
    fn place_soft(&mut self, asset: EnvironmentAsset, x: f32, z: f32, yaw: f32, scale: f32) {
        let mut object =
            EnvironmentObject::decoration(asset, Vec3::new(x, 0.0, z)).with_yaw(yaw);
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
        self.lights += 1;
    }

    /// A cold lantern: sea-green light with no flame under it.
    fn lamp(&mut self, x: f32, y: f32, z: f32, range: f32) {
        let light =
            SceneLight::brazier(Vec3::new(x, y, z), range).with_color(Vec3::new(0.30, 0.78, 0.74), 3.4);
        self.scene.push_light(light);
        self.lights += 1;
    }

    fn water(&mut self, rect: Rect, surface: f32, depth: f32) {
        self.scene.push_water(WaterBody::new(
            Vec3::new(rect.mid_x(), surface, rect.mid_z()),
            Vec2::new((rect.x1 - rect.x0) * 0.5, (rect.z1 - rect.z0) * 0.5),
            depth,
        ));
    }

    /// Bones scattered by the hash, `count` of them inside `rect`, keeping `margin` off the walls.
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
        let mut ground = EnvironmentObject::decoration(GROUND_PLAIN, Vec3::new(0.0, 0.0, -90.0));
        ground.transform.scale = Vec3::new(640.0, 1.0, 640.0);
        ground.collision = EnvironmentCollision::None;
        self.scene.push(ground);

        // Paving under every cell, then the walls on every floor edge.
        let mut vertices: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
        for row in 0..ROWS as i32 {
            for col in 0..17 {
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
                    let mut wall = EnvironmentObject::decoration(CRYPT_WALL, Vec3::new(wx, 0.0, wz))
                        .with_yaw(yaw);
                    wall.collision = default_collision(CRYPT_WALL);
                    self.scene.push(wall);
                    // The two ends of the wall, keyed in half-metres, with the direction the
                    // wall runs away from each.
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
            let straight = runs.len() == 2
                && runs[0].0 == -runs[1].0
                && runs[0].1 == -runs[1].1;
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
        self.brazier(-6.5, -22.8, true);
        self.brazier(6.5, -22.8, true);
        self.place(SARCOPHAGUS, -10.4, -12.0, std::f32::consts::FRAC_PI_2, 1.0);
        self.place(SARCOPHAGUS, 10.4, -12.0, std::f32::consts::FRAC_PI_2, 1.0);
        self.scatter(BONE_PILE, hall, 5, 2.0, 11);
        self.place(ROCK_MEDIUM, -9.8, -20.6, 0.6, 1.0);
        self.place(ROCK_SMALL, 9.6, -20.2, 2.0, 1.2);
        // Seawater standing in the low corners.
        self.water(
            Rect {
                x0: -11.5,
                x1: -5.0,
                z0: -21.0,
                z1: -15.5,
            },
            0.10,
            0.5,
        );
        self.water(
            Rect {
                x0: 5.0,
                x1: 11.5,
                z0: -8.0,
                z1: -2.5,
            },
            0.10,
            0.5,
        );
        // The corridor to the nave.
        self.brazier(-3.2, -27.0, false);
        self.brazier(3.2, -31.0, false);
    }

    fn nave(&mut self) {
        let nave = NAVE();
        // Two colonnades, kept off the middle so the corridor's line is open.
        for z in [-38.0, -46.0, -54.0] {
            self.pillar(-14.0, z, 1.0);
            self.pillar(14.0, z, 1.0);
        }
        self.brazier(-18.4, -33.6, true);
        self.brazier(18.4, -33.6, true);
        self.brazier(-18.4, -54.4, false);
        self.brazier(18.4, -54.4, false);
        // The flood: a channel down each side, with stones to cross by.
        self.water(
            Rect {
                x0: -20.0,
                x1: -16.5,
                z0: -56.0,
                z1: -32.0,
            },
            0.14,
            0.8,
        );
        self.water(
            Rect {
                x0: 16.5,
                x1: 20.0,
                z0: -56.0,
                z1: -32.0,
            },
            0.14,
            0.8,
        );
        self.scatter(ROCK_SMALL, nave, 7, 1.5, 21);
        self.scatter(BONE_PILE, nave, 4, 3.0, 22);
        self.place(SARCOPHAGUS, -19.0, -44.0, std::f32::consts::FRAC_PI_2, 1.0);
        self.place(SARCOPHAGUS, 19.0, -50.0, std::f32::consts::FRAC_PI_2, 1.0);
        // The corridor to the sluice.
        self.brazier(3.2, -59.0, false);
        self.lamp(-3.0, 3.0, -62.0, 12.0);
    }

    fn sluice(&mut self) {
        let hub = HUB();
        // The channel: waist-deep across the middle, running on into both galleries.
        self.water(
            Rect {
                x0: -44.0,
                x1: 44.0,
                z0: -80.0,
                z1: -72.0,
            },
            0.24,
            1.0,
        );
        // The rest of the hall is flooded ankle-deep.
        self.water(
            Rect {
                x0: -28.0,
                x1: 28.0,
                z0: -88.0,
                z1: -64.0,
            },
            0.08,
            0.5,
        );
        // Stepping stones across the channel, and boulders that are cover.
        for (index, x) in [-22.0, -14.0, -6.0, 2.0, 10.0, 18.0, 25.0].into_iter().enumerate() {
            let z = -76.0 + if index % 2 == 0 { 1.2 } else { -1.2 };
            self.place_soft(ROCK_SMALL, x, z, hash01(31, index as u32) * 6.0, 1.3);
        }
        for (x, z) in [(-17.0, -67.5), (17.0, -67.5), (-17.0, -84.5), (17.0, -84.5)] {
            self.place(ROCK_MEDIUM, x, z, hash01(32, (x + z) as u32) * 6.0, 1.3);
        }
        // Pillars standing in the water at the four corners of the hall.
        for (x, z) in [(-22.0, -68.0), (22.0, -68.0), (-22.0, -84.0), (22.0, -84.0)] {
            self.pillar(x, z, 1.0);
        }
        self.brazier(-26.0, -65.6, true);
        self.brazier(26.0, -65.6, true);
        self.brazier(-26.0, -86.4, false);
        self.brazier(26.0, -86.4, false);
        self.lamp(0.0, 3.0, -76.0, 22.0);
        self.scatter(BONE_PILE, hub, 4, 3.0, 33);
        // The corridors out of the hall's two ends: galleries.
        for side in [-1.0_f32, 1.0] {
            self.place_soft(ROCK_SPIRE, side * 36.0, -83.0, hash01(34, side as u32 + 1) * 6.0, 0.9);
            self.brazier(side * 36.0, -70.2, false);
            self.scatter(
                BONE_PILE,
                Rect {
                    x0: side.min(0.0) * 44.0 + side.max(0.0) * 28.0,
                    x1: side.min(0.0) * 28.0 + side.max(0.0) * 44.0,
                    z0: -80.0,
                    z1: -72.0,
                },
                2,
                1.5,
                35,
            );
        }
    }

    /// The west wing, the Sunken Cloister: arcades and pools. Casters stand in them.
    fn west_wing(&mut self) {
        let wing = WEST_WING();
        for z in [-62.0, -70.0] {
            self.pillar(-65.0, z, 1.0);
            self.pillar(-47.0, z, 1.0);
        }
        self.pillar(-65.0, -93.0, 1.0);
        self.pillar(-47.0, -93.0, 1.0);
        self.water(
            Rect {
                x0: -66.0,
                x1: -46.0,
                z0: -95.0,
                z1: -84.0,
            },
            0.20,
            0.9,
        );
        self.place(SARCOPHAGUS, -66.4, -78.0, 0.0, 1.0);
        self.place(SARCOPHAGUS, -66.4, -86.0, 0.0, 1.0);
        self.scatter(ROCK_SMALL, wing, 6, 2.0, 41);
        self.scatter(BONE_PILE, wing, 4, 3.0, 42);
        self.brazier(-66.2, -58.0, true);
        self.brazier(-46.0, -58.0, false);
        self.lamp(-56.0, 3.2, -90.0, 18.0);
        // The way north: a corridor of three cells.
        self.brazier(-53.0, -99.0, false);
        self.brazier(-59.0, -108.0, false);
    }

    /// The east wing, the Bilge Cisterns: cover and long sight lines. Archers stand back.
    fn east_wing(&mut self) {
        let wing = EAST_WING();
        // Broken columns: knee-high cover, the half-height of a pillar.
        for (x, z) in [(50.0, -84.0), (62.0, -84.0), (56.0, -72.0), (50.0, -62.0), (62.0, -62.0)] {
            self.pillar(x, z, 0.5);
        }
        self.pillar(47.0, -93.0, 1.0);
        self.pillar(65.0, -93.0, 1.0);
        self.water(
            Rect {
                x0: 46.0,
                x1: 66.0,
                z0: -95.0,
                z1: -84.0,
            },
            0.20,
            0.9,
        );
        for (x, z) in [(66.0, -66.0), (66.0, -74.0), (46.4, -90.0)] {
            self.place(SUPPLY_CRATE, x, z, hash01(51, (x + z) as u32) * 3.0, 1.0);
        }
        self.scatter(ROCK_SMALL, wing, 6, 2.0, 52);
        self.scatter(BONE_PILE, wing, 3, 3.0, 53);
        self.brazier(66.2, -58.0, true);
        self.brazier(46.0, -58.0, false);
        self.lamp(56.0, 3.2, -90.0, 18.0);
        self.brazier(53.0, -99.0, false);
        self.brazier(59.0, -108.0, false);
    }

    fn approaches(&mut self) {
        // The corridors from the wings round to the ossuary's two doors.
        for side in [-1.0_f32, 1.0] {
            self.brazier(side * 38.0, -114.0, false);
            self.scatter(
                BONE_PILE,
                Rect {
                    x0: side.min(0.0) * 52.0 + side.max(0.0) * 28.0,
                    x1: side.min(0.0) * 28.0 + side.max(0.0) * 52.0,
                    z0: -120.0,
                    z1: -112.0,
                },
                2,
                1.5,
                61,
            );
            self.place_soft(ROCK_SPIRE, side * 56.0, -117.0, hash01(62, side as u32 + 1) * 6.0, 0.8);
        }
    }

    fn ossuary(&mut self) {
        let hall = OSSUARY();
        // Rows of stone coffins, and the bones of whoever came out of them.
        for x in [-20.0, -8.0, 8.0, 20.0] {
            self.place(SARCOPHAGUS, x, -105.8, 0.0, 1.0);
            self.place(SARCOPHAGUS, x, -126.2, 0.0, 1.0);
        }
        for (x, z) in [(-22.0, -111.0), (22.0, -111.0), (-22.0, -122.0), (22.0, -122.0)] {
            self.pillar(x, z, 1.0);
        }
        self.scatter(BONE_PILE, hall, 14, 3.0, 71);
        self.brazier(-26.0, -105.6, true);
        self.brazier(26.0, -105.6, true);
        self.brazier(-26.0, -126.4, false);
        self.brazier(26.0, -126.4, false);
        self.lamp(0.0, 3.2, -116.0, 22.0);
        self.water(
            Rect {
                x0: -6.0,
                x1: 6.0,
                z0: -124.0,
                z1: -108.0,
            },
            0.07,
            0.4,
        );
        // The antechamber before the gate.
        self.brazier(-3.2, -131.0, true);
        self.brazier(3.2, -131.0, true);
        self.place_soft(BANNER, -3.4, -134.4, 0.0, 1.0);
        self.place_soft(BANNER, 3.4, -134.4, 0.0, 1.0);
    }

    fn chapel(&mut self) {
        let chapel = CHAPEL();
        // The nave of the drowned chapel: four pillars close enough to hold a player against
        // the Undertow, and far enough that the Abbess's slam does not reach them.
        for (x, z) in [(-14.0, -146.0), (14.0, -146.0), (-14.0, -174.0), (14.0, -174.0)] {
            self.pillar(x, z, 1.0);
        }
        self.pillar(-23.0, -160.0, 1.0);
        self.pillar(23.0, -160.0, 1.0);
        // The altar end.
        self.place(MAUSOLEUM, 0.0, -179.4, 0.0, 1.0);
        for x in [-24.0, -16.0, 16.0, 24.0] {
            self.place(SARCOPHAGUS, x, -182.4, 0.0, 1.0);
        }
        for z in [-142.0, -152.0, -168.0, -178.0] {
            self.place(SARCOPHAGUS, -26.4, z, std::f32::consts::FRAC_PI_2, 1.0);
            self.place(SARCOPHAGUS, 26.4, z, std::f32::consts::FRAC_PI_2, 1.0);
        }
        // The chapel is flooded to the ankle; the Undertow's pools are the deep part.
        self.water(
            Rect {
                x0: -26.0,
                x1: 26.0,
                z0: -182.0,
                z1: -138.0,
            },
            0.12,
            0.8,
        );
        for (x, z) in [(-26.0, -138.8), (26.0, -138.8), (-26.0, -181.2), (26.0, -181.2)] {
            self.brazier(x, z, true);
        }
        self.brazier(-8.0, -178.0, false);
        self.brazier(8.0, -178.0, false);
        self.lamp(0.0, 4.0, -160.0, 34.0);
        self.lamp(0.0, 4.0, -176.0, 20.0);
        self.scatter(ROCK_SMALL, chapel, 10, 4.0, 81);
        self.scatter(BONE_PILE, chapel, 8, 4.0, 82);
        self.place_soft(BANNER, -6.0, -183.0, 0.0, 1.0);
        self.place_soft(BANNER, 6.0, -183.0, 0.0, 1.0);
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
        name: "FLOODED PICKETS",
        members: &[
            ("skeleton_warrior", -8.0, -46.0),
            ("crypt_ghoul", 8.0, -46.0),
            ("drowned_tidecaller", 0.0, -53.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "SLUICE KEEPERS",
        members: &[
            ("grave_brute", 0.0, -77.0),
            ("drowned_cultist", -9.0, -84.0),
            ("drowned_cultist", 9.0, -84.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "WEST GALLERY LURKER",
        members: &[("catacomb_lurker", -38.0, -76.0)],
        patrol: &[[-44.0, -76.0], [-29.0, -76.0]],
    },
    Pack {
        name: "SUNKEN CLOISTER",
        members: &[
            ("skeleton_warrior", -57.0, -84.0),
            ("drowned_cultist", -62.0, -88.0),
            ("drowned_cultist", -50.0, -88.0),
            ("drowned_tidecaller", -56.0, -92.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "EAST GALLERY LURKER",
        members: &[("catacomb_lurker", 38.0, -76.0)],
        patrol: &[[44.0, -76.0], [29.0, -76.0]],
    },
    Pack {
        name: "BILGE BATTERY",
        members: &[
            ("grave_brute", 56.0, -84.0),
            ("bilge_archer", 50.0, -91.0),
            ("bilge_archer", 56.0, -92.5),
            ("bilge_archer", 62.0, -91.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "OSSUARY WATCH",
        members: &[
            ("skeleton_warrior", -14.0, -118.0),
            ("skeleton_warrior", -8.0, -121.0),
            ("crypt_ghoul", -4.0, -117.0),
            ("drowned_tidecaller", -13.0, -125.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "BONE CHAPTER",
        members: &[
            ("grave_brute", 14.0, -116.0),
            ("bilge_archer", 10.0, -108.0),
            ("bilge_archer", 18.0, -108.0),
            ("drowned_cultist", 14.0, -107.0),
        ],
        patrol: &[],
    },
    Pack {
        name: "THE DROWNED ABBESS",
        members: &[("drowned_abbess", 0.0, -160.0)],
        patrol: &[],
    },
];

/// The packs of each wing, for the completion rule: either wing is optional, not both.
const WEST: [&str; 2] = ["WEST GALLERY LURKER", "SUNKEN CLOISTER"];
const EAST: [&str; 2] = ["EAST GALLERY LURKER", "BILGE BATTERY"];

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

fn wing_total(names: &[&str]) -> f32 {
    PACKS
        .iter()
        .filter(|pack| names.contains(&pack.name))
        .map(completion_of)
        .sum()
}

/// What counts as a hundred per cent: everything the boss does not guard, less the larger wing.
fn required_completion() -> f32 {
    let total: f32 = PACKS[..PACKS.len() - 1].iter().map(completion_of).sum();
    total - wing_total(&WEST).max(wing_total(&EAST))
}

fn exported() -> EnvironmentScene {
    let mut builder = Builder::new();
    builder.shell();
    builder.entrance_hall();
    builder.nave();
    builder.sluice();
    builder.west_wing();
    builder.east_wing();
    builder.approaches();
    builder.ossuary();
    builder.chapel();
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

    let mut dungeon = DungeonSettings::new("DROWNED CATACOMBS", required_completion());
    dungeon.boss_pack = Some("THE DROWNED ABBESS".to_owned());
    dungeon.boss_gate = Some(BossGate {
        center: Vec3::new(0.0, 0.0, GATE_Z),
        half_extents: Vec3::new(4.0, 3.0, 0.6),
        yaw: 0.0,
    });
    // The healers she calls at half health arrive on either side of the altar.
    dungeon.add_spawns = vec![
        SpawnPoint::new(Vec3::new(-14.0, 0.0, -168.0)).with_yaw(std::f32::consts::PI),
        SpawnPoint::new(Vec3::new(14.0, 0.0, -168.0)).with_yaw(std::f32::consts::PI),
    ];
    dungeon.add_count = 0;
    scene.dungeon = Some(dungeon);
    scene
}

#[test]
#[ignore = "rewrites assets/scenes/drowned_catacombs.json"]
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
        let width = line.chars().filter(|c| *c != ' ').count();
        assert_eq!(width, 17, "row {} is {width} wide", ROWS - 1 - index);
    }
    // Every rect the dressing uses is entirely the room it names.
    for (rect, glyph_wanted, cols, rows) in [
        (HALL(), 'E', (7, 9), (0, 2)),
        (NAVE(), 'N', (6, 10), (4, 6)),
        (HUB(), 'H', (5, 11), (8, 10)),
        (OSSUARY(), 'O', (5, 11), (13, 15)),
        (CHAPEL(), 'A', (5, 11), (17, 22)),
    ] {
        for col in cols.0..=cols.1 {
            for row in rows.0..=rows.1 {
                assert_eq!(glyph(col, row), glyph_wanted, "({col}, {row})");
            }
        }
        assert!(rect.x0 < rect.x1 && rect.z0 < rect.z1);
    }
    // The spawn is in the hall, and the chapel is the room behind the gate.
    let hall = HALL();
    assert!(hall.x0 < 0.0 && hall.x1 > 0.0 && hall.z1 > -8.0);
    assert!((CHAPEL().z1 - GATE_Z).abs() < 1e-3);
}

#[test]
fn every_pack_stands_on_floor_and_out_of_reach_of_every_other() {
    let made = exported();
    let aggro = engine_game::enemy::AGGRO_RADIUS;
    for placement in &made.monsters {
        // x = 0 is the middle of a cell, so a cell spans its centre +- 4.
        let col = ((placement.position.x + CELL * 0.5) / CELL).floor() as i32 + ORIGIN_COL;
        let row = (-placement.position.z / CELL).floor() as i32;
        assert!(
            is_floor(col, row),
            "{} at {:?} is in a wall (cell {col}, {row})",
            placement.monster,
            placement.position
        );
    }
    // The spawn is out of every pack's reach.
    for placement in &made.monsters {
        let spawn = Vec3::new(0.0, 0.0, -8.0);
        let distance = (placement.position - spawn).length();
        assert!(distance > aggro + 3.0, "{} is {distance} m from the spawn", placement.monster);
    }
    // Two packs are two packs: no member of one within a pull of a member of another, the
    // lurkers' walks included.
    for (i, a) in PACKS.iter().enumerate() {
        for b in PACKS.iter().skip(i + 1) {
            let points = |pack: &Pack| -> Vec<(f32, f32)> {
                pack.members
                    .iter()
                    .map(|(_, x, z)| (*x, *z))
                    .chain(pack.patrol.iter().map(|p| (p[0], p[1])))
                    .collect()
            };
            // The Abbess is behind a gate; the lurkers deliberately reach into the hub.
            let near = points(a).iter().any(|p| {
                points(b)
                    .iter()
                    .any(|q| ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt() < aggro + 2.0)
            });
            assert!(!near, "{} and {} are within a pull of each other", a.name, b.name);
        }
    }
}

#[test]
fn either_wing_is_enough_and_neither_is_not() {
    let total: f32 = PACKS[..PACKS.len() - 1].iter().map(completion_of).sum();
    let required = required_completion();
    let (west, east) = (wing_total(&WEST), wing_total(&EAST));
    assert!(total - west >= required, "skipping the west falls short");
    assert!(total - east >= required, "skipping the east falls short");
    assert!(total - west - east < required, "skipping both is enough");
    assert!(required > 0.0 && required < total);
}
