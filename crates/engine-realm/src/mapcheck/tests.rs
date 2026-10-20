//! Tests for community map validation. Each rule is pinned by the code it reports.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test code")]

use serde_json::{Value, json};

use super::*;
use crate::catalog::Catalog;

const FIXTURE: &str = include_str!("../../tests/fixtures/community/valid_small.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("fixture is JSON")
}

fn check(value: &Value) -> Report {
    check_str(&value.to_string(), &Rules::default_community())
}

fn typed() -> CommunityMap {
    CommunityMap::parse(FIXTURE).expect("fixture parses")
}

#[track_caller]
fn assert_error(report: &Report, code: &str) {
    assert!(
        report.has_error(code),
        "expected error {code}, got:\n{report}"
    );
}

#[test]
fn the_valid_fixture_has_no_errors() {
    let report = check_str(FIXTURE, &Rules::default_community());
    assert!(report.is_valid(), "{report}");
    assert!(report.warnings.is_empty(), "{report}");
}

#[test]
fn malformed_json_is_reported() {
    let report = check_str("{ not json", &Rules::default_community());
    assert_error(&report, "malformed_json");
}

#[test]
fn oversize_input_is_refused_before_parsing() {
    let text = " ".repeat(MAX_INPUT_BYTES + 1);
    let report = check_str(&text, &Rules::default_community());
    assert_error(&report, "too_large");
    assert!(CommunityMap::parse(&text).is_err());
}

#[test]
fn a_missing_asset_is_reported_with_its_path() {
    let mut v = fixture();
    v["scene"]["objects"][1]["asset"] = json!("generated/not_in_library");
    let report = check(&v);
    assert_error(&report, "missing_asset");
    assert_eq!(report.errors[0].path, "scene.objects[1].asset");
}

#[test]
fn an_unknown_monster_is_reported() {
    let mut v = fixture();
    v["scene"]["monsters"][2]["monster"] = json!("dragon_of_doom");
    let report = check(&v);
    assert_error(&report, "unknown_monster");
    assert!(
        report
            .errors
            .iter()
            .any(|i| i.path == "scene.monsters[2].monster")
    );
}

#[test]
fn a_boss_outside_the_boss_pack_is_refused() {
    let mut v = fixture();
    v["scene"]["monsters"][0]["monster"] = json!("lord_ashgrave");
    let report = check(&v);
    assert_error(&report, "boss_outside_boss_pack");
}

#[test]
fn two_boss_packs_are_refused() {
    let mut v = fixture();
    let mut extra = v["scene"]["monsters"][6].clone();
    extra["pack"] = json!("SECOND");
    v["scene"]["monsters"].as_array_mut().unwrap().push(extra);
    let report = check(&v);
    assert_error(&report, "too_many_boss_packs");
    assert_error(&report, "boss_outside_boss_pack");
}

#[test]
fn a_spawn_inside_a_wall_is_invalid() {
    let mut v = fixture();
    // The wall stands at x = 30; the default crypt wall box is 8 wide and 5 tall.
    v["scene"]["spawn_points"][0]["position"] = json!([30.0, 0.0, 0.0]);
    assert_error(&check(&v), "invalid_spawn");
}

#[test]
fn a_spawn_outside_the_map_is_invalid() {
    let mut v = fixture();
    v["scene"]["spawn_points"][1]["position"] = json!([500.0, 0.0, 0.0]);
    assert_error(&check(&v), "invalid_spawn");
}

#[test]
fn a_spawn_below_the_ground_is_invalid() {
    let mut v = fixture();
    v["scene"]["spawn_points"][1]["position"] = json!([0.0, -5.0, 0.0]);
    assert_error(&check(&v), "invalid_spawn");
}

#[test]
fn a_nan_spawn_is_invalid() {
    let mut map = typed();
    map.scene.spawn_points[0].position[0] = f32::NAN;
    let report = validate(&map, &Rules::default_community());
    assert_error(&report, "invalid_spawn");
}

#[test]
fn a_non_finite_object_scale_is_reported() {
    let mut map = typed();
    map.scene.objects[0].scale[1] = f32::INFINITY;
    assert_error(&validate(&map, &Rules::default_community()), "non_finite");
}

#[test]
fn an_unreachable_completion_target_is_impossible() {
    let mut v = fixture();
    v["scene"]["dungeon"]["required_completion"] = json!(500.0);
    assert_error(&check(&v), "impossible_objective");
}

#[test]
fn a_boss_pack_that_does_not_exist_is_missing() {
    let mut v = fixture();
    v["scene"]["dungeon"]["boss_pack"] = json!("NOBODY");
    assert_error(&check(&v), "missing_boss");
}

#[test]
fn a_missing_dungeon_block_is_reported() {
    let mut v = fixture();
    v["scene"].as_object_mut().unwrap().remove("dungeon");
    assert_error(&check(&v), "missing_dungeon");
}

#[test]
fn a_boss_gate_outside_the_map_is_reported() {
    let mut v = fixture();
    v["scene"]["dungeon"]["boss_gate"]["center"] = json!([0.0, 0.0, 900.0]);
    assert_error(&check(&v), "bad_boss_gate");
}

