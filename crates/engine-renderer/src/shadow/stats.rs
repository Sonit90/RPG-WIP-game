//! What the shadow passes cost and what they contained.
//!
//! §19 asks for shadow work to be measured with the profiling the renderer already has
//! rather than with something new beside it. That is what this is: the CPU side is timed
//! with `Instant` exactly as the cull and the grouping passes are, the GPU side comes from
//! the existing [`GpuTimer`](crate::gpu_timer::GpuTimer) with a second timestamp pair rather
//! than a second query heap, and the draw and triangle counts come from the
//! [`CommandContext`](crate::command::CommandContext) that already counts every draw
//! recorded through it.

use super::settings::{MAX_CASCADES, ShadowSettings};

/// What one shadow view drew.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ShadowPassStats {
    /// Casters that survived this view's cull.
    pub casters: u32,
    /// Draw calls recorded for it.
    pub draw_calls: u32,
    /// Triangles submitted to it.
    pub triangles: u64,
}

/// What the whole shadow stage did this frame.
///
/// Copied out of the renderer rather than borrowed, so a caller can hold last frame's
/// numbers next to this frame's — which is how a regression is noticed at all.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ShadowStats {
    /// Whether any shadow work happened.
    pub enabled: bool,
    /// Cascades rendered.
    pub cascade_count: u32,
    /// Side of one cascade in texels.
    pub resolution: u32,
    /// The whole atlas, in texels.
    pub atlas_size: (u32, u32),
    /// Bytes the atlas occupies.
    pub atlas_bytes: u64,
    /// The far distance of each cascade, for the overlay to print.
    pub splits: [f32; MAX_CASCADES],
    /// Per-cascade totals.
    pub cascades: [ShadowPassStats; MAX_CASCADES],
    /// Local shadow views filled.
    pub local_views: u32,
    /// Local lights that were given a map.
    pub local_lights: u32,
    /// Local lights that asked for one and did not get it.
    pub local_lights_refused: u32,
    /// Totals across the local views.
    pub local: ShadowPassStats,
    /// Skinned submeshes drawn into shadow views, across every view.
    pub skinned_draws: u32,
    /// Casters dropped by each shadow LOD level, indexed by level.
    ///
    /// Index 3 is "no shadow": what the size test rejected, which is the number that says
    /// whether the LOD policy is earning its place.
    pub lod_counts: [u32; 4],
    /// Milliseconds the CPU spent culling and recording the shadow passes.
    pub cpu_ms: f64,
    /// Milliseconds of `cpu_ms` that were the caster collection and grouping, before any
    /// command was recorded.
    ///
    /// Split out because the two halves move for different reasons and are fixed by
    /// different work: preparation is a pure pass over the world, once per view, and
    /// recording is driver calls. A single `cpu_ms` cannot say which one grew.
    pub prepare_ms: f64,
    /// Milliseconds of `cpu_ms` spent recording the atlas views: every cascade and every
    /// local view, their batches and their skinned casters.
    pub atlas_ms: f64,
    /// Milliseconds of `cpu_ms` spent recording the camera's depth prepass.
    ///
    /// Apart from the atlas because it draws a different list — the camera's, not a light's
    /// — and is cut by different work.
    pub prepass_ms: f64,
    /// Milliseconds the GPU spent on them, or `None` before the first slot has completed a
    /// full lap. Lags by the same number of frames the frame timer does.
    pub gpu_ms: Option<f64>,
    /// Whether the depth prepass ran, which is what contact shadows need.
    pub prepass: bool,
    /// What the static-caster cache did this frame.
    pub static_cache: super::static_cache::StaticCacheStats,
    /// Views holding cached static casters at the end of this frame.
    pub static_views: u32,
}

impl ShadowStats {
    /// Zeroed statistics describing the configuration in `settings`.
    ///
    /// The configuration half is filled even when nothing was drawn, so an overlay reporting
    /// "3 cascades at 2048, 0 casters" says something useful instead of showing zeros that
    /// could mean either.
    #[must_use]
    pub fn for_settings(settings: &ShadowSettings) -> Self {
        let mut splits = [0.0; MAX_CASCADES];
        splits[..settings.cascade_count]
            .copy_from_slice(&settings.cascade_distances[..settings.cascade_count]);
        Self {
            enabled: settings.enabled,
            cascade_count: settings.cascade_count as u32,
            resolution: settings.resolution,
            atlas_size: settings.atlas_size(),
            atlas_bytes: settings.atlas_bytes(),
            splits,
            prepass: settings.contact.enabled,
            ..Self::default()
        }
    }

    /// Draw calls across every shadow view.
    #[must_use]
    pub fn draw_calls(&self) -> u32 {
        self.cascades
            .iter()
            .map(|pass| pass.draw_calls)
            .sum::<u32>()
            + self.local.draw_calls
    }

    /// Triangles across every shadow view.
    #[must_use]
    pub fn triangles(&self) -> u64 {
        self.cascades.iter().map(|pass| pass.triangles).sum::<u64>() + self.local.triangles
    }

    /// Casters across every shadow view.
    ///
    /// A caster in two cascades counts twice, deliberately: this is the *work*, and the same
    /// object drawn into three cascades costs three draws.
    #[must_use]
    pub fn casters(&self) -> u32 {
        self.cascades.iter().map(|pass| pass.casters).sum::<u32>() + self.local.casters
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
    fn the_configuration_is_reported_even_with_nothing_drawn() {
        let settings = ShadowPreset::High.settings();
        let stats = ShadowStats::for_settings(&settings);
        assert!(stats.enabled);
        assert_eq!(stats.cascade_count, 3);
        assert_eq!(stats.resolution, 1536);
        assert_eq!(stats.splits[2], 150.0);
        assert_eq!(stats.draw_calls(), 0);
    }

    #[test]
    fn totals_add_the_cascades_and_the_local_views() {
        let mut stats = ShadowStats::for_settings(&ShadowPreset::High.settings());
        stats.cascades[0] = ShadowPassStats {
            casters: 10,
            draw_calls: 4,
            triangles: 100,
        };
        stats.cascades[1] = ShadowPassStats {
            casters: 6,
            draw_calls: 3,
            triangles: 50,
        };
        stats.local = ShadowPassStats {
            casters: 2,
            draw_calls: 1,
            triangles: 20,
        };
        assert_eq!(stats.draw_calls(), 8);
        assert_eq!(stats.triangles(), 170);
        assert_eq!(stats.casters(), 18);
    }
}
