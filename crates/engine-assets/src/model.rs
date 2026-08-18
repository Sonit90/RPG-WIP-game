//! The runtime model: everything one drawable character or prop is made of.
//!
//! ```text
//! ModelAsset
//!  ├ lods[0]      LOD0 ── submeshes ── geometry + indices + material index
//!  ├ lods[1]      LOD1 ── the same shape, fewer triangles, same material indices
//!  ├ materials[]  parameters + texture indices
//!  ├ textures[]   decoded texels
//!  ├ skeleton     Some for a character, None for a prop
//!  └ clips[]      animations, bound to that skeleton
//! ```
//!
//! Indices, not pointers or handles: a model is a self-contained value that can be built,
//! validated, cloned and compared without a device, an allocator or a registry. Every index
//! inside it is checked once by [`crate::validate_model`], after which the renderer can
//! upload it without re-checking anything.
//!
//! # Why LODs live on the model rather than beside it
//!
//! A LOD is not a different model, it is the same model at a different cost, and every LOD
//! shares the materials, the textures and the skeleton. Keeping them together is what makes
//! "switch to LOD1" a change of which submesh list to draw rather than a second asset to
//! load, keep in sync, and get wrong.

use engine_core::math::Sphere;

use crate::animation::AnimationClip;
use crate::material::MaterialAsset;
use crate::mesh::{Geometry, SubMesh};
use crate::skeleton::Skeleton;
use crate::texture::TextureAsset;

/// One level of detail: the submeshes to draw, and how far away to start drawing them.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelLod {
    /// The geometry, one entry per material.
    pub submeshes: Vec<SubMesh>,
    /// Distance from the camera, in metres, at which this level takes over.
    ///
    /// LOD0 is always 0. Levels are sorted ascending, so selection is "the last level whose
    /// threshold has been passed".
    pub min_distance: f32,
}

impl ModelLod {
    /// Builds a level.
    #[must_use]
    pub fn new(submeshes: Vec<SubMesh>, min_distance: f32) -> Self {
        Self {
            submeshes,
            min_distance: min_distance.max(0.0),
        }
    }

    /// Triangles across every submesh.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.submeshes.iter().map(SubMesh::triangle_count).sum()
    }

    /// Vertices across every submesh.
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.submeshes
            .iter()
            .map(|submesh| submesh.geometry.len())
            .sum()
    }

    /// A sphere containing every submesh's bounds.
    ///
    /// Not the tightest possible sphere — it is the sphere around the submesh bounding
    /// spheres' extremes — which is the correct trade for a cull test: over-estimating draws
    /// something that could have been skipped, under-estimating pops a character off screen
    /// while it is still visible.
    #[must_use]
    pub fn bounds(&self) -> Sphere {
        let points = self.submeshes.iter().flat_map(|submesh| {
            let bounds = submesh.bounds;
            let radius = engine_core::math::Vec3::splat(bounds.radius);
            [bounds.center - radius, bounds.center + radius]
        });
        Sphere::around(points.collect::<Vec<_>>())
    }

    /// Whether any submesh needs a bone palette.
    #[must_use]
    pub fn is_skinned(&self) -> bool {
        self.submeshes
            .iter()
            .any(|submesh| submesh.geometry.is_skinned())
    }
}

/// A complete model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelAsset {
    /// What it is: `"fire_mage"`, `"training_dummy"`.
    pub name: String,
    /// Levels of detail, coarsest last. Never empty.
    pub lods: Vec<ModelLod>,
    /// Materials, addressed by [`SubMesh::material`].
    pub materials: Vec<MaterialAsset>,
    /// Textures, addressed by the material slots.
    pub textures: Vec<TextureAsset>,
    /// The bone hierarchy, for a skinned model.
    pub skeleton: Option<Skeleton>,
    /// Animations, bound to [`ModelAsset::skeleton`].
    pub clips: Vec<AnimationClip>,
}

