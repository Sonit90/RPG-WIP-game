//! A character the camera looks through: the world shows through it, and nothing of the
//! character's own far side does.
//!
//! ```text
//!   backdrop (green)    character at 50 % (red)     what reaches the screen
//!   ################    ......RRRR......            ......YYYY......   one blended layer,
//!   ################    ....RRRRRRRR....            ....YYYYYYYY....   the same mix in
//!   ################    ......RRRR......            ......YYYY......   every pixel
//! ```
//!
//! Drawn through the same pipeline variants the window uses for it — a depth-only prime and a
//! blended pass that tests against it — with flat lighting, so each pixel's colour is a
//! mixing ratio that can be read back: half red and half green is one layer of character over
//! the world; three quarters red would be the character blended over *itself*, which is the
//! x-ray look this exists to avoid.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_assets::procedural::fire_mage;
use engine_assets::{Geometry, ModelAsset};
use engine_core::math::{Mat4, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{Camera, Lighting, Projection};
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::{MeshData, SkinnedMeshData, SkinnedVertex};
use engine_renderer::offscreen::{
    CapturedImage, SceneObject, SkinnedObject, render_translucent_characters,
};
use engine_renderer::transparency::upright_quad;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 120;
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

fn adapter_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, BLACK).is_ok()
}

/// From the front and a little above, as the game's camera looks at the character.
fn camera() -> Camera {
    let mut camera = Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32))
        .expect("a valid projection");
    camera
        .look_at(Vec3::new(0.0, 1.1, 4.2), Vec3::new(0.0, 0.95, 0.0), Vec3::Y)
        .expect("the camera can look at the character");
    camera
}

/// Ambient only: a surface's colour is its base colour, whichever way it faces.
fn flat_lighting() -> Lighting {
    Lighting {
        ambient_intensity: 1.0,
        intensity: 0.0,
        ..Lighting::default()
    }
}

fn mage_meshes(model: &ModelAsset) -> Vec<SkinnedMeshData> {
    let lod = model.lod0().expect("the FireMage has a LOD0");
    lod.submeshes
        .iter()
        .filter_map(|submesh| {
            let Geometry::Skinned(vertices) = &submesh.geometry else {
                return None;
            };
            let vertices = vertices
                .iter()
                .map(|vertex| {
                    SkinnedVertex::new(
                        vertex.position,
                        vertex.normal,
                        vertex.uv,
                        vertex.joints,
                        vertex.weights,
                    )
                })
                .collect();
            Some(SkinnedMeshData::new(vertices, submesh.indices.clone()).expect("valid geometry"))
        })
        .collect()
}

/// The FireMage, in red, at `opacity`, in front of a green wall.
fn render(opacity: f32) -> CapturedImage {
    let model = fire_mage().expect("the FireMage must build");
    let skeleton = model.skeleton.as_ref().expect("a skeleton");
    let mut palette: Vec<Mat4> = Vec::new();
    skeleton
        .skinning_matrices(&skeleton.rest_pose(), &mut palette)
        .expect("the rest pose skins");
    let meshes = mage_meshes(&model);
    let characters: Vec<SkinnedObject<'_>> = meshes
        .iter()
        .map(|mesh| SkinnedObject {
            mesh,
            material: MaterialDesc::colored(1.0, 0.0, 0.0),
            transform: Transform::IDENTITY,
            palette: &palette,
        })
        .collect();
    let wall: MeshData = upright_quad(20.0);
    let backdrop = [SceneObject {
        mesh: &wall,
        material: MaterialDesc::colored(0.0, 1.0, 0.0),
        transform: Transform::from_translation(Vec3::new(0.0, 1.0, -1.5)),
    }];
    render_translucent_characters(
        WIDTH,
        HEIGHT,
        BLACK,
        &backdrop,
        &characters,
        opacity,
        &camera(),
        flat_lighting(),
    )
    .expect("the frame must render")
}

/// The pixels the character covers, from a whole render of it.
fn silhouette() -> Vec<(u32, u32)> {
    let whole = render(1.0);
    let mut pixels = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let pixel = whole.pixel(x, y).expect("inside");
            if pixel[0] > 200 && pixel[1] < 40 {
                pixels.push((x, y));
            }
        }
    }
    pixels
}

#[test]
fn the_wall_shows_through_the_character_as_one_even_layer() {
    let _gpu = gpu_lock();
    if !adapter_available() {
        return;
    }
    let shape = silhouette();
    assert!(
        shape.len() > 300,
        "the character covers {} pixels",
        shape.len()
    );

    let image = render(0.5);
    let mut uneven = 0;
    for &(x, y) in &shape {
        let pixel = image.pixel(x, y).expect("inside");
        // Half red over half green, everywhere: one layer of character over the wall. The
        // character over itself would be three quarters red and a quarter green; the wall
        // hidden would be no green at all.
        let (red, green) = (i32::from(pixel[0]), i32::from(pixel[1]));
        if (red - 128).abs() > 12 || (green - 128).abs() > 12 {
            uneven += 1;
        }
    }
    assert!(
        uneven * 100 < shape.len(),
        "{uneven} of {} character pixels were not an even half-and-half blend",
        shape.len()
    );
}

#[test]
fn the_fade_runs_from_solid_to_gone() {
    let _gpu = gpu_lock();
    if !adapter_available() {
        return;
    }
    let shape = silhouette();
    let green_at = |opacity: f32| {
        let image = render(opacity);
        shape
            .iter()
            .map(|&(x, y)| f32::from(image.pixel(x, y).expect("inside")[1]))
            .sum::<f32>()
            / shape.len() as f32
    };
    let solid = green_at(1.0);
    let mostly = green_at(0.8);
    let faint = green_at(0.2);
    let gone = green_at(0.0);
    assert!(solid < 5.0, "a whole character hides the wall: {solid}");
    assert!(
        (mostly - 51.0).abs() < 8.0,
        "80 % lets a fifth through: {mostly}"
    );
    assert!(
        (faint - 204.0).abs() < 8.0,
        "20 % lets four fifths through: {faint}"
    );
    assert!(gone > 250.0, "an invisible character is not drawn: {gone}");
}
