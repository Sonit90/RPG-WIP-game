//! What the pipeline knows about an asset that the asset itself does not say.
//!
//! ```text
//!  --category rock          stated by whoever ran the validator   ─┐
//!  asset metadata           stated by a metadata system            ├─► AssetCategory
//!  environment/rocks/…      inferred from the path                 │
//!  nothing                  the section 25 absolute limits        ─┘
//! ```
//!
//! # Why a category cannot come out of the file
//!
//! glTF has no idea whether the mesh in it is a pebble or a cathedral, and the one thing
//! that must never decide is the polygon count: an asset that promoted itself to the next
//! budget up every time it went over budget would make the limits meaningless. So the
//! category is something a person or a pipeline states, and the validator only ever reads it.
//!
//! # Where a category comes from, and why every report says which
//!
//! Four sources, most specific first — see [`CategorySource`]. The source is carried in the
//! spec and printed in the report, because "this asset is over the large_prop budget" and
//! "this asset was *guessed* to be a large_prop and is over that budget" are different
//! statements, and only one of them is worth arguing with.
//!
//! # No metadata file
//!
//! This module deliberately does **not** read a sidecar, a manifest or any other file of its
//! own. The engine has no asset-metadata system yet, and inventing one for the validator
//! would make the validator its owner — the wrong crate to own it, and a format everything
//! else would then have to be taught. When a metadata system exists, it sets
//! [`AssetSpec::category`] with [`CategorySource::Metadata`] and nothing else here changes.

use std::path::Path;

use engine_core::transform::Transform;
use engine_environment::EnvironmentCollision;

use super::limits::AssetCategory;

/// How many collision primitives an [`EnvironmentCollision`] is.
///
/// The engine already has a collision description, and this validator does not invent a
/// second one: it counts the primitives in the one that exists.
///
/// The current representation is a closed set of two — nothing, or one box — so the count is
/// 0 or 1 and the section 13 maximum of 20 cannot be reached by anything the engine can
/// currently author. The rule is still enforced, because the number that has to hold is the
/// specification's rather than today's representation's.
///
/// **TODO:** when `EnvironmentCollision` grows capsules, spheres, convex hulls or compound
/// shapes, extend this function to count them and revisit
/// [`AssetLimits::preferred_collision_primitives`](super::AssetLimits::preferred_collision_primitives).
/// No validation rule changes: only this count does.
#[must_use]
pub fn collision_primitives(collision: EnvironmentCollision) -> u64 {
    match collision {
        EnvironmentCollision::None => 0,
        EnvironmentCollision::Box { .. } => 1,
    }
}

/// Where an asset's category came from.
///
/// Printed in every report. A budget an asset was *told* to meet and a budget it was
/// *guessed* into are worth different amounts of trust, and a report that does not say which
/// invites an argument about the wrong number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CategorySource {
    /// Stated on the command line with `--category`. Wins over everything.
    CommandLine,
    /// Stated by an asset-metadata system.
    ///
    /// Reserved: the engine has no such system yet, so nothing produces this today. It is
    /// named here so that the precedence is written down in one place, and so that whatever
    /// grows into that role has a source to report itself as.
    Metadata,
    /// Inferred from a keyword in the asset's path — see [`category_from_path`].
    Path,
    /// Nothing said. The asset is measured against the section 25 absolute limits alone.
    #[default]
    Fallback,
}

impl CategorySource {
    /// The words a report prints.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::CommandLine => "command-line",
            Self::Metadata => "asset metadata",
            Self::Path => "path",
            Self::Fallback => "fallback (no category; section 25 absolute limits)",
        }
    }
}

