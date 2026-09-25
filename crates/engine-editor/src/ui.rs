//! What the editor looks like.
//!
//! ```text
//!  ┌ File Edit View Help │ ▶ ✥ ⟳ ⤢ │ + ▲ │ World Active │ ⊓ 1 m 22.5 ⌂ │ ↶ ↷ 💾 ─ ⌨ 📈 [Play] ┐
//!  ├───────────────┬───────────────────────────────────────────────┬───────────────────┤
//!  │ ASSETS    ≡ ▦ │ PERSPECTIVE - WORLD                GRID 1 M · SNAP │ HIERARCHY     29  │
//!  │ [search     ] │                                                │ [search         ] │
//!  │ ▾ ■ Built-in  │                                                │ ▾ ■ Rocks       4 │
//!  │   ■ Pillar    │                  the level                     │     ■ Rock Small  │
//!  │ ▸ ■ Generated │                                                ├───────────────────┤
//!  │               │                                                │ INSPECTOR         │
//!  │               │ ROCK SMALL                                  ✛  │ ▾ Transform       │
//!  ├───────────────┴───────────────────────────────────────────────┴───────────────────┤
//!  │ ✥ Move │ 1 selected │ Snap 1 m │ hint...                 X 12.0 Z -4.0 │ level.json ● │
//!  └────────────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Everything is drawn from the models in [`crate::panels`] with the primitives in
//! [`crate::widgets`], in the palette in [`crate::theme`]. The level is drawn first and the
//! panels over it, so nothing the editor draws in the world can spill onto a panel.
//!
//! It paints into the same [`UiDrawList`] the HUD and the menus use, so the whole interface
//! is still one batch and one draw call — see `engine_presentation::ui::paint`.

use std::fmt::Write as _;

use engine_core::math::{Quat, Vec3};
use engine_environment::EnvironmentScene;
use engine_presentation::ui::paint::MenuTheme;
use engine_render_data::ui::{UiColor, UiDrawList, UiRect};

use crate::browser::BrowserRow;
use crate::commands::BrowserView;
use crate::icons::Icon;
use crate::layout::{Layout, Splitter};
use crate::panels::{self, DialogKind, Element, HierarchyRow, Tile, ToolKind};
use crate::theme::{self, metrics};
use crate::widgets::{Paint, State};
use crate::{Editor, Modal, Selected, inspector, pick};

/// Draws the editor.
///
/// Nothing is drawn when the editor is closed, so the application can call this
/// unconditionally and a shipping build simply never has it to call. `_theme` is the game's
/// menu theme, which the editor no longer borrows: it has its own, in [`crate::theme`].
pub fn push_editor(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    _theme: &MenuTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    if !editor.is_active() {
        return;
    }
    let layout = editor.layout_for(viewport);

    // The world first, so the panels are painted over it: an outline that ran across the
    // browser would be a selection marker floating on a list of file names.
    push_world_overlay(list, editor, scene, scratch, &layout);
    if editor.shows_map_preview() {
        push_map_preview(list, editor, scene, &layout);
    }

    let mut paint = Paint::new(list, layout.scale);
    push_viewport_chips(&mut paint, editor, scene, &layout);
    if editor.shows_debug() {
        push_debug(&mut paint, editor, scene, &layout);
    }

    push_toolbar(&mut paint, editor, &layout);
    push_browser(&mut paint, editor, &layout);
    push_hierarchy(&mut paint, editor, scene, &layout);
    push_inspector(&mut paint, editor, scene, &layout);
    push_status(&mut paint, editor, scene, &layout);
    push_splitters(&mut paint, editor, &layout);

    // Everything that floats: the asset being carried, a menu, a dialog, a tooltip.
    push_carried(&mut paint, editor);
    push_menu(&mut paint, editor, scene, &layout);
    push_dialogs(&mut paint, editor, &layout);
    if let Some((title, key)) = editor.tooltip()
        && editor.open_menu().is_none()
        && !editor.is_dragging()
    {
        paint.tooltip(editor.pointer(), title, key, layout.window);
    }
}

// ---------------------------------------------------------------------------------------
// The map preview
// ---------------------------------------------------------------------------------------

/// The dungeon map's preview, in the viewport's top-right corner: the baked picture with every
/// encounter drawn where its monsters stand *now*.
///
/// The markers are derived from the scene on every frame, exactly as the game's map derives
/// them, so a pack dragged across the room moves on the preview while it is dragged; only the
/// picture waits for a bake, and the preview says when it is out of date.
fn push_map_preview(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    layout: &Layout,
) {
    use engine_environment::map::{EncounterKind, encounters, frame_for};
    use engine_render_data::ui::{TextAlign, UiIcon};

    let scale = layout.scale;
    let viewport = layout.viewport;
    let picture = editor.map_picture();
    let frame = picture.map_or_else(|| frame_for(scene), |picture| picture.frame);
    if !frame.is_valid() {
        return;
    }
    let pad = scale.px(8.0);
    let header = scale.px(20.0);
    let max_width = (viewport.width * 0.32).max(scale.px(160.0));
    let max_height = (viewport.height * 0.6).max(scale.px(160.0));
    let ppm = (max_width / frame.width()).min(max_height / frame.depth());
    let (map_width, map_height) = (frame.width() * ppm, frame.depth() * ppm);
    let panel = UiRect::new(
        viewport.right() - map_width - pad * 3.0,
        // Below the viewport's chips, which sit along its top edge.
        viewport.y + scale.px(34.0),
        map_width + pad * 2.0,
        map_height + pad * 2.0 + header,
    );
    list.rect(panel, [0.07, 0.07, 0.09, 0.94]);
    list.outline(panel, scale.px(1.0), theme::BORDER);
    let title = match picture {
        Some(picture) if picture.stale => "MAP - STALE, BAKE AGAIN".to_owned(),
        Some(picture) => format!("MAP - {} X {}", picture.width, picture.height),
        None => "MAP - NOT BAKED".to_owned(),
    };
    let stale = picture.is_none_or(|picture| picture.stale);
    list.text_at(
        (panel.x + pad, panel.y + scale.px(6.0)),
        &title,
        if stale {
            [1.0, 0.7, 0.35, 1.0]
        } else {
            theme::TEXT
        },
        scale.text(1.0),
    );
    list.text_aligned(
        (panel.right() - pad, panel.y + scale.px(6.0)),
        "F4",
        theme::TEXT_FAINT,
        scale.text(1.0),
        TextAlign::Right,
    );
    let area = UiRect::new(panel.x + pad, panel.y + header + pad, map_width, map_height);
    list.rect(area, [0.12, 0.10, 0.08, 1.0]);
    if picture.is_some() {
        list.map_image(
            area,
            (0.0, 0.0),
            (1.0 / map_width, 0.0),
            (0.0, 1.0 / map_height),
            [1.0; 4],
        );
    }
    let to_screen = |position: engine_core::math::Vec3| {
        let (u, v) = frame.world_to_uv(position);
        (area.x + u * area.width, area.y + v * area.height)
    };
    // The selected monsters' packs are ringed.
    let selected_packs: Vec<String> = editor
        .selected()
        .iter()
        .filter_map(|what| what.monster())
        .filter_map(|index| {
            scene
                .monsters
                .get(index)
                .map(|monster| monster.pack_key(index))
        })
        .collect();
    for encounter in encounters(scene) {
        let at = to_screen(encounter.position);
        let size = match encounter.kind {
            EncounterKind::Boss => scale.px(18.0),
            EncounterKind::Pack => scale.px(13.0),
        };
        if selected_packs.contains(&encounter.key) {
            list.disc(at, size * 0.8, [1.0, 0.95, 0.6, 1.0]);
        }
        list.icon(
            UiRect::new(at.0 - size * 0.5, at.1 - size * 0.5, size, size),
            match encounter.kind {
                EncounterKind::Boss => UiIcon::MapBoss,
                EncounterKind::Pack => UiIcon::MapPack,
            },
            [1.0; 4],
        );
    }
    // Where the player starts.
    let spawn = to_screen(scene.player_spawn().position);
    list.disc(spawn, scale.px(4.0), [0.3, 0.7, 1.0, 1.0]);
}

// ---------------------------------------------------------------------------------------
// Toolbar and status bar
// ---------------------------------------------------------------------------------------

/// The toolbar: menus, tools, modes, snapping, history, and the way back to the game.
fn push_toolbar(paint: &mut Paint<'_>, editor: &Editor, layout: &Layout) {
    let bar = layout.toolbar;
    paint.fill(bar, theme::CHROME);
    paint.fill(
        UiRect::new(
            bar.x,
            bar.bottom() - paint.px(1.0),
            bar.width,
            paint.px(1.0),
        ),
        theme::BORDER,
    );
    let pointer = editor.pointer();
    let open = editor.open_menu().map(|menu| menu.kind);
    let menu_bar_open = matches!(
        open,
        Some(
            panels::MenuKind::File
                | panels::MenuKind::Edit
                | panels::MenuKind::View
                | panels::MenuKind::Help
        )
    );
    for item in panels::toolbar(editor, layout) {
        let hover = item.rect.contains(pointer.0, pointer.1) && editor.modal().is_none();
        match &item.kind {
            ToolKind::Separator => paint.fill(item.rect, theme::DIVIDER),
            ToolKind::Menu(kind, name) => {
                let state = State {
                    active: false,
                    hover: hover || (menu_bar_open && open == Some(*kind)),
                    ..State::default()
                };
                if open == Some(*kind) {
                    paint.fill(item.rect, theme::RAISED);
                }
                paint.state_fill(item.rect, state);
                paint.centred(
                    item.rect,
                    name,
                    if open == Some(*kind) || hover {
                        theme::TEXT_STRONG
                    } else {
                        theme::TEXT
                    },
                );
            }
            ToolKind::Button {
                icon,
                active,
                disabled,
                command,
            } => {
                paint.button(
                    item.rect,
                    *icon,
                    State {
                        active: *active,
                        hover,
                        disabled: *disabled,
                        pressed: false,
                    },
                );
                // Unsaved work shows on the save button itself, as a dot in the corner.
                if matches!(command, crate::commands::Command::Save) && editor.is_dirty() {
                    let dot = paint.px(5.0);
                    paint.fill(
                        UiRect::new(
                            item.rect.right() - dot - paint.px(3.0),
                            item.rect.y + paint.px(3.0),
                            dot,
                            dot,
                        ),
                        theme::WARNING,
                    );
                }
            }
            ToolKind::Toggle {
                icon, text, active, ..
            } => {
                paint.text_button(
                    item.rect,
                    Some(*icon),
                    text,
                    State {
                        active: *active,
                        hover,
                        ..State::default()
                    },
                );
            }
            ToolKind::Dropdown {
                icon,
                text,
                active,
                menu,
            } => {
                if open == Some(*menu) {
                    paint.fill(item.rect, theme::RAISED);
                }
                let state = State {
                    hover,
                    disabled: !active,
                    ..State::default()
                };
                paint.text_button(item.rect, Some(*icon), text, state);
                let after = item.rect.x
                    + paint.px(4.0 + metrics::ICON_SLOT + 3.0)
                    + paint.ty.width(text)
                    + paint.px(4.0);
                let chevron = UiRect::new(after, item.rect.y, paint.px(10.0), item.rect.height);
                paint.icon(Icon::ChevronDown, chevron, theme::TEXT_FAINT);
            }
            ToolKind::Play => {
                paint.fill(
                    item.rect,
                    if hover {
                        [0.35, 0.66, 1.0, 1.0]
                    } else {
                        theme::ACCENT
                    },
                );
                let slot = paint.icon_slot(item.rect, item.rect.x + paint.px(6.0));
                paint.icon(Icon::Play, slot, theme::TEXT_STRONG);
                paint.label(
                    item.rect,
                    slot.right() + paint.px(3.0),
                    item.rect.right(),
                    "Play",
                    theme::TEXT_STRONG,
                );
            }
        }
    }
}

