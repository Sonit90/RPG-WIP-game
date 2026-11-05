//! The FireMage, drawn on a real GPU, asserted in pixels.
//!
//! This is Milestone 19's "manual visual verification" written down so it survives. Looking
//! at a character and deciding it is fine proves it was fine once, on one machine, on one
//! afternoon. These tests prove — every run, on any adapter — that:
//!
//! * the model reaches the screen at all,
//! * posing the skeleton actually moves the vertices,
//! * a bind-pose palette leaves the model undeformed,
//! * the skinned pass and the rigid pass agree about lighting and colour,
//! * and a broken palette is refused rather than drawn wrong.
//!
//! Every test skips itself when no D3D12 adapter is present.

#![cfg(windows)]

use engine_assets::procedural::{DummyVariant, fire_mage, training_dummy};
use engine_assets::{AnimationPlayer, Geometry, ModelAsset, Pose};
use engine_core::math::{Mat4, Quat, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{Camera, Lighting, MAX_PALETTE_BONES, Projection};
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::{SkinnedMeshData, SkinnedVertex};
use engine_renderer::offscreen::{
    CapturedImage, SceneObject, SkinnedObject, render_scene, render_skinned_scene,
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Whether a D3D12 adapter exists. Without one there is nothing to test.
fn adapter_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, CLEAR).is_ok()
}

/// A camera looking at a 1.8 m character from the front.
fn camera() -> Camera {
    let Ok(mut camera) = Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32)) else {
        panic!("a 4:3 perspective is valid");
    };
    let Ok(()) = camera.look_at(Vec3::new(0.0, 1.1, 4.2), Vec3::new(0.0, 0.95, 0.0), Vec3::Y)
    else {
        panic!("the camera must look at the character");
    };
    camera
}

/// The FireMage's LOD0 submeshes, converted to the renderer's vertex layout.
fn firemage_meshes(model: &ModelAsset) -> Vec<SkinnedMeshData> {
    let Some(lod) = model.lod0() else {
        panic!("the FireMage has a LOD0");
    };
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
            match SkinnedMeshData::new(vertices, submesh.indices.clone()) {
                Ok(data) => Some(data),
                Err(error) => panic!("the FireMage's geometry must upload: {error}"),
            }
        })
        .collect()
}

/// The palette for `pose`.
fn palette_for(model: &ModelAsset, pose: &Pose) -> Vec<Mat4> {
    let Some(skeleton) = model.skeleton.as_ref() else {
        panic!("the FireMage has a skeleton");
    };
    let mut palette = Vec::new();
    match skeleton.skinning_matrices(pose, &mut palette) {
        Ok(()) => palette,
        Err(error) => panic!("the pose must skin: {error}"),
    }
}

/// Renders the FireMage under `pose`.
fn render_mage(model: &ModelAsset, pose: &Pose) -> CapturedImage {
    let meshes = firemage_meshes(model);
    let palette = palette_for(model, pose);
    let objects: Vec<SkinnedObject<'_>> = meshes
        .iter()
        .map(|mesh| SkinnedObject {
            mesh,
            // One flat material for every submesh: this is a geometry test, and four
            // different colours would make "which pixels changed" a harder question.
            material: MaterialDesc::colored(0.8, 0.5, 0.3),
            transform: Transform::IDENTITY,
            palette: &palette,
        })
        .collect();

    match render_skinned_scene(
        WIDTH,
        HEIGHT,
        CLEAR,
        &objects,
        &camera(),
        Lighting::default(),
    ) {
        Ok(image) => image,
        Err(error) => panic!("the skinned scene must render: {error}"),
    }
}

/// Pixels that are not the clear colour.
fn drawn_pixels(image: &CapturedImage) -> usize {
    image.count_pixels(|pixel| pixel[0] > 8 || pixel[1] > 8 || pixel[2] > 8)
}

/// Pixels whose colour differs by more than `tolerance` in any channel.
fn differing_pixels(a: &CapturedImage, b: &CapturedImage, tolerance: u8) -> usize {
    let mut differing = 0;
    for y in 0..a.height {
        for x in 0..a.width {
            let (Some(left), Some(right)) = (a.pixel(x, y), b.pixel(x, y)) else {
                continue;
            };
            if left
                .iter()
                .zip(right)
                .any(|(l, r)| l.abs_diff(r) > tolerance)
            {
                differing += 1;
            }
        }
    }
    differing
}

