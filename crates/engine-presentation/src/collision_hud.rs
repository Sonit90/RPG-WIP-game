//! The collision demo's developer readout, and the labels over its test geometry.
//!
//! ```text
//! COLLISION DEMO - STATION 3 STEPS
//! WALK +X OVER 0.15, 0.30, 0.45, THEN STOP AT 0.90
//!
//! POS       50.00     0.01     0.00
//! VEL        4.00    -0.02     0.00   SPEED 4.00
//! GROUNDED   YES   STATE RUNNING
//! NORMAL     0.00     1.00     0.00   SLOPE 0.0 DEG
//! SURFACE   STEP 0.30M
//!
//! CAPSULE   RADIUS 0.40   HEIGHT 1.80
//! LIMITS    STEP 0.45   SLOPE 50.0 DEG
//!
//! BROAD     LINEAR   CANDIDATES 6
//! NARROW    TESTS 6   CONTACTS 2
//! MOVE      SUBSTEPS 2   ITERATIONS 4   STEPPED 0.30
//! STATICS   31
//!
//! 1-7 TEST STATIONS   R RESET   F3 COLLISION DRAW   F5 THIS READOUT
//! ```
//!
//! This is **not** the combat HUD and shares nothing with it. It exists only in the
//! collision demo scene, it is compiled behind the application's `debug-overlay` feature,
//! and every number in it is read from the simulation rather than tracked separately here.
//!
//! It goes through the renderer's existing developer overlay — the same path
//! [`crate::hud`] uses — because the project already has one debug text channel and a
//! second one would be a second thing to keep working. The font is a 5x7 bitmap with
//! uppercase letters, digits and a little punctuation, so every line is written uppercase.

use std::fmt::Write as _;

use engine_collision::CharacterConfig;
use engine_core::math::{Mat4, Vec3};
use engine_game::demo::{DemoScene, Marker, Station};
use engine_game::sim::Simulation;
use engine_render_data::{TextAlign, UiColor, UiDrawList};

use crate::nameplate::project;

/// Everything the readout reports.
#[derive(Debug, Clone, Copy)]
pub struct CollisionHudModel<'a> {
    /// The simulation being driven.
    pub simulation: &'a Simulation,
    /// The demo scene's stations and labels.
    pub scene: &'a DemoScene,
    /// The station most recently teleported to, if any.
    pub station: Option<Station>,
    /// Static objects the debug view drew, or zero when it is off.
    pub debug_entities: usize,
}

