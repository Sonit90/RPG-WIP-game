//! What the shadow system is asked to do: cascade counts, resolutions, biases and budgets.
//!
//! Every number a shadow pass reads is here, and nothing here needs a device. That split is
//! the same one [`crate::culling`] makes: the policy is plain CPU data that a test can build
//! and assert on, and the D3D12 objects that carry it out live elsewhere.
//!
//! # Presets, and why the settings type is not an enum
//!
//! The player picks one of five words — off, low, medium, high, ultra. The renderer needs
//! twenty numbers. [`ShadowPreset`] maps the first onto the second in one place, and
//! everything downstream reads [`ShadowSettings`] rather than the preset, so a developer can
//! change one number without inventing a sixth quality level and the quality levels cannot
//! drift away from what is actually implemented.

/// Most cascades the system can be configured for.
///
/// Four. The constant buffer declares fixed-size arrays and the atlas is laid out for this
/// many; raising it is a change to this constant, the HLSL `MAX_CASCADES`, and nothing else.
pub const MAX_CASCADES: usize = 4;

/// Most local-light shadow *views* that may be rendered in one frame.
///
/// A spot light is one view and a point light is six, so this is not a light count — it is
/// the number of atlas tiles the local renderer may fill. Sixteen is four point lights, or
/// sixteen spots, and it is the size of the array in the frame constant buffer.
pub const MAX_LOCAL_SHADOW_VIEWS: usize = 16;

/// Most local lights the forward lighting pass shades with.
///
/// Eight, shadow-casting or not. There is no light culling structure — the shader loops over
/// this array for every pixel — so the number is a shading cost per pixel and not a scene
/// budget. It is documented as a limitation in `docs/shadows.md`: a scene that wants a
/// hundred torches at once needs clustered shading first.
pub const MAX_LOCAL_LIGHTS: usize = 8;

/// How wide the PCF kernel is.
///
/// Named rather than a raw number because the shader unrolls a fixed loop per value: adding
/// a variant means adding a branch there, and a "kernel width" field would suggest arbitrary
/// widths are supported when three are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcfKernel {
    /// One comparison, hardware-filtered. The cheapest thing that is still a comparison
    /// sampler: the 2x2 bilinear the comparison sampler performs is already a soft edge.
    Single,
    /// Nine taps. The default, and what §13 of the shadow plan asks for.
    Tap3x3,
    /// Twenty-five taps, for a soft edge on hardware that can afford it.
    Tap5x5,
}

impl PcfKernel {
    /// Taps one shadow lookup costs.
    #[must_use]
    pub fn taps(self) -> u32 {
        match self {
            Self::Single => 1,
            Self::Tap3x3 => 9,
            Self::Tap5x5 => 25,
        }
    }

    /// The value the shader switches on.
    #[must_use]
    pub fn shader_code(self) -> u32 {
        match self {
            Self::Single => 0,
            Self::Tap3x3 => 1,
            Self::Tap5x5 => 2,
        }
    }
}

/// Depth bias, in the three forms the artefacts require.
///
/// Three, because there are three different artefacts and no single number fixes them:
///
/// ```text
/// constant   a flat offset          fixes acne on surfaces facing the light
/// slope      scaled by the slope    fixes acne on surfaces edge-on to it
/// normal     offset along the normal fixes what is left, without peter-panning
/// ```
///
/// The first two are rasterizer state and cost nothing. The third moves the *sample
/// position* rather than the depth, so it does not detach a contact shadow the way a large
/// constant bias does — which is exactly the trade §11 asks to avoid making badly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowBias {
    /// Constant depth offset, in shadow-map depth units, applied by the rasterizer.
    ///
    /// Expressed as an integer count of the smallest representable depth increment, which is
    /// what `D3D12_RASTERIZER_DESC::DepthBias` takes.
    pub constant: i32,
    /// Slope-scaled depth offset, applied by the rasterizer.
    pub slope: f32,
    /// How far the receiving position is pushed along its normal before it is looked up,
    /// measured in shadow-map texels.
    ///
    /// Texels rather than metres: a cascade covering 150 m has texels twenty times the size
    /// of one covering 15 m, and an offset that fixed the far cascade would visibly detach
    /// the near one.
    pub normal_texels: f32,
    /// A ceiling on the normal offset in metres, so the coarsest cascade cannot slide a
    /// shadow across a room.
    pub normal_max_meters: f32,
}

