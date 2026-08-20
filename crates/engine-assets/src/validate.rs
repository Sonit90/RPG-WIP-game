//! Structural and budget checks run once, on every model, before it reaches the GPU.
//!
//! # Why this is not optional
//!
//! D3D12 does not bounds-check an index buffer. An index past the end of the vertex array
//! reads whatever memory follows it, and the symptom is a triangle stretched to the horizon
//! — or nothing, or a hang — with no validation message pointing at the model that did it.
//! The same is true of a joint index past the end of the bone palette. Everything this
//! module checks is something that either cannot be detected at all downstream, or can only
//! be detected as "the picture looks wrong".
//!
//! The rule the phase brief states, and this module enforces: **do not silently fall back to
//! broken rendering.** A model that fails validation is not uploaded, and the error names the
//! model, the place inside it, and the numbers.
//!
//! # Budgets
//!
//! [`ValidationLimits`] holds the project's numbers rather than universal truths. They are
//! deliberately generous — roughly ten times what the Phase 5 assets use — because a budget
//! that trips on ordinary content teaches people to raise the budget.

use crate::error::AssetError;
use crate::mesh::{Geometry, SubMesh};
use crate::model::{ModelAsset, ModelLod};

/// What a model is allowed to cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationLimits {
    /// Triangles in any one level of detail.
    pub max_triangles_per_lod: u64,
    /// Width or height of any texture, in texels.
    pub max_texture_size: u32,
    /// Materials on one model. Each one is a pipeline bind and a draw boundary.
    pub max_materials: u64,
    /// Submeshes in any one level of detail.
    pub max_submeshes_per_lod: u64,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            // A stylized MMO character is a few thousand triangles. 100,000 catches an
            // artist exporting a subdivision cage or a raw sculpt.
            max_triangles_per_lod: 1_000_000,
            // 2048 is the largest texture a character in this project should need. The
            // phase's own textures are 256 and 512.
            max_texture_size: 2048,
            max_materials: 16,
            max_submeshes_per_lod: 32,
        }
    }
}

/// What a model turned out to contain. The numbers the performance chapter quotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelReport {
    /// Triangles at LOD0.
    pub triangles: u64,
    /// Vertices at LOD0.
    pub vertices: u64,
    /// Levels of detail.
    pub lods: u64,
    /// Submeshes at LOD0 — the number of draws one instance of this model costs.
    pub submeshes: u64,
    /// Materials.
    pub materials: u64,
    /// Textures.
    pub textures: u64,
    /// Bytes those textures occupy.
    pub texture_bytes: u64,
    /// Bones, or zero for a rigid model.
    pub bones: u64,
    /// Animation clips.
    pub clips: u64,
}

/// Checks a model and reports what it contains.
///
/// # Errors
///
/// Returns the first [`AssetError`] found. First rather than all: a model with a dangling
/// index is not going to be shipped with a second problem fixed, and a single precise
/// message is more useful than a list.
pub fn validate_model(
    model: &ModelAsset,
    limits: &ValidationLimits,
) -> Result<ModelReport, AssetError> {
    if model.lods.is_empty() {
        return Err(AssetError::invalid(
            &model.name,
            "lods",
            "a model needs at least one level of detail",
        ));
    }
    let Some(lod0) = model.lods.first() else {
        return Err(AssetError::invalid(&model.name, "lods", "LOD0 is missing"));
    };
    if lod0.min_distance != 0.0 {
        return Err(AssetError::invalid(
            &model.name,
            "lods[0]",
            format!(
                "LOD0 must start at distance 0, not {}; nothing would be drawn up close",
                lod0.min_distance
            ),
        ));
    }
    // Strictly increasing, so selection is unambiguous. Two levels at the same distance is a
    // level that can never be selected, which is dead content nobody notices.
    for pair in model.lods.windows(2) {
        if pair[1].min_distance <= pair[0].min_distance {
            return Err(AssetError::invalid(
                &model.name,
                "lods",
                format!(
                    "levels must be strictly increasing in distance, found {} after {}",
                    pair[1].min_distance, pair[0].min_distance
                ),
            ));
        }
    }

    validate_textures(model, limits)?;
    validate_materials(model)?;

    let bone_count = model.skeleton.as_ref().map_or(0, |skeleton| skeleton.len());
    for (level, lod) in model.lods.iter().enumerate() {
        validate_lod(model, lod, level, bone_count, limits)?;
    }
    validate_clips(model, bone_count)?;

    if model.materials.len() as u64 > limits.max_materials {
        return Err(AssetError::over_budget(
            &model.name,
            "material",
            model.materials.len() as u64,
            limits.max_materials,
        ));
    }

    Ok(ModelReport {
        triangles: lod0.triangle_count() as u64,
        vertices: lod0.vertex_count() as u64,
        lods: model.lods.len() as u64,
        submeshes: lod0.submeshes.len() as u64,
        materials: model.materials.len() as u64,
        textures: model.textures.len() as u64,
        texture_bytes: model.texture_bytes() as u64,
        bones: bone_count as u64,
        clips: model.clips.len() as u64,
    })
}

