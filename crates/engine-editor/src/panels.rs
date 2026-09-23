//! What the panels hold, for one frame: the toolbar's controls, a menu's items, the
//! hierarchy's rows, the inspector's fields and the browser's tiles — each with the rectangle
//! it occupies.
//!
//! Built from the editor and the level by pure functions, and read by both the painter in
//! [`crate::ui`] and the input code in [`crate::interaction`]. Nothing is drawn here and
//! nothing is changed: that split is what makes a control that is drawn and a control that
//! is clicked the same control, and what lets the tests ask "where is the Save button" of the
//! same code the screen does.

use engine_environment::EnvironmentScene;
use engine_environment::object::EnvironmentCollision;
use engine_render_data::ui::{UiColor, UiRect};

use crate::browser::BrowserRow;
use crate::commands::{BrowserView, Command, Pivot, Tool};
use crate::icons::Icon;
use crate::inspector::Property;
use crate::layout::Layout;
use crate::theme::{self, kind, metrics};
use crate::{Editor, GRID_STEPS, Selected, YAW_STEPS};

// ---------------------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------------------

/// Which popup a toolbar control opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    /// File.
    File,
    /// Edit.
    Edit,
    /// View.
    View,
    /// Help.
    Help,
    /// The grid size.
    Grid,
    /// The rotation step.
    Rotation,
    /// What can be done to the thing that was right-clicked.
    Object,
    /// What can be done at an empty spot in the viewport.
    Viewport,
}

/// What a toolbar control is.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolKind {
    /// A word that opens a menu: File, Edit.
    Menu(MenuKind, &'static str),
    /// An icon that runs a command.
    Button {
        /// The picture.
        icon: Icon,
        /// What it does.
        command: Command,
        /// Whether it is the current tool or a toggle that is on.
        active: bool,
        /// Whether it can be used right now.
        disabled: bool,
    },
    /// An icon and a word that runs a command: `World`, `Active`.
    Toggle {
        /// The picture.
        icon: Icon,
        /// The word.
        text: String,
        /// What it does.
        command: Command,
        /// Whether it is on.
        active: bool,
    },
    /// An icon and a value that opens a menu of values: the grid size.
    Dropdown {
        /// The picture.
        icon: Icon,
        /// The value.
        text: String,
        /// The menu of values.
        menu: MenuKind,
        /// Whether what it sets is in effect.
        active: bool,
    },
    /// Back to the game, in the accent colour.
    Play,
    /// A hairline between groups.
    Separator,
}

/// One control on the toolbar.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolItem {
    /// Where it is.
    pub rect: UiRect,
    /// What it is.
    pub kind: ToolKind,
    /// What its tooltip says.
    pub tip: &'static str,
    /// The key that does the same thing, for the tooltip.
    pub key: &'static str,
}

impl ToolItem {
    /// The command it runs when clicked, if it runs one rather than opening a menu.
    #[must_use]
    pub fn command(&self) -> Option<Command> {
        match &self.kind {
            ToolKind::Button {
                command, disabled, ..
            } if !disabled => Some(*command),
            ToolKind::Toggle { command, .. } => Some(*command),
            ToolKind::Play => Some(Command::Play),
            _ => None,
        }
    }

    /// The menu it opens, if it opens one.
    #[must_use]
    pub fn menu(&self) -> Option<MenuKind> {
        match &self.kind {
            ToolKind::Menu(menu, _) => Some(*menu),
            ToolKind::Dropdown { menu, .. } => Some(*menu),
            _ => None,
        }
    }
}

/// A grid size, written the way the toolbar writes it.
#[must_use]
pub fn grid_label(step: f32) -> String {
    if step <= 0.0 {
        "Off".to_owned()
    } else if (step - step.round()).abs() < 1e-4 {
        format!("{step:.0} m")
    } else {
        format!("{step} m")
    }
}