impl Default for ShadowBias {
    /// The tuned defaults, measured against the arena at 2048 per cascade.
    ///
    /// The constant term is small on purpose: most of the work is done by the slope term,
    /// which is what acne actually scales with, and by the normal offset, which does not
    /// detach contact points. A constant bias large enough to fix acne on its own is the
    /// classic cause of peter-panning.
    fn default() -> Self {
        Self {
            constant: 200,
            slope: 2.5,
            normal_texels: 1.4,
            normal_max_meters: 0.35,
        }
    }
}

/// How the shadow passes pick geometry for a caster.
///
/// The levels §10 names, and the thresholds that choose between them. The mesh a level
/// resolves to is whatever the entity already offers — see
/// [`ShadowCasting`](engine_render_data::ShadowCasting) — so nothing here asks for a shadow
/// mesh nobody authored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowLodSettings {
    /// Whether the shadow pass may substitute coarser geometry at all.
    pub enabled: bool,
    /// How many levels coarser than the camera's a cascade goes, per cascade index.
    ///
    /// Cascade 0 draws what the camera draws; each cascade beyond it steps down by this
    /// much. One is the natural value: a cascade covers roughly three times the distance of
    /// the one before it, so its texels are three times as wide and the detail that level
    /// removes could not have been resolved anyway.
    pub levels_per_cascade: u32,
    /// A caster whose shadow would cover fewer than this many texels is not drawn.
    ///
    /// This is [`ShadowLod::None`] — the fourth level, "no shadow". A pebble 140 m away
    /// occupies a fraction of a texel of the last cascade; drawing it costs a draw call to
    /// change nothing, and a scene made of pebbles costs thousands of them.
    pub cull_texels: f32,
}

impl Default for ShadowLodSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            levels_per_cascade: 1,
            cull_texels: 2.0,
        }
    }
}

/// Which geometry a caster contributes at a given level.
///
/// The four levels §10 names, as one type rather than a bare index, so the "no shadow" case
/// cannot be confused with a mesh index of three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShadowLod {
    /// The geometry the camera draws.
    Full,
    /// One level coarser, if the entity offers one.
    Reduced,
    /// The coarsest stand-in the entity offers.
    Simplified,
    /// Not drawn into this shadow view at all.
    None,
}

impl ShadowLod {
    /// The mesh level this resolves to, or `None` if the caster is dropped.
    #[must_use]
    pub fn mesh_level(self) -> Option<usize> {
        match self {
            Self::Full => Some(0),
            Self::Reduced => Some(1),
            Self::Simplified => Some(2),
            Self::None => None,
        }
    }

    /// The level `steps` coarser than this one, saturating at [`ShadowLod::Simplified`].
    ///
    /// Saturates rather than falling off the end into `None`: dropping a caster is a
    /// decision made by size, not by counting levels, and a model with two levels must not
    /// vanish because a third cascade asked for a third one.
    #[must_use]
    pub fn coarser(self, steps: u32) -> Self {
        let level = match self {
            Self::Full => 0,
            Self::Reduced => 1,
            Self::Simplified | Self::None => 2,
        };
        match (level + steps).min(2) {
            0 => Self::Full,
            1 => Self::Reduced,
            _ => Self::Simplified,
        }
    }
}

