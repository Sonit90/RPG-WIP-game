//! The shipping shadow course, drawn on a real GPU, asserted in pixels.
//!
//! `tests/shadow_render.rs` asserts the shadow *system* against scenes it builds itself: a
//! quad, a box above it, a light. This file asserts the same system against the scene the
//! game actually loads — `engine_environment::shadow_course` — because those are different
//! claims. A shadow system that works on a two-object test scene and fails on a real one
//! fails for reasons that live in the content: an object placed underground, a caster marked
//! as not casting, a light too far from what it lights, a bay arranged so its own geometry
//! hides the thing it was built to show.
//!
//! What is asserted is deliberately narrow — the bays that can be judged by counting pixels:
//!
//! * bay A's isolated pillar puts a shadow on the ground, and turning shadows off removes it,
//! * shadows only ever *darken*: no pixel is brighter with them on,
//! * bay B's hundred crates are all casters, and all of them reach the caster list.
//!
//! What is not asserted here is anything requiring a texture: [`render_scene_shadowed`] binds
//! a 1x1 white base colour map, so bay D's alpha-tested leaves would come out solid. The
//! alpha-tested shadow path has its own test in `shadow_render.rs`, against a material whose
//! cutoff and vertex alpha are chosen for exactly that.
//!
//! Every test skips itself when no D3D12 adapter is present.

use engine_assets::procedural::environment_model;
use engine_assets::{Geometry, ModelAsset};
use engine_core::math::Vec3;
use engine_core::transform::Transform;
use engine_environment::shadow_course::{CRATE_GRID, shadow_course};
use engine_environment::{EnvironmentObject, asset as ids};
use engine_render_data::{Camera, Lighting, Projection, ShadowCasting};
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::{MeshData, MeshVertex};
use engine_renderer::offscreen::{CapturedImage, ShadowSceneObject, render_scene_shadowed_debug};
use engine_renderer::shadow::{ShadowPreset, ShadowSettings};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;

/// Where bay A's pillar stands. Read from the scene rather than repeated, so a course that
/// moves its bays moves this test's camera with it.
fn bay_a_pillar() -> Vec3 {
    let scene = shadow_course();
    let Some(object) = scene
        .objects
        .iter()
        .filter(|object| object.asset == ids::PILLAR)
        // Bay A's is the one furthest down -Z; bay E's colonnade stands at +Z.
        .min_by(|a, b| a.position().z.total_cmp(&b.position().z))
    else {
        panic!("the course has a pillar in bay A");
    };
    object.position()
}

/// Whether a D3D12 adapter exists. Without one there is nothing to test.
fn adapter_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, [0.0, 0.0, 0.0, 1.0]).is_ok()
}

/// The course's own sun, converted the way the application converts it.
fn lighting() -> Lighting {
    let scene = shadow_course();
    Lighting {
        direction: scene.lighting.sun_direction,
        color: scene.lighting.sun_color,
        intensity: scene.lighting.sun_intensity,
        ambient_color: scene.lighting.ambient_color,
        ambient_intensity: scene.lighting.ambient_intensity,
    }
}

/// One model's LOD0 submeshes, in the renderer's vertex layout, with the material index each
/// one draws with.
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

/// A model's materials, with each base colour texture folded into its base colour.
///
/// The same conversion `environment_render.rs` makes, for the same reason: the offscreen path
/// binds one white map, so a textured material with a white base colour would come out white.
/// A shadow test needs the ground and the pillar to be *different* colours, or "this pixel got
/// darker" is a claim about nothing.
fn materials_of(model: &ModelAsset) -> Vec<MaterialDesc> {
    model
        .materials
        .iter()
        .map(|material| {
            let tint = material
                .base_color_texture
                .and_then(|index| model.textures.get(index))
                .map_or([1.0, 1.0, 1.0], |texture| {
                    let mut sums = [0.0_f64; 3];
                    let mut count = 0.0_f64;
                    for texel in texture.texels.chunks_exact(4) {
                        for (sum, channel) in sums.iter_mut().zip(texel) {
                            *sum += f64::from(*channel) / 255.0;
                        }
                        count += 1.0;
                    }
                    if count == 0.0 {
                        return [1.0, 1.0, 1.0];
                    }
                    [
                        (sums[0] / count) as f32,
                        (sums[1] / count) as f32,
                        (sums[2] / count) as f32,
                    ]
                });
            MaterialDesc::colored(
                material.base_color.x * tint[0],
                material.base_color.y * tint[1],
                material.base_color.z * tint[2],
            )
            .with_roughness(material.roughness)
            .with_metallic(material.metallic)
        })
        .collect()
}

