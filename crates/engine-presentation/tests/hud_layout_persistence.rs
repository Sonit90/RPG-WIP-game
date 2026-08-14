//! The HUD arrangement, end to end: arrange it, write it, restart, and change resolution.
//!
//! The unit suites cover each piece — the layout's arithmetic, the bars' model, the
//! configuration file's validation. This is the seam between them, which is the part a
//! player actually experiences: what they set up on Monday at 1080p is what they find on
//! Tuesday on a 1440p ultrawide.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::input::KeyCode;
use engine_game::firemage::FireAbility;
use engine_game::input_map::Bindings;
use engine_presentation::ui::action_bar::{ActionBarSet, SlotAddress};
use engine_presentation::ui::hud_layout::{HudElementId, HudLayout};
use engine_render_data::ui::UiScale;
use engine_settings::hud::{HUD_FILE_NAME, HudConfigManager, HudOrientation};

use std::fs;
use std::path::PathBuf;

const HD: (u32, u32) = (1280, 720);
const FHD: (u32, u32) = (1920, 1080);
const QHD: (u32, u32) = (2560, 1440);
const ULTRAWIDE: (u32, u32) = (3440, 1440);

/// A directory of this test's own, removed when the guard is dropped.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("engine-hud-e2e-{name}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a writable temporary directory");
        Self { path }
    }

    fn file(&self) -> PathBuf {
        self.path.join(HUD_FILE_NAME)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The application's own save: the layout owns where things are, the bars own what is in
/// them, and the two are merged into one file.
fn save(manager: &mut HudConfigManager, layout: &HudLayout, bars: &ActionBarSet) {
    let mut config = layout.config().clone();
    bars.write_config(&mut config);
    manager.update(config);
    manager.save_if_dirty();
}

/// The application's own load.
fn load(manager: &HudConfigManager) -> (HudLayout, ActionBarSet) {
    (
        HudLayout::new(manager.config().clone()),
        ActionBarSet::from_config(manager.config()),
    )
}

/// Where the point an element is anchored by sits, as a fraction of the viewport.
///
/// The anchored corner rather than the centre, because the centre is *not* what a
/// resolution-independent layout preserves: an element keeps its pixel size, so on a smaller
/// screen it covers a larger fraction and its middle necessarily moves. What is preserved —
/// and what a player means by "it stayed where I put it" — is the corner or edge it was
/// parked against.
fn anchor_point(layout: &HudLayout, id: HudElementId, viewport: (u32, u32)) -> (f32, f32) {
    let rect = layout.rect_of(id, UiScale::default(), viewport);
    let (align_x, align_y) = layout.placement(id).anchor.alignment();
    (
        (rect.x + rect.width * align_x) / viewport.0 as f32,
        (rect.y + rect.height * align_y) / viewport.1 as f32,
    )
}

#[test]
fn a_first_launch_writes_the_class_bar_and_the_second_reads_it_back() {
    let dir = TempDir::new("first-launch");

    // First launch: no file, so the application fills bar one in and saves.
    let mut manager = HudConfigManager::load(dir.file());
    assert!(!manager.existed(), "a file appeared from nowhere");
    let bars = ActionBarSet::with_default_bindings();
    let layout = HudLayout::new(manager.config().clone());
    save(&mut manager, &layout, &bars);

    // Second launch: the file is the arrangement.
    let manager = HudConfigManager::load(dir.file());
    assert!(manager.existed());
    let (layout, restored) = load(&manager);
    assert_eq!(restored, bars, "the class bar did not survive a restart");
    assert!(layout.is_enabled(HudElementId::ActionBar(0)));
    for index in 1..engine_settings::hud::ACTION_BARS {
        assert!(!layout.is_enabled(HudElementId::ActionBar(index)));
    }
    for id in HudElementId::FRAMES {
        assert!(layout.is_enabled(id), "{id:?} is not on by default");
    }
}

#[test]
fn a_cleared_bar_stays_cleared_across_a_restart() {
    // The reason `existed` matters: helpfully putting the class back would undo something
    // the player deliberately did.
    let dir = TempDir::new("cleared");
    let mut manager = HudConfigManager::load(dir.file());
    let mut bars = ActionBarSet::with_default_bindings();
    for slot in 0..engine_settings::hud::ACTION_BAR_SLOTS {
        bars.clear_slot(SlotAddress::new(0, slot));
    }
    let layout = HudLayout::new(manager.config().clone());
    save(&mut manager, &layout, &bars);

    let manager = HudConfigManager::load(dir.file());
    assert!(manager.existed());
    let (_, restored) = load(&manager);
    assert!(
        restored.bar(0).expect("bar one").is_empty(),
        "the class was put back"
    );
}

#[test]
fn a_whole_arrangement_survives_a_restart() {
    let dir = TempDir::new("arrangement");
    let mut manager = HudConfigManager::load(dir.file());
    let mut layout = HudLayout::new(manager.config().clone());
    let mut bars = ActionBarSet::with_default_bindings();
    let bindings = Bindings::default();

    // Everything the brief asks to persist, changed from its default.
    layout.set_enabled(HudElementId::ActionBar(2), true);
    layout.flip_orientation(2);
    layout.set_scale(HudElementId::ActionBar(2), 1.3);
    layout.move_to(
        HudElementId::ActionBar(2),
        (140.0, 300.0),
        layout.element_size(HudElementId::ActionBar(2)),
        UiScale::default(),
        FHD,
    );
    layout.set_enabled(HudElementId::Party, false);
    layout.set_scale(HudElementId::Target, 1.5);
    layout.move_to(
        HudElementId::Player,
        (900.0, 700.0),
        layout.element_size(HudElementId::Player),
        UiScale::default(),
        FHD,
    );
    assert!(bars.assign_spell(SlotAddress::new(2, 0), Some(FireAbility::Pyroblast)));
    assert!(bars.assign_key(SlotAddress::new(2, 0), KeyCode::F4, &bindings));
    assert!(bars.clear_slot(SlotAddress::new(0, 9)));

    let placements: Vec<_> = HudElementId::all().map(|id| layout.placement(id)).collect();
    save(&mut manager, &layout, &bars);

    // Restart.
    let manager = HudConfigManager::load(dir.file());
    let (reloaded, restored_bars) = load(&manager);

    assert_eq!(restored_bars, bars, "the slots changed");
    assert_eq!(
        HudElementId::all()
            .map(|id| reloaded.placement(id))
            .collect::<Vec<_>>(),
        placements,
        "a placement changed"
    );
    assert_eq!(reloaded.orientation(2), HudOrientation::Vertical);
    assert!(reloaded.is_enabled(HudElementId::ActionBar(2)));
    assert!(!reloaded.is_enabled(HudElementId::Party));
    assert!((reloaded.scale_of(HudElementId::Target) - 1.5).abs() < 1e-5);
    assert_eq!(
        restored_bars
            .slot(SlotAddress::new(2, 0))
            .expect("the slot exists")
            .key,
        Some(KeyCode::F4)
    );
}

#[test]
fn an_arrangement_means_the_same_thing_at_every_resolution() {
    let dir = TempDir::new("resolution");
    let mut manager = HudConfigManager::load(dir.file());
    let mut layout = HudLayout::new(manager.config().clone());
    for id in HudElementId::all() {
        layout.set_enabled(id, true);
    }

    // Arrange at 1080p, in four different corners of the screen.
    let targets = [
        (HudElementId::ActionBar(0), (700.0, 980.0)),
        (HudElementId::Player, (40.0, 60.0)),
        (HudElementId::Target, (1500.0, 90.0)),
        (HudElementId::Party, (60.0, 800.0)),
    ];
    for (id, target) in targets {
        layout.move_to(id, target, layout.element_size(id), UiScale::default(), FHD);
    }
    save(
        &mut manager,
        &layout,
        &ActionBarSet::with_default_bindings(),
    );

    // Restart on a different display. Everything is in the same relative place, and nothing
    // is off the edge.
    let manager = HudConfigManager::load(dir.file());
    let (reloaded, _) = load(&manager);
    for (id, _) in targets {
        let reference = anchor_point(&reloaded, id, FHD);
        for viewport in [HD, QHD, ULTRAWIDE, (1280, 1024), (800, 600)] {
            let rect = reloaded.rect_of(id, UiScale::default(), viewport);
            let where_now = anchor_point(&reloaded, id, viewport);
            assert!(
                (where_now.0 - reference.0).abs() < 0.01
                    && (where_now.1 - reference.1).abs() < 0.01,
                "{id:?} moved from {reference:?} to {where_now:?} at {viewport:?}"
            );
            let visible_x = (rect.right().min(viewport.0 as f32) - rect.x.max(0.0)).max(0.0);
            let visible_y = (rect.bottom().min(viewport.1 as f32) - rect.y.max(0.0)).max(0.0);
            assert!(
                visible_x > 1.0 && visible_y > 1.0,
                "{id:?} is off a {viewport:?} screen: {rect:?}"
            );
        }
    }
}

#[test]
fn a_hand_edited_file_full_of_nonsense_still_produces_a_playable_hud() {
    let dir = TempDir::new("nonsense");
    fs::write(
        dir.file(),
        r#"{
            "bars": [
                {
                    "placement": { "enabled": true, "anchor": "nowhere", "scale": 99.0 },
                    "orientation": "diagonal",
                    "slots": [ { "spell": "frostbolt", "key": "MOUSE9" } ]
                }
            ],
            "frames": { "player": { "offset_x": 1e30 } }
        }"#,
    )
    .expect("a writable temporary file");

    let manager = HudConfigManager::load(dir.file());
    let (layout, bars) = load(&manager);

    // An unparseable file is a default layout, not a crash and not a refusal to start.
    assert_eq!(layout.config(), &engine_settings::hud::HudConfig::default());
    assert_eq!(bars, ActionBarSet::default());
    for id in HudElementId::all() {
        let rect = layout.rect_of(id, UiScale::default(), FHD);
        assert!(rect.x.is_finite() && rect.y.is_finite(), "{id:?} {rect:?}");
    }
    // And it was repaired, so the next launch does not have to do this again.
    let repaired = HudConfigManager::load(dir.file());
    assert!(repaired.existed(), "the file was not rewritten as valid");
}

#[test]
fn no_two_slots_share_a_key_through_a_save_and_a_reload() {
    let dir = TempDir::new("keys");
    let mut manager = HudConfigManager::load(dir.file());
    let mut bars = ActionBarSet::with_default_bindings();
    let bindings = Bindings::default();

    // Hand `1` from bar one to bar four, the way the slot menu's REPLACE does.
    bars.assign_spell(SlotAddress::new(3, 6), Some(FireAbility::Blink));
    assert!(bars.assign_key(SlotAddress::new(3, 6), KeyCode::Digit1, &bindings));
    let layout = HudLayout::new(manager.config().clone());
    save(&mut manager, &layout, &bars);

    let manager = HudConfigManager::load(dir.file());
    let (_, restored) = load(&manager);
    assert_eq!(
        restored.slot_for_key(KeyCode::Digit1),
        Some(SlotAddress::new(3, 6))
    );
    let mut keys: Vec<KeyCode> = restored
        .occupied()
        .filter_map(|(_, slot)| slot.key)
        .collect();
    let count = keys.len();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), count, "a key is bound twice after a reload");
}
