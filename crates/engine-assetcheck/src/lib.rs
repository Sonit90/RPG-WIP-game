//! `asset_check`: the asset validator, from a command line.
//!
//! ```text
//!  asset_check assets/props/torch.glb            one asset
//!  asset_check assets/environment                a directory, recursively
//!  asset_check --category rock assets/rocks      with the category stated
//!  asset_check --library                         the environment library the game ships
//! ```
//!
//! # What it is for
//!
//! Keeping an expensive or malformed asset out of the runtime library, at the point where
//! that is cheap: before anybody has built a level out of it. It imports each file with the
//! engine's own importer, runs [`engine_assets::validate_asset`] over the result, prints the
//! report and exits non-zero if anything failed — which is what makes it usable as a build
//! step rather than only as something a person runs.
//!
//! # Why the work lives here and not in `main`
//!
//! So it can be tested. [`run`] takes arguments and a writer and returns an [`Outcome`];
//! `main` turns that into an exit code and nothing else. The exit-code rules — errors fail,
//! warnings do not unless asked — are the part most likely to be depended on by a script,
//! and they are covered by tests at the bottom of this file.

#![deny(missing_docs)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use engine_assets::asset_validator::{
    AssetCategory, AssetLimits, AssetSpec, AssetValidationResult, CategorySource, report,
    summary_line, validate_asset,
};
use engine_assets::disk::validate_file;
use engine_assets::procedural::environment::environment_model;
use engine_environment::asset::LIBRARY;
use engine_environment::object::default_collision;

/// What a run found, across every asset it looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Outcome {
    /// Assets validated.
    pub assets: usize,
    /// Assets with at least one error-severity finding.
    pub failed: usize,
    /// Error-severity findings, across every asset.
    pub errors: usize,
    /// Warning-severity findings, across every asset.
    pub warnings: usize,
    /// Files that could not be read or imported at all.
    pub unreadable: usize,
}

impl Outcome {
    /// Whether the run should fail the build.
    ///
    /// Errors always fail. Warnings fail only when the caller asked for it, because the
    /// specification's preferred targets are advice, and a build that fails on advice is a
    /// build people learn to ignore.
    #[must_use]
    pub fn failing(self, strict_warnings: bool) -> bool {
        self.failed > 0 || self.unreadable > 0 || (strict_warnings && self.warnings > 0)
    }

    /// Folds one asset's result in.
    fn record(&mut self, result: &AssetValidationResult) {
        self.assets += 1;
        let errors = result.errors().count();
        self.errors += errors;
        self.warnings += result.warnings().count();
        if errors > 0 {
            self.failed += 1;
        }
    }
}

/// How a run was configured.
///
/// The default is "check nothing, with the specification's own limits": every field is what
/// a run with no arguments would use, so [`parse`] only ever sets what was actually typed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Options {
    /// Files and directories to validate.
    pub targets: Vec<PathBuf>,
    /// Validate the procedural environment library instead of files.
    pub library: bool,
    /// The category to use for every asset, overriding metadata and the path.
    pub category: Option<AssetCategory>,
    /// Treat every asset as an explicitly justified exception.
    pub exception: bool,
    /// The collision primitive count to assume for every asset.
    pub collision: Option<u64>,
    /// One line per asset rather than a full report.
    pub summary: bool,
    /// Fail the run on warnings as well as errors.
    pub strict_warnings: bool,
    /// Print the usage text and do nothing else.
    pub help: bool,
    /// The limits every asset is measured against.
    pub limits: AssetLimits,
}

/// The usage text.
pub const USAGE: &str = "\
asset_check - validate assets against docs/style/ASSET_LIMITATIONS.md

    asset_check [options] <file-or-directory>...
    asset_check --library

Options:
    --category <name>   measure every asset as this category, overriding metadata
    --exception         treat every asset as an explicitly justified exception
    --collision <n>     assume n collision primitives per asset
    --library           validate the procedural environment library the game ships
    --summary           one line per asset instead of a full report
    --strict-warnings   exit non-zero on warnings as well as errors
    -h, --help          print this

Categories:
    tiny_prop small_prop medium_prop large_prop hero_prop architecture
    hero_architecture foliage rock tree unclassified

An asset's category is resolved in this order: --category, asset metadata where a
system supplies it, a keyword in the asset's path, then the section 25 absolute
limits. Every report says which of the four it used.

Exit status is 0 when nothing failed and 1 when anything did.";

