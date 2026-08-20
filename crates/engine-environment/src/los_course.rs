//! The line-of-sight course: three domes, six dummies and two pits.
//!
//! ```text
//!            x = -18            x = 0             x = +18
//!
//!   z = -4     (((D)))          (((.)))           (((D)))     three domes, r = 7, h = 3.15
//!               3 2 1            empty             1 2 3      dummies on two of them
//!
//!   z = +8    [ P I T ]                          [ P I T ]    two 9 x 9 shafts, 3.5 m deep
//!
//!   z = +14                       (P)                         player spawn, facing -Z
//! ```
//!
//! # What the shape of it is for
//!
//! Every other test bed here answers a question about a system by giving it room to run.
//! This one answers a question about a *rule*: a targeted ability requires an unobstructed
//! path to its target, and the only way to know the rule is right is to stand in front of
//! something round and try to cast through it. So the geometry is arranged as a gradient
//! rather than as a scene:
//!
//! | Dummy | Where it stands | What should happen |
//! | --- | --- | --- |
//! | 1 | on the near slope, six-tenths of the way out | in sight, at any range on the bar |
//! | 2 | on the crest | in sight, with the ray grazing the surface below it |
//! | 3 | on the far slope, the same distance past the middle | blocked by the dome |
//!
//! The middle dome carries nobody. It is the control: a blocker with nothing behind it, so
//! that "the ray stopped" can be told apart from "the ray stopped at *that*" in the F3
//! readout, and so there is something to walk around and look past.
//!
//! A dome is a *hill*, not a hemisphere: gentle enough to walk up, so the same three
//! questions can be asked again from the crest of the one in the middle. See
//! [`crate::asset::dome_profile`] for why the curve is the shape it is.
//!
//! The two pits are the same question asked from below. A caster standing three and a half
//! metres down has its chest under the lip, and every dummy on the course is then behind the
//! floor it is standing on — which is the one case the ray has to get right for a reason
//! that has nothing to do with the domes.
//!
//! # Only the layout lives here
//!
//! This module names the domes, their radius, the tiles their collision surface is built
//! from, the pits and the six places a dummy stands. It spawns nothing: `engine_game::los_course`
//! reads all of it and builds the fight, exactly as `engine_game::arena` reads
//! [`crate::arena`]. The two facts that must not drift apart — where a dome is and where a
//! dummy stands on it — are one calculation here rather than two lists in two crates.

use engine_core::math::{Quat, Vec3};
use engine_core::transform::Transform;

use crate::asset::{DOME, GROUND_PLAIN, PILLAR, RIDGE, SKY_DOME, WALL_SEGMENT};
use crate::lighting::EnvironmentLighting;
use crate::object::{EnvironmentCollision, EnvironmentObject};
use crate::scene::{EnvironmentScene, SpawnPoints};

/// Half-width of the square the player is confined to, in metres.
pub const COURSE_HALF_EXTENT: f32 = 40.0;

/// Radius of each dome, in metres.
///
/// Seven, because the ray has to be stopped by the *bulk* of the dome rather than by its
/// lip: a dummy on the far slope of a two-metre mound is visible over the top of it, which
/// would make the third row of the table above pass for the wrong reason.
pub const DOME_RADIUS: f32 = 7.0;

/// How far from the middle of the course the outer two domes stand, in metres.
pub const DOME_SPACING: f32 = 18.0;

/// The z every dome is centred on.
pub const DOME_Z: f32 = -4.0;

/// How far out along the player's line each flanking dummy stands, as a fraction of the
/// radius.
///
/// Six-tenths puts dummy 1 well up the near slope and dummy 3 the same distance down the
/// far one, so the pair is symmetric about the crest and the only difference between them
/// is which side of the curve they are on.
pub const DUMMY_OFFSET: f32 = 0.6;

/// How deep a pit is, in metres.
///
/// Deeper than a character is tall, so a caster standing in one has its combat point below
/// the lip and cannot see over it. Shallower than a fall that would need a landing rule.
pub const PIT_DEPTH: f32 = 3.5;

/// Half-width of a pit, in metres.
pub const PIT_HALF: f32 = 4.5;

/// The z both pits are centred on.
pub const PIT_Z: f32 = 8.0;

/// How far either side of the middle a pit is centred, in metres.
pub const PIT_SPACING: f32 = 10.0;

/// The angle of the ramp out of a pit, in degrees.
///
/// Well under the character controller's walkable limit, because the ramp is how a player
/// gets *out* of the pit and a test course whose test can only be run once is not a test
/// course. Thirty degrees over 3.5 m of rise is a six-metre run, which fits inside the pit
/// with room to stand at the bottom of it.
pub const PIT_RAMP_DEGREES: f32 = 30.0;

/// Half-width of the ramp out of a pit, in metres.
pub const PIT_RAMP_HALF_WIDTH: f32 = 2.0;

