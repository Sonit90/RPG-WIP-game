//! The dungeon map: projections that agree with the picture, a minimap that turns or does not,
//! a full-screen map that zooms, pans and selects, and a journal that reads the game's tables.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_environment::map::{EncounterKind, frame_for};
use engine_environment::{DungeonSettings, EnvironmentScene, MonsterPlacement, scene_file};
use engine_game::enemy::{EnemyAbility, EnemyKind, LootEntry};
use engine_presentation::ui::dungeon_map::{
    AbilityFilter, AbilityTag, DungeonMap, DungeonMapInfo, DungeonProgress, EncounterStatus,
    MAX_ZOOM, MapKeys, MapMarkers, MapOutcome, MapTheme, MapUnit, MarkerClass, abilities_of,
    ability_effects, ability_tags, ability_timing, loot_detail,
};
use engine_presentation::ui::input::UiInput;
use engine_presentation::ui::map_style::title_case;
use engine_render_data::ui::{UiCommand, UiDrawList, UiRect, UiScale};
use engine_settings::MinimapOrientation;

const VIEWPORT: (u32, u32) = (1920, 1080);

/// A small non-square level: two packs and a boss.
fn level() -> EnvironmentScene {
    let mut scene = EnvironmentScene::empty("test_crypt");
    scene.half_extent = 200.0;
    scene.spawns.player = Vec3::new(0.0, 0.0, 0.0);
    scene.monsters.push(
        MonsterPlacement::new("skeleton_warrior", Vec3::new(10.0, 0.0, -20.0)).in_pack("GATE"),
    );
    scene.monsters.push(
        MonsterPlacement::new("skeleton_warrior", Vec3::new(12.0, 0.0, -20.0)).in_pack("GATE"),
    );
    scene
        .monsters
        .push(MonsterPlacement::new("crypt_archer", Vec3::new(11.0, 0.0, -23.0)).in_pack("GATE"));
    scene.monsters.push(MonsterPlacement::new(
        "grave_brute",
        Vec3::new(-30.0, 0.0, -60.0),
    ));
    scene.monsters.push(
        MonsterPlacement::new("lord_ashgrave", Vec3::new(0.0, 0.0, -140.0))
            .in_pack("HOLLOW STEWARD"),
    );
    let mut dungeon = DungeonSettings::new("TEST CRYPT", 10.0);
    dungeon.boss_pack = Some("HOLLOW STEWARD".to_owned());
    dungeon.add_count = 2;
    dungeon.add_pool = vec!["crypt_ghoul".to_owned(), "grave_acolyte".to_owned()];
    scene.dungeon = Some(dungeon);
    scene
}

fn map_of(scene: &EnvironmentScene) -> DungeonMap {
    let frame = frame_for(scene);
    let mut map = DungeonMap::new(UiScale::new(1.0));
    map.set_info(Some(DungeonMapInfo::from_scene(scene, frame, true)));
    map
}

fn markers(position: Vec3, facing: f32, view_yaw: f32) -> MapMarkers {
    MapMarkers {
        player: Some(MapUnit {
            position,
            facing: Some(facing),
        }),
        view_yaw,
        ..MapMarkers::default()
    }
}

fn click(at: (f32, f32)) -> [UiInput; 2] {
    let press = UiInput {
        pointer: at,
        pointer_down: true,
        pointer_pressed: true,
        ..UiInput::new()
    };
    let release = UiInput {
        pointer: at,
        pointer_released: true,
        ..UiInput::new()
    };
    [press, release]
}

