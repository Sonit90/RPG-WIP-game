//! Which entities are drawn into a shadow view.
//!
//! The shadow counterpart of [`crate::culling`], and deliberately built out of the same
//! parts: it walks the same flat [`RenderWorld`] arrays, it appends to the same
//! [`RenderList`], and the list is grouped by material and mesh by the same counting sort —
//! so the shadow pass gets instanced batching for free and a caster that is one draw in the
//! camera pass is one draw in the shadow pass.
//!
//! ```text
//! cascade bounds
//!      | frustum test against the light's own projection
//!      v
//! shadow caster culling
//!      | casts_shadow, then how many texels the shadow would cover
//!      v
//! shadow render list          (mesh handles already swapped for the LOD's)
//!      | grouped by material and mesh
//!      v
//! depth rendering
//! ```
//!
//! There is no second scene traversal and no shadow-specific spatial structure: §9 asks for
//! the existing culling to be reused, and the existing culling is a linear pass over an array
//! of bounding spheres that a cascade's frustum tests exactly as well as the camera's does.

use engine_core::math::{Frustum, Vec3};
use engine_render_data::RenderWorld;

use crate::render_list::RenderList;

use super::cascade::{CascadeView, shadow_lod_for};
use super::local::LocalShadowView;
use super::settings::{ShadowLod, ShadowSettings};

/// What one caster-gathering pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ShadowCullStats {
    /// Entities that were drawable and asked to cast.
    pub considered: u32,
    /// Casters that survived and will be drawn.
    pub visible: u32,
    /// Casters rejected by the light's frustum.
    pub culled: u32,
    /// Casters rejected for being too small to resolve.
    pub too_small: u32,
    /// How many survivors landed on each LOD level, indexed by level.
    pub lod_counts: [u32; 3],
}

/// Which of the world's slots a gathering pass looks at.
#[derive(Debug, Clone, Copy)]
pub enum CasterSlots<'a> {
    /// Every slot.
    All,
    /// Only the slots spawned static, for a list that is kept across frames.
    Static,
    /// Exactly these slots: the frame's movers, gathered once and tested per view.
    Only(&'a [u32]),
}

/// Gathers shadow casters into render lists.
///
/// Holds only statistics, exactly like [`Culler`](crate::culling::Culler): the lists are the
/// caller's and are kept across frames, so after the scene settles this allocates nothing.
#[derive(Debug, Default)]
pub struct ShadowCasterCollector {
    stats: ShadowCullStats,
}

impl ShadowCasterCollector {
    /// A collector that has run no passes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// What the last pass did.
    #[must_use]
    pub fn stats(&self) -> ShadowCullStats {
        self.stats
    }

    /// Fills `list` with the casters that reach `cascade`.
    ///
    /// The mesh written into the list is already the one the shadow LOD chose, so recording
    /// never has to ask again — and a caster whose stand-in differs from its main mesh sorts
    /// into a different batch, which is correct: it is different geometry.
    pub fn collect_cascade(
        &mut self,
        world: &RenderWorld,
        cascade: &CascadeView,
        cascade_index: usize,
        settings: &ShadowSettings,
        list: &mut RenderList,
    ) {
        let frustum = cascade.frustum();
        self.collect_cascade_with(
            world,
            &frustum,
            cascade,
            cascade_index,
            settings,
            CasterSlots::All,
            list,
        );
    }

