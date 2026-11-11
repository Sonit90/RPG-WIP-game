//! The shadow stage's backend-neutral half: what the frame's shadow maps contain.
//!
//! Everything here is plain CPU code with no graphics API in it: the settings, the cascades,
//! the caster lists, the light selection, the statistics, and the constants the lighting pass
//! samples with. [`super::renderer::ShadowRenderer`] (Direct3D 12) owns one of these and
//! records the depth passes from its lists; another backend can do the same.
//!
//! ```text
//! prepare()                 cascades, caster culling, LOD, local-light selection
//!    |
//! write_frame_constants()   the matrices and rectangles the lighting pass samples with
//! ```
//!
//! "What is in the shadow map" is CPU work with no device in it and is testable without a
//! GPU, which is why it is kept apart from recording.

use engine_core::math::{Frustum, Mat4, Vec3};
use engine_render_data::{Camera, Lighting, RenderWorld};

use crate::render_list::RenderList;
use crate::scene::FrameConstants;

use super::atlas::{AtlasRect, ShadowAtlasLayout};
use super::cascade::{CascadeView, build_cascades};
use super::caster::{CasterSlots, ShadowCasterCollector};
use super::local::{LocalShadowView, SelectedLight, select_shadow_lights};
use super::settings::ShadowSettings;
use super::static_cache::StaticShadowCache;
use super::stats::ShadowStats;

/// What one depth pass is rendered with.
///
/// Deliberately *not* the frame constants: a shadow pass runs once per cascade and once per
/// local light, and re-uploading two kilobytes of lighting data for each of them to change
/// one matrix would be most of the pass's CPU cost.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ShadowViewConstants {
    /// The light's view-projection, column-major.
    pub view_projection: [f32; 16],
    /// `[1 for a cascade, 0 for a local view, 0, 0]`.
    pub params: [f32; 4],
}

impl ShadowViewConstants {
    /// The constants for one view.
    #[must_use]
    pub fn new(view_projection: Mat4, is_cascade: bool) -> Self {
        Self {
            view_projection: view_projection.to_cols_array(),
            params: [f32::from(u8::from(is_cascade)), 0.0, 0.0, 0.0],
        }
    }
}

/// The highest debug view `shade_surface` knows.
///
/// ```text
/// 0  none          4  base colour map     8  shading normal, after the TBN transform
/// 1  cascade tint  5  normal map          9  roughness, after the ORM map scales it
/// 2  shadow factor 6  ORM map            10  metallic, likewise
/// 3  the atlas     7  emissive map       11  occlusion
/// ```
///
/// 4 to 7 are the raw texels a descriptor resolved to; 8 to 11 are what the shading made of
/// them, which is the half a texel view cannot answer — a normal map bound correctly and
/// applied with the wrong handedness looks right at 5 and wrong at 8.
///
/// The shader's `if` chain and this number are the same fact stated twice, so raising one
/// without the other silently disables the new view.
pub const MAX_DEBUG_MODE: u32 = 11;

/// Decides what this frame's shadow maps contain, and keeps everything that decision reuses
/// from frame to frame.
///
/// The CPU half of the shadow stage. The fields are `pub(crate)` because the backend that
/// records the passes reads this frame's lists and rectangles directly.
pub struct ShadowPlan {
    pub(crate) settings: ShadowSettings,

    /// This frame's cascades and where each one lives in the atlas.
    pub(crate) cascades: Vec<CascadeView>,
    pub(crate) cascade_rects: Vec<AtlasRect>,
    /// One render list per cascade, kept across frames so the build stops allocating.
    pub(crate) cascade_lists: Vec<RenderList>,

    /// This frame's local shadow views, the lights that won them, and their rectangles.
    pub(crate) local_views: Vec<LocalShadowView>,
    pub(crate) local_selected: Vec<SelectedLight>,
    pub(crate) local_rects: Vec<AtlasRect>,
    pub(crate) local_lists: Vec<RenderList>,
    /// Scratch for the light sort, kept for the same reason.
    pub(crate) light_scratch: Vec<(usize, f32)>,

    pub(crate) collector: ShadowCasterCollector,

    pub(crate) stats: ShadowStats,
    /// 0 none, 1 cascade tint, 2 shadow factor only.
    pub(crate) debug_mode: u32,
    /// Whether the camera prepass runs for depth priming alone, without contact shadows.
    pub(crate) priming_prepass: bool,
    /// Every view's static casters, kept across frames.
    pub(crate) static_cache: StaticShadowCache,
    /// This frame's movers: casting slots that are not part of the static scene.
    pub(crate) movers: Vec<u32>,
    /// Whether the last cascade draws movers and characters. On by default; off is a measured
    /// trade-off (`--shadow-tweak far-movers=off`): something moving 50 m and more away loses
    /// its shadow, which is a few pixels at that range, for its draws in the largest view.
    pub(crate) far_movers: bool,
    /// Milliseconds the last `prepare` took, folded into the stats by `record`.
    pub(crate) prepare_ms: f64,
}