/// The status bar: the tool, the selection, snapping and a hint on the left; where the
/// pointer is and whether the level is saved on the right.
fn push_status(paint: &mut Paint<'_>, editor: &Editor, scene: &EnvironmentScene, layout: &Layout) {
    let bar = layout.status;
    paint.fill(bar, theme::CHROME);
    paint.fill(
        UiRect::new(bar.x, bar.y, bar.width, paint.px(1.0)),
        theme::BORDER,
    );
    let pad = paint.px(8.0);
    let gap = paint.px(14.0);
    let mut x = bar.x + pad;

    // The mode, first and in the accent colour when it is not plain selection.
    let (icon, mode, accent) = if editor.is_sculpting() {
        (
            Icon::Sculpt,
            format!("Sculpt: {}", editor.brush().as_str()),
            true,
        )
    } else if editor.is_place_mode() {
        (
            Icon::Place,
            format!(
                "Placing {}",
                theme::display_name(editor.browser().selected_name())
            ),
            true,
        )
    } else {
        let tool = editor.tool();
        (
            match tool {
                crate::commands::Tool::Select => Icon::Select,
                crate::commands::Tool::Move => Icon::Move,
                crate::commands::Tool::Turn => Icon::Rotate,
                crate::commands::Tool::Scale => Icon::Scale,
            },
            tool.label().to_owned(),
            false,
        )
    };
    let tint = if accent {
        theme::ACCENT
    } else {
        theme::TEXT_DIM
    };
    let slot = paint.icon_slot(bar, x);
    paint.icon(icon, slot, tint);
    x = slot.right() + paint.px(3.0);
    paint.label(
        bar,
        x,
        bar.right(),
        &mode,
        if accent {
            theme::TEXT_STRONG
        } else {
            theme::TEXT
        },
    );
    x += paint.ty.width(&mode) + gap;

    let separator = |paint: &mut Paint<'_>, x: &mut f32| {
        paint.divider_v(
            *x - gap * 0.5,
            bar.y + paint.px(5.0),
            bar.bottom() - paint.px(5.0),
        );
    };
    separator(paint, &mut x);
    let selected = match editor.selected().len() {
        0 => "Nothing selected".to_owned(),
        1 => editor
            .selection()
            .map_or_else(String::new, |what| panels::item_name(scene, what)),
        count => format!("{count} selected"),
    };
    paint.label(bar, x, bar.right(), &selected, theme::TEXT);
    x += paint.ty.width(&selected) + gap;
    separator(paint, &mut x);

    let snap = if editor.is_module_snap() {
        "Module 4 m".to_owned()
    } else if editor.is_snapping() {
        format!("Snap {}", panels::grid_label(editor.grid()))
    } else {
        "Snap off".to_owned()
    };
    paint.label(bar, x, bar.right(), &snap, theme::TEXT_DIM);
    x += paint.ty.width(&snap) + gap;
    separator(paint, &mut x);

    // The right-hand end, measured first so the hint on the left stops short of it.
    let file = editor
        .save_path()
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let (saved, saved_color) = if editor.is_dirty() {
        ("Unsaved", theme::WARNING)
    } else {
        ("Saved", theme::TEXT_FAINT)
    };
    let mut right = bar.right() - pad;
    right -= paint.ty.width(saved);
    paint.label(bar, right, bar.right(), saved, saved_color);
    let dot = paint.px(6.0);
    right -= dot + paint.px(5.0);
    paint.fill(
        UiRect::new(right, bar.y + (bar.height - dot) * 0.5, dot, dot),
        saved_color,
    );
    right -= gap * 0.5 + paint.ty.width(&file);
    paint.label(bar, right, bar.right(), &file, theme::TEXT);
    right -= gap;
    paint.divider_v(
        right + gap * 0.5,
        bar.y + paint.px(5.0),
        bar.bottom() - paint.px(5.0),
    );
    if let Some(at) = editor.cursor_world() {
        let mut where_ = String::with_capacity(32);
        // `+ 0.0` turns a negative zero into a zero: `-0.0` under the pointer reads as a bug.
        let _ = write!(
            where_,
            "X {:.1}  Y {:.1}  Z {:.1}",
            at.x + 0.0,
            at.y + 0.0,
            at.z + 0.0
        );
        right -= paint.ty.width(&where_);
        paint.label(bar, right, bar.right(), &where_, theme::TEXT_DIM);
        right -= gap;
    }

    // What just happened, if it was recent; otherwise what the author can do next.
    match editor
        .recent_status()
        .filter(|_| editor.gizmo_axis().is_none())
    {
        Some(status) => paint.label(bar, x, right, status, theme::TEXT),
        None => paint.label(bar, x, right, panels::hint(editor), theme::TEXT_FAINT),
    }
}

/// The splitters, drawn as a thin accent line while they are hovered or dragged.
fn push_splitters(paint: &mut Paint<'_>, editor: &Editor, layout: &Layout) {
    let Some(which) = editor.splitter_hover() else {
        return;
    };
    if editor.is_dragging() || editor.open_menu().is_some() || editor.modal().is_some() {
        return;
    }
    let zone = layout.splitter(which);
    let line = match which {
        Splitter::Split => UiRect::new(
            zone.x,
            zone.y + zone.height * 0.5 - paint.px(1.0),
            zone.width,
            paint.px(2.0),
        ),
        _ => UiRect::new(
            zone.x + zone.width * 0.5 - paint.px(1.0),
            zone.y,
            paint.px(2.0),
            zone.height,
        ),
    };
    paint.fill(line, theme::ACCENT);
}

// ---------------------------------------------------------------------------------------
// Content browser
// ---------------------------------------------------------------------------------------

/// The content browser: header, search, rows or tiles, and a footer.
fn push_browser(paint: &mut Paint<'_>, editor: &Editor, layout: &Layout) {
    paint.panel(layout.left);
    let counts = editor.browser().catalogue().counts();
    let mut detail = String::new();
    let _ = write!(
        detail,
        "{}",
        counts[0] + engine_environment::asset::LIBRARY.len()
    );
    paint.header(layout.browser_header, "Assets", Some(&detail));
    let pointer = editor.pointer();
    for (rect, view, icon, _) in panels::browser_view_buttons(layout) {
        paint.button(
            rect,
            icon,
            State {
                active: editor.browser_view() == view,
                hover: rect.contains(pointer.0, pointer.1),
                ..State::default()
            },
        );
    }

    let search = layout.browser_search;
    let filter = editor.browser().filter();
    paint.field(
        search,
        Some(Icon::Search),
        filter,
        "Search assets",
        editor.browser().is_typing(),
        search.contains(pointer.0, pointer.1),
    );
    if !filter.is_empty() {
        paint.icon(Icon::Close, panels::clear_button(search), theme::TEXT_DIM);
    }

    match editor.browser_view() {
        BrowserView::List => push_browser_list(paint, editor, layout),
        BrowserView::Tiles => push_browser_tiles(paint, editor, layout),
    }

    // The footer: how much there is, and a warning if anything could not be read.
    let footer = layout.browser_footer;
    paint.divider_h(footer.x, footer.right(), footer.y);
    let unreadable = editor.browser().catalogue().report().unreadable.len();
    let x = footer.x + paint.px(metrics::PADDING + 2.0);
    if unreadable > 0 {
        let slot = paint.icon_slot(footer, x);
        paint.icon(Icon::Warning, slot, theme::WARNING);
        let text = format!("{unreadable} file(s) could not be read");
        paint.label(
            footer,
            slot.right() + paint.px(3.0),
            footer.right(),
            &text,
            theme::WARNING,
        );
    } else {
        let text = format!(
            "{} models - {} materials - {} levels",
            counts[0] + engine_environment::asset::LIBRARY.len(),
            counts[1],
            counts[3]
        );
        paint.label(footer, x, footer.right(), &text, theme::TEXT_FAINT);
    }
}