#[test]
fn the_firemage_is_actually_drawn() {
    if !adapter_available() {
        return;
    }
    let Ok(model) = fire_mage() else {
        panic!("the FireMage must build");
    };
    let Some(skeleton) = model.skeleton.as_ref() else {
        panic!("the FireMage has a skeleton");
    };
    let image = render_mage(&model, &skeleton.rest_pose());

    let drawn = drawn_pixels(&image);
    assert!(
        drawn > 2_000,
        "a 1.8 m character filling most of a 320x240 frame should cover thousands of \
         pixels, not {drawn}"
    );
    // And it is lit, not a flat silhouette: a Lambert term across a body whose faces point
    // in six directions produces a spread of brightnesses rather than one value. The dim
    // count is small because most of what the camera sees is the front of the character,
    // which faces the light — measured at 1,888 bright and 25 dim on an RX 6700 XT.
    let bright = image.count_pixels(|pixel| pixel[0] > 120);
    let dim = image.count_pixels(|pixel| pixel[0] > 8 && pixel[0] < 100);
    assert!(bright > 100 && dim > 10, "bright {bright}, dim {dim}");
}

#[test]
fn the_bind_pose_draws_the_model_undeformed() {
    // The definition of a bind pose: skinning it must be the identity. If the inverse bind
    // matrices were derived with the multiplication the wrong way round, this frame is empty
    // or full of geometry stretched across the screen — and *only* a rendered frame catches
    // the version of that bug that lives in the shader rather than in the maths.
    if !adapter_available() {
        return;
    }
    let Ok(model) = fire_mage() else {
        panic!("the FireMage must build");
    };
    let Some(skeleton) = model.skeleton.as_ref() else {
        panic!("the FireMage has a skeleton");
    };

    let posed = render_mage(&model, &skeleton.rest_pose());

    // The same geometry through the *rigid* pipeline, with the bind-pose vertices as they
    // are authored. If skinning by a bind-pose palette is the identity, the two images are
    // the same picture.
    let meshes = firemage_meshes(&model);
    let rigid: Vec<engine_renderer::mesh::MeshData> = meshes
        .iter()
        .map(|mesh| {
            let vertices = mesh
                .vertices()
                .iter()
                .map(|vertex| {
                    engine_renderer::mesh::MeshVertex::new(
                        vertex.position,
                        vertex.normal,
                        vertex.uv,
                    )
                })
                .collect();
            match engine_renderer::mesh::MeshData::new(vertices, mesh.indices().to_vec()) {
                Ok(data) => data,
                Err(error) => panic!("the same geometry must be a valid rigid mesh: {error}"),
            }
        })
        .collect();
    let objects: Vec<SceneObject<'_>> = rigid
        .iter()
        .map(|mesh| SceneObject {
            mesh,
            material: MaterialDesc::colored(0.8, 0.5, 0.3),
            transform: Transform::IDENTITY,
        })
        .collect();
    let Ok(unskinned) = render_scene(
        WIDTH,
        HEIGHT,
        CLEAR,
        &objects,
        &camera(),
        Lighting::default(),
    ) else {
        panic!("the rigid scene must render");
    };

    let differing = differing_pixels(&posed, &unskinned, 1);
    assert_eq!(
        differing, 0,
        "skinning by a bind-pose palette must be the identity: {differing} pixels differ"
    );
}

#[test]
fn posing_the_skeleton_moves_the_character() {
    // The other half: skinning must not be the identity for a pose that is not the bind
    // pose. A shader that ignored its palette entirely would pass the test above and fail
    // this one.
    if !adapter_available() {
        return;
    }
    let Ok(model) = fire_mage() else {
        panic!("the FireMage must build");
    };
    let Some(skeleton) = model.skeleton.as_ref() else {
        panic!("the FireMage has a skeleton");
    };

    let rest = render_mage(&model, &skeleton.rest_pose());

    let mut pose = skeleton.rest_pose();
    let Some(shoulder) = skeleton.bone_index("shoulder_l") else {
        panic!("the FireMage has a left shoulder");
    };
    // Arm down from the T-pose: the single largest change any of its clips makes.
    pose.locals_mut()[shoulder as usize].rotation =
        Quat::from_axis_angle(Vec3::Z, -80.0_f32.to_radians());
    let posed = render_mage(&model, &pose);

    // An arm is a small part of a 320x240 frame; 229 pixels was the measurement, and the
    // threshold is what separates "the palette is read" from "the palette is ignored", not
    // a claim about how big an arm is.
    let moved = differing_pixels(&rest, &posed, 8);
    assert!(
        moved > 120,
        "bringing an arm down from a T-pose should move the arm, and moved {moved} pixels"
    );
}

