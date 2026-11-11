//! Skinned draws, and the description of an uploaded model.
//!
//! Two things live here that the rest of `world.rs` does not need:
//!
//! ```text
//! SkinnedDrawList     one entry per skinned submesh visible this frame, plus the bone
//!                     matrices they index. Rebuilt every frame, not slot-managed.
//!
//! ModelBinding        what one uploaded model became: per level of detail, a list of
//!                     (geometry, material, bounds). Produced by the renderer at load,
//!                     read by extraction every frame.
//! ```
//!
//! # Why skinned entities are not `RenderWorld` slots
//!
//! A `RenderEntity` exists so a dummy keeps its slot, its handle and its place in the
//! culler's ordering across frames — the churn that mattered when hundreds of them are
//! spawned and despawned. A skinned character is the opposite case: there are a handful, and
//! each one carries a bone palette that is *entirely* rewritten every frame anyway. Giving
//! them slots would buy stability for the one field that has none.
//!
//! So the skinned list is cleared and refilled per frame, like the UI draw list, and for the
//! same reason.
//!
//! # Why the palette is one flat array
//!
//! Every skinned instance's matrices live end to end in [`SkinnedDrawList::palette`], and an
//! instance names a range. One allocation that reaches a high-water mark and stops growing,
//! rather than a `Vec<Mat4>` per character per frame — and the renderer uploads a contiguous
//! slice rather than gathering.

use engine_core::math::{Mat4, Sphere};
use engine_core::transform::Transform;

use crate::world::{Handle, InstanceStyle, MaterialHandle, MeshHandle, declare_handle};

declare_handle!(SkinnedMeshHandle, "an uploaded skinned mesh");

/// The most bones one skinned draw may use.
///
/// Mirrors `engine_assets::skeleton::MAX_BONES`. The two are separate constants because the
/// crates do not depend on each other; `a_palette_never_exceeds_the_shader_array` pins them
/// together from the renderer's side, where both are visible.
pub const MAX_PALETTE_BONES: usize = 128;

/// Where one skinned submesh's geometry lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartGeometry {
    /// A rigid mesh, drawn through the instanced batch path.
    Rigid(MeshHandle),
    /// A skinned mesh, drawn through the skinning pipeline with a bone palette.
    Skinned(SkinnedMeshHandle),
}

impl PartGeometry {
    /// Whether this part needs a bone palette.
    #[must_use]
    pub fn is_skinned(self) -> bool {
        matches!(self, Self::Skinned(_))
    }
}

/// One material's worth of an uploaded model: what to draw and what to draw it with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPart {
    /// The geometry.
    pub geometry: PartGeometry,
    /// The surface.
    pub material: MaterialHandle,
    /// Local-space bounds, in the bind pose for skinned geometry.
    pub bounds: Sphere,
}

/// One level of detail of an uploaded model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelLodBinding {
    /// The parts to draw, one per material.
    pub parts: Vec<ModelPart>,
    /// Distance in metres at which this level takes over. Level 0 is always 0.
    pub min_distance: f32,
    /// Local-space bounds covering every part.
    pub bounds: Sphere,
}

/// What one uploaded model became.
///
/// The replacement for the fixed `player_mesh` / `dummy_mesh` fields the phase started with.
/// It is produced by the renderer — which owns the handles — and read by extraction, which
/// owns nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelBinding {
    /// Levels of detail, finest first. Never empty.
    pub lods: Vec<ModelLodBinding>,
    /// Whether the model is drawn through the skinning pipeline.
    pub skinned: bool,
    /// How tall the model was authored, in metres, so a caller can scale it to a gameplay
    /// height without re-measuring the geometry.
    pub model_height: f32,
}

impl ModelBinding {
    /// Which level to draw at `distance` metres, given a bias.
    ///
    /// The same rule as `engine_assets::ModelAsset::select_lod`, restated here because
    /// extraction has the binding and not the asset. `lod_bias` at or below zero pins the
    /// finest level, and a non-finite distance does the same rather than indexing on the
    /// result of a `NaN` comparison.
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
                break;
            }
        }
        selected
    }

    /// The level at `index`, or the finest one if `index` is past the end.
    #[must_use]
    pub fn lod(&self, index: usize) -> Option<&ModelLodBinding> {
        self.lods.get(index).or_else(|| self.lods.first())
    }
}