/// One asset or folder row: disclosure, icon, name, and a kind word for what is not a model.
fn push_asset_row(
    paint: &mut Paint<'_>,
    editor: &Editor,
    row: BrowserRow,
    rect: UiRect,
    hover: bool,
) {
    let browser = editor.browser();
    let selected = BrowserRowSelected::of(editor, row);
    if selected {
        paint.fill(rect, theme::ACCENT_FILL);
        paint.fill(
            UiRect::new(rect.x, rect.y, paint.px(2.0), rect.height),
            theme::ACCENT,
        );
    } else if hover {
        paint.fill(rect, theme::HOVER);
    }
    let depth = browser.depth(row) as f32;
    let mut x = rect.x + paint.px(metrics::PADDING) + paint.px(metrics::INDENT) * depth;
    let folder = matches!(
        row,
        BrowserRow::Folder(_) | BrowserRow::BuiltinFolder | BrowserRow::MonsterFolder
    );
    let empty = match row {
        BrowserRow::Folder(index) => browser
            .catalogue()
            .folders()
            .get(index)
            .is_none_or(|folder| folder.children.is_empty() && folder.assets.is_empty()),
        _ => false,
    };
    let chevron = paint.px(10.0);
    if folder && !empty {
        let slot = UiRect::new(x, rect.y, chevron, rect.height);
        paint.icon(
            if browser.is_expanded(row) {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            },
            slot,
            theme::TEXT_DIM,
        );
    }
    x += chevron + paint.px(2.0);
    let (icon, tint) = panels::row_icon(editor, row);
    let usable =
        (folder && !empty) || browser.is_placeable(row) || browser.scene_path(row).is_some();
    let slot = paint.icon_slot(rect, x);
    paint.icon(icon, slot, if usable { tint } else { theme::TEXT_FAINT });
    x = slot.right() + paint.px(3.0);
    let kind = panels::row_kind(editor, row);
    let right = rect.right()
        - paint.px(6.0)
        - if kind.is_empty() {
            0.0
        } else {
            paint.ty.width(kind) + paint.px(8.0)
        };
    let name = browser.name(row);
    paint.label(
        rect,
        x,
        right,
        &name,
        if selected {
            theme::TEXT_STRONG
        } else if usable {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );
    if !kind.is_empty() {
        paint.right(rect, rect.right() - paint.px(6.0), kind, theme::TEXT_FAINT);
    }
}

/// Whether a browser row is the asset the next placement uses.
struct BrowserRowSelected;

impl BrowserRowSelected {
    fn of(editor: &Editor, row: BrowserRow) -> bool {
        editor.browser().is_selected(row)
    }
}

/// The browser as a tree.
fn push_browser_list(paint: &mut Paint<'_>, editor: &Editor, layout: &Layout) {
    let browser = editor.browser();
    let pointer = editor.pointer();
    let visible = layout.browser_rows();
    let first = browser.scroll();
    if browser.rows().is_empty() || (browser.rows().len() == 1 && !browser.filter().is_empty()) {
        push_empty(
            paint,
            layout.browser_list,
            if browser.filter().is_empty() {
                "No assets yet"
            } else {
                "No assets match"
            },
            if browser.filter().is_empty() {
                "Put .glb files under assets/ and press F2"
            } else {
                "Try a shorter search"
            },
        );
        if browser.rows().is_empty() {
            return;
        }
    }
    for slot in 0..visible {
        let Some(row) = browser.row(first + slot) else {
            break;
        };
        let rect = layout.browser_row(slot);
        push_asset_row(
            paint,
            editor,
            row,
            rect,
            rect.contains(pointer.0, pointer.1),
        );
    }
    push_scrollbar(
        paint,
        layout.browser_list,
        first,
        visible,
        browser.rows().len(),
    );
}

/// The browser as tiles: a breadcrumb, then the open folder's contents.
fn push_browser_tiles(paint: &mut Paint<'_>, editor: &Editor, layout: &Layout) {
    let pointer = editor.pointer();
    let crumbs = layout.crumbs();
    let mut x = crumbs.x + paint.px(metrics::PADDING);
    let path = panels::crumbs(editor);
    for (index, (name, _)) in path.iter().enumerate() {
        let last = index + 1 == path.len();
        let width = paint.ty.width(name) + paint.px(14.0);
        let rect = UiRect::new(x, crumbs.y, width, crumbs.height);
        if rect.contains(pointer.0, pointer.1) && !last {
            paint.fill(rect, theme::HOVER);
        }
        paint.label(
            rect,
            x + paint.px(2.0),
            rect.right(),
            name,
            if last {
                theme::TEXT_STRONG
            } else {
                theme::TEXT_DIM
            },
        );
        if !last {
            let chevron = UiRect::new(
                rect.right() - paint.px(10.0),
                rect.y,
                paint.px(9.0),
                rect.height,
            );
            paint.icon(Icon::ChevronRight, chevron, theme::TEXT_FAINT);
        }
        x += width;
    }
    paint.divider_h(crumbs.x, crumbs.right(), crumbs.bottom());

    let tiles = panels::tiles(editor);
    if tiles.is_empty() {
        push_empty(
            paint,
            layout.tile_area(),
            if editor.browser().filter().is_empty() {
                "This folder is empty"
            } else {
                "No assets match"
            },
            if editor.browser().filter().is_empty() {
                "Drop files into it and press F2"
            } else {
                "Try a shorter search"
            },
        );
        return;
    }
    let (columns, rows) = layout.tile_grid();
    let first = editor.tile_scroll() * columns;
    for slot in 0..columns * rows {
        let Some(tile) = tiles.get(first + slot) else {
            break;
        };
        let rect = layout.tile(slot).inset(paint.px(2.0));
        let hover = rect.contains(pointer.0, pointer.1);
        let (icon, tint, name, selected, usable) = match tile {
            Tile::Open(folder) => {
                let name = match folder {
                    panels::TileFolder::Root => "Assets".to_owned(),
                    panels::TileFolder::Builtin => "Built-in".to_owned(),
                    panels::TileFolder::Monsters => "Monsters".to_owned(),
                    panels::TileFolder::Folder(index) => editor
                        .browser()
                        .catalogue()
                        .folders()
                        .get(*index)
                        .map_or_else(String::new, |folder| theme::display_name(&folder.name)),
                };
                (Icon::Folder, theme::kind::FOLDER, name, false, true)
            }
            Tile::Asset(row) => {
                let (icon, tint) = panels::row_icon(editor, *row);
                let browser = editor.browser();
                let usable = browser.is_placeable(*row) || browser.scene_path(*row).is_some();
                (
                    icon,
                    tint,
                    browser.name(*row),
                    browser.is_selected(*row),
                    usable,
                )
            }
        };
        paint.fill(
            rect,
            if selected {
                theme::ACCENT_FILL
            } else {
                theme::FIELD
            },
        );
        if hover {
            paint.fill(rect, theme::HOVER);
        }
        paint.frame(
            rect,
            if selected {
                theme::ACCENT
            } else {
                theme::BORDER
            },
        );
        // The picture at three times its row size, in the middle of the tile's upper part.
        let picture = UiRect::new(
            rect.x,
            rect.y + paint.px(4.0),
            rect.width,
            rect.height - paint.px(20.0),
        );
        let cell = paint.ty.cell * 3.0;
        crate::icons::draw_in(
            paint.list,
            icon,
            picture,
            cell,
            if usable { tint } else { theme::TEXT_FAINT },
        );
        let label = UiRect::new(
            rect.x + paint.px(3.0),
            rect.bottom() - paint.px(16.0),
            rect.width - paint.px(6.0),
            paint.px(14.0),
        );
        paint.centred(
            label,
            &name,
            if selected {
                theme::TEXT_STRONG
            } else {
                theme::TEXT
            },
        );
    }
    let total = tiles.len().div_ceil(columns);
    push_scrollbar(paint, layout.tile_area(), editor.tile_scroll(), rows, total);
}

/// A thin scroll indicator at the right edge of a list, when there is more list than room.
fn push_scrollbar(paint: &mut Paint<'_>, area: UiRect, first: usize, visible: usize, total: usize) {
    if total <= visible || total == 0 {
        return;
    }
    let track = UiRect::new(
        area.right() - paint.px(4.0),
        area.y,
        paint.px(3.0),
        area.height,
    );
    let height = (track.height * visible as f32 / total as f32).max(paint.px(12.0));
    let top = track.y + (track.height - height) * first as f32 / (total - visible) as f32;
    paint.fill(
        UiRect::new(track.x, top, track.width, height),
        theme::DIVIDER,
    );
}

/// A panel's empty state: a short title and what to do about it, wrapped to the panel.
fn push_empty(paint: &mut Paint<'_>, area: UiRect, title: &str, text: &str) {
    let line = paint.ty.height() + paint.px(6.0);
    let mut top = area.y + paint.px(24.0);
    paint.centred(
        UiRect::new(area.x, top, area.width, line),
        title,
        theme::TEXT_DIM,
    );
    top += line + paint.px(2.0);
    let room = paint.ty.fits(area.width - paint.px(24.0)).max(8);
    for part in wrap(text, room) {
        paint.centred(
            UiRect::new(area.x, top, area.width, line),
            &part,
            theme::TEXT_FAINT,
        );
        top += line - paint.px(2.0);
    }
}

/// `text` broken into lines of at most `width` characters, at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

// ---------------------------------------------------------------------------------------
// Hierarchy
// ---------------------------------------------------------------------------------------

/// The hierarchy: header, search, and the level's things grouped by kind.
fn push_hierarchy(
    paint: &mut Paint<'_>,
    editor: &Editor,
    scene: &EnvironmentScene,
    layout: &Layout,
) {
    paint.panel(layout.hierarchy);
    let count = crate::hierarchy_len(scene);
    let mut detail = String::new();
    let _ = write!(detail, "{count}");
    paint.header(layout.hierarchy_header, "Hierarchy", Some(&detail));
    let pointer = editor.pointer();

    let search = layout.hierarchy_search;
    paint.field(
        search,
        Some(Icon::Search),
        editor.hierarchy_filter(),
        "Search the level",
        editor.is_filtering_hierarchy(),
        search.contains(pointer.0, pointer.1),
    );
    if !editor.hierarchy_filter().is_empty() {
        paint.icon(Icon::Close, panels::clear_button(search), theme::TEXT_DIM);
    }

    let view = editor.hierarchy_view(scene);
    if view.is_empty() {
        if count == 0 {
            push_empty(
                paint,
                layout.hierarchy_list,
                "The level is empty",
                "Drag an asset into the viewport",
            );
        } else {
            push_empty(
                paint,
                layout.hierarchy_list,
                "Nothing matches",
                "Try a shorter search",
            );
        }
        return;
    }
    let visible = layout.hierarchy_rows();
    let first = editor.hierarchy_scroll().min(view.len());
    let primary = editor.selection();
    for slot in 0..visible {
        let Some(row) = view.get(first + slot).copied() else {
            break;
        };
        let rect = layout.hierarchy_row(slot);
        let hover = rect.contains(pointer.0, pointer.1) && editor.open_menu().is_none();
        let (eye, lock) = panels::row_toggles(layout, rect);
        match row {
            HierarchyRow::Group(group, count) => {
                if hover {
                    paint.fill(rect, theme::HOVER);
                }
                let mut x = rect.x + paint.px(metrics::PADDING);
                let collapsed =
                    editor.is_group_collapsed(group) && editor.hierarchy_filter().trim().is_empty();
                paint.icon(
                    if collapsed {
                        Icon::ChevronRight
                    } else {
                        Icon::ChevronDown
                    },
                    UiRect::new(x, rect.y, paint.px(10.0), rect.height),
                    theme::TEXT_DIM,
                );
                x += paint.px(12.0);
                let slot_rect = paint.icon_slot(rect, x);
                paint.icon(group.icon(), slot_rect, group.color());
                x = slot_rect.right() + paint.px(3.0);
                paint.label(rect, x, eye.x, group.label(), theme::TEXT_DIM);
                let mut number = String::new();
                let _ = write!(number, "{count}");
                paint.label(
                    rect,
                    x + paint.ty.width(group.label()) + paint.px(6.0),
                    eye.x,
                    &number,
                    theme::TEXT_FAINT,
                );
                let objects: Vec<usize> = (0..scene.objects.len())
                    .filter(|index| panels::Group::of(scene, Selected::Object(*index)) == group)
                    .collect();
                if !objects.is_empty()
                    && (hover || objects.iter().all(|index| editor.is_hidden(*index)))
                {
                    let all_hidden = objects.iter().all(|index| editor.is_hidden(*index));
                    paint.icon(
                        if all_hidden { Icon::EyeOff } else { Icon::Eye },
                        eye,
                        if eye.contains(pointer.0, pointer.1) {
                            theme::TEXT
                        } else {
                            theme::TEXT_FAINT
                        },
                    );
                }
            }
            HierarchyRow::Item(what) => {
                let selected = editor.is_selected(what);
                if selected {
                    paint.fill(
                        rect,
                        if primary == Some(what) {
                            theme::ACCENT_STRONG
                        } else {
                            theme::ACCENT_FILL
                        },
                    );
                } else if hover || editor.hovered() == Some(what) {
                    paint.fill(rect, theme::HOVER);
                }
                let (hidden, locked) = match what {
                    Selected::Object(index) => (editor.is_hidden(index), editor.is_locked(index)),
                    _ => (false, false),
                };
                let x = rect.x + paint.px(metrics::PADDING + 12.0 + metrics::INDENT);
                let (icon, tint) = panels::item_icon(scene, what);
                let slot_rect = paint.icon_slot(rect, x);
                paint.icon(
                    icon,
                    slot_rect,
                    if hidden { theme::TEXT_FAINT } else { tint },
                );
                let name = panels::item_name(scene, what);
                paint.label(
                    rect,
                    slot_rect.right() + paint.px(3.0),
                    lock.x - paint.px(2.0),
                    &name,
                    if hidden {
                        theme::TEXT_FAINT
                    } else if selected {
                        theme::TEXT_STRONG
                    } else {
                        theme::TEXT
                    },
                );
                if let Selected::Object(_) = what {
                    // The toggles show while they are set, or while the row is under the
                    // pointer: a column of forty eyes says nothing an author needs.
                    if hover || hidden {
                        paint.icon(
                            if hidden { Icon::EyeOff } else { Icon::Eye },
                            eye,
                            if eye.contains(pointer.0, pointer.1) {
                                theme::TEXT
                            } else {
                                theme::TEXT_DIM
                            },
                        );
                    }
                    if hover || locked {
                        paint.icon(
                            if locked { Icon::Lock } else { Icon::Unlock },
                            lock,
                            if locked {
                                theme::WARNING
                            } else if lock.contains(pointer.0, pointer.1) {
                                theme::TEXT
                            } else {
                                theme::TEXT_DIM
                            },
                        );
                    }
                }
            }
        }
    }
    push_scrollbar(paint, layout.hierarchy_list, first, visible, view.len());
}

// ---------------------------------------------------------------------------------------
// Inspector
// ---------------------------------------------------------------------------------------

/// The inspector: what is selected, in sections.
fn push_inspector(
    paint: &mut Paint<'_>,
    editor: &Editor,
    scene: &EnvironmentScene,
    layout: &Layout,
) {
    paint.panel(layout.inspector);
    paint.fill(
        UiRect::new(
            layout.inspector.x,
            layout.inspector.y,
            layout.inspector.width,
            paint.px(1.0),
        ),
        theme::BORDER,
    );
    paint.header(layout.inspector_header, "Inspector", None);
    let pointer = editor.pointer();

    if editor.is_sculpting() {
        push_brush(paint, editor, layout);
        return;
    }

    let editing = editor.inspector_edit();
    let primary = editor.selection();
    for element in panels::inspector(editor, scene, layout) {
        let hover = element.rect().contains(pointer.0, pointer.1) && editor.open_menu().is_none();
        match element {
            Element::Empty { rect } => {
                let area = UiRect::new(rect.x, rect.y + paint.px(20.0), rect.width, rect.height);
                paint.icon(
                    Icon::Select,
                    UiRect::new(area.x, area.y, area.width, paint.px(20.0)),
                    theme::TEXT_FAINT,
                );
                push_empty(
                    paint,
                    UiRect::new(area.x, area.y + paint.px(4.0), area.width, area.height),
                    "No selection",
                    "Select something in the viewport or the hierarchy",
                );
            }
            Element::Title {
                rect,
                icon,
                tint,
                name,
                detail,
            } => {
                let slot = paint.icon_slot(rect, rect.x);
                paint.icon(icon, slot, tint);
                let x = slot.right() + paint.px(4.0);
                let right = rect.right() - paint.ty.width(&detail) - paint.px(8.0);
                paint.label(rect, x, right, &name, theme::TEXT_STRONG);
                paint.right(rect, rect.right(), &detail, theme::TEXT_DIM);
            }
            Element::Section {
                rect,
                section,
                open,
            } => {
                paint.fill(rect, theme::PANEL_HEADER);
                if hover {
                    paint.fill(rect, theme::HOVER);
                }
                let x = rect.x + paint.px(metrics::PADDING);
                paint.icon(
                    if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    UiRect::new(x, rect.y, paint.px(10.0), rect.height),
                    theme::TEXT_DIM,
                );
                paint.label(
                    rect,
                    x + paint.px(13.0),
                    rect.right(),
                    section.label(),
                    theme::TEXT,
                );
            }
            Element::Label { rect, text } => {
                paint.label(rect, rect.x, rect.right(), text, theme::TEXT_DIM);
            }
            Element::Tag {
                rect,
                letter,
                color,
                ..
            } => {
                paint.fill(
                    rect,
                    if hover {
                        [color[0], color[1], color[2], 0.35]
                    } else {
                        [color[0], color[1], color[2], 0.18]
                    },
                );
                paint.centred(rect, letter, color);
            }
            Element::Value { rect, property } => {
                let active = editing.filter(|edit| edit.property == property);
                if let Some(edit) = active {
                    paint.field(rect, None, edit.field.text(), "", true, false);
                } else {
                    paint.fill(rect, theme::FIELD);
                    paint.frame(rect, if hover { theme::DIVIDER } else { theme::BORDER });
                    let value = primary
                        .and_then(|what| inspector::get(scene, editor.monsters(), what, property))
                        .unwrap_or(0.0);
                    let decimals = match property {
                        inspector::Property::ScaleX
                        | inspector::Property::ScaleY
                        | inspector::Property::ScaleZ
                        | inspector::Property::Intensity
                        | inspector::Property::Red
                        | inspector::Property::Green
                        | inspector::Property::Blue => 2,
                        _ => 1,
                    };
                    let mut text = String::with_capacity(12);
                    let _ = write!(text, "{value:.decimals$}");
                    // Numbers line up on the right, the way every spreadsheet and inspector
                    // writes them, so a column of positions reads as a column.
                    paint.right(
                        rect,
                        rect.right() - paint.px(4.0),
                        &paint.ty.fit(&text, rect.width - paint.px(6.0)),
                        theme::TEXT,
                    );
                }
            }
            Element::Reset { rect, .. } => {
                paint.icon(
                    Icon::Reset,
                    rect,
                    if hover {
                        theme::TEXT
                    } else {
                        theme::TEXT_FAINT
                    },
                );
            }
            Element::Check {
                rect,
                label,
                checked,
                ..
            } => {
                paint.checkbox(
                    UiRect::new(rect.x + paint.px(58.0), rect.y, paint.px(14.0), rect.height),
                    checked,
                    hover,
                );
                paint.label(
                    rect,
                    rect.x + paint.px(78.0),
                    rect.right(),
                    label,
                    theme::TEXT,
                );
            }
            Element::Info { rect, label, value } => {
                paint.label(
                    rect,
                    rect.x,
                    rect.x + paint.px(56.0),
                    label,
                    theme::TEXT_DIM,
                );
                paint.label(
                    rect,
                    rect.x + paint.px(58.0),
                    rect.right(),
                    &value,
                    theme::TEXT,
                );
            }
            Element::Swatch { rect, color } => {
                paint.fill(rect, color);
                paint.frame(rect, theme::BORDER);
            }
            Element::Button { rect, label, .. } => {
                paint.fill(rect, theme::RAISED);
                if hover {
                    paint.fill(rect, theme::HOVER);
                }
                paint.frame(rect, theme::DIVIDER);
                paint.centred(rect, label, theme::TEXT);
            }
            Element::Note { rect, text } => {
                paint.label(rect, rect.x, rect.right(), &text, theme::TEXT_FAINT);
            }
        }
    }
}

/// The inspector while sculpting: the brush's settings.
fn push_brush(paint: &mut Paint<'_>, editor: &Editor, layout: &Layout) {
    let body = layout.inspector_body;
    let pad = paint.px(metrics::PADDING + 2.0);
    let row = paint.px(metrics::ROW_HEIGHT);
    let mut y = body.y + paint.px(8.0);
    let mut line = |paint: &mut Paint<'_>, label: &str, value: String| {
        let rect = UiRect::new(body.x + pad, y, body.width - pad * 2.0, row);
        paint.label(
            rect,
            rect.x,
            rect.x + paint.px(70.0),
            label,
            theme::TEXT_DIM,
        );
        paint.label(
            rect,
            rect.x + paint.px(72.0),
            rect.right(),
            &value,
            theme::TEXT,
        );
        y += row;
    };
    line(paint, "Brush", editor.brush().as_str().to_owned());
    line(paint, "Size", format!("{:.1} m", editor.brush_radius()));
    line(
        paint,
        "Strength",
        format!("{:.2} m", editor.brush_strength()),
    );
    y += paint.px(6.0);
    let hint = UiRect::new(body.x + pad, y, body.width - pad * 2.0, row);
    paint.label(
        hint,
        hint.x,
        hint.right(),
        "Tab mode - 1 2 size - 3 4 strength",
        theme::TEXT_FAINT,
    );
}