impl ShadowPlan {
    /// Creates an empty plan for `settings`, which the caller has already validated.
    #[must_use]
    pub fn new(settings: ShadowSettings) -> Self {
        Self {
            stats: ShadowStats::for_settings(&settings),
            settings,
            cascades: Vec::new(),
            cascade_rects: Vec::new(),
            cascade_lists: Vec::new(),
            local_views: Vec::new(),
            local_selected: Vec::new(),
            local_rects: Vec::new(),
            local_lists: Vec::new(),
            light_scratch: Vec::new(),
            collector: ShadowCasterCollector::new(),
            debug_mode: 0,
            priming_prepass: false,
            static_cache: StaticShadowCache::default(),
            movers: Vec::new(),
            far_movers: true,
            prepare_ms: 0.0,
        }
    }

    /// Replaces the settings, which the caller has already validated, and resets the stats.
    pub fn apply_settings(&mut self, settings: ShadowSettings) {
        self.settings = settings;
        self.stats = ShadowStats::for_settings(&self.settings);
        self.stats.prepass = self.needs_prepass();
    }

    /// What the system is currently configured with.
    #[must_use]
    pub fn settings(&self) -> &ShadowSettings {
        &self.settings
    }

    /// Selects the shader's shadow debug view.
    ///
    /// 0 none, 1 cascade tint, 2 shadow factor, 3 the atlas drawn in the corner, 4 to 7 the
    /// material texture set raw — base colour, normal, ORM, emissive, in the order
    /// [`crate::material::MaterialTextureSlot`] freezes — and 8 to 11 what the shading made
    /// of them: the normal after the TBN transform, roughness, metallic, occlusion. See
    /// [`MAX_DEBUG_MODE`]. Clamped rather than rejected: the shader's `if` chain falls
    /// through to "no debug view" for anything it does not know, and clamping here keeps the
    /// two ends agreeing on how many views there are.
    pub fn set_debug_mode(&mut self, mode: u32) {
        self.debug_mode = mode.min(MAX_DEBUG_MODE);
    }

    /// The current debug view.
    #[must_use]
    pub fn debug_mode(&self) -> u32 {
        self.debug_mode
    }

    /// What the last frame's shadow stage did.
    #[must_use]
    pub fn stats(&self) -> ShadowStats {
        self.stats
    }

    /// This frame's cascades, for a debug view that wants to draw their bounds.
    #[must_use]
    pub fn cascades(&self) -> &[CascadeView] {
        &self.cascades
    }

    /// Whether the depth prepass has to run this frame.
    ///
    /// Only contact shadows need it — they march against the depth the prepass wrote — so a
    /// configuration without them pays nothing for it.
    #[must_use]
    pub fn needs_prepass(&self) -> bool {
        self.needs_depth_copy() || self.priming_prepass
    }

    /// Whether the prepass's depth is copied for, and marched by, the contact shadows.
    ///
    /// Only when contact shadows are on. A prepass that runs for depth priming alone writes
    /// the camera's depth buffer and stops there.
    #[must_use]
    pub fn needs_depth_copy(&self) -> bool {
        self.settings.enabled && self.settings.contact.enabled
    }

    /// Runs the camera prepass even without contact shadows, so the camera pass can test
    /// against it instead of writing its own depth. See `DepthMode::Primed`.
    pub fn set_priming_prepass(&mut self, enabled: bool) {
        self.priming_prepass = enabled;
    }

    /// Whether the last cascade draws movers and characters (see the field).
    pub fn set_far_movers(&mut self, enabled: bool) {
        self.far_movers = enabled;
    }

    /// Turns the static-caster cache on or off, and sets how far a cached cascade may drift
    /// before it is gathered again, as a fraction of its radius. Off gathers every caster
    /// into every view every frame: the A/B control.
    pub fn set_static_cache(&mut self, enabled: bool, margin_fraction: f32) {
        self.static_cache.set_enabled(enabled);
        self.static_cache.set_margin_fraction(margin_fraction);
    }