/// The toolbar's controls, laid out left to right, with the play button at the far right.
#[must_use]
pub fn toolbar(editor: &Editor, layout: &Layout) -> Vec<ToolItem> {
    let bar = layout.toolbar;
    let ty = layout.ty;
    let button = layout.px(metrics::BUTTON);
    let gap = layout.px(2.0);
    let y = bar.y + (bar.height - button) * 0.5;
    let mut x = bar.x + layout.px(6.0);
    let mut items = Vec::with_capacity(32);

    let push = |items: &mut Vec<ToolItem>, x: &mut f32, width: f32, kind: ToolKind, tip, key| {
        items.push(ToolItem {
            rect: UiRect::new(*x, y, width, button),
            kind,
            tip,
            key,
        });
        *x += width + gap;
    };
    let word = |text: &str| ty.width(text) + layout.px(16.0);
    let with_icon = |text: &str| ty.width(text) + layout.px(metrics::ICON_SLOT + 16.0);

    for (menu, name, tip) in [
        (MenuKind::File, "File", "Save, save as, back to the game"),
        (MenuKind::Edit, "Edit", "Undo, duplicate, delete, select"),
        (MenuKind::View, "View", "Frame, panels, debug overlay"),
        (MenuKind::Help, "Help", "Keyboard shortcuts"),
    ] {
        push(
            &mut items,
            &mut x,
            word(name),
            ToolKind::Menu(menu, name),
            tip,
            "",
        );
    }
    let separator = |items: &mut Vec<ToolItem>, x: &mut f32| {
        items.push(ToolItem {
            rect: UiRect::new(
                *x + layout.px(3.0),
                y + layout.px(4.0),
                layout.px(1.0),
                button - layout.px(8.0),
            ),
            kind: ToolKind::Separator,
            tip: "",
            key: "",
        });
        *x += layout.px(9.0);
    };
    separator(&mut items, &mut x);

    let world_mode = !editor.is_sculpting() && !editor.is_place_mode();
    for (icon, tool, tip, key) in [
        (
            Icon::Select,
            Tool::Select,
            "Select: click objects, no handles",
            "",
        ),
        (Icon::Move, Tool::Move, "Move along an axis", "1"),
        (
            Icon::Rotate,
            Tool::Turn,
            "Rotate about the vertical axis",
            "2",
        ),
        (
            Icon::Scale,
            Tool::Scale,
            "Scale along the object's own axes",
            "3",
        ),
    ] {
        items.push(ToolItem {
            rect: UiRect::new(x, y, button, button),
            kind: ToolKind::Button {
                icon,
                command: Command::SetTool(tool),
                active: world_mode && editor.tool() == tool,
                disabled: false,
            },
            tip,
            key,
        });
        x += button + gap;
    }
    separator(&mut items, &mut x);
    for (icon, command, active, tip, key) in [
        (
            Icon::Place,
            Command::TogglePlacing,
            editor.is_place_mode(),
            "Placing mode: every click puts the chosen asset down",
            "N",
        ),
        (
            Icon::Sculpt,
            Command::ToggleSculpt,
            editor.is_sculpting(),
            "Sculpt the terrain",
            "B",
        ),
    ] {
        items.push(ToolItem {
            rect: UiRect::new(x, y, button, button),
            kind: ToolKind::Button {
                icon,
                command,
                active,
                disabled: false,
            },
            tip,
            key,
        });
        x += button + gap;
    }
    separator(&mut items, &mut x);

    let space = if editor.is_local_space() {
        "Local"
    } else {
        "World"
    };
    push(
        &mut items,
        &mut x,
        with_icon("Local"),
        ToolKind::Toggle {
            icon: if editor.is_local_space() {
                Icon::Local
            } else {
                Icon::World
            },
            text: space.to_owned(),
            command: Command::ToggleSpace,
            active: false,
        },
        "Nudge along world axes or the object's own",
        "L",
    );
    push(
        &mut items,
        &mut x,
        with_icon("Center"),
        ToolKind::Toggle {
            icon: Icon::Pivot,
            text: editor.pivot().label().to_owned(),
            command: Command::TogglePivot,
            active: false,
        },
        "Pivot: the active object, or the center of the selection",
        "",
    );
    separator(&mut items, &mut x);

    items.push(ToolItem {
        rect: UiRect::new(x, y, button, button),
        kind: ToolKind::Button {
            icon: Icon::Magnet,
            command: Command::ToggleSnap,
            active: editor.is_snapping(),
            disabled: editor.is_module_snap(),
        },
        tip: "Snap to the grid",
        key: "G",
    });
    x += button + gap;
    let grid_text = if editor.is_module_snap() {
        "4 m".to_owned()
    } else {
        grid_label(editor.grid_size())
    };
    push(
        &mut items,
        &mut x,
        with_icon("0.25 m") + layout.px(10.0),
        ToolKind::Dropdown {
            icon: Icon::Grid,
            text: grid_text,
            menu: MenuKind::Grid,
            active: editor.grid() > 0.0,
        },
        "Grid size",
        "G",
    );
    push(
        &mut items,
        &mut x,
        with_icon("22.5") + layout.px(10.0),
        ToolKind::Dropdown {
            icon: Icon::Rotate,
            text: format!("{}", editor.yaw_step()),
            menu: MenuKind::Rotation,
            active: true,
        },
        "Rotation step, in degrees",
        "Shift+G",
    );
    items.push(ToolItem {
        rect: UiRect::new(x, y, button, button),
        kind: ToolKind::Button {
            icon: Icon::Cave,
            command: Command::ToggleModule,
            active: editor.is_module_snap(),
            disabled: false,
        },
        tip: "Module snap: 4 m grid and quarter turns for cave pieces",
        key: "M",
    });
    x += button + gap;
    separator(&mut items, &mut x);

    for (icon, command, disabled, tip, key) in [
        (
            Icon::Undo,
            Command::Undo,
            !editor.can_undo(),
            "Undo",
            "Ctrl+Z",
        ),
        (
            Icon::Redo,
            Command::Redo,
            !editor.can_redo(),
            "Redo",
            "Ctrl+Y",
        ),
        (Icon::Save, Command::Save, false, "Save the level", "Ctrl+S"),
    ] {
        items.push(ToolItem {
            rect: UiRect::new(x, y, button, button),
            kind: ToolKind::Button {
                icon,
                command,
                active: false,
                disabled,
            },
            tip,
            key,
        });
        x += button + gap;
    }

    // The right-hand end: help, the debug overlay, and the way back to the game.
    let play_width = with_icon("Play") + layout.px(4.0);
    let mut right = bar.right() - layout.px(6.0) - play_width;
    items.push(ToolItem {
        rect: UiRect::new(right, y, play_width, button),
        kind: ToolKind::Play,
        tip: "Back to the game, to walk through the level",
        key: "F6",
    });
    for (icon, command, active, tip, key) in [
        (
            Icon::Chart,
            Command::ToggleDebug,
            editor.shows_debug(),
            "Debug overlay",
            "F3",
        ),
        (
            Icon::Keyboard,
            Command::ToggleShortcuts,
            editor.shows_shortcuts(),
            "Keyboard shortcuts",
            "F1",
        ),
    ] {
        right -= button + gap;
        items.push(ToolItem {
            rect: UiRect::new(right, y, button, button),
            kind: ToolKind::Button {
                icon,
                command,
                active,
                disabled: false,
            },
            tip,
            key,
        });
    }
    items
}

// ---------------------------------------------------------------------------------------
// Menus
// ---------------------------------------------------------------------------------------

/// One line of a popup menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry {
    /// What it says.
    pub label: String,
    /// The key that does the same thing.
    pub key: &'static str,
    /// What it does; `None` for a heading.
    pub command: Option<Command>,
    /// Whether it can be chosen now.
    pub enabled: bool,
    /// A tick beside it, for a setting that is on.
    pub checked: bool,
    /// A hairline above it.
    pub separator: bool,
}

impl MenuEntry {
    fn new(label: impl Into<String>, key: &'static str, command: Command) -> Self {
        Self {
            label: label.into(),
            key,
            command: Some(command),
            enabled: true,
            checked: false,
            separator: false,
        }
    }

    fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    fn after_separator(mut self) -> Self {
        self.separator = true;
        self
    }
}

/// An open popup menu.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenMenu {
    /// Which menu.
    pub kind: MenuKind,
    /// Its top-left corner, before it is moved to fit the window.
    pub origin: (f32, f32),
}