/// Parses the command line.
///
/// # Errors
///
/// Returns a message naming the argument that was not understood. An unknown flag stops the
/// run rather than being ignored: a typo'd `--catgory` that silently validated everything
/// against the wrong budget would be worse than no validation at all.
pub fn parse(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "-h" | "--help" => options.help = true,
            "--library" => options.library = true,
            "--exception" => options.exception = true,
            "--summary" => options.summary = true,
            "--strict-warnings" => options.strict_warnings = true,
            "--category" => {
                let Some(value) = rest.next() else {
                    return Err("--category needs a category name".to_owned());
                };
                let Some(category) = AssetCategory::parse(value) else {
                    return Err(format!("'{value}' is not a category; see --help"));
                };
                options.category = Some(category);
            }
            "--collision" => {
                let Some(value) = rest.next() else {
                    return Err("--collision needs a number".to_owned());
                };
                let Ok(count) = value.parse::<u64>() else {
                    return Err(format!("'{value}' is not a primitive count"));
                };
                options.collision = Some(count);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option '{other}'; see --help"));
            }
            path => options.targets.push(PathBuf::from(path)),
        }
    }
    if !options.help && !options.library && options.targets.is_empty() {
        return Err("nothing to validate; name a file or a directory, or --help".to_owned());
    }
    Ok(options)
}

/// Validates everything the options name, writing the reports to `out`.
pub fn run(options: &Options, out: &mut String) -> Outcome {
    if options.help {
        let _ = writeln!(out, "{USAGE}");
        return Outcome::default();
    }
    let mut outcome = Outcome::default();
    if options.library {
        validate_library(options, out, &mut outcome);
    }
    for target in &options.targets {
        for path in assets_under(target) {
            validate_one(&path, options, out, &mut outcome);
        }
    }
    write_summary(out, &outcome, options);
    outcome
}

/// Every `.glb` or `.gltf` under `target`, or `target` itself if it is a file.
///
/// Sorted, so two runs over the same tree report in the same order and a diff of two runs is
/// about the assets rather than about the filesystem.
#[must_use]
pub fn assets_under(target: &Path) -> Vec<PathBuf> {
    if target.is_file() {
        return vec![target.to_path_buf()];
    }
    let mut found = Vec::new();
    collect(target, &mut found);
    found.sort();
    found
}

/// Walks `directory`, adding every model file to `found`.
fn collect(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, found);
            continue;
        }
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase());
        if matches!(extension.as_deref(), Some("glb" | "gltf")) {
            found.push(path);
        }
    }
}

/// Validates one file and writes its report.
fn validate_one(path: &Path, options: &Options, out: &mut String, outcome: &mut Outcome) {
    let spec = spec_for(path, options);
    match validate_file(path, &spec, &options.limits) {
        Ok(result) => {
            outcome.record(&result);
            write_result(out, &result, options);
        }
        Err(error) => {
            outcome.unreadable += 1;
            let _ = writeln!(out, "UNREADABLE  {}: {error}", path.display());
        }
    }
}

/// The spec for `path`: what the path suggests, with the command line's overrides applied.
///
/// The precedence the documentation states, in code: `--category` first, then whatever a
/// metadata system supplied (none does yet), then the path, then the fallback that measures
/// against the absolute limits alone.
fn spec_for(path: &Path, options: &Options) -> AssetSpec {
    let mut spec = AssetSpec::for_path(path);
    if let Some(category) = options.category {
        spec = spec.with_category(category, CategorySource::CommandLine);
    }
    if options.exception {
        spec.exception = true;
    }
    if let Some(collision) = options.collision {
        spec.collision_primitives = Some(collision);
    }
    spec
}

/// Validates the models the game builds for itself, against the same limits.
///
/// The environment library is content — it is what the arena is made of — and content that
/// is generated rather than imported still has to fit the budget it will be drawn inside.
/// Running it here is what keeps the specification honest about the assets that exist today.
fn validate_library(options: &Options, out: &mut String, outcome: &mut Outcome) {
    for asset in LIBRARY {
        let category = options
            .category
            .unwrap_or_else(|| library_category(asset.name()));
        // Stated metadata rather than a guess: [`library_category`] is a table somebody
        // wrote about assets this crate can name, which is the role an asset-metadata
        // system will fill for imported content.
        let source = if options.category.is_some() {
            CategorySource::CommandLine
        } else {
            CategorySource::Metadata
        };
        let mut spec = AssetSpec::sourced(format!("procedural:{}", asset.name()), category, source)
            .with_collision(engine_assets::asset_validator::collision_primitives(
                default_collision(asset),
            ));
        spec.exception = options.exception;
        if let Some(collision) = options.collision {
            spec.collision_primitives = Some(collision);
        }
        let model = environment_model(asset);
        let result = validate_asset(&model, &spec, &options.limits);
        outcome.record(&result);
        write_result(out, &result, options);
    }
}

