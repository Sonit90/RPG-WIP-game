//! The collision demo scene: a test arena with nothing in it but geometry.
//!
//! ```text
//!   x=0        25        50        75       105       135       165
//!  WALL      CORNER    STEPS     SLOPES     DROP    HIGH SPEED  COMBINED
//!  SLIDE
//!   |          |         |         |          |         |          |
//!  ===========================================================================  platform
//!                                                                              (top at y=0)
//!
//!                            .............................  void floor at y = -15
//! ```
//!
//! Separate from [`crate::arena`] on purpose, and the separation is the whole point of the
//! scene. A collision bug has to be reproducible without abilities, dummies, damage,
//! telemetry or a damage meter in the way, and test geometry mixed into the combat arena
//! would be geometry a player trips over during a fight.
//!
//! Nothing here is meant to be pretty. It is meant to make collision behaviour observable:
//! every station is a question ("does it slide?", "does it step?") and standing in one and
//! walking forward is the answer.
//!
//! ## Reproducibility
//!
//! The layout is a list of constants, so two runs build the same scene to the bit. Each
//! station has a spawn point, and [`PlayerCommand::Teleport`](crate::command::PlayerCommand)
//! puts the character on it at rest — so a test is repeated by pressing a number, not by
//! walking back across the level and hoping to arrive the same way.

use engine_collision::shape::Obb;
use engine_collision::world::CollisionWorld;
use engine_core::math::Vec3;

use crate::entity::EntityId;
use crate::movement::MovementConfig;
use crate::sim::{SimConfig, Simulation};
use crate::world::EntityDesc;

/// Top of the main platform. Everything stands on this.
pub const FLOOR_Y: f32 = 0.0;

/// The catch floor a long fall lands on.
pub const VOID_Y: f32 = -15.0;

/// Height of an ordinary test wall, in metres. Taller than a jump.
const WALL_HEIGHT: f32 = 4.0;

/// Thickness of an ordinary test wall, in metres.
const WALL_THICKNESS: f32 = 1.0;

/// How thin the tunnelling barriers are, in metres.
///
/// An eighth of the character's radius. Anything a swept test can cross, this crosses.
const THIN: f32 = 0.05;

/// One place a developer can be put down to run one test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Station {
    /// Which number key selects it, 1 to 9.
    pub key: u8,
    /// What is being tested here.
    pub name: &'static str,
    /// Where the character's feet go.
    pub spawn: Vec3,
    /// What to do once standing there.
    pub instruction: &'static str,
}

/// A label floating over a piece of test geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    /// Where the text is anchored, in world space.
    pub position: Vec3,
    /// The text. Uppercase, because the developer font is.
    pub text: &'static str,
}

/// Everything the demo scene built.
#[derive(Debug, Clone)]
pub struct DemoScene {
    /// The character.
    pub player: EntityId,
    /// The test stations, in key order.
    pub stations: Vec<Station>,
    /// The labels over the geometry.
    pub markers: Vec<Marker>,
}

impl DemoScene {
    /// The station a number key selects, if any.
    #[must_use]
    pub fn station(&self, key: u8) -> Option<Station> {
        self.stations.iter().copied().find(|s| s.key == key)
    }

    /// Where the character starts, and where `R` puts it back.
    #[must_use]
    pub fn spawn(&self) -> Vec3 {
        self.stations
            .first()
            .map_or(Vec3::new(0.0, FLOOR_Y + 0.1, 0.0), |station| station.spawn)
    }
}