/// What `kind` offers, given what is selected.
#[must_use]
pub fn menu_entries(kind: MenuKind, editor: &Editor, scene: &EnvironmentScene) -> Vec<MenuEntry> {
    let selected = !editor.selected().is_empty();
    let objects = editor.selected().iter().any(|what| what.object().is_some());
    match kind {
        MenuKind::File => vec![
            MenuEntry::new("Save", "Ctrl+S", Command::Save),
            MenuEntry::new("Save As...", "Ctrl+Shift+S", Command::SaveAs),
            MenuEntry::new("Back to the Game", "F6", Command::Play).after_separator(),
        ],
        MenuKind::Edit => vec![
            MenuEntry::new("Undo", "Ctrl+Z", Command::Undo).enabled(editor.can_undo()),
            MenuEntry::new("Redo", "Ctrl+Y", Command::Redo).enabled(editor.can_redo()),
            MenuEntry::new("Copy", "Ctrl+C", Command::Copy)
                .enabled(selected)
                .after_separator(),
            MenuEntry::new("Paste", "Ctrl+V", Command::Paste).enabled(editor.can_paste()),
            MenuEntry::new("Duplicate", "Ctrl+D", Command::Duplicate).enabled(selected),
            MenuEntry::new("Delete", "Del", Command::Delete).enabled(selected),
            MenuEntry::new("Select All", "Ctrl+A", Command::SelectAll).after_separator(),
            MenuEntry::new("Select None", "Esc", Command::SelectNone).enabled(selected),
            MenuEntry::new("Reset Transform", "0", Command::ResetTransform)
                .enabled(selected)
                .after_separator(),
            MenuEntry::new("Drop to Ground", "Ctrl+G", Command::DropToGround).enabled(selected),
            MenuEntry::new("Hide", "H", Command::Hide)
                .enabled(objects)
                .after_separator(),
            MenuEntry::new("Show All", "Shift+H", Command::ShowAll).enabled(editor.has_hidden()),
            MenuEntry::new("Lock / Unlock", "K", Command::ToggleLock).enabled(objects),
        ],
        MenuKind::View => vec![
            MenuEntry::new("Frame Selection", "F", Command::Frame),
            MenuEntry::new(
                "Assets as List",
                "",
                Command::SetBrowserView(BrowserView::List),
            )
            .checked(editor.browser_view() == BrowserView::List)
            .after_separator(),
            MenuEntry::new(
                "Assets as Tiles",
                "",
                Command::SetBrowserView(BrowserView::Tiles),
            )
            .checked(editor.browser_view() == BrowserView::Tiles),
            MenuEntry::new("Local Axes", "L", Command::ToggleSpace)
                .checked(editor.is_local_space())
                .after_separator(),
            MenuEntry::new("Snap to Grid", "", Command::ToggleSnap).checked(editor.is_snapping()),
            MenuEntry::new("Module Snap", "M", Command::ToggleModule)
                .checked(editor.is_module_snap()),
            MenuEntry::new("Debug Overlay", "F3", Command::ToggleDebug)
                .checked(editor.shows_debug())
                .after_separator(),
            MenuEntry::new("Reset Panel Layout", "", Command::ResetLayout),
        ],
        MenuKind::Help => vec![MenuEntry::new(
            "Keyboard Shortcuts",
            "F1",
            Command::ToggleShortcuts,
        )],
        MenuKind::Grid => GRID_STEPS
            .iter()
            .enumerate()
            .filter(|(_, step)| **step > 0.0)
            .map(|(index, step)| {
                MenuEntry::new(grid_label(*step), "", Command::SetGrid(index)).checked(
                    editor.is_snapping()
                        && !editor.is_module_snap()
                        && editor.grid_index() == index,
                )
            })
            .chain(std::iter::once(
                MenuEntry::new("Snap Off", "", Command::ToggleSnap)
                    .checked(!editor.is_snapping())
                    .after_separator(),
            ))
            .collect(),
        MenuKind::Rotation => YAW_STEPS
            .iter()
            .enumerate()
            .map(|(index, step)| {
                MenuEntry::new(
                    format!("{step} degrees"),
                    "",
                    Command::SetRotationStep(index),
                )
                .checked(editor.yaw_step_index() == index && !editor.is_module_snap())
            })
            .collect(),
        MenuKind::Object => {
            let spawn = matches!(editor.selection(), Some(Selected::Spawn(index)) if index > 0);
            let light = matches!(editor.selection(), Some(Selected::Light(_)));
            let single_object = matches!(editor.selection(), Some(Selected::Object(_)));
            let locked = editor
                .selection()
                .and_then(Selected::object)
                .is_some_and(|index| editor.is_locked(index));
            let mut entries = vec![
                MenuEntry::new("Focus", "F", Command::Frame),
                MenuEntry::new("Duplicate", "Ctrl+D", Command::Duplicate).after_separator(),
                MenuEntry::new("Copy", "Ctrl+C", Command::Copy),
                MenuEntry::new("Paste", "Ctrl+V", Command::Paste).enabled(editor.can_paste()),
                MenuEntry::new("Delete", "Del", Command::Delete),
                MenuEntry::new("Reset Transform", "0", Command::ResetTransform).after_separator(),
                MenuEntry::new("Drop to Ground", "Ctrl+G", Command::DropToGround),
            ];
            if single_object {
                entries.push(
                    MenuEntry::new("Select All of This Asset", "", Command::SelectSameAsset)
                        .after_separator(),
                );
                entries.push(MenuEntry::new("Show in Browser", "", Command::RevealAsset));
            }
            if objects {
                entries.push(MenuEntry::new("Hide", "H", Command::Hide).after_separator());
                entries.push(MenuEntry::new(
                    if locked { "Unlock" } else { "Lock" },
                    "K",
                    Command::ToggleLock,
                ));
            }
            if spawn {
                entries.push(
                    MenuEntry::new("Make Player Start", "", Command::MakePrimarySpawn)
                        .after_separator(),
                );
            }
            if light {
                let casts = editor
                    .selection()
                    .and_then(Selected::light)
                    .and_then(|index| scene.lights.get(index))
                    .is_some_and(|light| light.casts_shadow);
                entries.push(
                    MenuEntry::new("Casts Shadow", "", Command::ToggleShadow)
                        .checked(casts)
                        .after_separator(),
                );
            }
            entries
        }
        MenuKind::Viewport => vec![
            MenuEntry::new("Paste Here", "Ctrl+V", Command::Paste).enabled(editor.can_paste()),
            MenuEntry::new("Add Light Here", "I", Command::PutLight).after_separator(),
            MenuEntry::new("Add Water Here", "V", Command::PutWater),
            MenuEntry::new("Add Spawn Point Here", "P", Command::PutSpawn),
            MenuEntry::new("Select All", "Ctrl+A", Command::SelectAll).after_separator(),
            MenuEntry::new("Frame Level", "F", Command::Frame),
            MenuEntry::new("Show All", "Shift+H", Command::ShowAll).enabled(editor.has_hidden()),
        ],
    }
}