impl std::fmt::Display for CategorySource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Everything the validator needs that is not inside the model.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetSpec {
    /// What to call the asset in reports: a path, usually.
    pub asset_id: String,
    /// The budget it is measured against.
    pub category: AssetCategory,
    /// Where that category came from.
    pub category_source: CategorySource,
    /// Whether it has been explicitly justified as an exception to the absolute limits.
    ///
    /// Section 25: "assets exceeding these limits should fail validation unless explicitly
    /// marked as exceptions". This is that mark, and it is deliberately a thing a person has
    /// to state rather than something the asset can claim for itself.
    pub exception: bool,
    /// Collision primitives the pipeline built, or `None` when it built none yet.
    ///
    /// `None` and `Some(0)` are different: the first is "collision has not been authored at
    /// this stage", the second is "this asset has no collision", and only the second is
    /// worth a warning on an asset that should block the player.
    pub collision_primitives: Option<u64>,
    /// The transform the asset is imported or placed with.
    ///
    /// Validated, never applied: a NaN scale or a scale of zero is reported here rather than
    /// silently repaired, because the fix belongs in the source asset — section 14.
    pub transform: Transform,
}

impl AssetSpec {
    /// A spec for `asset_id` in `category`, stated by the caller.
    ///
    /// The caller is a person, a tool or a test that knows what the asset is, so the source
    /// is [`CategorySource::CommandLine`]. Use [`AssetSpec::sourced`] to say otherwise.
    #[must_use]
    pub fn new(asset_id: impl Into<String>, category: AssetCategory) -> Self {
        Self::sourced(asset_id, category, CategorySource::CommandLine)
    }

    /// A spec whose category came from `source`.
    #[must_use]
    pub fn sourced(
        asset_id: impl Into<String>,
        category: AssetCategory,
        source: CategorySource,
    ) -> Self {
        Self {
            asset_id: asset_id.into(),
            category,
            category_source: source,
            exception: false,
            collision_primitives: None,
            transform: Transform::IDENTITY,
        }
    }

    /// The spec for an asset nobody has classified.
    ///
    /// The runtime load path's spec, and the documented fallback for anything the
    /// specification has no category for — a character, a VFX mesh, a piece of background
    /// scenery. [`AssetCategory::Unclassified`] applies section 25's absolute limits and
    /// none of the per-category art-direction targets, which is the honest answer for an
    /// asset the environment specification does not describe.
    ///
    /// **TODO:** characters and VFX meshes need budgets of their own — sections 20 and 21 of
    /// `docs/style/ASSET_LIMITATIONS.md` say what a VFX mesh should cost and nothing says
    /// what a character should. Those belong in their own specifications, and when they
    /// exist they become categories here. Until then this fallback is deliberate, and the
    /// validator does not invent numbers for them.
    #[must_use]
    pub fn unclassified(asset_id: impl Into<String>) -> Self {
        Self::sourced(
            asset_id,
            AssetCategory::Unclassified,
            CategorySource::Fallback,
        )
    }

    /// Marks the asset as an explicitly justified exception.
    #[must_use]
    pub fn as_exception(mut self, exception: bool) -> Self {
        self.exception = exception;
        self
    }

    /// States how many collision primitives the pipeline built.
    #[must_use]
    pub fn with_collision(mut self, primitives: u64) -> Self {
        self.collision_primitives = Some(primitives);
        self
    }

    /// States the transform the asset is imported or placed with.
    #[must_use]
    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    /// States the category and where it came from, overriding whatever was there.
    #[must_use]
    pub fn with_category(mut self, category: AssetCategory, source: CategorySource) -> Self {
        self.category = category;
        self.category_source = source;
        self
    }

    /// The spec for `path`, with the category inferred from the path where it can be.
    ///
    /// Step three and four of the precedence: a path keyword, or the unclassified fallback.
    /// A caller that knows better — the command line, or a pipeline holding metadata —
    /// overrides it with [`AssetSpec::with_category`].
    #[must_use]
    pub fn for_path(path: &Path) -> Self {
        let id = path.to_string_lossy().replace('\\', "/");
        match category_from_path(path) {
            Some(category) => Self::sourced(id, category, CategorySource::Path),
            None => Self::unclassified(id),
        }
    }
}