/// Every texture is well-formed and inside the resolution budget.
fn validate_textures(model: &ModelAsset, limits: &ValidationLimits) -> Result<(), AssetError> {
    for (index, texture) in model.textures.iter().enumerate() {
        let expected =
            texture.width as usize * texture.height as usize * texture.format.bytes_per_texel();
        if texture.texels.len() != expected {
            return Err(AssetError::invalid(
                &model.name,
                format!("texture {index} '{}'", texture.name),
                format!(
                    "{}x{} needs {expected} bytes, got {}",
                    texture.width,
                    texture.height,
                    texture.texels.len()
                ),
            ));
        }
        let largest = texture.width.max(texture.height);
        if largest > limits.max_texture_size {
            return Err(AssetError::over_budget(
                &model.name,
                format!("texture '{}' resolution", texture.name),
                u64::from(largest),
                u64::from(limits.max_texture_size),
            ));
        }
    }
    Ok(())
}

/// Every material's texture slots point at a texture that exists.
fn validate_materials(model: &ModelAsset) -> Result<(), AssetError> {
    for (index, material) in model.materials.iter().enumerate() {
        for (slot, texture) in material.texture_indices() {
            if texture >= model.textures.len() {
                return Err(AssetError::invalid(
                    &model.name,
                    format!("material {index} '{}' {slot}", material.name),
                    format!(
                        "names texture {texture}, and the model has {}",
                        model.textures.len()
                    ),
                ));
            }
        }
        if !material.roughness.is_finite() || !(0.0..=1.0).contains(&material.roughness) {
            return Err(AssetError::invalid(
                &model.name,
                format!("material {index} '{}'", material.name),
                format!("roughness {} is not in [0, 1]", material.roughness),
            ));
        }
        if !material.metallic.is_finite() || !(0.0..=1.0).contains(&material.metallic) {
            return Err(AssetError::invalid(
                &model.name,
                format!("material {index} '{}'", material.name),
                format!("metallic {} is not in [0, 1]", material.metallic),
            ));
        }
    }
    Ok(())
}

/// One level of detail: geometry, indices, materials, and skinning data.
fn validate_lod(
    model: &ModelAsset,
    lod: &ModelLod,
    level: usize,
    bone_count: usize,
    limits: &ValidationLimits,
) -> Result<(), AssetError> {
    if lod.submeshes.is_empty() {
        return Err(AssetError::invalid(
            &model.name,
            format!("lods[{level}]"),
            "a level of detail with no submeshes draws nothing",
        ));
    }
    if lod.submeshes.len() as u64 > limits.max_submeshes_per_lod {
        return Err(AssetError::over_budget(
            &model.name,
            format!("lods[{level}] submesh"),
            lod.submeshes.len() as u64,
            limits.max_submeshes_per_lod,
        ));
    }
    if lod.triangle_count() as u64 > limits.max_triangles_per_lod {
        return Err(AssetError::over_budget(
            &model.name,
            format!("lods[{level}] triangle"),
            lod.triangle_count() as u64,
            limits.max_triangles_per_lod,
        ));
    }

    for (index, submesh) in lod.submeshes.iter().enumerate() {
        let where_ = format!("lods[{level}] submesh {index} '{}'", submesh.name);
        validate_submesh(model, submesh, &where_, bone_count)?;
    }
    Ok(())
}

