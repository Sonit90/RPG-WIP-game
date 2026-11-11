//! The numbers, in one place: what an asset of each category is allowed to cost.
//!
//! `docs/style/ASSET_LIMITATIONS.md` is the specification. This file is that document
//! expressed as data, and nothing else in the engine may hold a copy of one of these
//! numbers: a limit that appears twice is a limit that will disagree with itself the first
//! time somebody edits one of the two.
//!
//! # Three kinds of number
//!
//! ```text
//!  hard limit         exceeding it is an Error   the asset does not enter the library
//!  preferred target   exceeding it is a Warning  the asset is expensive, not broken
//!  recommended shape  a tolerance band           LOD reduction, physical size
//! ```
//!
//! A 4096 texture is inside the hard limit and outside the preferred target, so it warns
//! and loads — which is the whole reason the two are separate fields rather than one.
//!
//! # Data driven
//!
//! Every rule in [`super::validate_asset`] reads a field of [`AssetLimits`] or
//! [`CategoryLimits`]. Adding a category is adding a row to [`CATEGORIES`]; changing a
//! budget is changing a number here. Neither touches validation logic.

/// What kind of thing an asset is, which is what decides its budget.
///
/// # Why this is not inferred
///
/// A hero asset is a *decision* — "this one piece carries a room" — not a measurement. If
/// the category were inferred from polygon count then every asset that went over budget
/// would promote itself to the next budget up, and the limits would enforce nothing. The
/// category comes from the asset's metadata or the person running the validator; the
/// validator only ever reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum AssetCategory {
    /// A mug, a candle, a nail. Under a metre, never looked at closely.
    TinyProp,
    /// A torch, a bucket, a stool.
    SmallProp,
    /// A barrel, a chest, a market stall.
    MediumProp,
    /// A cart, a statue, a siege engine.
    LargeProp,
    /// A prop that carries a scene on its own, and is allowed to cost like it.
    HeroProp,
    /// A wall, a floor, a stair — a modular building piece.
    Architecture,
    /// A gatehouse, a shrine, a landmark building.
    HeroArchitecture,
    /// Grass, ferns, bushes: alpha-tested cards and clusters, never unique leaves.
    Foliage,
    /// A rock. Silhouette is the whole asset; surface detail belongs in the normal map.
    Rock,
    /// A tree. Trunk silhouette first, branches second, leaves on cards.
    Tree,
    /// No category was supplied.
    ///
    /// The runtime load path's category: only the absolute limits of section 25 apply, and
    /// none of the per-category art-direction targets do. A character imported by
    /// [`crate::disk::load_glb`] is this, because that layer knows a file name and not what
    /// the thing in it is for.
    #[default]
    Unclassified,
}

impl AssetCategory {
    /// Every category, in budget order.
    pub const ALL: [Self; 11] = [
        Self::TinyProp,
        Self::SmallProp,
        Self::MediumProp,
        Self::LargeProp,
        Self::HeroProp,
        Self::Architecture,
        Self::HeroArchitecture,
        Self::Foliage,
        Self::Rock,
        Self::Tree,
        Self::Unclassified,
    ];

    /// The name the metadata file, the command line and every report use.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.limits().name
    }

    /// The category `name` spells, if any. Case-insensitive, `-` and `_` interchangeable.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let wanted = name.trim().to_lowercase().replace('-', "_");
        Self::ALL
            .into_iter()
            .find(|category| category.name() == wanted)
    }

    /// What an asset of this category is allowed to cost.
    #[must_use]
    pub const fn limits(self) -> &'static CategoryLimits {
        &CATEGORIES[self as usize]
    }

    /// Whether this category is one of the two the specification calls "hero".
    ///
    /// Hero assets are allowed eight material slots and eight submeshes where a normal asset
    /// gets four — see sections 5 and 21.
    #[must_use]
    pub const fn is_hero(self) -> bool {
        matches!(self, Self::HeroProp | Self::HeroArchitecture)
    }
}