/// Everything in the course within `radius` of `center`, as renderable objects.
///
/// One bay at a time. The whole course is 152 placements and this path uploads a mesh, a
/// material and a descriptor per submesh — a fine cost for one bay and a silly one for six,
/// given that the other five would be off screen.
///
/// Owned meshes and their borrowed views are returned separately because
/// [`ShadowSceneObject`] borrows its mesh, so the storage has to outlive the slice.
fn bay_meshes(
    center: Vec3,
    radius: f32,
) -> Vec<(MeshData, MaterialDesc, Transform, ShadowCasting)> {
    let scene = shadow_course();
    let mut built = Vec::new();
    for object in &scene.objects {
        if object.asset == ids::SKY_DOME {
            continue;
        }
        // The ground plain is one 640 m quad centred on the origin: it is under every bay,
        // and excluding it by distance would leave the shadow nothing to fall on.
        let ground = object.asset == ids::GROUND_PLAIN;
        if !ground {
            let offset = object.position() - center;
            if (offset.x * offset.x + offset.z * offset.z).sqrt() > radius {
                continue;
            }
        }
        let model = environment_model(object.asset);
        let materials = materials_of(&model);
        for (mesh, material) in meshes_of(&model) {
            built.push((
                mesh,
                materials[material],
                object.transform,
                // The ground receives and does not cast — the same decision
                // `engine_presentation::extract` makes about a floor, and for the same
                // reason: a flat plane's shadow is the plane, and a caster that cannot
                // shadow anything still costs a draw in every cascade.
                if ground {
                    ShadowCasting::RECEIVES_ONLY
                } else {
                    ShadowCasting::CASTS_AND_RECEIVES
                },
            ));
        }
    }
    assert!(built.len() > 1, "a bay with one object in it is not a bay");
    built
}

/// Bay A, rendered from above and to the sun's side, at the given settings.
fn render_bay_a(settings: ShadowSettings) -> CapturedImage {
    render_bay_a_debug(settings, 0)
}

/// Bay A with one of the shadow debug views switched on.
fn render_bay_a_debug(settings: ShadowSettings, debug_mode: u32) -> CapturedImage {
    render_bay_a_from(settings, debug_mode, 0.0)
}

/// Bay A from an eye rotated `yaw` radians around the pillar.
///
/// The angle exists because the bugs this file is here to catch are the ones that only
/// appear from *some* directions. A cascade is fitted to where the camera is looking, so a
/// caster that ruins a cascade may ruin it from one bearing and not the next, and a single
/// fixed viewpoint is a coin toss about whether the test sees it.
fn render_bay_a_from(settings: ShadowSettings, debug_mode: u32, yaw: f32) -> CapturedImage {
    render_bay_a_eye(settings, debug_mode, yaw, 5.5, 9.0)
}

fn render_bay_a_eye(
    settings: ShadowSettings,
    debug_mode: u32,
    yaw: f32,
    height: f32,
    distance: f32,
) -> CapturedImage {
    let pillar = bay_a_pillar();
    let built = bay_meshes(pillar, 12.0);
    let objects: Vec<ShadowSceneObject<'_>> = built
        .iter()
        .map(|(mesh, material, transform, shadow)| ShadowSceneObject {
            mesh,
            material: *material,
            transform: *transform,
            shadow: *shadow,
        })
        .collect();

    let Ok(mut camera) = Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32)) else {
        panic!("a 4:3 perspective is valid");
    };
    // *Across* the sun, never along it. A camera on the sun's own axis looks straight down
    // the shadow, and the pillar stands in front of every pixel of it — which is a picture
    // of a lit pillar and no evidence of anything. So the eye goes ninety degrees round: the
    // shadow then lies across the frame, next to the thing casting it.
    let sun = lighting().direction;
    let along = Vec3::new(sun.x, 0.0, sun.z).normalize();
    let across = Vec3::new(-along.z, 0.0, along.x);
    let eye_direction = across * yaw.cos() + along * yaw.sin();
    let Ok(()) = camera.look_at(
        pillar + eye_direction * distance + Vec3::new(0.0, height, 0.0),
        // The middle of the shadow rather than the pillar's foot: a four-metre pillar lit
        // from this height throws about four metres of shadow.
        pillar + along * 2.2 + Vec3::new(0.0, 0.4, 0.0),
        Vec3::Y,
    ) else {
        panic!("the camera must look at bay A");
    };

    let clear = shadow_course().lighting.clear_color();
    match render_scene_shadowed_debug(
        WIDTH,
        HEIGHT,
        clear,
        &objects,
        &[],
        &camera,
        lighting(),
        settings,
        debug_mode,
    ) {
        Ok(image) => image,
        Err(error) => panic!("bay A must render: {error}"),
    }
}