/// Where a menu and each of its lines sit, kept inside the window.
#[must_use]
pub fn menu_rects(
    layout: &Layout,
    origin: (f32, f32),
    entries: &[MenuEntry],
) -> (UiRect, Vec<UiRect>) {
    let ty = layout.ty;
    let row = layout.row_height();
    let gap = layout.px(7.0);
    let label = entries
        .iter()
        .map(|entry| ty.width(&entry.label))
        .fold(0.0, f32::max);
    let key = entries
        .iter()
        .map(|entry| ty.width(entry.key))
        .fold(0.0, f32::max);
    let width =
        (layout.px(metrics::ICON_SLOT + 10.0) + label + layout.px(24.0) + key + layout.px(10.0))
            .max(layout.px(150.0));
    let height = entries
        .iter()
        .map(|entry| row + if entry.separator { gap } else { 0.0 })
        .sum::<f32>()
        + layout.px(8.0);
    let x = origin
        .0
        .min(layout.window.0 as f32 - width - layout.px(2.0))
        .max(0.0);
    let y = if origin.1 + height > layout.window.1 as f32 {
        (origin.1 - height).max(0.0)
    } else {
        origin.1
    };
    let panel = UiRect::new(x.round(), y.round(), width, height);
    let mut items = Vec::with_capacity(entries.len());
    let mut top = panel.y + layout.px(4.0);
    for entry in entries {
        if entry.separator {
            top += gap;
        }
        items.push(UiRect::new(
            panel.x + layout.px(4.0),
            top,
            width - layout.px(8.0),
            row,
        ));
        top += row;
    }
    (panel, items)
}

// ---------------------------------------------------------------------------------------
// Hierarchy
// ---------------------------------------------------------------------------------------

/// A kind of thing in a level, as the hierarchy groups them.
///
/// Derived from what the level already says — an asset's folder or name — rather than from a
/// taxonomy the scene format does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    /// Floors, platforms, ramps.
    Ground,
    /// Cliff pieces.
    Cliffs,
    /// Cave pieces.
    Caves,
    /// Rocks and boulders.
    Rocks,
    /// Trees, bushes, ferns.
    Vegetation,
    /// Everything else placed: pillars, crates, braziers.
    Props,
    /// Character models.
    Characters,
    /// Bodies of water.
    Water,
    /// Spawn points.
    Spawns,
    /// Lights.
    Lights,
}

impl Group {
    /// Every group, in the order the hierarchy lists them.
    pub const ALL: [Self; 10] = [
        Self::Ground,
        Self::Cliffs,
        Self::Caves,
        Self::Rocks,
        Self::Vegetation,
        Self::Props,
        Self::Characters,
        Self::Water,
        Self::Spawns,
        Self::Lights,
    ];

    /// Its bit in a set of collapsed groups.
    #[must_use]
    pub fn bit(self) -> u16 {
        1 << (self as u16)
    }

    /// What the hierarchy calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Ground => "Ground",
            Self::Cliffs => "Cliffs",
            Self::Caves => "Caves",
            Self::Rocks => "Rocks",
            Self::Vegetation => "Vegetation",
            Self::Props => "Props",
            Self::Characters => "Characters",
            Self::Water => "Water",
            Self::Spawns => "Spawn Points",
            Self::Lights => "Lights",
        }
    }

    /// Its picture.
    #[must_use]
    pub fn icon(self) -> Icon {
        match self {
            Self::Ground => Icon::Grid,
            Self::Cliffs => Icon::Sculpt,
            Self::Caves => Icon::Cave,
            Self::Rocks => Icon::Rock,
            Self::Vegetation => Icon::Tree,
            Self::Props => Icon::Cube,
            Self::Characters => Icon::Spawn,
            Self::Water => Icon::Water,
            Self::Spawns => Icon::Spawn,
            Self::Lights => Icon::Light,
        }
    }

    /// Its tint.
    #[must_use]
    pub fn color(self) -> UiColor {
        match self {
            Self::Ground | Self::Props | Self::Characters => kind::MODEL,
            Self::Cliffs | Self::Caves | Self::Rocks => kind::STONE,
            Self::Vegetation => kind::PLANT,
            Self::Water => kind::WATER,
            Self::Spawns => kind::SPAWN,
            Self::Lights => kind::LIGHT,
        }
    }

    /// The group an asset name belongs to, by its folder and then by its name.
    #[must_use]
    pub fn of_asset(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        let leaf = lower.rsplit('/').next().unwrap_or(&lower);
        let in_folder = |folder: &str| lower.contains(&format!("/{folder}/"));
        let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| leaf.starts_with(prefix));
        if in_folder("caves") || starts(&["cave_"]) {
            Self::Caves
        } else if in_folder("cliffs") || starts(&["cliff_"]) {
            Self::Cliffs
        } else if in_folder("rocks") || starts(&["rock", "boulder", "ridge"]) {
            Self::Rocks
        } else if in_folder("vegetation")
            || starts(&["tree", "pine", "fern", "shrub", "bush", "leaf"])
        {
            Self::Vegetation
        } else if in_folder("ground")
            || starts(&["ground", "floor", "arena_floor", "platform", "ramp", "sky"])
        {
            Self::Ground
        } else if in_folder("characters") || in_folder("animations") {
            Self::Characters
        } else if in_folder("water") {
            Self::Water
        } else {
            Self::Props
        }
    }

    /// The group a thing in the level belongs to.
    #[must_use]
    pub fn of(scene: &EnvironmentScene, what: Selected) -> Self {
        match what {
            Selected::Object(index) => scene
                .objects
                .get(index)
                .map_or(Self::Props, |object| Self::of_asset(object.asset.name())),
            Selected::Water(_) => Self::Water,
            Selected::Spawn(_) => Self::Spawns,
            Selected::Light(_) => Self::Lights,
        }
    }
}

/// One line of the hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HierarchyRow {
    /// A group's heading, and how many of its things are listed.
    Group(Group, usize),
    /// One thing.
    Item(Selected),
}

/// What a thing in the level is called in the hierarchy and the inspector.
#[must_use]
pub fn item_name(scene: &EnvironmentScene, what: Selected) -> String {
    match what {
        Selected::Object(index) => scene.objects.get(index).map_or_else(
            || "Object".to_owned(),
            |object| theme::display_name(object.asset.name()),
        ),
        Selected::Water(index) => format!("Water {}", index + 1),
        Selected::Spawn(0) => "Player Start".to_owned(),
        Selected::Spawn(index) => format!("Spawn Point {}", index + 1),
        Selected::Light(index) => format!("Light {}", index + 1),
    }
}

/// The picture a thing gets in the hierarchy.
#[must_use]
pub fn item_icon(scene: &EnvironmentScene, what: Selected) -> (Icon, UiColor) {
    let group = Group::of(scene, what);
    (group.icon(), group.color())
}

// ---------------------------------------------------------------------------------------
// Inspector
// ---------------------------------------------------------------------------------------

/// A collapsible section of the inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Position, rotation, scale.
    Transform,
    /// A body of water's size and depth.
    Water,
    /// A light's reach, strength and colour.
    Light,
    /// Whether a spawn point is where the player starts.
    Spawn,
    /// The asset behind a placement and what it blocks.
    Asset,
    /// Hidden and locked.
    Visibility,
}

