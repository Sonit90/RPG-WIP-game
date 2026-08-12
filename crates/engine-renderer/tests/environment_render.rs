//! The combat arena, drawn on a real GPU, asserted in pixels.
//!
//! Milestone 20 asks for manual visual verification. Looking at an arena and deciding it is
//! fine proves it was fine once, on one machine, on one afternoon; these tests prove — every
//! run, on any adapter — the things a person would be looking for:
//!
//! * the environment reaches the screen at all, rather than the frame being the clear colour,
//! * there is a sky above and ground below, and they are different,
//! * the scene has depth: the far half of the view is not the near half,
//! * the arena floor is under the player rather than through them or above them,
//! * the boundary is visible from the middle of the arena, so the space reads as enclosed,
//! * and every asset in the library draws something rather than nothing.
//!
//! What they deliberately do **not** assert is that it looks good. That is a judgement, it is
//! recorded in `phase_environment.md`, and no test can hold it.
//!
//! # Why the images are untextured
//!
//! [`render_scene`] binds the shared 1x1 white map, so a material's base colour *texture*
//! does not participate. Every textured environment material has a white base colour, so
//! without help every stone, plank and leaf would come out the same white — and `materials_of`
//! folds each texture's mean texel into the base colour to avoid exactly that. What is
//! asserted here is therefore geometry, placement, depth and lighting, with each surface at
//! the colour it averages to; the textures' own detail is asserted in `engine-assets`, where
//! it is generated.
//!
//! Every test skips itself when no D3D12 adapter is present.

use engine_assets::procedural::{environment_library, environment_model};
use engine_assets::{Geometry, ModelAsset};
use engine_core::math::Vec3;
use engine_core::transform::Transform;
use engine_environment::{EnvironmentScene, arena, asset as ids};
use engine_render_data::{Camera, Lighting, Projection};
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::{MeshData, MeshVertex};
use engine_renderer::offscreen::{CapturedImage, SceneObject, render_scene};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 270;

/// Whether a D3D12 adapter exists. Without one there is nothing to test.
fn adapter_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, [0.0, 0.0, 0.0, 1.0]).is_ok()
}

/// The arena's own light, converted the way the application converts it.
fn lighting() -> Lighting {
    let scene = arena::combat_arena();
    Lighting {
        direction: scene.lighting.sun_direction,
        color: scene.lighting.sun_color,
        intensity: scene.lighting.sun_intensity,
        ambient_color: scene.lighting.ambient_color,
        ambient_intensity: scene.lighting.ambient_intensity,
    }
}

/// The arena's clear colour.
fn clear() -> [f32; 4] {
    arena::combat_arena().lighting.clear_color()
}

/// The gameplay camera's rough pose: behind and above the player, looking down the arena.
fn gameplay_camera() -> Camera {
    let Ok(mut camera) = Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32)) else {
        panic!("a 16:9 perspective is valid");
    };
    let Ok(()) = camera.look_at(
        Vec3::new(0.0, 4.5, 9.0),
        Vec3::new(0.0, 1.2, -14.0),
        Vec3::Y,
    ) else {
        panic!("the camera must look down the arena");
    };
    camera
}

/// One model's LOD0 submeshes, converted to the renderer's vertex layout.
///
/// Returns a mesh and the material index it draws with, per submesh.
fn meshes_of(model: &ModelAsset) -> Vec<(MeshData, usize)> {
    let Some(lod) = model.lod0() else {
        panic!("{} has a LOD0", model.name);
    };
    lod.submeshes
        .iter()
        .filter_map(|submesh| {
            let Geometry::Static(vertices) = &submesh.geometry else {
                return None;
            };
            let vertices = vertices
                .iter()
                .map(|vertex| MeshVertex::new(vertex.position, vertex.normal, vertex.uv))
                .collect();
            match MeshData::new(vertices, submesh.indices.clone()) {
                Ok(mesh) => Some((mesh, submesh.material)),
                Err(error) => panic!("{}'s geometry is invalid: {error}", model.name),
            }
        })
        .collect()
}

/// A model's materials, converted to the renderer's description.
///
/// A material's base colour texture is **folded into its base colour** as the texture's mean
/// texel. [`render_scene`] binds the shared 1x1 white map, and every textured environment
/// material has a white base colour — so without this every stone, plank and leaf would come
/// out the same white and a test asking "is the sky a different colour from the floor?" could
/// only ever fail. The mean is what the surface averages to at a distance, which is exactly
/// the property these tests are about.
fn materials_of(model: &ModelAsset) -> Vec<MaterialDesc> {
    model
        .materials
        .iter()
        .map(|material| {
            let tint = material
                .base_color_texture
                .and_then(|index| model.textures.get(index))
                .map_or([1.0, 1.0, 1.0], mean_texel);
            MaterialDesc::colored(
                material.base_color.x * tint[0],
                material.base_color.y * tint[1],
                material.base_color.z * tint[2],
            )
            .with_roughness(material.roughness)
            .with_metallic(material.metallic)
            .with_emissive(material.emissive)
        })
        .collect()
}

