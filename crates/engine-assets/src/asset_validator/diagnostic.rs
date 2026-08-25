//! One thing that is wrong with an asset, said precisely enough to act on.
//!
//! ```text
//!  [ASSET_VERTEX_LIMIT]  Error
//!  large_rock exceeds the large_prop vertex budget
//!  at model            actual 18,421   limit 15,000
//!  recommendation: decimate or retopologize LOD0
//! ```
//!
//! # Why a structured value and not a string
//!
//! A message is for a person and a code is for a program. The import pipeline wants to ask
//! "did anything with severity Error happen", a build step wants to ask "did
//! `ASSET_TRIANGLE_LIMIT` happen", and an artist wants to read the sentence. Formatting the
//! sentence early throws the first two away, so the sentence is built last — see
//! [`super::report`].

use std::fmt;

use crate::error::AssetError;

/// How much a diagnostic matters.
///
/// Ordered, so `severity >= Severity::Warning` is a meaningful question and the worst
/// finding in a list is its maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Worth knowing. Never a reason to stop.
    Info,
    /// The asset is legal and expensive, or legal and unusual. It loads.
    ///
    /// Every preferred target in the specification lands here: a 4096 texture where 2048 is
    /// preferred is a cost decision somebody should look at, not a broken asset.
    Warning,
    /// The asset does not enter the runtime library.
    ///
    /// Either the runtime cannot draw it safely, or it is past a limit
    /// `docs/style/ASSET_LIMITATIONS.md` calls hard.
    Error,
}

impl Severity {
    /// The word reports print.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// The stable machine-readable name of a rule.
///
/// These strings are an interface: a build script greps for them and a person searches the
/// documentation for them. Rename one and every filter anybody wrote stops matching, so
/// they are spelled once, here, and never inlined at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ValidationCode {
    /// LOD0 has more vertices than the category, or the absolute ceiling, allows.
    VertexLimit,
    /// A level of detail has more triangles than allowed.
    TriangleLimit,
    /// More material slots than the category allows.
    MaterialLimit,
    /// More material slots than the category prefers.
    MaterialPreferred,
    /// A material's parameters are unusable — a NaN, a value outside its range.
    MaterialInvalid,
    /// A submesh names a material the model does not have.
    MaterialReference,
    /// A texture is larger than the runtime accepts.
    TextureLimit,
    /// A texture is larger than the category prefers.
    TexturePreferred,
    /// A texture's texels do not match its dimensions, or a dimension is zero.
    TextureData,
    /// A texture is in a format its slot cannot use — a normal map declared as sRGB.
    TextureFormat,
    /// A material names a texture the model does not have.
    TextureReference,
    /// The model carries textures nothing samples.
    TextureCount,
    /// Geometry with no usable normals.
    MissingNormals,
    /// Geometry with no usable tangents, on an asset that needs them.
    MissingTangents,
    /// Geometry with no texture coordinates.
    MissingUv,
    /// Normals that are not unit length, or not finite.
    InvalidNormals,
    /// Tangents that are degenerate, not finite, or carry a handedness other than ±1.
    InvalidTangents,
    /// Texture coordinates that are not finite.
    InvalidUv,
    /// An index past the end of the vertices, or a count that is not a triangle list.
    InvalidIndices,
    /// Geometry that cannot be drawn as it stands: a non-finite position, a zero-area face.
    InvalidGeometry,
    /// A submesh or level of detail with nothing in it.
    EmptyMesh,
    /// More submeshes in one level than the category allows.
    SubmeshLimit,
    /// More submeshes in one level than the category prefers.
    SubmeshPreferred,
    /// Skinning data that does not match the skeleton.
    SkinningInvalid,
    /// An animation clip that does not match the skeleton.
    AnimationInvalid,
    /// The level-of-detail chain cannot be selected from: unordered, or not a reduction.
    LodInvalid,
    /// Fewer levels of detail than the category expects.
    LodMissing,
    /// A level of detail is a long way from its recommended share of LOD0.
    LodReduction,
    /// More collision primitives than the specification allows.
    CollisionLimit,
    /// No collision on an asset whose category is expected to block the player.
    CollisionMissing,
    /// A transform that is not finite, is degenerate, or is wildly out of range.
    InvalidScale,
    /// The asset's measured size does not look like the metres the engine works in.
    BoundsSuspect,
    /// Triangles are dense enough for the size of the asset to suggest raw generator output.
    GeometryDensity,
}