/// Rewrites `lines` in place from `model`.
///
/// In place, like [`crate::hud::write_lines`] and for the same reason: formatting allocates
/// and the readout is rebuilt several times a second, so the `String` buffers are reused.
pub fn write_lines(lines: &mut Vec<String>, model: &CollisionHudModel<'_>) {
    let mut row = 0;
    let line = |lines: &mut Vec<String>, row: &mut usize| -> usize {
        while lines.len() <= *row {
            lines.push(String::new());
        }
        lines[*row].clear();
        let index = *row;
        *row += 1;
        index
    };

    let simulation = model.simulation;
    let config: CharacterConfig = simulation.config().character;
    let collision = simulation.collision_world();
    let last = simulation.last_move();

    let index = line(lines, &mut row);
    match model.station {
        Some(station) => {
            let _ = write!(
                lines[index],
                "COLLISION DEMO - STATION {} {}",
                station.key, station.name
            );
            let index = line(lines, &mut row);
            let _ = write!(lines[index], "{}", station.instruction);
        }
        None => {
            let _ = write!(lines[index], "COLLISION DEMO");
            let index = line(lines, &mut row);
            let _ = write!(lines[index], "PRESS 1-7 TO STAND AT A TEST STATION");
        }
    }

    let Some(body) = simulation.player_body() else {
        let index = line(lines, &mut row);
        let _ = write!(lines[index], "NO CHARACTER IN THE SCENE");
        lines.truncate(row);
        return;
    };

    let index = line(lines, &mut row);
    lines[index].clear();

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "POS      {:>8.2} {:>8.2} {:>8.2}",
        body.position.x, body.position.y, body.position.z
    );

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "VEL      {:>8.2} {:>8.2} {:>8.2}   SPEED {:>5.2}",
        body.velocity.x,
        body.velocity.y,
        body.velocity.z,
        body.horizontal_speed()
    );

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "GROUNDED {}   STATE {}",
        if body.grounded { "YES" } else { "NO " },
        body.state.as_str()
    );

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "NORMAL   {:>8.2} {:>8.2} {:>8.2}   SLOPE {:>5.1} DEG",
        body.ground_normal.x,
        body.ground_normal.y,
        body.ground_normal.z,
        body.slope_angle().to_degrees()
    );

    let index = line(lines, &mut row);
    let surface = last.ground.map_or("NONE - AIRBORNE", |id| {
        let label = collision.label(id);
        if label.is_empty() { "UNNAMED" } else { label }
    });
    let _ = write!(lines[index], "SURFACE  {surface}");

    let index = line(lines, &mut row);
    lines[index].clear();

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "CAPSULE  RADIUS {:.2}   HEIGHT {:.2}",
        config.radius, config.height
    );

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "LIMITS   STEP {:.2}   SLOPE {:.1} DEG",
        config.max_step_height,
        config.max_slope_angle.to_degrees()
    );

    let index = line(lines, &mut row);
    lines[index].clear();

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "BROAD    {}   CANDIDATES {}",
        collision.broad_phase().name(),
        last.stats.broad_candidates
    );

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "NARROW   TESTS {}   CONTACTS {}",
        last.stats.narrow_tests, last.stats.contacts
    );

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "MOVE     SUBSTEPS {}   ITERATIONS {}   STEPPED {:.2}",
        last.substeps, last.iterations, last.stepped_up
    );

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "WORLD    STATICS {}   DRAWN {}   WALL {}   CEILING {}",
        collision.len(),
        model.debug_entities,
        if last.hit_wall { "YES" } else { "NO " },
        if last.hit_ceiling { "YES" } else { "NO " },
    );

    let index = line(lines, &mut row);
    lines[index].clear();

    let index = line(lines, &mut row);
    let _ = write!(
        lines[index],
        "1-{} TEST STATIONS   R RESET   F3 COLLISION DRAW   F5 THIS READOUT",
        model.scene.stations.len().max(1)
    );

    // Anything the previous, longer readout left behind would otherwise still be drawn.
    lines.truncate(row);
}

/// Colour the world-space test labels are drawn in.
const MARKER_COLOR: UiColor = [0.95, 0.92, 0.55, 1.0];

/// How far a label is still drawn from, in metres.
///
/// Past this the course is a wall of overlapping text rather than a set of labels.
pub const MARKER_MAX_DISTANCE: f32 = 60.0;