/// The category a path suggests, by the words in it.
///
/// A convenience, never an authority: it exists so that running the validator over a content
/// tree nobody has classified still measures a rock against the rock budget instead of
/// against nothing. Anything it cannot recognise is left unclassified, because a wrong budget
/// is worse than no budget — and every report says the category was inferred, so a wrong
/// guess is visible rather than silent.
#[must_use]
pub fn category_from_path(path: &Path) -> Option<AssetCategory> {
    let text = path.to_string_lossy().to_lowercase().replace('\\', "/");
    // Most specific first: "hero_rocks" is rock, and "hero_architecture" is not architecture.
    let table = [
        ("hero_architecture", AssetCategory::HeroArchitecture),
        ("hero_arch", AssetCategory::HeroArchitecture),
        ("hero", AssetCategory::HeroProp),
        ("architecture", AssetCategory::Architecture),
        ("building", AssetCategory::Architecture),
        ("modular", AssetCategory::Architecture),
        ("rock", AssetCategory::Rock),
        ("boulder", AssetCategory::Rock),
        ("cliff", AssetCategory::Rock),
        ("tree", AssetCategory::Tree),
        ("foliage", AssetCategory::Foliage),
        ("plant", AssetCategory::Foliage),
        ("grass", AssetCategory::Foliage),
        ("shrub", AssetCategory::Foliage),
        ("tiny", AssetCategory::TinyProp),
        ("small", AssetCategory::SmallProp),
        ("medium", AssetCategory::MediumProp),
        ("large", AssetCategory::LargeProp),
    ];
    table
        .into_iter()
        .find(|(word, _)| text.contains(word))
        .map(|(_, category)| category)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::Vec3;

    #[test]
    fn a_path_suggests_a_budget_and_the_spec_says_it_was_a_guess() {
        let spec = AssetSpec::for_path(Path::new("environment/rocks/giant_rock.glb"));
        assert_eq!(spec.category, AssetCategory::Rock);
        assert_eq!(spec.category_source, CategorySource::Path);
        assert_eq!(spec.asset_id, "environment/rocks/giant_rock.glb");
    }

    #[test]
    fn a_path_nobody_named_falls_back_to_the_absolute_limits_and_says_so() {
        let spec = AssetSpec::for_path(Path::new("characters/firemage.glb"));
        assert_eq!(
            spec.category,
            AssetCategory::Unclassified,
            "a character has no environment budget, and one must not be guessed for it"
        );
        assert_eq!(spec.category_source, CategorySource::Fallback);
    }

    #[test]
    fn an_explicit_category_overrides_what_the_path_suggested() {
        let spec = AssetSpec::for_path(Path::new("environment/rocks/giant_rock.glb"))
            .with_category(AssetCategory::HeroProp, CategorySource::CommandLine);
        assert_eq!(spec.category, AssetCategory::HeroProp);
        assert_eq!(spec.category_source, CategorySource::CommandLine);
    }

    #[test]
    fn the_path_table_prefers_the_more_specific_word() {
        assert_eq!(
            category_from_path(Path::new("environment/hero_architecture/gate.glb")),
            Some(AssetCategory::HeroArchitecture)
        );
        assert_eq!(
            category_from_path(Path::new("props/large_cart.glb")),
            Some(AssetCategory::LargeProp)
        );
        assert_eq!(category_from_path(Path::new("vfx/fire_plume.glb")), None);
    }

    #[test]
    fn every_source_prints_something_a_reader_can_act_on() {
        assert_eq!(CategorySource::CommandLine.label(), "command-line");
        assert_eq!(CategorySource::Path.label(), "path");
        assert!(CategorySource::Fallback.to_string().contains("section 25"));
        assert_eq!(CategorySource::Metadata.label(), "asset metadata");
    }

    #[test]
    fn a_spec_carries_the_transform_without_applying_it() {
        let transform = Transform::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let spec = AssetSpec::new("x", AssetCategory::Rock).with_transform(transform);
        assert_eq!(spec.transform.translation, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn collision_is_counted_out_of_the_engines_own_description() {
        assert_eq!(collision_primitives(EnvironmentCollision::None), 0);
        assert_eq!(
            collision_primitives(EnvironmentCollision::standing(1.0, 2.0, 1.0)),
            1
        );
    }
}
