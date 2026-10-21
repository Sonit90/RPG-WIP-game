//! How the editor's interface answers the pointer: the toolbar, menus, dialogs, splitters,
//! the content browser, the hierarchy and the inspector.
//!
//! The viewport's own gestures — selecting, the gizmo, the selection box, dropping an asset —
//! stay in the crate root, where they always were. Everything here reads the same models the
//! painter draws ([`crate::panels`]), so a control is clicked exactly where it is drawn.

use std::path::Path;

use engine_core::input::{InputState, KeyCode, MouseButton};
use engine_core::math::{Mat4, Vec3};
use engine_environment::EnvironmentScene;
use engine_presentation::ui::input::UiInput;
use engine_presentation::ui::targeting::cursor_ray;

use crate::browser::BrowserRow;
use crate::commands::{BrowserView, Command};
use crate::community::EditTarget;
use crate::layout::{Layout, Splitter};
use crate::panels::{
    self, DialogButton, DialogKind, Element, Group, HierarchyRow, MenuKind, OpenMenu, Tile,
};
use crate::theme::metrics;
use crate::{
    CLICK_SLOP_PIXELS, DOUBLE_CLICK_SECONDS, Editor, InspectorEdit, Modal, PlacementDrag, Resize,
    Scrub, Selected, Tip, field, format_value, inspector,
};

/// Which area of the interface a tooltip belongs to, so its key never collides with another's.
const TIP_TOOLBAR: u32 = 1 << 16;
/// See [`TIP_TOOLBAR`].
const TIP_BROWSER: u32 = 2 << 16;
/// See [`TIP_TOOLBAR`].
const TIP_HIERARCHY: u32 = 3 << 16;
/// See [`TIP_TOOLBAR`].
const TIP_INSPECTOR: u32 = 4 << 16;

impl Editor {
    /// Rests the tooltip on `key`, restarting the delay if the pointer moved to a new one.
    pub(crate) fn rest_tip(&mut self, key: u32, title: impl Into<String>, shortcut: &'static str) {
        let since = match self.tip.as_ref().or(self.tip_last.as_ref()) {
            Some(tip) if tip.key == key => tip.since,
            _ => self.clock,
        };
        self.tip = Some(Tip {
            key,
            title: title.into(),
            shortcut,
            since,
        });
    }

    // -----------------------------------------------------------------------------------
    // Dialogs, menus, splitters
    // -----------------------------------------------------------------------------------

    /// One frame of whichever dialog is up. Returns whether the level changed.
    pub(crate) fn update_modal(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        layout: &Layout,
    ) -> bool {
        let Some(modal) = self.modal.clone() else {
            return false;
        };
        let kind = match modal {
            Modal::Shortcuts => DialogKind::Shortcuts,
            Modal::Unsaved { .. } => DialogKind::Unsaved,
        };
        let dialog = panels::dialog(layout, kind);
        let (x, y) = ui_input.pointer;
        let mut chosen = None;
        if input.was_key_pressed(KeyCode::Escape)
            || (modal == Modal::Shortcuts && input.was_key_pressed(KeyCode::F1))
        {
            chosen = Some(DialogButton::Cancel);
        } else if input.was_key_pressed(KeyCode::Enter) {
            chosen = dialog
                .buttons
                .iter()
                .find(|(.., primary)| *primary)
                .map(|(_, button, ..)| *button);
        }
        for (rect, button, label, _) in &dialog.buttons {
            if rect.contains(x, y) {
                self.rest_tip(TIP_TOOLBAR + 900, *label, "");
                if ui_input.pointer_pressed {
                    chosen = Some(*button);
                }
            }
        }
        if ui_input.pointer_pressed && (dialog.close.contains(x, y) || !dialog.panel.contains(x, y))
        {
            chosen = Some(DialogButton::Cancel);
        }
        self.tip = None;
        let Some(button) = chosen else {
            return false;
        };
        self.modal = None;
        match (modal, button) {
            (Modal::Unsaved { path }, DialogButton::Save) => {
                self.save(scene);
                if self.is_dirty() {
                    // The save failed and the status line says why: opening now would lose
                    // the work the author just asked to keep.
                    return false;
                }
                self.queue_open(path);
            }
            (Modal::Unsaved { path }, DialogButton::Discard) => {
                self.queue_open(path);
            }
            _ => {}
        }
        false
    }

