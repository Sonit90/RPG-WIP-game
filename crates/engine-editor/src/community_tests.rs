//! Tests for authoring community maps: the dungeon panel, the listing, validation, the
//! approved-library warning, export and the template.
//!
//! They drive the editor the way the screen does — a click on an element the panel built, keys
//! into a field, a frame at a time — and assert on the level, so a button that is drawn but not
//! wired fails here.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test code may assert on known-good values"
)]

use std::path::PathBuf;

use engine_core::input::{InputEvent, InputState, KeyCode};
use engine_core::math::{Mat4, Vec3};
use engine_environment::{EnvironmentScene, scene_file};
use engine_presentation::ui::input::UiInput;
use engine_realm::mapcheck::{self, Rules, Target};
use engine_render_data::{Camera, Projection};

use crate::commands::Command;
use crate::community::{self, LevelField, SETTLE_SECONDS, Tab, TextTarget};
use crate::panels::{self, Element};
use crate::{Editor, EditorOutcome, Selected};

const VIEWPORT: (u32, u32) = (1920, 1080);
const FRAME: f32 = 1.0 / 60.0;

fn overhead() -> Mat4 {
    let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
    camera.set_position(Vec3::new(0.0, 20.0, 0.0));
    camera
        .look_at(
            Vec3::new(0.0, 20.0, 0.0),
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
        )
        .expect("a valid view");
    camera.view_projection()
}

