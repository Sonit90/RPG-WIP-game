//! What the shadow views keep of the static scene from one frame to the next.
//!
//! # Static casters, recorded once
//!
//! A level's walls, floors, trees and gravestones are spawned once and never move
//! ([`RenderWorld::spawn_static`]). Gathering them into every shadow view, grouping them and
//! composing their instance matrices again every frame was most of the shadow stage's CPU
//! time, for an answer that changes only when the static scene does or when a view moves
//! far enough to see different casters. So each view keeps its static casters — the grouped
//! list and the composed instances, in batch order — and rebuilds them only then.
//!
//! What is *not* cached is the depth. The GPU still rasterizes every static caster into the
//! atlas every frame: that is cheap (a few hundredths of a millisecond for the whole atlas),
//! and keeping a static depth map instead would cost a full-tile copy per view per frame,
//! which is more than drawing the casters again.
//!
//! ```text
//! static casters   gathered when the cache is stale, then memcpy'd into the ring per frame
//! movers           gathered per frame from the frame's short mover list (dummies, ...)
//! characters       drawn per frame, as before
//! ```
//!
//! # When a view's casters are stale
//!
//! * **Every view**, when [`RenderWorld::static_generation`] moves (a static entity spawned,
//!   despawned, moved, hidden or re-shaded: a level-of-detail swap, an editor edit) or the
//!   shadow settings change.
//! * **A local view** is keyed by its light and face and by its exact view-projection. Static
//!   lights never move, so its casters are gathered once per light per scene; the selection
//!   choosing other lights from frame to frame does not touch them.
//! * **A cascade** follows the camera. Its casters are gathered for the box grown by a margin
//!   ([`CascadeView::expanded_frustum`]), and reused while the box's snapped centre stays
//!   within that margin of where it was. Exact: the grown list holds every caster the current
//!   box could need, and the extras land outside its tile or past its far plane.
//!
//! [`RenderWorld::spawn_static`]: engine_render_data::RenderWorld::spawn_static
//! [`RenderWorld::static_generation`]: engine_render_data::RenderWorld::static_generation

use std::collections::HashMap;

use engine_core::math::Vec3;

use crate::mesh_pipeline::InstanceTransform;
use crate::render_list::RenderList;

use super::cascade::CascadeView;
use super::settings::ShadowSettings;

/// Default margin a cached cascade may drift before its casters are gathered again, as a
/// fraction of the cascade's radius.
pub const DEFAULT_CASCADE_MARGIN: f32 = 0.15;

/// One view's static casters, as recorded the last time they were gathered.
#[derive(Debug, Default)]
pub struct StaticView {
    /// What the casters were gathered for. `None` until the first gather, and after an
    /// invalidation.
    key: Option<ViewKey>,
    /// The casters, grouped by material and mesh.
    pub list: RenderList,
    /// Their per-instance data, composed once, in the order the batches walk.
    pub instances: Vec<InstanceTransform>,
}

impl StaticView {
    /// Forgets what this view holds; the next frame gathers it again.
    fn invalidate(&mut self) {
        self.key = None;
    }

    /// Records that the list and the instances now describe `key`.
    fn rebuilt(&mut self, key: ViewKey) {
        self.instances.clear();
        self.instances
            .extend(self.list.order().iter().map(|&entry| {
                let entry = entry as usize;
                InstanceTransform::shadowed(
                    self.list.transforms()[entry].matrix(),
                    self.list.styles()[entry],
                    self.list.receives_shadow()[entry],
                )
            }));
        self.key = Some(key);
    }
}

/// What a cached list was gathered for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ViewKey {
    /// A cascade: its light space, its size and where its grown box was centred.
    Cascade {
        light_view: [f32; 16],
        radius: f32,
        texel: f32,
        center: Vec3,
        margin: f32,
    },
    /// A local view: its exact matrix. A static light's never changes.
    Local { view_projection: [f32; 16] },
}

impl ViewKey {
    fn cascade(view: &CascadeView, margin: f32) -> Self {
        Self::Cascade {
            light_view: view.light_view.to_cols_array(),
            radius: view.radius,
            texel: view.texel_world_size,
            center: view.light_center,
            margin,
        }
    }