/// One skinned submesh to draw this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinnedInstance {
    /// Where it is.
    pub transform: Transform,
    /// What to draw.
    pub mesh: SkinnedMeshHandle,
    /// How it looks.
    pub material: MaterialHandle,
    /// World-space bounds, already placed by the caller.
    pub bounds: Sphere,
    /// Per-instance visual state.
    pub style: InstanceStyle,
    /// Where this instance's bone matrices start in [`SkinnedDrawList::palette`].
    pub palette_start: u32,
    /// How many bones it uses.
    pub palette_len: u32,
    /// Whether the shadow passes render this submesh's depth.
    ///
    /// The skinned counterpart of
    /// [`ShadowCasting::casts`](crate::world::ShadowCasting::casts), carried on the instance
    /// rather than in a parallel array because a skinned draw list is rebuilt whole every
    /// frame. True for a character: the player and every monster cast, which is the single
    /// most visible shadow in a third-person game.
    pub casts_shadow: bool,
    /// Whether the lighting pass darkens this submesh where a shadow falls on it.
    pub receives_shadow: bool,
}

/// Every skinned draw for one frame, and the matrices they index.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SkinnedDrawList {
    instances: Vec<SkinnedInstance>,
    palette: Vec<Mat4>,
}

impl SkinnedDrawList {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Empties the list, keeping its allocations.
    pub fn clear(&mut self) {
        self.instances.clear();
        self.palette.clear();
    }

    /// Adds `matrices` to the palette and returns the range they occupy.
    ///
    /// Called once per skinned *character*; the submeshes that follow all name the same
    /// range, because a character's parts share its skeleton.
    ///
    /// Returns `None` if `matrices` is empty or longer than [`MAX_PALETTE_BONES`] — the
    /// shader's array is a fixed size, and a palette past its end reads whatever follows.
    pub fn push_palette(&mut self, matrices: &[Mat4]) -> Option<(u32, u32)> {
        if matrices.is_empty() || matrices.len() > MAX_PALETTE_BONES {
            return None;
        }
        let start = self.palette.len() as u32;
        self.palette.extend_from_slice(matrices);
        Some((start, matrices.len() as u32))
    }

    /// Adds one submesh to draw.
    pub fn push(&mut self, instance: SkinnedInstance) {
        self.instances.push(instance);
    }

    /// Replaces the palette wholesale, reusing the allocation.
    ///
    /// What a consumer that filters the *instances* — the renderer's cull — does with the
    /// matrices: an instance names a range by absolute index, so compacting the palette
    /// alongside the filter would mean rewriting every surviving instance's range to save a
    /// few kilobytes of memcpy.
    pub fn set_palette(&mut self, matrices: &[Mat4]) {
        self.palette.clear();
        self.palette.extend_from_slice(matrices);
    }

    /// The draws.
    #[must_use]
    pub fn instances(&self) -> &[SkinnedInstance] {
        &self.instances
    }

    /// The bone matrices every instance indexes.
    #[must_use]
    pub fn palette(&self) -> &[Mat4] {
        &self.palette
    }