impl std::fmt::Display for AssetCategory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// One row of the budget table: everything that varies from category to category.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CategoryLimits {
    /// The category's name, in the spelling metadata and reports use.
    pub name: &'static str,
    /// LOD0 vertices, from section 2. Exceeding it is an error.
    pub max_vertices: u64,
    /// LOD0 triangles, from section 1.1. Exceeding it is an error.
    pub max_triangles: u64,
    /// Material slots, from section 5. Exceeding it is an error.
    pub max_material_slots: u64,
    /// Material slots beyond which the asset is expensive but legal. A warning.
    pub preferred_material_slots: u64,
    /// Submeshes — draw calls per instance — from section 21. An error.
    pub max_submeshes: u64,
    /// Submeshes beyond which the asset is worth a second look. A warning.
    pub preferred_submeshes: u64,
    /// The texture resolution this category is authored at, from section 6. A warning.
    ///
    /// Not a limit: the hard limit is [`AssetLimits::max_texture_size`], and a 4096 map on a
    /// small prop is over budget rather than broken.
    pub preferred_texture_size: u32,
    /// Levels of detail this category is expected to ship, from section 4. A warning.
    pub expected_lods: u64,
    /// Whether an asset of this category is expected to block the player.
    pub requires_collision: bool,
    /// The plausible range of the asset's longest dimension, in metres — section 14.
    ///
    /// A torch is one to two metres. An asset a hundred times that was authored in
    /// centimetres, and the validator says so rather than the artist finding out when it
    /// fills the level.
    pub size_range_meters: (f32, f32),
}