/// The category each environment asset is authored as.
///
/// A table rather than an inference: the library is eighteen named things, and what each of
/// them is for is a decision somebody made when they were authored.
#[must_use]
pub fn library_category(name: &str) -> AssetCategory {
    match name {
        "rock_small" | "rock_medium" | "rock_spire" => AssetCategory::Rock,
        "shrub" | "fern" | "leaf_screen" => AssetCategory::Foliage,
        "pine" => AssetCategory::Tree,
        "wall_segment" | "pillar" | "arch" | "dome" => AssetCategory::Architecture,
        "brazier" | "supply_crate" => AssetCategory::MediumProp,
        // Six metres of pole and cloth: a large prop rather than a medium one.
        "banner" => AssetCategory::LargeProp,
        // Ground, arena floor, ridge line and sky shell: scenery the size of the level, which
        // has no art-direction budget in the specification and is measured against the
        // absolute runtime limits alone.
        _ => AssetCategory::Unclassified,
    }
}

/// One asset's report, in whichever form was asked for.
fn write_result(out: &mut String, result: &AssetValidationResult, options: &Options) {
    if options.summary {
        let _ = writeln!(out, "{}", summary_line(result));
    } else {
        let _ = writeln!(out, "{}", report(result, &options.limits));
        let _ = writeln!(out, "{}", "-".repeat(78));
    }
}