    /// Opens `kind` with its top-left at `origin`, or closes it if it is already open.
    pub(crate) fn toggle_menu(&mut self, kind: MenuKind, origin: (f32, f32)) {
        if self.menu.as_ref().is_some_and(|menu| menu.kind == kind) {
            self.menu = None;
            return;
        }
        self.menu = Some(OpenMenu { kind, origin });
        self.menu_origin = Some(origin);
    }

    /// One frame of an open menu. Returns whether the menu answered the input — anything it
    /// did not answer carries on to the rest of the editor — and whether the level changed.
    pub(crate) fn update_menu(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        layout: &Layout,
    ) -> (bool, bool) {
        let Some(menu) = self.menu.clone() else {
            return (false, false);
        };
        if input.was_key_pressed(KeyCode::Escape) {
            self.menu = None;
            return (true, false);
        }
        let entries = panels::menu_entries(menu.kind, self, scene);
        let (panel, items) = panels::menu_rects(layout, menu.origin, &entries);
        let (x, y) = ui_input.pointer;
        let top_level = matches!(
            menu.kind,
            MenuKind::File | MenuKind::Edit | MenuKind::View | MenuKind::Help
        );

        // Along the menu bar, the open menu follows the pointer from one heading to the next,
        // the way every menu bar does.
        if top_level && layout.toolbar.contains(x, y) {
            for item in panels::toolbar(self, layout) {
                if let Some(kind) = item.menu()
                    && kind != menu.kind
                    && matches!(
                        kind,
                        MenuKind::File | MenuKind::Edit | MenuKind::View | MenuKind::Help
                    )
                    && item.rect.contains(x, y)
                {
                    self.menu = Some(OpenMenu {
                        kind,
                        origin: (item.rect.x, item.rect.bottom() + layout.px(2.0)),
                    });
                    return (true, false);
                }
            }
        }

        if !ui_input.pointer_pressed && !input.mouse().was_pressed(MouseButton::Right) {
            return (true, false);
        }
        if panel.contains(x, y) {
            let hit = items.iter().position(|rect| rect.contains(x, y));
            if let Some(index) = hit
                && let Some(entry) = entries.get(index)
                && entry.enabled
                && let Some(command) = entry.command
            {
                self.menu = None;
                let changed = self.execute(command, scene);
                self.menu_origin = None;
                return (true, changed);
            }
            return (true, false);
        }
        // A press anywhere else closes the menu. On the toolbar it carries on, so clicking
        // another heading or a tool works in one click; anywhere else it is spent closing.
        self.menu = None;
        self.menu_origin = None;
        (!layout.toolbar.contains(x, y), false)
    }

    /// Dragging a splitter between panels. Returns whether one has the pointer.
    pub(crate) fn update_splitters(&mut self, ui_input: &UiInput, layout: &Layout) -> bool {
        let (x, y) = ui_input.pointer;
        if let Some(resize) = self.resizing {
            if !ui_input.pointer_down {
                self.resizing = None;
                return true;
            }
            let factor = layout.scale.factor().max(0.01);
            let (dx, dy) = ((x - resize.from.0) / factor, y - resize.from.1);
            let mut sizes = resize.start;
            match resize.splitter {
                Splitter::Left => sizes.left = resize.start.left + dx,
                Splitter::Right => sizes.right = resize.start.right - dx,
                Splitter::Split => {
                    let body = layout.right.height.max(1.0);
                    sizes.split = (resize.start.split + dy / body).clamp(0.15, 0.85);
                }
            }
            // Stored as the layout clamped them, so a splitter dragged past its end does not
            // have to be dragged all the way back before it moves again.
            let clamped = Layout::new(layout.scale, layout.window, sizes);
            sizes.left = clamped.left.width / factor;
            sizes.right = clamped.right.width / factor;
            self.panels = sizes;
            return true;
        }
        if !ui_input.pointer_pressed {
            return false;
        }
        for splitter in [Splitter::Left, Splitter::Right, Splitter::Split] {
            if layout.splitter(splitter).contains(x, y) {
                self.resizing = Some(Resize {
                    splitter,
                    from: (x, y),
                    start: self.panels,
                });
                return true;
            }
        }
        false
    }

    /// Which splitter the pointer is over or dragging, for the painter to light it.
    #[must_use]
    pub fn splitter_hover(&self) -> Option<Splitter> {
        if let Some(resize) = self.resizing {
            return Some(resize.splitter);
        }
        let layout = self.layout();
        let (x, y) = self.pointer;
        [Splitter::Left, Splitter::Right, Splitter::Split]
            .into_iter()
            .find(|splitter| layout.splitter(*splitter).contains(x, y))
    }