/// A directory of this test's own, so parallel tests never share a file.
fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = std::env::temp_dir()
        .join("mygame_editor_community_tests")
        .join(format!(
            "{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    std::fs::create_dir_all(&directory).expect("a temp directory");
    directory.join(name)
}

/// An editor that is open on the template, as a community map.
fn community_editor() -> (Editor, EnvironmentScene) {
    let (meta, scene) = community::template().expect("the template reads");
    let mut editor = Editor::new();
    editor.enter(Vec3::ZERO, "community_blank", VIEWPORT);
    editor.save_path = scratch("community_blank.json");
    editor.community.meta = Some(meta);
    (editor, scene)
}

fn frame_with(
    editor: &mut Editor,
    scene: &mut EnvironmentScene,
    input: &InputState,
    ui: &UiInput,
    seconds: f32,
) -> EditorOutcome {
    editor.update(input, ui, scene, &overhead(), VIEWPORT, seconds)
}

fn idle_frames(editor: &mut Editor, scene: &mut EnvironmentScene, seconds: f32) {
    let mut left = seconds;
    while left > 0.0 {
        frame_with(
            editor,
            scene,
            &InputState::new(),
            &UiInput::new(),
            FRAME.min(left),
        );
        left -= FRAME;
    }
}

fn key(key: KeyCode) -> InputState {
    let mut input = InputState::new();
    input.apply(InputEvent::KeyDown { key, repeat: false });
    input
}

fn type_keys(editor: &mut Editor, scene: &mut EnvironmentScene, keys: &[KeyCode]) {
    for pressed in keys {
        frame_with(editor, scene, &key(*pressed), &UiInput::new(), FRAME);
    }
}

fn elements(editor: &Editor, scene: &EnvironmentScene) -> Vec<Element> {
    panels::inspector(editor, scene, &editor.layout_for(VIEWPORT))
}

fn centre_of(element: &Element) -> (f32, f32) {
    let rect = element.rect();
    (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
}

/// The first element `wanted` accepts, scrolling the page down until it comes into view.
fn find_scrolling(
    editor: &mut Editor,
    scene: &EnvironmentScene,
    wanted: &impl Fn(&Element) -> bool,
) -> Option<Element> {
    editor.inspector_scroll = 0.0;
    for _ in 0..60 {
        if let Some(found) = elements(editor, scene).into_iter().find(|e| wanted(e)) {
            return Some(found);
        }
        editor.inspector_scroll += 40.0;
    }
    None
}

/// Presses the left button on the first element `wanted` accepts.
fn click(
    editor: &mut Editor,
    scene: &mut EnvironmentScene,
    wanted: impl Fn(&Element) -> bool,
) -> EditorOutcome {
    let element = find_scrolling(editor, scene, &wanted).expect("the panel has that control");
    let pointer = centre_of(&element);
    let outcome = frame_with(
        editor,
        scene,
        &InputState::new(),
        &UiInput {
            pointer,
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new()
        },
        FRAME,
    );
    frame_with(
        editor,
        scene,
        &InputState::new(),
        &UiInput {
            pointer,
            pointer_released: true,
            ..UiInput::new()
        },
        FRAME,
    );
    outcome
}

fn click_button(editor: &mut Editor, scene: &mut EnvironmentScene, wanted: Command) {
    click(
        editor,
        scene,
        |element| matches!(element, Element::Button { command, .. } if *command == wanted),
    );
}

fn undo(editor: &mut Editor, scene: &mut EnvironmentScene) {
    editor.execute(Command::Undo, scene);
}

fn redo(editor: &mut Editor, scene: &mut EnvironmentScene) {
    editor.execute(Command::Redo, scene);
}

fn mapcheck_text(scene: &EnvironmentScene, meta: &mapcheck::Meta) -> mapcheck::Report {
    mapcheck::check_str(
        &community::export_text(scene, meta),
        &Rules::default_community(),
    )
}

// ------------------------------------------------------------------------------ the template

#[test]
fn the_template_validates_clean() {
    let (meta, scene) = community::template().expect("the template reads");
    let report = community::check_scene(&scene, Some(&meta));
    assert!(report.errors.is_empty(), "{report}");
    assert!(report.warnings.is_empty(), "{report}");
    // And the file itself, as the server would read it.
    let raw = include_str!("../../../assets/templates/community_blank.json");
    let report = mapcheck::check_str(raw, &Rules::default_community());
    assert!(report.is_valid(), "{report}");
    assert!(!scene.spawn_points.is_empty());
    assert!(
        scene
            .dungeon
            .as_ref()
            .is_some_and(|d| d.boss_gate.is_some())
    );
}

#[test]
fn new_community_map_writes_a_working_copy_that_opens_as_a_community_map() {
    let mut editor = Editor::new();
    editor.enter(Vec3::ZERO, "anything", VIEWPORT);
    editor.save_path = scratch("level.json");
    let mut scene = EnvironmentScene::empty("anything");
    editor.execute(Command::NewCommunityMap, &mut scene);
    let copy = editor
        .take_open_request()
        .expect("the copy is to be opened");
    assert!(copy.is_file(), "{}", copy.display());
    assert!(community::sidecar_path(&copy).is_file());

    // What the application does with an open request.
    let loaded = scene_file::load(&copy).expect("an ordinary level file");
    editor.opened(&loaded, copy.clone());
    assert!(editor.is_community());
    let meta = editor.community_meta().expect("the listing came with it");
    assert_eq!(meta.title, "New Community Map");
    assert!(community::check_scene(&loaded, Some(meta)).is_valid());
    assert_eq!(&*loaded.name, copy.file_stem().unwrap().to_str().unwrap());
}

// ------------------------------------------------------------------- the dungeon panel

#[test]
fn a_typed_required_completion_sets_the_block_and_one_undo_takes_it_back() {
    let (mut editor, mut scene) = community_editor();
    editor.execute(Command::SetTab(Tab::Dungeon), &mut scene);
    let before = scene.dungeon.clone();
    let revision = editor.history_depth();

    click(&mut editor, &mut scene, |element| {
        matches!(
            element,
            Element::Field {
                field: LevelField::RequiredCompletion,
                ..
            }
        )
    });
    assert!(editor.level_edit().is_some(), "the click opened the field");
    type_keys(&mut editor, &mut scene, &[KeyCode::Digit7, KeyCode::Enter]);

    assert!(editor.level_edit().is_none());
    assert_eq!(scene.dungeon.as_ref().unwrap().required_completion, 7.0);
    assert_eq!(editor.history_depth().0, revision.0 + 1, "one undo step");

    undo(&mut editor, &mut scene);
    assert_eq!(scene.dungeon, before);
    redo(&mut editor, &mut scene);
    assert_eq!(scene.dungeon.as_ref().unwrap().required_completion, 7.0);
}

#[test]
fn the_timer_field_is_optional_clamped_undoable_and_exports() {
    let (mut editor, mut scene) = community_editor();
    editor.execute(Command::SetTab(Tab::Dungeon), &mut scene);
    assert_eq!(scene.dungeon.as_ref().unwrap().timer_seconds, None);
    let before = scene.dungeon.clone();
    let revision = editor.history_depth();

    click(&mut editor, &mut scene, |element| {
        matches!(
            element,
            Element::Field {
                field: LevelField::TimerSeconds,
                ..
            }
        )
    });
    type_keys(
        &mut editor,
        &mut scene,
        &[
            KeyCode::Digit9,
            KeyCode::Digit0,
            KeyCode::Digit0,
            KeyCode::Enter,
        ],
    );
    assert_eq!(scene.dungeon.as_ref().unwrap().timer_seconds, Some(900));
    assert_eq!(editor.history_depth().0, revision.0 + 1, "one undo step");
    undo(&mut editor, &mut scene);
    assert_eq!(scene.dungeon, before);
    redo(&mut editor, &mut scene);
    assert_eq!(scene.dungeon.as_ref().unwrap().timer_seconds, Some(900));

    // Below the range is lifted to it, zero removes the clock, and the file validates.
    assert!(community::set_field(
        &mut scene,
        &mut None,
        LevelField::TimerSeconds,
        10.0
    ));
    assert_eq!(scene.dungeon.as_ref().unwrap().timer_seconds, Some(300));
    let text = community::export_text(&scene, editor.community_meta().expect("listing"));
    assert!(text.contains("\"timer_seconds\": 300") || text.contains("\"timer_seconds\":300"));
    assert!(
        mapcheck::check_str(&text, &Rules::default_community())
            .errors
            .is_empty()
    );
    assert!(community::set_field(
        &mut scene,
        &mut None,
        LevelField::TimerSeconds,
        0.0
    ));
    assert_eq!(scene.dungeon.as_ref().unwrap().timer_seconds, None);
}

#[test]
fn the_boss_pack_is_picked_from_the_placed_packs_or_taken_from_the_selection() {
    let (mut editor, mut scene) = community_editor();
    editor.execute(Command::SetTab(Tab::Dungeon), &mut scene);
    assert_eq!(community::pack_names(&scene), ["PACK 1", "BOSS"]);
    assert_eq!(
        scene.dungeon.as_ref().unwrap().boss_pack.as_deref(),
        Some("BOSS")
    );

    // Pick from the placed packs with the arrows.
    click_button(&mut editor, &mut scene, Command::CycleBossPack(false));
    assert_eq!(
        scene.dungeon.as_ref().unwrap().boss_pack.as_deref(),
        Some("PACK 1")
    );
    undo(&mut editor, &mut scene);
    assert_eq!(
        scene.dungeon.as_ref().unwrap().boss_pack.as_deref(),
        Some("BOSS")
    );

    // Or mark the selected monster's pack.
    editor.select(Selected::Monster(0), false);
    click_button(&mut editor, &mut scene, Command::MarkBossPack);
    assert_eq!(
        scene.dungeon.as_ref().unwrap().boss_pack.as_deref(),
        Some("PACK 1")
    );
    undo(&mut editor, &mut scene);
    redo(&mut editor, &mut scene);
    assert_eq!(
        scene.dungeon.as_ref().unwrap().boss_pack.as_deref(),
        Some("PACK 1")
    );

    // And pull the selected monsters into it.
    editor.select(Selected::Monster(3), false);
    click_button(&mut editor, &mut scene, Command::AssignBossPack);
    assert_eq!(scene.monsters[3].pack.as_deref(), Some("PACK 1"));
    undo(&mut editor, &mut scene);
    assert_eq!(scene.monsters[3].pack.as_deref(), Some("BOSS"));
}

#[test]
fn the_gate_is_placed_typed_into_and_removed_through_undo() {
    let (mut editor, mut scene) = community_editor();
    editor.execute(Command::SetTab(Tab::Dungeon), &mut scene);
    let gate = scene.dungeon.as_ref().unwrap().boss_gate.unwrap();

    click_button(&mut editor, &mut scene, Command::RemoveGate);
    assert!(scene.dungeon.as_ref().unwrap().boss_gate.is_none());
    click_button(&mut editor, &mut scene, Command::AddGate);
    let placed = scene.dungeon.as_ref().unwrap().boss_gate.expect("a gate");
    assert_eq!(placed.half_extents, gate.half_extents, "the default size");

    click(&mut editor, &mut scene, |element| {
        matches!(
            element,
            Element::Field {
                field: LevelField::GateWidth,
                ..
            }
        )
    });
    type_keys(
        &mut editor,
        &mut scene,
        &[KeyCode::Digit6, KeyCode::Digit0, KeyCode::Enter],
    );
    let wide = scene.dungeon.as_ref().unwrap().boss_gate.unwrap();
    assert_eq!(wide.half_extents.x, 30.0, "the field is the full width");

    undo(&mut editor, &mut scene);
    undo(&mut editor, &mut scene);
    undo(&mut editor, &mut scene);
    assert_eq!(scene.dungeon.as_ref().unwrap().boss_gate, Some(gate));
}

#[test]
fn spawn_points_are_added_and_the_selected_one_is_moved_by_its_inspector() {
    let (mut editor, mut scene) = community_editor();
    editor.execute(Command::SetTab(Tab::Dungeon), &mut scene);
    let spawns = scene.spawn_points.len();
    click_button(&mut editor, &mut scene, Command::AddSpawnHere);
    assert_eq!(scene.spawn_points.len(), spawns + 1);
    assert_eq!(editor.selection(), Some(Selected::Spawn(spawns)));
    undo(&mut editor, &mut scene);
    assert_eq!(scene.spawn_points.len(), spawns);
    redo(&mut editor, &mut scene);
    assert_eq!(scene.spawn_points.len(), spawns + 1);
    editor.select(Selected::Spawn(spawns), false);

    // Moving it is the Selection page's position fields, which already undo.
    editor.execute(Command::SetTab(Tab::Inspector), &mut scene);
    click(&mut editor, &mut scene, |element| {
        matches!(
            element,
            Element::Value {
                property: crate::inspector::Property::PositionX,
                ..
            }
        )
    });
    type_keys(&mut editor, &mut scene, &[KeyCode::Digit9, KeyCode::Enter]);
    assert_eq!(scene.spawn_points[spawns].position.x, 9.0);
}

#[test]
fn adds_and_patrols_are_assigned_from_the_panel() {
    let (mut editor, mut scene) = community_editor();
    editor.execute(Command::SetTab(Tab::Dungeon), &mut scene);
    editor.select(Selected::Monster(0), false);
    click_button(&mut editor, &mut scene, Command::AddPatrolPoint);
    click_button(&mut editor, &mut scene, Command::AddPatrolPoint);
    assert_eq!(scene.monsters[0].patrol.len(), 2);
    undo(&mut editor, &mut scene);
    assert_eq!(scene.monsters[0].patrol.len(), 1);
    click_button(&mut editor, &mut scene, Command::ClearPatrol);
    assert!(scene.monsters[0].patrol.is_empty());

    click_button(&mut editor, &mut scene, Command::ClearPool);
    assert!(scene.dungeon.as_ref().unwrap().add_pool.is_empty());
    click_button(&mut editor, &mut scene, Command::AddPoolType);
    assert_eq!(
        scene.dungeon.as_ref().unwrap().add_pool,
        ["skeleton_warrior"]
    );

    // A boss cannot be an add.
    editor.select(Selected::Monster(3), false);
    click_button(&mut editor, &mut scene, Command::ClearPool);
    click_button(&mut editor, &mut scene, Command::AddPoolType);
    assert!(scene.dungeon.as_ref().unwrap().add_pool.is_empty());
}

#[test]
fn the_completion_total_is_the_validators_sum_and_the_panel_says_when_it_falls_short() {
    let (mut editor, mut scene) = community_editor();
    let rules = Rules::default_community();
    let worth = |scene: &EnvironmentScene| -> f32 {
        scene
            .monsters
            .iter()
            .filter_map(|m| mapcheck::monster_credit(&rules, &m.monster, m.enabled, m.completion))
            .sum()
    };
    let total = community::completion(&scene);
    assert_eq!(total.placed, worth(&scene));
    assert!(total.placed > 0.0, "three skeletons are worth something");
    assert_eq!(total.required, Some(10.0));
    assert!(total.is_enough());

    editor.execute(Command::SetTab(Tab::Dungeon), &mut scene);
    let shown = |editor: &Editor, scene: &EnvironmentScene, label: &str| {
        elements(editor, scene)
            .into_iter()
            .find_map(|element| match element {
                Element::Info {
                    label: l, value, ..
                } if l == label => Some(value),
                _ => None,
            })
    };
    assert_eq!(
        shown(&editor, &scene, "Placed"),
        Some(format!("{:.1} completion", total.placed))
    );
    assert_eq!(shown(&editor, &scene, "Needed"), Some("10.0".to_owned()));

    // A boss is not counted, and a disabled monster is not either.
    scene.monsters[0].enabled = false;
    let fewer = community::completion(&scene);
    assert!(fewer.placed < total.placed);
    assert_eq!(fewer.placed, worth(&scene));
    scene.dungeon.as_mut().unwrap().required_completion = fewer.placed + 5.0;
    assert!(!community::completion(&scene).is_enough());
    let short = elements(&editor, &scene).into_iter().any(|element| {
        matches!(&element, Element::Notice { text, .. } if text.starts_with("Short by 5.0"))
    });
    assert!(short, "the panel says the objective cannot be met");

    // And the validator agrees with the panel.
    let meta = editor.community_meta().cloned().unwrap();
    assert!(mapcheck_text(&scene, &meta).has_error("impossible_objective"));
}

// ------------------------------------------------------------------------------- the listing

#[test]
fn the_listing_is_typed_into_fields_that_hold_the_validators_limits_and_undo() {
    let (mut editor, mut scene) = community_editor();
    editor.execute(Command::SetTab(Tab::Community), &mut scene);
    let before = editor.community_meta().cloned().unwrap();

    click(&mut editor, &mut scene, |element| {
        matches!(
            element,
            Element::Text {
                target: TextTarget::Title,
                ..
            }
        )
    });
    // Capitals with shift, a space, then letters.
    let mut shifted = InputState::new();
    shifted.apply(InputEvent::KeyDown {
        key: KeyCode::LeftShift,
        repeat: false,
    });
    shifted.apply(InputEvent::KeyDown {
        key: KeyCode::C,
        repeat: false,
    });
    frame_with(&mut editor, &mut scene, &shifted, &UiInput::new(), FRAME);
    type_keys(
        &mut editor,
        &mut scene,
        &[
            KeyCode::R,
            KeyCode::Y,
            KeyCode::P,
            KeyCode::T,
            KeyCode::Enter,
        ],
    );
    assert_eq!(editor.community_meta().unwrap().title, "Crypt");
    undo(&mut editor, &mut scene);
    assert_eq!(editor.community_meta().unwrap(), &before);
    redo(&mut editor, &mut scene);
    assert_eq!(editor.community_meta().unwrap().title, "Crypt");
}

#[test]
fn the_listing_setters_cut_to_the_validators_limits() {
    let mut scene = EnvironmentScene::empty("limits");
    let mut meta = Some(community::default_meta(&scene));
    let long = "x".repeat(2000);
    community::set_text(&mut scene, &mut meta, TextTarget::Title, &long);
    community::set_text(&mut scene, &mut meta, TextTarget::Description, &long);
    community::set_text(
        &mut scene,
        &mut meta,
        TextTarget::Tags,
        "one two three four five six seven",
    );
    community::set_text(
        &mut scene,
        &mut meta,
        TextTarget::Tags,
        &format!("a {long}"),
    );
    let listing = meta.as_ref().unwrap();
    assert_eq!(listing.title.chars().count(), mapcheck::TITLE_MAX);
    assert_eq!(
        listing.description.chars().count(),
        mapcheck::DESCRIPTION_MAX
    );
    assert_eq!(listing.tags.len(), 2);
    assert_eq!(listing.tags[1].chars().count(), mapcheck::TAG_MAX);

    community::set_text(
        &mut scene,
        &mut meta,
        TextTarget::Tags,
        "one two three four five six seven",
    );
    assert_eq!(meta.as_ref().unwrap().tags.len(), mapcheck::TAGS_MAX);

    assert!(community::set_field(
        &mut scene,
        &mut meta,
        LevelField::Difficulty,
        9.0
    ));
    assert_eq!(
        meta.as_ref().unwrap().difficulty,
        mapcheck::DIFFICULTY_RANGE.1
    );
    community::set_field(&mut scene, &mut meta, LevelField::Difficulty, -3.0);
    assert_eq!(
        meta.as_ref().unwrap().difficulty,
        mapcheck::DIFFICULTY_RANGE.0
    );
}

#[test]
fn a_level_becomes_a_community_map_with_one_button_and_that_is_undoable() {
    let mut editor = Editor::new();
    editor.enter(Vec3::ZERO, "plain", VIEWPORT);
    editor.save_path = scratch("plain.json");
    let mut scene = EnvironmentScene::empty("plain");
    assert!(!editor.is_community());
    editor.execute(Command::SetTab(Tab::Community), &mut scene);
    click_button(&mut editor, &mut scene, Command::MakeCommunity);
    assert!(editor.is_community());
    undo(&mut editor, &mut scene);
    assert!(!editor.is_community());
}

// --------------------------------------------------------------------------- validation

#[test]
fn validation_runs_once_when_edits_settle_and_never_per_frame() {
    let (mut editor, mut scene) = community_editor();
    // Opened: one validation once it has been left alone for a moment.
    editor.opened(&scene, editor.save_path.clone());
    editor.community.meta = Some(community::template().unwrap().0);
    editor.community.dirty_at = Some(editor.clock);
    assert_eq!(editor.validation_runs(), 0);

    idle_frames(&mut editor, &mut scene, SETTLE_SECONDS * 0.5);
    assert_eq!(editor.validation_runs(), 0, "not before the level settles");
    idle_frames(&mut editor, &mut scene, SETTLE_SECONDS);
    assert_eq!(editor.validation_runs(), 1);
    idle_frames(&mut editor, &mut scene, 10.0);
    assert_eq!(
        editor.validation_runs(),
        1,
        "600 idle frames validate nothing"
    );

    // An edit schedules one more, and a burst of edits is one run.
    for _ in 0..5 {
        editor.execute(Command::ToggleDuplicateAdds, &mut scene);
        idle_frames(&mut editor, &mut scene, FRAME * 4.0);
    }
    assert_eq!(editor.validation_runs(), 1, "still settling");
    assert!(editor.validation_is_stale());
    idle_frames(&mut editor, &mut scene, SETTLE_SECONDS + 0.2);
    assert_eq!(editor.validation_runs(), 2);
    assert!(!editor.validation_is_stale());
}

#[test]
fn the_validate_button_runs_it_immediately_and_an_ordinary_level_is_never_checked_unasked() {
    let (mut editor, mut scene) = community_editor();
    editor.execute(Command::SetTab(Tab::Validate), &mut scene);
    assert!(editor.validation().is_none());
    click_button(&mut editor, &mut scene, Command::Validate);
    assert_eq!(editor.validation_runs(), 1);
    assert!(editor.validation().unwrap().is_valid());

    let mut plain = Editor::new();
    plain.enter(Vec3::ZERO, "plain", VIEWPORT);
    plain.save_path = scratch("plain.json");
    let mut level = EnvironmentScene::empty("plain");
    plain.revision += 1;
    idle_frames(&mut plain, &mut level, 5.0);
    assert_eq!(plain.validation_runs(), 0);
}

/// The template with a monster the library does not have and an asset it does not have.
fn broken() -> (Editor, EnvironmentScene) {
    let (mut editor, mut scene) = community_editor();
    scene.monsters[1].monster = "dragon_of_doom".to_owned();
    scene.objects[2].asset = engine_environment::EnvironmentAsset::interned("generated/rock");
    editor.validate_now(&scene);
    (editor, scene)
}

#[test]
fn an_issue_with_a_location_selects_what_it_names() {
    let (mut editor, scene) = broken();
    let report = editor.validation().unwrap().clone();
    let at = |code: &str| {
        report
            .errors
            .iter()
            .position(|issue| issue.code == code)
            .unwrap_or_else(|| panic!("no {code} in\n{report}"))
    };

    assert_eq!(
        editor.select_issue(&scene, at("unknown_monster")),
        Some(Target::Monster(1))
    );
    assert_eq!(editor.selection(), Some(Selected::Monster(1)));
    assert!(editor.status().starts_with("unknown_monster"));

    assert_eq!(
        editor.select_issue(&scene, at("missing_asset")),
        Some(Target::Object(2))
    );
    assert_eq!(editor.selection(), Some(Selected::Object(2)));
}

#[test]
fn clicking_an_issue_row_goes_to_it_and_a_dungeon_issue_opens_the_dungeon_tab() {
    let (mut editor, mut scene) = broken();
    scene.dungeon.as_mut().unwrap().boss_pack = Some("NOBODY".to_owned());
    editor.validate_now(&scene);
    editor.execute(Command::SetTab(Tab::Validate), &mut scene);

    let report = editor.validation().unwrap().clone();
    let monster_row = report
        .errors
        .iter()
        .position(|issue| issue.code == "unknown_monster")
        .unwrap();
    click(
        &mut editor,
        &mut scene,
        |element| matches!(element, Element::Issue { index, .. } if *index == monster_row),
    );
    assert_eq!(editor.selection(), Some(Selected::Monster(1)));
    assert_eq!(editor.tab(), Tab::Validate, "the list stays up");

    let boss_row = report
        .errors
        .iter()
        .position(|issue| issue.code == "missing_boss")
        .unwrap();
    click(
        &mut editor,
        &mut scene,
        |element| matches!(element, Element::Issue { index, .. } if *index == boss_row),
    );
    assert_eq!(editor.tab(), Tab::Dungeon);

    // A listing issue opens the listing.
    editor.community.meta.as_mut().unwrap().title = "x".to_owned();
    editor.validate_now(&scene);
    let title_row = editor
        .validation()
        .unwrap()
        .errors
        .iter()
        .position(|issue| issue.target() == Target::Meta)
        .unwrap();
    editor.select_issue(&scene, title_row);
    assert_eq!(editor.tab(), Tab::Community);
}

#[test]
fn an_issue_for_something_that_is_gone_selects_nothing() {
    let (mut editor, mut scene) = broken();
    scene.monsters.truncate(1);
    let index = editor
        .validation()
        .unwrap()
        .errors
        .iter()
        .position(|issue| issue.code == "unknown_monster")
        .unwrap();
    assert_eq!(editor.select_issue(&scene, index), None);
    assert_eq!(editor.select_issue(&scene, 999), None);
}

// ----------------------------------------------------------------- the approved library

#[test]
fn placing_something_outside_the_approved_library_warns_in_a_community_map_only() {
    let (mut editor, _scene) = community_editor();
    editor.check_asset_approved("crypt_floor");
    assert_eq!(editor.approval_warning(), None);
    editor.check_asset_approved("generated/rock");
    let warning = editor.approval_warning().expect("a warning");
    assert!(warning.starts_with("Approved library only"), "{warning}");
    assert_eq!(editor.status(), warning);
    editor.check_monster_approved("skeleton_warrior");
    assert_eq!(editor.approval_warning(), None, "an approved one clears it");

    let mut plain = Editor::new();
    plain.enter(Vec3::ZERO, "plain", VIEWPORT);
    plain.check_asset_approved("generated/rock");
    assert_eq!(
        plain.approval_warning(),
        None,
        "an ordinary level is not nagged"
    );
}

#[test]
fn placing_an_unapproved_monster_with_u_warns_and_the_validator_agrees() {
    let (mut editor, mut scene) = community_editor();
    let mut catalogue = crate::monster::tests::catalogue().entries().to_vec();
    let mut stray = catalogue[0].clone();
    stray.id = "dragon_of_doom".to_owned();
    catalogue.push(stray);
    editor.set_monster_catalogue(catalogue);
    assert!(editor.browser_mut().choose_monster("dragon_of_doom"));

    let centre = (VIEWPORT.0 as f32 / 2.0, VIEWPORT.1 as f32 / 2.0);
    frame_with(
        &mut editor,
        &mut scene,
        &key(KeyCode::U),
        &UiInput {
            pointer: centre,
            ..UiInput::new()
        },
        FRAME,
    );
    assert!(
        editor
            .approval_warning()
            .is_some_and(|w| w.contains("Dragon Of Doom")),
        "{:?}",
        editor.approval_warning()
    );
    let meta = editor.community_meta().cloned().unwrap();
    assert!(mapcheck_text(&scene, &meta).has_error("unknown_monster"));
}

#[test]
fn turning_a_monster_into_an_unapproved_type_warns() {
    let (mut editor, mut scene) = community_editor();
    scene.monsters[0].monster = "dragon_of_doom".to_owned();
    editor.select(Selected::Monster(0), false);
    editor.check_selected_monsters_approved(&scene);
    assert!(editor.approval_warning().is_some());
    scene.monsters[0].monster = "skeleton_warrior".to_owned();
    editor.check_selected_monsters_approved(&scene);
    assert!(editor.approval_warning().is_none());
}

// ----------------------------------------------------------------------------------- export

#[test]
fn export_is_refused_with_the_issue_list_when_the_map_has_errors() {
    let (mut editor, scene) = broken();
    let path = scratch("refused.community.json");
    let result = editor.export_community(&scene, &path);
    let Err(community::ExportError::Invalid(report)) = result else {
        panic!("expected a refusal, got {result:?}");
    };
    assert!(report.has_error("unknown_monster"));
    assert!(report.has_error("missing_asset"));
    assert!(!path.exists(), "nothing was written");
    assert_eq!(editor.tab(), Tab::Validate, "the list is put on screen");
    assert_eq!(editor.validation(), Some(&report));
    assert!(editor.status().starts_with("Export refused"));
}

#[test]
fn export_needs_a_listing() {
    let mut editor = Editor::new();
    editor.enter(Vec3::ZERO, "plain", VIEWPORT);
    let scene = EnvironmentScene::empty("plain");
    let path = scratch("plain.community.json");
    assert_eq!(
        editor.export_community(&scene, &path),
        Err(community::ExportError::NotCommunity)
    );
    assert!(!path.exists());
}

#[test]
fn an_exported_map_validates_clean_and_loads_back_unchanged() {
    let (mut editor, mut scene) = community_editor();
    // Make it the author's own: edit through the panels, not the file.
    editor.execute(Command::SetTab(Tab::Dungeon), &mut scene);
    click_button(&mut editor, &mut scene, Command::AddSpawnHere);
    editor.select(Selected::Monster(0), false);
    click_button(&mut editor, &mut scene, Command::AddPatrolPoint);
    editor.community.meta.as_mut().unwrap().description = "A short crypt".to_owned();
    let meta = editor.community_meta().cloned().unwrap();
    let original = scene.clone();

    let path = scratch("my_crypt.community.json");
    let report = editor
        .export_community(&scene, &path)
        .expect("a clean map exports");
    assert!(report.errors.is_empty(), "{report}");
    assert_eq!(editor.last_export(), Some(path.as_path()));

    // What the server would say about the file on disk.
    let text = std::fs::read_to_string(&path).expect("the file");
    let report = mapcheck::check_str(&text, &Rules::default_community());
    assert!(report.errors.is_empty(), "{report}");
    assert!(mapcheck::CommunityMap::parse(&text).is_ok());

    // Open it in an editor that has never seen the map.
    let mut other = Editor::new();
    other.enter(Vec3::ZERO, "elsewhere", VIEWPORT);
    other.save_path = scratch("elsewhere.json");
    other.request_open(path.clone());
    let copy = other.take_open_request().expect("a working copy to open");
    assert_ne!(copy, path, "the export itself is never overwritten");
    let loaded = scene_file::load(&copy).expect("an ordinary level");
    other.opened(&loaded, copy);
    assert_eq!(loaded, original, "the level is unchanged");
    assert_eq!(other.community_meta(), Some(&meta), "so is the listing");
}

#[test]
fn the_export_prompt_names_the_file_and_writes_it_beside_the_level() {
    let (mut editor, mut scene) = community_editor();
    editor.execute(Command::SetTab(Tab::Validate), &mut scene);
    click_button(&mut editor, &mut scene, Command::ExportCommunity);
    assert!(editor.is_export_prompt());
    assert_eq!(editor.save_as_text(), Some("community_blank"));
    type_keys(
        &mut editor,
        &mut scene,
        &[
            KeyCode::C,
            KeyCode::R,
            KeyCode::Y,
            KeyCode::P,
            KeyCode::T,
            KeyCode::Enter,
        ],
    );
    assert!(!editor.is_export_prompt());
    let written = editor.last_export().expect("exported").to_path_buf();
    assert_eq!(
        written.file_name().unwrap().to_str().unwrap(),
        "crypt.community.json"
    );
    assert_eq!(written.parent(), editor.save_path().parent());
    // The level itself was not renamed or saved by the prompt.
    assert_eq!(&*scene.name, "community_blank");
}

#[test]
fn saving_a_community_level_keeps_its_listing_and_opening_it_brings_it_back() {
    let (mut editor, mut scene) = community_editor();
    editor.community.meta.as_mut().unwrap().title = "Kept Title".to_owned();
    editor.execute(Command::Save, &mut scene);
    let saved = editor.save_path().to_path_buf();
    let loaded = scene_file::load(&saved).expect("the level");

    let mut other = Editor::new();
    other.enter(Vec3::ZERO, "elsewhere", VIEWPORT);
    other.opened(&loaded, saved);
    assert_eq!(other.community_meta().unwrap().title, "Kept Title");
}

#[test]
fn the_tab_strip_switches_pages_and_each_page_has_content() {
    let (mut editor, mut scene) = community_editor();
    for tab in Tab::ALL {
        click(
            &mut editor,
            &mut scene,
            |element| matches!(element, Element::Tab { tab: t, .. } if *t == tab),
        );
        assert_eq!(editor.tab(), tab);
        let layout = editor.layout_for(VIEWPORT);
        let page = elements(&editor, &scene);
        let strip: Vec<_> = page
            .iter()
            .filter(|element| matches!(element, Element::Tab { .. }))
            .collect();
        assert_eq!(strip.len(), Tab::ALL.len());
        for tab in strip {
            assert!(
                layout
                    .inspector_tabs
                    .contains(centre_of(tab).0, centre_of(tab).1)
            );
        }
        assert!(page.len() > Tab::ALL.len(), "{tab:?} shows something");
        for element in page.iter().filter(|e| !matches!(e, Element::Tab { .. })) {
            assert!(
                element.rect().y >= layout.inspector_body.y - 1.0
                    && element.rect().bottom() <= layout.inspector_body.bottom() + 1.0,
                "{element:?} sits in the body"
            );
        }
    }
}

#[test]
fn every_page_draws_its_words_and_its_issues() {
    use engine_presentation::ui::paint::MenuTheme;
    use engine_render_data::ui::UiDrawList;

    let (mut editor, mut scene) = broken();
    let draw = |editor: &Editor, scene: &EnvironmentScene| {
        let mut list = UiDrawList::new();
        crate::ui::push_editor(
            &mut list,
            editor,
            scene,
            &MenuTheme::default(),
            &mut String::new(),
            VIEWPORT,
        );
        list.text().to_owned()
    };
    for (tab, words) in [
        (Tab::Dungeon, ["Objective", "Boss", "Gate"]),
        (Tab::Community, ["Listing", "Title", "Library"]),
        (Tab::Validate, ["Check", "Validate", "unknown_monster"]),
    ] {
        editor.execute(Command::SetTab(tab), &mut scene);
        let text = draw(&editor, &scene);
        for word in words {
            assert!(
                text.contains(word),
                "{word} is missing from {tab:?}: {text}"
            );
        }
        for label in ["Selection", "Dungeon", "Listing", "Validate"] {
            assert!(text.contains(label), "the tab {label} is not drawn");
        }
    }
}