impl Section {
    /// Its bit in a set of collapsed sections.
    #[must_use]
    pub fn bit(self) -> u8 {
        1 << (self as u8)
    }

    /// Its heading.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Transform => "Transform",
            Self::Water => "Water",
            Self::Light => "Light",
            Self::Spawn => "Spawn",
            Self::Asset => "Asset",
            Self::Visibility => "Editor",
        }
    }
}

/// One piece of the inspector.
#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    /// Nothing is selected.
    Empty {
        /// Where.
        rect: UiRect,
    },
    /// The name of what is selected, and what it is.
    Title {
        /// Where.
        rect: UiRect,
        /// Its picture.
        icon: Icon,
        /// The picture's tint.
        tint: UiColor,
        /// Its name.
        name: String,
        /// What it is, or how many are selected.
        detail: String,
    },
    /// A section heading that folds its section away.
    Section {
        /// Where.
        rect: UiRect,
        /// Which.
        section: Section,
        /// Whether its contents are shown.
        open: bool,
    },
    /// A property row's label.
    Label {
        /// Where.
        rect: UiRect,
        /// What it says.
        text: &'static str,
    },
    /// The coloured tag in front of a number, which can be dragged to scrub it.
    Tag {
        /// Where.
        rect: UiRect,
        /// Which number it scrubs.
        property: Property,
        /// The letter.
        letter: &'static str,
        /// Its colour.
        color: UiColor,
    },
    /// A number that can be typed into or stepped with the wheel.
    Value {
        /// Where.
        rect: UiRect,
        /// Which.
        property: Property,
    },
    /// A small button that puts a row back to its default.
    Reset {
        /// Where.
        rect: UiRect,
        /// What it runs.
        command: Command,
    },
    /// A checkbox that runs a command.
    Check {
        /// Where.
        rect: UiRect,
        /// What it says.
        label: &'static str,
        /// Whether it is ticked.
        checked: bool,
        /// What clicking it runs.
        command: Command,
    },
    /// A label and a read-only value.
    Info {
        /// Where.
        rect: UiRect,
        /// The label.
        label: &'static str,
        /// The value.
        value: String,
    },
    /// A swatch of a light's colour.
    Swatch {
        /// Where.
        rect: UiRect,
        /// The colour.
        color: UiColor,
    },
    /// A button that runs a command.
    Button {
        /// Where.
        rect: UiRect,
        /// What it says.
        label: &'static str,
        /// What it runs.
        command: Command,
    },
    /// A line of guidance.
    Note {
        /// Where.
        rect: UiRect,
        /// What it says.
        text: String,
    },
}

impl Element {
    /// Where it is.
    #[must_use]
    pub fn rect(&self) -> UiRect {
        match self {
            Self::Empty { rect }
            | Self::Title { rect, .. }
            | Self::Section { rect, .. }
            | Self::Label { rect, .. }
            | Self::Tag { rect, .. }
            | Self::Value { rect, .. }
            | Self::Reset { rect, .. }
            | Self::Check { rect, .. }
            | Self::Info { rect, .. }
            | Self::Swatch { rect, .. }
            | Self::Button { rect, .. }
            | Self::Note { rect, .. } => *rect,
        }
    }
}

/// Builds the inspector top to bottom.
struct Column<'a> {
    layout: &'a Layout,
    body: UiRect,
    y: f32,
    out: Vec<Element>,
}