/// Where the player starts.
pub const PLAYER_SPAWN: Vec3 = Vec3::new(0.0, 0.0, 14.0);

/// How far above the collision floor the drawn floor sits, in metres.
///
/// The floor is a slab whose top face is at y = 0 and the quad that draws it is a
/// centimetre above that: two coplanar surfaces are a depth-buffer tie, which reads as the
/// whole floor flickering.
const FLOOR_LIFT: f32 = 0.01;

/// One dome, and everything derived from where it stands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dome {
    /// What the collision debug view and the F3 readout call it.
    pub label: &'static str,
    /// The middle of its flat base.
    pub center: Vec3,
    /// Its radius, in metres.
    pub radius: f32,
    /// Whether dummies stand on it.
    pub occupied: bool,
}

/// How many rings a dome's collision surface is tiled with, crest to rim.
///
/// A dome is not one of the three shapes `engine-collision` has — a plane, a box and an
/// oriented box — and one test course is not a reason to grow the narrow phase. So the
/// surface is *tiled*: each step of the profile becomes a band of oriented boxes, each
/// tilted to the slope of the curve across that band, and a character walks up the tilted
/// faces exactly as it walks up the collision course's ramps.
///
/// ```text
///          ______                a band is a flat box tilted to the chord of the
///        /`      `\              profile it spans. Five of them follow the curve
///      /`          `\            to within a couple of centimetres, and the seam
///    _/              \_          between two is a step of under three.
/// ```
///
/// The alternative — a grid of upright boxes, each as tall as the surface above it — is what
/// this replaced. It blocked a ray correctly and was a staircase to walk on: at this radius
/// its steps were over a metre, which is more than twice the character's step height, so the
/// hill could be looked at and not climbed.
pub const DOME_RINGS: u32 = 5;

/// How long a tile is allowed to be around its ring, in metres.
///
/// A tile is a flat box and the ring it belongs to is a circle, so a tile is a chord: the
/// shorter it is, the rounder the ring. A metre and a half puts the middle of a chord within
/// two centimetres of the arc at the outer ring of a seven-metre dome.
pub const DOME_TILE_ARC: f32 = 1.5;

/// A box in the collision world, as the two corners a caller builds a shape from.
///
/// Plain numbers rather than an `engine_collision::Shape`, for the reason
/// [`crate::object::EnvironmentObject::world_collision`] returns plain numbers: this crate
/// does not depend on `engine-collision` and must not learn to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slab {
    /// The lower corner.
    pub min: Vec3,
    /// The upper corner.
    pub max: Vec3,
    /// What a contact report calls it.
    pub label: &'static str,
}

impl Slab {
    /// A slab between two corners.
    #[must_use]
    pub fn new(min: Vec3, max: Vec3, label: &'static str) -> Self {
        Self { min, max, label }
    }

    /// The middle of the slab.
    #[must_use]
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Its full size along each axis.
    #[must_use]
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }
}

/// One tilted box: a patch of a dome's surface.
///
/// The numbers an oriented box is built from, for the reason [`Slab`] is two corners: this
/// crate does not depend on `engine-collision`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    /// Which dome it belongs to, and what a contact report calls it.
    pub label: &'static str,
    /// The middle of the box.
    pub center: Vec3,
    /// Half its width across the ring, half its thickness, half its length down the slope.
    pub half_extents: Vec3,
    /// Which way round the dome it sits, radians about +Y. Its length runs outwards.
    pub yaw: f32,
    /// How far it is tilted about its own +X, in radians. Positive descends outwards.
    pub pitch: f32,
}

impl Tile {
    /// The normal of the face a character stands on.
    ///
    /// `+Y` carried round by the tile's own rotation — a pitch about `+X` and then a yaw
    /// about `+Y`, which is the order the rotation is built in.
    #[must_use]
    pub fn up(&self) -> Vec3 {
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(sin_yaw * sin_pitch, cos_pitch, cos_yaw * sin_pitch)
    }

    /// The angle of the face a character stands on, in degrees.
    #[must_use]
    pub fn slope_degrees(&self) -> f32 {
        self.pitch.abs().to_degrees()
    }
}

impl Dome {
    /// How high the crest stands above the floor, in metres.
    #[must_use]
    pub fn height(&self) -> f32 {
        self.radius * crate::asset::DOME_HEIGHT
    }

    /// How high the drawn surface is above `(x, z)`, or the floor outside the footprint.
    ///
    /// [`crate::asset::dome_profile`] scaled to this dome: the same curve the mesh is built
    /// from, so a dummy put down at this height stands on the hill the player can see.
    #[must_use]
    pub fn surface_height(&self, x: f32, z: f32) -> f32 {
        let distance = (x - self.center.x).hypot(z - self.center.z);
        self.center.y + self.radius * crate::asset::dome_profile(distance / self.radius)
    }