// ---------------------------------------------------------------------------------------
// Viewport chips and the debug overlay
// ---------------------------------------------------------------------------------------

/// The small chips over the corners of the viewport: the view, snapping, what is selected,
/// the camera's axes, and a banner for the two modes that change what a click does.
fn push_viewport_chips(
    paint: &mut Paint<'_>,
    editor: &Editor,
    scene: &EnvironmentScene,
    layout: &Layout,
) {
    let view = layout.viewport;
    let margin = paint.px(8.0);
    let height = paint.px(20.0);

    let space = if editor.is_local_space() {
        "Local"
    } else {
        "World"
    };
    let left = format!("Perspective - {space}");
    let width = paint.chip_width(&left, false);
    paint.chip_text(
        UiRect::new(view.x + margin, view.y + margin, width, height),
        None,
        &left,
        theme::TEXT_DIM,
    );

    let snap = if editor.is_module_snap() {
        "Module 4 m".to_owned()
    } else if editor.is_snapping() {
        format!("Grid {} - Snap on", panels::grid_label(editor.grid()))
    } else {
        "Snap off".to_owned()
    };
    let width = paint.chip_width(&snap, true);
    paint.chip_text(
        UiRect::new(
            view.right() - margin - width,
            view.y + margin,
            width,
            height,
        ),
        Some((
            Icon::Magnet,
            if editor.is_snapping() {
                theme::ACCENT
            } else {
                theme::TEXT_FAINT
            },
        )),
        &snap,
        theme::TEXT_DIM,
    );

    // The mode banner, top centre, only while a click means something other than select.
    let banner = if editor.is_place_mode() {
        Some(format!(
            "Placing {} - click, drag to paint, Shift+drag to fill - Esc to stop",
            theme::display_name(editor.browser().selected_name())
        ))
    } else if editor.is_sculpting() {
        Some(format!(
            "Sculpting: {} - drag to shape the ground - B to stop",
            editor.brush().as_str()
        ))
    } else {
        None
    };
    if let Some(banner) = banner {
        let width = paint.chip_width(&banner, true);
        let rect = UiRect::new(
            view.x + (view.width - width) * 0.5,
            view.y + margin,
            width,
            height,
        );
        paint.fill(rect, theme::OVERLAY);
        paint.frame(rect, theme::ACCENT);
        let slot = paint.icon_slot(rect, rect.x + paint.px(3.0));
        paint.icon(
            if editor.is_sculpting() {
                Icon::Sculpt
            } else {
                Icon::Place
            },
            slot,
            theme::ACCENT,
        );
        paint.label(
            rect,
            slot.right() + paint.px(2.0),
            rect.right(),
            &banner,
            theme::TEXT_STRONG,
        );
    }

    // Bottom left: what is selected.
    if let Some(primary) = editor.selection() {
        let count = editor.selected().len();
        let text = if count > 1 {
            format!(
                "{count} selected - {} active",
                panels::item_name(scene, primary)
            )
        } else {
            panels::item_name(scene, primary)
        };
        let (icon, tint) = panels::item_icon(scene, primary);
        let width = paint.chip_width(&text, true);
        paint.chip_text(
            UiRect::new(
                view.x + margin,
                view.bottom() - margin - height,
                width,
                height,
            ),
            Some((icon, tint)),
            &text,
            theme::TEXT,
        );
    }

    push_axes(paint, editor, layout);
}

/// The camera's orientation, as three short axes in the bottom-right of the viewport.
fn push_axes(paint: &mut Paint<'_>, editor: &Editor, layout: &Layout) {
    let (yaw, pitch) = editor.camera_angles();
    let rotation = Quat::from_yaw_pitch_roll(yaw, pitch, 0.0);
    let right = rotation.rotate(Vec3::new(1.0, 0.0, 0.0));
    let up = rotation.rotate(Vec3::new(0.0, 1.0, 0.0));
    let forward = rotation.rotate(Vec3::new(0.0, 0.0, -1.0));
    let size = paint.px(22.0);
    let centre = (
        layout.viewport.right() - paint.px(36.0),
        layout.viewport.bottom() - paint.px(36.0),
    );
    let mut axes = [
        (Vec3::new(1.0, 0.0, 0.0), "X", theme::AXIS_X),
        (Vec3::new(0.0, 1.0, 0.0), "Y", theme::AXIS_Y),
        (Vec3::new(0.0, 0.0, 1.0), "Z", theme::AXIS_Z),
    ];
    // The axis pointing away is drawn first, so the nearer ones are on top.
    axes.sort_by(|a, b| b.0.dot(forward).total_cmp(&a.0.dot(forward)));
    let back = UiRect::new(
        centre.0 - size - paint.px(6.0),
        centre.1 - size - paint.px(6.0),
        (size + paint.px(6.0)) * 2.0,
        (size + paint.px(6.0)) * 2.0,
    );
    paint.fill(back, [0.0, 0.0, 0.0, 0.25]);
    for (axis, label, color) in axes {
        let to = (
            centre.0 + axis.dot(right) * size,
            centre.1 - axis.dot(up) * size,
        );
        stroke(paint.list, centre, to, paint.px(2.0), color);
        let tag = paint.px(9.0);
        paint.fill(
            UiRect::new(to.0 - tag * 0.5, to.1 - tag * 0.5, tag, tag),
            color,
        );
        paint.centred(
            UiRect::new(to.0 - tag * 0.5, to.1 - tag * 0.5, tag, tag),
            label,
            theme::CHROME,
        );
    }
}

/// The debug overlay: numbers about the level and the editor, for whoever is working on
/// either. Off unless asked for, with F3.
fn push_debug(paint: &mut Paint<'_>, editor: &Editor, scene: &EnvironmentScene, layout: &Layout) {
    let (undo, redo) = editor.history_depth();
    let terrain = scene.terrain.as_ref().map_or_else(
        || "none".to_owned(),
        |terrain| format!("{0}x{0}", terrain.resolution),
    );
    let eye = editor.eye();
    let selection: String = editor
        .selected()
        .iter()
        .take(6)
        .map(|what| match what {
            Selected::Object(index) => format!("obj#{index} "),
            Selected::Water(index) => format!("water#{index} "),
            Selected::Spawn(index) => format!("spawn#{index} "),
            Selected::Light(index) => format!("light#{index} "),
            Selected::Monster(index) => format!("monster#{index} "),
        })
        .collect();
    let lines = [
        (
            "FPS",
            format!(
                "{:.0}  ({:.2} ms)",
                editor.fps(),
                1000.0 / editor.fps().max(1.0)
            ),
        ),
        (
            "Objects",
            format!("{} ({} block)", scene.len(), scene.blocking_count()),
        ),
        (
            "Shapes",
            format!("{} compound", scene.collision_shapes.len()),
        ),
        ("Water", format!("{}", scene.water.len())),
        ("Spawns", format!("{}", scene.spawn_points.len())),
        ("Lights", format!("{}", scene.lights.len())),
        ("Monsters", format!("{}", scene.monsters.len())),
        ("Terrain", terrain),
        ("History", format!("{undo} undo / {redo} redo")),
        ("Camera", format!("{:.1} {:.1} {:.1}", eye.x, eye.y, eye.z)),
        (
            "Select",
            if selection.is_empty() {
                "-".to_owned()
            } else {
                selection
            },
        ),
        ("Autosave", editor.autosave_path().display().to_string()),
    ];
    let row = paint.px(16.0);
    let label = paint.px(64.0);
    let width = lines
        .iter()
        .map(|(_, value)| paint.ty.width(value))
        .fold(0.0, f32::max)
        .min(paint.px(360.0))
        + label
        + paint.px(20.0);
    let panel = UiRect::new(
        layout.viewport.x + paint.px(8.0),
        layout.viewport.y + paint.px(34.0),
        width,
        row * lines.len() as f32 + paint.px(28.0),
    );
    paint.fill(panel, [0.03, 0.035, 0.04, 0.86]);
    paint.frame(panel, theme::DIVIDER);
    let title = UiRect::new(
        panel.x + paint.px(8.0),
        panel.y + paint.px(4.0),
        panel.width,
        row,
    );
    paint.label(
        title,
        title.x,
        title.right(),
        "Debug  (F3, collision drawn in the level)",
        theme::WARNING,
    );
    for (index, (name, value)) in lines.iter().enumerate() {
        let rect = UiRect::new(
            panel.x + paint.px(8.0),
            panel.y + paint.px(22.0) + row * index as f32,
            panel.width - paint.px(16.0),
            row,
        );
        paint.label(rect, rect.x, rect.x + label, name, theme::TEXT_DIM);
        paint.label(rect, rect.x + label, rect.right(), value, theme::TEXT);
    }
}

// ---------------------------------------------------------------------------------------
// Floating things: the carried asset, menus, dialogs
// ---------------------------------------------------------------------------------------

/// The name of an asset being carried out of the browser, riding beside the pointer.
fn push_carried(paint: &mut Paint<'_>, editor: &Editor) {
    if !editor.is_placing() {
        return;
    }
    let name = theme::display_name(editor.browser().selected_name());
    let pointer = editor.pointer();
    let width = paint.chip_width(&name, true);
    let rect = UiRect::new(
        pointer.0 + paint.px(14.0),
        pointer.1 + paint.px(10.0),
        width,
        paint.px(20.0),
    );
    paint.fill(rect, theme::RAISED);
    paint.frame(rect, theme::ACCENT);
    let slot = paint.icon_slot(rect, rect.x + paint.px(3.0));
    paint.icon(Icon::Place, slot, theme::ACCENT);
    paint.label(
        rect,
        slot.right() + paint.px(2.0),
        rect.right(),
        &name,
        theme::TEXT_STRONG,
    );
}

/// The open popup menu.
fn push_menu(paint: &mut Paint<'_>, editor: &Editor, scene: &EnvironmentScene, layout: &Layout) {
    let Some(menu) = editor.open_menu() else {
        return;
    };
    let entries = panels::menu_entries(menu.kind, editor, scene);
    let (panel, items) = panels::menu_rects(layout, menu.origin, &entries);
    paint.shadow(panel);
    paint.fill(panel, theme::RAISED);
    paint.frame(panel, theme::DIVIDER);
    let pointer = editor.pointer();
    for (entry, rect) in entries.iter().zip(&items) {
        if entry.separator {
            paint.divider_h(
                panel.x + paint.px(6.0),
                panel.right() - paint.px(6.0),
                rect.y - paint.px(4.0),
            );
        }
        let hover = rect.contains(pointer.0, pointer.1) && entry.enabled;
        if hover {
            paint.fill(*rect, theme::ACCENT_FILL);
        }
        let ink = if entry.enabled {
            theme::TEXT
        } else {
            theme::TEXT_FAINT
        };
        let slot = paint.icon_slot(*rect, rect.x + paint.px(2.0));
        if entry.checked {
            paint.icon(Icon::Check, slot, theme::ACCENT);
        }
        paint.label(
            *rect,
            slot.right() + paint.px(4.0),
            rect.right(),
            &entry.label,
            if hover { theme::TEXT_STRONG } else { ink },
        );
        if !entry.key.is_empty() {
            paint.right(
                *rect,
                rect.right() - paint.px(6.0),
                entry.key,
                theme::TEXT_FAINT,
            );
        }
    }
}