/// How many shadow maps local lights may have, and how big they are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalShadowSettings {
    /// Whether local lights may cast shadows at all.
    pub enabled: bool,
    /// Side of one local shadow tile, in texels.
    pub resolution: u32,
    /// Most views that may be filled in one frame, across every local light.
    ///
    /// A view, not a light: a point light spends six of these. Capped by
    /// [`MAX_LOCAL_SHADOW_VIEWS`], which is what the constant buffer holds.
    pub max_views: usize,
    /// Whether point lights may cast shadows.
    ///
    /// Separately switchable from spot lights because the cost is six times as high for one
    /// light, and because a scene lit by point lights that only need to *look* lit is the
    /// common case.
    pub point_lights: bool,
    /// Beyond this distance from the camera a local light gets no shadow map, whatever its
    /// priority.
    ///
    /// A light whose whole lit region is a few pixels across does not need a shadow map, and
    /// a dungeon corridor full of torches would otherwise spend the entire budget on lights
    /// behind the player.
    pub max_distance: f32,
}

impl Default for LocalShadowSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            // 512, not 1024. A local shadow map covers a torch's few metres rather than a
            // cascade's hundred, so its texels are already far denser than the sun's at the
            // same resolution — and the budget is counted in *views*, six of which one point
            // light spends. Halving the side is what buys four times as many of them.
            resolution: 512,
            max_views: 8,
            point_lights: true,
            max_distance: 40.0,
        }
    }
}

/// The screen-space contact shadow: a short ray march against the scene depth buffer.
///
/// Deliberately short-ranged. Its job is the centimetres a shadow map cannot resolve — where
/// a boot meets the floor, where a prop sits on a table — and not to be a second shadowing
/// system. See `docs/shadows.md` for why it needs the depth prepass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactShadowSettings {
    /// Whether the effect runs.
    pub enabled: bool,
    /// How far the ray travels, in metres.
    pub length: f32,
    /// Samples along the ray. More is smoother and linearly more expensive.
    pub steps: u32,
    /// How much nearer than the ray a depth sample must be to count as an occluder, in
    /// metres.
    ///
    /// Without it, every silhouette edge in the frame occludes everything behind it: the
    /// depth buffer records a surface, not a solid, and a ray passing *behind* a thin object
    /// would otherwise be reported as blocked by it.
    pub thickness: f32,
    /// Beyond this distance from the camera the effect is faded out and then skipped.
    ///
    /// The detail it adds is invisible at range, and the march is a per-pixel cost that is
    /// not.
    pub max_distance: f32,
}

impl Default for ContactShadowSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            length: 0.35,
            steps: 8,
            thickness: 0.06,
            max_distance: 25.0,
        }
    }
}

/// Everything the shadow system is configured with.
///
/// Constructed from a [`ShadowPreset`] and then adjusted: a developer overlay, a benchmark
/// or a test scene changes one field without inventing a quality level for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowSettings {
    /// Whether any shadow work happens. False is [`ShadowPreset::Off`]: no atlas is
    /// allocated, no pass is recorded, and the lighting shader takes the unshadowed path.
    pub enabled: bool,
    /// How many cascades the directional light is split into, 1 to [`MAX_CASCADES`].
    pub cascade_count: usize,
    /// The far distance of each cascade, in metres from the camera.
    ///
    /// Only the first `cascade_count` entries are read. They must increase; a configuration
    /// that does not is repaired by [`ShadowSettings::validated`] rather than rejected,
    /// because these numbers are meant to be edited.
    pub cascade_distances: [f32; MAX_CASCADES],
    /// Side of one cascade's shadow map, in texels.
    pub resolution: u32,
    /// The fraction of a cascade's depth range spent blending into the next one.
    ///
    /// 0 is a hard seam. 0.1 blends over the last tenth, which is enough to hide the change
    /// in texel size without sampling two cascades for most of the frame.
    pub blend_fraction: f32,
    /// The PCF kernel every shadow lookup uses.
    pub pcf: PcfKernel,
    /// The three biases.
    pub bias: ShadowBias,
    /// Whether cascades are snapped to their own texel grid.
    ///
    /// On, always, in a shipping configuration — it is what stops the shadows crawling when
    /// the camera moves. Switchable because "off" is how the test proves the snapping is
    /// doing something.
    pub stabilize: bool,
    /// How far behind a cascade's bounding sphere the light frustum starts, in metres.
    ///
    /// A wall standing between the sun and the cascade is outside the cascade's bounds and
    /// still casts into it. This is how much of that "behind" is kept: too little and tall
    /// geometry stops casting, too much and depth precision is spent on empty air.
    pub caster_extrusion: f32,
    /// How geometry is chosen for a caster.
    pub lod: ShadowLodSettings,
    /// Local-light shadow budget.
    pub local: LocalShadowSettings,
    /// The screen-space contact shadow.
    pub contact: ContactShadowSettings,
}