    /// The tilted boxes the collision world builds this dome's surface from.
    ///
    /// `DOME_RINGS` bands from the crest outwards. A band spans one step of the profile and
    /// is tilted to the chord between that step's two ends; each tile in it is then sunk by
    /// its own half-thickness along its own up, so its **top face** lands on the curve
    /// rather than its middle. That is the correction the collision course's ramps make, and
    /// it is what stops every band presenting half a tile's thickness as a lip at its lower
    /// edge — five lips on the way up a hill is five things to trip on.
    ///
    /// The tiles are thick, so each band reaches through the one below it and into the
    /// floor: what a character walks on is a continuous surface rather than a set of plates
    /// with gaps between them, and a ray meets something solid wherever it enters the hill.
    #[must_use]
    pub fn collision(&self) -> Vec<Tile> {
        /// How thick a tile is, in metres.
        ///
        /// A thin plate laid along a chord leaves a wedge of empty space under it, and a
        /// wedge inside a hill is somewhere a ray gets through and a foot falls into.
        const THICKNESS: f32 = 1.6;
        /// How much wider than its own chord a tile is made.
        ///
        /// The tiles of a ring are chords of a circle, so neighbours meet at an angle. A
        /// little overlap is what stops the seam between two of them being a crack.
        const OVERLAP: f32 = 1.12;

        let half_thickness = THICKNESS * 0.5;
        let mut tiles = Vec::new();
        for ring in 0..DOME_RINGS {
            let inner_t = ring as f32 / DOME_RINGS as f32;
            let outer_t = (ring + 1) as f32 / DOME_RINGS as f32;
            let inner_radius = self.radius * inner_t;
            let outer_radius = self.radius * outer_t;
            let inner_height = self.center.y + self.radius * crate::asset::dome_profile(inner_t);
            let outer_height = self.center.y + self.radius * crate::asset::dome_profile(outer_t);

            let run = outer_radius - inner_radius;
            let fall = inner_height - outer_height;
            let pitch = fall.atan2(run);
            let half_length = run.hypot(fall) * 0.5;

            // As many tiles as it takes to keep every chord short, and never fewer than
            // three: a ring of two boxes is not a ring.
            let circumference = std::f32::consts::TAU * outer_radius;
            let segments = ((circumference / DOME_TILE_ARC).ceil() as u32).max(3);
            let half_width = circumference / segments as f32 * 0.5 * OVERLAP;
            let middle_radius = (inner_radius + outer_radius) * 0.5;
            for step in 0..segments {
                let yaw = step as f32 / segments as f32 * std::f32::consts::TAU;
                let (sin_yaw, cos_yaw) = yaw.sin_cos();
                let tile = Tile {
                    label: self.label,
                    center: Vec3::new(
                        self.center.x + sin_yaw * middle_radius,
                        (inner_height + outer_height) * 0.5,
                        self.center.z + cos_yaw * middle_radius,
                    ),
                    half_extents: Vec3::new(half_width, half_thickness, half_length),
                    yaw,
                    pitch,
                };
                tiles.push(Tile {
                    center: tile.center - tile.up() * half_thickness,
                    ..tile
                });
            }
        }
        tiles
    }

    /// Where the three dummies on this dome stand, nearest first.
    ///
    /// Along the line from the player's spawn to the middle of the dome, so that "in front
    /// of the crest" and "behind it" are statements about the shot the player actually
    /// takes from where the scene puts them.
    #[must_use]
    pub fn dummy_stands(&self) -> Vec<DummyStand> {
        if !self.occupied {
            return Vec::new();
        }
        let toward = Vec3::new(
            self.center.x - PLAYER_SPAWN.x,
            0.0,
            self.center.z - PLAYER_SPAWN.z,
        );
        let direction = if toward.length() > 1e-3 {
            toward.normalize()
        } else {
            Vec3::new(0.0, 0.0, -1.0)
        };
        let offset = self.radius * DUMMY_OFFSET;
        [
            ("NEAR SLOPE", -offset, LineOfSight::Clear),
            ("CREST", 0.0, LineOfSight::Grazing),
            ("FAR SLOPE", offset, LineOfSight::Blocked),
        ]
        .into_iter()
        .map(|(name, along, expected)| {
            let ground = self.center + direction * along;
            DummyStand {
                dome: self.label,
                name,
                position: Vec3::new(ground.x, self.surface_height(ground.x, ground.z), ground.z),
                expected,
            }
        })
        .collect()
    }
}

/// What the course says should happen when the player casts at a dummy from the spawn.
///
/// Written down because the course is a test: a stand whose answer is not stated is a
/// dummy somebody has to guess about, and the guess is what a regression hides behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOfSight {
    /// In sight, with room to spare.
    Clear,
    /// In sight, with the ray passing close over the surface below it.
    Grazing,
    /// Not in sight: the dome is in the way.
    Blocked,
}