/// Whichever dialog is up, over a dimmed editor.
fn push_dialogs(paint: &mut Paint<'_>, editor: &Editor, layout: &Layout) {
    let kind = if editor.save_as_text().is_some() {
        DialogKind::SaveAs
    } else {
        match editor.modal() {
            Some(Modal::Shortcuts) => DialogKind::Shortcuts,
            Some(Modal::Unsaved { .. }) => DialogKind::Unsaved,
            None => return,
        }
    };
    let window = UiRect::new(0.0, 0.0, layout.window.0 as f32, layout.window.1 as f32);
    paint.fill(window, theme::SCRIM);
    let dialog = panels::dialog(layout, kind);
    paint.shadow(dialog.panel);
    paint.fill(dialog.panel, theme::PANEL);
    paint.frame(dialog.panel, theme::DIVIDER);
    let title = match kind {
        DialogKind::Shortcuts => "Keyboard Shortcuts",
        DialogKind::Unsaved => "Unsaved Changes",
        DialogKind::SaveAs => "Save Level As",
    };
    paint.header(dialog.title, title, None);
    let pointer = editor.pointer();
    paint.button(
        dialog.close,
        Icon::Close,
        State {
            hover: dialog.close.contains(pointer.0, pointer.1),
            ..State::default()
        },
    );

    let body = dialog.body;
    let line = paint.px(metrics::ROW_HEIGHT);
    match kind {
        DialogKind::Shortcuts => push_shortcuts(paint, body),
        DialogKind::Unsaved => {
            let file = editor
                .save_path()
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let target = match editor.modal() {
                Some(Modal::Unsaved { path }) => path
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                _ => String::new(),
            };
            let slot = paint.icon_slot(UiRect::new(body.x, body.y, body.width, line), body.x);
            paint.icon(Icon::Warning, slot, theme::WARNING);
            paint.label(
                UiRect::new(body.x, body.y, body.width, line),
                slot.right() + paint.px(6.0),
                body.right(),
                &format!("{file} has changes that are not saved."),
                theme::TEXT,
            );
            paint.label(
                UiRect::new(body.x, body.y + line, body.width, line),
                slot.right() + paint.px(6.0),
                body.right(),
                &format!("Save them before opening {target}?"),
                theme::TEXT_DIM,
            );
        }
        DialogKind::SaveAs => {
            let folder = editor
                .save_path()
                .parent()
                .map_or_else(String::new, |folder| {
                    folder.display().to_string().replace('\\', "/")
                });
            paint.label(
                UiRect::new(body.x, body.y, body.width, line),
                body.x,
                body.right(),
                &format!("In {folder}/"),
                theme::TEXT_DIM,
            );
            let field = UiRect::new(
                body.x,
                body.y + line + paint.px(4.0),
                body.width,
                paint.px(metrics::FIELD_HEIGHT + 4.0),
            );
            let name = editor.save_as_text().unwrap_or("");
            paint.field(field, Some(Icon::Save), name, "level name", true, false);
            paint.right(
                field,
                field.right() - paint.px(6.0),
                ".json",
                theme::TEXT_FAINT,
            );
        }
    }
    for (rect, _, label, primary) in &dialog.buttons {
        let hover = rect.contains(pointer.0, pointer.1);
        paint.fill(
            *rect,
            if *primary {
                theme::ACCENT
            } else {
                theme::RAISED
            },
        );
        if hover {
            paint.fill(*rect, theme::HOVER);
        }
        paint.frame(
            *rect,
            if *primary {
                theme::ACCENT
            } else {
                theme::DIVIDER
            },
        );
        paint.centred(
            *rect,
            label,
            if *primary {
                theme::TEXT_STRONG
            } else {
                theme::TEXT
            },
        );
    }
}

/// The shortcuts window's contents: groups in two columns, each line a row of key caps and
/// what they do.
fn push_shortcuts(paint: &mut Paint<'_>, body: UiRect) {
    let columns = 2;
    let gutter = paint.px(24.0);
    let column_width = (body.width - gutter) / columns as f32;
    let row = paint.px(19.0);
    let mut heights = vec![0.0_f32; columns];
    for (group, shortcuts) in panels::SHORTCUTS {
        // The shortest column takes the next group, so the columns come out even.
        let column = heights
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(index, _)| index);
        let x = body.x + column as f32 * (column_width + gutter);
        let mut y = body.y + heights[column];
        let heading = UiRect::new(x, y, column_width, row);
        paint.label(heading, x, heading.right(), group, theme::ACCENT);
        paint.divider_h(x, heading.right(), heading.bottom() - paint.px(2.0));
        y += row + paint.px(3.0);
        // The key column is as wide as this group's widest set of keys, so the descriptions
        // start in one line and get all the room that is left.
        let caps = |keys: &str| -> f32 {
            keys.split(" / ")
                .map(|part| paint.ty.width(part) + paint.px(11.0))
                .sum::<f32>()
        };
        let key_width = shortcuts
            .iter()
            .map(|(keys, _)| caps(keys))
            .fold(0.0, f32::max)
            + paint.px(8.0);
        for (keys, what) in *shortcuts {
            let rect = UiRect::new(x, y, column_width, row);
            let cap_height = row - paint.px(4.0);
            let mut cap_x = x;
            for part in keys.split(" / ") {
                cap_x += paint.key((cap_x, y + paint.px(2.0)), part, cap_height) + paint.px(3.0);
            }
            paint.label(rect, x + key_width, rect.right(), what, theme::TEXT_DIM);
            y += row;
        }
        heights[column] = y - body.y + paint.px(12.0);
    }
}

// ---------------------------------------------------------------------------------------
// The world overlay: what is selected, where it is, and the handles that move it.
// ---------------------------------------------------------------------------------------

/// What a selected thing is outlined in.
///
/// Cyan, and drawn over the level rather than into it: this crate has no renderer and cannot
/// tint a material, and an outline painted in the interface layer is visible whatever the
/// object's own material, whatever the light on it, and even when the object is inside
/// another one.
const SELECTION_COLOR: UiColor = [0.42, 0.76, 1.00, 0.95];

/// What everything *else* in a multiple selection is outlined in: the same colour, quieter.
///
/// The primary is what the inspector describes and what the gizmo stands on, so it is the one
/// drawn in full.
const SELECTION_SECONDARY: UiColor = [0.42, 0.76, 1.00, 0.45];

/// What the striped silhouette of an off-screen selection is drawn in.
///
/// Amber rather than cyan, because it is not the object: it is a *report* about an object
/// that is not on screen, and the two must not be mistaken for one another at a glance.
const OFFSCREEN_COLOR: UiColor = [1.00, 0.68, 0.18, 0.95];

/// How many selected things get an outline before the overlay stops drawing them.
///
/// `CTRL+A` on a finished level selects every placement in it, and a wireframe box each is
/// hundreds of quads that say nothing an author can read anyway — past a few dozen the screen
/// is a mesh of lines. The primary is always drawn, so the gizmo never stands on an object
/// with no outline under it.
const OUTLINE_LIMIT: usize = 48;

/// How thick the selection outline is, in logical pixels.
const OUTLINE_PIXELS: f32 = 2.0;

/// How thick a gizmo handle is, in logical pixels.
const HANDLE_PIXELS: f32 = 3.0;

/// The most quads one [`stroke`] may cost.
///
/// A line is drawn as a run of quads, so its cost is its length in pixels. A corner of a
/// selection box that sits just in front of the near plane projects tens of thousands of
/// pixels outside the window, and a wireframe drawn through it would be a draw list too big
/// for the frame's upload slot - which is a fatal renderer error, not a drawing artefact.
/// Segments are clipped to the window before they are stroked; this is the backstop for the
/// callers that cannot clip, and it turns an impossible line into a dotted one.
const STROKE_MAX_QUADS: f32 = 512.0;

/// How wide the striped silhouette is when nothing about the object can be projected, in
/// logical pixels. See [`push_offscreen`].
const SILHOUETTE_MIN: f32 = 26.0;

/// The largest a striped silhouette may be drawn, in logical pixels.
///
/// A selected floor slab seen from a metre away would otherwise cover the whole screen with
/// stripes, which is the one thing an editor-only marker must never do.
const SILHOUETTE_MAX: f32 = 220.0;

/// How far a silhouette is kept from the edge of the window, in logical pixels.
const SILHOUETTE_MARGIN: f32 = 16.0;

/// Draws what is selected and the handles that move it, over the level and under the panels.
///
/// Under the panels deliberately: the browser and the hierarchy are opaque, and an outline
/// painted over them would be a selection marker floating on a list.
fn push_world_overlay(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    scratch: &mut String,
    layout: &Layout,
) {
    let viewport = layout.window;
    let area = layout.viewport;
    let scale = editor.scale();

    push_brush_cursor(list, editor, scene, layout);

    // What the pointer is over, outlined faintly: the answer to "what will this click
    // select" before the click.
    // Not water: its pickable box is the whole volume of a lake, and a wireframe that size
    // under the pointer is noise rather than an answer.
    if let Some(hovered) = editor.hovered()
        && !editor.is_selected(hovered)
        && !matches!(hovered, Selected::Water(_))
        && let Some((center, half_extents, rotation)) = pick::selection_box(scene, hovered)
    {
        let corners = box_corners(center, half_extents, rotation);
        push_wireframe(
            list,
            &corners,
            &editor.view_projection(),
            viewport,
            scale.px(1.0),
            HOVER_COLOR,
        );
    }
    let view_projection = editor.view_projection();
    let primary = editor.selection();

    for (order, what) in editor.selected().iter().enumerate() {
        let is_primary = primary == Some(*what);
        if order >= OUTLINE_LIMIT && !is_primary {
            continue;
        }
        let Some((center, half_extents, rotation)) = pick::selection_box(scene, *what) else {
            continue;
        };
        let corners = box_corners(center, half_extents, rotation);
        let projected: Vec<(f32, f32)> = corners
            .iter()
            .filter_map(|corner| {
                engine_presentation::nameplate::project(*corner, &view_projection, viewport)
            })
            .collect();
        let bounds = bounds_of(&projected);
        let on_screen = bounds.is_some_and(|rect| overlaps(rect, area));

        if on_screen && projected.len() == corners.len() {
            // Every corner is in front of the eye: the box can be drawn as a box.
            push_wireframe(
                list,
                &corners,
                &view_projection,
                viewport,
                scale.px(OUTLINE_PIXELS),
                if is_primary {
                    SELECTION_COLOR
                } else {
                    SELECTION_SECONDARY
                },
            );
        } else if on_screen {
            // Partly behind the eye: the corners that project still say where it is, and a
            // rectangle around them is honest about that where a half-drawn box is not.
            if let Some(rect) = bounds {
                list.outline(
                    rect,
                    scale.px(OUTLINE_PIXELS),
                    if is_primary {
                        SELECTION_COLOR
                    } else {
                        SELECTION_SECONDARY
                    },
                );
            }
        } else if is_primary {
            // Off screen entirely. The selection is not hidden because the camera turned
            // away from it — it is reported at the edge it went out by.
            push_offscreen(
                list,
                editor,
                scene,
                *what,
                center,
                half_extents,
                bounds,
                scratch,
                area,
            );
        }
    }

    push_lights(list, editor, scene, viewport);
    push_drop_preview(list, editor, scene, viewport);
    push_gizmo(list, editor, scene);

    // How far the drag has gone, beside the pointer, while it goes.
    if let Some(readout) = editor.drag_readout() {
        let mut paint = Paint::new(list, scale);
        let pointer = editor.pointer();
        let width = paint.chip_width(readout, false);
        let rect = UiRect::new(
            pointer.0 + paint.px(16.0),
            pointer.1 + paint.px(14.0),
            width,
            paint.px(20.0),
        );
        paint.fill(rect, theme::RAISED);
        paint.frame(rect, theme::DIVIDER);
        paint.label(
            rect,
            rect.x + paint.px(6.0),
            rect.right(),
            readout,
            theme::TEXT_STRONG,
        );
    }

    // The selection box, over everything in the world it is selecting from.
    if let Some(marquee) = editor.marquee() {
        let rect = marquee.rect();
        list.rect(rect, [0.20, 0.95, 1.00, 0.10]);
        list.outline(rect, scale.px(1.0), SELECTION_COLOR);
    }
}

/// What a light's marker is drawn in: warm, like the light it stands for.
const LIGHT_COLOR: UiColor = [1.00, 0.82, 0.35, 0.95];

/// Draws a marker where every light in the level is.
///
/// A light has no mesh and never will, so without this an author could only find one by
/// what it lights — which is nothing at noon. A diamond at the light, and a line down to the
/// ground under it so its height reads at a glance.
fn push_lights(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    viewport: (u32, u32),
) {
    let scale = editor.scale();
    let view_projection = editor.view_projection();
    let size = scale.px(7.0);
    for (index, light) in scene.lights.iter().enumerate() {
        let Some(at) =
            engine_presentation::nameplate::project(light.position, &view_projection, viewport)
        else {
            continue;
        };
        let floor = Vec3::new(
            light.position.x,
            scene.ground_at(light.position),
            light.position.z,
        );
        if let Some(down) =
            engine_presentation::nameplate::project(floor, &view_projection, viewport)
            && let Some((from, to)) = clip_segment(
                at,
                down,
                UiRect::new(0.0, 0.0, viewport.0 as f32, viewport.1 as f32),
            )
        {
            stroke(list, from, to, scale.px(1.0), [1.0, 0.82, 0.35, 0.45]);
        }
        let selected = editor.is_selected(Selected::Light(index));
        let color = if selected {
            SELECTION_COLOR
        } else {
            LIGHT_COLOR
        };
        // A diamond, as four short strokes: a square would read as a selection handle.
        let points = [
            (at.0, at.1 - size),
            (at.0 + size, at.1),
            (at.0, at.1 + size),
            (at.0 - size, at.1),
        ];
        for corner in 0..4 {
            stroke(
                list,
                points[corner],
                points[(corner + 1) % 4],
                scale.px(2.0),
                color,
            );
        }
        list.rect(
            UiRect::new(
                at.0 - scale.px(1.5),
                at.1 - scale.px(1.5),
                scale.px(3.0),
                scale.px(3.0),
            ),
            color,
        );
    }
}