#[test]
fn a_bad_add_pool_is_reported() {
    let mut v = fixture();
    v["scene"]["dungeon"]["add_pool"] = json!(["skeleton_warrior", "nonsense"]);
    assert_error(&check(&v), "bad_add_pool");
    v["scene"]["dungeon"]["add_pool"] = json!(["lord_ashgrave"]);
    assert_error(&check(&v), "bad_add_pool");
}

#[test]
fn scripts_and_water_are_unsupported_features() {
    let mut v = fixture();
    v["scene"]["script"] = json!("os.execute('x')");
    v["scene"]["water"] = json!({ "level": 3.0 });
    v["scene"]["monsters"][0]["on_death"] = json!("spawn_dragon");
    let report = check(&v);
    assert_error(&report, "unsupported_feature");
    let paths: Vec<&str> = report.errors.iter().map(|i| i.path.as_str()).collect();
    assert!(paths.contains(&"scene.script"));
    assert!(paths.contains(&"scene.water"));
    assert!(paths.contains(&"scene.monsters[0].on_death"));
}

#[test]
fn declared_loot_is_refused() {
    let mut v = fixture();
    v["scene"]["monsters"][0]["loot"] = json!(["epic_sword"]);
    v["scene"]["dungeon"]["rewards"] = json!({ "experience": 99999 });
    let report = check(&v);
    assert_error(&report, "loot_not_allowed");
    assert_eq!(
        report
            .errors
            .iter()
            .filter(|i| i.code == "loot_not_allowed")
            .count(),
        2
    );
}

#[test]
fn bad_meta_strings_are_reported() {
    let mut v = fixture();
    v["meta"]["title"] = json!("ab");
    assert_error(&check(&v), "bad_meta");

    let mut v = fixture();
    v["meta"]["title"] = json!("bad\u{7}title");
    assert_error(&check(&v), "bad_meta");

    let mut v = fixture();
    v["meta"]["description"] = json!("x".repeat(1001));
    assert_error(&check(&v), "bad_meta");

    let mut v = fixture();
    v["meta"]["tags"] = json!(["a", "b", "c", "d", "e", "f"]);
    assert_error(&check(&v), "bad_meta");

    let mut v = fixture();
    v["meta"]["tags"] = json!(["this tag is far too long"]);
    assert_error(&check(&v), "bad_meta");

    let mut v = fixture();
    v["meta"]["difficulty"] = json!(9);
    assert_error(&check(&v), "bad_meta");
}

#[test]
fn numeric_sanity_is_enforced() {
    let mut v = fixture();
    v["scene"]["objects"][0]["scale"] = json!([20.0, 1.0, 1.0]);
    v["scene"]["monsters"][0]["aggro_radius"] = json!(200.0);
    v["scene"]["monsters"][1]["level"] = json!(99);
    let report = check(&v);
    assert_eq!(
        report
            .errors
            .iter()
            .filter(|i| i.code == "bad_number")
            .count(),
        3,
        "{report}"
    );
}

#[test]
fn limits_are_enforced() {
    let mut v = fixture();
    let object = v["scene"]["objects"][0].clone();
    v["scene"]["objects"] = Value::Array(vec![object; 4001]);
    assert_error(&check(&v), "too_many_objects");

    let mut v = fixture();
    v["scene"]["half_extent"] = json!(400.0);
    assert_error(&check(&v), "bad_extent");
}

#[test]
fn patrols_are_whitelisted_and_checked() {
    let mut v = fixture();
    v["scene"]["monsters"][0]["patrol"] = json!([[1.0, 2.0], [3.0, 4.0]]);
    assert!(check(&v).is_valid());

    v["scene"]["monsters"][0]["patrol"] = json!([[1.0, 2.0], [500.0, 4.0]]);
    assert_error(&check(&v), "bad_patrol");

    let many: Vec<Value> = (0..17).map(|_| json!([0.0, 0.0])).collect();
    v["scene"]["monsters"][0]["patrol"] = Value::Array(many);
    assert_error(&check(&v), "bad_patrol");
}

#[test]
fn a_wrong_format_version_is_refused() {
    let mut v = fixture();
    v["format"] = json!(2);
    assert_error(&check(&v), "unsupported_version");
}

#[test]
fn a_missing_required_field_is_a_schema_error() {
    let mut v = fixture();
    v["scene"].as_object_mut().unwrap().remove("objects");
    assert_error(&check(&v), "schema_error");
}

/// Every official dungeon passes through the same validator under the relaxed rules.
#[test]
fn official_dungeons_pass_the_validator() {
    let catalog = Catalog::builtin().expect("catalog");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let mut failures = String::new();
    for dungeon in catalog.dungeons() {
        let text = std::fs::read_to_string(root.join(&dungeon.scene)).expect("scene file");
        let scene: SceneDocument = serde_json::from_str(&text).expect("scene parses");
        let report = validate_official(&scene, dungeon);
        println!("{}: {report}", dungeon.id);
        if !report.is_valid() {
            failures.push_str(&format!("{}:\n{report}\n", dungeon.id));
        }
    }
    assert!(failures.is_empty(), "{failures}");
}