impl ModelAsset {
    /// A model with one level of detail and no skeleton.
    #[must_use]
    pub fn rigid(
        name: impl Into<String>,
        submeshes: Vec<SubMesh>,
        materials: Vec<MaterialAsset>,
        textures: Vec<TextureAsset>,
    ) -> Self {
        Self {
            name: name.into(),
            lods: vec![ModelLod::new(submeshes, 0.0)],
            materials,
            textures,
            skeleton: None,
            clips: Vec::new(),
        }
    }

    /// The same model with an extra, coarser level of detail.
    ///
    /// # Panics
    ///
    /// Never. A level added out of order is re-sorted rather than rejected, because a
    /// caller listing levels coarsest-first is expressing the same intent.
    #[must_use]
    pub fn with_lod(mut self, submeshes: Vec<SubMesh>, min_distance: f32) -> Self {
        self.lods.push(ModelLod::new(submeshes, min_distance));
        self.lods
            .sort_by(|a, b| a.min_distance.total_cmp(&b.min_distance));
        self
    }

    /// The same model with a skeleton and clips.
    #[must_use]
    pub fn with_animation(mut self, skeleton: Skeleton, clips: Vec<AnimationClip>) -> Self {
        self.skeleton = Some(skeleton);
        self.clips = clips;
        self
    }

    /// The finest level of detail. Always present.
    #[must_use]
    pub fn lod0(&self) -> Option<&ModelLod> {
        self.lods.first()
    }

    /// Which level to draw at `distance` metres from the camera.
    ///
    /// `lod_bias` scales the distance before the comparison: values above 1 switch to
    /// cheaper levels sooner, which is the knob a quality setting turns. Zero or negative
    /// bias pins LOD0, which is what a screenshot or a cinematic wants.
    ///
    /// Returns an index into [`ModelAsset::lods`], never out of range.
    #[must_use]
    pub fn select_lod(&self, distance: f32, lod_bias: f32) -> usize {
        if lod_bias <= 0.0 || !distance.is_finite() {
            return 0;
        }
        let effective = distance.max(0.0) * lod_bias;
        let mut selected = 0;
        for (index, lod) in self.lods.iter().enumerate() {
            if effective >= lod.min_distance {
                selected = index;
            } else {
                // Levels are sorted, so the first one not yet reached ends the search.
                break;
            }
        }
        selected
    }

    /// Whether this model needs a skeleton and a skinning pipeline.
    ///
    /// A model with skinned geometry and no skeleton is rejected by validation, so this is a
    /// single question rather than two that can disagree.
    #[must_use]
    pub fn is_skinned(&self) -> bool {
        self.skeleton.is_some()
    }

    /// The index of the clip called `name`.
    #[must_use]
    pub fn clip_index(&self, name: &str) -> Option<usize> {
        self.clips.iter().position(|clip| clip.name == name)
    }

    /// Bytes every texture occupies, ignoring alignment.
    #[must_use]
    pub fn texture_bytes(&self) -> usize {
        self.textures.iter().map(TextureAsset::byte_size).sum()
    }