/// The budget table. One row per [`AssetCategory`], in declaration order.
///
/// The numbers are `docs/style/ASSET_LIMITATIONS.md` sections 1.1, 2, 4, 5, 6, 14, 17, 18,
/// 19 and 21. Nothing else in the workspace may restate one of them.
pub const CATEGORIES: [CategoryLimits; 11] = [
    CategoryLimits {
        name: "tiny_prop",
        max_vertices: 500,
        max_triangles: 300,
        max_material_slots: 4,
        preferred_material_slots: 1,
        max_submeshes: 4,
        preferred_submeshes: 1,
        preferred_texture_size: 512,
        // "Very small assets may use fewer LODs" — a 300-triangle prop is already its own
        // LOD2, and generating three more levels of it costs more than it saves.
        expected_lods: 2,
        requires_collision: false,
        size_range_meters: (0.02, 1.0),
    },
    CategoryLimits {
        name: "small_prop",
        max_vertices: 2_000,
        max_triangles: 1_500,
        max_material_slots: 4,
        preferred_material_slots: 2,
        max_submeshes: 4,
        preferred_submeshes: 2,
        preferred_texture_size: 1024,
        expected_lods: 3,
        requires_collision: false,
        size_range_meters: (0.05, 2.5),
    },
    CategoryLimits {
        name: "medium_prop",
        max_vertices: 6_000,
        max_triangles: 5_000,
        max_material_slots: 4,
        preferred_material_slots: 2,
        max_submeshes: 4,
        preferred_submeshes: 2,
        preferred_texture_size: 2048,
        expected_lods: 4,
        requires_collision: true,
        size_range_meters: (0.2, 4.0),
    },
    CategoryLimits {
        name: "large_prop",
        max_vertices: 15_000,
        max_triangles: 12_000,
        // Section 5's "large assets" allowance.
        max_material_slots: 6,
        preferred_material_slots: 2,
        max_submeshes: 4,
        preferred_submeshes: 2,
        preferred_texture_size: 2048,
        expected_lods: 4,
        requires_collision: true,
        size_range_meters: (0.5, 12.0),
    },
    CategoryLimits {
        name: "hero_prop",
        max_vertices: 30_000,
        max_triangles: 25_000,
        max_material_slots: 8,
        preferred_material_slots: 4,
        max_submeshes: 8,
        preferred_submeshes: 4,
        preferred_texture_size: 2048,
        expected_lods: 4,
        requires_collision: true,
        size_range_meters: (0.5, 20.0),
    },
    CategoryLimits {
        name: "architecture",
        max_vertices: 25_000,
        max_triangles: 20_000,
        max_material_slots: 6,
        preferred_material_slots: 3,
        max_submeshes: 4,
        preferred_submeshes: 2,
        preferred_texture_size: 2048,
        expected_lods: 4,
        requires_collision: true,
        size_range_meters: (0.5, 30.0),
    },
    CategoryLimits {
        name: "hero_architecture",
        max_vertices: 50_000,
        max_triangles: 40_000,
        max_material_slots: 8,
        preferred_material_slots: 4,
        max_submeshes: 8,
        preferred_submeshes: 4,
        preferred_texture_size: 2048,
        expected_lods: 4,
        requires_collision: true,
        size_range_meters: (2.0, 80.0),
    },
    CategoryLimits {
        name: "foliage",
        // Section 17: a plant is 500-2,000 triangles at LOD0, on cards.
        max_vertices: 3_000,
        max_triangles: 2_000,
        max_material_slots: 2,
        preferred_material_slots: 1,
        max_submeshes: 4,
        preferred_submeshes: 2,
        preferred_texture_size: 1024,
        expected_lods: 4,
        requires_collision: false,
        size_range_meters: (0.05, 4.0),
    },
    CategoryLimits {
        name: "rock",
        // Section 18: a hero rock is 8,000-15,000 triangles, and nothing above that.
        max_vertices: 18_000,
        max_triangles: 15_000,
        max_material_slots: 4,
        preferred_material_slots: 1,
        max_submeshes: 4,
        preferred_submeshes: 1,
        preferred_texture_size: 2048,
        expected_lods: 4,
        requires_collision: true,
        size_range_meters: (0.05, 8.0),
    },
    CategoryLimits {
        name: "tree",
        // Section 19: a large tree is 8,000-15,000 triangles at LOD0.
        max_vertices: 18_000,
        max_triangles: 15_000,
        max_material_slots: 4,
        preferred_material_slots: 2,
        max_submeshes: 4,
        preferred_submeshes: 2,
        preferred_texture_size: 2048,
        expected_lods: 4,
        requires_collision: true,
        size_range_meters: (0.5, 20.0),
    },
    CategoryLimits {
        name: "unclassified",
        // The absolute limits of section 25 and nothing more: an unclassified asset has no
        // art-direction budget to be measured against, only the runtime's own ceiling.
        max_vertices: u64::MAX,
        max_triangles: u64::MAX,
        max_material_slots: 8,
        preferred_material_slots: 8,
        max_submeshes: 8,
        preferred_submeshes: 8,
        preferred_texture_size: 4096,
        expected_lods: 1,
        requires_collision: false,
        size_range_meters: (0.0, f32::INFINITY),
    },
];