/// The mean colour of a texture, in 0..1 per channel.
fn mean_texel(texture: &engine_assets::TextureAsset) -> [f32; 3] {
    let mut total = [0.0_f64; 3];
    let mut count = 0.0;
    for texel in texture.texels.chunks_exact(4) {
        for channel in 0..3 {
            total[channel] += f64::from(texel[channel]);
        }
        count += 1.0;
    }
    if count == 0.0 {
        return [1.0, 1.0, 1.0];
    }
    [
        (total[0] / count / 255.0) as f32,
        (total[1] / count / 255.0) as f32,
        (total[2] / count / 255.0) as f32,
    ]
}

/// Everything a scene needs on the GPU: one mesh set and one material set per asset.
///
/// Built once and borrowed by the `SceneObject` list, because `SceneObject` holds a
/// `&MeshData` and the meshes have to outlive it.
struct Library {
    assets: Vec<engine_environment::EnvironmentAsset>,
    meshes: Vec<Vec<(MeshData, usize)>>,
    materials: Vec<Vec<MaterialDesc>>,
}

impl Library {
    fn build() -> Self {
        let mut assets = Vec::new();
        let mut meshes = Vec::new();
        let mut materials = Vec::new();
        for (asset, model) in environment_library() {
            assets.push(asset);
            meshes.push(meshes_of(&model));
            materials.push(materials_of(&model));
        }
        Self {
            assets,
            meshes,
            materials,
        }
    }

    fn index_of(&self, asset: engine_environment::EnvironmentAsset) -> Option<usize> {
        self.assets.iter().position(|known| *known == asset)
    }

    /// The draw list for `scene`, at LOD0 throughout.
    ///
    /// LOD0 everywhere rather than by distance, because these tests are about whether the
    /// arena is drawn correctly and not about which level a treeline picks. Level selection
    /// has its own tests, in `engine-presentation`, where it does not need a device.
    fn objects<'a>(&'a self, scene: &EnvironmentScene) -> Vec<SceneObject<'a>> {
        let mut objects = Vec::new();
        for placement in &scene.objects {
            let Some(index) = self.index_of(placement.asset) else {
                panic!("{} is not in the library", placement.asset);
            };
            for (mesh, material) in &self.meshes[index] {
                objects.push(SceneObject {
                    mesh,
                    material: self.materials[index][*material],
                    transform: placement.transform,
                });
            }
        }
        objects
    }
}

/// Mean luminance of a rectangle of the image, in 0..255.
fn mean_luminance(image: &CapturedImage, x0: u32, y0: u32, x1: u32, y1: u32) -> f64 {
    let mut total = 0.0;
    let mut count = 0.0;
    for y in y0..y1.min(image.height) {
        for x in x0..x1.min(image.width) {
            let Some(pixel) = image.pixel(x, y) else {
                continue;
            };
            total += 0.2126 * f64::from(pixel[0])
                + 0.7152 * f64::from(pixel[1])
                + 0.0722 * f64::from(pixel[2]);
            count += 1.0;
        }
    }
    if count == 0.0 { 0.0 } else { total / count }
}

/// Mean colour of a rectangle of the image, in 0..255 per channel.
fn mean_color(image: &CapturedImage, x0: u32, y0: u32, x1: u32, y1: u32) -> [f64; 3] {
    let mut total = [0.0; 3];
    let mut count = 0.0;
    for y in y0..y1.min(image.height) {
        for x in x0..x1.min(image.width) {
            let Some(pixel) = image.pixel(x, y) else {
                continue;
            };
            for channel in 0..3 {
                total[channel] += f64::from(pixel[channel]);
            }
            count += 1.0;
        }
    }
    if count == 0.0 {
        return [0.0; 3];
    }
    [total[0] / count, total[1] / count, total[2] / count]
}

/// How many distinct quantized colours the image holds.
///
/// Quantized to five bits a channel, so shading gradients on one surface do not each count
/// as a separate colour. A frame with a handful of distinct colours is a frame with a handful
/// of things in it, whatever else it may be.
fn distinct_colors(image: &CapturedImage) -> usize {
    let mut seen = std::collections::HashSet::new();
    for y in 0..image.height {
        for x in 0..image.width {
            let Some(pixel) = image.pixel(x, y) else {
                continue;
            };
            seen.insert((pixel[0] >> 3, pixel[1] >> 3, pixel[2] >> 3));
        }
    }
    seen.len()
}