    /// [`ShadowCasterCollector::collect_cascade`] against `frustum` rather than the
    /// cascade's own, over `slots` only.
    ///
    /// The frustum is what a cached list is grown by ([`CascadeView::expanded_frustum`]);
    /// the level of detail is still the cascade's own, which depends on the caster's size
    /// and the cascade's texel and on nothing the camera does.
    #[allow(
        clippy::too_many_arguments,
        reason = "the cascade pass's inputs plus the two that vary between its callers"
    )]
    pub fn collect_cascade_with(
        &mut self,
        world: &RenderWorld,
        frustum: &Frustum,
        cascade: &CascadeView,
        cascade_index: usize,
        settings: &ShadowSettings,
        slots: CasterSlots<'_>,
        list: &mut RenderList,
    ) {
        self.collect_with_bounds(world, frustum, list, slots, |_, radius| {
            shadow_lod_for(radius, cascade, &settings.lod, cascade_index)
        });
    }

    /// Fills `list` with the casters that reach a local light's view.
    ///
    /// The size test is the same one the cascades use, with the texel size worked out for a
    /// perspective view: a shadow map covering a cone gets coarser with distance from the
    /// light, so a caster at the edge of a torch's reach is worth less of the map than one
    /// beside it.
    pub fn collect_local(
        &mut self,
        world: &RenderWorld,
        view: &LocalShadowView,
        resolution: u32,
        settings: &ShadowSettings,
        list: &mut RenderList,
    ) {
        self.collect_local_with(world, view, resolution, settings, CasterSlots::All, list);
    }

    /// [`ShadowCasterCollector::collect_local`] over `slots` only.
    pub fn collect_local_with(
        &mut self,
        world: &RenderWorld,
        view: &LocalShadowView,
        resolution: u32,
        settings: &ShadowSettings,
        slots: CasterSlots<'_>,
        list: &mut RenderList,
    ) {
        let frustum = view.frustum();
        let origin = view.bounds.center;
        let resolution = resolution.max(1) as f32;
        let bounds = world.bounds();
        self.collect_with_bounds(world, &frustum, list, slots, |slot, radius| {
            // Texels the caster covers: its angular size, divided by the angle one texel
            // spans. A ninety-degree face across `resolution` texels is the worst case, and
            // using it for a spot light's narrower cone only makes the test conservative.
            let distance = (bounds[slot].center - origin).length().max(1e-3);
            let texel_angle = std::f32::consts::FRAC_PI_2 / resolution;
            let texels = (radius * 2.0 / distance) / texel_angle;
            if texels < settings.lod.cull_texels {
                ShadowLod::None
            } else if settings.lod.enabled {
                // Local maps are small and near, so their casters step down one level at
                // the far half of the light's reach rather than per cascade.
                let far = view.far.max(1e-3);
                ShadowLod::Full.coarser(u32::from(distance > far * 0.5))
            } else {
                ShadowLod::Full
            }
        });
    }

    /// The body both passes share.
    ///
    /// One pass over the arrays, testing the cheapest thing first: the flags are a byte
    /// each and reject a decal before its bounds are ever loaded.
    fn collect_with_bounds(
        &mut self,
        world: &RenderWorld,
        frustum: &Frustum,
        list: &mut RenderList,
        slots: CasterSlots<'_>,
        level_of: impl Fn(usize, f32) -> ShadowLod,
    ) {
        list.clear();
        let mut stats = ShadowCullStats::default();
        let statics = world.static_flags();

        let bounds = world.bounds();
        let transforms = world.transforms();
        let meshes = world.meshes();
        let materials = world.materials();
        let styles = world.styles();
        let shadow = world.shadow_casting();
        let flags = world.visible_flags();

        let mut gather = |slot: usize| {
            if !flags[slot] || !shadow[slot].casts {
                return;
            }
            stats.considered += 1;
            if !frustum.intersects_sphere(&bounds[slot]) {
                stats.culled += 1;
                return;
            }
            let level = level_of(slot, bounds[slot].radius);
            let Some(mesh_level) = level.mesh_level() else {
                stats.too_small += 1;
                return;
            };
            stats.lod_counts[mesh_level.min(2)] += 1;
            stats.visible += 1;
            list.push(
                world.entity_at(slot),
                transforms[slot],
                shadow[slot].mesh_for_level(meshes[slot], mesh_level),
                materials[slot],
                styles[slot],
                // A caster's own "does it receive shadows" is irrelevant to the depth pass;
                // the field exists for the camera list, and false here would be read by
                // nothing. It is set to what the entity actually says so a list dumped for
                // debugging is not misleading.
                shadow[slot].receives,
            );
        };
        match slots {
            CasterSlots::All => (0..world.slot_count()).for_each(&mut gather),
            CasterSlots::Static => (0..world.slot_count())
                .filter(|&slot| statics.get(slot).copied().unwrap_or(false))
                .for_each(&mut gather),
            CasterSlots::Only(only) => only.iter().for_each(|&slot| gather(slot as usize)),
        }

        stats.culled = stats.considered - stats.visible - stats.too_small;
        self.stats = stats;
    }
}