impl Default for ShadowSettings {
    fn default() -> Self {
        ShadowPreset::High.settings()
    }
}

impl ShadowSettings {
    /// The last cascade's far distance: the range beyond which nothing is shadowed.
    #[must_use]
    pub fn max_distance(&self) -> f32 {
        self.cascade_distances[self.cascade_count.clamp(1, MAX_CASCADES) - 1]
    }

    /// The settings with every field forced into a range the renderer can honour.
    ///
    /// Repaired rather than rejected, for the same reason `engine-settings` replaces an
    /// out-of-range value with its default: these numbers are edited by hand and by sliders,
    /// and a shadow configuration is never worth failing a frame over.
    #[must_use]
    pub fn validated(mut self) -> Self {
        self.cascade_count = self.cascade_count.clamp(1, MAX_CASCADES);
        // Powers of two from 256: the atlas is laid out in tiles and a texel size that is
        // not a power of two makes the snapping arithmetic lie by a fraction of a texel.
        self.resolution = self.resolution.clamp(256, 4096).next_power_of_two();
        self.blend_fraction = if self.blend_fraction.is_finite() {
            self.blend_fraction.clamp(0.0, 0.5)
        } else {
            0.0
        };
        self.caster_extrusion = if self.caster_extrusion.is_finite() {
            self.caster_extrusion.clamp(0.0, 500.0)
        } else {
            50.0
        };

        // Monotonic and positive. A hand-edited list with a typo in it produces cascades
        // that overlap or invert, which renders as one cascade covering everything.
        let mut previous = 0.0_f32;
        for index in 0..MAX_CASCADES {
            let distance = self.cascade_distances[index];
            let repaired = if distance.is_finite() && distance > previous {
                distance
            } else {
                previous + 1.0
            };
            self.cascade_distances[index] = repaired;
            previous = repaired;
        }

        self.local.resolution = self.local.resolution.clamp(128, 4096).next_power_of_two();
        self.local.max_views = self.local.max_views.min(MAX_LOCAL_SHADOW_VIEWS);
        self.local.max_distance = self.local.max_distance.max(0.0);
        self.contact.steps = self.contact.steps.clamp(2, 32);
        self.contact.length = self.contact.length.clamp(0.0, 5.0);
        self.contact.thickness = self.contact.thickness.max(0.0);
        self.lod.cull_texels = self.lod.cull_texels.max(0.0);
        self
    }

    /// The settings with the directional cascades switched to `count`.
    #[must_use]
    pub fn with_cascade_count(mut self, count: usize) -> Self {
        self.cascade_count = count;
        self.validated()
    }

    /// The settings with each cascade's shadow map `resolution` texels square.
    #[must_use]
    pub fn with_resolution(mut self, resolution: u32) -> Self {
        self.resolution = resolution;
        self.validated()
    }

    /// The settings with contact shadows switched on or off.
    #[must_use]
    pub fn with_contact_shadows(mut self, enabled: bool) -> Self {
        self.contact.enabled = enabled;
        self
    }