/// Renders the combat arena from the gameplay camera.
fn render_arena(library: &Library, scene: &EnvironmentScene, camera: &Camera) -> CapturedImage {
    let objects = library.objects(scene);
    match render_scene(WIDTH, HEIGHT, clear(), &objects, camera, lighting()) {
        Ok(image) => image,
        Err(error) => panic!("the arena must render: {error}"),
    }
}

#[test]
fn the_arena_reaches_the_screen_rather_than_the_clear_colour() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let library = Library::build();
    let scene = arena::combat_arena();
    let image = render_arena(&library, &scene, &gameplay_camera());

    // The clear colour is the arena's, darkened. If the frame were still it, nothing drew.
    let clear = clear();
    let expected = [
        (clear[0] * 255.0) as i32,
        (clear[1] * 255.0) as i32,
        (clear[2] * 255.0) as i32,
    ];
    let cleared = image.count_pixels(|pixel| {
        (i32::from(pixel[0]) - expected[0]).abs() < 6
            && (i32::from(pixel[1]) - expected[1]).abs() < 6
            && (i32::from(pixel[2]) - expected[2]).abs() < 6
    });
    let total = (WIDTH * HEIGHT) as usize;
    assert!(
        cleared * 20 < total,
        "{cleared} of {total} pixels are still the clear colour: the arena did not draw"
    );

    // And it is a scene rather than a wash: a frame of one surface has a handful of
    // quantized colours, and this one has to have many.
    let colors = distinct_colors(&image);
    assert!(colors > 40, "only {colors} distinct colours in the frame");
}

#[test]
fn there_is_a_sky_above_and_ground_below_and_they_are_not_the_same() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let library = Library::build();
    let scene = arena::combat_arena();
    let image = render_arena(&library, &scene, &gameplay_camera());

    // The top eighth of the frame from a camera looking slightly downwards is sky; the
    // bottom eighth is the floor a few metres in front of the player.
    let sky = mean_color(&image, 0, 0, WIDTH, HEIGHT / 8);
    let ground = mean_color(&image, 0, HEIGHT * 7 / 8, WIDTH, HEIGHT);

    assert!(
        sky[0] + sky[1] + sky[2] > 30.0,
        "the sky is black: {sky:?} — the dome is missing or wound the wrong way"
    );
    assert!(
        ground[0] + ground[1] + ground[2] > 30.0,
        "the ground is black: {ground:?}"
    );
    // Different enough that a horizon exists. Without this a uniformly grey frame would
    // pass every other assertion in this file.
    let difference: f64 = (0..3).map(|c| (sky[c] - ground[c]).abs()).sum();
    assert!(
        difference > 24.0,
        "the sky and the ground are the same colour: {sky:?} vs {ground:?}"
    );
}

#[test]
fn the_floor_is_under_the_camera_rather_than_through_it_or_above_it() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let library = Library::build();
    let scene = arena::combat_arena();

    // Looking straight down from head height at the player's feet. Whatever is there is the
    // floor, and it must be lit rather than black — a floor drawn from below, or one the
    // camera is inside, comes out as the clear colour.
    let Ok(mut camera) = Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32)) else {
        panic!("a 16:9 perspective is valid");
    };
    let Ok(()) = camera.look_at(
        Vec3::new(0.0, 1.7, 0.0),
        Vec3::new(0.01, 0.0, 0.01),
        Vec3::Y,
    ) else {
        panic!("the camera must look down");
    };

    let image = render_arena(&library, &scene, &camera);
    let middle = mean_luminance(&image, WIDTH / 3, HEIGHT / 3, WIDTH * 2 / 3, HEIGHT * 2 / 3);
    assert!(
        middle > 20.0,
        "there is no lit floor under the player: mean luminance {middle}"
    );
}

#[test]
fn the_boundary_is_visible_from_the_middle_of_the_arena() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let library = Library::build();
    let full = arena::combat_arena();

    // The same view twice: once with the whole arena, once with the boundary taken out. If
    // the kerb, the pillars and the arch contribute nothing, the two frames are the same and
    // the arena does not read as an enclosed space.
    let mut bare = full.clone();
    bare.objects.retain(|object| {
        object.asset != ids::WALL_SEGMENT
            && object.asset != ids::PILLAR
            && object.asset != ids::ARCH
            && object.asset != ids::BANNER
    });

    let Ok(mut camera) = Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32)) else {
        panic!("a 16:9 perspective is valid");
    };
    // From the middle, looking at the gate.
    let Ok(()) = camera.look_at(
        Vec3::new(0.0, 3.0, 6.0),
        Vec3::new(0.0, 3.0, -40.0),
        Vec3::Y,
    ) else {
        panic!("the camera must look at the gate");
    };

    let with = render_arena(&library, &full, &camera);
    let without = render_arena(&library, &bare, &camera);

    let mut changed = 0;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (Some(a), Some(b)) = (with.pixel(x, y), without.pixel(x, y)) else {
                continue;
            };
            if (0..3).any(|c| (i32::from(a[c]) - i32::from(b[c])).abs() > 8) {
                changed += 1;
            }
        }
    }
    let total = (WIDTH * HEIGHT) as usize;
    assert!(
        changed * 200 > total,
        "the boundary changes {changed} of {total} pixels: it is not visible from the fight"
    );
}

