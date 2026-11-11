//! What a validation run found: the verdict, the findings, and the measurements.
//!
//! ```text
//!  AssetValidationResult
//!   ├ asset_id     "environment/rocks/giant_rock.glb"
//!   ├ category     large_prop
//!   ├ passed       false
//!   ├ diagnostics  every finding, in the order the rules ran
//!   └ statistics   what the asset turned out to contain
//! ```
//!
//! # Why not a boolean
//!
//! Because "no" is not an answer anybody can act on. The result carries the numbers whether
//! it passed or failed, so the same value prints the passing report with its budgets filled
//! in and the failing one with its recommendations — and so a build step can compare two
//! runs and say which asset got heavier.

use super::diagnostic::{Diagnostic, Severity, ValidationCode};
use super::limits::AssetCategory;
use super::spec::CategorySource;

/// What an asset turned out to contain.
///
/// Collected during the single pass the validator makes over the model, never by walking it
/// again: every field here is a counter incremented by a rule that was reading that data
/// anyway.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AssetStatistics {
    /// Vertices at LOD0.
    pub vertex_count: u64,
    /// Triangles at LOD0.
    pub triangle_count: u64,
    /// Vertices across every level of detail.
    pub total_vertex_count: u64,
    /// Triangles across every level of detail.
    pub total_triangle_count: u64,
    /// Material slots.
    pub material_count: u64,
    /// Textures.
    pub texture_count: u64,
    /// Bytes those textures occupy, uncompressed.
    pub texture_bytes: u64,
    /// The largest texture, as width and height.
    pub max_texture_size: (u32, u32),
    /// Levels of detail.
    pub lod_count: u64,
    /// Submeshes at LOD0 — the draws one instance of this asset costs.
    pub submesh_count: u64,
    /// Triangles at each level of detail, LOD0 first.
    pub lod_triangles: Vec<u64>,
    /// Bones, or zero for a rigid asset.
    pub bone_count: u64,
    /// Animation clips.
    pub clip_count: u64,
    /// Collision primitives, where the pipeline supplied them.
    pub collision_primitives: Option<u64>,
    /// The longest dimension of LOD0's bounding sphere, in metres.
    pub size_meters: f32,
    /// Degenerate — zero-area or repeated-index — triangles at LOD0.
    pub degenerate_triangles: u64,
    /// Whether every submesh carries usable tangents.
    pub has_tangents: bool,
    /// Whether every submesh carries texture coordinates.
    pub has_uvs: bool,
}

/// The verdict on one asset.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetValidationResult {
    /// What was validated: a path, an asset name, whatever the caller called it.
    pub asset_id: String,
    /// The budget it was measured against.
    pub category: AssetCategory,
    /// Where that budget came from: stated, inferred, or the absolute-limit fallback.
    ///
    /// Reported, because a budget an asset was told to meet and one it was guessed into are
    /// worth different amounts of trust.
    pub category_source: CategorySource,
    /// Whether it was explicitly marked as an exception to the absolute limits.
    pub exception: bool,
    /// Everything found, in the order the rules ran.
    pub diagnostics: Vec<Diagnostic>,
    /// What it contains.
    pub statistics: AssetStatistics,
}

impl AssetValidationResult {
    /// An empty result for `asset_id`, before any rule has run.
    #[must_use]
    pub fn new(asset_id: impl Into<String>, category: AssetCategory, exception: bool) -> Self {
        Self {
            asset_id: asset_id.into(),
            category,
            category_source: CategorySource::default(),
            exception,
            diagnostics: Vec::new(),
            statistics: AssetStatistics::default(),
        }
    }

    /// Whether the asset may enter the runtime library: no diagnostic is an error.
    ///
    /// Warnings never fail an asset. That is the whole distinction the specification draws
    /// between a hard limit and a preferred target, and collapsing it would mean either
    /// shipping past hard limits or refusing every 4096 texture.
    #[must_use]
    pub fn passed(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    /// The findings that stop the asset.
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.of_severity(Severity::Error)
    }

    /// The findings that do not.
    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.of_severity(Severity::Warning)
    }

    /// The findings of one severity.
    pub fn of_severity(&self, severity: Severity) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(move |diagnostic| diagnostic.severity == severity)
    }

    /// The first error, which is the one a fail-fast caller reports.
    #[must_use]
    pub fn first_error(&self) -> Option<&Diagnostic> {
        self.errors().next()
    }

    /// Whether any finding carries `code`. What a test asserts on.
    #[must_use]
    pub fn has(&self, code: ValidationCode) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code)
    }

    /// The finding carrying `code`, if any.
    #[must_use]
    pub fn find(&self, code: ValidationCode) -> Option<&Diagnostic> {
        self.diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == code)
    }

    /// The worst severity found, or `None` for a clean asset.
    #[must_use]
    pub fn worst(&self) -> Option<Severity> {
        self.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.severity)
            .max()
    }

    /// Records a finding.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }
}

#[cfg(test)]
mod tests {
    use super::super::diagnostic::Subject;
    use super::*;

    fn result_with(diagnostics: Vec<Diagnostic>) -> AssetValidationResult {
        let mut result = AssetValidationResult::new("test", AssetCategory::SmallProp, false);
        result.diagnostics = diagnostics;
        result
    }

    #[test]
    fn an_asset_with_only_warnings_passes() {
        let result = result_with(vec![Diagnostic::warning(
            ValidationCode::TexturePreferred,
            Subject::Model,
            "2048 where 1024 is preferred",
        )]);
        assert!(result.passed(), "a preferred target is not a hard limit");
        assert_eq!(result.warnings().count(), 1);
        assert_eq!(result.worst(), Some(Severity::Warning));
    }

    #[test]
    fn one_error_anywhere_fails_the_asset() {
        let result = result_with(vec![
            Diagnostic::warning(
                ValidationCode::LodMissing,
                Subject::Model,
                "only two levels",
            ),
            Diagnostic::error(ValidationCode::VertexLimit, Subject::Model, "far too many"),
        ]);
        assert!(!result.passed());
        assert!(result.has(ValidationCode::VertexLimit));
        let Some(first) = result.first_error() else {
            panic!("there is an error");
        };
        assert_eq!(first.code, ValidationCode::VertexLimit);
    }

    #[test]
    fn a_clean_asset_has_no_worst_severity() {
        let result = result_with(Vec::new());
        assert!(result.passed());
        assert_eq!(result.worst(), None);
    }
}