/// The stations, in the order their keys are pressed.
///
/// A `const` rather than a builder: this is a fixed test course, and the numbers are the
/// scene's contract with whoever is testing it.
pub const STATIONS: [Station; 7] = [
    Station {
        key: 1,
        name: "WALL / SLIDE",
        spawn: Vec3::new(0.0, 0.1, 0.0),
        instruction: "WALK +X INTO THE WALL, THEN DIAGONALLY ALONG IT",
    },
    Station {
        key: 2,
        name: "CORNER",
        spawn: Vec3::new(25.0, 0.1, -6.0),
        instruction: "RUN INTO THE INSIDE CORNER AND BACK OUT OF IT",
    },
    Station {
        key: 3,
        name: "STEPS",
        spawn: Vec3::new(50.0, 0.1, 0.0),
        instruction: "WALK +X OVER 0.15, 0.30, 0.45, THEN STOP AT 0.90",
    },
    Station {
        key: 4,
        name: "SLOPES",
        spawn: Vec3::new(75.0, 0.1, 0.0),
        instruction: "WALK +X UP EACH RAMP: 15, 30, 45 WALK, 70 DOES NOT",
    },
    Station {
        key: 5,
        name: "DROP",
        spawn: Vec3::new(105.0, 4.1, 0.0),
        instruction: "WALK OFF ANY EDGE, THEN OFF THE PLATFORM AT Z = 15",
    },
    Station {
        key: 6,
        name: "HIGH SPEED",
        spawn: Vec3::new(135.0, 60.0, 0.0),
        instruction: "FALL 52 M ONTO A 5 CM SLAB, THEN RUN AT THE 5 CM WALL",
    },
    Station {
        key: 7,
        name: "COMBINED",
        spawn: Vec3::new(165.0, 0.1, -12.0),
        instruction: "RAMP INTO A CORNER, WITH A STEP IN THE WAY",
    },
];

/// Builds the collision demo scene.
///
/// The simulation is an ordinary [`Simulation`] with one entity in it. Combat, abilities,
/// statuses and telemetry are all present and all inert, because they have nothing to act
/// on — which is cheaper and far less code than a second kind of simulation would be.
#[must_use]
pub fn collision_demo() -> (Simulation, DemoScene) {
    let mut simulation = Simulation::new(SimConfig {
        movement: MovementConfig {
            // The arena clamp is what confines the combat scene. Here the geometry is the
            // confinement, and a clamp at the usual 40 m would cut the course in half.
            arena_half_extent: 0.0,
            ..MovementConfig::default()
        },
        ..SimConfig::default()
    });

    let spawn = STATIONS[0].spawn;
    let player = simulation.world_mut().spawn(EntityDesc::player(spawn));
    simulation.set_player_entity(player);

    let mut markers = Vec::with_capacity(32);
    build_collision(simulation.collision_world_mut(), &mut markers);

    (
        simulation,
        DemoScene {
            player,
            stations: STATIONS.to_vec(),
            markers,
        },
    )
}

/// Fills a collision world with the test course, and `markers` with its labels.
///
/// Public so a headless test can build the geometry without a simulation around it.
pub fn build_collision(world: &mut CollisionWorld, markers: &mut Vec<Marker>) {
    world.clear();
    markers.clear();

    // The ground everything that falls lands on, so a mistake is a long drop rather than an
    // infinite one.
    world.add_ground(VOID_Y, "VOID FLOOR");
    // The main platform. One slab, with its top at y = 0.
    world.add_box(
        Vec3::new(-15.0, -2.0, -15.0),
        Vec3::new(190.0, FLOOR_Y, 15.0),
        "PLATFORM",
    );

    wall_station(world, markers);
    corner_station(world, markers);
    step_station(world, markers);
    slope_station(world, markers);
    drop_station(world, markers);
    high_speed_station(world, markers);
    combined_station(world, markers);

    world.rebuild_broad_phase();
}

/// A single long wall to walk into and slide along.
fn wall_station(world: &mut CollisionWorld, markers: &mut Vec<Marker>) {
    let x = STATIONS[0].spawn.x;
    world.add_box(
        Vec3::new(x + 6.0, FLOOR_Y, -10.0),
        Vec3::new(x + 6.0 + WALL_THICKNESS, FLOOR_Y + WALL_HEIGHT, 10.0),
        "WALL",
    );
    markers.push(Marker {
        position: Vec3::new(x, FLOOR_Y + 5.0, 0.0),
        text: "1 WALL / SLIDE",
    });
}