    // -----------------------------------------------------------------------------------
    // Toolbar
    // -----------------------------------------------------------------------------------

    /// The toolbar: tooltips, buttons and the menus they open. Returns whether the level
    /// changed.
    pub(crate) fn update_toolbar(
        &mut self,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        layout: &Layout,
    ) -> bool {
        let (x, y) = ui_input.pointer;
        if !layout.toolbar.contains(x, y) {
            return false;
        }
        let items = panels::toolbar(self, layout);
        let Some((index, item)) = items
            .iter()
            .enumerate()
            .find(|(_, item)| item.rect.contains(x, y) && !item.tip.is_empty())
        else {
            return false;
        };
        self.rest_tip(TIP_TOOLBAR + index as u32, item.tip, item.key);
        if !ui_input.pointer_pressed {
            return false;
        }
        self.tip = None;
        if let Some(menu) = item.menu() {
            self.toggle_menu(menu, (item.rect.x, item.rect.bottom() + layout.px(2.0)));
            return false;
        }
        item.command()
            .is_some_and(|command| self.execute(command, scene))
    }

    /// Opens a context menu on a right-click that did not turn the camera. Returns whether
    /// one opened.
    pub(crate) fn update_context_menu(
        &mut self,
        input: &InputState,
        scene: &EnvironmentScene,
        layout: &Layout,
    ) -> bool {
        let mouse = input.mouse();
        let (x, y) = self.pointer;
        if mouse.was_pressed(MouseButton::Right) {
            self.right_press = Some(((x, y), self.clock));
        }
        if !mouse.was_released(MouseButton::Right) {
            return false;
        }
        let Some((from, when)) = self.right_press.take() else {
            return false;
        };
        let slop = layout.px(CLICK_SLOP_PIXELS);
        if (x - from.0).abs() > slop || (y - from.1).abs() > slop || self.clock - when > 0.6 {
            // A drag: the camera turned, and that is all it was.
            return false;
        }
        if self.is_dragging() || self.sculpting || self.placing {
            return false;
        }

        let target = if layout.viewport.contains(x, y) {
            cursor_ray((x, y), &self.view_projection, self.viewport).and_then(
                |(origin, direction)| {
                    crate::pick::under_ray_where(scene, origin, direction, |what| {
                        self.pickable(what)
                    })
                },
            )
        } else if layout.hierarchy_list.contains(x, y) {
            let view = self.hierarchy_view(scene);
            (0..layout.hierarchy_rows())
                .find(|slot| layout.hierarchy_row(*slot).contains(x, y))
                .and_then(|slot| view.get(self.hierarchy_scroll + slot).copied())
                .and_then(|row| match row {
                    HierarchyRow::Item(what) => Some(what),
                    HierarchyRow::Group(..) => None,
                })
        } else {
            return false;
        };
        let kind = match target {
            Some(what) => {
                if !self.is_selected(what) {
                    self.select(what, false);
                }
                MenuKind::Object
            }
            None if layout.viewport.contains(x, y) => MenuKind::Viewport,
            None => return false,
        };
        self.cancel_manipulation();
        self.menu = Some(OpenMenu {
            kind,
            origin: (x + layout.px(2.0), y + layout.px(2.0)),
        });
        self.menu_origin = Some((x, y));
        true
    }

    // -----------------------------------------------------------------------------------
    // Content browser
    // -----------------------------------------------------------------------------------

    /// The content browser: its view buttons, its search field, and its rows or tiles.
    pub(crate) fn update_browser_panel(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        layout: &Layout,
    ) {
        let (x, y) = ui_input.pointer;
        let over = layout.left.contains(x, y);

        // A click anywhere else is done with the search, and keeps what it found.
        if ui_input.pointer_pressed
            && self.browser.is_typing()
            && !layout.browser_search.contains(x, y)
        {
            self.browser.set_typing(false);
        }
        if !over {
            return;
        }

        for (index, (rect, view, _, tip)) in
            panels::browser_view_buttons(layout).into_iter().enumerate()
        {
            if rect.contains(x, y) {
                self.rest_tip(TIP_BROWSER + index as u32, tip, "");
                if ui_input.pointer_pressed {
                    self.browser_view = view;
                }
                return;
            }
        }
        if layout.browser_search.contains(x, y) {
            self.rest_tip(TIP_BROWSER + 10, "Search every asset", "Ctrl+F");
            if ui_input.pointer_pressed {
                if !self.browser.filter().is_empty()
                    && panels::clear_button(layout.browser_search).contains(x, y)
                {
                    self.browser.set_filter("");
                    self.browser.set_typing(false);
                } else {
                    self.browser.set_typing(true);
                }
            }
            return;
        }

        match self.browser_view {
            BrowserView::List => self.update_browser_list(input, ui_input, layout),
            BrowserView::Tiles => self.update_browser_tiles(input, ui_input, layout),
        }
    }