/// Whether a skinned instance should be drawn into a shadow view.
///
/// Skinned draws are not slot-managed — the whole list is rebuilt every frame — so they are
/// filtered with a function rather than gathered into a second list: the shadow pass walks
/// the same [`SkinnedDrawList`](engine_render_data::SkinnedDrawList) the camera pass does
/// and skips what this rejects.
#[must_use]
pub fn skinned_caster_visible(
    instance: &engine_render_data::SkinnedInstance,
    frustum: &Frustum,
) -> bool {
    instance.casts_shadow && frustum.intersects_sphere(&instance.bounds)
}

/// How far a point is in front of the camera, along its view direction.
///
/// The depth the cascade selection is made against, on the CPU. The shader computes the same
/// number from the interpolated world position; having it here is what lets a test assert
/// which cascade a given object lands in.
#[must_use]
pub fn view_depth_of(point: Vec3, camera_position: Vec3, camera_forward: Vec3) -> f32 {
    (point - camera_position).dot(camera_forward)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::shadow::cascade::build_cascades;
    use crate::shadow::settings::ShadowPreset;
    use engine_core::math::{Sphere, Vec3};
    use engine_core::transform::Transform;
    use engine_render_data::{
        Camera, MaterialHandle, MeshHandle, Projection, RenderEntityDesc, ShadowCasting,
    };

    fn camera() -> Camera {
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid aspect");
        camera.set_position(Vec3::new(0.0, 2.0, 0.0));
        camera
    }

    fn prop(world: &mut RenderWorld, position: Vec3, radius: f32, shadow: ShadowCasting) {
        world.spawn(RenderEntityDesc {
            transform: Transform::from_translation(position),
            mesh: MeshHandle::new(0, 1),
            material: MaterialHandle::new(0, 1),
            bounds: Sphere::new(Vec3::ZERO, radius),
            shadow,
            ..RenderEntityDesc::default()
        });
    }

    #[test]
    fn an_entity_that_does_not_cast_is_never_gathered() {
        let settings = ShadowPreset::High.settings();
        let camera = camera();
        let mut cascades = Vec::new();
        build_cascades(&settings, &camera, Vec3::new(0.0, -1.0, 0.0), &mut cascades);

        let mut world = RenderWorld::new();
        prop(
            &mut world,
            Vec3::new(0.0, 1.0, -5.0),
            1.0,
            ShadowCasting::CASTS_AND_RECEIVES,
        );
        prop(
            &mut world,
            Vec3::new(1.0, 1.0, -5.0),
            1.0,
            ShadowCasting::RECEIVES_ONLY,
        );
        prop(
            &mut world,
            Vec3::new(2.0, 1.0, -5.0),
            1.0,
            ShadowCasting::NONE,
        );

        let mut collector = ShadowCasterCollector::new();
        let mut list = RenderList::new();
        collector.collect_cascade(&world, &cascades[0], 0, &settings, &mut list);

        assert_eq!(list.len(), 1, "only the caster belongs in a depth pass");
        assert_eq!(collector.stats().considered, 1);
    }

    #[test]
    fn a_caster_outside_the_cascade_is_culled() {
        let settings = ShadowPreset::High.settings();
        let camera = camera();
        let mut cascades = Vec::new();
        build_cascades(&settings, &camera, Vec3::new(0.0, -1.0, 0.0), &mut cascades);

        let mut world = RenderWorld::new();
        prop(
            &mut world,
            Vec3::new(0.0, 1.0, -5.0),
            1.0,
            ShadowCasting::default(),
        );
        // Far off to the side of the first cascade, which covers about fifteen metres.
        prop(
            &mut world,
            Vec3::new(400.0, 1.0, -5.0),
            1.0,
            ShadowCasting::default(),
        );

        let mut collector = ShadowCasterCollector::new();
        let mut list = RenderList::new();
        collector.collect_cascade(&world, &cascades[0], 0, &settings, &mut list);
        assert_eq!(list.len(), 1);
        assert_eq!(collector.stats().culled, 1);
    }

    #[test]
    fn a_caster_above_the_cascade_is_kept_because_it_shadows_into_it() {
        // The extruded region: outside the cascade's sphere, still between it and the sun.
        let settings = ShadowPreset::High.settings();
        let camera = camera();
        let mut cascades = Vec::new();
        build_cascades(&settings, &camera, Vec3::new(0.0, -1.0, 0.0), &mut cascades);

        let mut world = RenderWorld::new();
        prop(
            &mut world,
            cascades[0].center + Vec3::new(0.0, 30.0, 0.0),
            1.0,
            ShadowCasting::default(),
        );
        let mut collector = ShadowCasterCollector::new();
        let mut list = RenderList::new();
        collector.collect_cascade(&world, &cascades[0], 0, &settings, &mut list);
        assert_eq!(
            list.len(),
            1,
            "a caster overhead must still reach the cascade"
        );
    }

    #[test]
    fn a_caster_too_small_to_resolve_is_dropped_by_the_last_cascade() {
        let settings = ShadowPreset::High.settings();
        let camera = camera();
        let mut cascades = Vec::new();
        build_cascades(&settings, &camera, Vec3::new(0.0, -1.0, 0.0), &mut cascades);

        let mut world = RenderWorld::new();
        prop(
            &mut world,
            cascades[2].center,
            0.01,
            ShadowCasting::default(),
        );

        let mut collector = ShadowCasterCollector::new();
        let mut list = RenderList::new();
        collector.collect_cascade(&world, &cascades[2], 2, &settings, &mut list);
        assert!(list.is_empty());
        assert_eq!(collector.stats().too_small, 1);
    }

    #[test]
    fn a_coarser_level_substitutes_the_mesh_the_entity_offers() {
        let settings = ShadowPreset::High.settings();
        let camera = camera();
        let mut cascades = Vec::new();
        build_cascades(&settings, &camera, Vec3::new(0.0, -1.0, 0.0), &mut cascades);

        let coarse = MeshHandle::new(7, 1);
        let mut world = RenderWorld::new();
        // At the near cascade's centre, which the second cascade's much larger box also
        // contains — so the same object can be asked for by both and the answers compared.
        world.spawn(RenderEntityDesc {
            transform: Transform::from_translation(cascades[0].center),
            mesh: MeshHandle::new(0, 1),
            material: MaterialHandle::new(0, 1),
            bounds: Sphere::new(Vec3::ZERO, 2.0),
            shadow: ShadowCasting::CASTS_AND_RECEIVES
                .with_lod_meshes([coarse, MeshHandle::INVALID]),
            ..RenderEntityDesc::default()
        });

        let mut collector = ShadowCasterCollector::new();
        let mut list = RenderList::new();
        collector.collect_cascade(&world, &cascades[1], 1, &settings, &mut list);
        assert_eq!(
            list.meshes(),
            &[coarse],
            "cascade 1 asks for one level coarser"
        );

        collector.collect_cascade(&world, &cascades[0], 0, &settings, &mut list);
        assert_eq!(
            list.meshes(),
            &[MeshHandle::new(0, 1)],
            "cascade 0 draws what the camera draws"
        );
    }

    #[test]
    fn gathering_reuses_the_lists_allocation() {
        let settings = ShadowPreset::High.settings();
        let camera = camera();
        let mut cascades = Vec::new();
        build_cascades(&settings, &camera, Vec3::new(0.0, -1.0, 0.0), &mut cascades);

        let mut world = RenderWorld::new();
        for index in 0..64 {
            prop(
                &mut world,
                cascades[0].center + Vec3::new(index as f32 * 0.1, 0.0, 0.0),
                0.5,
                ShadowCasting::default(),
            );
        }
        let mut collector = ShadowCasterCollector::new();
        let mut list = RenderList::new();
        for _ in 0..4 {
            collector.collect_cascade(&world, &cascades[0], 0, &settings, &mut list);
        }
        let capacity = list.capacity();
        collector.collect_cascade(&world, &cascades[0], 0, &settings, &mut list);
        assert_eq!(list.capacity(), capacity);
    }
}