    /// Whether any shadow work happens at all.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.settings.enabled
    }

    /// Decides what this frame's shadow maps contain. Pure CPU.
    ///
    /// Builds the cascades, culls the casters into one render list per view, and picks which
    /// local lights can afford a shadow map. Nothing here touches a graphics API, which is what lets
    /// a test assert on the result and what would let this move onto a worker thread.
    ///
    /// `material_count` and `mesh_count` are the lengths of the renderer's handle arrays: the
    /// key space the lists are grouped over, exactly as the camera's list is. Passing anything
    /// larger than the real arrays makes the grouping give up and keep gather order, which is
    /// what every shadow view did before they were passed: a draw per caster.
    ///
    /// `layout` is where each view lives in the atlas, which the backend's atlas resource
    /// decides when it is created or resized.
    pub fn prepare(
        &mut self,
        world: &RenderWorld,
        camera: &Camera,
        lighting: Lighting,
        material_count: usize,
        mesh_count: usize,
        layout: &ShadowAtlasLayout,
    ) {
        let started = std::time::Instant::now();
        self.stats = ShadowStats::for_settings(&self.settings);
        self.stats.enabled = self.settings.enabled;
        self.cascades.clear();
        self.cascade_rects.clear();
        self.local_views.clear();
        self.local_selected.clear();
        self.local_rects.clear();

        if !self.settings.enabled {
            self.prepare_ms = started.elapsed().as_secs_f64() * 1e3;
            return;
        }

        build_cascades(
            &self.settings,
            camera,
            lighting.direction,
            &mut self.cascades,
        );

        // One list per cascade, allocated once and kept. `resize_with` only grows, so a
        // quality change that adds a cascade allocates once and never again.
        if self.cascade_lists.len() < self.cascades.len() {
            self.cascade_lists
                .resize_with(self.cascades.len(), RenderList::new);
        }

        // The frame's movers: everything that casts and is not part of the static scene,
        // gathered once here and tested per view below. With the cache off, that is every
        // caster, and every view is gathered from scratch exactly as before the cache.
        self.static_cache
            .begin_frame(world.static_generation(), &self.settings);
        let caching = self.static_cache.enabled();
        self.movers.clear();
        {
            let (flags, shadow, statics) = (
                world.visible_flags(),
                world.shadow_casting(),
                world.static_flags(),
            );
            for slot in 0..world.slot_count() {
                if flags[slot] && shadow[slot].casts && !(caching && statics[slot]) {
                    self.movers.push(slot as u32);
                }
            }
        }

        let layout = *layout;
        let mut lod_counts = [0_u32; 4];
        for index in 0..self.cascades.len() {
            let cascade = self.cascades[index];
            let mut static_casters = 0;
            if caching {
                let (collector, settings) = (&mut self.collector, &self.settings);
                let view = self.static_cache.cascade(index, &cascade, |frustum, list| {
                    collector.collect_cascade_with(
                        world,
                        frustum,
                        &cascade,
                        index,
                        settings,
                        CasterSlots::Static,
                        list,
                    );
                    list.group_by_material_and_mesh(material_count, mesh_count);
                });
                static_casters = view.list.len() as u32;
            }
            let mut list = std::mem::take(&mut self.cascade_lists[index]);
            let movers: &[u32] = if self.far_movers || index + 1 < self.cascades.len() {
                &self.movers
            } else {
                &[]
            };
            self.collector.collect_cascade_with(
                world,
                &cascade.frustum(),
                &cascade,
                index,
                &self.settings,
                CasterSlots::Only(movers),
                &mut list,
            );
            // The same grouping the camera pass uses, over the same key space, which is what
            // gives the shadow pass instanced batching.
            list.group_by_material_and_mesh(material_count, mesh_count);
            let stats = self.collector.stats();
            self.stats.cascades[index].casters = static_casters + stats.visible;
            for (level, count) in stats.lod_counts.iter().enumerate() {
                lod_counts[level] += count;
            }
            lod_counts[3] += stats.too_small;
            self.static_cache.count_moving(list.len());
            self.cascade_lists[index] = list;
            self.cascade_rects
                .push(layout.cascade_rect(index).unwrap_or(AtlasRect {
                    x: 0,
                    y: 0,
                    size: 1,
                }));
        }
        self.stats.lod_counts = lod_counts;

        // Local lights, if the budget allows any.
        let frustum = Frustum::from_view_projection(&camera.view_projection());
        select_shadow_lights(
            world.lights(),
            camera.position(),
            &frustum,
            &self.settings,
            &mut self.light_scratch,
            &mut self.local_selected,
            &mut self.local_views,
        );
        self.stats.local_views = self.local_views.len() as u32;
        self.stats.local_lights = self.local_selected.len() as u32;
        self.stats.local_lights_refused = world
            .lights()
            .iter()
            .filter(|light| light.casts_shadow && light.is_significant())
            .count()
            .saturating_sub(self.local_selected.len())
            as u32;

        if self.local_lists.len() < self.local_views.len() {
            self.local_lists
                .resize_with(self.local_views.len(), RenderList::new);
        }
        for index in 0..self.local_views.len() {
            let view = self.local_views[index];
            let resolution = layout.local_resolution();
            if caching {
                let (collector, settings) = (&mut self.collector, &self.settings);
                let cached = self.static_cache.local(
                    view.light,
                    view.face.unwrap_or(0),
                    view.view_projection.to_cols_array(),
                    |list| {
                        collector.collect_local_with(
                            world,
                            &view,
                            resolution,
                            settings,
                            CasterSlots::Static,
                            list,
                        );
                        list.group_by_material_and_mesh(material_count, mesh_count);
                    },
                );
                self.stats.local.casters += cached.list.len() as u32;
            }
            let mut list = std::mem::take(&mut self.local_lists[index]);
            self.collector.collect_local_with(
                world,
                &view,
                resolution,
                &self.settings,
                CasterSlots::Only(&self.movers),
                &mut list,
            );
            list.group_by_material_and_mesh(material_count, mesh_count);
            self.stats.local.casters += self.collector.stats().visible;
            self.static_cache.count_moving(list.len());
            self.local_lists[index] = list;
            self.local_rects
                .push(layout.local_rect(index).unwrap_or(AtlasRect {
                    x: 0,
                    y: 0,
                    size: 1,
                }));
        }

        self.stats.static_cache = self.static_cache.stats();
        self.stats.static_views = self.static_cache.cached_views();
        self.prepare_ms = started.elapsed().as_secs_f64() * 1e3;
    }

    /// Writes everything the lighting pass needs to sample what `prepare` selected.
    ///
    /// `atlas_size` is the size in texels of the atlas texture the backend created.
    pub fn write_frame_constants(
        &self,
        constants: &mut FrameConstants,
        lights: &[engine_render_data::LocalLight],
        atlas_size: (u32, u32),
    ) {
        constants.set_atlas_size(atlas_size);
        constants.set_debug_mode(self.debug_mode);
        if !self.settings.enabled {
            // Explicitly: a disabled shadow system must leave the shader on the unshadowed
            // path rather than sampling a stale atlas.
            constants.set_directional_shadows(&[], &[], atlas_size, &self.settings);
            constants.set_local_lights(lights, &[], &[], &[], atlas_size);
            constants.set_contact_shadows(&super::settings::ContactShadowSettings {
                enabled: false,
                ..self.settings.contact
            });
            return;
        }
        constants.set_directional_shadows(
            &self.cascades,
            &self.cascade_rects,
            atlas_size,
            &self.settings,
        );
        constants.set_local_lights(
            lights,
            &self.local_selected,
            &self.local_views,
            &self.local_rects,
            atlas_size,
        );
        constants.set_contact_shadows(&self.settings.contact);
    }

    /// Records the GPU time the shadow passes took, once the timer has resolved it.
    pub fn set_gpu_ms(&mut self, milliseconds: Option<f64>) {
        self.stats.gpu_ms = milliseconds;
    }
}

