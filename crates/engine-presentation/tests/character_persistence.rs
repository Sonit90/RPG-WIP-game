//! What the player is wearing, end to end: equip it, write it, restart, and find it there.
//!
//! The unit suites cover each piece — the sheet's move rules, the file's validation, the
//! translation between names and items. This is the seam between them, which is the part a
//! player actually experiences: the ring they put on before dinner is on the same finger
//! afterwards, and the gap they left in the bag is in the same cell.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_game::arena::{self, ArenaConfig};
use engine_game::character::{
    EquipSlot, ItemFamily, ItemSlot, ItemStack, Stat, item_by_key, starting_item,
};
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_presentation::character_save::{apply_config, config_of};
use engine_presentation::snapshot::UiSnapshot;
use engine_presentation::ui::character_menu::{CharacterMenu, CharacterMenuOutcome};
use engine_presentation::ui::input::UiInput;
use engine_settings::character::{
    CHARACTER_FILE_NAME, CharacterConfigManager, EquippedEntry, HeroLoadout,
};

use std::fs;
use std::path::PathBuf;

const VIEWPORT: (u32, u32) = (1920, 1080);

/// A directory of this test's own, removed when the guard is dropped.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("engine-character-e2e-{name}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a writable temporary directory");
        Self { path }
    }

    fn file(&self) -> PathBuf {
        self.path.join(CHARACTER_FILE_NAME)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A running arena with one step taken, and every hero as new: wearing and carrying nothing.
fn fresh() -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    simulation
}

/// [`fresh`], with every hero handed four white pieces to wear and five to carry, so there is
/// something to move around.
fn simulation() -> Simulation {
    let mut simulation = fresh();
    for hero in Hero::ALL {
        let sheet = simulation.roster_mut().sheet_mut(hero);
        for slot in [
            EquipSlot::Chest,
            EquipSlot::Legs,
            EquipSlot::Wrist,
            EquipSlot::Feet,
        ] {
            let def = starting_item(slot.family()).expect("a white piece");
            sheet
                .equipment_mut()
                .equip(slot, ItemStack::one(def.id))
                .expect("fits");
        }
        for family in [
            ItemFamily::Head,
            ItemFamily::Shoulder,
            ItemFamily::Back,
            ItemFamily::Ring,
            ItemFamily::Trinket,
        ] {
            let def = starting_item(family).expect("a white piece");
            sheet.inventory_mut().add(ItemStack::one(def.id));
        }
    }
    simulation.refresh_character();
    simulation
}

/// A simulation whose roster came out of `path`, the way the application starts up.
fn restart(path: &PathBuf) -> Simulation {
    let mut simulation = fresh();
    let manager = CharacterConfigManager::load(path);
    if manager.existed() {
        apply_config(manager.config(), simulation.roster_mut());
        simulation.refresh_character();
    }
    simulation
}

/// Saves `simulation`'s roster to `path`, the way a completed drag does.
fn save(simulation: &Simulation, path: &PathBuf) {
    let mut manager = CharacterConfigManager::load(path);
    manager.update(config_of(simulation.roster()));
    manager.save();
}