    /// Bytes the shadow atlas occupies at these settings.
    ///
    /// Reported so a quality level can be judged by what it costs rather than by its name.
    /// Depth is `D32_FLOAT`: four bytes a texel, no stencil.
    #[must_use]
    pub fn atlas_bytes(&self) -> u64 {
        if !self.enabled {
            return 0;
        }
        let (width, height) = self.atlas_size();
        u64::from(width) * u64::from(height) * 4
    }

    /// The atlas dimensions these settings need, in texels.
    ///
    /// The layout is fixed and simple, because a shadow atlas that is packed cleverly is an
    /// atlas whose contents move between frames:
    ///
    /// ```text
    /// +----------------+----------------+
    /// |   cascade 0    |   cascade 1    |   a square grid of cascade tiles,
    /// +----------------+----------------+   two per row
    /// |   cascade 2    |  local tiles   |
    /// +----------------+----------------+
    /// ```
    ///
    /// With four cascades the local tiles get a row of their own beneath the grid. The
    /// local region is always as wide as the cascade region, and as tall as it needs to be
    /// for `max_views` tiles.
    #[must_use]
    pub fn atlas_size(&self) -> (u32, u32) {
        if !self.enabled {
            return (0, 0);
        }
        let columns = if self.cascade_count > 1 { 2 } else { 1 };
        let cascade_rows = self.cascade_count.div_ceil(columns) as u32;
        let width = self.resolution * columns as u32;
        let mut height = self.resolution * cascade_rows;

        let local_views = self.local_view_budget();
        if local_views > 0 {
            let free_tiles = (columns as u32 * cascade_rows) - self.cascade_count as u32;
            let per_tile = (self.resolution / self.local.resolution).max(1);
            let in_free = (free_tiles * per_tile * per_tile) as usize;
            if in_free < local_views {
                let remaining = local_views - in_free;
                let per_row = (width / self.local.resolution).max(1) as usize;
                height += (remaining.div_ceil(per_row) as u32) * self.local.resolution;
            }
        }
        (width, height)
    }

    /// How many local shadow views these settings actually allow.
    ///
    /// Zero when local shadows are off, which is what makes the atlas layout above collapse
    /// to the cascade grid alone.
    #[must_use]
    pub fn local_view_budget(&self) -> usize {
        if !self.enabled || !self.local.enabled {
            0
        } else {
            self.local.max_views.min(MAX_LOCAL_SHADOW_VIEWS)
        }
    }
}

/// The five words a player chooses between.
///
/// Mapped to numbers in exactly one place — [`ShadowPreset::settings`] — so a quality level
/// cannot promise something the renderer does not implement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShadowPreset {
    /// No shadow work at all.
    Off,
    /// Two cascades at 1024, no local shadows, no contact shadows.
    Low,
    /// Three cascades at 1024, two local shadow views.
    Medium,
    /// Three cascades at 2048, four local shadow views, contact shadows.
    #[default]
    High,
    /// Four cascades at 2048, eight local shadow views, contact shadows, a wider filter.
    Ultra,
}

/// The cascade ranges every preset starts from, in metres.
///
/// 0–15, 15–50, 50–150 is the configuration §6 asks for, with a fourth range for Ultra. The
/// steps are roughly logarithmic, which is what puts a similar number of texels per metre in
/// each cascade rather than spending the first one on the ground under the character's feet.
const DEFAULT_CASCADE_DISTANCES: [f32; MAX_CASCADES] = [15.0, 50.0, 150.0, 320.0];

impl ShadowPreset {
    /// Every preset, worst to best, for a settings menu to list.
    pub const ALL: [Self; 5] = [Self::Off, Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// The word this preset is written as in a settings file and a log line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }

    /// The preset a word names, or `None` for a word that names nothing.
    ///
    /// Case-insensitive, because the file is hand-edited.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.as_str().eq_ignore_ascii_case(name.trim()))
    }