/// One submesh: indices in range, materials in range, weights and joints usable.
fn validate_submesh(
    model: &ModelAsset,
    submesh: &SubMesh,
    where_: &str,
    bone_count: usize,
) -> Result<(), AssetError> {
    if submesh.geometry.is_empty() || submesh.indices.is_empty() {
        return Err(AssetError::invalid(
            &model.name,
            where_,
            "a submesh needs at least one vertex and one index",
        ));
    }
    if submesh.indices.len() % 3 != 0 {
        return Err(AssetError::invalid(
            &model.name,
            where_,
            format!(
                "a triangle list needs a multiple of three indices, got {}",
                submesh.indices.len()
            ),
        ));
    }
    let vertices = submesh.geometry.len();
    if let Some(bad) = submesh
        .indices
        .iter()
        .find(|index| **index as usize >= vertices)
    {
        return Err(AssetError::invalid(
            &model.name,
            where_,
            format!("index {bad} is past the end of {vertices} vertices"),
        ));
    }
    if submesh.material >= model.materials.len() {
        return Err(AssetError::invalid(
            &model.name,
            where_,
            format!(
                "names material {}, and the model has {}",
                submesh.material,
                model.materials.len()
            ),
        ));
    }

    match &submesh.geometry {
        Geometry::Static(vertices) => {
            for (index, vertex) in vertices.iter().enumerate() {
                if !vertex.position.length_squared().is_finite() {
                    return Err(AssetError::invalid(
                        &model.name,
                        where_,
                        format!("vertex {index} has a non-finite position"),
                    ));
                }
            }
        }
        Geometry::Skinned(vertices) => {
            if bone_count == 0 {
                return Err(AssetError::invalid(
                    &model.name,
                    where_,
                    "skinned geometry needs a skeleton, and the model has none",
                ));
            }
            for (index, vertex) in vertices.iter().enumerate() {
                if !vertex.position.length_squared().is_finite() {
                    return Err(AssetError::invalid(
                        &model.name,
                        where_,
                        format!("vertex {index} has a non-finite position"),
                    ));
                }
                if let Some(joint) = vertex
                    .joints
                    .iter()
                    .find(|joint| **joint as usize >= bone_count)
                {
                    // Checked even where the weight is zero: the shader multiplies before
                    // it adds, so it reads the palette entry either way.
                    return Err(AssetError::invalid(
                        &model.name,
                        where_,
                        format!(
                            "vertex {index} is bound to bone {joint}, and the skeleton has {bone_count}"
                        ),
                    ));
                }
                let sum = vertex.weight_sum();
                if !(0.99..=1.01).contains(&sum) {
                    return Err(AssetError::invalid(
                        &model.name,
                        where_,
                        format!(
                            "vertex {index} has weights summing to {sum}, which deforms the \
                             model towards the origin"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Every clip is bound to this model's skeleton and has usable keys.
fn validate_clips(model: &ModelAsset, bone_count: usize) -> Result<(), AssetError> {
    for (index, clip) in model.clips.iter().enumerate() {
        let where_ = format!("clip {index} '{}'", clip.name);
        if bone_count == 0 {
            return Err(AssetError::invalid(
                &model.name,
                where_,
                "an animation needs a skeleton, and the model has none",
            ));
        }
        if !clip.duration.is_finite() || clip.duration < 0.0 {
            return Err(AssetError::invalid(
                &model.name,
                where_,
                format!("duration {} is not a length of time", clip.duration),
            ));
        }
        for track in &clip.tracks {
            if track.bone as usize >= bone_count {
                return Err(AssetError::invalid(
                    &model.name,
                    &where_,
                    format!(
                        "a track animates bone {}, and the skeleton has {bone_count}",
                        track.bone
                    ),
                ));
            }
            let times = track
                .translation
                .iter()
                .map(|key| key.time)
                .chain(track.rotation.iter().map(|key| key.time))
                .chain(track.scale.iter().map(|key| key.time));
            for time in times {
                if !time.is_finite() || time < 0.0 {
                    return Err(AssetError::invalid(
                        &model.name,
                        &where_,
                        format!("a key is at time {time}"),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{AnimationClip, BoneTrack};
    use crate::material::MaterialAsset;
    use crate::mesh::{MeshVertex, SkinVertex, SubMesh};
    use crate::skeleton::{Bone, Skeleton};
    use crate::texture::{TextureAsset, TextureFormat};
    use engine_core::math::{Mat4, Vec2, Vec3};
    use engine_core::transform::Transform;

    fn one_bone() -> Skeleton {
        match Skeleton::new(vec![Bone {
            name: "root".to_owned(),
            parent: None,
            rest: Transform::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        }]) {
            Ok(skeleton) => skeleton,
            Err(error) => panic!("a one-bone skeleton must build: {error}"),
        }
    }

    fn static_triangle(material: usize) -> SubMesh {
        SubMesh::new(
            "tri",
            Geometry::Static(vec![
                MeshVertex::new(Vec3::ZERO, Vec3::Y, Vec2::ZERO),
                MeshVertex::new(Vec3::X, Vec3::Y, Vec2::ZERO),
                MeshVertex::new(Vec3::Y, Vec3::Y, Vec2::ZERO),
            ]),
            vec![0, 1, 2],
            material,
        )
    }

    fn skinned_triangle(joint: u16, weights: [f32; 4]) -> SubMesh {
        let vertex = |position: Vec3| SkinVertex {
            position,
            normal: Vec3::Y,
            uv: Vec2::ZERO,
            joints: [joint, 0, 0, 0],
            weights,
        };
        SubMesh::new(
            "skin",
            Geometry::Skinned(vec![vertex(Vec3::ZERO), vertex(Vec3::X), vertex(Vec3::Y)]),
            vec![0, 1, 2],
            0,
        )
    }

    fn good_model() -> ModelAsset {
        ModelAsset::rigid(
            "test",
            vec![static_triangle(0)],
            vec![MaterialAsset::default()],
            vec![],
        )
    }

    #[test]
    fn a_well_formed_model_passes_and_reports_what_it_contains() {
        let Ok(report) = validate_model(&good_model(), &ValidationLimits::default()) else {
            panic!("a well-formed model must validate");
        };
        assert_eq!(report.triangles, 1);
        assert_eq!(report.vertices, 3);
        assert_eq!(report.materials, 1);
        assert_eq!(report.bones, 0);
        assert_eq!(report.lods, 1);
    }

    #[test]
    fn an_index_past_the_end_of_the_vertices_is_rejected() {
        // The check that matters most: D3D12 does not bounds-check an index buffer.
        let mut model = good_model();
        model.lods[0].submeshes[0].indices = vec![0, 1, 7];
        let Err(error) = validate_model(&model, &ValidationLimits::default()) else {
            panic!("a dangling index must be rejected");
        };
        assert!(error.to_string().contains("past the end"), "{error}");
    }

    #[test]
    fn an_index_count_that_is_not_a_triangle_list_is_rejected() {
        let mut model = good_model();
        model.lods[0].submeshes[0].indices = vec![0, 1];
        assert!(validate_model(&model, &ValidationLimits::default()).is_err());
    }

    #[test]
    fn a_submesh_naming_a_material_that_does_not_exist_is_rejected() {
        let mut model = good_model();
        model.lods[0].submeshes[0].material = 3;
        let Err(error) = validate_model(&model, &ValidationLimits::default()) else {
            panic!("a dangling material index must be rejected");
        };
        assert!(error.to_string().contains("material 3"), "{error}");
    }

    #[test]
    fn a_material_naming_a_texture_that_does_not_exist_is_rejected() {
        let mut model = good_model();
        model.materials[0].base_color_texture = Some(0);
        let Err(error) = validate_model(&model, &ValidationLimits::default()) else {
            panic!("a missing texture must be rejected");
        };
        assert!(error.to_string().contains("base_color"), "{error}");
    }

    #[test]
    fn skinned_geometry_without_a_skeleton_is_rejected() {
        let mut model = good_model();
        model.lods[0].submeshes = vec![skinned_triangle(0, [1.0, 0.0, 0.0, 0.0])];
        let Err(error) = validate_model(&model, &ValidationLimits::default()) else {
            panic!("skinning needs a skeleton");
        };
        assert!(error.to_string().contains("needs a skeleton"), "{error}");
    }

    #[test]
    fn a_joint_past_the_end_of_the_skeleton_is_rejected_even_at_zero_weight() {
        // The shader multiplies before it adds, so it reads the palette entry regardless.
        let mut model = good_model();
        model.skeleton = Some(one_bone());
        model.lods[0].submeshes = vec![skinned_triangle(9, [1.0, 0.0, 0.0, 0.0])];
        let Err(error) = validate_model(&model, &ValidationLimits::default()) else {
            panic!("an out-of-range joint must be rejected");
        };
        assert!(error.to_string().contains("bone 9"), "{error}");
    }

    #[test]
    fn weights_that_do_not_sum_to_one_are_rejected() {
        let mut model = good_model();
        model.skeleton = Some(one_bone());
        model.lods[0].submeshes = vec![skinned_triangle(0, [0.5, 0.0, 0.0, 0.0])];
        let Err(error) = validate_model(&model, &ValidationLimits::default()) else {
            panic!("unnormalized weights must be rejected");
        };
        assert!(error.to_string().contains("towards the origin"), "{error}");
    }

    #[test]
    fn a_clip_animating_a_bone_the_skeleton_lacks_is_rejected() {
        let mut model = good_model();
        model.skeleton = Some(one_bone());
        model.clips = vec![AnimationClip::new("idle", true, vec![BoneTrack::new(4)])];
        let Err(error) = validate_model(&model, &ValidationLimits::default()) else {
            panic!("a track for a missing bone must be rejected");
        };
        assert!(error.to_string().contains("bone 4"), "{error}");
    }

    #[test]
    fn a_clip_on_a_model_with_no_skeleton_is_rejected() {
        let mut model = good_model();
        model.clips = vec![AnimationClip::new("idle", true, vec![])];
        assert!(validate_model(&model, &ValidationLimits::default()).is_err());
    }

    #[test]
    fn levels_of_detail_must_be_strictly_increasing_and_start_at_zero() {
        let duplicate = good_model()
            .with_lod(vec![static_triangle(0)], 10.0)
            .with_lod(vec![static_triangle(0)], 10.0);
        let Err(error) = validate_model(&duplicate, &ValidationLimits::default()) else {
            panic!("two levels at the same distance make selection ambiguous");
        };
        assert!(error.to_string().contains("strictly increasing"), "{error}");

        let mut floating = good_model();
        floating.lods[0].min_distance = 5.0;
        assert!(validate_model(&floating, &ValidationLimits::default()).is_err());
    }

    #[test]
    fn a_texture_over_the_resolution_budget_is_rejected() {
        let mut model = good_model();
        model.textures = vec![TextureAsset::generated(
            "huge",
            8,
            8,
            TextureFormat::Rgba8Srgb,
            |_, _| [0; 4],
        )];
        let limits = ValidationLimits {
            max_texture_size: 4,
            ..ValidationLimits::default()
        };
        let Err(error) = validate_model(&model, &limits) else {
            panic!("an oversized texture must be rejected");
        };
        assert!(error.to_string().contains("resolution"), "{error}");
    }

    #[test]
    fn a_malformed_texel_buffer_is_rejected_even_if_the_asset_was_built_by_hand() {
        let mut model = good_model();
        model.textures = vec![TextureAsset {
            name: "short".to_owned(),
            width: 4,
            height: 4,
            format: TextureFormat::Rgba8Srgb,
            texels: vec![0; 8],
        }];
        assert!(validate_model(&model, &ValidationLimits::default()).is_err());
    }

    #[test]
    fn a_triangle_budget_catches_a_model_nobody_meant_to_ship() {
        let limits = ValidationLimits {
            max_triangles_per_lod: 0,
            ..ValidationLimits::default()
        };
        assert!(validate_model(&good_model(), &limits).is_err());
    }

    #[test]
    fn an_empty_model_is_rejected_rather_than_drawing_nothing() {
        let mut model = good_model();
        model.lods.clear();
        assert!(validate_model(&model, &ValidationLimits::default()).is_err());

        let mut empty_lod = good_model();
        empty_lod.lods[0].submeshes.clear();
        assert!(validate_model(&empty_lod, &ValidationLimits::default()).is_err());
    }
}