impl LineOfSight {
    /// Whether the ability should be allowed.
    #[must_use]
    pub fn is_visible(self) -> bool {
        matches!(self, Self::Clear | Self::Grazing)
    }

    /// Short uppercase name, for the developer overlay's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "CLEAR",
            Self::Grazing => "GRAZING",
            Self::Blocked => "BLOCKED",
        }
    }
}

/// One place a dummy stands, and what the course expects of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DummyStand {
    /// Which dome it is on.
    pub dome: &'static str,
    /// Where on that dome: `"NEAR SLOPE"`, `"CREST"`, `"FAR SLOPE"`.
    pub name: &'static str,
    /// Where its feet go.
    pub position: Vec3,
    /// What a cast from [`PLAYER_SPAWN`] should be told.
    pub expected: LineOfSight,
}

/// One pit: a square shaft in the floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pit {
    /// What a contact report calls it.
    pub label: &'static str,
    /// The middle of its opening, on the floor plane.
    pub center: Vec3,
    /// Half its width, in metres.
    pub half: f32,
    /// How far below the floor its bottom is, in metres.
    pub depth: f32,
}

impl Pit {
    /// The opening, as `(x_min, x_max, z_min, z_max)`.
    #[must_use]
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        (
            self.center.x - self.half,
            self.center.x + self.half,
            self.center.z - self.half,
            self.center.z + self.half,
        )
    }

    /// Where a character standing on its bottom has its feet.
    #[must_use]
    pub fn floor(&self) -> Vec3 {
        Vec3::new(self.center.x, self.center.y - self.depth, self.center.z)
    }

    /// The ramp that climbs out of the pit, towards +Z and the player's side of the course.
    ///
    /// Its top edge meets the lip at the pit's own +Z wall rather than somewhere in the
    /// middle of the hole — a ramp that reaches floor height over open air is a ramp that
    /// walks a player off the end of itself.
    #[must_use]
    pub fn ramp(&self) -> Ramp {
        let radians = PIT_RAMP_DEGREES.to_radians();
        let half_length = self.depth / (2.0 * radians.sin());
        let run = 2.0 * half_length * radians.cos();
        let bottom = self.center.y - self.depth;
        // The bottom end sits a full run short of the lip, so the top edge lands on it.
        let start_z = self.center.z + self.half - run;
        // The surface climbs towards +Z, so its normal leans back towards -Z.
        let up = Vec3::new(0.0, radians.cos(), -radians.sin());
        let center = Vec3::new(
            self.center.x,
            bottom + half_length * radians.sin(),
            start_z + half_length * radians.cos(),
        ) - up * RAMP_HALF_THICKNESS;
        Ramp {
            label: self.label,
            center,
            half_extents: Vec3::new(PIT_RAMP_HALF_WIDTH, RAMP_HALF_THICKNESS, half_length),
            // A rotation about +X by -angle carries the slab's own +Z axis upwards, which is
            // what makes the top face climb towards +Z.
            pitch: -radians,
        }
    }
}

/// How thick a ramp slab is, half of it, in metres.
const RAMP_HALF_THICKNESS: f32 = 0.25;

/// A sloped slab: the ramp out of a pit.
///
/// An oriented box, expressed as the numbers one is built from, for the same reason
/// [`Slab`] is: this crate does not depend on `engine-collision`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ramp {
    /// Which pit it climbs out of.
    pub label: &'static str,
    /// The middle of the slab.
    pub center: Vec3,
    /// Half its width, thickness and length.
    pub half_extents: Vec3,
    /// Its rotation about +X, in radians. Negative climbs towards +Z.
    pub pitch: f32,
}

/// The three domes, left to right.
#[must_use]
pub fn domes() -> [Dome; 3] {
    [
        Dome {
            label: "LEFT DOME",
            center: Vec3::new(-DOME_SPACING, 0.0, DOME_Z),
            radius: DOME_RADIUS,
            occupied: true,
        },
        Dome {
            label: "MIDDLE DOME",
            center: Vec3::new(0.0, 0.0, DOME_Z),
            radius: DOME_RADIUS,
            occupied: false,
        },
        Dome {
            label: "RIGHT DOME",
            center: Vec3::new(DOME_SPACING, 0.0, DOME_Z),
            radius: DOME_RADIUS,
            occupied: true,
        },
    ]
}

/// The two pits, left to right.
#[must_use]
pub fn pits() -> [Pit; 2] {
    [
        Pit {
            label: "LEFT PIT",
            center: Vec3::new(-PIT_SPACING, 0.0, PIT_Z),
            half: PIT_HALF,
            depth: PIT_DEPTH,
        },
        Pit {
            label: "RIGHT PIT",
            center: Vec3::new(PIT_SPACING, 0.0, PIT_Z),
            half: PIT_HALF,
            depth: PIT_DEPTH,
        },
    ]
}

/// Every place a dummy stands, left dome first.
#[must_use]
pub fn dummy_stands() -> Vec<DummyStand> {
    domes()
        .iter()
        .flat_map(|dome| dome.dummy_stands())
        .collect()
}