#[test]
fn the_scene_has_depth_rather_than_being_a_backdrop() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let library = Library::build();
    let full = arena::combat_arena();

    // Everything past the arena wall — the treeline, the scatter and the ridges — removed.
    // A scene whose background contributes nothing is a scene with no depth, which is the
    // one thing the phase brief asks the background to provide.
    let mut near_only = full.clone();
    near_only.objects.retain(|object| {
        let position = object.position();
        position.x.hypot(position.z) < arena::ARENA_HALF_EXTENT + 2.0
    });

    let camera = gameplay_camera();
    let with = render_arena(&library, &full, &camera);
    let without = render_arena(&library, &near_only, &camera);

    let mut changed = 0;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (Some(a), Some(b)) = (with.pixel(x, y), without.pixel(x, y)) else {
                continue;
            };
            if (0..3).any(|c| (i32::from(a[c]) - i32::from(b[c])).abs() > 8) {
                changed += 1;
            }
        }
    }
    assert!(
        changed > 400,
        "the background changes only {changed} pixels: there is nothing behind the arena"
    );
}

#[test]
fn every_asset_in_the_library_draws_something() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    // One asset at a time against a black clear, framed to its own size. An asset that is
    // wound inside-out, degenerate or entirely below the ground draws nothing, and nothing
    // is exactly what an empty space in an arena looks like.
    let black = [0.0, 0.0, 0.0, 1.0];
    for asset in ids::LIBRARY {
        // The sky is a shell four hundred metres across and the plain is a unit quad the
        // scene scales; neither is framed by a camera pointed at a prop, and both have their
        // own assertions in `engine-assets`.
        if asset == ids::SKY_DOME || asset == ids::GROUND_PLAIN {
            continue;
        }
        let model = environment_model(asset);
        let meshes = meshes_of(&model);
        let materials = materials_of(&model);
        assert!(!meshes.is_empty(), "{asset} has no rigid geometry");

        let Some(lod) = model.lod0() else {
            panic!("{asset} has a LOD0");
        };
        let bounds = lod.bounds();
        let distance = (bounds.radius * 3.0).max(2.0);

        let Ok(mut camera) = Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32))
        else {
            panic!("a 16:9 perspective is valid");
        };
        let Ok(()) = camera.look_at(
            bounds.center + Vec3::new(distance * 0.7, bounds.radius * 0.6, distance * 0.7),
            bounds.center,
            Vec3::Y,
        ) else {
            panic!("the camera must look at {asset}");
        };

        let objects: Vec<SceneObject<'_>> = meshes
            .iter()
            .map(|(mesh, material)| SceneObject {
                mesh,
                material: materials[*material],
                transform: Transform::IDENTITY,
            })
            .collect();
        let image = match render_scene(WIDTH, HEIGHT, black, &objects, &camera, lighting()) {
            Ok(image) => image,
            Err(error) => panic!("{asset} must render: {error}"),
        };

        let lit = image.count_pixels(|pixel| pixel[0] > 8 || pixel[1] > 8 || pixel[2] > 8);
        assert!(
            lit > 200,
            "{asset} covers only {lit} pixels: it is inside out, degenerate or underground"
        );
    }
}

#[test]
fn the_environment_demo_shows_more_than_the_combat_arena_does() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let library = Library::build();
    // Looking down the demo row, which stands past the combat space on the +Z side.
    let Ok(mut camera) = Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32)) else {
        panic!("a 16:9 perspective is valid");
    };
    let Ok(()) = camera.look_at(
        Vec3::new(-26.0, 5.0, 22.0),
        Vec3::new(10.0, 2.0, 31.0),
        Vec3::Y,
    ) else {
        panic!("the camera must look down the demo row");
    };

    let arena_image = render_arena(&library, &arena::combat_arena(), &camera);
    let demo_image = render_arena(&library, &arena::environment_demo(), &camera);

    let mut changed = 0;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (Some(a), Some(b)) = (arena_image.pixel(x, y), demo_image.pixel(x, y)) else {
                continue;
            };
            if (0..3).any(|c| (i32::from(a[c]) - i32::from(b[c])).abs() > 8) {
                changed += 1;
            }
        }
    }
    assert!(
        changed > 2_000,
        "the demo row changes only {changed} pixels: it is not where the camera is looking"
    );
}