/// The last lines: how many assets, how many failed, and what the exit code will be.
fn write_summary(out: &mut String, outcome: &Outcome, options: &Options) {
    let _ = writeln!(
        out,
        "\n{} asset(s) checked, {} failed, {} error(s), {} warning(s){}",
        outcome.assets,
        outcome.failed,
        outcome.errors,
        outcome.warnings,
        if outcome.unreadable > 0 {
            format!(", {} unreadable", outcome.unreadable)
        } else {
            String::new()
        }
    );
    if outcome.failing(options.strict_warnings) {
        let _ = writeln!(
            out,
            "FAILED - see docs/asset-validation.md for what each code means"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(targets: &[&str]) -> Options {
        Options {
            targets: targets.iter().map(PathBuf::from).collect(),
            ..Options::default()
        }
    }

    #[test]
    fn a_category_and_a_collision_count_are_read_off_the_command_line() {
        let arguments: Vec<String> = ["--category", "rock", "--collision", "3", "a.glb"]
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect();
        let Ok(parsed) = parse(&arguments) else {
            panic!("those are valid arguments");
        };
        assert_eq!(parsed.category, Some(AssetCategory::Rock));
        assert_eq!(parsed.collision, Some(3));
        assert_eq!(parsed.targets, vec![PathBuf::from("a.glb")]);
    }

    #[test]
    fn an_unknown_option_stops_the_run_rather_than_validating_the_wrong_thing() {
        let arguments = vec!["--catgory".to_owned(), "rock".to_owned()];
        assert!(parse(&arguments).is_err());
        let empty: Vec<String> = Vec::new();
        assert!(parse(&empty).is_err(), "there is nothing to validate");
        let help = vec!["--help".to_owned()];
        assert!(parse(&help).is_ok(), "help needs no target");
    }

    #[test]
    fn the_run_fails_when_an_asset_has_errors_and_passes_when_it_only_has_warnings() {
        let errors = Outcome {
            assets: 2,
            failed: 1,
            errors: 3,
            warnings: 1,
            unreadable: 0,
        };
        assert!(errors.failing(false), "an error fails the run");

        let warnings = Outcome {
            assets: 2,
            failed: 0,
            errors: 0,
            warnings: 4,
            unreadable: 0,
        };
        assert!(
            !warnings.failing(false),
            "a preferred target is advice, and a build that fails on advice is ignored"
        );
        assert!(
            warnings.failing(true),
            "--strict-warnings is how a project opts into failing on advice"
        );

        let clean = Outcome {
            assets: 5,
            ..Outcome::default()
        };
        assert!(!clean.failing(true));
    }

    #[test]
    fn a_file_that_is_not_a_model_is_counted_as_unreadable_and_fails_the_run() {
        let mut out = String::new();
        let mut options = options(&["Cargo.toml"]);
        options.targets = vec![PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/Cargo.toml"
        ))];
        let outcome = run(&options, &mut out);
        // A path that is a file is validated whatever its extension: naming it explicitly is
        // a statement that it is an asset.
        assert_eq!(outcome.unreadable, 1, "{out}");
        assert!(outcome.failing(false));
        assert!(out.contains("UNREADABLE"), "{out}");
    }

    #[test]
    fn a_directory_with_no_models_in_it_checks_nothing_and_fails_nothing() {
        let mut options = options(&[]);
        options.targets = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut out = String::new();
        let outcome = run(&options, &mut out);
        assert_eq!(outcome.assets, 0);
        assert!(!outcome.failing(false), "{out}");
    }

    #[test]
    fn the_environment_library_the_game_ships_is_checkable_from_the_command_line() {
        let options = Options {
            library: true,
            summary: true,
            ..Options::default()
        };
        let mut out = String::new();
        let outcome = run(&options, &mut out);
        assert_eq!(
            outcome.assets,
            LIBRARY.len(),
            "every library asset is checked: {out}"
        );
        assert!(out.contains("rock_small"), "{out}");
    }

    #[test]
    fn a_library_run_that_only_warns_exits_zero_unless_warnings_were_asked_to_count() {
        let options = Options {
            library: true,
            summary: true,
            ..Options::default()
        };
        let mut out = String::new();
        let outcome = run(&options, &mut out);
        assert_eq!(
            outcome.errors, 0,
            "the shipped library has no errors: {out}"
        );
        assert!(outcome.warnings > 0, "it does have advice: {out}");
        assert!(!outcome.failing(false), "warnings alone must exit zero");
        assert!(outcome.failing(true), "--strict-warnings makes them count");
    }

    #[test]
    fn an_asset_measured_against_a_budget_it_cannot_meet_fails_the_run() {
        // Every library asset as a tiny prop: 300 triangles and 500 vertices. The sky dome
        // alone is 288 triangles and 576 vertices, so at least one asset must fail, and a
        // failing asset must be the difference between exit 0 and exit 1.
        let options = Options {
            library: true,
            summary: true,
            category: Some(AssetCategory::TinyProp),
            ..Options::default()
        };
        let mut out = String::new();
        let outcome = run(&options, &mut out);
        assert!(outcome.failed > 0, "{out}");
        assert!(outcome.errors > 0, "{out}");
        assert!(outcome.failing(false), "an error always fails the run");
        assert!(out.contains("FAIL"), "{out}");
        assert!(
            out.contains("FAILED - see docs/asset-validation.md"),
            "{out}"
        );
    }

    #[test]
    fn a_category_is_resolved_from_the_path_and_the_report_says_it_was_inferred() {
        let spec = spec_for(
            Path::new("assets/environment/rocks/large_rock.glb"),
            &Options::default(),
        );
        assert_eq!(spec.category, AssetCategory::Rock);
        assert_eq!(spec.category_source, CategorySource::Path);
    }

    #[test]
    fn the_command_line_overrides_the_path_and_the_report_says_so() {
        let options = Options {
            category: Some(AssetCategory::HeroProp),
            ..Options::default()
        };
        let spec = spec_for(
            Path::new("assets/environment/rocks/large_rock.glb"),
            &options,
        );
        assert_eq!(spec.category, AssetCategory::HeroProp);
        assert_eq!(spec.category_source, CategorySource::CommandLine);
    }

    #[test]
    fn an_asset_no_source_classifies_falls_back_to_the_absolute_limits() {
        let spec = spec_for(Path::new("assets/vfx/fire_plume.glb"), &Options::default());
        assert_eq!(
            spec.category,
            AssetCategory::Unclassified,
            "no character or VFX budget exists, and one must not be invented"
        );
        assert_eq!(spec.category_source, CategorySource::Fallback);
    }

    #[test]
    fn help_prints_the_usage_and_validates_nothing() {
        let options = Options {
            help: true,
            ..Options::default()
        };
        let mut out = String::new();
        let outcome = run(&options, &mut out);
        assert_eq!(outcome.assets, 0);
        assert!(out.contains("--category"), "{out}");
    }
}
