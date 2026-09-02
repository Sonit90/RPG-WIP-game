//! The ground-targeting indicator, drawn on a real GPU over real terrain, asserted in pixels.
//!
//! The unit tests in `engine_presentation::ui::ground_decal` prove where every piece of the
//! outline *goes*. They cannot prove what the frame looks like, and three of the four defects
//! this feature went through were only visible in a frame:
//!
//! * a wide band cut into long pieces rode above and below the ground the border was
//!   following, and visibly parted company with it,
//! * carrying the indicator's colour in the *base* colour instead of the emissive term made
//!   every piece shade differently as it tilted, so a ring on rough ground was a ring of
//!   facets,
//! * and quads meeting at an angle left a wedge of bare ground on the outside of every
//!   corner until the joints were mitered.
//!
//! So these tests render the thing and measure the pixels. They use
//! [`render_surface_scene`], which runs the renderer's own opaque and transparent phases with
//! its own back-to-front sort, so what is measured is the path the window takes.
//!
//! Every test skips itself when no D3D12 adapter is present.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::{Quat, Vec2, Vec3};
use engine_core::transform::Transform;
use engine_environment::terrain::Terrain;
use engine_presentation::ui::ground_decal::{
    DecalBand, DecalScratch, GroundSurface, MAX_SEGMENTS, build_band,
};
use engine_presentation::ui::targeting::TargetShape;
use engine_render_data::{Camera, Lighting, Projection};
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::{MeshData, MeshVertex};
use engine_renderer::offscreen::{CapturedImage, SceneObject, render_surface_scene};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 400;

/// The app's own indicator numbers, so this measures what the game draws.
///
/// Duplicated from `app.rs` rather than exported: they are the application's presentation
/// choice, and a test that read them from the binary could not fail when they changed to
/// something unreadable.
const BORDER_WIDTH: f32 = 0.28;
const BORDER_LIFT: f32 = 0.05;
const GLOW_WIDTH: f32 = 0.85;
const GLOW_LIFT: f32 = 0.035;

/// Whether a D3D12 adapter exists. Without one there is nothing to test.
fn adapter_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, [0.0, 0.0, 0.0, 1.0]).is_ok()
}

/// Ground built from a closure of world X and Z, on a three-quarter-metre grid.
fn terrain_from(height: impl Fn(f32, f32) -> f32) -> Terrain {
    let mut terrain = Terrain::flat(97, 0.75, 0.0);
    for z in 0..terrain.resolution {
        for x in 0..terrain.resolution {
            let world_x = terrain.origin.x + x as f32 * terrain.cell_size;
            let world_z = terrain.origin.z + z as f32 * terrain.cell_size;
            terrain.heights[z * terrain.resolution + x] = height(world_x, world_z);
        }
    }
    terrain
}

/// The height field as a drawable grid, so the capture has real ground in it.
fn terrain_mesh(terrain: &Terrain) -> MeshData {
    let side = terrain.resolution;
    let mut vertices = Vec::with_capacity(side * side);
    for z in 0..side {
        for x in 0..side {
            let position = terrain.position_at_point(x, z);
            let normal = terrain.normal_at(position);
            vertices.push(MeshVertex::new(
                position,
                normal,
                Vec2::new(x as f32, z as f32),
            ));
        }
    }
    let mut indices = Vec::with_capacity((side - 1) * (side - 1) * 6);
    for z in 0..side - 1 {
        for x in 0..side - 1 {
            let base = (z * side + x) as u32;
            let right = base + 1;
            let down = base + side as u32;
            let diagonal = down + 1;
            indices.extend_from_slice(&[base, down, right, right, down, diagonal]);
        }
    }
    MeshData::new(vertices, indices).expect("a terrain grid is a valid mesh")
}

/// The app's border material: the whole colour in the emissive term.
fn border_material() -> MaterialDesc {
    MaterialDesc::colored(0.02, 0.013, 0.005)
        .with_roughness(1.0)
        .with_emissive(Vec3::new(1.15, 0.60, 0.17))
}

/// The app's translucent band: the same, at a low opacity.
fn glow_material() -> MaterialDesc {
    MaterialDesc::colored(0.01, 0.006, 0.002)
        .with_roughness(1.0)
        .with_emissive(Vec3::new(0.95, 0.48, 0.13))
        .with_opacity(0.22)
}