/// The eight corners of an oriented box, in world space.
fn box_corners(center: Vec3, half_extents: Vec3, rotation: Quat) -> [Vec3; 8] {
    let mut corners = [Vec3::ZERO; 8];
    for (index, corner) in corners.iter_mut().enumerate() {
        let sign = |bit: usize| if index & (1 << bit) == 0 { -1.0 } else { 1.0 };
        let local = Vec3::new(
            half_extents.x * sign(0),
            half_extents.y * sign(1),
            half_extents.z * sign(2),
        );
        *corner = center + rotation.rotate(local);
    }
    corners
}

/// The twelve edges of a box, as pairs of indices into [`box_corners`]'s output.
///
/// Two corners share an edge when their sign bits differ in exactly one place, which is what
/// this table is: the same box every time, written once.
const BOX_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (0, 2),
    (0, 4),
    (1, 3),
    (1, 5),
    (2, 3),
    (2, 6),
    (3, 7),
    (4, 5),
    (4, 6),
    (5, 7),
    (6, 7),
];

/// Draws a box as twelve lines.
fn push_wireframe(
    list: &mut UiDrawList,
    corners: &[Vec3; 8],
    view_projection: &engine_core::math::Mat4,
    viewport: (u32, u32),
    thickness: f32,
    color: UiColor,
) {
    let mut screen = [None; 8];
    for (slot, corner) in corners.iter().enumerate() {
        screen[slot] = engine_presentation::nameplate::project(*corner, view_projection, viewport);
    }
    // Clipped to the window before it is drawn. A box the camera is inside projects corners
    // hundreds of thousands of pixels away, and stroking to one of those is a run of quads
    // that long: what is off screen has to be dropped here rather than drawn and discarded.
    let window = UiRect::new(
        -thickness,
        -thickness,
        viewport.0 as f32 + thickness * 2.0,
        viewport.1 as f32 + thickness * 2.0,
    );
    for (from, to) in BOX_EDGES {
        let (Some(from), Some(to)) = (screen[from], screen[to]) else {
            continue;
        };
        let Some((from, to)) = clip_segment(from, to, window) else {
            continue;
        };
        stroke(list, from, to, thickness, color);
    }
}

/// The part of the segment `from`-`to` that is inside `rect`, if any.
///
/// Liang-Barsky: the segment is a parameter range, and each edge of the rectangle trims one
/// end of it. Returns `None` when the trimming leaves nothing, which is the whole point -
/// a segment that misses the window entirely is not drawn at all.
fn clip_segment(
    from: (f32, f32),
    to: (f32, f32),
    rect: UiRect,
) -> Option<((f32, f32), (f32, f32))> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let (mut enter, mut leave) = (0.0_f32, 1.0_f32);
    for (direction, distance) in [
        (-dx, from.0 - rect.x),
        (dx, rect.right() - from.0),
        (-dy, from.1 - rect.y),
        (dy, rect.bottom() - from.1),
    ] {
        if direction == 0.0 {
            // Parallel to this edge: either wholly inside it or wholly outside.
            if distance < 0.0 {
                return None;
            }
            continue;
        }
        let t = distance / direction;
        if direction < 0.0 {
            enter = enter.max(t);
        } else {
            leave = leave.min(t);
        }
        if enter > leave {
            return None;
        }
    }
    if !(enter.is_finite() && leave.is_finite()) {
        return None;
    }
    Some((
        (from.0 + dx * enter, from.1 + dy * enter),
        (from.0 + dx * leave, from.1 + dy * leave),
    ))
}

/// The rectangle that contains every projected point, if there are any.
fn bounds_of(points: &[(f32, f32)]) -> Option<UiRect> {
    let (mut left, mut top) = (f32::MAX, f32::MAX);
    let (mut right, mut bottom) = (f32::MIN, f32::MIN);
    for (x, y) in points {
        left = left.min(*x);
        right = right.max(*x);
        top = top.min(*y);
        bottom = bottom.max(*y);
    }
    (!points.is_empty()).then(|| UiRect::new(left, top, right - left, bottom - top))
}

/// The colour each brush mode is drawn in, so the ring says what a click will do before the
/// label is read: green builds up, amber digs down, blue levels, violet softens.
fn brush_color(mode: engine_environment::terrain::BrushMode) -> UiColor {
    use engine_environment::terrain::BrushMode;
    match mode {
        BrushMode::Raise => [0.45, 0.88, 0.45, 1.0],
        BrushMode::Lower => [0.96, 0.62, 0.30, 1.0],
        BrushMode::Flatten => [0.36, 0.66, 1.00, 1.0],
        BrushMode::Smooth => [0.78, 0.62, 1.00, 1.0],
    }
}

/// The brush's picture, for the label beside the ring.
fn brush_icon(mode: engine_environment::terrain::BrushMode) -> Icon {
    use engine_environment::terrain::BrushMode;
    match mode {
        BrushMode::Raise => Icon::ArrowUp,
        BrushMode::Lower => Icon::ArrowDown,
        BrushMode::Flatten => Icon::Flatten,
        BrushMode::Smooth => Icon::Smooth,
    }
}

/// How high the raise and lower arrows stand, in metres: longer for a stronger brush, and
/// kept between a hand's width and a storey so it stays readable at either end.
#[must_use]
pub fn brush_arrow_length(strength: f32) -> f32 {
    (1.0 + strength * 4.0).clamp(1.0, 8.0)
}

/// The terrain brush as a cursor, drawn on the ground under the pointer.
///
/// ```text
///        RAISE              LOWER             FLATTEN              SMOOTH
///          ^                 |              ------------          ~~~~~~~~
///       .--|--.           .--|--.          .----------.          .~~~~~~.
///      (   |   )         (   v   )        (  target    )        ( ~~~~~~ )
///       '-----'           '-----'          '----------'          '~~~~~~'
/// ```
///
/// The outer ring is the brush's reach, following the ground so it sits on a hill the way the
/// stroke will; the inner ring is where the smoothstep falloff is at half strength. The mode
/// is its colour and its shape: an arrow up or down whose length is the strength, the level a
/// flatten will pull the ground to, or a wavy ring for smoothing. While the button is down the
/// ring is drawn heavier, so painting and aiming look different.
fn push_brush_cursor(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    layout: &Layout,
) {
    use engine_environment::terrain::BrushMode;
    if !editor.is_sculpting() || editor.open_menu().is_some() || editor.modal().is_some() {
        return;
    }
    let pointer = editor.pointer();
    if !layout.viewport.contains(pointer.0, pointer.1) {
        return;
    }
    let Some(centre) = editor.cursor_world() else {
        return;
    };
    let scale = editor.scale();
    let view_projection = editor.view_projection();
    let window = layout.window;
    let mode = editor.brush();
    let color = brush_color(mode);
    let faint = [color[0], color[1], color[2], 0.45];
    let shadow = [0.0, 0.0, 0.0, 0.45];
    let painting = editor.is_brushing();
    let radius = editor.brush_radius();
    let lift = 0.06;
    let project =
        |point: Vec3| engine_presentation::nameplate::project(point, &view_projection, window);
    let clip = layout.viewport;

    // A closed ring at `radius`, each point lifted onto the ground (or held at `height`, for
    // the flatten target), with `wobble` bending it into waves.
    let ring = |list: &mut UiDrawList,
                radius: f32,
                height: Option<f32>,
                wobble: f32,
                thickness: f32,
                color: UiColor| {
        let segments = 72;
        let point_at = |step: usize| {
            let angle = step as f32 / segments as f32 * std::f32::consts::TAU;
            let bend = 1.0 + wobble * (angle * 10.0).sin();
            let x = centre.x + angle.cos() * radius * bend;
            let z = centre.z + angle.sin() * radius * bend;
            let y = height.unwrap_or_else(|| scene.ground_at(Vec3::new(x, 0.0, z))) + lift;
            Vec3::new(x, y, z)
        };
        let mut previous = project(point_at(0));
        for step in 1..=segments {
            let next = project(point_at(step));
            if let (Some(from), Some(to)) = (previous, next)
                && let Some((from, to)) = clip_segment(from, to, clip)
            {
                stroke(list, from, to, thickness, color);
            }
            previous = next;
        }
    };

    let heavy = scale.px(if painting { 3.0 } else { 2.0 });
    // A dark edge under the ring, so it reads on bright grass and in a dark valley alike.
    ring(list, radius, None, 0.0, heavy + scale.px(2.0), shadow);
    ring(list, radius, None, 0.0, heavy, color);
    let wobble = if mode == BrushMode::Smooth { 0.12 } else { 0.0 };
    ring(list, radius * 0.5, None, wobble, scale.px(1.0), faint);

    let ground = scene.ground_at(centre);
    let on_ground = Vec3::new(centre.x, ground + lift, centre.z);
    match mode {
        BrushMode::Raise | BrushMode::Lower => {
            let length = brush_arrow_length(editor.brush_strength());
            let raised = Vec3::new(centre.x, ground + length, centre.z);
            let (base, tip) = if mode == BrushMode::Raise {
                (on_ground, raised)
            } else {
                (raised, on_ground)
            };
            if let (Some(from), Some(to)) = (project(base), project(tip)) {
                stroke(list, from, to, heavy + scale.px(2.0), shadow);
                stroke(list, from, to, heavy, color);
                // The head: two short strokes back from the tip.
                let (dx, dy) = (to.0 - from.0, to.1 - from.1);
                let length = (dx * dx + dy * dy).sqrt().max(1.0);
                let (ux, uy) = (dx / length, dy / length);
                let head = scale.px(9.0);
                for side in [-1.0_f32, 1.0] {
                    let back = (
                        to.0 - ux * head - uy * head * 0.6 * side,
                        to.1 - uy * head + ux * head * 0.6 * side,
                    );
                    stroke(list, to, back, heavy, color);
                }
            }
        }
        BrushMode::Flatten => {
            // The level the ground will be pulled to: the height under the middle of the
            // brush, held flat, so it shows where the plateau will cut into the hill.
            ring(list, radius * 0.98, Some(ground), 0.0, scale.px(1.0), faint);
            let arm = radius * 0.35;
            for (from, to) in [
                (
                    Vec3::new(centre.x - arm, ground + lift, centre.z),
                    Vec3::new(centre.x + arm, ground + lift, centre.z),
                ),
                (
                    Vec3::new(centre.x, ground + lift, centre.z - arm),
                    Vec3::new(centre.x, ground + lift, centre.z + arm),
                ),
            ] {
                if let (Some(from), Some(to)) = (project(from), project(to)) {
                    stroke(list, from, to, scale.px(1.0), faint);
                }
            }
        }
        BrushMode::Smooth => {}
    }

    // The centre, as a small cross: where the stroke is aimed.
    if let Some(at) = project(on_ground) {
        let arm = scale.px(5.0);
        stroke(
            list,
            (at.0 - arm, at.1),
            (at.0 + arm, at.1),
            scale.px(1.0),
            color,
        );
        stroke(
            list,
            (at.0, at.1 - arm),
            (at.0, at.1 + arm),
            scale.px(1.0),
            color,
        );
    }

    // And in words, beside the pointer: the mode, the size, the strength.
    let mut paint = Paint::new(list, scale);
    let text = format!(
        "{}  {:.1} m  -  strength {:.2}",
        theme::display_name(&mode.as_str().to_ascii_lowercase()),
        radius,
        editor.brush_strength()
    );
    let width = paint.chip_width(&text, true);
    let rect = UiRect::new(
        pointer.0 + paint.px(18.0),
        pointer.1 + paint.px(16.0),
        width,
        paint.px(20.0),
    );
    paint.fill(rect, theme::OVERLAY);
    paint.frame(rect, if painting { color } else { theme::DIVIDER });
    let slot = paint.icon_slot(rect, rect.x + paint.px(3.0));
    paint.icon(brush_icon(mode), slot, color);
    paint.label(
        rect,
        slot.right() + paint.px(2.0),
        rect.right(),
        &text,
        theme::TEXT_STRONG,
    );
}

/// Whether any of `rect` is inside `area`.
fn overlaps(rect: UiRect, area: UiRect) -> bool {
    rect.right() >= area.x
        && rect.bottom() >= area.y
        && rect.x <= area.right()
        && rect.y <= area.bottom()
}

/// What the pointer is over is outlined in.
const HOVER_COLOR: UiColor = [1.0, 1.0, 1.0, 0.30];