impl Column<'_> {
    fn row(&mut self, height: f32) -> UiRect {
        let pad = self.layout.px(metrics::PADDING + 2.0);
        let rect = UiRect::new(
            self.body.x + pad,
            self.y,
            self.body.width - pad * 2.0,
            height,
        );
        self.y += height;
        rect
    }

    fn section(&mut self, section: Section, collapsed: u8) -> bool {
        self.y += self.layout.px(4.0);
        let height = self.layout.px(metrics::ROW_HEIGHT);
        let rect = UiRect::new(self.body.x, self.y, self.body.width, height);
        self.y += height + self.layout.px(3.0);
        let open = collapsed & section.bit() == 0;
        self.out.push(Element::Section {
            rect,
            section,
            open,
        });
        open
    }

    /// A row of up to three numbers with coloured tags, and a reset button if there is one.
    fn numbers(
        &mut self,
        label: &'static str,
        cells: &[(Property, &'static str, UiColor)],
        reset: Option<Command>,
    ) {
        let layout = self.layout;
        let rect = self.row(layout.px(metrics::ROW_HEIGHT + 2.0));
        let label_width = layout.px(58.0);
        let reset_width = layout.px(18.0);
        self.out.push(Element::Label {
            rect: UiRect::new(rect.x, rect.y, label_width, rect.height),
            text: label,
        });
        let cells_x = rect.x + label_width;
        let cells_width = (rect.width - label_width - reset_width).max(0.0);
        let cell = (cells_width / 3.0).floor();
        let tag = layout.px(12.0);
        let inset = layout.px(2.0);
        for (column, (property, letter, color)) in cells.iter().enumerate() {
            let x = cells_x + column as f32 * cell;
            self.out.push(Element::Tag {
                rect: UiRect::new(x, rect.y + inset, tag, rect.height - inset * 2.0),
                property: *property,
                letter,
                color: *color,
            });
            self.out.push(Element::Value {
                rect: UiRect::new(
                    x + tag,
                    rect.y + inset,
                    cell - tag - layout.px(3.0),
                    rect.height - inset * 2.0,
                ),
                property: *property,
            });
        }
        if let Some(command) = reset {
            self.out.push(Element::Reset {
                rect: UiRect::new(rect.right() - reset_width, rect.y, reset_width, rect.height),
                command,
            });
        }
    }

    fn info(&mut self, label: &'static str, value: String) {
        let rect = self.row(self.layout.px(metrics::ROW_HEIGHT - 2.0));
        self.out.push(Element::Info { rect, label, value });
    }

    fn check(&mut self, label: &'static str, checked: bool, command: Command) {
        let rect = self.row(self.layout.px(metrics::ROW_HEIGHT));
        self.out.push(Element::Check {
            rect,
            label,
            checked,
            command,
        });
    }
}

/// Everything the inspector shows for the current selection, top to bottom, scrolled by
/// `scroll` pixels. Elements that would fall outside the panel are left out.
#[must_use]
pub fn inspector(editor: &Editor, scene: &EnvironmentScene, layout: &Layout) -> Vec<Element> {
    let body = layout.inspector_body;
    let mut column = Column {
        layout,
        body,
        y: body.y + layout.px(6.0) - editor.inspector_scroll(),
        out: Vec::with_capacity(32),
    };
    let Some(primary) = editor.selection() else {
        column.out.push(Element::Empty { rect: body });
        return column.out;
    };
    let collapsed = editor.collapsed_sections();

    let (icon, tint) = item_icon(scene, primary);
    let count = editor.selected().len();
    let detail = if count > 1 {
        format!("{count} selected")
    } else {
        Group::of(scene, primary).label().to_owned()
    };
    let rect = column.row(layout.px(24.0));
    column.out.push(Element::Title {
        rect,
        icon,
        tint,
        name: item_name(scene, primary),
        detail,
    });

    let xyz = |x: Property, y: Property, z: Property| {
        [
            (x, "X", theme::AXIS_X),
            (y, "Y", theme::AXIS_Y),
            (z, "Z", theme::AXIS_Z),
        ]
    };
    let position = xyz(
        Property::PositionX,
        Property::PositionY,
        Property::PositionZ,
    );

    if column.section(Section::Transform, collapsed) {
        column.numbers("Position", &position, None);
        match primary {
            Selected::Object(_) => {
                column.numbers(
                    "Rotation",
                    &[(Property::Yaw, "Y", theme::AXIS_Y)],
                    Some(Command::ResetProperty(Property::Yaw)),
                );
                column.numbers(
                    "Scale",
                    &xyz(Property::ScaleX, Property::ScaleY, Property::ScaleZ),
                    Some(Command::ResetProperty(Property::ScaleX)),
                );
            }
            Selected::Water(_) | Selected::Spawn(_) => {
                column.numbers(
                    if matches!(primary, Selected::Spawn(_)) {
                        "Facing"
                    } else {
                        "Rotation"
                    },
                    &[(Property::Yaw, "Y", theme::AXIS_Y)],
                    Some(Command::ResetProperty(Property::Yaw)),
                );
            }
            Selected::Light(_) => {}
        }
    }

    match primary {
        Selected::Object(index) => {
            if column.section(Section::Asset, collapsed)
                && let Some(object) = scene.objects.get(index)
            {
                let name = object.asset.name();
                column.info("Asset", theme::display_name(name));
                column.info(
                    "Source",
                    if name.contains('/') {
                        name.rsplit_once('/')
                            .map_or(name, |(folder, _)| folder)
                            .to_owned()
                    } else {
                        "Built-in library".to_owned()
                    },
                );
                column.info(
                    "Collision",
                    match object.collision {
                        EnvironmentCollision::None => "None".to_owned(),
                        EnvironmentCollision::Box { .. } => "Box".to_owned(),
                        EnvironmentCollision::Compound(shape) => format!(
                            "Compound, {} parts",
                            scene
                                .collision_shapes
                                .get(shape as usize)
                                .map_or(0, |shape| shape.boxes.len())
                        ),
                    },
                );
                let rect = column.row(layout.px(metrics::ROW_HEIGHT + 2.0));
                column.out.push(Element::Button {
                    rect: UiRect::new(
                        rect.x,
                        rect.y + layout.px(2.0),
                        layout.px(120.0).min(rect.width),
                        rect.height - layout.px(4.0),
                    ),
                    label: "Show in Browser",
                    command: Command::RevealAsset,
                });
            }
            if column.section(Section::Visibility, collapsed) {
                column.check(
                    "Hidden in the editor",
                    editor.is_hidden(index),
                    Command::ToggleHidden(index),
                );
                column.check(
                    "Locked",
                    editor.is_locked(index),
                    Command::ToggleLocked(index),
                );
            }
        }
        Selected::Water(_) => {
            if column.section(Section::Water, collapsed) {
                let dim = theme::TEXT_DIM;
                column.numbers(
                    "Size",
                    &[
                        (Property::SizeX, "W", dim),
                        (Property::SizeZ, "L", dim),
                        (Property::Depth, "D", dim),
                    ],
                    None,
                );
            }
        }
        Selected::Spawn(index) => {
            if column.section(Section::Spawn, collapsed) {
                if index == 0 {
                    column.info("Role", "Player start".to_owned());
                } else {
                    column.info("Role", "Alternative start".to_owned());
                    let rect = column.row(layout.px(metrics::ROW_HEIGHT + 2.0));
                    column.out.push(Element::Button {
                        rect: UiRect::new(
                            rect.x,
                            rect.y + layout.px(2.0),
                            layout.px(140.0).min(rect.width),
                            rect.height - layout.px(4.0),
                        ),
                        label: "Make Player Start",
                        command: Command::MakePrimarySpawn,
                    });
                }
            }
        }
        Selected::Light(index) => {
            if column.section(Section::Light, collapsed)
                && let Some(light) = scene.lights.get(index)
            {
                let dim = theme::TEXT_DIM;
                column.numbers("Range", &[(Property::Range, "M", dim)], None);
                column.numbers("Intensity", &[(Property::Intensity, "I", dim)], None);
                column.numbers(
                    "Color",
                    &[
                        (Property::Red, "R", theme::AXIS_X),
                        (Property::Green, "G", theme::AXIS_Y),
                        (Property::Blue, "B", theme::AXIS_Z),
                    ],
                    None,
                );
                let rect = column.row(layout.px(12.0));
                let peak = light.color.max_element().max(1.0);
                column.out.push(Element::Swatch {
                    rect: UiRect::new(
                        rect.x + layout.px(58.0),
                        rect.y + layout.px(2.0),
                        rect.width - layout.px(76.0),
                        layout.px(8.0),
                    ),
                    color: [
                        light.color.x / peak,
                        light.color.y / peak,
                        light.color.z / peak,
                        1.0,
                    ],
                });
                column.check("Casts shadow", light.casts_shadow, Command::ToggleShadow);
            }
        }
    }

    if count > 1 {
        let rect = column.row(layout.px(metrics::ROW_HEIGHT));
        column.out.push(Element::Note {
            rect,
            text: "A position moves every selected item".to_owned(),
        });
    }

    column.out.retain(|element| {
        let rect = element.rect();
        rect.y >= body.y - 1.0 && rect.bottom() <= body.bottom() + 1.0
    });
    column.out
}

/// How tall the inspector's content is, unscrolled — what the scroll is clamped against.
#[must_use]
pub fn inspector_height(editor: &Editor, scene: &EnvironmentScene, layout: &Layout) -> f32 {
    // Measured on a copy of the layout with a very tall body, so nothing is left out.
    let mut tall = *layout;
    tall.inspector_body.height = 100_000.0;
    let mut elements = inspector(editor, scene, &tall);
    elements.retain(|element| !matches!(element, Element::Empty { .. }));
    elements
        .iter()
        .map(|element| element.rect().bottom())
        .fold(tall.inspector_body.y, f32::max)
        - tall.inspector_body.y
        + editor.inspector_scroll()
        + layout.px(8.0)
}

// ---------------------------------------------------------------------------------------
// Browser
// ---------------------------------------------------------------------------------------

/// Which folder the tile view is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TileFolder {
    /// `assets/` itself, with the built-in library as a folder in it.
    #[default]
    Root,
    /// The assets compiled into this build.
    Builtin,
    /// A folder on disk, by index into the catalogue's folders.
    Folder(usize),
}