fn middle(rect: UiRect) -> (f32, f32) {
    (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
}

fn map_images(list: &UiDrawList) -> Vec<UiCommand> {
    list.commands()
        .iter()
        .copied()
        .filter(|command| matches!(command, UiCommand::MapImage { .. }))
        .collect()
}

/// The texture coordinate a `MapImage` command samples at a screen point.
fn sampled_uv(command: UiCommand, x: f32, y: f32) -> (f32, f32) {
    let UiCommand::MapImage {
        rect,
        origin,
        du,
        dv,
        ..
    } = command
    else {
        panic!("not a map image");
    };
    let (px, py) = (x - rect.x, y - rect.y);
    (
        origin.0 + du.0 * px + dv.0 * py,
        origin.1 + du.1 * px + dv.1 * py,
    )
}

fn journal(map: &mut DungeonMap) -> String {
    let _ = map.sidebar_lines(VIEWPORT);
    map.expand_all();
    map.sidebar_lines(VIEWPORT).join("\n")
}

#[test]
fn the_level_s_encounters_become_markers_in_title_case_and_the_boss_is_marked() {
    let map = map_of(&level());
    let info = map.info().expect("a level");
    assert_eq!(info.name, "TEST CRYPT");
    assert_eq!(info.encounters.len(), 3);
    assert_eq!(info.encounters[0].title, "Gate");
    assert_eq!(info.encounters[0].monster_count(), 3);
    assert_eq!(
        info.encounters[1].title, "Grave Brute",
        "an unnamed pack is named for its monster"
    );
    assert_eq!(info.encounters[2].kind, EncounterKind::Boss);
    assert_eq!(MarkerClass::of(&info.encounters[2]), MarkerClass::Boss);
    assert_eq!(MarkerClass::of(&info.encounters[0]), MarkerClass::Pack);
    assert_eq!(
        info.encounters[0].kind_label(),
        "Monster pack \u{00B7} 3 enemies"
    );
    assert_eq!(info.totals(), (2, 5, 1));
}

#[test]
fn north_up_keeps_north_at_the_top_and_turns_the_arrow_instead() {
    let mut map = map_of(&level());
    map.set_orientation(MinimapOrientation::NorthUp);
    let facing_east = -std::f32::consts::FRAC_PI_2;
    let markers = markers(Vec3::ZERO, facing_east, 1.0);
    let projection = map
        .minimap_projection(VIEWPORT, &markers)
        .expect("a projection");
    assert_eq!(projection.rotation, 0.0);
    let (cx, cy) = projection.center;
    let north = projection.to_screen(Vec3::new(0.0, 0.0, -10.0));
    let east = projection.to_screen(Vec3::new(10.0, 0.0, 0.0));
    assert!((north.0 - cx).abs() < 1e-3 && north.1 < cy, "-Z is up");
    assert!((east.1 - cy).abs() < 1e-3 && east.0 > cx, "+X is right");
    assert!((projection.arrow_rotation(facing_east) - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
}

#[test]
fn rotating_keeps_the_player_centred_and_the_view_direction_up() {
    let mut map = map_of(&level());
    map.set_orientation(MinimapOrientation::Rotating);
    let player = Vec3::new(5.0, 0.0, -30.0);
    for view_yaw in [0.0f32, 0.7, std::f32::consts::FRAC_PI_2, 3.0, -2.2] {
        let markers = markers(player, view_yaw, view_yaw);
        let projection = map
            .minimap_projection(VIEWPORT, &markers)
            .expect("a projection");
        let centre = projection.to_screen(player);
        assert!(
            (centre.0 - projection.center.0).abs() < 1e-3
                && (centre.1 - projection.center.1).abs() < 1e-3
        );
        let ahead = Vec3::new(
            player.x - view_yaw.sin() * 10.0,
            0.0,
            player.z - view_yaw.cos() * 10.0,
        );
        let at = projection.to_screen(ahead);
        assert!(
            (at.0 - projection.center.0).abs() < 1e-3,
            "{view_yaw}: {at:?}"
        );
        assert!(at.1 < projection.center.1);
        assert!(projection.arrow_rotation(view_yaw).abs() < 1e-6);
        let (wx, wz) = projection.to_world(at);
        assert!((wx - ahead.x).abs() < 1e-3 && (wz - ahead.z).abs() < 1e-3);
    }
}

#[test]
fn the_minimap_picture_and_its_markers_agree_in_both_modes() {
    let scene = level();
    for orientation in MinimapOrientation::ALL {
        let mut map = map_of(&scene);
        map.set_orientation(orientation);
        let markers = markers(Vec3::new(4.0, 0.0, -18.0), 0.3, 0.9);
        let mut list = UiDrawList::new();
        map.push_minimap(&mut list, &markers, VIEWPORT, &MapTheme);
        let images = map_images(&list);
        assert_eq!(images.len(), 1, "one picture, reused");
        let projection = map
            .minimap_projection(VIEWPORT, &markers)
            .expect("a projection");
        let frame = map.info().expect("a level").frame;
        for point in [
            Vec3::new(10.0, 0.0, -20.0),
            Vec3::new(4.0, 0.0, -18.0),
            Vec3::new(-20.0, 0.0, -30.0),
        ] {
            let (x, y) = projection.to_screen(point);
            let uv = sampled_uv(images[0], x, y);
            let expected = frame.world_to_uv(point);
            assert!(
                (uv.0 - expected.0).abs() < 1e-4 && (uv.1 - expected.1).abs() < 1e-4,
                "{orientation:?}"
            );
        }
        assert!(
            list.commands()
                .iter()
                .any(|command| matches!(command, UiCommand::Clip(clip) if clip.circle.is_some()))
        );
    }
}

#[test]
fn the_canvas_keeps_markers_on_the_picture_at_every_zoom_and_pan() {
    let mut map = map_of(&level());
    assert_eq!(map.toggle(), MapOutcome::Opened);
    let markers = markers(Vec3::ZERO, 0.0, 0.0);
    let frame = map.info().expect("a level").frame;
    for (zoom_notches, pan) in [
        (0.0, (0.0, 0.0)),
        (3.0, (40.0, -25.0)),
        (8.0, (-200.0, 100.0)),
    ] {
        let layout = map.screen_layout(VIEWPORT);
        let at = (
            layout.canvas.x + layout.canvas.width * 0.4,
            layout.canvas.y + layout.canvas.height * 0.4,
        );
        map.update(
            &UiInput {
                pointer: at,
                ..UiInput::new()
            },
            zoom_notches,
            VIEWPORT,
            &markers,
        );
        map.advance(1.0, VIEWPORT);
        let press = UiInput {
            pointer: at,
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new()
        };
        let drag = UiInput {
            pointer: (at.0 + pan.0, at.1 + pan.1),
            pointer_down: true,
            ..UiInput::new()
        };
        let release = UiInput {
            pointer: (at.0 + pan.0, at.1 + pan.1),
            pointer_released: true,
            ..UiInput::new()
        };
        for input in [press, drag, release] {
            map.update(&input, 0.0, VIEWPORT, &markers);
        }
        let projection = map.canvas_projection(VIEWPORT).expect("a projection");
        let mut list = UiDrawList::new();
        map.push_map_screen(&mut list, &markers, VIEWPORT, &MapTheme);
        let images = map_images(&list);
        assert_eq!(images.len(), 1);
        for encounter in &map.info().expect("a level").encounters {
            let (x, y) = projection.to_screen(encounter.position);
            let uv = sampled_uv(images[0], x, y);
            let expected = frame.world_to_uv(encounter.position);
            assert!((uv.0 - expected.0).abs() < 1e-4 && (uv.1 - expected.1).abs() < 1e-4);
        }
    }
    assert!(map.zoom() <= MAX_ZOOM);
}

#[test]
fn zooming_glides_and_keeps_the_world_under_the_cursor_still() {
    let mut map = map_of(&level());
    map.open();
    let markers = markers(Vec3::ZERO, 0.0, 0.0);
    let layout = map.screen_layout(VIEWPORT);
    let cursor = (
        layout.canvas.x + layout.canvas.width * 0.3,
        layout.canvas.y + layout.canvas.height * 0.7,
    );
    let before = map
        .canvas_projection(VIEWPORT)
        .expect("projection")
        .to_world(cursor);
    map.update(
        &UiInput {
            pointer: cursor,
            ..UiInput::new()
        },
        2.0,
        VIEWPORT,
        &markers,
    );
    assert!(
        (map.zoom() - 1.0).abs() < 1e-6,
        "the zoom glides rather than jumping"
    );
    map.advance(0.02, VIEWPORT);
    assert!(
        map.zoom() > 1.0 && map.zoom() < 1.44,
        "part of the way after a frame: {}",
        map.zoom()
    );
    map.advance(1.0, VIEWPORT);
    assert!((map.zoom() - 1.44).abs() < 1e-2, "{}", map.zoom());
    let after = map
        .canvas_projection(VIEWPORT)
        .expect("projection")
        .to_world(cursor);
    assert!(
        (before.0 - after.0).abs() < 0.05 && (before.1 - after.1).abs() < 0.05,
        "{before:?} {after:?}"
    );
}

#[test]
fn with_reduced_motion_the_zoom_is_instant() {
    let mut map = map_of(&level());
    map.set_reduced_motion(true);
    map.open();
    let cursor = middle(map.screen_layout(VIEWPORT).canvas);
    map.update(
        &UiInput {
            pointer: cursor,
            ..UiInput::new()
        },
        1.0,
        VIEWPORT,
        &MapMarkers::default(),
    );
    assert!((map.zoom() - 1.2).abs() < 1e-4, "{}", map.zoom());
}

#[test]
fn a_non_square_level_is_never_stretched() {
    let mut map = map_of(&level());
    map.open();
    let frame = map.info().expect("a level").frame;
    assert!(
        (frame.aspect() - 1.0).abs() > 0.2,
        "the test level is not square: {frame:?}"
    );
    let projection = map.canvas_projection(VIEWPORT).expect("projection");
    let mut list = UiDrawList::new();
    map.push_map_screen(&mut list, &MapMarkers::default(), VIEWPORT, &MapTheme);
    let UiCommand::MapImage { du, dv, .. } = map_images(&list)[0] else {
        unreachable!()
    };
    let metres_across = du.0 * frame.width();
    let metres_down = dv.1 * frame.depth();
    assert!((metres_across - metres_down).abs() < 1e-6);
    assert!((metres_across - 1.0 / projection.pixels_per_metre).abs() < 1e-6);
}

#[test]
fn clicking_a_marker_selects_it_empty_ground_clears_it_and_a_drag_does_neither() {
    let mut map = map_of(&level());
    map.open();
    let markers = markers(Vec3::ZERO, 0.0, 0.0);
    let projection = map.canvas_projection(VIEWPORT).expect("projection");
    let at = projection.to_screen(map.info().expect("a level").encounters[0].position);
    for input in click(at) {
        map.update(&input, 0.0, VIEWPORT, &markers);
    }
    assert_eq!(map.selected(), Some(0));
    let boss = projection.to_screen(map.info().expect("a level").encounters[2].position);
    for input in click(boss) {
        map.update(&input, 0.0, VIEWPORT, &markers);
    }
    assert_eq!(
        map.selected(),
        Some(2),
        "another marker replaces the selection"
    );
    let layout = map.screen_layout(VIEWPORT);
    let empty = (
        layout.canvas.x + layout.canvas.width * 0.9,
        layout.canvas.y + layout.canvas.height * 0.3,
    );
    for input in click(empty) {
        map.update(&input, 0.0, VIEWPORT, &markers);
    }
    assert_eq!(map.selected(), None, "empty ground clears it");
    let press = UiInput {
        pointer: at,
        pointer_down: true,
        pointer_pressed: true,
        ..UiInput::new()
    };
    let drag = UiInput {
        pointer: (at.0 + 80.0, at.1),
        pointer_down: true,
        ..UiInput::new()
    };
    let release = UiInput {
        pointer: (at.0 + 80.0, at.1),
        pointer_released: true,
        ..UiInput::new()
    };
    for input in [press, drag, release] {
        map.update(&input, 0.0, VIEWPORT, &markers);
    }
    assert_eq!(
        map.selected(),
        None,
        "a drag that started on a marker pans, it does not select"
    );
}

#[test]
fn the_map_opens_and_closes_and_its_close_button_works() {
    let mut map = map_of(&level());
    assert_eq!(map.toggle(), MapOutcome::Opened);
    assert_eq!(map.toggle(), MapOutcome::Closed);
    assert_eq!(map.toggle(), MapOutcome::Opened);
    let close = map.screen_layout(VIEWPORT).close;
    let outcome = map.update(
        &click(middle(close))[0],
        0.0,
        VIEWPORT,
        &MapMarkers::default(),
    );
    assert_eq!(outcome, MapOutcome::Closed);
    assert!(!map.is_open());
    let mut empty = DungeonMap::new(UiScale::new(1.0));
    assert_eq!(empty.toggle(), MapOutcome::Idle, "no level, no map");
}

#[test]
fn every_control_does_what_it_says_and_does_nothing_when_it_cannot() {
    let mut map = map_of(&level());
    map.set_reduced_motion(true);
    map.open();
    let markers = markers(Vec3::new(-30.0, 0.0, -60.0), 0.0, 0.0);
    let layout = map.screen_layout(VIEWPORT);
    map.update(&click(middle(layout.zoom_out))[0], 0.0, VIEWPORT, &markers);
    assert!(
        (map.zoom() - 1.0).abs() < 1e-6,
        "zoom out is disabled at the fitted view"
    );
    map.update(&click(middle(layout.zoom_in))[0], 0.0, VIEWPORT, &markers);
    assert!(map.zoom() > 1.4);
    map.update(&click(middle(layout.zoom_out))[0], 0.0, VIEWPORT, &markers);
    assert!((map.zoom() - 1.0).abs() < 1e-3);
    map.update(&click(middle(layout.recenter))[0], 0.0, VIEWPORT, &markers);
    assert_eq!(map.center(), (-30.0, -60.0), "recentre finds the player");
    assert!(map.zoom() >= 2.5);
    map.update(&click(middle(layout.fit))[0], 0.0, VIEWPORT, &markers);
    assert!((map.zoom() - 1.0).abs() < 1e-6);
    assert_eq!(map.center(), map.info().expect("a level").frame.center());
}

#[test]
fn the_keys_zoom_frame_and_step_through_the_encounters() {
    let mut map = map_of(&level());
    map.set_reduced_motion(true);
    map.open();
    map.handle_keys(
        MapKeys {
            next: true,
            ..MapKeys::default()
        },
        VIEWPORT,
    );
    assert_eq!(map.selected(), Some(0));
    map.handle_keys(
        MapKeys {
            previous: true,
            ..MapKeys::default()
        },
        VIEWPORT,
    );
    assert_eq!(map.selected(), Some(2), "wraps round");
    map.handle_keys(
        MapKeys {
            zoom_in: true,
            ..MapKeys::default()
        },
        VIEWPORT,
    );
    assert!(map.zoom() > 1.0);
    map.handle_keys(
        MapKeys {
            fit: true,
            ..MapKeys::default()
        },
        VIEWPORT,
    );
    assert!((map.zoom() - 1.0).abs() < 1e-6);
}

#[test]
fn clicking_the_minimap_opens_the_map_and_claims_the_pointer() {
    let mut map = map_of(&level());
    let markers = markers(Vec3::ZERO, 0.0, 0.0);
    let layout = map.minimap_layout(VIEWPORT);
    assert!(map.wants_pointer(layout.center, VIEWPORT));
    assert!(!map.wants_pointer((200.0, 600.0), VIEWPORT));
    let outcome = map.update(&click(layout.center)[0], 0.0, VIEWPORT, &markers);
    assert_eq!(outcome, MapOutcome::Opened);
    assert!(map.is_open());
}

#[test]
fn the_layout_fits_every_target_resolution_and_scale() {
    for viewport in [(1920, 1080), (2560, 1440), (3840, 2160)] {
        for scale in [0.75, 1.0, 1.5, 2.0] {
            let mut map = map_of(&level());
            map.set_scale(UiScale::new(scale));
            map.open();
            let _ = map.sidebar_lines(viewport);
            let minimap = map.minimap_layout(viewport).bounds();
            assert!(
                minimap.x >= 0.0 && minimap.right() <= viewport.0 as f32,
                "{viewport:?} {scale}"
            );
            assert!(minimap.y >= 0.0 && minimap.bottom() <= viewport.1 as f32 * 0.5);
            let screen = map.screen_layout(viewport);
            assert!(
                screen.canvas.right() <= screen.sidebar.x,
                "the journal does not cover the map"
            );
            assert!(
                screen.canvas.width >= screen.sidebar.width,
                "the map stays the focus: {viewport:?} {scale}"
            );
            assert!(
                screen.panel.right() <= viewport.0 as f32
                    && screen.panel.bottom() <= viewport.1 as f32
            );
            assert!(screen.sidebar_body.height > 0.0);
            assert!(
                screen.controls.right() <= screen.canvas.right()
                    && screen.help.right() < screen.controls.x
            );
        }
    }
}

#[test]
fn the_map_screen_draws_no_bitmap_text() {
    let mut map = map_of(&level());
    map.open();
    map.select(Some(0));
    let _ = map.sidebar_lines(VIEWPORT);
    let mut list = UiDrawList::new();
    map.push_map_screen(
        &mut list,
        &markers(Vec3::ZERO, 0.0, 0.0),
        VIEWPORT,
        &MapTheme,
    );
    map.push_minimap(
        &mut list,
        &markers(Vec3::ZERO, 0.0, 0.0),
        VIEWPORT,
        &MapTheme,
    );
    assert!(
        list.commands()
            .iter()
            .any(|command| matches!(command, UiCommand::Label { .. }))
    );
    assert!(
        !list
            .commands()
            .iter()
            .any(|command| matches!(command, UiCommand::Text { .. })),
        "every word is set in the interface fonts"
    );
    assert!(
        list.commands()
            .iter()
            .any(|command| matches!(command, UiCommand::Panel { .. }))
    );
}

#[test]
fn a_pack_s_journal_lists_every_monster_and_every_ability_they_use() {
    let mut map = map_of(&level());
    map.open();
    map.select(Some(0));
    let lines = journal(&mut map);
    assert!(
        lines.contains("Gate | Monster pack \u{00B7} 3 enemies"),
        "{lines}"
    );
    assert!(
        lines.contains("CHIP 2 \u{00D7} Skeleton Warrior"),
        "{lines}"
    );
    assert!(lines.contains("Skeleton Warrior x2"), "{lines}");
    assert!(lines.contains("Crypt Archer x1"), "{lines}");
    for kind in [EnemyKind::SkeletonWarrior, EnemyKind::CryptArcher] {
        for (ability, _) in abilities_of(kind) {
            let name = title_case(ability.definition().name);
            assert!(
                lines.contains(&format!("ABILITY {name} |")),
                "{name} missing:\n{lines}"
            );
        }
    }
    assert!(
        lines.contains("COUNTERPLAY:"),
        "the design's answers are shown"
    );
}

#[test]
fn abilities_start_folded_and_open_onto_their_details() {
    let mut map = map_of(&level());
    map.open();
    map.select(Some(0));
    let folded = map.sidebar_lines(VIEWPORT).join("\n");
    assert!(
        folded.contains("[-] Skeleton Warrior"),
        "the first card starts open:\n{folded}"
    );
    assert!(
        folded.contains("[+] Crypt Archer"),
        "the rest start folded:\n{folded}"
    );
    assert!(
        folded.contains("[+] ABILITY"),
        "abilities start folded:\n{folded}"
    );
    assert!(
        !folded.contains("COUNTERPLAY:"),
        "details wait to be opened"
    );
}

#[test]
fn the_filter_counts_what_passes_it() {
    let mut map = map_of(&level());
    map.open();
    map.select(Some(0));
    let lines = journal(&mut map);
    assert!(lines.contains("FILTERS All"), "{lines}");
    let interruptible = abilities_of(EnemyKind::SkeletonWarrior)
        .into_iter()
        .chain(abilities_of(EnemyKind::CryptArcher))
        .filter(|(ability, _)| ability.definition().interruptible)
        .count();
    assert!(
        lines.contains(&format!("Interrupts {interruptible}")),
        "{lines}"
    );
    assert!(AbilityFilter::Interrupts.accepts(EnemyAbility::ExecutionStrike.definition()));
    assert!(!AbilityFilter::Interrupts.accepts(EnemyAbility::BoneSlash.definition()));
}

#[test]
fn a_boss_s_journal_lists_its_phases_its_guests_and_says_when_it_has_no_loot() {
    let mut map = map_of(&level());
    map.open();
    map.select(Some(2));
    let lines = journal(&mut map);
    assert!(
        lines.contains("Lord Ashgrave | Boss encounter \u{00B7} Hollow Steward"),
        "{lines}"
    );
    for (ability, _) in abilities_of(EnemyKind::CryptSteward) {
        let name = title_case(ability.definition().name);
        assert!(lines.contains(&format!("ABILITY {name} |")), "{lines}");
    }
    assert!(
        lines.contains("below 30% health"),
        "phases are headed: {lines}"
    );
    assert!(lines.contains("GUESTS Summoned guests"), "{lines}");
    assert!(
        lines.contains("CHIP Empowered Ghoul") && lines.contains("CHIP Empowered Acolyte"),
        "{lines}"
    );
    assert!(EnemyKind::CryptSteward.archetype().loot.is_empty());
    assert!(
        lines.contains("EMPTY No loot has been configured for this encounter."),
        "{lines}"
    );
}

#[test]
fn the_run_s_state_reaches_the_markers_and_the_journal() {
    let mut map = map_of(&level());
    map.open();
    let mut markers = markers(Vec3::ZERO, 0.0, 0.0);
    markers.statuses = vec![
        EncounterStatus::Cleared,
        EncounterStatus::Engaged,
        EncounterStatus::Waiting,
    ];
    markers.progress = Some(DungeonProgress {
        percent: 40,
        fraction: 0.4,
        unlocked: false,
        completed: false,
    });
    map.update(&UiInput::new(), 0.0, VIEWPORT, &markers);
    let overview = map.sidebar_lines(VIEWPORT).join("\n");
    assert!(overview.contains("PROGRESS"), "{overview}");
    map.select(Some(0));
    let _ = map.sidebar_lines(VIEWPORT);
    let mut list = UiDrawList::new();
    map.push_map_screen(&mut list, &markers, VIEWPORT, &MapTheme);
    let text = list.text().to_owned();
    assert!(text.contains("CLEARED"), "the cleared pack's badge: {text}");
    assert!(text.contains("40%"), "the header's completion");
    assert!(text.contains("Boss sealed until 100%"));
    map.select(Some(2));
    let _ = map.sidebar_lines(VIEWPORT);
    let mut list = UiDrawList::new();
    map.push_map_screen(&mut list, &markers, VIEWPORT, &MapTheme);
    assert!(list.text().contains("SEALED"), "a sealed boss says so");
}

#[test]
fn a_loot_row_is_the_item_table_s_own_row() {
    let ring =
        engine_game::character::item_by_key("reinforced_iron_band").expect("the ring exists");
    let line = loot_detail(&LootEntry {
        item: ring.id,
        chance: 0.25,
    })
    .expect("resolves");
    assert_eq!(line.name, "Reinforced Iron Band");
    assert_eq!(line.quality, ring.quality);
    assert!(line.meta.contains("Item level 10"), "{}", line.meta);
    assert!(line.stats.contains("+10 Stamina"), "{}", line.stats);
    assert_eq!(line.chance, "25% drop chance");
    let unknown = engine_game::character::ItemId(9999);
    assert!(
        loot_detail(&LootEntry {
            item: unknown,
            chance: 1.0
        })
        .is_none(),
        "never invented"
    );
}

#[test]
fn ability_numbers_are_read_off_the_table() {
    let spear = EnemyAbility::BoneSpear.definition();
    let timing = ability_timing(spear, 1.0);
    assert!(
        timing.contains(&format!("{} s cast", spear.cast_time.round() as i32)),
        "{timing}"
    );
    assert!(
        timing.contains(&format!("{} s cooldown", spear.cooldown.round() as i32)),
        "{timing}"
    );
    let effects = ability_effects(spear, 1.0).join(" / ");
    assert!(
        effects.contains(&format!("{} physical damage", spear.damage.round() as i32)),
        "{effects}"
    );
    assert!(effects.contains("line"), "{effects}");
    let drain = EnemyAbility::GraveDrain.definition();
    assert!(ability_timing(drain, 1.0).contains("channel"));
    assert!(ability_tags(drain).contains(&AbilityTag::Interruptible));
}

#[test]
fn moving_a_monster_moves_its_marker_and_keeps_the_selection() {
    let mut scene = level();
    let mut map = map_of(&scene);
    map.open();
    map.select(Some(2));
    scene.monsters[4].position.z += 12.0;
    scene.monsters.insert(
        0,
        MonsterPlacement::new("crypt_ghoul", Vec3::new(50.0, 0.0, -50.0)),
    );
    map.refresh_encounters(&scene);
    let boss = map.selected_encounter().expect("still selected");
    assert_eq!(boss.kind, EncounterKind::Boss);
    assert!((boss.position.z - (-128.0)).abs() < 1e-4);
    assert_eq!(map.info().expect("a level").encounters.len(), 4);
}

#[test]
fn the_real_crypt_has_a_map_whose_every_marker_describes_itself() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scenes/graveyard_crypt.json");
    let scene = scene_file::load(&path).expect("the crypt loads");
    let frame = frame_for(&scene);
    let mut map = DungeonMap::new(UiScale::new(1.0));
    map.set_info(Some(DungeonMapInfo::from_scene(&scene, frame, false)));
    let count = map.info().expect("a level").encounters.len();
    assert!(count >= 10, "{count} packs");
    let bosses = map
        .info()
        .expect("a level")
        .encounters
        .iter()
        .filter(|e| e.kind == EncounterKind::Boss)
        .count();
    assert_eq!(bosses, 1);
    for encounter in &map.info().expect("a level").encounters {
        assert!(
            frame.contains(encounter.position),
            "{} is on the map",
            encounter.key
        );
        for member in &encounter.members {
            assert!(
                member.kind.is_some(),
                "{} names a monster this build knows",
                member.id
            );
        }
    }
    map.open();
    for index in 0..count {
        map.select(Some(index));
        let lines = journal(&mut map);
        assert!(
            lines.contains("ABILITY "),
            "encounter {index} lists abilities"
        );
        let mut list = UiDrawList::new();
        map.push_map_screen(&mut list, &MapMarkers::default(), VIEWPORT, &MapTheme);
        assert!(!list.is_empty());
    }
}