/// Where a carried asset would land: a footprint box on the surface, so an author sees the
/// drop before they make it — and in placing mode, the cell a click fills, or the whole
/// rectangle a `SHIFT`-drag will fill, with its count.
fn push_drop_preview(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    viewport: (u32, u32),
) {
    let scale = editor.scale();
    let view_projection = editor.view_projection();
    let project =
        |point: Vec3| engine_presentation::nameplate::project(point, &view_projection, viewport);
    // A line along the ground from `from` to `to`, bent over the terrain in between.
    let ground_line = |list: &mut UiDrawList,
                       from: (f32, f32),
                       to: (f32, f32),
                       thickness: f32,
                       color: UiColor| {
        let pieces = 8;
        let at = |t: f32| {
            let (x, z) = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            Vec3::new(x, scene.ground_at(Vec3::new(x, 0.0, z)) + 0.05, z)
        };
        for piece in 0..pieces {
            let (a, b) = (
                piece as f32 / pieces as f32,
                (piece + 1) as f32 / pieces as f32,
            );
            if let (Some(a), Some(b)) = (project(at(a)), project(at(b))) {
                stroke(list, a, b, thickness, color);
            }
        }
    };
    let rectangle = |list: &mut UiDrawList,
                     low: (f32, f32),
                     high: (f32, f32),
                     thickness: f32,
                     color: UiColor| {
        ground_line(list, (low.0, low.1), (high.0, low.1), thickness, color);
        ground_line(list, (high.0, low.1), (high.0, high.1), thickness, color);
        ground_line(list, (high.0, high.1), (low.0, high.1), thickness, color);
        ground_line(list, (low.0, high.1), (low.0, low.1), thickness, color);
    };

    if editor.is_place_mode() {
        let spacing = editor.preview_spacing();
        let half = spacing * 0.5;
        let (start, current, fill) = match editor.paint_stroke() {
            Some(stroke) => (stroke.start, stroke.current, stroke.fill),
            None => {
                let Some(at) = editor.cursor_world() else {
                    return;
                };
                let cell = (
                    (at.x / spacing).round() as i64,
                    (at.z / spacing).round() as i64,
                );
                (cell, cell, false)
            }
        };
        let (x0, x1) = (start.0.min(current.0), start.0.max(current.0));
        let (z0, z1) = (start.1.min(current.1), start.1.max(current.1));
        let (x0, x1, z0, z1) = if fill {
            (x0, x1, z0, z1)
        } else {
            (current.0, current.0, current.1, current.1)
        };
        let low = (x0 as f32 * spacing - half, z0 as f32 * spacing - half);
        let high = (x1 as f32 * spacing + half, z1 as f32 * spacing + half);
        rectangle(list, low, high, scale.px(2.0), theme::ACCENT);
        if fill {
            // The cells inside, faintly, so the count on the label can be seen as well as read.
            for x in x0..x1 {
                let line = x as f32 * spacing + half;
                ground_line(
                    list,
                    (line, low.1),
                    (line, high.1),
                    scale.px(1.0),
                    theme::ACCENT_FILL,
                );
            }
            for z in z0..z1 {
                let line = z as f32 * spacing + half;
                ground_line(
                    list,
                    (low.0, line),
                    (high.0, line),
                    scale.px(1.0),
                    theme::ACCENT_FILL,
                );
            }
            let (columns, rows) = (x1 - x0 + 1, z1 - z0 + 1);
            let mut paint = Paint::new(list, scale);
            let text = format!(
                "{columns} x {rows} = {} tiles, {spacing} m apart",
                columns * rows
            );
            let pointer = editor.pointer();
            let width = paint.chip_width(&text, false);
            let rect = UiRect::new(
                pointer.0 + paint.px(16.0),
                pointer.1 + paint.px(14.0),
                width,
                paint.px(20.0),
            );
            paint.fill(rect, theme::RAISED);
            paint.frame(rect, theme::ACCENT);
            paint.label(
                rect,
                rect.x + paint.px(6.0),
                rect.right(),
                &text,
                theme::TEXT_STRONG,
            );
        }
        return;
    }

    let Some(at) = editor.drop_preview(scene) else {
        return;
    };
    let half = Vec3::new(0.5, 0.5, 0.5);
    let corners = box_corners(at + Vec3::new(0.0, half.y, 0.0), half, Quat::IDENTITY);
    push_wireframe(
        list,
        &corners,
        &view_projection,
        viewport,
        scale.px(1.0),
        theme::ACCENT,
    );
    // A cross on the ground where its origin will be.
    let arm = 0.8;
    for (from, to) in [
        (at - Vec3::new(arm, 0.0, 0.0), at + Vec3::new(arm, 0.0, 0.0)),
        (at - Vec3::new(0.0, 0.0, arm), at + Vec3::new(0.0, 0.0, arm)),
    ] {
        if let (Some(from), Some(to)) = (project(from), project(to)) {
            stroke(list, from, to, scale.px(2.0), theme::ACCENT);
        }
    }
}

/// Draws the striped silhouette that stands for a selection the camera is not looking at.
///
/// # Why stripes
///
/// It has to be unmistakably *not the object*. A solid shape at the edge of the screen reads
/// as a thing standing there; a hatched one reads as a marker, and the hatch is what makes it
/// legible over a bright sky and a dark cave mouth alike. It is drawn where the object is in
/// the direction it lies, clamped inside the window, so turning the camera walks it around
/// the edge towards the object and it meets the object's real outline as it comes into view.
#[allow(
    clippy::too_many_arguments,
    reason = "one call site; splitting it would only move the arguments"
)]
fn push_offscreen(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    what: Selected,
    center: Vec3,
    half_extents: Vec3,
    bounds: Option<UiRect>,
    scratch: &mut String,
    area: UiRect,
) {
    let scale = editor.scale();
    let view_projection = editor.view_projection();
    let window = (area.right().max(1.0) as u32, area.bottom().max(1.0) as u32);
    let anchor = edge_anchor(center, &view_projection, window);

    // How big it would be if it were on screen: the projection's own arithmetic, done by hand
    // because there is nothing to project when the object is behind the eye.
    let size = match bounds {
        Some(rect) => rect.width.max(rect.height),
        None => {
            let distance = (center - editor.eye()).length().max(0.1);
            let focal = window.1 as f32 * 0.5 / (crate::camera::FOV_Y * 0.5).tan();
            half_extents.max_element() * 2.0 * focal / distance
        }
    }
    .clamp(scale.px(SILHOUETTE_MIN), scale.px(SILHOUETTE_MAX));

    let margin = scale.px(SILHOUETTE_MARGIN);
    let half = size * 0.5;
    let x = anchor.0.clamp(
        area.x + margin + half,
        (area.right() - margin - half).max(area.x + margin + half),
    );
    let y = anchor.1.clamp(
        area.y + margin + half,
        (area.bottom() - margin - half).max(area.y + margin + half),
    );
    let rect = UiRect::new(x - half, y - half, size, size);

    stripes(list, rect, scale.px(7.0), scale.px(2.0), OFFSCREEN_COLOR);
    list.outline(rect, scale.px(OUTLINE_PIXELS), OFFSCREEN_COLOR);

    // Named, so a silhouette at the edge of the screen is "the pillar I selected" rather than
    // "something out there".
    scratch.clear();
    scratch.push_str("Selected: ");
    scratch.push_str(&panels::item_name(scene, what));
    list.text_at(
        (rect.x, rect.bottom() + scale.px(2.0)),
        scratch,
        OFFSCREEN_COLOR,
        scale.text(1.0),
    );
}

/// Where an off-screen point lies, as a position on the window's edge.
///
/// A point behind the eye has no projection at all, and one in front of it but outside the
/// frustum projects outside the window. Both are answered the same way: take the clip-space
/// direction, flip it when the point is behind the eye — a point behind the camera is in the
/// opposite direction on screen from what its clip coordinates say — and push it out to the
/// unit square, which is the window's edge.
fn edge_anchor(
    point: Vec3,
    view_projection: &engine_core::math::Mat4,
    viewport: (u32, u32),
) -> (f32, f32) {
    let clip = *view_projection * point.extend(1.0);
    let (x, y) = if clip.w > 1e-6 {
        (clip.x / clip.w, clip.y / clip.w)
    } else {
        (-clip.x, -clip.y)
    };
    let longest = x.abs().max(y.abs()).max(1e-6);
    let (x, y) = (x / longest, y / longest);
    (
        (x * 0.5 + 0.5) * viewport.0 as f32,
        (1.0 - (y * 0.5 + 0.5)) * viewport.1 as f32,
    )
}

/// Fills `rect` with diagonal stripes, `spacing` apart and `thickness` wide.
///
/// Clipped to the rectangle by construction rather than by a clip rectangle, because the draw
/// list has no clipping and a stripe that ran past the edge would be a diagonal line loose on
/// the screen.
fn stripes(list: &mut UiDrawList, rect: UiRect, spacing: f32, thickness: f32, color: UiColor) {
    if rect.width <= 0.0 || rect.height <= 0.0 || spacing <= 0.0 {
        return;
    }
    let mut offset = -rect.height;
    while offset < rect.width {
        // A 45-degree stripe from the top edge to the right edge, cut off at whichever of the
        // rectangle's sides it reaches first.
        let start = if offset >= 0.0 {
            (rect.x + offset, rect.y)
        } else {
            (rect.x, rect.y - offset)
        };
        let run = (rect.width - (start.0 - rect.x)).min(rect.height - (start.1 - rect.y));
        if run > 0.0 {
            stroke(
                list,
                start,
                (start.0 + run, start.1 + run),
                thickness,
                color,
            );
        }
        offset += spacing;
    }
}

/// Draws the selection's gizmo: three arrows to move it, a ring to turn it, or three
/// box-ended handles and a centre square to scale it.
fn push_gizmo(list: &mut UiDrawList, editor: &Editor, scene: &EnvironmentScene) {
    let Some(screen) = editor.gizmo(scene) else {
        return;
    };
    let scale = editor.scale();
    let thickness = scale.px(HANDLE_PIXELS);
    let hovered = editor.gizmo_hover();
    let shadow = [0.0, 0.0, 0.0, 0.65];

    if screen.mode == crate::gizmo::GizmoMode::Rotate {
        let color = if hovered == Some(crate::gizmo::Handle::Ring) {
            theme::HANDLE_ACTIVE
        } else {
            crate::gizmo::Axis::Y.color()
        };
        for (from, to) in screen.ring_segments() {
            stroke(list, from, to, thickness + scale.px(2.0), shadow);
        }
        for (from, to) in screen.ring_segments() {
            stroke(list, from, to, thickness, color);
        }
    } else {
        // A dark line under every handle, one pixel wider. That is the whole of "readable
        // against any background": a red axis over a red rock is invisible, and a red axis
        // with a black edge is not.
        for handle in screen.handles.into_iter().flatten() {
            stroke(
                list,
                handle.from,
                handle.to,
                thickness + scale.px(2.0),
                shadow,
            );
        }
        let square = screen.mode == crate::gizmo::GizmoMode::Scale;
        for handle in screen.handles.into_iter().flatten() {
            let active = hovered == Some(crate::gizmo::Handle::Axis(handle.axis));
            let color = if active {
                theme::HANDLE_ACTIVE
            } else {
                handle.axis.color()
            };
            stroke(list, handle.from, handle.to, thickness, color);
            // A cap at the tip: the thing to aim at, and what makes the end of the handle a
            // target rather than a place where a line stops. A scale handle's is a bigger
            // block, the convention every package uses to tell the two apart.
            let cap = thickness * if square { 3.4 } else { 2.4 };
            let tip = UiRect::new(handle.to.0 - cap * 0.5, handle.to.1 - cap * 0.5, cap, cap);
            if square {
                list.rect(tip.inset(-scale.px(1.0)), shadow);
            }
            list.rect(tip, color);
            list.text_at(
                (handle.to.0 + cap, handle.to.1 - cap),
                handle.axis.label(),
                color,
                scale.text(1.0),
            );
        }
    }
    // The origin, so the gizmo has a centre even when an axis points at the eye and its handle
    // is a dot. The scale gizmo's centre is a handle of its own: all three axes at once.
    let uniform = screen.mode == crate::gizmo::GizmoMode::Scale;
    let pip = thickness * if uniform { 3.6 } else { 2.0 };
    list.rect(
        UiRect::new(
            screen.origin.0 - pip * 0.5,
            screen.origin.1 - pip * 0.5,
            pip,
            pip,
        ),
        if uniform && hovered == Some(crate::gizmo::Handle::Uniform) {
            theme::HANDLE_ACTIVE
        } else {
            [1.0, 1.0, 1.0, 0.9]
        },
    );
}