/// Two perpendicular walls, making an inside corner.
fn corner_station(world: &mut CollisionWorld, markers: &mut Vec<Marker>) {
    let x = STATIONS[1].spawn.x;
    world.add_box(
        Vec3::new(x + 5.0, FLOOR_Y, -10.0),
        Vec3::new(x + 5.0 + WALL_THICKNESS, FLOOR_Y + WALL_HEIGHT, 2.0),
        "CORNER WALL +X",
    );
    world.add_box(
        Vec3::new(x - 6.0, FLOOR_Y, 2.0),
        Vec3::new(x + 6.0, FLOOR_Y + WALL_HEIGHT, 2.0 + WALL_THICKNESS),
        "CORNER WALL +Z",
    );
    // And a narrow corridor beside it, two capsule-widths across.
    world.add_box(
        Vec3::new(x - 10.0, FLOOR_Y, -12.0),
        Vec3::new(x - 9.0, FLOOR_Y + WALL_HEIGHT, -2.0),
        "CORRIDOR -X",
    );
    world.add_box(
        Vec3::new(x - 7.4, FLOOR_Y, -12.0),
        Vec3::new(x - 6.4, FLOOR_Y + WALL_HEIGHT, -2.0),
        "CORRIDOR +X",
    );
    markers.push(Marker {
        position: Vec3::new(x, FLOOR_Y + 5.0, -6.0),
        text: "2 CORNER",
    });
    markers.push(Marker {
        position: Vec3::new(x - 8.2, FLOOR_Y + 5.0, -7.0),
        text: "CORRIDOR 1.6M",
    });
}

/// Heights of the four steps, in metres.
///
/// The third is the character's `max_step_height` and the fourth is twice it — so the pair
/// either side of the limit is directly comparable.
const STEP_HEIGHTS: [f32; 4] = [0.15, 0.30, 0.45, 0.90];

/// Labels for the four steps.
const STEP_LABELS: [&str; 4] = [
    "STEP 0.15M",
    "STEP 0.30M",
    "STEP 0.45M MAX",
    "0.90M TOO HIGH",
];

/// Four blocks of increasing height in a row.
fn step_station(world: &mut CollisionWorld, markers: &mut Vec<Marker>) {
    let x = STATIONS[2].spawn.x;
    markers.push(Marker {
        position: Vec3::new(x, FLOOR_Y + 5.0, 0.0),
        text: "3 STEPS",
    });

    for (index, height) in STEP_HEIGHTS.iter().enumerate() {
        // Separated by a gap of floor, *not* built into a staircase. Each block has to be
        // approached from ground level, or the 0.90 m obstacle would be only 0.45 m above
        // the block before it and the character would walk up the thing that is there to
        // stop it.
        let near = x + 4.0 + index as f32 * 5.0;
        world.add_box(
            Vec3::new(near, FLOOR_Y, -5.0),
            Vec3::new(near + 2.5, FLOOR_Y + height, 5.0),
            STEP_LABELS[index],
        );
        markers.push(Marker {
            position: Vec3::new(near + 1.25, FLOOR_Y + height + 1.5, -5.5),
            text: STEP_LABELS[index],
        });
    }
}

/// Angles of the four ramps, in degrees.
const SLOPE_ANGLES: [f32; 4] = [15.0, 30.0, 45.0, 70.0];

/// Labels for the four ramps.
const SLOPE_LABELS: [&str; 4] = ["SLOPE 15", "SLOPE 30", "SLOPE 45", "SLOPE 70 TOO STEEP"];

/// Four ramps of increasing angle, side by side.
fn slope_station(world: &mut CollisionWorld, markers: &mut Vec<Marker>) {
    let x = STATIONS[3].spawn.x;
    markers.push(Marker {
        position: Vec3::new(x, FLOOR_Y + 5.0, 0.0),
        text: "4 SLOPES",
    });

    for (index, degrees) in SLOPE_ANGLES.iter().enumerate() {
        let z = -9.0 + index as f32 * 6.0;
        world.add_ramp(ramp(x + 4.0, z, *degrees), SLOPE_LABELS[index]);
        markers.push(Marker {
            position: Vec3::new(x + 2.0, FLOOR_Y + 2.0, z),
            text: SLOPE_LABELS[index],
        });
    }
}