/// The floor, as the slabs a collision world is built from.
///
/// A slab of floor rather than a ground plane, because the course has holes in it and a
/// plane has none. The two pits share a z range, so the decomposition is five strips along
/// x, two of which are split in half by the shaft that passes through them:
///
/// ```text
///   |  1  |  2  |  3  |  4  |  5  |     strips
///   |     |=====|     |=====|     |     the two pit openings
/// ```
///
/// The bottom of every slab is the bottom of the pits, so nothing can get under the floor
/// and out through the side of a shaft.
#[must_use]
pub fn floor_slabs() -> Vec<Slab> {
    let extent = COURSE_HALF_EXTENT;
    let bottom = -PIT_DEPTH;
    let pits = pits();
    let (left_min, left_max, pit_z_min, pit_z_max) = pits[0].bounds();
    let (right_min, right_max, _, _) = pits[1].bounds();

    let full = |x0: f32, x1: f32, label: &'static str| {
        Slab::new(
            Vec3::new(x0, bottom, -extent),
            Vec3::new(x1, 0.0, extent),
            label,
        )
    };
    let split = |x0: f32, x1: f32, near: &'static str, far: &'static str| {
        [
            Slab::new(
                Vec3::new(x0, bottom, -extent),
                Vec3::new(x1, 0.0, pit_z_min),
                far,
            ),
            Slab::new(
                Vec3::new(x0, bottom, pit_z_max),
                Vec3::new(x1, 0.0, extent),
                near,
            ),
        ]
    };

    let mut slabs = vec![full(-extent, left_min, "COURSE FLOOR")];
    slabs.extend(split(left_min, left_max, "COURSE FLOOR", "COURSE FLOOR"));
    slabs.push(full(left_max, right_min, "COURSE FLOOR"));
    slabs.extend(split(right_min, right_max, "COURSE FLOOR", "COURSE FLOOR"));
    slabs.push(full(right_max, extent, "COURSE FLOOR"));
    slabs
}

/// The line-of-sight course.
///
/// Built the same way every layout here is: one pass, no random number generator and no
/// clock, so two runs are the same course and a screenshot is comparable to the last one.
#[must_use]
pub fn los_course() -> EnvironmentScene {
    let mut scene = EnvironmentScene::with_capacity("los_course", 96);
    scene.half_extent = COURSE_HALF_EXTENT;
    scene.lighting = EnvironmentLighting::arena_afternoon();
    scene.spawns = SpawnPoints {
        player: PLAYER_SPAWN,
        // The course spawns its dummies from `dummy_stands`, not from these — but a scene's
        // spawn points are read by anything that loads it, so they point at somewhere with
        // room rather than at the middle of a dome.
        featured_dummy: Vec3::new(0.0, 0.0, 24.0),
        group_center: Vec3::new(0.0, 0.0, 30.0),
        group_spacing: 3.0,
    };

    push_ground(&mut scene);
    push_domes(&mut scene);
    push_pits(&mut scene);
    push_boundary(&mut scene);
    push_background(&mut scene);
    scene
}

/// The floor the player walks on, drawn as one quad per collision slab.
fn push_ground(scene: &mut EnvironmentScene) {
    for slab in floor_slabs() {
        let size = slab.size();
        let middle = slab.center();
        scene.push(EnvironmentObject {
            asset: GROUND_PLAIN,
            transform: Transform::from_translation(Vec3::new(middle.x, FLOOR_LIFT, middle.z))
                .with_scale(Vec3::new(size.x, 1.0, size.z)),
            // The floor's collision is the slab this quad was cut from, and the course
            // builds it directly — an `EnvironmentCollision::Box` here would be a second
            // description of the same floor.
            collision: EnvironmentCollision::None,
        });
    }
}

/// The three domes.
///
/// Decoration, because a dome's collision is a field of boxes and an environment object
/// carries one. `engine_game::los_course` builds them from [`Dome::collision`].
fn push_domes(scene: &mut EnvironmentScene) {
    for dome in domes() {
        scene.push(
            EnvironmentObject::decoration(DOME, dome.center)
                // The model is a unit hemisphere, so the placement's scale *is* its radius.
                .with_scale(dome.radius),
        );
    }
}