    /// Whether a list gathered for `self` is still complete for `current`.
    ///
    /// A containment test rather than an equality: the current box must lie inside the grown
    /// one on every light-space axis. The radius is *mathematically* constant but is computed
    /// from the rotated frustum's corners, so it differs in its last bits from one camera
    /// angle to the next — an equality test failed on every frame the camera turned.
    fn covers(&self, current: &CascadeView) -> bool {
        let Self::Cascade {
            light_view,
            radius,
            texel: _,
            center,
            margin,
        } = *self
        else {
            return false;
        };
        // How far off-centre the current box may sit and still be inside the grown one.
        let slack = radius + margin - current.radius;
        let moved = current.light_center - center;
        light_view == current.light_view.to_cols_array()
            && slack >= 0.0
            && moved.x.abs() <= slack
            && moved.y.abs() <= slack
            && moved.z.abs() <= slack
    }
}

/// What the cache did in one frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StaticCacheStats {
    /// Views whose static casters came from the cache this frame.
    pub reused: u32,
    /// Views whose static casters were gathered again this frame.
    pub rebuilt: u32,
    /// Static instances drawn this frame, across every view.
    pub cached_instances: u32,
    /// Moving instances drawn this frame, across every view.
    pub moving_instances: u32,
}

/// Every view's static casters, and when they are stale.
#[derive(Debug)]
pub struct StaticShadowCache {
    /// Off draws every caster as a mover, every frame: the A/B control.
    enabled: bool,
    /// How far, as a fraction of its radius, a cascade may drift before it is gathered again.
    margin_fraction: f32,
    /// The world's static generation the views were gathered against.
    generation: Option<u64>,
    /// The settings they were gathered with.
    settings: Option<ShadowSettings>,
    cascades: Vec<StaticView>,
    locals: HashMap<(usize, u8), StaticView>,
    stats: StaticCacheStats,
}

impl Default for StaticShadowCache {
    fn default() -> Self {
        Self {
            enabled: true,
            margin_fraction: DEFAULT_CASCADE_MARGIN,
            generation: None,
            settings: None,
            cascades: Vec::new(),
            locals: HashMap::new(),
            stats: StaticCacheStats::default(),
        }
    }
}