/// The two bands of the indicator for `shape`, laid on `terrain`.
fn bands(
    terrain: &Terrain,
    shape: TargetShape,
    anchor: Vec3,
    facing: f32,
) -> (Vec<Transform>, Vec<Transform>) {
    let ground = GroundSurface::Terrain(terrain);
    let mut scratch = DecalScratch::new();
    let mut border = Vec::new();
    let mut glow = Vec::new();
    build_band(
        shape,
        anchor,
        facing,
        DecalBand::edged(BORDER_WIDTH, BORDER_LIFT),
        &ground,
        &mut scratch,
        &mut border,
    );
    build_band(
        shape,
        anchor,
        facing,
        DecalBand::edged(GLOW_WIDTH, GLOW_LIFT).with_inset(BORDER_WIDTH + GLOW_WIDTH * 0.5),
        &ground,
        &mut scratch,
        &mut glow,
    );
    (border, glow)
}

/// A camera at `eye` looking at `target`, the way the game builds one.
fn camera_at(eye: Vec3, target: Vec3) -> Camera {
    let mut camera =
        Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32)).expect("a projection");
    camera.set_position(eye);
    let forward = (target - eye).normalize();
    camera.set_rotation(Quat::from_yaw_pitch_roll(
        (-forward.x).atan2(-forward.z),
        forward.y.asin(),
        0.0,
    ));
    camera
}

fn lighting() -> Lighting {
    Lighting {
        direction: Vec3::new(-0.4, -0.8, -0.45).normalize(),
        color: Vec3::new(1.0, 0.96, 0.88),
        intensity: 1.8,
        ambient_color: Vec3::new(0.5, 0.56, 0.66),
        ambient_intensity: 0.35,
    }
}

/// Renders `terrain`, and the indicator over it when `with_indicator`.
///
/// The pair is what makes "the interior is see-through" measurable: the same frame with and
/// without the indicator, compared pixel for pixel.
fn capture(
    terrain: &Terrain,
    pieces: &(Vec<Transform>, Vec<Transform>),
    eye: Vec3,
    target: Vec3,
    with_indicator: bool,
) -> CapturedImage {
    let ground_mesh = terrain_mesh(terrain);
    let quad = MeshData::plane(1.0);
    let mut objects = vec![SceneObject {
        mesh: &ground_mesh,
        material: MaterialDesc::colored(0.34, 0.40, 0.26).with_roughness(0.9),
        transform: Transform::IDENTITY,
    }];
    if with_indicator {
        for piece in &pieces.1 {
            objects.push(SceneObject {
                mesh: &quad,
                material: glow_material(),
                transform: *piece,
            });
        }
        for piece in &pieces.0 {
            objects.push(SceneObject {
                mesh: &quad,
                material: border_material(),
                transform: *piece,
            });
        }
    }
    render_surface_scene(
        WIDTH,
        HEIGHT,
        [0.06, 0.08, 0.11, 1.0],
        &objects,
        &camera_at(eye, target),
        lighting(),
    )
    .expect("the offscreen scene renders")
}

/// Whether a pixel is the indicator's border: strongly warm and bright.
///
/// The border is emissive orange and the ground is desaturated green, so "much more red than
/// green" separates them without depending on the exact shade of either.
fn is_border(pixel: [u8; 4]) -> bool {
    let (red, green, blue) = (pixel[0] as i32, pixel[1] as i32, pixel[2] as i32);
    red > 150 && red - green > 40 && green > blue
}

/// The rough ground every test uses, and a camera looking down at it.
fn rough_scene() -> (Terrain, Vec3, Vec3) {
    (
        terrain_from(|x, z| (x * 1.1).sin() * 0.45 + (z * 0.9).cos() * 0.35),
        Vec3::new(0.0, 7.0, 15.0),
        Vec3::ZERO,
    )
}

#[test]
fn the_border_reaches_the_screen_over_rough_ground() {
    if !adapter_available() {
        return;
    }
    let (terrain, eye, target) = rough_scene();
    let pieces = bands(
        &terrain,
        TargetShape::Circle { radius: 8.0 },
        Vec3::ZERO,
        0.0,
    );
    let with = capture(&terrain, &pieces, eye, target, true);

    let border = with.count_pixels(is_border);
    assert!(
        border > 1500,
        "only {border} border pixels: the outline is not on the screen",
    );
}