    /// The numbers this preset stands for.
    ///
    /// The one place quality levels become configuration. Every value here was chosen
    /// against the arena and the dungeon; see `docs/shadows.md` for what each one costs.
    #[must_use]
    pub fn settings(self) -> ShadowSettings {
        let base = ShadowSettings {
            enabled: true,
            cascade_count: 3,
            cascade_distances: DEFAULT_CASCADE_DISTANCES,
            resolution: 2048,
            blend_fraction: 0.1,
            pcf: PcfKernel::Tap3x3,
            bias: ShadowBias::default(),
            stabilize: true,
            caster_extrusion: 60.0,
            lod: ShadowLodSettings::default(),
            local: LocalShadowSettings::default(),
            contact: ContactShadowSettings::default(),
        };

        match self {
            Self::Off => ShadowSettings {
                enabled: false,
                local: LocalShadowSettings {
                    enabled: false,
                    ..base.local
                },
                contact: ContactShadowSettings {
                    enabled: false,
                    ..base.contact
                },
                ..base
            },
            Self::Low => ShadowSettings {
                cascade_count: 2,
                resolution: 1024,
                // One comparison sample. The hardware's own 2x2 filter still softens the
                // edge, so this is a softer image than an unfiltered lookup and costs a
                // ninth of the 3x3.
                pcf: PcfKernel::Single,
                // A coarser map needs a larger normal offset to stay off its own surface.
                bias: ShadowBias {
                    normal_texels: 1.8,
                    ..base.bias
                },
                local: LocalShadowSettings {
                    enabled: false,
                    ..base.local
                },
                lod: ShadowLodSettings {
                    // A cheap configuration drops small casters sooner.
                    cull_texels: 3.0,
                    ..base.lod
                },
                ..base
            }
            .validated(),
            Self::Medium => ShadowSettings {
                resolution: 1024,
                local: LocalShadowSettings {
                    max_views: 4,
                    // Point lights are off here, not rationed: one of them spends six views,
                    // so a Medium budget would be one torch and nothing else. Spot lights —
                    // one view each — still shadow.
                    point_lights: false,
                    ..base.local
                },
                bias: ShadowBias {
                    normal_texels: 1.6,
                    ..base.bias
                },
                ..base
            }
            .validated(),
            Self::High => ShadowSettings {
                // Eight views: one point light and two spots, or eight spots. They all fit
                // in the quarter of the atlas the three cascades leave empty, so this costs
                // no extra memory at all — the tiles were already allocated.
                local: LocalShadowSettings {
                    max_views: 8,
                    ..base.local
                },
                contact: ContactShadowSettings {
                    enabled: true,
                    ..base.contact
                },
                ..base
            }
            .validated(),
            Self::Ultra => ShadowSettings {
                cascade_count: 4,
                pcf: PcfKernel::Tap5x5,
                local: LocalShadowSettings {
                    max_views: MAX_LOCAL_SHADOW_VIEWS,
                    ..base.local
                },
                contact: ContactShadowSettings {
                    enabled: true,
                    steps: 12,
                    ..base.contact
                },
                ..base
            }
            .validated(),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_produces_settings_the_renderer_can_honour() {
        for preset in ShadowPreset::ALL {
            let settings = preset.settings();
            assert_eq!(
                settings,
                settings.validated(),
                "{} needs repairing, so it is not a configuration that was actually checked",
                preset.as_str()
            );
            assert!(settings.cascade_count <= MAX_CASCADES);
            assert!(settings.local.max_views <= MAX_LOCAL_SHADOW_VIEWS);
        }
    }

    #[test]
    fn off_allocates_nothing() {
        let settings = ShadowPreset::Off.settings();
        assert!(!settings.enabled);
        assert_eq!(settings.atlas_bytes(), 0);
        assert_eq!(settings.atlas_size(), (0, 0));
        assert_eq!(settings.local_view_budget(), 0);
    }

    #[test]
    fn the_presets_are_ordered_by_what_they_cost() {
        // Not a style rule: a quality level that costs less than the one below it is a
        // mislabelled setting, and this is the only place that would catch it.
        let costs: Vec<u64> = ShadowPreset::ALL
            .into_iter()
            .map(|preset| preset.settings().atlas_bytes())
            .collect();
        for pair in costs.windows(2) {
            assert!(
                pair[1] >= pair[0],
                "a higher quality level allocates less shadow memory: {costs:?}"
            );
        }
    }

    #[test]
    fn the_atlas_holds_every_cascade_and_every_local_view() {
        for preset in ShadowPreset::ALL {
            let settings = preset.settings();
            if !settings.enabled {
                continue;
            }
            let (width, height) = settings.atlas_size();
            let cascade_area = (settings.resolution as u64).pow(2) * settings.cascade_count as u64;
            let local_area =
                (settings.local.resolution as u64).pow(2) * settings.local_view_budget() as u64;
            assert!(
                u64::from(width) * u64::from(height) >= cascade_area + local_area,
                "{} allocates {width}x{height}, which cannot hold its own tiles",
                preset.as_str()
            );
        }
    }

    #[test]
    fn a_preset_that_allows_point_lights_can_afford_one() {
        // A point light costs six views. A preset that allows them and budgets fewer than
        // six is a setting that silently never applies — which is exactly what happened
        // when High budgeted four, and what this test exists to stop happening again.
        for preset in ShadowPreset::ALL {
            let settings = preset.settings();
            if !settings.enabled || !settings.local.enabled || !settings.local.point_lights {
                continue;
            }
            assert!(
                settings.local_view_budget() >= 6,
                "{} allows point lights but budgets {} views, and one point light needs six",
                preset.as_str(),
                settings.local_view_budget()
            );
        }
    }

    #[test]
    fn a_hand_edited_cascade_list_is_repaired_rather_than_obeyed() {
        let broken = ShadowSettings {
            cascade_count: 9,
            cascade_distances: [50.0, 10.0, f32::NAN, -3.0],
            resolution: 1500,
            blend_fraction: f32::INFINITY,
            ..ShadowPreset::High.settings()
        }
        .validated();

        assert_eq!(broken.cascade_count, MAX_CASCADES);
        assert!(broken.resolution.is_power_of_two());
        assert_eq!(broken.blend_fraction, 0.0);
        for pair in broken.cascade_distances.windows(2) {
            assert!(
                pair[1] > pair[0],
                "{:?} is not increasing",
                broken.cascade_distances
            );
        }
    }

    #[test]
    fn preset_names_round_trip() {
        for preset in ShadowPreset::ALL {
            assert_eq!(ShadowPreset::from_name(preset.as_str()), Some(preset));
        }
        assert_eq!(ShadowPreset::from_name("  HIGH "), Some(ShadowPreset::High));
        assert_eq!(ShadowPreset::from_name("cinematic"), None);
    }

    #[test]
    fn a_coarser_level_saturates_instead_of_dropping_the_caster() {
        // Dropping a caster is a decision made by how small its shadow is, never by having
        // run out of levels to step down.
        assert_eq!(ShadowLod::Full.coarser(1), ShadowLod::Reduced);
        assert_eq!(ShadowLod::Full.coarser(5), ShadowLod::Simplified);
        assert_eq!(ShadowLod::Simplified.coarser(3), ShadowLod::Simplified);
        assert_eq!(ShadowLod::None.mesh_level(), None);
    }

    #[test]
    fn the_maximum_shadow_distance_is_the_last_cascades_far_plane() {
        let settings = ShadowPreset::Medium.settings();
        assert_eq!(
            settings.max_distance(),
            settings.cascade_distances[settings.cascade_count - 1]
        );
    }
}