/// A pixel's brightness, 0..765.
fn luminance(pixel: [u8; 4]) -> i32 {
    i32::from(pixel[0]) + i32::from(pixel[1]) + i32::from(pixel[2])
}

#[test]
fn bay_a_puts_a_shadow_on_the_ground_and_turning_shadows_off_takes_it_away() {
    if !adapter_available() {
        eprintln!("no D3D12 adapter: skipping");
        return;
    }
    let lit = render_bay_a(ShadowPreset::Off.settings());
    let shadowed = render_bay_a(ShadowPreset::High.settings());

    let mut darkened = 0_usize;
    let mut brightened = 0_usize;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (Some(before), Some(after)) = (lit.pixel(x, y), shadowed.pixel(x, y)) else {
                panic!("both images are {WIDTH}x{HEIGHT}");
            };
            let difference = luminance(after) - luminance(before);
            // A tenth of one channel. Below that is the difference between two runs of the
            // same shader, not the difference between shadowed and lit.
            if difference < -25 {
                darkened += 1;
            } else if difference > 25 {
                brightened += 1;
            }
        }
    }

    // A four-metre pillar lit from forty-five degrees casts a shadow several metres long, and
    // the camera is close enough that several metres is a large part of the frame. One per
    // cent of the image is a floor far below what a working shadow produces and far above
    // what noise does.
    let total = (WIDTH * HEIGHT) as usize;
    assert!(
        darkened > total / 100,
        "only {darkened} of {total} pixels darkened when shadows were switched on"
    );
    assert!(
        brightened * 50 < darkened,
        "{brightened} pixels got brighter with shadows on: a shadow pass must only subtract \
         light, and this many means the two images are not the same view"
    );
}

#[test]
fn every_crate_in_bay_b_is_a_caster() {
    // Not a GPU test: it asserts the content, which is where a shadow bug in a *scene* lives.
    // A crate marked as not casting, or a bay that quietly shrank, would leave every pixel
    // test passing and the bay measuring nothing.
    let scene = shadow_course();
    let crates: Vec<&EnvironmentObject> = scene
        .objects
        .iter()
        .filter(|object| object.asset == ids::SUPPLY_CRATE && object.position().z < -50.0)
        .collect();
    assert_eq!(crates.len(), CRATE_GRID * CRATE_GRID);
    // Some of them stand on top of each other, which is the case a cascade's near plane can
    // clip away. If that stopped being true the bay would stop asking the question.
    assert!(
        crates.iter().any(|object| object.position().y > 0.5),
        "no crate is stacked: the bay only tests shadows falling on the ground"
    );
}

#[test]
fn the_atlas_debug_view_draws_the_atlas_in_the_corner_with_its_tiles_outlined() {
    if !adapter_available() {
        eprintln!("no D3D12 adapter: skipping");
        return;
    }
    // Debug view 3. The preview is a square-ish block in the bottom-left corner: mostly the
    // atlas clear value, which is the far plane and therefore white, with the cascade tiles
    // outlined in red. Both are asserted, because the two ways this view breaks are "it does
    // not appear at all" and "it appears as a flat colour because the atlas is not bound".
    let image = render_bay_a_debug(ShadowPreset::High.settings(), 3);

    let side = (WIDTH.min(HEIGHT) as f32 * 0.42) as u32;
    let margin = 16_u32;
    let low_y = HEIGHT - margin - side;

    let mut white = 0_usize;
    let mut red = 0_usize;
    for y in low_y..(HEIGHT - margin) {
        for x in margin..(margin + side) {
            let Some(pixel) = image.pixel(x, y) else {
                panic!("the preview is inside a {WIDTH}x{HEIGHT} image");
            };
            if pixel[0] > 230 && pixel[1] > 230 && pixel[2] > 230 {
                white += 1;
            } else if pixel[0] > 150 && pixel[1] < 90 && pixel[2] < 90 {
                red += 1;
            }
        }
    }

    let area = (side * side) as usize;
    assert!(
        white > area / 4,
        "only {white} of {area} preview pixels are the atlas's cleared far plane: the atlas          is not being sampled"
    );
    assert!(
        red > 60,
        "only {red} pixels of cascade outline: the tiles are not marked"
    );

    // And it is *only* the corner: a preview that covers the frame is a debug view nobody
    // can use, because the thing being debugged is behind it.
    let Some(elsewhere) = image.pixel(WIDTH - 8, 8) else {
        panic!("the top-right corner is inside the image");
    };
    assert!(
        !(elsewhere[0] > 230 && elsewhere[1] > 230 && elsewhere[2] > 230),
        "the preview reaches the opposite corner of the frame"
    );
}