/// The two pits: a bottom, four sides, and both facings of each side.
///
/// Two quads per side, back to back. A single-sided quad seen from the wrong side is not
/// there, and which side of a pit wall the camera ends up on depends on whether the player
/// is standing in the pit or looking into it — which is to say, both.
fn push_pits(scene: &mut EnvironmentScene) {
    for pit in pits() {
        let width = pit.half * 2.0;
        let bottom = pit.center.y - pit.depth;

        scene.push(EnvironmentObject {
            asset: GROUND_PLAIN,
            transform: Transform::from_translation(Vec3::new(
                pit.center.x,
                bottom + FLOOR_LIFT,
                pit.center.z,
            ))
            .with_scale(Vec3::new(width, 1.0, width)),
            collision: EnvironmentCollision::None,
        });

        // Each side, as an offset from the middle of the pit and the yaw that turns the
        // quad's local +Z to face along it.
        let sides: [(Vec3, f32); 4] = [
            (Vec3::new(0.0, 0.0, -pit.half), 0.0),
            (Vec3::new(0.0, 0.0, pit.half), std::f32::consts::PI),
            (Vec3::new(-pit.half, 0.0, 0.0), std::f32::consts::FRAC_PI_2),
            (Vec3::new(pit.half, 0.0, 0.0), -std::f32::consts::FRAC_PI_2),
        ];
        for (offset, yaw) in sides {
            let middle = Vec3::new(
                pit.center.x + offset.x,
                bottom + pit.depth * 0.5,
                pit.center.z + offset.z,
            );
            // A pitch of a quarter turn stands the quad up: its local +Z becomes vertical
            // and its +Y normal turns to face along the yaw. The second copy is pitched the
            // other way, which is the same wall with its front face on the other side.
            for pitch in [std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_2] {
                scene.push(EnvironmentObject {
                    asset: GROUND_PLAIN,
                    transform: Transform {
                        translation: middle,
                        rotation: Quat::from_yaw_pitch_roll(yaw, pitch, 0.0),
                        scale: Vec3::new(width, 1.0, pit.depth),
                    },
                    collision: EnvironmentCollision::None,
                });
            }
        }

        // The ramp out, drawn as the surface the collision world uses.
        let ramp = pit.ramp();
        let size = ramp.half_extents * 2.0;
        scene.push(EnvironmentObject {
            asset: GROUND_PLAIN,
            transform: Transform {
                translation: ramp.center + Vec3::new(0.0, RAMP_HALF_THICKNESS, 0.0),
                rotation: Quat::from_yaw_pitch_roll(0.0, ramp.pitch, 0.0),
                scale: Vec3::new(size.x, 1.0, size.z),
            },
            collision: EnvironmentCollision::None,
        });
    }
}

/// The kerb and the corner pillars: what tells the player where the course ends.
///
/// The confining wall itself is collision the course builds, exactly as the arena's is.
/// This is the part of the boundary the eye reads.
fn push_boundary(scene: &mut EnvironmentScene) {
    const INSET: f32 = 1.4;
    let edge = COURSE_HALF_EXTENT - INSET;
    let wall_collision = EnvironmentCollision::standing(4.0, 1.15, 0.45);
    let mut offset = -36.0;
    while offset <= 36.0 {
        for side in [-1.0_f32, 1.0] {
            scene.push(
                EnvironmentObject::decoration(WALL_SEGMENT, Vec3::new(offset, 0.0, side * edge))
                    .with_collision(wall_collision),
            );
            scene.push(
                EnvironmentObject::decoration(WALL_SEGMENT, Vec3::new(side * edge, 0.0, offset))
                    .with_collision(wall_collision)
                    .with_yaw(std::f32::consts::FRAC_PI_2),
            );
        }
        offset += 8.0;
    }

    let pillar_collision = EnvironmentCollision::standing(0.62, 4.6, 0.62);
    for x in [-edge, edge] {
        for z in [-edge, edge] {
            scene.push(
                EnvironmentObject::decoration(PILLAR, Vec3::new(x, 0.0, z))
                    .with_collision(pillar_collision),
            );
        }
    }
}