/// A ramp whose walking surface starts at `(x, FLOOR_Y, z)` and rises along +X.
///
/// Rotating a slab by `+degrees` about `+Z` carries its own `+X` axis upwards, so the top
/// face climbs towards `+X` and its normal leans back towards −X.
///
/// The slab is then sunk by its own half-thickness along that normal, so its **top face**
/// meets the floor at `x` rather than its centre. Without that, the ramp's lower end
/// presents a 0.5 m vertical lip — taller than the character can step onto — and a gentle
/// ramp becomes unreachable for a reason that has nothing to do with its angle.
fn ramp(x: f32, z: f32, degrees: f32) -> Obb {
    let radians = degrees.to_radians();
    let half_length = 5.0;
    let half_thickness = 0.25;
    let up = Vec3::new(-radians.sin(), radians.cos(), 0.0);
    let centre = Vec3::new(
        x + half_length * radians.cos(),
        FLOOR_Y + half_length * radians.sin(),
        z,
    ) - up * half_thickness;
    Obb::ramp(
        centre,
        Vec3::new(half_length, half_thickness, 2.5),
        Vec3::Z,
        radians,
    )
}

/// A raised platform with edges to walk off.
fn drop_station(world: &mut CollisionWorld, markers: &mut Vec<Marker>) {
    let x = STATIONS[4].spawn.x;
    world.add_box(
        Vec3::new(x - 5.0, FLOOR_Y, -6.0),
        Vec3::new(x + 5.0, FLOOR_Y + 4.0, 6.0),
        "LEDGE",
    );
    markers.push(Marker {
        position: Vec3::new(x, FLOOR_Y + 9.0, 0.0),
        text: "5 DROP",
    });
    markers.push(Marker {
        position: Vec3::new(x, FLOOR_Y + 1.0, 12.0),
        text: "PLATFORM EDGE AT Z 15",
    });
}

/// A 5 cm slab far below a 52 m drop, and a 5 cm wall to run at.
fn high_speed_station(world: &mut CollisionWorld, markers: &mut Vec<Marker>) {
    let x = STATIONS[5].spawn.x;
    world.add_box(
        Vec3::new(x - 5.0, 8.0, -6.0),
        Vec3::new(x + 5.0, 8.0 + THIN, 6.0),
        "THIN SLAB 5CM",
    );
    world.add_box(
        Vec3::new(x + 8.0, FLOOR_Y, -8.0),
        Vec3::new(x + 8.0 + THIN, FLOOR_Y + WALL_HEIGHT, 8.0),
        "THIN WALL 5CM",
    );
    markers.push(Marker {
        position: Vec3::new(x, 8.0 + 3.0, 0.0),
        text: "6 HIGH SPEED",
    });
    markers.push(Marker {
        position: Vec3::new(x + 8.0, FLOOR_Y + 5.0, 0.0),
        text: "THIN WALL 5CM",
    });
}