impl ValidationCode {
    /// The stable name. What reports print and scripts match on.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VertexLimit => "ASSET_VERTEX_LIMIT",
            Self::TriangleLimit => "ASSET_TRIANGLE_LIMIT",
            Self::MaterialLimit => "ASSET_MATERIAL_LIMIT",
            Self::MaterialPreferred => "ASSET_MATERIAL_PREFERRED",
            Self::MaterialInvalid => "ASSET_MATERIAL_INVALID",
            Self::MaterialReference => "ASSET_MATERIAL_REFERENCE",
            Self::TextureLimit => "ASSET_TEXTURE_LIMIT",
            Self::TexturePreferred => "ASSET_TEXTURE_PREFERRED",
            Self::TextureData => "ASSET_TEXTURE_DATA",
            Self::TextureFormat => "ASSET_TEXTURE_FORMAT",
            Self::TextureReference => "ASSET_TEXTURE_REFERENCE",
            Self::TextureCount => "ASSET_TEXTURE_COUNT",
            Self::MissingNormals => "ASSET_MISSING_NORMALS",
            Self::MissingTangents => "ASSET_MISSING_TANGENTS",
            Self::MissingUv => "ASSET_MISSING_UV",
            Self::InvalidNormals => "ASSET_INVALID_NORMALS",
            Self::InvalidTangents => "ASSET_INVALID_TANGENTS",
            Self::InvalidUv => "ASSET_INVALID_UV",
            Self::InvalidIndices => "ASSET_INVALID_INDICES",
            Self::InvalidGeometry => "ASSET_INVALID_GEOMETRY",
            Self::EmptyMesh => "ASSET_EMPTY_MESH",
            Self::SubmeshLimit => "ASSET_SUBMESH_LIMIT",
            Self::SubmeshPreferred => "ASSET_SUBMESH_PREFERRED",
            Self::SkinningInvalid => "ASSET_SKINNING_INVALID",
            Self::AnimationInvalid => "ASSET_ANIMATION_INVALID",
            Self::LodInvalid => "ASSET_LOD_INVALID",
            Self::LodMissing => "ASSET_LOD_MISSING",
            Self::LodReduction => "ASSET_LOD_REDUCTION",
            Self::CollisionLimit => "ASSET_COLLISION_LIMIT",
            Self::CollisionMissing => "ASSET_COLLISION_MISSING",
            Self::InvalidScale => "ASSET_INVALID_SCALE",
            Self::BoundsSuspect => "ASSET_BOUNDS_SUSPECT",
            Self::GeometryDensity => "ASSET_GEOMETRY_DENSITY",
        }
    }
}

impl fmt::Display for ValidationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What part of the asset a diagnostic is about.
///
/// The half of a diagnostic an artist reads first: "which of my forty submeshes". Carrying
/// the name as well as the index is deliberate — an index alone sends somebody counting
/// through an outliner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// The asset as a whole.
    Model,
    /// One level of detail.
    Lod {
        /// Which level: 0 is LOD0.
        level: usize,
    },
    /// One submesh inside one level of detail.
    Submesh {
        /// Which level the submesh is in.
        level: usize,
        /// Its position in that level.
        index: usize,
        /// Its name, as the source file spells it.
        name: String,
    },
    /// One material slot.
    Material {
        /// Its position in the model's material list.
        index: usize,
        /// Its name.
        name: String,
    },
    /// One texture.
    Texture {
        /// Its position in the model's texture list.
        index: usize,
        /// Its name.
        name: String,
    },
    /// One animation clip.
    Clip {
        /// Its position in the model's clip list.
        index: usize,
        /// Its name.
        name: String,
    },
    /// The asset's collision geometry.
    Collision,
}

impl fmt::Display for Subject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model => formatter.write_str("model"),
            Self::Lod { level } => write!(formatter, "lods[{level}]"),
            Self::Submesh { level, index, name } => {
                write!(formatter, "lods[{level}] submesh {index} '{name}'")
            }
            Self::Material { index, name } => write!(formatter, "material {index} '{name}'"),
            Self::Texture { index, name } => write!(formatter, "texture {index} '{name}'"),
            Self::Clip { index, name } => write!(formatter, "clip {index} '{name}'"),
            Self::Collision => formatter.write_str("collision"),
        }
    }
}

/// A number a diagnostic measured or allowed.
///
/// Three shapes because the rules measure three kinds of thing, and printing a resolution as
/// two separate counts loses the fact that they belong together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Measure {
    /// A count of things: vertices, triangles, slots, primitives.
    Count(u64),
    /// A continuous quantity: metres, a fraction, a density.
    Scalar(f32),
    /// A texture size.
    Resolution {
        /// Texels across.
        width: u32,
        /// Texels down.
        height: u32,
    },
}

impl Measure {
    /// The count this is, if it is one. What a test asserts on.
    #[must_use]
    pub const fn count(self) -> Option<u64> {
        match self {
            Self::Count(value) => Some(value),
            _ => None,
        }
    }
}

impl fmt::Display for Measure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Count(value) => formatter.write_str(&grouped(*value)),
            Self::Scalar(value) => write!(formatter, "{value:.3}"),
            Self::Resolution { width, height } => write!(formatter, "{width}x{height}"),
        }
    }
}