/// One tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    /// A folder to open.
    Open(TileFolder),
    /// An asset, as a browser row.
    Asset(BrowserRow),
}

/// What the tile view shows: the open folder's folders and then its assets — or, while a
/// search is typed, every match.
#[must_use]
pub fn tiles(editor: &Editor) -> Vec<Tile> {
    let browser = editor.browser();
    if !browser.filter().trim().is_empty() {
        return browser
            .rows()
            .iter()
            .filter(|row| !matches!(row, BrowserRow::Folder(_) | BrowserRow::BuiltinFolder))
            .map(|row| Tile::Asset(*row))
            .collect();
    }
    let catalogue = browser.catalogue();
    match editor.tile_folder() {
        TileFolder::Root => {
            let mut out = vec![Tile::Open(TileFolder::Builtin)];
            if let Some(root) = catalogue.folders().first() {
                out.extend(
                    root.children
                        .iter()
                        .map(|child| Tile::Open(TileFolder::Folder(*child))),
                );
                out.extend(
                    root.assets
                        .iter()
                        .map(|asset| Tile::Asset(BrowserRow::Entry(*asset))),
                );
            }
            out
        }
        TileFolder::Builtin => (0..engine_environment::asset::LIBRARY.len())
            .map(|index| Tile::Asset(BrowserRow::Builtin(index)))
            .collect(),
        TileFolder::Folder(index) => {
            catalogue
                .folders()
                .get(index)
                .map_or_else(Vec::new, |folder| {
                    folder
                        .children
                        .iter()
                        .map(|child| Tile::Open(TileFolder::Folder(*child)))
                        .chain(
                            folder
                                .assets
                                .iter()
                                .map(|asset| Tile::Asset(BrowserRow::Entry(*asset))),
                        )
                        .collect()
                })
        }
    }
}

/// The breadcrumb to the open folder, root first: each step's name and the folder it opens.
#[must_use]
pub fn crumbs(editor: &Editor) -> Vec<(String, TileFolder)> {
    let catalogue = editor.browser().catalogue();
    let mut path = Vec::new();
    match editor.tile_folder() {
        TileFolder::Root => {}
        TileFolder::Builtin => path.push(("Built-in".to_owned(), TileFolder::Builtin)),
        TileFolder::Folder(index) => {
            let mut current = Some(index);
            while let Some(at) = current {
                let Some(folder) = catalogue.folders().get(at) else {
                    break;
                };
                if folder.parent.is_none() {
                    break;
                }
                path.push((theme::display_name(&folder.name), TileFolder::Folder(at)));
                current = folder.parent;
            }
            path.reverse();
        }
    }
    path.insert(0, ("Assets".to_owned(), TileFolder::Root));
    path
}

/// The picture and tint of a browser row.
#[must_use]
pub fn row_icon(editor: &Editor, row: BrowserRow) -> (Icon, UiColor) {
    match row {
        BrowserRow::Folder(_) | BrowserRow::BuiltinFolder => (Icon::Folder, kind::FOLDER),
        BrowserRow::Builtin(index) => {
            let group = Group::of_asset(engine_environment::asset::LIBRARY[index].name());
            (group.icon(), group.color())
        }
        BrowserRow::Entry(index) => {
            match editor
                .browser()
                .catalogue()
                .entries()
                .get(index)
                .map(|entry| entry.kind)
            {
                Some(engine_assets::catalogue::AssetKind::Model) => {
                    let id = &editor.browser().catalogue().entries()[index].id;
                    let group = Group::of_asset(id);
                    (group.icon(), group.color())
                }
                Some(engine_assets::catalogue::AssetKind::Material) => {
                    (Icon::Material, kind::MATERIAL)
                }
                Some(engine_assets::catalogue::AssetKind::Texture) => {
                    (Icon::Texture, kind::TEXTURE)
                }
                Some(engine_assets::catalogue::AssetKind::Scene) => (Icon::Scene, kind::SCENE),
                None => (Icon::Warning, theme::WARNING),
            }
        }
    }
}

/// A short word for what kind of file a row is, for the right-hand edge of the row.
#[must_use]
pub fn row_kind(editor: &Editor, row: BrowserRow) -> &'static str {
    match row {
        BrowserRow::Entry(index) => {
            match editor
                .browser()
                .catalogue()
                .entries()
                .get(index)
                .map(|entry| entry.kind)
            {
                Some(engine_assets::catalogue::AssetKind::Material) => "Material",
                Some(engine_assets::catalogue::AssetKind::Texture) => "Texture",
                Some(engine_assets::catalogue::AssetKind::Scene) => "Level",
                _ => "",
            }
        }
        _ => "",
    }
}

/// The contextual hint the status bar gives for what the pointer and the editor are doing.
#[must_use]
pub fn hint(editor: &Editor) -> &'static str {
    if editor.is_placing() {
        "Release over the viewport to place it"
    } else if editor.is_sculpting() {
        "Drag to shape the ground  -  Tab brush  -  1 2 size  -  3 4 strength"
    } else if editor.is_place_mode() {
        "Click to place  -  Tab next asset  -  Esc to stop"
    } else if editor.gizmo_axis().is_some() {
        "Hold Ctrl to move freely without snapping"
    } else if editor.hovered().is_some() {
        "Click to select  -  Ctrl+click to add  -  right-click for actions"
    } else if editor.selection().is_some() {
        "Drag a handle  -  F focus  -  Ctrl+D duplicate  -  Del delete"
    } else {
        "Drag an asset into the viewport  -  right-drag to look  -  WASD to fly"
    }
}

/// How `pivot` reads.
#[must_use]
pub fn pivot_label(pivot: Pivot) -> &'static str {
    pivot.label()
}

// ---------------------------------------------------------------------------------------
// Small pieces of geometry the painter and the input code share
// ---------------------------------------------------------------------------------------