/// The sky and the distant ridges, so the course is not a floor in a void.
fn push_background(scene: &mut EnvironmentScene) {
    scene.push(EnvironmentObject::decoration(SKY_DOME, Vec3::ZERO));
    for (index, angle) in [0.0_f32, 1.6, 3.1, 4.7].into_iter().enumerate() {
        let distance = 240.0;
        scene.push(
            EnvironmentObject::decoration(
                RIDGE,
                Vec3::new(angle.cos() * distance, 0.0, angle.sin() * distance),
            )
            .with_yaw(-angle)
            .with_scale(1.0 + index as f32 * 0.1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_course_is_the_same_course_every_time() {
        assert_eq!(los_course(), los_course());
        assert_eq!(los_course().signature(), los_course().signature());
    }

    #[test]
    fn two_domes_carry_three_dummies_each_and_the_middle_one_carries_none() {
        let stands = dummy_stands();
        assert_eq!(stands.len(), 6);
        assert_eq!(stands.iter().filter(|s| s.dome == "LEFT DOME").count(), 3);
        assert_eq!(stands.iter().filter(|s| s.dome == "RIGHT DOME").count(), 3);
        assert!(stands.iter().all(|stand| stand.dome != "MIDDLE DOME"));
        for stand in &stands {
            assert!(
                stand.position.y > 0.5,
                "{} {} stands at y = {}",
                stand.dome,
                stand.name,
                stand.position.y
            );
        }
    }

    #[test]
    fn the_crest_is_the_highest_stand_and_the_two_slopes_are_level_with_each_other() {
        for dome in domes().into_iter().filter(|dome| dome.occupied) {
            let stands = dome.dummy_stands();
            let [near, crest, far] = [&stands[0], &stands[1], &stands[2]];
            assert!((crest.position.y - dome.height()).abs() < 1e-3);
            assert!(near.position.y < crest.position.y);
            assert!((near.position.y - far.position.y).abs() < 1e-3);
            // And the far one really is further away, which is what makes it the far one.
            let distance = |stand: &DummyStand| (stand.position - PLAYER_SPAWN).length();
            assert!(distance(near) < distance(crest));
            assert!(distance(crest) < distance(far));
            // Everything on the bar reaches 35 m; a stand the player cannot cast at from
            // the spawn tests the range rule instead of the one it was built for.
            assert!(
                distance(far) < 35.0,
                "{} is {} m away",
                far.name,
                distance(far)
            );
        }
    }

    #[test]
    fn a_domes_tiles_follow_the_curve_they_approximate() {
        for dome in domes() {
            let tiles = dome.collision();
            assert!(!tiles.is_empty());
            for tile in &tiles {
                // The top face is what a character stands on, so it is the top face that has
                // to be on the surface — the middle of the box is half a thickness below it.
                let surface = tile.center + tile.up() * tile.half_extents.y;
                let expected = dome.surface_height(surface.x, surface.z);
                // A tile is a chord and the profile it spans is curved, so the middle of a
                // tile sits a little under the surface where the curve bulges up and a
                // little over it where the curve bends the other way. Ten centimetres over
                // a 3.15 m hill: a player walks the drawn surface, not a shape beside it.
                let error = surface.y - expected;
                assert!(
                    error.abs() < 0.10,
                    "{} has a tile {error:.3} m off the surface at {surface:?}",
                    tile.label
                );
                assert!(surface.y <= dome.center.y + dome.height() + 1e-3);
            }
        }
    }

    #[test]
    fn every_tile_is_gentle_enough_to_walk_up_and_they_meet_without_a_step() {
        /// What the character controller refuses, in degrees.
        const CONTROLLER_LIMIT: f32 = 50.0;
        /// What the character controller can step up, in metres.
        const STEP_HEIGHT: f32 = 0.45;

        for dome in domes() {
            let tiles = dome.collision();
            let steepest = tiles
                .iter()
                .map(Tile::slope_degrees)
                .fold(0.0_f32, f32::max);
            assert!(
                steepest < CONTROLLER_LIMIT - 5.0,
                "{} has a {steepest} degree tile against a {CONTROLLER_LIMIT} degree limit",
                dome.label
            );

            // Walk the profile from rim to crest and check what a foot meets at each band
            // boundary: the tiles either side of it are chords of the same curve, so the
            // difference between where one ends and the next begins is the step to climb.
            for ring in 1..DOME_RINGS {
                let t = ring as f32 / DOME_RINGS as f32;
                let radius = dome.radius * t;
                let below = dome.surface_height(dome.center.x + radius + 0.01, dome.center.z);
                let above = dome.surface_height(dome.center.x + radius - 0.01, dome.center.z);
                assert!(
                    (above - below).abs() < STEP_HEIGHT,
                    "{} steps {:.2} m at ring {ring}",
                    dome.label,
                    above - below
                );
            }
        }
    }

    #[test]
    fn the_floor_covers_the_course_except_where_the_pits_are() {
        let slabs = floor_slabs();
        let inside = |point: Vec3| {
            slabs.iter().any(|slab| {
                point.x >= slab.min.x
                    && point.x <= slab.max.x
                    && point.z >= slab.min.z
                    && point.z <= slab.max.z
            })
        };
        assert!(inside(PLAYER_SPAWN));
        for dome in domes() {
            assert!(inside(dome.center));
        }
        for pit in pits() {
            assert!(!inside(pit.center), "{} has a floor over it", pit.label);
            // And its lip is still floor, so the hole is the size it says it is.
            assert!(inside(Vec3::new(
                pit.center.x,
                0.0,
                pit.center.z + pit.half + 0.5
            )));
        }
    }

    #[test]
    fn a_pits_ramp_starts_on_its_floor_and_ends_on_its_lip() {
        for pit in pits() {
            let ramp = pit.ramp();
            let radians = PIT_RAMP_DEGREES.to_radians();
            let along = Vec3::new(0.0, radians.sin(), radians.cos());
            let up = Vec3::new(0.0, radians.cos(), -radians.sin());
            let surface = ramp.center + up * RAMP_HALF_THICKNESS;
            let bottom = surface - along * ramp.half_extents.z;
            let top = surface + along * ramp.half_extents.z;
            assert!(
                (bottom.y - (pit.center.y - pit.depth)).abs() < 1e-3,
                "the ramp starts at y = {}",
                bottom.y
            );
            assert!(
                (top.y - pit.center.y).abs() < 1e-3,
                "the ramp ends at y = {}",
                top.y
            );
            assert!(
                (top.z - (pit.center.z + pit.half)).abs() < 1e-3,
                "the ramp ends at z = {} and the lip is at {}",
                top.z,
                pit.center.z + pit.half
            );
            // And the whole of it is inside the hole it climbs out of.
            assert!(bottom.z > pit.center.z - pit.half);
            // Walkable: the controller refuses anything past its slope limit, and a ramp a
            // player cannot walk up is a pit a player cannot leave.
            const { assert!(PIT_RAMP_DEGREES < 45.0) };
        }
    }

    #[test]
    fn no_pit_swallows_the_player_spawn_or_undercuts_a_dome() {
        for pit in pits() {
            let (x_min, x_max, z_min, z_max) = pit.bounds();
            assert!(
                PLAYER_SPAWN.z > z_max,
                "the player would spawn in {}",
                pit.label
            );
            for dome in domes() {
                let gap = ((dome.center.x - pit.center.x).abs() - dome.radius - pit.half)
                    .max((dome.center.z - pit.center.z).abs() - dome.radius - pit.half);
                assert!(gap > 0.0, "{} overlaps {}", pit.label, dome.label);
            }
            assert!(x_min < x_max && z_min < z_max);
        }
    }

    #[test]
    fn every_pit_wall_is_drawn_from_both_sides_and_the_ramp_faces_up() {
        let scene = los_course();
        let quads: Vec<&EnvironmentObject> = scene
            .objects
            .iter()
            .filter(|object| object.asset == GROUND_PLAIN)
            .collect();

        // A quad's drawn side is its +Y axis carried round by its rotation. The floor and
        // the pit bottoms face up; each pit wall is two quads with opposite facings, so
        // whichever side of it the camera is on, the wall is there.
        let facing = |object: &EnvironmentObject| object.transform.rotation.rotate(Vec3::Y);
        let up = quads
            .iter()
            .filter(|object| facing(object).dot(Vec3::Y) > 0.99)
            .count();
        // Seven floor slabs, two pit bottoms, and one ramp per pit, whose surface is tilted
        // but still faces up.
        assert_eq!(up, 7 + 2);

        for pit in pits() {
            let ramp = pit.ramp();
            let Some(surface) = quads.iter().find(|object| {
                (object.position().x - ramp.center.x).abs() < 1e-3
                    && (object.position().z - ramp.center.z).abs() < 1e-3
                    && object.position().y > ramp.center.y - 1.0
                    && facing(object).dot(Vec3::Y) < 0.99
                    && facing(object).dot(Vec3::Y) > 0.5
            }) else {
                panic!("{} has no ramp surface drawn over its ramp", pit.label);
            };
            // The drawn surface leans the same way the collision ramp does, or a player
            // walks up something that is not where it appears to be.
            let radians = PIT_RAMP_DEGREES.to_radians();
            let collision_up = Vec3::new(0.0, radians.cos(), -radians.sin());
            assert!(
                facing(surface).dot(collision_up) > 0.999,
                "the drawn ramp faces {:?} and the collision ramp faces {collision_up:?}",
                facing(surface)
            );
        }

        // Four walls a pit, each drawn twice, and the two copies face opposite ways.
        for pit in pits() {
            let (x_min, x_max, z_min, z_max) = pit.bounds();
            for (x, z) in [
                (pit.center.x, z_min),
                (pit.center.x, z_max),
                (x_min, pit.center.z),
                (x_max, pit.center.z),
            ] {
                let sides: Vec<Vec3> = quads
                    .iter()
                    .filter(|object| {
                        (object.position().x - x).abs() < 1e-3
                            && (object.position().z - z).abs() < 1e-3
                            && object.position().y < 0.0
                    })
                    .map(|object| facing(object))
                    .collect();
                assert_eq!(sides.len(), 2, "the wall at ({x}, {z}) is not drawn twice");
                assert!(
                    sides[0].dot(sides[1]) < -0.999,
                    "the two copies of the wall at ({x}, {z}) face the same way"
                );
                // And a wall is vertical: a pit with a sloped side is a ramp nobody meant.
                assert!(sides[0].dot(Vec3::Y).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn the_scene_draws_a_dome_for_every_dome_and_a_floor_with_holes_in_it() {
        let scene = los_course();
        assert_eq!(scene.count_of(DOME), domes().len());
        assert_eq!(scene.count_of(SKY_DOME), 1);
        // Seven floor quads, and one bottom, eight wall quads and a ramp per pit.
        assert_eq!(scene.count_of(GROUND_PLAIN), 7 + 2 * (1 + 8 + 1));
        assert!(scene.count_of(WALL_SEGMENT) >= 32);
    }
}