/// A number with thousands separators: `72341` becomes `72,341`.
///
/// Reports are read by people comparing a measurement against a budget, and the difference
/// between 7,234 and 72,341 is the difference between fine and not, so the digits are
/// grouped rather than left as a run.
#[must_use]
pub fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// One finding: what rule, how bad, where, and by how much.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// Whether this stops the asset.
    pub severity: Severity,
    /// Which rule found it.
    pub code: ValidationCode,
    /// The sentence a person reads. Names the thing and the numbers.
    pub message: String,
    /// What part of the asset it is about.
    pub subject: Subject,
    /// What was measured, where a measurement makes sense.
    pub actual: Option<Measure>,
    /// What was allowed, where a limit makes sense.
    pub limit: Option<Measure>,
    /// What to do about it, where there is a useful answer.
    pub recommendation: Option<String>,
    /// The budget's name when this is a budget failure rather than a structural one.
    ///
    /// Only used to turn a diagnostic back into the [`AssetError`] the runtime load path
    /// reports, which distinguishes the two — see [`Diagnostic::to_asset_error`].
    pub budget: Option<String>,
}

impl Diagnostic {
    /// A structural failure: something the runtime cannot draw.
    #[must_use]
    pub fn error(code: ValidationCode, subject: Subject, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            subject,
            actual: None,
            limit: None,
            recommendation: None,
            budget: None,
        }
    }

    /// A cost or shape finding: legal, and worth somebody's attention.
    #[must_use]
    pub fn warning(code: ValidationCode, subject: Subject, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, subject, message)
        }
    }

    /// Something worth reporting that is neither wrong nor expensive.
    #[must_use]
    pub fn info(code: ValidationCode, subject: Subject, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            ..Self::error(code, subject, message)
        }
    }

    /// Attaches the measurement and the limit.
    #[must_use]
    pub fn measured(mut self, actual: Measure, limit: Measure) -> Self {
        self.actual = Some(actual);
        self.limit = Some(limit);
        self
    }

    /// Attaches the advice.
    #[must_use]
    pub fn advising(mut self, recommendation: impl Into<String>) -> Self {
        self.recommendation = Some(recommendation.into());
        self
    }

    /// Marks this as a budget failure and names the budget.
    #[must_use]
    pub fn over(mut self, budget: impl Into<String>) -> Self {
        self.budget = Some(budget.into());
        self
    }

    /// The [`AssetError`] this diagnostic is, for the callers that report one error.
    ///
    /// A budget failure becomes [`AssetError::OverBudget`] with both numbers, and everything
    /// else becomes [`AssetError::Invalid`] naming the place inside the model. This is the
    /// one bridge between the collecting validator and the fail-fast runtime path, which is
    /// what keeps the two from being two implementations of the same checks.
    #[must_use]
    pub fn to_asset_error(&self, model: &str) -> AssetError {
        match (&self.budget, self.actual, self.limit) {
            (Some(budget), Some(Measure::Count(actual)), Some(Measure::Count(limit))) => {
                AssetError::over_budget(model, budget.clone(), actual, limit)
            }
            _ => AssetError::invalid(model, self.subject.to_string(), self.message.clone()),
        }
    }
}

impl fmt::Display for Diagnostic {
    /// One line: the code, the place, and the sentence.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "[{}] {} at {}: {}",
            self.code, self.severity, self.subject, self.message
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severities_order_from_harmless_to_fatal() {
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn every_code_is_spelled_once_and_looks_like_a_code() {
        let codes = [
            ValidationCode::VertexLimit,
            ValidationCode::TriangleLimit,
            ValidationCode::MaterialLimit,
            ValidationCode::TextureLimit,
            ValidationCode::MissingNormals,
            ValidationCode::MissingTangents,
            ValidationCode::MissingUv,
            ValidationCode::InvalidIndices,
            ValidationCode::InvalidGeometry,
            ValidationCode::InvalidScale,
            ValidationCode::LodInvalid,
            ValidationCode::CollisionLimit,
        ];
        for code in codes {
            let text = code.as_str();
            assert!(text.starts_with("ASSET_"), "{text}");
            assert_eq!(text.to_uppercase(), text, "{text}");
        }
    }

    #[test]
    fn a_subject_says_which_submesh_of_which_level() {
        let subject = Subject::Submesh {
            level: 1,
            index: 3,
            name: "canopy".to_owned(),
        };
        assert_eq!(subject.to_string(), "lods[1] submesh 3 'canopy'");
    }

    #[test]
    fn numbers_are_grouped_so_a_budget_can_be_read_at_a_glance() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(72_341), "72,341");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn a_budget_diagnostic_becomes_an_over_budget_error_and_a_structural_one_does_not() {
        let budget = Diagnostic::error(
            ValidationCode::TriangleLimit,
            Subject::Lod { level: 0 },
            "too many triangles",
        )
        .measured(Measure::Count(90_000), Measure::Count(20_000))
        .over("triangle");
        let error = budget.to_asset_error("rock");
        assert!(matches!(error, AssetError::OverBudget { .. }), "{error}");
        assert!(error.to_string().contains("90000"), "{error}");

        let structural = Diagnostic::error(
            ValidationCode::InvalidIndices,
            Subject::Submesh {
                level: 0,
                index: 0,
                name: "body".to_owned(),
            },
            "index 7 is past the end of 3 vertices",
        );
        let error = structural.to_asset_error("rock");
        assert!(matches!(error, AssetError::Invalid { .. }), "{error}");
        assert!(error.to_string().contains("past the end"), "{error}");
    }
}