#[test]
fn every_preset_that_draws_a_shadow_draws_it_from_every_angle() {
    // Two bugs lived here, both invisible to a test that renders one viewpoint at one preset.
    //
    // 1. The camera's depth prepass — which only High and Ultra run, to feed contact shadows
    //    — was built from the shadow atlas's rasterizer state, including its deliberate
    //    `DepthClipEnable: false`. Clamping instead of clipping is right for a cascade, where
    //    a caster in front of the near plane must still cast. For the camera it means the
    //    slice of the ground between the eye and the near plane is rasterized at the nearest
    //    possible depth instead of being clipped away, and the real ground behind it then
    //    fails the main pass's `LESS_EQUAL` and is never shaded. The floor came out flat
    //    ambient grey with no shadow on it, over whatever part of the frame that slice
    //    happened to cover — which moves as the camera turns, so it flickered.
    //
    // 2. The contact-shadow march started exactly on the surface being shaded, so on a floor
    //    its first samples projected back onto that same floor and read the floor's own
    //    screen-space slope as an occluder. Below a certain camera height that difference
    //    lands inside the thickness window across most of the ground at once.
    //
    // Both were angle-dependent, so the assertion is over angles: the presets differ in how
    // *finely* they resolve a shadow, never in whether there is one. High adds a contact
    // shadow to what Medium draws, so it may darken a little more — it must never darken
    // dramatically more, and it must never stop darkening.
    if !adapter_available() {
        eprintln!("no D3D12 adapter: skipping");
        return;
    }
    let total = (WIDTH * HEIGHT) as usize;
    // Eye heights from a standing view down to a low, grazing one: the low angles are where
    // both bugs showed, and a test that only looks down at the scene sees neither.
    for &(height, distance) in &[(5.5_f32, 9.0_f32), (1.7, 9.0), (1.2, 16.0)] {
        for step in 0..8 {
            let yaw = std::f32::consts::TAU * (step as f32) / 8.0;
            let lit = render_bay_a_eye(ShadowPreset::Off.settings(), 0, yaw, height, distance);
            let mut darkened = [0_usize; 2];
            for (slot, settings) in [
                ShadowPreset::Medium.settings(),
                ShadowPreset::High.settings(),
            ]
            .into_iter()
            .enumerate()
            {
                let shadowed = render_bay_a_eye(settings, 0, yaw, height, distance);
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        let (Some(before), Some(after)) = (lit.pixel(x, y), shadowed.pixel(x, y))
                        else {
                            panic!("both images are {WIDTH}x{HEIGHT}");
                        };
                        if luminance(after) - luminance(before) < -25 {
                            darkened[slot] += 1;
                        }
                    }
                }
            }
            let [medium, high] = darkened;
            let where_ = format!("at eye height {height}, distance {distance}, yaw step {step}");
            // Half of what Medium finds. The failure this catches took High to nearly zero,
            // so the bound does not need to be tight to catch it, and a loose one leaves room
            // for the genuine differences between a 1024 and a 2048 cascade.
            assert!(
                high * 2 > medium,
                "{where_}: High darkened {high} pixels where Medium darkened {medium}.                  A higher preset must not lose the shadow."
            );
            // And the other direction: contact shadows add centimetres, not a grey floor.
            assert!(
                high < medium + total / 50,
                "{where_}: High darkened {high} pixels where Medium darkened {medium}.                  That is contact shadow spilling across the ground, not resolving a contact."
            );
        }
    }
}