/// The limits that do not depend on what kind of asset it is.
///
/// Section 25 of the specification, plus the tolerances the shape rules need. A value here
/// is the *ceiling*: a category may be stricter, never looser.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssetLimits {
    /// Vertices in any normal asset, at any level of detail. Section 25.
    pub max_vertices: u64,
    /// Vertices in an asset explicitly marked as an exception.
    pub exception_max_vertices: u64,
    /// Triangles in any normal asset, at any level of detail. Section 25.
    pub max_triangles: u64,
    /// Triangles in an asset explicitly marked as an exception.
    pub exception_max_triangles: u64,
    /// Material slots on any asset, whatever its category. Section 25.
    pub max_material_slots: u64,
    /// Submeshes on an asset whose category does not say otherwise. Section 25.
    pub max_submeshes: u64,
    /// The largest texture the runtime accepts, in texels on the longest side. Section 25.
    pub max_texture_size: u32,
    /// The resolution most environment assets are authored at. Section 6.
    pub default_texture_size: u32,
    /// Collision primitives on one asset. Section 13.
    pub max_collision_primitives: u64,
    /// Collision primitives beyond which the shape is worth simplifying. Section 13.
    pub preferred_collision_primitives: u64,
    /// The fraction of LOD0's triangles each level should keep. Section 4.
    ///
    /// `[1.0, 0.5, 0.2, 0.075]`: LOD0 whole, then a half, a fifth, and the middle of the
    /// specification's 5-10% band.
    pub lod_triangle_targets: [f32; 4],
    /// How far a level may sit from its target before it is reported, as a fraction.
    ///
    /// 0.6 turns the 50% target into an accepted band of 20%-80%, and the 20% target into
    /// 8%-32%. The specification asks for reduction "based on visual error, not blindly on
    /// percentages", so the band is wide on purpose: it catches a level that is not a
    /// reduction at all, not a decimator that landed on 46%.
    pub lod_tolerance: f32,
    /// Triangles per metre of bounding diameter, beyond which density is reported.
    ///
    /// A pebble with twenty thousand triangles is inside every count budget and is still an
    /// unretopologized generator output. This is the rule that says so.
    pub max_triangles_per_meter: f64,
    /// The largest absolute scale factor an import transform may carry.
    pub max_scale: f32,
    /// The smallest absolute scale factor an import transform may carry.
    pub min_scale: f32,
    /// Whether the art-direction rules apply, as opposed to the structural ones alone.
    ///
    /// `false` is the runtime load path — see [`AssetLimits::runtime`]. `true` is every
    /// import-time and tooling caller.
    pub content_rules: bool,
}

impl Default for AssetLimits {
    /// Section 25, verbatim.
    fn default() -> Self {
        Self {
            max_vertices: 50_000,
            exception_max_vertices: 100_000,
            max_triangles: 40_000,
            exception_max_triangles: 80_000,
            max_material_slots: 8,
            max_submeshes: 8,
            max_texture_size: 4096,
            default_texture_size: 2048,
            max_collision_primitives: 20,
            preferred_collision_primitives: 10,
            lod_triangle_targets: [1.0, 0.5, 0.2, 0.075],
            lod_tolerance: 0.6,
            max_triangles_per_meter: 20_000.0,
            max_scale: 10_000.0,
            min_scale: 1.0e-4,
            content_rules: true,
        }
    }
}

impl AssetLimits {
    /// The limits the runtime load path applies: structure only, no art direction.
    ///
    /// # Why the two differ
    ///
    /// Everything the structural half checks is something the GPU cannot survive — an index
    /// past the end of a vertex buffer is undefined behaviour, not a budget. Everything the
    /// art-direction half checks is a cost decision, and a cost decision needs a category,
    /// which the loader does not have. So the loader refuses the first kind and the asset
    /// pipeline reports the second; see this module's crate documentation for where each
    /// one runs.
    ///
    /// The caps passed in are the caller's existing [`crate::ValidationLimits`], so this
    /// path enforces exactly what it enforced before the validator existed.
    #[must_use]
    pub fn runtime(
        max_triangles_per_lod: u64,
        max_texture_size: u32,
        max_materials: u64,
        max_submeshes_per_lod: u64,
    ) -> Self {
        Self {
            max_vertices: u64::MAX,
            exception_max_vertices: u64::MAX,
            max_triangles: max_triangles_per_lod,
            exception_max_triangles: max_triangles_per_lod,
            max_material_slots: max_materials,
            max_submeshes: max_submeshes_per_lod,
            max_texture_size,
            default_texture_size: max_texture_size,
            content_rules: false,
            ..Self::default()
        }
    }

    /// The vertex ceiling for an asset, given whether it is a marked exception.
    #[must_use]
    pub fn vertex_ceiling(&self, category: AssetCategory, exception: bool) -> u64 {
        if exception {
            self.exception_max_vertices
        } else {
            category.limits().max_vertices.min(self.max_vertices)
        }
    }