    /// The browser as a tree of rows.
    fn update_browser_list(&mut self, input: &InputState, ui_input: &UiInput, layout: &Layout) {
        let (x, y) = ui_input.pointer;
        let visible = layout.browser_rows();
        self.browser.clamp_scroll(visible);
        if !layout.browser_list.contains(x, y) {
            return;
        }
        let wheel = input.wheel_delta();
        if wheel != 0.0 {
            let step = (wheel.abs().max(1.0) * 3.0) as i32;
            self.browser
                .scroll_by(if wheel > 0.0 { -step } else { step }, visible);
        }
        if !ui_input.pointer_pressed {
            return;
        }
        let Some(slot) = (0..visible).find(|slot| layout.browser_row(*slot).contains(x, y)) else {
            return;
        };
        let Some(row) = self.browser.row(self.browser.scroll() + slot) else {
            return;
        };
        self.press_asset(row, (x, y));
    }

    /// The browser as tiles of the open folder, with a breadcrumb above them.
    fn update_browser_tiles(&mut self, input: &InputState, ui_input: &UiInput, layout: &Layout) {
        let (x, y) = ui_input.pointer;
        if !layout.browser_list.contains(x, y) {
            return;
        }
        let tiles = panels::tiles(self);
        let (columns, rows) = layout.tile_grid();
        let total_rows = tiles.len().div_ceil(columns);
        let last = total_rows.saturating_sub(rows);
        self.tile_scroll = self.tile_scroll.min(last);
        let wheel = input.wheel_delta();
        if wheel != 0.0 {
            self.tile_scroll = if wheel > 0.0 {
                self.tile_scroll.saturating_sub(1)
            } else {
                (self.tile_scroll + 1).min(last)
            };
        }
        if !ui_input.pointer_pressed {
            return;
        }
        if layout.crumbs().contains(x, y) {
            let mut left = layout.crumbs().x + layout.px(metrics::PADDING);
            for (name, folder) in panels::crumbs(self) {
                let width = layout.ty.width(&name) + layout.px(14.0);
                if x >= left && x < left + width {
                    self.tile_folder = folder;
                    self.tile_scroll = 0;
                    return;
                }
                left += width;
            }
            return;
        }
        let first = self.tile_scroll * columns;
        for slot in 0..columns * rows {
            if !layout.tile(slot).contains(x, y) {
                continue;
            }
            match tiles.get(first + slot) {
                Some(Tile::Open(folder)) => {
                    self.tile_folder = *folder;
                    self.tile_scroll = 0;
                }
                Some(Tile::Asset(row)) => self.press_asset(*row, (x, y)),
                None => {}
            }
            return;
        }
    }