/// The direction a light travels, normalized, with a fallback for a degenerate one.
///
/// A light with no direction at all is authored by accident — a script that zeroed a vector
/// — and it would produce a `NaN` view matrix and a black frame. Straight down is the
/// answer that keeps the frame renderable and looks obviously wrong.
#[must_use]
pub fn safe_light_direction(direction: Vec3) -> Vec3 {
    if direction.length_squared() > 1e-6 {
        direction.normalize()
    } else {
        Vec3::new(0.0, -1.0, 0.0)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::shadow::settings::ShadowPreset;

    #[test]
    fn a_shadow_views_constants_carry_the_matrix_untransposed() {
        let matrix = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let constants = ShadowViewConstants::new(matrix, true);
        assert_eq!(&constants.view_projection[12..15], &[1.0, 2.0, 3.0]);
        assert_eq!(constants.params[0], 1.0);
        // Small on purpose: this is uploaded once per cascade and once per local view.
        assert_eq!(size_of::<ShadowViewConstants>(), 80);
    }

    #[test]
    fn a_degenerate_light_direction_does_not_produce_a_nan_matrix() {
        assert_eq!(safe_light_direction(Vec3::ZERO), Vec3::new(0.0, -1.0, 0.0));
        assert!(
            (safe_light_direction(Vec3::new(0.0, -2.0, 0.0)).y + 1.0).abs() < 1e-6,
            "an ordinary direction must be normalized, not replaced"
        );
    }

    #[test]
    fn the_prepass_only_runs_for_contact_shadows() {
        // It is the one thing that needs it, and it costs a second pass over every visible
        // object — so a configuration without contact shadows must not pay for one.
        assert!(ShadowPreset::High.settings().contact.enabled);
        assert!(!ShadowPreset::Medium.settings().contact.enabled);
    }
}