    /// Triangles at LOD0.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.lod0().map_or(0, ModelLod::triangle_count)
    }

    /// How tall LOD0 stands in its bind pose, in the units the model is authored in.
    ///
    /// Measured through the skinning matrices rather than off the raw positions, because a
    /// skinned mesh's vertices are in whatever space its skin was authored in — an export
    /// whose vertices are in centimetres and whose rig carries the conversion measures 190
    /// raw and 1.9 posed. The number a character definition wants is the second one: it is
    /// what the character will actually be on screen before gameplay scales it.
    ///
    /// A rigid model measures its positions directly. A model with no geometry, or one whose
    /// pose cannot be composed, reports zero and leaves the caller to decide what that means.
    #[must_use]
    pub fn bind_pose_height(&self) -> f32 {
        let Some(lod) = self.lod0() else {
            return 0.0;
        };
        let palette = match self.skeleton.as_ref() {
            Some(skeleton) => {
                let mut palette = Vec::with_capacity(skeleton.len());
                if skeleton
                    .skinning_matrices(&skeleton.rest_pose(), &mut palette)
                    .is_err()
                {
                    return 0.0;
                }
                palette
            }
            None => Vec::new(),
        };

        let mut lowest = f32::MAX;
        let mut highest = f32::MIN;
        for submesh in &lod.submeshes {
            match &submesh.geometry {
                Geometry::Static(vertices) => {
                    for vertex in vertices {
                        lowest = lowest.min(vertex.position.y);
                        highest = highest.max(vertex.position.y);
                    }
                }
                Geometry::Skinned(vertices) => {
                    for vertex in vertices {
                        let mut y = 0.0;
                        for (joint, weight) in vertex.joints.iter().zip(vertex.weights) {
                            let Some(matrix) = palette.get(*joint as usize) else {
                                continue;
                            };
                            y += weight * matrix.transform_point(vertex.position).y;
                        }
                        lowest = lowest.min(y);
                        highest = highest.max(y);
                    }
                }
            }
        }

        let height = highest - lowest;
        if height.is_finite() && height > 0.0 {
            height
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{Geometry, MeshVertex};
    use engine_core::math::{Vec2, Vec3};

    fn triangle(scale: f32) -> Vec<SubMesh> {
        let vertices = vec![
            MeshVertex::new(Vec3::new(-scale, 0.0, 0.0), Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::new(scale, 0.0, 0.0), Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::new(0.0, scale, 0.0), Vec3::Y, Vec2::ZERO),
        ];
        vec![SubMesh::new(
            "tri",
            Geometry::Static(vertices),
            vec![0, 1, 2],
            0,
        )]
    }

    fn three_level_model() -> ModelAsset {
        ModelAsset::rigid(
            "test",
            triangle(1.0),
            vec![MaterialAsset::default()],
            vec![],
        )
        .with_lod(triangle(1.0), 30.0)
        .with_lod(triangle(1.0), 12.0)
    }

    #[test]
    fn levels_are_kept_sorted_however_they_were_added() {
        let model = three_level_model();
        let thresholds: Vec<f32> = model.lods.iter().map(|lod| lod.min_distance).collect();
        assert_eq!(thresholds, vec![0.0, 12.0, 30.0]);
    }

    #[test]
    fn the_level_is_the_last_threshold_the_distance_has_passed() {
        let model = three_level_model();
        assert_eq!(model.select_lod(0.0, 1.0), 0);
        assert_eq!(model.select_lod(11.9, 1.0), 0);
        assert_eq!(
            model.select_lod(12.0, 1.0),
            1,
            "the boundary belongs to LOD1"
        );
        assert_eq!(model.select_lod(29.9, 1.0), 1);
        assert_eq!(model.select_lod(1000.0, 1.0), 2);
    }

    #[test]
    fn a_bias_moves_the_boundaries_and_zero_pins_the_finest_level() {
        let model = three_level_model();
        // Twice the bias halves the distance at which a level takes over.
        assert_eq!(model.select_lod(6.0, 2.0), 1);
        assert_eq!(model.select_lod(1000.0, 0.0), 0);
        assert_eq!(model.select_lod(1000.0, -1.0), 0);
    }

    #[test]
    fn a_nonsense_distance_selects_the_finest_level_rather_than_a_random_one() {
        // A NaN distance comes from a camera that has not been positioned yet. Drawing LOD0
        // is wrong-but-correct; indexing with a NaN comparison is undefined behaviour of the
        // "sometimes" kind.
        let model = three_level_model();
        assert_eq!(model.select_lod(f32::NAN, 1.0), 0);
        assert_eq!(model.select_lod(f32::INFINITY, 1.0), 0);
    }

    #[test]
    fn a_model_with_no_skeleton_is_not_skinned() {
        let model = three_level_model();
        assert!(!model.is_skinned());
        assert_eq!(model.triangle_count(), 1);
        assert_eq!(model.texture_bytes(), 0);
        assert_eq!(model.clip_index("idle"), None);
    }

    #[test]
    fn bounds_contain_every_submesh() {
        let model = three_level_model();
        let Some(lod) = model.lod0() else {
            panic!("LOD0 must exist");
        };
        assert!(lod.bounds().radius >= 1.0);
        assert_eq!(lod.vertex_count(), 3);
    }
}