    /// A press on an asset row or tile: open a folder, open a level on a double click, or
    /// choose an asset and start carrying it into the world.
    fn press_asset(&mut self, row: BrowserRow, at: (f32, f32)) {
        let doubled = self
            .last_row_click
            .is_some_and(|(last, when)| last == row && self.clock - when <= DOUBLE_CLICK_SECONDS);
        match row {
            BrowserRow::Folder(_) | BrowserRow::BuiltinFolder | BrowserRow::MonsterFolder => {
                self.browser.toggle(row);
                self.last_row_click = None;
            }
            _ if self.browser.scene_path(row).is_some() => {
                // A level is not placed: a double click opens it.
                if doubled {
                    let path = self.browser.scene_path(row).map(Path::to_path_buf);
                    self.last_row_click = None;
                    if let Some(path) = path {
                        self.request_open(path);
                    }
                } else {
                    self.last_row_click = Some((row, self.clock));
                    self.status = "Double-click a level to open it".to_owned();
                }
            }
            _ if !self.browser.is_placeable(row) => {
                // A material or a texture: something to look at, and nothing to carry into
                // the world — carrying it would drop whatever was chosen before.
                self.last_row_click = None;
                self.status = "Materials and textures are not placed on their own".to_owned();
            }
            _ => {
                self.browser.choose_row(row);
                self.status = format!(
                    "{}: drag into the viewport, or double-click to place",
                    crate::theme::display_name(self.browser.selected_name())
                );
                // A press on a row is the *start* of a placement, and nothing has been placed
                // yet: it becomes one only when the button comes up over the world. A second
                // press on the same row inside the double-click interval means "put one in
                // front of me" instead.
                if doubled {
                    self.place_at_centre = true;
                    self.placement = None;
                    self.last_row_click = None;
                } else {
                    self.placement = Some(PlacementDrag { from: at });
                    self.last_row_click = Some((row, self.clock));
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------
    // Hierarchy
    // -----------------------------------------------------------------------------------

    /// Starts typing into the hierarchy's search field.
    pub(crate) fn start_hierarchy_filter(&mut self) {
        // Typing adds to the filter that is there rather than replacing it.
        self.hierarchy_typing = Some(field::TextField::appending(
            &self.hierarchy_filter,
            field::Charset::FileName,
        ));
    }

    /// Every thing the filter lets through, grouped, in the order the hierarchy lists them —
    /// whether or not its group is folded away.
    #[must_use]
    pub fn hierarchy_items(&self, scene: &EnvironmentScene) -> Vec<Selected> {
        let needle = self.hierarchy_filter.trim().to_ascii_lowercase();
        let mut items: Vec<(Group, Selected)> = (0..scene.objects.len())
            .map(Selected::Object)
            .chain((0..scene.water.len()).map(Selected::Water))
            .chain((0..scene.spawn_points.len()).map(Selected::Spawn))
            .chain((0..scene.lights.len()).map(Selected::Light))
            .chain((0..scene.monsters.len()).map(Selected::Monster))
            .filter(|what| {
                needle.is_empty()
                    || panels::item_name(scene, *what)
                        .to_ascii_lowercase()
                        .contains(&needle)
                    || matches!(what, Selected::Object(index)
                        if scene.objects[*index].asset.name().to_ascii_lowercase().contains(&needle))
            })
            .map(|what| (Group::of(scene, what), what))
            .collect();
        // Stable, so things keep their file order inside a group.
        items.sort_by_key(|(group, _)| *group);
        items.into_iter().map(|(_, what)| what).collect()
    }

    /// The hierarchy's lines: each group that has anything in it, and its things unless it
    /// is folded away. A search shows every group open.
    #[must_use]
    pub fn hierarchy_view(&self, scene: &EnvironmentScene) -> Vec<HierarchyRow> {
        let searching = !self.hierarchy_filter.trim().is_empty();
        let items = self.hierarchy_items(scene);
        let mut rows = Vec::with_capacity(items.len() + Group::ALL.len());
        let mut index = 0;
        while index < items.len() {
            let group = Group::of(scene, items[index]);
            let end = items[index..]
                .iter()
                .position(|what| Group::of(scene, *what) != group)
                .map_or(items.len(), |offset| index + offset);
            rows.push(HierarchyRow::Group(group, end - index));
            if searching || !self.is_group_collapsed(group) {
                rows.extend(
                    items[index..end]
                        .iter()
                        .map(|what| HierarchyRow::Item(*what)),
                );
            }
            index = end;
        }
        rows
    }

    /// The hierarchy: its search field, its groups, the eye and lock on each row, and
    /// selection with click, `CTRL`-click, `SHIFT`-click and double-click. Returns whether the
    /// level changed — only a visibility toggle changes what is drawn.
    pub(crate) fn update_hierarchy_panel(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        layout: &Layout,
    ) -> bool {
        let (x, y) = ui_input.pointer;
        if !layout.hierarchy.contains(x, y) {
            return false;
        }
        if layout.hierarchy_search.contains(x, y) {
            self.rest_tip(TIP_HIERARCHY + 1, "Search the level by name", "Ctrl+F");
            if ui_input.pointer_pressed {
                if !self.hierarchy_filter.is_empty()
                    && panels::clear_button(layout.hierarchy_search).contains(x, y)
                {
                    self.hierarchy_filter.clear();
                    self.hierarchy_typing = None;
                } else {
                    self.start_hierarchy_filter();
                }
            }
            return false;
        }

        let view = self.hierarchy_view(scene);
        let visible = layout.hierarchy_rows();
        let last = view.len().saturating_sub(visible);
        let wheel = input.wheel_delta();
        if wheel != 0.0 {
            let step = (wheel.abs().max(1.0) * 3.0) as usize;
            self.hierarchy_scroll = if wheel > 0.0 {
                self.hierarchy_scroll.saturating_sub(step)
            } else {
                (self.hierarchy_scroll + step).min(last)
            };
        }
        self.hierarchy_scroll = self.hierarchy_scroll.min(last);

        let Some(slot) = (0..visible).find(|slot| layout.hierarchy_row(*slot).contains(x, y))
        else {
            return false;
        };
        let Some(row) = view.get(self.hierarchy_scroll + slot).copied() else {
            return false;
        };
        let rect = layout.hierarchy_row(slot);
        let (eye, lock) = panels::row_toggles(layout, rect);

        match row {
            HierarchyRow::Group(group, _) => {
                let objects = self.group_objects(scene, group);
                if !objects.is_empty() && eye.contains(x, y) {
                    let any_shown = objects.iter().any(|index| !self.is_hidden(*index));
                    self.rest_tip(
                        TIP_HIERARCHY + 2,
                        if any_shown {
                            "Hide the whole group"
                        } else {
                            "Show the whole group"
                        },
                        "",
                    );
                    if ui_input.pointer_pressed {
                        for index in objects {
                            if let Some(flags) = self.flags.get_mut(index) {
                                if any_shown {
                                    *flags |= crate::HIDDEN;
                                } else {
                                    *flags &= !crate::HIDDEN;
                                }
                            }
                        }
                        let flags = &self.flags;
                        self.selected.retain(|what| {
                            what.object().is_none_or(|index| {
                                flags
                                    .get(index)
                                    .is_none_or(|flags| flags & crate::HIDDEN == 0)
                            })
                        });
                        return true;
                    }
                    return false;
                }
                if ui_input.pointer_pressed {
                    self.collapsed_groups ^= group.bit();
                }
                false
            }
            HierarchyRow::Item(what) => {
                if let Selected::Object(index) = what {
                    if eye.contains(x, y) {
                        self.rest_tip(
                            TIP_HIERARCHY + 3,
                            if self.is_hidden(index) {
                                "Show"
                            } else {
                                "Hide"
                            },
                            "H",
                        );
                        return ui_input.pointer_pressed
                            && self.execute(Command::ToggleHidden(index), scene);
                    }
                    if lock.contains(x, y) {
                        self.rest_tip(
                            TIP_HIERARCHY + 4,
                            if self.is_locked(index) {
                                "Unlock"
                            } else {
                                "Lock: clicks in the viewport pass through"
                            },
                            "K",
                        );
                        if ui_input.pointer_pressed {
                            self.execute(Command::ToggleLocked(index), scene);
                        }
                        return false;
                    }
                }
                if !ui_input.pointer_pressed {
                    return false;
                }
                let modifiers = input.modifiers();
                let doubled = self.last_hierarchy_click.is_some_and(|(last, when)| {
                    last == what && self.clock - when <= DOUBLE_CLICK_SECONDS
                });
                if doubled && !modifiers.ctrl() && !modifiers.shift() {
                    self.selected = vec![what];
                    self.frame_selection(scene);
                    self.last_hierarchy_click = None;
                    return false;
                }
                self.last_hierarchy_click = Some((what, self.clock));
                if modifiers.shift()
                    && let Some(anchor) = self.hierarchy_anchor
                    && anchor.same_kind(what)
                {
                    // A range, in the order the hierarchy shows things, of the anchor's kind.
                    let items = self.hierarchy_items(scene);
                    let from = items.iter().position(|item| *item == anchor);
                    let to = items.iter().position(|item| *item == what);
                    if let (Some(from), Some(to)) = (from, to) {
                        let (low, high) = (from.min(to), from.max(to));
                        self.selected = items[low..=high]
                            .iter()
                            .copied()
                            .filter(|item| item.same_kind(what))
                            .collect();
                        // The one clicked is the primary.
                        self.selected.retain(|item| *item != what);
                        self.selected.push(what);
                    }
                } else {
                    self.select(what, modifiers.ctrl());
                    self.hierarchy_anchor = Some(what);
                }
                self.revealed = self.selection();
                self.cancel_manipulation();
                self.status = if self.selected.len() > 1 {
                    format!("{} selected", self.selected.len())
                } else {
                    format!("Selected {}", panels::item_name(scene, what))
                };
                false
            }
        }
    }

    /// The placements in `group`.
    fn group_objects(&self, scene: &EnvironmentScene, group: Group) -> Vec<usize> {
        (0..scene.objects.len())
            .filter(|index| Group::of(scene, Selected::Object(*index)) == group)
            .collect()
    }

    /// Opens the primary selection's group and scrolls the hierarchy to it — once, when the
    /// selection changes, so a group the author folds afterwards stays folded.
    pub(crate) fn reveal_selection(&mut self, scene: &EnvironmentScene, layout: &Layout) {
        let primary = self.selection();
        if primary == self.revealed {
            return;
        }
        self.revealed = primary;
        let Some(primary) = primary else {
            return;
        };
        let group = Group::of(scene, primary);
        self.collapsed_groups &= !group.bit();
        let view = self.hierarchy_view(scene);
        let Some(index) = view
            .iter()
            .position(|row| *row == HierarchyRow::Item(primary))
        else {
            return;
        };
        let visible = layout.hierarchy_rows().max(1);
        if index < self.hierarchy_scroll {
            self.hierarchy_scroll = index.saturating_sub(1);
        } else if index >= self.hierarchy_scroll + visible {
            self.hierarchy_scroll = index + 1 - visible;
        }
    }

    // -----------------------------------------------------------------------------------
    // Inspector
    // -----------------------------------------------------------------------------------

    /// The inspector: sections that fold, numbers that are typed, stepped with the wheel or
    /// scrubbed by dragging their tag, checkboxes and buttons. Returns whether the level
    /// changed.
    pub(crate) fn update_inspector_panel(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        layout: &Layout,
    ) -> bool {
        let (x, y) = ui_input.pointer;
        let mut changed = false;

        // A scrub owns the pointer until the button comes up, wherever it wanders.
        if let Some(mut scrub) = self.scrub {
            if !ui_input.pointer_down {
                self.scrub = None;
                if !scrub.moved {
                    self.begin_inspector_edit(scene, scrub.property);
                }
                return false;
            }
            let Some(primary) = self.selection() else {
                self.scrub = None;
                return false;
            };
            let step = scrub.property.wheel_step(self.grid(), self.yaw_step());
            let pixels = layout.px(4.0).max(1.0);
            let value = scrub.start + ((x - scrub.from) / pixels).round() * step * 0.25;
            if (x - scrub.from).abs() >= layout.px(2.0) || scrub.moved {
                if !scrub.moved {
                    self.snapshot(scene);
                    scrub.moved = true;
                }
                let selection = self.selected.clone();
                if inspector::set(
                    scene,
                    self.browser.monsters(),
                    &selection,
                    primary,
                    scrub.property,
                    value,
                ) {
                    self.water_touched = !primary.is_scene_kind();
                    changed = true;
                }
                self.status = format_value(value);
            }
            self.scrub = Some(scrub);
            return changed;
        }

        if !layout.inspector.contains(x, y) {
            if ui_input.pointer_pressed && self.inspector_edit.is_some() {
                changed |= self.commit_inspector_edit(scene);
            }
            if ui_input.pointer_pressed && self.community.edit.is_some() {
                changed |= self.commit_level_edit(scene);
            }
            return changed;
        }

        let elements = panels::inspector(self, scene, layout);
        let under = elements
            .iter()
            .find(|element| element.rect().contains(x, y))
            .cloned();

        if ui_input.pointer_pressed {
            // A click anywhere commits what was being typed, as leaving a field does in every
            // program with fields in it.
            let was_editing = self.inspector_edit.as_ref().map(|edit| edit.property);
            if self.inspector_edit.is_some() {
                changed |= self.commit_inspector_edit(scene);
            }
            let was_typing = self.community.edit.as_ref().map(|edit| edit.target);
            if self.community.edit.is_some() {
                changed |= self.commit_level_edit(scene);
            }
            match under {
                Some(Element::Section { section, .. }) => {
                    self.collapsed_sections ^= section.bit();
                }
                Some(Element::Tab { tab, .. }) => {
                    changed |= self.execute(Command::SetTab(tab), scene);
                }
                Some(Element::Field { field, .. })
                    if was_typing != Some(EditTarget::Number(field)) =>
                {
                    self.begin_level_edit(scene, EditTarget::Number(field));
                }
                Some(Element::Text { target, .. })
                    if was_typing != Some(EditTarget::Text(target)) =>
                {
                    self.begin_level_edit(scene, EditTarget::Text(target));
                }
                Some(Element::Issue { index, .. }) => {
                    changed |= self.execute(Command::SelectIssue(index), scene);
                }
                Some(Element::Value { property, .. }) if was_editing != Some(property) => {
                    self.begin_inspector_edit(scene, property);
                }
                Some(Element::Tag { property, .. }) => {
                    if let Some(start) = self.selection().and_then(|primary| {
                        inspector::get(scene, self.browser.monsters(), primary, property)
                    }) {
                        self.scrub = Some(Scrub {
                            property,
                            from: x,
                            start,
                            moved: false,
                        });
                    }
                }
                Some(
                    Element::Reset { command, .. }
                    | Element::Check { command, .. }
                    | Element::Button { command, .. },
                ) => {
                    changed |= self.execute(command, scene);
                }
                _ => {}
            }
            return changed;
        }

        match &under {
            Some(Element::Tag { .. }) => {
                self.rest_tip(TIP_INSPECTOR + 1, "Drag to change, click to type", "");
            }
            Some(Element::Value { .. }) => {
                self.rest_tip(TIP_INSPECTOR + 2, "Click to type, wheel to step", "");
            }
            Some(Element::Reset { .. }) => {
                self.rest_tip(TIP_INSPECTOR + 3, "Reset", "0");
            }
            _ => {}
        }

        let wheel = input.wheel_delta();
        if wheel == 0.0 {
            return changed;
        }
        if let Some(Element::Value { property, .. }) = under
            && self.inspector_edit.is_none()
            && let Some(primary) = self.selection()
            && let Some(value) = inspector::get(scene, self.browser.monsters(), primary, property)
        {
            let step = property.wheel_step(self.grid(), self.yaw_step());
            let fast = if input.modifiers().shift() { 10.0 } else { 1.0 };
            // A run of notches on one number is one edit, not one per notch.
            let continuing = self
                .wheel_run
                .is_some_and(|(last, when)| last == property && self.clock - when < 0.6);
            if !continuing {
                self.snapshot(scene);
            }
            self.wheel_run = Some((property, self.clock));
            let selection = self.selected.clone();
            if inspector::set(
                scene,
                self.browser.monsters(),
                &selection,
                primary,
                property,
                value + wheel * step * fast,
            ) {
                self.water_touched = !primary.is_scene_kind();
                changed = true;
            }
            return changed;
        }
        // Anywhere else on the panel, the wheel scrolls it.
        let content = panels::inspector_height(self, scene, layout);
        let room = (content - layout.inspector_body.height).max(0.0);
        self.inspector_scroll =
            (self.inspector_scroll - wheel * layout.row_height() * 2.0).clamp(0.0, room);
        changed
    }

    /// Starts typing into one of the inspector's numbers.
    fn begin_inspector_edit(&mut self, scene: &EnvironmentScene, property: inspector::Property) {
        let Some(primary) = self.selection() else {
            return;
        };
        let value =
            inspector::get(scene, self.browser.monsters(), primary, property).unwrap_or(0.0);
        self.inspector_edit = Some(InspectorEdit {
            property,
            field: field::TextField::new(&format_value(value), field::Charset::Number),
        });
        self.status = "Type a number: Enter sets it, Tab moves on, Esc cancels".to_owned();
    }

    // -----------------------------------------------------------------------------------
    // Viewport hover
    // -----------------------------------------------------------------------------------

    /// What the pointer is over in the viewport, and the ground under it.
    pub(crate) fn update_hover(
        &mut self,
        ui_input: &UiInput,
        scene: &EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        over_viewport: bool,
    ) {
        if !over_viewport
            || self.gizmo_drag.is_some()
            || self.gizmo_hover.is_some()
            || self.sculpting
            || self.placing
        {
            self.hovered = None;
            self.cursor_world = if over_viewport {
                crate::ground_under(ui_input.pointer, view_projection, viewport, scene)
            } else {
                None
            };
            return;
        }
        self.cursor_world = crate::ground_under(ui_input.pointer, view_projection, viewport, scene);
        self.hovered = cursor_ray(ui_input.pointer, view_projection, viewport).and_then(
            |(origin, direction)| {
                crate::pick::under_ray_where(scene, origin, direction, |what| self.pickable(what))
            },
        );
    }

    /// Where a carried asset would land if it were dropped now, for the placement preview.
    #[must_use]
    pub fn drop_preview(&self, scene: &EnvironmentScene) -> Option<Vec3> {
        let dragging = self.placement.is_some() || self.placing;
        if !dragging
            || !self
                .layout()
                .viewport
                .contains(self.pointer.0, self.pointer.1)
        {
            return None;
        }
        self.drop_point(self.pointer, scene, &self.view_projection, self.viewport)
            .map(|point| self.snap(point))
    }
}