impl StaticShadowCache {
    /// Whether static casters are cached at all.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Turns the cache on or off. Off forgets everything it held.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.invalidate_all();
    }

    /// Sets the cascade margin, as a fraction of each cascade's radius.
    pub fn set_margin_fraction(&mut self, fraction: f32) {
        self.margin_fraction = if fraction.is_finite() {
            fraction.max(0.0)
        } else {
            DEFAULT_CASCADE_MARGIN
        };
        self.invalidate_all();
    }

    /// Starts a frame: drops every view if the static scene or the settings changed.
    pub fn begin_frame(&mut self, generation: u64, settings: &ShadowSettings) {
        self.stats = StaticCacheStats::default();
        if self.generation != Some(generation) || self.settings.as_ref() != Some(settings) {
            self.invalidate_all();
            self.generation = Some(generation);
            self.settings = Some(*settings);
        }
    }

    /// What the cache did this frame.
    #[must_use]
    pub fn stats(&self) -> StaticCacheStats {
        self.stats
    }

    /// Views holding casters right now.
    #[must_use]
    pub fn cached_views(&self) -> u32 {
        let cascades = self
            .cascades
            .iter()
            .filter(|view| view.key.is_some())
            .count();
        let locals = self
            .locals
            .values()
            .filter(|view| view.key.is_some())
            .count();
        (cascades + locals) as u32
    }

    fn invalidate_all(&mut self) {
        for view in &mut self.cascades {
            view.invalidate();
        }
        // Local views are keyed by light, and a light index is only meaningful within one
        // static scene: drop them rather than keep entries for lights that may be different
        // lights now.
        self.locals.clear();
    }

    /// Cascade `index`'s static casters for `view`, gathered by `gather` if they are stale.
    ///
    /// `gather` fills the list it is given for the frustum it is given — the cascade's box
    /// grown by the margin — and the instances are composed from it here.
    pub fn cascade(
        &mut self,
        index: usize,
        view: &CascadeView,
        gather: impl FnOnce(&engine_core::math::Frustum, &mut RenderList),
    ) -> &StaticView {
        if self.cascades.len() <= index {
            self.cascades.resize_with(index + 1, StaticView::default);
        }
        let margin = view.radius * self.margin_fraction;
        let entry = &mut self.cascades[index];
        if entry.key.is_some_and(|key| key.covers(view)) {
            self.stats.reused += 1;
        } else {
            gather(&view.expanded_frustum(margin), &mut entry.list);
            entry.rebuilt(ViewKey::cascade(view, margin));
            self.stats.rebuilt += 1;
        }
        self.stats.cached_instances += entry.instances.len() as u32;
        entry
    }

    /// The static casters of `light`'s view `face` (0 for a spot light), gathered by
    /// `gather` if they are stale or were never gathered.
    pub fn local(
        &mut self,
        light: usize,
        face: u8,
        view_projection: [f32; 16],
        gather: impl FnOnce(&mut RenderList),
    ) -> &StaticView {
        let key = ViewKey::Local { view_projection };
        let entry = self.locals.entry((light, face)).or_default();
        if entry.key == Some(key) {
            self.stats.reused += 1;
        } else {
            gather(&mut entry.list);
            entry.rebuilt(key);
            self.stats.rebuilt += 1;
        }
        self.stats.cached_instances += entry.instances.len() as u32;
        entry
    }

    /// Counts the moving instances one view draws, for the frame's statistics.
    pub fn count_moving(&mut self, instances: usize) {
        self.stats.moving_instances += instances as u32;
    }

    /// Cascade `index`'s current static view, if it was prepared this frame.
    #[must_use]
    pub fn cascade_view(&self, index: usize) -> Option<&StaticView> {
        self.cascades.get(index).filter(|view| view.key.is_some())
    }

    /// The current static view of `light`'s `face`, if it was prepared.
    #[must_use]
    pub fn local_view(&self, light: usize, face: u8) -> Option<&StaticView> {
        self.locals
            .get(&(light, face))
            .filter(|view| view.key.is_some())
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::shadow::cascade::build_cascades;
    use crate::shadow::caster::{CasterSlots, ShadowCasterCollector};
    use crate::shadow::settings::ShadowPreset;
    use engine_core::math::{Sphere, Vec3};
    use engine_core::transform::Transform;
    use engine_render_data::{
        Camera, MaterialHandle, MeshHandle, Projection, RenderEntityDesc, RenderWorld,
        ShadowCasting,
    };

    const SUN: Vec3 = Vec3::new(-0.62, -0.55, -0.56);

    fn camera_at(position: Vec3, yaw: f32) -> Camera {
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid aspect");
        camera.set_position(position);
        camera.set_rotation(engine_core::math::Quat::from_axis_angle(Vec3::Y, yaw));
        camera
    }

    /// A field of static props, scattered deterministically, a few metres apart.
    fn field() -> RenderWorld {
        let mut world = RenderWorld::new();
        let mut seed = 0x9e37_79b9_u32;
        for _ in 0..1500 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let x = (seed % 4000) as f32 / 10.0 - 200.0;
            let z = (seed / 4000 % 4000) as f32 / 10.0 - 200.0;
            let radius = 0.2 + (seed % 7) as f32 * 0.6;
            world.spawn_static(RenderEntityDesc {
                transform: Transform::from_translation(Vec3::new(x, radius, z)),
                mesh: MeshHandle::new(seed % 5, 1),
                material: MaterialHandle::new(seed % 3, 1),
                bounds: Sphere::new(Vec3::ZERO, radius),
                shadow: ShadowCasting::CASTS_AND_RECEIVES
                    .with_lod_meshes([MeshHandle::new(5, 1), MeshHandle::new(6, 1)]),
                ..RenderEntityDesc::default()
            });
        }
        world
    }

    fn gather_cascade(
        cache: &mut StaticShadowCache,
        collector: &mut ShadowCasterCollector,
        world: &RenderWorld,
        settings: &ShadowSettings,
        view: &CascadeView,
        index: usize,
    ) -> (bool, Vec<(u32, MeshHandle)>) {
        let before = cache.stats().rebuilt;
        let cached = cache.cascade(index, view, |frustum, list| {
            collector.collect_cascade_with(
                world,
                frustum,
                view,
                index,
                settings,
                CasterSlots::Static,
                list,
            );
            list.group_by_material_and_mesh(3, 7);
        });
        let entries = (0..cached.list.len())
            .map(|entry| {
                (
                    cached.list.entities()[entry].index() as u32,
                    cached.list.meshes()[entry],
                )
            })
            .collect();
        (cache.stats().rebuilt > before, entries)
    }

    #[test]
    fn a_cached_cascade_holds_every_caster_the_exact_cull_would_and_with_the_same_mesh() {
        // The property the whole cascade cache rests on: while it reuses a list, that list is
        // a superset of what gathering from scratch would produce, with the same level of
        // detail for every caster. Checked over a walk and a turn of the camera.
        let settings = ShadowPreset::High.settings();
        let world = field();
        let mut cache = StaticShadowCache::default();
        let mut collector = ShadowCasterCollector::new();
        cache.begin_frame(world.static_generation(), &settings);

        let mut reused = 0;
        for step in 0..120 {
            let t = step as f32;
            let camera = camera_at(Vec3::new(t * 0.4, 2.0, -t * 0.25), t * 0.02);
            let mut cascades = Vec::new();
            build_cascades(&settings, &camera, SUN, &mut cascades);
            for (index, view) in cascades.iter().enumerate() {
                let (rebuilt, cached) =
                    gather_cascade(&mut cache, &mut collector, &world, &settings, view, index);
                if !rebuilt {
                    reused += 1;
                }
                let mut exact = crate::render_list::RenderList::new();
                collector.collect_cascade(&world, view, index, &settings, &mut exact);
                for entry in 0..exact.len() {
                    let key = (
                        exact.entities()[entry].index() as u32,
                        exact.meshes()[entry],
                    );
                    assert!(
                        cached.contains(&key),
                        "step {step}, cascade {index}: caster {key:?} missing from the cache"
                    );
                }
            }
        }
        assert!(
            reused > 200,
            "the cache must actually be reused ({reused} of 360)"
        );
    }

    #[test]
    fn a_cascade_is_gathered_again_once_it_drifts_past_its_margin() {
        let settings = ShadowPreset::High.settings();
        let world = field();
        let mut cache = StaticShadowCache::default();
        let mut collector = ShadowCasterCollector::new();
        cache.begin_frame(world.static_generation(), &settings);

        let near = |x: f32| {
            let mut cascades = Vec::new();
            build_cascades(
                &settings,
                &camera_at(Vec3::new(x, 2.0, 0.0), 0.0),
                SUN,
                &mut cascades,
            );
            cascades[0]
        };
        let first = near(0.0);
        assert!(gather_cascade(&mut cache, &mut collector, &world, &settings, &first, 0).0);
        // A metre is well inside the near cascade's margin (15% of a ~19 m radius).
        let moved = near(1.0);
        assert!(!gather_cascade(&mut cache, &mut collector, &world, &settings, &moved, 0).0);
        // Twenty metres is not.
        let far = near(20.0);
        assert!(gather_cascade(&mut cache, &mut collector, &world, &settings, &far, 0).0);
    }

    #[test]
    fn a_local_view_is_gathered_once_until_the_static_scene_changes() {
        let settings = ShadowPreset::High.settings();
        let mut cache = StaticShadowCache::default();
        let mut gathered = 0;
        let matrix = [1.0; 16];
        for generation in [7, 7, 7, 8] {
            cache.begin_frame(generation, &settings);
            for _ in 0..3 {
                cache.local(3, 2, matrix, |_| gathered += 1);
                // Another light coming and going does not disturb it.
                cache.local(4, 0, matrix, |_| {});
            }
        }
        assert_eq!(gathered, 2, "once for generation 7, once for 8");

        // A different matrix for the same light and face is a different view.
        let mut regathered = false;
        cache.local(3, 2, [2.0; 16], |_| regathered = true);
        assert!(regathered);
    }

    #[test]
    fn a_settings_change_or_turning_the_cache_off_forgets_every_view() {
        let high = ShadowPreset::High.settings();
        let mut cache = StaticShadowCache::default();
        cache.begin_frame(1, &high);
        cache.local(0, 0, [1.0; 16], |_| {});
        assert_eq!(cache.cached_views(), 1);

        cache.begin_frame(1, &ShadowPreset::Ultra.settings());
        assert_eq!(cache.cached_views(), 0);

        cache.local(0, 0, [1.0; 16], |_| {});
        cache.set_enabled(false);
        assert_eq!(cache.cached_views(), 0);
        assert!(!cache.enabled());
    }
}