#[test]
fn the_interior_stays_terrain_while_the_outline_reads_clearly() {
    // The brief's first requirement, as a measurement: the area the indicator marks must
    // still show the ground. The previous opaque disc covered every pixel of it, so it fails
    // this and the transparent band passes it.
    if !adapter_available() {
        return;
    }
    let (terrain, eye, target) = rough_scene();
    let pieces = bands(
        &terrain,
        TargetShape::Circle { radius: 8.0 },
        Vec3::ZERO,
        0.0,
    );
    let without = capture(&terrain, &pieces, eye, target, false);
    let with = capture(&terrain, &pieces, eye, target, true);

    // Every pixel the indicator touched at all, and every pixel it replaced outright.
    let mut touched = 0_usize;
    let mut covered = 0_usize;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let before = without.pixel(x, y).expect("in bounds");
            let after = with.pixel(x, y).expect("in bounds");
            if before == after {
                continue;
            }
            touched += 1;
            let difference = (0..3)
                .map(|channel| (i32::from(after[channel]) - i32::from(before[channel])).abs())
                .max()
                .unwrap_or(0);
            if difference > 90 {
                covered += 1;
            }
        }
    }

    assert!(
        touched > 5000,
        "the indicator changed only {touched} pixels"
    );
    // Most of what the indicator touches it merely tints. A filled disc would invert this
    // ratio, because every pixel of its interior would be replaced rather than tinted.
    let covered_share = covered as f32 / touched as f32;
    assert!(
        covered_share < 0.45,
        "{:.0}% of the marked area was painted over rather than tinted",
        covered_share * 100.0,
    );
}

#[test]
fn the_border_is_one_flat_colour_however_the_ground_tilts() {
    // The faceting defect, as a measurement. With the colour in the base term instead of the
    // emissive one, each piece's tilt changed its lit contribution and the ring came out as
    // a chain of visibly different shades; the spread below was far wider than this bound.
    if !adapter_available() {
        return;
    }
    let (terrain, eye, target) = rough_scene();
    let pieces = bands(
        &terrain,
        TargetShape::Circle { radius: 8.0 },
        Vec3::ZERO,
        0.0,
    );
    let with = capture(&terrain, &pieces, eye, target, true);

    // The brightest pixels only: an anti-aliased edge blends towards the ground and would
    // otherwise be counted as a shade of the border rather than a mix with the terrain.
    let mut reds = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let pixel = with.pixel(x, y).expect("in bounds");
            if is_border(pixel) && pixel[0] > 220 {
                reds.push(pixel[1]);
            }
        }
    }
    assert!(reds.len() > 800, "only {} core border pixels", reds.len());
    reds.sort_unstable();
    // The middle 98%, so a handful of blend pixels cannot decide the result.
    let low = reds[reds.len() / 100];
    let high = reds[reds.len() - 1 - reds.len() / 100];
    assert!(
        u32::from(high - low) < 24,
        "the border's green channel ran from {low} to {high}: it is being shaded per piece",
    );
}

#[test]
fn every_shape_stays_inside_its_piece_budget_on_rough_ground() {
    // The cost of the feature, stated. Both bands are cut at the same fine step, so this is
    // the number that decides how many blended draws a frame of aiming adds.
    let terrain = terrain_from(|x, z| (x * 1.1).sin() * 0.45 + (z * 0.9).cos() * 0.35);
    for (name, shape) in [
        ("meteor", TargetShape::Circle { radius: 5.0 }),
        ("cataclysm", TargetShape::Circle { radius: 10.0 }),
        (
            "cone",
            TargetShape::Cone {
                range: 12.0,
                half_angle: 0.6,
            },
        ),
        (
            "firestorm",
            TargetShape::Line {
                width: 10.0,
                length: 20.0,
            },
        ),
    ] {
        let (border, glow) = bands(&terrain, shape, Vec3::ZERO, 0.0);
        assert!(
            border.len() <= MAX_SEGMENTS && glow.len() <= MAX_SEGMENTS,
            "{name}: {} border and {} glow pieces",
            border.len(),
            glow.len(),
        );
        assert!(
            !border.is_empty() && !glow.is_empty(),
            "{name} drew no indicator at all",
        );
    }
}