/// Draws a line as a run of squares.
///
/// The draw list has rectangles and no line primitive, deliberately: it exists to describe a
/// HUD, and a HUD is rectangles. A gizmo handle is the first thing in this project that wants
/// an arbitrary direction, and a run of `thickness`-sized quads is a line at the scale these
/// are drawn at — a handle is a couple of hundred pixels, so it is a few dozen quads.
fn stroke(list: &mut UiDrawList, from: (f32, f32), to: (f32, f32), thickness: f32, color: UiColor) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt();
    let thickness = thickness.max(1.0);
    if length < 0.5 {
        list.rect(
            UiRect::new(
                from.0 - thickness * 0.5,
                from.1 - thickness * 0.5,
                thickness,
                thickness,
            ),
            color,
        );
        return;
    }
    // An axis-aligned line is one rectangle rather than a hundred, which is most of them: two
    // of the three handles are nearly axis-aligned whenever the camera is level.
    if dy.abs() < 0.5 {
        let left = from.0.min(to.0);
        list.rect(
            UiRect::new(left, from.1 - thickness * 0.5, length, thickness),
            color,
        );
        return;
    }
    if dx.abs() < 0.5 {
        let top = from.1.min(to.1);
        list.rect(
            UiRect::new(from.0 - thickness * 0.5, top, thickness, length),
            color,
        );
        return;
    }
    let steps = (length / (thickness * 0.7))
        .ceil()
        .clamp(1.0, STROKE_MAX_QUADS);
    for step in 0..=(steps as usize) {
        let t = step as f32 / steps;
        let (x, y) = (from.0 + dx * t, from.1 + dy * t);
        list.rect(
            UiRect::new(
                x - thickness * 0.5,
                y - thickness * 0.5,
                thickness,
                thickness,
            ),
            color,
        );
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_core::math::Vec3;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    use engine_core::input::InputState;
    use engine_core::math::Mat4;
    use engine_environment::EnvironmentObject;
    use engine_environment::asset::PILLAR;
    use engine_environment::object::default_collision;
    use engine_presentation::ui::input::UiInput;
    use engine_render_data::{Camera, Projection};

    /// A camera at `eye`, looking at `at`.
    fn camera(eye: Vec3, at: Vec3) -> Mat4 {
        let mut camera =
            Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
        camera.set_position(eye);
        camera
            .look_at(eye, at, Vec3::new(0.0, 1.0, 0.0))
            .expect("a valid view");
        camera.view_projection()
    }

    /// An editor with one pillar at the origin selected, seen from `eye`.
    fn selected_pillar(eye: Vec3, at: Vec3) -> (Editor, EnvironmentScene) {
        let mut scene = EnvironmentScene::with_capacity("test", 1);
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test", VIEWPORT);
        editor.select(Selected::Object(0), false);
        // One frame, so the editor is holding the matrix the overlay draws from.
        editor.update(
            &InputState::new(),
            &UiInput::new(),
            &mut scene,
            &camera(eye, at),
            VIEWPORT,
            1.0 / 60.0,
        );
        (editor, scene)
    }

    fn painted(editor: &Editor, scene: &EnvironmentScene) -> UiDrawList {
        let mut list = UiDrawList::new();
        let mut scratch = String::new();
        push_editor(
            &mut list,
            editor,
            scene,
            &MenuTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        list
    }

    /// How many quads of `color` the list holds.
    fn quads_of(list: &UiDrawList, color: UiColor) -> usize {
        list.commands()
            .iter()
            .filter(|command| {
                matches!(command, engine_render_data::ui::UiCommand::Rect { color: painted, .. } if *painted == color)
            })
            .count()
    }

    #[test]
    fn an_outline_seen_from_inside_arms_length_stays_a_handful_of_quads() {
        // A corner of the box a hair in front of the near plane projects hundreds of
        // thousands of pixels away. Stroking to it unclipped is a run of one quad per two
        // pixels - thirty thousand of them from this camera - which overran the frame's
        // upload slot and killed the renderer rather than drawing badly.
        let (editor, scene) = selected_pillar(Vec3::new(0.0, 0.5, 2.0), Vec3::ZERO);
        let list = painted(&editor, &scene);
        let quads = list.commands().len();
        assert!(
            quads < 4_000,
            "the overlay is {quads} commands from one selected object"
        );
    }

    #[test]
    fn a_selected_object_in_view_is_outlined_and_carries_a_gizmo() {
        let (editor, scene) = selected_pillar(Vec3::new(0.0, 6.0, 18.0), Vec3::ZERO);
        let list = painted(&editor, &scene);

        for axis in crate::gizmo::Axis::ALL {
            assert!(
                quads_of(&list, axis.color()) > 0,
                "the {} handle was not drawn",
                axis.label()
            );
        }
        assert!(
            quads_of(&list, SELECTION_COLOR) > 0,
            "the selection has no outline"
        );
        assert!(
            quads_of(&list, OFFSCREEN_COLOR) == 0,
            "an object in view was reported as off screen"
        );
    }

    #[test]
    fn a_selection_the_camera_has_turned_away_from_is_drawn_as_a_striped_silhouette() {
        // Looking away from the pillar, from the same place: it is behind the eye.
        let (editor, scene) = selected_pillar(Vec3::new(0.0, 6.0, 18.0), Vec3::new(0.0, 6.0, 90.0));
        let list = painted(&editor, &scene);

        let stripes = quads_of(&list, OFFSCREEN_COLOR);
        assert!(stripes > 4, "the silhouette has {stripes} stripes in it");
        assert!(
            list.text().contains("Selected:"),
            "the silhouette does not say what it is: {}",
            list.text()
        );
        assert_eq!(
            quads_of(&list, SELECTION_COLOR),
            0,
            "an object behind the camera was outlined as if it were in view"
        );
    }

    #[test]
    fn the_silhouette_stays_inside_the_window_and_moves_with_the_camera() {
        // Two cameras that have both turned away from the pillar, one to each side of it, so
        // the silhouette has a side of the window to be on.
        let (left, scene) = selected_pillar(Vec3::new(0.0, 6.0, 20.0), Vec3::new(60.0, 6.0, 60.0));
        let (right, _) = selected_pillar(Vec3::new(0.0, 6.0, 20.0), Vec3::new(-60.0, 6.0, 60.0));

        let first = silhouette_of(&painted(&left, &scene));
        let second = silhouette_of(&painted(&right, &scene));
        assert!(
            (first.0 - second.0).abs() > 1.0,
            "the silhouette sits in the same place whichever way the camera faces"
        );
        for (x, y) in [first, second] {
            assert!(
                x >= 0.0 && x <= VIEWPORT.0 as f32 && y >= 0.0 && y <= VIEWPORT.1 as f32,
                "the silhouette is off the window at {x}, {y}"
            );
        }
    }

    /// Where the striped silhouette was drawn: the middle of its stripes.
    fn silhouette_of(list: &UiDrawList) -> (f32, f32) {
        let mut sum = (0.0, 0.0);
        let mut count = 0.0;
        for command in list.commands() {
            if let engine_render_data::ui::UiCommand::Rect { rect, color } = command
                && *color == OFFSCREEN_COLOR
            {
                sum = (sum.0 + rect.x, sum.1 + rect.y);
                count += 1.0;
            }
        }
        assert!(count > 0.0, "nothing striped was drawn");
        (sum.0 / count, sum.1 / count)
    }

    #[test]
    fn every_browser_row_is_inside_the_panel_and_none_overlap() {
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test", VIEWPORT);
        let layout = editor.layout_for(VIEWPORT);
        let panel = layout.left;
        let visible = layout.browser_rows();
        assert!(visible > 4, "the panel holds {visible} rows");

        let mut previous_bottom = panel.y;
        for slot in 0..visible {
            let rect = layout.browser_row(slot);
            assert!(
                rect.x >= panel.x && rect.right() <= panel.right(),
                "{rect:?}"
            );
            assert!(
                rect.y >= previous_bottom,
                "row {slot} overlaps the one above"
            );
            previous_bottom = rect.bottom();
        }
        // The last row stops above the footer rather than running under it.
        assert!(
            previous_bottom <= layout.browser_footer.y,
            "the list runs into the footer"
        );
    }

    #[test]
    fn an_open_editor_draws_its_browser_rows() {
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test", VIEWPORT);
        let mut list = UiDrawList::new();
        let mut scratch = String::new();
        push_editor(
            &mut list,
            &editor,
            &EnvironmentScene::with_capacity("test", 0),
            &MenuTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        let text = list.text();
        assert!(text.contains("Assets"), "the heading is missing: {text}");
        assert!(text.contains("Built-in"), "the built-in folder is missing");
        assert!(
            !text.contains("Ground Plain"),
            "the builtin folder starts closed, so its assets are not drawn"
        );

        // Opened, the library is on screen.
        editor
            .browser_mut()
            .toggle(crate::browser::BrowserRow::BuiltinFolder);
        list.clear();
        push_editor(
            &mut list,
            &editor,
            &EnvironmentScene::with_capacity("test", 0),
            &MenuTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        assert!(
            list.text().contains("Ground Plain"),
            "the first library asset is not listed"
        );
    }

    #[test]
    fn a_closed_editor_draws_nothing() {
        let editor = Editor::new();
        let mut list = UiDrawList::new();
        let mut scratch = String::new();
        push_editor(
            &mut list,
            &editor,
            &EnvironmentScene::with_capacity("test", 0),
            &MenuTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        assert!(list.is_empty(), "a closed editor put something on screen");
    }

    #[test]
    fn an_open_editor_draws_its_panel_and_its_keys() {
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test", VIEWPORT);
        let mut list = UiDrawList::new();
        let mut scratch = String::new();
        push_editor(
            &mut list,
            &editor,
            &EnvironmentScene::with_capacity("test", 0),
            &MenuTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        assert!(!list.is_empty());
        for heading in ["File", "Assets", "Hierarchy", "Inspector", "Play"] {
            assert!(
                list.text().contains(heading),
                "{heading} is missing from the editor: {}",
                list.text()
            );
        }
        // And none of the old wall of keys: help is a window now, not the bottom of the screen.
        assert!(!list.text().contains("PLACING MODE: LMB"));
    }

    /// An editor sculpting a flat level, the pointer in the middle of the viewport.
    fn sculpting(mode_steps: usize) -> (Editor, EnvironmentScene) {
        let mut scene = EnvironmentScene::with_capacity("test", 0);
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test", VIEWPORT);
        editor.execute(crate::commands::Command::ToggleSculpt, &mut scene);
        let view = camera(Vec3::new(0.0, 12.0, 18.0), Vec3::ZERO);
        let pointer = (VIEWPORT.0 as f32 * 0.5, VIEWPORT.1 as f32 * 0.5);
        for _ in 0..mode_steps {
            let mut tab = InputState::new();
            tab.apply(engine_core::input::InputEvent::KeyDown {
                key: engine_core::input::KeyCode::Tab,
                repeat: false,
            });
            editor.update(
                &tab,
                &UiInput {
                    pointer,
                    ..UiInput::new()
                },
                &mut scene,
                &view,
                VIEWPORT,
                1.0 / 60.0,
            );
        }
        editor.update(
            &InputState::new(),
            &UiInput {
                pointer,
                ..UiInput::new()
            },
            &mut scene,
            &view,
            VIEWPORT,
            1.0 / 60.0,
        );
        (editor, scene)
    }

    #[test]
    fn the_brush_is_drawn_under_the_pointer_in_its_modes_colour_with_its_name() {
        use engine_environment::terrain::BrushMode;
        for (steps, mode) in [
            (0, BrushMode::Raise),
            (1, BrushMode::Lower),
            (2, BrushMode::Flatten),
            (3, BrushMode::Smooth),
        ] {
            let (editor, scene) = sculpting(steps);
            assert_eq!(editor.brush(), mode);
            let list = painted(&editor, &scene);
            assert!(
                quads_of(&list, brush_color(mode)) > 20,
                "{mode:?}: no ring in its colour"
            );
            for other in [
                BrushMode::Raise,
                BrushMode::Lower,
                BrushMode::Flatten,
                BrushMode::Smooth,
            ] {
                if other != mode {
                    assert_eq!(
                        quads_of(&list, brush_color(other)),
                        0,
                        "{mode:?} drew {other:?}'s colour"
                    );
                }
            }
            let name = crate::theme::display_name(&mode.as_str().to_ascii_lowercase());
            assert!(
                list.text().contains(&name),
                "{mode:?}: the label is missing"
            );
        }
    }

    #[test]
    fn no_brush_is_drawn_when_not_sculpting_or_over_a_panel() {
        let (mut editor, mut scene) = sculpting(0);
        let view = camera(Vec3::new(0.0, 12.0, 18.0), Vec3::ZERO);
        // Over the browser: the brush would not paint there, so it is not drawn there.
        editor.update(
            &InputState::new(),
            &UiInput {
                pointer: (60.0, 300.0),
                ..UiInput::new()
            },
            &mut scene,
            &view,
            VIEWPORT,
            1.0 / 60.0,
        );
        assert_eq!(
            quads_of(&painted(&editor, &scene), brush_color(editor.brush())),
            0
        );

        editor.execute(crate::commands::Command::ToggleSculpt, &mut scene);
        let pointer = (VIEWPORT.0 as f32 * 0.5, VIEWPORT.1 as f32 * 0.5);
        editor.update(
            &InputState::new(),
            &UiInput {
                pointer,
                ..UiInput::new()
            },
            &mut scene,
            &view,
            VIEWPORT,
            1.0 / 60.0,
        );
        assert_eq!(
            quads_of(&painted(&editor, &scene), brush_color(editor.brush())),
            0
        );
    }

    #[test]
    fn a_bigger_brush_draws_a_bigger_ring_and_a_stronger_one_a_longer_arrow() {
        let ring_width = |radius_steps: usize| {
            let (mut editor, mut scene) = sculpting(0);
            let view = camera(Vec3::new(0.0, 12.0, 18.0), Vec3::ZERO);
            let pointer = (VIEWPORT.0 as f32 * 0.5, VIEWPORT.1 as f32 * 0.5);
            for _ in 0..radius_steps {
                let mut key = InputState::new();
                key.apply(engine_core::input::InputEvent::KeyDown {
                    key: engine_core::input::KeyCode::Digit2,
                    repeat: false,
                });
                editor.update(
                    &key,
                    &UiInput {
                        pointer,
                        ..UiInput::new()
                    },
                    &mut scene,
                    &view,
                    VIEWPORT,
                    1.0 / 60.0,
                );
            }
            let list = painted(&editor, &scene);
            let color = brush_color(editor.brush());
            let xs: Vec<f32> = list
                .commands()
                .iter()
                .filter_map(|command| match command {
                    engine_render_data::ui::UiCommand::Rect { rect, color: c } if *c == color => {
                        Some(rect.x)
                    }
                    _ => None,
                })
                .collect();
            xs.iter().copied().fold(f32::MIN, f32::max)
                - xs.iter().copied().fold(f32::MAX, f32::min)
        };
        assert!(
            ring_width(2) > ring_width(0) * 1.3,
            "the ring did not grow with the brush"
        );
        assert!(brush_arrow_length(1.0) > brush_arrow_length(0.2));
    }
}
