//! The validation result as something a person reads.
//!
//! ```text
//! Asset:    environment/rocks/giant_rock.glb
//! Category: large_prop
//! Category source: path
//!
//! FAIL
//!
//! Vertices:     72,341 / 15,000
//! Triangles:    118,422 / 12,000
//! ...
//!
//! Errors:
//!   [ASSET_VERTEX_LIMIT]
//!   Vertices: 72,341
//!   Limit:    15,000
//! ```
//!
//! # Why the formatting lives apart from the rules
//!
//! Because they have different readers. A rule produces a [`Diagnostic`] a program can
//! filter; this module turns a pile of them into the six lines an artist needs. Keeping them
//! in one place would mean either a rule that knows about column widths or a result that has
//! already thrown away everything except a string.

use std::fmt::Write as _;

use super::diagnostic::{Diagnostic, Severity, grouped};
use super::limits::AssetLimits;
use super::result::AssetValidationResult;

/// The full report for one asset: the verdict, the numbers, and every finding.
#[must_use]
pub fn report(result: &AssetValidationResult, limits: &AssetLimits) -> String {
    let statistics = &result.statistics;
    let category = result.category;
    let mut out = String::with_capacity(512);

    let _ = writeln!(out, "Asset:    {}", result.asset_id);
    let _ = writeln!(out, "Category: {category}");
    let _ = writeln!(out, "Category source: {}", result.category_source);
    if result.exception {
        let _ = writeln!(out, "Exception: yes (section 25 exception limits apply)");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", if result.passed() { "PASS" } else { "FAIL" });
    let _ = writeln!(out);

    let vertex_ceiling = limits.vertex_ceiling(category, result.exception);
    let triangle_ceiling = limits.triangle_ceiling(category, result.exception);
    let _ = writeln!(
        out,
        "Vertices:     {}",
        against(statistics.vertex_count, vertex_ceiling)
    );
    let _ = writeln!(
        out,
        "Triangles:    {}",
        against(statistics.triangle_count, triangle_ceiling)
    );
    let _ = writeln!(
        out,
        "Submeshes:    {}",
        against(statistics.submesh_count, limits.submesh_ceiling(category))
    );
    let _ = writeln!(
        out,
        "Materials:    {}",
        against(statistics.material_count, limits.material_ceiling(category))
    );
    let _ = writeln!(out, "Textures:     {}", grouped(statistics.texture_count));
    if statistics.texture_count > 0 {
        let (width, height) = statistics.max_texture_size;
        let _ = writeln!(
            out,
            "Max texture:  {width}x{height} ({} KiB total)",
            grouped(statistics.texture_bytes / 1024)
        );
    }
    let levels: Vec<String> = statistics
        .lod_triangles
        .iter()
        .map(|triangles| grouped(*triangles))
        .collect();
    let _ = writeln!(
        out,
        "LODs:         {} [{}]",
        statistics.lod_count,
        levels.join(", ")
    );
    match statistics.collision_primitives {
        Some(primitives) => {
            let _ = writeln!(
                out,
                "Collision:    {} primitive(s) / {}",
                primitives, limits.max_collision_primitives
            );
        }
        None => {
            let _ = writeln!(out, "Collision:    not supplied");
        }
    }
    let _ = writeln!(out, "Size:         {:.2} m across", statistics.size_meters);
    if statistics.bone_count > 0 {
        let _ = writeln!(
            out,
            "Rig:          {} bones, {} clips",
            statistics.bone_count, statistics.clip_count
        );
    }

    section(&mut out, "Errors", result, Severity::Error);
    section(&mut out, "Warnings", result, Severity::Warning);
    section(&mut out, "Notes", result, Severity::Info);

    let advice: Vec<&str> = result
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .filter_map(|diagnostic| diagnostic.recommendation.as_deref())
        .collect();
    if !advice.is_empty() {
        let _ = writeln!(out, "\nRecommendations:");
        let mut seen: Vec<&str> = Vec::with_capacity(advice.len());
        for line in advice {
            if !seen.contains(&line) {
                seen.push(line);
                let _ = writeln!(out, "  - {line}");
            }
        }
    }
    out
}

/// One severity's findings, or nothing at all where there are none.
fn section(out: &mut String, heading: &str, result: &AssetValidationResult, severity: Severity) {
    let mut first = true;
    for diagnostic in result.of_severity(severity) {
        if first {
            let _ = writeln!(out, "\n{heading}:");
            first = false;
        }
        write_diagnostic(out, diagnostic);
    }
}

/// One finding: its code, where it is, what it says, and both numbers.
fn write_diagnostic(out: &mut String, diagnostic: &Diagnostic) {
    let _ = writeln!(out, "  [{}] {}", diagnostic.code, diagnostic.subject);
    let _ = writeln!(out, "  {}", diagnostic.message);
    if let (Some(actual), Some(limit)) = (diagnostic.actual, diagnostic.limit) {
        let _ = writeln!(out, "  actual: {actual}");
        let _ = writeln!(out, "  limit:  {limit}");
    }
    if let Some(recommendation) = &diagnostic.recommendation {
        let _ = writeln!(out, "  hint:   {recommendation}");
    }
    let _ = writeln!(out);
}

/// `1,842 / 2,000`, or just the measurement where the budget is unbounded.
fn against(measured: u64, limit: u64) -> String {
    if limit == u64::MAX {
        grouped(measured)
    } else {
        format!("{} / {}", grouped(measured), grouped(limit))
    }
}

/// One line for a directory run: the verdict, the asset, and what it cost.
#[must_use]
pub fn summary_line(result: &AssetValidationResult) -> String {
    let verdict = if result.passed() { "PASS" } else { "FAIL" };
    let errors = result.errors().count();
    let warnings = result.warnings().count();
    format!(
        "{verdict}  {:<48}  {:>9} tris  {:>9} verts  {errors} error(s), {warnings} warning(s)",
        result.asset_id,
        grouped(result.statistics.triangle_count),
        grouped(result.statistics.vertex_count),
    )
}

#[cfg(test)]
mod tests {
    use super::super::diagnostic::{Measure, Subject, ValidationCode};
    use super::super::limits::AssetCategory;
    use super::*;

    fn failing() -> AssetValidationResult {
        let mut result = AssetValidationResult::new(
            "environment/rocks/giant_rock.glb",
            AssetCategory::LargeProp,
            false,
        );
        result.statistics.vertex_count = 72_341;
        result.statistics.triangle_count = 118_422;
        result.statistics.lod_triangles = vec![118_422];
        result.statistics.lod_count = 1;
        result.push(
            Diagnostic::error(
                ValidationCode::VertexLimit,
                Subject::Lod { level: 0 },
                "72,341 vertices is past the large_prop maximum of 15,000",
            )
            .measured(Measure::Count(72_341), Measure::Count(15_000))
            .advising("decimate or retopologize the mesh"),
        );
        result
    }

    #[test]
    fn a_failing_report_names_the_code_the_measurement_and_the_limit() {
        let text = report(&failing(), &AssetLimits::default());
        assert!(text.contains("FAIL"), "{text}");
        assert!(text.contains("environment/rocks/giant_rock.glb"), "{text}");
        assert!(text.contains("Category: large_prop"), "{text}");
        assert!(text.contains("[ASSET_VERTEX_LIMIT]"), "{text}");
        assert!(text.contains("72,341"), "{text}");
        assert!(text.contains("15,000"), "{text}");
        assert!(text.contains("Recommendations:"), "{text}");
    }

    #[test]
    fn a_passing_report_still_prints_the_budgets_it_was_measured_against() {
        let mut result =
            AssetValidationResult::new("props/torch.glb", AssetCategory::SmallProp, false);
        result.statistics.vertex_count = 1_842;
        result.statistics.triangle_count = 1_412;
        result.statistics.material_count = 2;
        let text = report(&result, &AssetLimits::default());
        assert!(text.contains("PASS"), "{text}");
        assert!(text.contains("1,842 / 2,000"), "{text}");
        assert!(text.contains("1,412 / 1,500"), "{text}");
        assert!(!text.contains("Errors:"), "{text}");
    }

    #[test]
    fn a_summary_line_fits_a_directory_listing() {
        let line = summary_line(&failing());
        assert!(line.starts_with("FAIL"), "{line}");
        assert!(line.contains("118,422"), "{line}");
        assert!(line.contains("1 error(s)"), "{line}");
    }
}