#[test]
fn every_authored_clip_poses_the_character_differently_from_its_rest_pose() {
    // Eight clips, each sampled at its own midpoint. A clip that animates nothing — a typo
    // in a bone index, a track that was dropped — renders identically to the rest pose, and
    // is otherwise completely silent.
    if !adapter_available() {
        return;
    }
    let Ok(model) = fire_mage() else {
        panic!("the FireMage must build");
    };
    let Some(skeleton) = model.skeleton.as_ref() else {
        panic!("the FireMage has a skeleton");
    };
    let rest = render_mage(&model, &skeleton.rest_pose());

    for (index, clip) in model.clips.iter().enumerate() {
        let mut player = AnimationPlayer::new(skeleton);
        player.play(index, 0.0);
        let Ok(()) = player.update(clip.duration * 0.5, &model.clips) else {
            panic!("'{}' must sample", clip.name);
        };
        let posed = render_mage(&model, player.pose());
        let moved = differing_pixels(&rest, &posed, 8);
        assert!(
            moved > 100,
            "'{}' changes only {moved} pixels, which is not an animation",
            clip.name
        );
    }
}

#[test]
fn a_palette_longer_than_the_shader_array_is_refused_rather_than_drawn() {
    if !adapter_available() {
        return;
    }
    let vertex = SkinnedVertex::new(
        Vec3::ZERO,
        Vec3::Y,
        engine_core::math::Vec2::ZERO,
        [0; 4],
        [1.0, 0.0, 0.0, 0.0],
    );
    let Ok(mesh) = SkinnedMeshData::new(vec![vertex; 3], vec![0, 1, 2]) else {
        panic!("a triangle is a valid mesh");
    };
    let palette = vec![Mat4::IDENTITY; MAX_PALETTE_BONES + 1];
    let objects = [SkinnedObject {
        mesh: &mesh,
        material: MaterialDesc::default(),
        transform: Transform::IDENTITY,
        palette: &palette,
    }];
    assert!(
        render_skinned_scene(64, 64, CLEAR, &objects, &camera(), Lighting::default()).is_err(),
        "a palette past the end of the shader's array must be refused"
    );
}

#[test]
fn the_training_dummy_is_drawn_and_is_not_the_character() {
    // Milestone 6's readability requirement, as a measurement: the dummy and the mage cover
    // visibly different silhouettes from the same camera.
    if !adapter_available() {
        return;
    }
    let dummy = training_dummy(DummyVariant::Single);
    let Some(lod) = dummy.lod0() else {
        panic!("the dummy has a LOD0");
    };
    let meshes: Vec<engine_renderer::mesh::MeshData> = lod
        .submeshes
        .iter()
        .filter_map(|submesh| {
            let Geometry::Static(vertices) = &submesh.geometry else {
                return None;
            };
            let vertices = vertices
                .iter()
                .map(|vertex| {
                    engine_renderer::mesh::MeshVertex::new(
                        vertex.position,
                        vertex.normal,
                        vertex.uv,
                    )
                })
                .collect();
            match engine_renderer::mesh::MeshData::new(vertices, submesh.indices.clone()) {
                Ok(data) => Some(data),
                Err(error) => panic!("the dummy's geometry must upload: {error}"),
            }
        })
        .collect();
    let objects: Vec<SceneObject<'_>> = meshes
        .iter()
        .map(|mesh| SceneObject {
            mesh,
            material: MaterialDesc::colored(0.8, 0.5, 0.3),
            transform: Transform::IDENTITY,
        })
        .collect();
    let Ok(image) = render_scene(
        WIDTH,
        HEIGHT,
        CLEAR,
        &objects,
        &camera(),
        Lighting::default(),
    ) else {
        panic!("the dummy scene must render");
    };
    let dummy_pixels = drawn_pixels(&image);
    assert!(
        dummy_pixels > 1_000,
        "the dummy covers {dummy_pixels} pixels"
    );

    let Ok(model) = fire_mage() else {
        panic!("the FireMage must build");
    };
    let Some(skeleton) = model.skeleton.as_ref() else {
        panic!("the FireMage has a skeleton");
    };
    let mage = render_mage(&model, &skeleton.rest_pose());
    let differing = differing_pixels(&image, &mage, 16);
    assert!(
        differing > 2_000,
        "a post with a straw body and a hooded figure with a staff should not look alike: \
         {differing} pixels differ"
    );
}