fn centre(rect: engine_render_data::ui::UiRect) -> (f32, f32) {
    (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
}

fn press(x: f32, y: f32) -> UiInput {
    UiInput {
        pointer_down: true,
        pointer_pressed: true,
        ..UiInput::new().with_pointer(x, y)
    }
}

fn release(x: f32, y: f32) -> UiInput {
    UiInput {
        pointer_released: true,
        ..UiInput::new().with_pointer(x, y)
    }
}

#[test]
fn a_drag_in_the_panel_reaches_the_simulation_and_survives_a_restart() {
    let dir = TempDir::new("drag-and-restart");
    let mut simulation = simulation();
    let mut snapshot = UiSnapshot::build(&simulation);

    let mut menu = CharacterMenu::new();
    menu.open();

    // Pick the ring out of the bag and drop it on a finger, as a player would.
    let cell = snapshot
        .character
        .inventory
        .iter()
        .position(|slot| slot.is_some_and(|stack| stack.fits(EquipSlot::Ring1)))
        .expect("a ring in the bag");
    let (from_x, from_y) = centre(menu.cell_rect(cell, VIEWPORT));
    menu.update(&press(from_x, from_y), &snapshot, VIEWPORT);
    let (to_x, to_y) = centre(menu.equipment_rect(EquipSlot::Ring2, VIEWPORT));
    let outcome = menu.update(&release(to_x, to_y), &snapshot, VIEWPORT);

    let CharacterMenuOutcome::Move { from, to } = outcome else {
        panic!("the drop asked for nothing: {outcome:?}");
    };
    simulation.push_command(engine_game::PlayerCommand::MoveItem { from, to });
    simulation.step_once();

    // The panel now reads the sheet as it actually is.
    snapshot.refresh(&simulation, None);
    assert!(snapshot.character.equipment[EquipSlot::Ring2.index()].is_some());
    assert!(snapshot.character.inventory[cell].is_none());

    save(&simulation, &dir.file());
    let reloaded = restart(&dir.file());
    assert_eq!(
        reloaded.sheet().equipment().get(EquipSlot::Ring2),
        simulation.sheet().equipment().get(EquipSlot::Ring2),
        "the ring did not survive the restart"
    );
    assert_eq!(reloaded.sheet().inventory().get(cell), None);
}

#[test]
fn a_first_launch_starts_with_nothing_and_the_second_reads_it_back() {
    let dir = TempDir::new("first-launch");
    let manager = CharacterConfigManager::load(dir.file());
    assert!(!manager.existed(), "a file appeared out of nowhere");

    // Nothing to apply, so the simulation keeps what it started with: no items at all.
    let simulation = restart(&dir.file());
    assert_eq!(simulation.sheet().equipment().count(), 0);
    assert_eq!(simulation.sheet().inventory().count(), 0);
    save(&simulation, &dir.file());

    let second = CharacterConfigManager::load(dir.file());
    assert!(second.existed());
    let reloaded = restart(&dir.file());
    assert_eq!(reloaded.roster(), simulation.roster());
}

#[test]
fn every_character_keeps_its_own_gear_across_a_restart() {
    let dir = TempDir::new("per-hero");
    let mut simulation = simulation();

    // Undress the Warrior only.
    let free = simulation
        .roster()
        .sheet(Hero::Warrior)
        .inventory()
        .cells()
        .iter()
        .position(Option::is_none)
        .expect("a free cell");
    simulation
        .roster_mut()
        .sheet_mut(Hero::Warrior)
        .move_item(
            ItemSlot::Equipment(EquipSlot::Legs),
            ItemSlot::Inventory(free),
        )
        .expect("the leggings come off");
    save(&simulation, &dir.file());

    let reloaded = restart(&dir.file());
    assert_eq!(
        reloaded
            .roster()
            .sheet(Hero::Warrior)
            .equipment()
            .get(EquipSlot::Legs),
        None
    );
    assert!(
        reloaded
            .roster()
            .sheet(Hero::Druid)
            .equipment()
            .get(EquipSlot::Legs)
            .is_some(),
        "undressing the Warrior undressed the Druid"
    );
    assert!(
        reloaded
            .roster()
            .sheet(Hero::Warrior)
            .inventory()
            .get(free)
            .is_some(),
        "the leggings did not stay where they were put"
    );
}

#[test]
fn stats_come_back_with_the_gear_that_produced_them() {
    let dir = TempDir::new("stats");
    let mut simulation = simulation();
    let ring = item_by_key("reinforced_iron_band").expect("the stamina ring");
    let cell = simulation
        .sheet()
        .inventory()
        .cells()
        .iter()
        .position(Option::is_none)
        .expect("a free cell");
    simulation
        .roster_mut()
        .sheet_mut(Hero::FireMage)
        .inventory_mut()
        .put(cell, Some(ItemStack::one(ring.id)));
    simulation
        .move_item(
            ItemSlot::Inventory(cell),
            ItemSlot::Equipment(EquipSlot::Ring1),
        )
        .expect("on it goes");

    let stamina = simulation.character_stats().get(Stat::Stamina);
    let health = simulation
        .world()
        .health(simulation.player().entity)
        .map(engine_game::Health::max)
        .expect("health");
    save(&simulation, &dir.file());

    let reloaded = restart(&dir.file());
    assert_eq!(reloaded.character_stats().get(Stat::Stamina), stamina);
    assert_eq!(
        reloaded
            .world()
            .health(reloaded.player().entity)
            .map(engine_game::Health::max),
        Some(health),
        "the health bar did not come back with the ring"
    );
}

#[test]
fn a_hand_edited_file_full_of_nonsense_still_produces_a_playable_character() {
    let dir = TempDir::new("nonsense");
    fs::write(
        dir.file(),
        r#"{
            "heroes": [
                {
                    "hero": "firemage",
                    "equipment": [
                        { "slot": "ring1", "item": "worn_travellers_cloak" },
                        { "slot": "not_a_slot", "item": "simple_cloth_boots" },
                        { "slot": "chest", "item": "simple_cloth_chestpiece" },
                        { "slot": "chest", "item": "simple_cloth_leggings" }
                    ],
                    "inventory": [
                        { "index": 2, "item": "plain_copper_band", "quantity": 1 },
                        { "index": 99999, "item": "tarnished_pendant", "quantity": 1 },
                        { "index": 3, "item": "a_sword_from_the_future", "quantity": 4 }
                    ]
                },
                { "hero": "", "equipment": [], "inventory": [] }
            ]
        }"#,
    )
    .expect("a writable file");

    let simulation = restart(&dir.file());
    let sheet = simulation.sheet();
    assert_eq!(
        sheet.equipment().get(EquipSlot::Ring1),
        None,
        "a cloak reached a finger"
    );
    assert!(
        sheet.equipment().get(EquipSlot::Chest).is_some(),
        "the one legal entry was dropped with the rest"
    );
    assert!(sheet.inventory().get(2).is_some(), "the legal ring is gone");
    assert_eq!(sheet.inventory().get(3), None, "an unknown item was kept");
    assert_eq!(sheet.inventory().count(), 1);
    // And the character is still playable: health from Stamina, main stat intact.
    assert!(simulation.character_stats().get(Stat::Stamina) > 0.0);
}

#[test]
fn a_save_written_by_a_later_build_costs_that_item_and_nothing_else() {
    let dir = TempDir::new("later-build");
    let mut manager = CharacterConfigManager::load(dir.file());
    let mut config = engine_settings::character::CharacterConfig::default();
    config.set_loadout(HeroLoadout {
        hero: "druid".to_owned(),
        equipment: vec![
            EquippedEntry::new("head", "crown_of_a_patch_not_yet_written"),
            EquippedEntry::new("feet", "simple_cloth_boots"),
        ],
        inventory: Vec::new(),
        ..HeroLoadout::default()
    });
    manager.update(config);
    manager.save();

    let mut simulation = restart(&dir.file());
    simulation.set_hero(Hero::Druid);
    assert_eq!(simulation.sheet().equipment().get(EquipSlot::Head), None);
    assert!(
        simulation
            .sheet()
            .equipment()
            .get(EquipSlot::Feet)
            .is_some()
    );
}