    /// How many draws there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Whether there is nothing to draw.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// The matrices instance `index` uses, or `None` if it names a range outside the palette.
    #[must_use]
    pub fn palette_of(&self, index: usize) -> Option<&[Mat4]> {
        let instance = self.instances.get(index)?;
        let start = instance.palette_start as usize;
        let end = start + instance.palette_len as usize;
        self.palette.get(start..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::Vec3;

    fn binding(distances: &[f32]) -> ModelBinding {
        ModelBinding {
            lods: distances
                .iter()
                .map(|distance| ModelLodBinding {
                    parts: Vec::new(),
                    min_distance: *distance,
                    bounds: Sphere::new(Vec3::ZERO, 1.0),
                })
                .collect(),
            skinned: false,
            model_height: 1.8,
        }
    }

    #[test]
    fn the_level_is_the_last_threshold_the_distance_has_passed() {
        let binding = binding(&[0.0, 12.0, 30.0]);
        assert_eq!(binding.select_lod(0.0, 1.0), 0);
        assert_eq!(binding.select_lod(12.0, 1.0), 1);
        assert_eq!(binding.select_lod(29.9, 1.0), 1);
        assert_eq!(binding.select_lod(400.0, 1.0), 2);
    }

    #[test]
    fn a_bias_moves_the_boundaries_and_a_nonsense_distance_pins_the_finest_level() {
        let binding = binding(&[0.0, 12.0, 30.0]);
        assert_eq!(binding.select_lod(6.0, 2.0), 1);
        assert_eq!(binding.select_lod(400.0, 0.0), 0);
        assert_eq!(binding.select_lod(f32::NAN, 1.0), 0);
    }

    #[test]
    fn a_level_past_the_end_falls_back_to_the_finest_rather_than_drawing_nothing() {
        let binding = binding(&[0.0, 12.0]);
        assert!(binding.lod(0).is_some());
        assert_eq!(binding.lod(9).map(|lod| lod.min_distance), Some(0.0));
    }

    #[test]
    fn a_palette_names_the_range_it_was_written_to() {
        let mut list = SkinnedDrawList::new();
        let Some((first_start, first_len)) = list.push_palette(&[Mat4::IDENTITY; 3]) else {
            panic!("three bones is a valid palette");
        };
        let Some((second_start, second_len)) = list.push_palette(&[Mat4::IDENTITY; 5]) else {
            panic!("five bones is a valid palette");
        };
        assert_eq!((first_start, first_len), (0, 3));
        assert_eq!((second_start, second_len), (3, 5));
        assert_eq!(list.palette().len(), 8);
    }

    #[test]
    fn a_palette_larger_than_the_shader_array_is_refused_rather_than_read_past() {
        // The shader declares a fixed-size array; a longer palette reads whatever follows
        // it in the upload ring, which is another object's constants.
        let mut list = SkinnedDrawList::new();
        assert!(
            list.push_palette(&[Mat4::IDENTITY; MAX_PALETTE_BONES + 1])
                .is_none()
        );
        assert!(list.push_palette(&[]).is_none());
        assert!(list.palette().is_empty());
    }

    #[test]
    fn an_instance_reads_back_exactly_the_matrices_it_named() {
        let mut list = SkinnedDrawList::new();
        let Some((start, len)) = list.push_palette(&[
            Mat4::from_translation(Vec3::X),
            Mat4::from_translation(Vec3::Y),
        ]) else {
            panic!("two bones is a valid palette");
        };
        list.push(SkinnedInstance {
            transform: Transform::IDENTITY,
            mesh: SkinnedMeshHandle::new(0, 1),
            material: MaterialHandle::new(0, 1),
            bounds: Sphere::new(Vec3::ZERO, 1.0),
            style: InstanceStyle::NEUTRAL,
            palette_start: start,
            palette_len: len,
            casts_shadow: true,
            receives_shadow: true,
        });
        let Some(palette) = list.palette_of(0) else {
            panic!("the instance names a range inside the palette");
        };
        assert_eq!(palette.len(), 2);
        assert_eq!(palette[1].transform_point(Vec3::ZERO), Vec3::Y);
    }

    #[test]
    fn clearing_keeps_the_allocation_and_empties_both_arrays() {
        let mut list = SkinnedDrawList::new();
        assert!(list.push_palette(&[Mat4::IDENTITY; 4]).is_some());
        list.push(SkinnedInstance {
            transform: Transform::IDENTITY,
            mesh: SkinnedMeshHandle::new(0, 1),
            material: MaterialHandle::new(0, 1),
            bounds: Sphere::new(Vec3::ZERO, 1.0),
            style: InstanceStyle::NEUTRAL,
            palette_start: 0,
            palette_len: 4,
            casts_shadow: true,
            receives_shadow: true,
        });
        list.clear();
        assert!(list.is_empty());
        assert!(list.palette().is_empty());
        assert_eq!(list.len(), 0);
    }
}
