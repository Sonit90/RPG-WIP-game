//! The validator from outside the crate: the public API, and the content that exists today.
//!
//! The unit tests beside each rule prove the rule. These prove the two things a unit test
//! cannot: that the whole of the validator is reachable through `engine_assets`'s public
//! surface, and that the assets the game actually ships pass the limits
//! `docs/style/ASSET_LIMITATIONS.md` sets for them.

use std::path::Path;

use engine_assets::asset_validator::{
    AssetCategory, AssetLimits, AssetSpec, ValidationCode, collision_primitives, report,
    validate_asset,
};
use engine_assets::disk::{find_model, validate_file};
use engine_assets::procedural::environment::environment_model;
use engine_assets::{ModelAsset, ValidationLimits, validate_model};
use engine_environment::asset::LIBRARY;
use engine_environment::object::default_collision;

/// The category each shipped environment asset is authored as.
///
/// The same table the command line uses, restated here rather than depended on: this crate
/// must not gain a dependency on a tool, and the table is six lines.
fn category_of(name: &str) -> AssetCategory {
    match name {
        "rock_small" | "rock_medium" | "rock_spire" => AssetCategory::Rock,
        "shrub" | "fern" | "leaf_screen" => AssetCategory::Foliage,
        "pine" => AssetCategory::Tree,
        "wall_segment" | "pillar" | "arch" | "dome" => AssetCategory::Architecture,
        "brazier" | "supply_crate" => AssetCategory::MediumProp,
        "crypt_wall" | "wall_pier" | "crypt_gate" | "iron_fence" | "crypt_floor" => {
            AssetCategory::Architecture
        }
        "gravestone" | "grave_cross" | "bone_pile" => AssetCategory::SmallProp,
        "sarcophagus" => AssetCategory::MediumProp,
        "dead_tree" => AssetCategory::Tree,
        "mausoleum" => AssetCategory::HeroArchitecture,
        "banner" => AssetCategory::LargeProp,
        _ => AssetCategory::Unclassified,
    }
}

#[test]
fn every_environment_asset_the_game_ships_passes_its_own_category_budget() {
    let limits = AssetLimits::default();
    for asset in LIBRARY {
        let model = environment_model(asset);
        let spec = AssetSpec::new(format!("procedural:{asset}"), category_of(asset.name()))
            .with_collision(collision_primitives(default_collision(asset)));
        let result = validate_asset(&model, &spec, &limits);
        assert!(
            result.passed(),
            "{asset} must be inside the limits it is authored to:\n{}",
            report(&result, &limits)
        );
    }
}

#[test]
fn the_delivered_character_export_is_inside_the_absolute_runtime_limits() {
    // Skipped where the export is not installed, so a checkout with no content still passes.
    let Ok(Some(model)) = find_model("firemage", "firemage.glb") else {
        return;
    };
    let limits = AssetLimits::default();
    let spec = AssetSpec::unclassified("firemage.glb");
    let result = validate_asset(&model, &spec, &limits);
    assert!(
        result.passed(),
        "the delivered character must be inside section 25's absolute limits:\n{}",
        report(&result, &limits)
    );
}

#[test]
fn a_file_that_is_not_a_model_is_reported_as_malformed_rather_than_validated() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let spec = AssetSpec::new("Cargo.toml", AssetCategory::SmallProp);
    let outcome = validate_file(&manifest, &spec, &AssetLimits::default());
    assert!(outcome.is_err(), "a manifest is not a glTF document");
}

#[test]
fn the_runtime_loader_and_the_asset_pipeline_agree_about_broken_geometry() {
    // One model, two entry points: the fail-fast one the game uses and the collecting one
    // the pipeline uses. A dangling index must stop both, or the two have drifted apart.
    let mut model: ModelAsset = environment_model(LIBRARY[3]);
    model.lods[0].submeshes[0].indices = vec![0, 1, 999_999];

    let runtime = validate_model(&model, &ValidationLimits::default());
    assert!(runtime.is_err(), "the loader must refuse it");

    let result = validate_asset(
        &model,
        &AssetSpec::new("pillar", AssetCategory::Architecture),
        &AssetLimits::default(),
    );
    assert!(result.has(ValidationCode::InvalidIndices), "{result:#?}");
    assert!(!result.passed());
}

#[test]
fn validation_never_modifies_the_asset_it_measures() {
    let before = environment_model(LIBRARY[6]);
    let after = before.clone();
    let spec = AssetSpec::new("rock_small", AssetCategory::Rock).with_collision(1);
    let _ = validate_asset(&after, &spec, &AssetLimits::default());
    assert_eq!(before, after, "a validator reports; a tool repairs");
}