/// Draws the scene's test labels where their geometry is on screen.
///
/// Projection through the same camera the world is drawn with — the same machinery
/// [`crate::nameplate`] uses for enemy nameplates, rather than a second way of putting text
/// over a world position.
pub fn push_markers(
    list: &mut UiDrawList,
    markers: &[Marker],
    view_projection: &Mat4,
    viewport: (u32, u32),
    eye: Vec3,
    scale: f32,
) {
    for marker in markers {
        if (marker.position - eye).length() > MARKER_MAX_DISTANCE {
            continue;
        }
        // `project` returns `None` for anything behind the camera, which is what keeps a
        // label from appearing mirrored on the far side of the screen.
        let Some((x, y)) = project(marker.position, view_projection, viewport) else {
            continue;
        };
        list.text_aligned((x, y), marker.text, MARKER_COLOR, scale, TextAlign::Center);
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::command::PlayerCommand;
    use engine_game::demo;

    fn readout(station: Option<Station>) -> Vec<String> {
        let (mut simulation, scene) = demo::collision_demo();
        simulation.step_many(30);
        let mut lines = Vec::new();
        write_lines(
            &mut lines,
            &CollisionHudModel {
                simulation: &simulation,
                scene: &scene,
                station,
                debug_entities: 42,
            },
        );
        lines
    }

    #[test]
    fn the_readout_names_every_thing_the_milestone_asks_for() {
        let lines = readout(None);
        let text = lines.join("\n");
        for expected in [
            "COLLISION DEMO",
            "POS",
            "VEL",
            "GROUNDED",
            "NORMAL",
            "SLOPE",
            "SURFACE",
            "CAPSULE",
            "RADIUS",
            "HEIGHT",
            "STEP",
            "BROAD",
            "CANDIDATES",
            "NARROW",
            "TESTS",
            "CONTACTS",
            "STATICS",
        ] {
            assert!(
                text.contains(expected),
                "the readout never says {expected}:\n{text}"
            );
        }
    }

    #[test]
    fn the_selected_station_and_its_instruction_are_shown() {
        let (_, scene) = demo::collision_demo();
        let station = scene.station(4).expect("station 4 is the slopes");
        let text = readout(Some(station)).join("\n");
        assert!(text.contains("STATION 4"), "{text}");
        assert!(text.contains("SLOPES"), "{text}");
        assert!(text.contains(station.instruction), "{text}");
    }

    #[test]
    fn every_character_has_a_glyph_in_the_developer_font() {
        // The overlay font is 5x7 uppercase. A lowercase letter or an accent draws as a
        // hole, and a hole in a debug readout is a number that cannot be read.
        for station in demo::STATIONS {
            let text = readout(Some(station)).join("\n");
            for character in text.chars() {
                assert!(
                    character.is_ascii_uppercase()
                        || character.is_ascii_digit()
                        || " -+.,:/()[]<>%*=\n".contains(character),
                    "{character:?} has no glyph, in:\n{text}"
                );
            }
        }
    }

    #[test]
    fn the_surface_under_the_character_is_named() {
        let (mut simulation, scene) = demo::collision_demo();
        simulation.step_many(30);
        let mut lines = Vec::new();
        write_lines(
            &mut lines,
            &CollisionHudModel {
                simulation: &simulation,
                scene: &scene,
                station: None,
                debug_entities: 0,
            },
        );
        let text = lines.join("\n");
        assert!(
            text.contains("PLATFORM"),
            "the character is standing on the platform:\n{text}"
        );
    }

    #[test]
    fn the_readout_shrinks_when_there_is_less_to_say() {
        // A shorter readout must not leave the previous one's last lines on screen.
        let long = readout(Some(demo::STATIONS[0])).len();
        let short = readout(None).len();
        assert!(long >= short);

        let (mut simulation, scene) = demo::collision_demo();
        let mut lines = Vec::new();
        let mut model = CollisionHudModel {
            simulation: &simulation,
            scene: &scene,
            station: Some(demo::STATIONS[0]),
            debug_entities: 0,
        };
        write_lines(&mut lines, &model);
        let with_station = lines.len();
        model.station = None;
        write_lines(&mut lines, &model);
        assert_eq!(lines.len(), with_station, "both forms are the same height");

        simulation.push_command(PlayerCommand::Teleport {
            position: Vec3::new(0.0, 0.1, 0.0),
        });
        simulation.step_once();
    }

    #[test]
    fn rebuilding_the_readout_reuses_its_buffers() {
        let (mut simulation, scene) = demo::collision_demo();
        let mut lines = Vec::new();
        for _ in 0..10 {
            simulation.step_once();
            write_lines(
                &mut lines,
                &CollisionHudModel {
                    simulation: &simulation,
                    scene: &scene,
                    station: None,
                    debug_entities: 0,
                },
            );
        }
        let capacities: Vec<usize> = lines.iter().map(String::capacity).collect();

        for _ in 0..200 {
            simulation.step_once();
            write_lines(
                &mut lines,
                &CollisionHudModel {
                    simulation: &simulation,
                    scene: &scene,
                    station: None,
                    debug_entities: 0,
                },
            );
        }
        assert_eq!(
            lines.iter().map(String::capacity).collect::<Vec<_>>(),
            capacities,
            "a rebuild grew a line buffer"
        );
    }

    #[test]
    fn markers_are_drawn_only_where_they_are_visible_and_near() {
        let (_, scene) = demo::collision_demo();
        let mut list = UiDrawList::new();
        let eye = Vec3::new(0.0, 3.0, 12.0);
        let view_projection = Mat4::perspective(1.0, 16.0 / 9.0, 0.1, 500.0)
            * Mat4::look_at(eye, Vec3::new(0.0, 1.0, 0.0), Vec3::Y);

        push_markers(
            &mut list,
            &scene.markers,
            &view_projection,
            (1920, 1080),
            eye,
            2.0,
        );
        let near = list.len();
        assert!(near > 0, "nothing was drawn for the station in front");
        assert!(
            near < scene.markers.len(),
            "the whole course was drawn from one station: {near} of {}",
            scene.markers.len()
        );
    }
}