    /// The triangle ceiling for an asset, given whether it is a marked exception.
    #[must_use]
    pub fn triangle_ceiling(&self, category: AssetCategory, exception: bool) -> u64 {
        if exception {
            self.exception_max_triangles
        } else {
            category.limits().max_triangles.min(self.max_triangles)
        }
    }

    /// The material slot ceiling for an asset.
    #[must_use]
    pub fn material_ceiling(&self, category: AssetCategory) -> u64 {
        category
            .limits()
            .max_material_slots
            .min(self.max_material_slots)
    }

    /// The submesh ceiling for one level of detail.
    #[must_use]
    pub fn submesh_ceiling(&self, category: AssetCategory) -> u64 {
        category.limits().max_submeshes.min(self.max_submeshes)
    }

    /// The accepted band of LOD0 fractions for level `level`, or `None` past the table.
    #[must_use]
    pub fn lod_band(&self, level: usize) -> Option<(f32, f32)> {
        let target = *self.lod_triangle_targets.get(level)?;
        let tolerance = self.lod_tolerance.clamp(0.0, 1.0);
        Some((target * (1.0 - tolerance), target * (1.0 + tolerance)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_is_named_once_and_parses_back_to_itself() {
        let mut names: Vec<&str> = AssetCategory::ALL.iter().map(|c| c.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "a category name is used twice");
        for category in AssetCategory::ALL {
            assert_eq!(AssetCategory::parse(category.name()), Some(category));
        }
        assert_eq!(
            AssetCategory::parse("Hero-Prop"),
            Some(AssetCategory::HeroProp),
            "the command line is allowed to spell it either way"
        );
        assert_eq!(AssetCategory::parse("weapon"), None);
    }

    #[test]
    fn the_table_row_matches_the_category_it_is_indexed_by() {
        // The one thing that would silently mis-budget every asset: a row out of order.
        for category in AssetCategory::ALL {
            assert_eq!(
                CATEGORIES[category as usize].name,
                category.name(),
                "the table is out of step with the enum at {category}"
            );
        }
    }

    #[test]
    fn no_category_is_looser_than_the_absolute_runtime_limits() {
        let limits = AssetLimits::default();
        for category in AssetCategory::ALL {
            let row = category.limits();
            assert!(
                row.max_material_slots <= limits.max_material_slots,
                "{category}"
            );
            assert!(row.max_submeshes <= limits.max_submeshes, "{category}");
            assert!(
                row.preferred_texture_size <= limits.max_texture_size,
                "{category}"
            );
            if category != AssetCategory::Unclassified {
                assert!(row.max_vertices <= limits.max_vertices, "{category}");
                assert!(row.max_triangles <= limits.max_triangles, "{category}");
                assert!(
                    row.preferred_material_slots <= row.max_material_slots,
                    "{category}"
                );
            }
        }
    }

    #[test]
    fn an_exception_is_the_only_way_past_the_absolute_ceiling() {
        let limits = AssetLimits::default();
        assert_eq!(
            limits.vertex_ceiling(AssetCategory::HeroProp, false),
            30_000
        );
        assert_eq!(
            limits.vertex_ceiling(AssetCategory::HeroProp, true),
            100_000
        );
        assert_eq!(limits.triangle_ceiling(AssetCategory::Rock, false), 15_000);
        assert_eq!(limits.triangle_ceiling(AssetCategory::Rock, true), 80_000);
    }

    #[test]
    fn the_lod_bands_bracket_the_specifications_percentages() {
        let limits = AssetLimits::default();
        let Some((low, high)) = limits.lod_band(1) else {
            panic!("LOD1 has a band");
        };
        assert!(low <= 0.5 && high >= 0.5, "{low}..{high}");
        let Some((low, high)) = limits.lod_band(3) else {
            panic!("LOD3 has a band");
        };
        // The specification's 5-10% band must fit inside the accepted one.
        assert!(low <= 0.05 && high >= 0.10, "{low}..{high}");
        assert_eq!(limits.lod_band(4), None);
    }
}