/// A ramp that leads into a corner, with a step across the approach.
fn combined_station(world: &mut CollisionWorld, markers: &mut Vec<Marker>) {
    let x = STATIONS[6].spawn.x;
    world.add_ramp(ramp(x - 3.0, -8.0, 25.0), "COMBINED RAMP");
    world.add_box(
        Vec3::new(x + 5.0, FLOOR_Y, -12.0),
        Vec3::new(x + 5.0 + WALL_THICKNESS, FLOOR_Y + WALL_HEIGHT, 0.0),
        "COMBINED WALL +X",
    );
    world.add_box(
        Vec3::new(x - 6.0, FLOOR_Y, 0.0),
        Vec3::new(x + 6.0, FLOOR_Y + WALL_HEIGHT, WALL_THICKNESS),
        "COMBINED WALL +Z",
    );
    world.add_box(
        Vec3::new(x - 2.0, FLOOR_Y, -6.0),
        Vec3::new(x + 4.0, FLOOR_Y + 0.35, -5.0),
        "COMBINED STEP 0.35M",
    );
    markers.push(Marker {
        position: Vec3::new(x, FLOOR_Y + 5.0, -12.0),
        text: "7 COMBINED",
    });
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::command::PlayerCommand;
    use engine_collision::world::QueryBuffers;
    use engine_collision::{CollisionMask, controller::CharacterConfig};

    #[test]
    fn the_demo_scene_has_a_player_and_no_combat_at_all() {
        let (simulation, scene) = collision_demo();
        assert_eq!(simulation.world().len(), 1, "one entity: the character");
        assert_eq!(simulation.player().entity, scene.player);
        assert_eq!(
            simulation.player().target,
            None,
            "there is nothing to fight"
        );
        assert_eq!(crate::arena::dummies(simulation.world()).count(), 0);
    }

    #[test]
    fn every_station_has_a_key_a_name_and_a_place_to_stand() {
        let (_, scene) = collision_demo();
        assert_eq!(scene.stations.len(), 7);
        for (index, station) in scene.stations.iter().enumerate() {
            assert_eq!(station.key, index as u8 + 1, "the keys are 1 to 7 in order");
            assert!(!station.name.is_empty());
            assert!(!station.instruction.is_empty());
            assert!(scene.station(station.key).is_some());
        }
        assert_eq!(scene.station(9), None);
        assert_eq!(scene.spawn(), STATIONS[0].spawn);
    }

    #[test]
    fn no_station_puts_the_character_inside_something() {
        // The scene is only reproducible if every teleport lands somewhere legal. A spawn
        // inside geometry is a character that pops sideways on its first tick.
        let (simulation, scene) = collision_demo();
        let config = CharacterConfig::default();
        let mut buffers = QueryBuffers::new();
        for station in &scene.stations {
            assert!(
                !simulation.collision_world().capsule_overlaps(
                    &config.capsule_at(station.spawn),
                    CollisionMask::WORLD,
                    &mut buffers,
                ),
                "{} spawns inside geometry",
                station.name
            );
        }
    }

    #[test]
    fn the_scene_is_built_identically_every_time() {
        let describe = || {
            let (simulation, scene) = collision_demo();
            let world = simulation.collision_world();
            let shapes: Vec<_> = world
                .statics()
                .map(|id| (world.label(id), world.bounds(id)))
                .collect();
            (shapes, scene.markers.clone(), scene.stations.clone())
        };
        let (shapes, markers, stations) = describe();
        let (again, markers_again, stations_again) = describe();
        assert_eq!(shapes, again);
        assert_eq!(markers, markers_again);
        assert_eq!(stations, stations_again);
        assert!(shapes.len() > 20, "the course has geometry in it");
    }

    #[test]
    fn the_broad_phase_is_ready_when_the_scene_is_handed_over() {
        let (simulation, _) = collision_demo();
        assert!(
            !simulation.collision_world().is_dirty(),
            "a scene must not hand over a world that still has to be rebuilt"
        );
    }

    #[test]
    fn every_label_belongs_to_something_that_exists() {
        let (simulation, scene) = collision_demo();
        let world = simulation.collision_world();
        assert!(!scene.markers.is_empty());
        for marker in &scene.markers {
            assert!(!marker.text.is_empty());
            assert!(
                marker.position.y > VOID_Y,
                "{:?} is below the world",
                marker
            );
        }
        for id in world.statics() {
            assert!(!world.label(id).is_empty(), "an unnamed collision object");
        }
    }

    #[test]
    fn teleporting_puts_the_character_at_a_station_at_rest() {
        let (mut simulation, scene) = collision_demo();
        let steps = scene.station(3).expect("station 3 is the steps");

        simulation.push_command(PlayerCommand::Teleport {
            position: steps.spawn,
        });
        simulation.step_once();

        let body = simulation.player_body().expect("the player exists");
        assert!(
            (body.position.x - steps.spawn.x).abs() < 0.1,
            "{:?}",
            body.position
        );
        assert!(body.grounded, "it should be standing: {body:?}");
        assert!(body.horizontal_speed() < 1e-3, "a teleport is not a launch");
    }

    #[test]
    fn a_teleport_does_not_leave_a_trail_for_the_renderer_to_interpolate() {
        let (mut simulation, scene) = collision_demo();
        simulation.step_many(10);
        let far = scene.station(7).expect("station 7 exists");
        simulation.push_command(PlayerCommand::Teleport {
            position: far.spawn,
        });
        simulation.step_once();

        let world = simulation.world();
        let player = simulation.player().entity;
        let previous = world.previous_body(player).expect("history").position;
        let current = world.body(player).expect("the player").position;
        assert!(
            (previous - current).length() < 1.0,
            "the character would slide {} m across the level",
            (previous - current).length()
        );
    }

    #[test]
    fn the_course_is_laid_out_left_to_right_without_overlapping_stations() {
        let mut previous = f32::NEG_INFINITY;
        for station in STATIONS {
            assert!(
                station.spawn.x > previous,
                "{} is not past the station before it",
                station.name
            );
            previous = station.spawn.x;
        }
    }
}