/// The list and tile buttons at the right of the browser's header.
#[must_use]
pub fn browser_view_buttons(layout: &Layout) -> [(UiRect, BrowserView, Icon, &'static str); 2] {
    let header = layout.browser_header;
    let side = header.height - layout.px(4.0);
    let y = header.y + layout.px(2.0);
    let right = header.right() - layout.px(4.0);
    [
        (
            UiRect::new(right - side * 2.0 - layout.px(2.0), y, side, side),
            BrowserView::List,
            Icon::List,
            "Show assets as a list",
        ),
        (
            UiRect::new(right - side, y, side, side),
            BrowserView::Tiles,
            Icon::Tiles,
            "Show assets as tiles",
        ),
    ]
}

/// The small clear button at the right end of a search field.
#[must_use]
pub fn clear_button(field: UiRect) -> UiRect {
    UiRect::new(
        field.right() - field.height,
        field.y,
        field.height,
        field.height,
    )
}

/// Where a hierarchy row's eye and lock toggles sit.
#[must_use]
pub fn row_toggles(layout: &Layout, row: UiRect) -> (UiRect, UiRect) {
    let side = layout.px(metrics::ICON_SLOT);
    let y = row.y + (row.height - side) * 0.5;
    let eye = UiRect::new(row.right() - layout.px(4.0) - side, y, side, side);
    let lock = UiRect::new(eye.x - side - layout.px(2.0), y, side, side);
    (eye, lock)
}

/// A dialog's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogButton {
    /// Save, then carry on.
    Save,
    /// Carry on without saving.
    Discard,
    /// Do nothing.
    Cancel,
    /// Close the window.
    Close,
}

/// What kind of dialog is being laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// The keyboard shortcuts window.
    Shortcuts,
    /// Save before opening another level?
    Unsaved,
    /// Name the level.
    SaveAs,
}

/// Where a dialog and its parts sit.
#[derive(Debug, Clone, PartialEq)]
pub struct Dialog {
    /// The whole dialog.
    pub panel: UiRect,
    /// Its title strip.
    pub title: UiRect,
    /// The close button in the title strip.
    pub close: UiRect,
    /// Where its content goes.
    pub body: UiRect,
    /// Its buttons, right to left: the rectangle, what it means, what it says, and whether it
    /// is the one Enter presses.
    pub buttons: Vec<(UiRect, DialogButton, &'static str, bool)>,
}

/// The layout of a dialog, centred in the window.
#[must_use]
pub fn dialog(layout: &Layout, kind: DialogKind) -> Dialog {
    let (width, height) = match kind {
        DialogKind::Shortcuts => (860.0, 500.0),
        DialogKind::Unsaved => (440.0, 150.0),
        DialogKind::SaveAs => (460.0, 140.0),
    };
    let window = (layout.window.0 as f32, layout.window.1 as f32);
    let width = layout.px(width).min(window.0 - layout.px(24.0));
    let height = layout.px(height).min(window.1 - layout.px(24.0));
    let panel = UiRect::new(
        ((window.0 - width) * 0.5).round(),
        ((window.1 - height) * 0.45).round(),
        width,
        height,
    );
    let title = UiRect::new(
        panel.x,
        panel.y,
        panel.width,
        layout.px(metrics::HEADER_HEIGHT + 4.0),
    );
    let close = UiRect::new(
        title.right() - title.height,
        title.y,
        title.height,
        title.height,
    );
    let pad = layout.px(12.0);
    let button_height = layout.px(24.0);
    let footer = if kind == DialogKind::Shortcuts {
        0.0
    } else {
        button_height + pad
    };
    let body = UiRect::new(
        panel.x + pad,
        title.bottom() + pad,
        panel.width - pad * 2.0,
        (panel.height - title.height - pad * 2.0 - footer).max(0.0),
    );
    let mut buttons = Vec::new();
    let labels: &[(DialogButton, &'static str, bool)] = match kind {
        DialogKind::Shortcuts => &[],
        DialogKind::Unsaved => &[
            (DialogButton::Save, "Save and Open", true),
            (DialogButton::Discard, "Discard", false),
            (DialogButton::Cancel, "Cancel", false),
        ],
        DialogKind::SaveAs => &[
            (DialogButton::Save, "Save", true),
            (DialogButton::Cancel, "Cancel", false),
        ],
    };
    let mut right = panel.right() - pad;
    for (button, label, primary) in labels {
        let width = layout.ty.width(label) + layout.px(24.0);
        let rect = UiRect::new(
            right - width,
            panel.bottom() - pad - button_height,
            width,
            button_height,
        );
        buttons.push((rect, *button, *label, *primary));
        right = rect.x - layout.px(8.0);
    }
    Dialog {
        panel,
        title,
        close,
        body,
        buttons,
    }
}

/// The keyboard shortcuts, grouped: what the shortcuts window lists.
pub const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Tools",
        &[
            ("1  2  3", "Move, rotate, scale"),
            ("N", "Placing mode"),
            ("B", "Sculpt terrain"),
            ("L", "Local or world axes"),
            ("G / Shift+G", "Grid size / rotation step"),
            ("M", "Module snap for caves"),
            ("Ctrl + drag", "Drag without snapping"),
        ],
    ),
    (
        "Selection",
        &[
            ("Click", "Select"),
            ("Ctrl+click", "Add or remove"),
            ("Shift+click", "Range, in the hierarchy"),
            ("Drag", "Box select"),
            ("Ctrl+A / Esc", "All / none"),
            ("F / Home", "Frame the selection"),
            ("Right-click", "Actions"),
        ],
    ),
    (
        "Editing",
        &[
            ("Arrows", "Nudge (Shift: up and down)"),
            ("R / T", "Rotate / scale a step"),
            ("0", "Reset rotation and scale"),
            ("Ctrl+D", "Duplicate"),
            ("Ctrl+C / Ctrl+V", "Copy / paste"),
            ("Del / X", "Delete"),
            ("Ctrl+G", "Drop to ground"),
            ("H / Shift+H / K", "Hide / show all / lock"),
        ],
    ),
    (
        "Level",
        &[
            ("V / P / I", "Add water / spawn / light"),
            ("Tab", "Next asset"),
            ("Ctrl+F", "Search the panel under the pointer"),
            ("F2", "Rescan assets"),
            ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
            ("Ctrl+S", "Save (Shift: save as)"),
            ("F6", "Back to the game"),
        ],
    ),
    (
        "Camera",
        &[
            ("Right-drag", "Look"),
            ("WASD / QE", "Fly / down, up"),
            ("Shift", "Fly faster"),
            ("Alt + drag", "Orbit"),
            ("Wheel", "Forward and back"),
        ],
    ),
    (
        "Terrain",
        &[
            ("Drag", "Paint"),
            ("Tab", "Raise, lower, flatten, smooth"),
            ("1 2", "Brush size"),
            ("3 4", "Brush strength"),
        ],
    ),
];
