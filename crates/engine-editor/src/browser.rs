//! The asset browser: what is in `assets/`, as a tree the author picks from.
//!
//! ```text
//!  ASSETS  24 MODELS            F2 rescans, F filters
//!  ▼ builtin                    the assets compiled into this build
//!      ground_plain
//!      pillar
//!  ▼ generated
//!    ▼ environment
//!      ▼ rocks
//!          rock_small           ← the one the next click places
//!          rock_medium
//!      ▶ cliffs
//!    ▶ materials
//!  ▶ characters
//! ```
//!
//! # Why the tree is flattened every frame it changes
//!
//! What the panel draws is a list of rows, and a row is either a folder or an asset. The list
//! is rebuilt when something changes it — an expansion, a filter, a rescan — and read
//! unchanged on every other frame. The alternative, walking the tree during layout and again
//! during the hit test, is two traversals that can disagree about which row is under the
//! cursor.
//!
//! # Built-ins are a folder
//!
//! The eighteen assets compiled into this build are shown as a folder called `builtin`
//! alongside whatever is on disk. They are not files and cannot be replaced by dropping
//! something over them, but they are assets the editor can place, and a browser that hid them
//! would be a browser that shows nothing at all in a checkout where nobody has run the
//! generator.
//!
//! # A filter flattens the tree
//!
//! While something is typed, folders are not drawn: what an author wants after typing `rock`
//! is every rock, not the three folders they are spread across. Clearing the filter puts the
//! tree back exactly as it was, because the expansion state is never touched by filtering.

use std::path::{Path, PathBuf};

use engine_assets::catalogue::AssetCatalogue;
use engine_core::input::{InputState, KeyCode};
use engine_core::math::Vec3;
use engine_environment::asset::LIBRARY;
use engine_environment::object::CollisionShape;
use engine_environment::object::default_collision;
use engine_environment::{CollisionBox, EnvironmentAsset, EnvironmentCollision};

/// How deep the tree is opened when a directory is first read.
///
/// Two, which is `generated/environment` — the grouping — with the leaf folders that hold the
/// files closed under it. See [`AssetBrowser::scan`].
pub const OPEN_TO_DEPTH: usize = 2;

/// One line of the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserRow {
    /// A folder, by index into the catalogue's folders.
    Folder(usize),
    /// An asset on disk, by index into the catalogue's entries.
    Entry(usize),
    /// The `builtin` folder, which has no directory behind it.
    BuiltinFolder,
    /// One of the assets compiled into this build, by index into [`LIBRARY`].
    Builtin(usize),
}

/// What the next click will place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// An asset compiled into this build.
    Builtin(usize),
    /// An asset on disk, by index into the catalogue's entries.
    Entry(usize),
}

/// The browser's state: what is on disk, what is open, and what is chosen.
#[derive(Debug)]
pub struct AssetBrowser {
    catalogue: AssetCatalogue,
    /// Whether each folder is open, indexed as the catalogue's folders are.
    expanded: Vec<bool>,
    /// Whether the `builtin` folder is open.
    builtin_expanded: bool,
    rows: Vec<BrowserRow>,
    scroll: usize,
    filter: String,
    /// Whether keystrokes are going into the filter rather than to the editor's verbs.
    typing: bool,
    selection: Selection,
    /// What each file-backed asset blocks, worked out the first time it is placed.
    ///
    /// A cache because the answer costs a sidecar read and, failing that, a `.glb` import —
    /// and because an author placing forty rocks must not pay for forty of them.
    collision: Vec<(String, CollisionShape)>,
}

/// What a placement should block.
///
/// Two answers, because they come from different places and mean different things. A built-in
/// carries an authored primitive that the library decided; a file carries a set of boxes that
/// the level has to hold, because the level is what a scene file writes.
#[derive(Debug, Clone, PartialEq)]
pub enum PlacementCollision {
    /// A primitive the caller can use as it is.
    Simple(EnvironmentCollision),
    /// Boxes the caller must add to the scene's shape table before a placement can name them.
    Shape(CollisionShape),
}

impl Default for AssetBrowser {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetBrowser {
    /// A browser over nothing, with the built-in library selected.
    ///
    /// Scanning is not done here: the editor is built at startup and the directory is read
    /// when the editor is opened, so a checkout that gains an asset between launching the
    /// game and opening the editor shows it.
    #[must_use]
    pub fn new() -> Self {
        let catalogue = AssetCatalogue::empty(default_root());
        let mut browser = Self {
            expanded: vec![true; catalogue.folders().len()],
            catalogue,
            builtin_expanded: true,
            rows: Vec::new(),
            scroll: 0,
            filter: String::new(),
            typing: false,
            selection: Selection::Builtin(0),
            collision: Vec::new(),
        };
        browser.rebuild_rows();
        browser
    }

    /// Reads `root` and opens the tree down to where the assets actually are.
    ///
    /// The rule is [`OPEN_TO_DEPTH`] plus every folder that holds nothing but folders. A
    /// content directory is two or three levels of grouping over one level of files —
    /// `generated/environment/rocks/rock_small.glb` — so opening one level shows an author a
    /// folder called `generated` and nothing else, and opening everything shows two hundred
    /// rows. Opening the grouping and leaving the leaves closed shows the *categories*, which
    /// is what somebody looking for a rock is looking for.
    ///
    /// The `builtin` folder starts closed, because it is eighteen rows of fallback in front
    /// of the content the project actually has.
    pub fn scan(&mut self, root: impl Into<PathBuf>) {
        self.catalogue = AssetCatalogue::scan(root);
        self.expanded = self.default_expansion(|_| false);
        self.builtin_expanded = false;
        self.scroll = 0;
        self.clamp_selection();
        self.rebuild_rows();
    }

    /// Which folders are open when nothing has said otherwise.
    ///
    /// `remembered` answers "was this folder open before", which a rescan uses to keep an
    /// author's tree as they left it.
    fn default_expansion(&self, remembered: impl Fn(&str) -> bool) -> Vec<bool> {
        self.catalogue
            .folders()
            .iter()
            .map(|folder| {
                // A folder holding only folders is a grouping, not a place things are: opening
                // it costs one row per child and saves a click on every one of them.
                let grouping = folder.assets.is_empty() && !folder.children.is_empty();
                remembered(&folder.path) || folder.depth <= OPEN_TO_DEPTH || grouping
            })
            .collect()
    }

    /// Reads the same directory again, keeping what is open and what is chosen.
    ///
    /// What `F2` does, and what makes "drop a file in `assets/` and it appears" true without
    /// restarting the game.
    pub fn rescan(&mut self) {
        let open: Vec<String> = self
            .catalogue
            .folders()
            .iter()
            .enumerate()
            .filter(|(index, _)| self.expanded.get(*index).copied().unwrap_or(false))
            .map(|(_, folder)| folder.path.clone())
            .collect();
        let chosen = self.selected_id();

        self.catalogue.rescan();
        self.expanded = self.default_expansion(|path| open.contains(&path.to_owned()));
        // The chosen asset by *name*, so a rescan that renumbered the entries does not move
        // the selection to whatever happens to be in that slot now.
        if let Some(id) = chosen
            && let Some(index) = self
                .catalogue
                .entries()
                .iter()
                .position(|entry| entry.id == id)
        {
            self.selection = Selection::Entry(index);
        } else {
            self.clamp_selection();
        }
        self.rebuild_rows();
    }

    /// What is on disk.
    #[must_use]
    pub fn catalogue(&self) -> &AssetCatalogue {
        &self.catalogue
    }

    /// The rows the panel draws, top to bottom.
    #[must_use]
    pub fn rows(&self) -> &[BrowserRow] {
        &self.rows
    }

    /// The first row shown in the panel.
    #[must_use]
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// What is typed in the filter.
    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Whether keystrokes are going into the filter.
    ///
    /// The editor asks before it reads its own keys: `W` means "type a w" while the filter is
    /// open and "fly forward" while it is not, and only one of those can be true.
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.typing
    }

    /// What the next click places.
    #[must_use]
    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// Whether `row` is the chosen asset.
    #[must_use]
    pub fn is_selected(&self, row: BrowserRow) -> bool {
        match (row, self.selection) {
            (BrowserRow::Builtin(row), Selection::Builtin(chosen)) => row == chosen,
            (BrowserRow::Entry(row), Selection::Entry(chosen)) => row == chosen,
            _ => false,
        }
    }

    /// Whether a folder row is open.
    #[must_use]
    pub fn is_expanded(&self, row: BrowserRow) -> bool {
        match row {
            BrowserRow::Folder(index) => self.expanded.get(index).copied().unwrap_or(false),
            BrowserRow::BuiltinFolder => self.builtin_expanded,
            _ => false,
        }
    }

    /// The id the chosen asset is placed under.
    ///
    /// A built-in is its own name; a file is its path under the assets root without the
    /// extension. Both become an [`EnvironmentAsset`] through the same interner, so a level
    /// stores one kind of name and the loader answers one kind of question.
    #[must_use]
    pub fn selected_asset(&self) -> EnvironmentAsset {
        match self.selection {
            Selection::Builtin(index) => LIBRARY[index.min(LIBRARY.len() - 1)],
            Selection::Entry(index) => match self.catalogue.entries().get(index) {
                Some(entry) => EnvironmentAsset::interned(&entry.id),
                None => LIBRARY[0],
            },
        }
    }

    /// What the chosen asset is called, for a status line.
    #[must_use]
    pub fn selected_name(&self) -> &str {
        match self.selection {
            Selection::Builtin(index) => LIBRARY[index.min(LIBRARY.len() - 1)].name(),
            Selection::Entry(index) => self
                .catalogue
                .entries()
                .get(index)
                .map_or("nothing", |entry| entry.name.as_str()),
        }
    }

    /// The chosen asset's id, if it is a file.
    fn selected_id(&self) -> Option<String> {
        match self.selection {
            Selection::Builtin(_) => None,
            Selection::Entry(index) => self
                .catalogue
                .entries()
                .get(index)
                .map(|entry| entry.id.clone()),
        }
    }

    /// What the chosen asset blocks when it is placed.
    ///
    /// ```text
    ///  a built-in            the library's authored primitive
    ///  a file with a sidecar the boxes it names: the solid parts of a hollow piece
    ///  a file without one    a box around its geometry
    /// ```
    ///
    /// A built-in has an authored answer no bounding box could produce — a pine collides with
    /// its trunk and not its canopy. A file may have one too, in the `.collision.json` beside
    /// it, and that is what makes a cliff overhang something a player can walk under and a
    /// cave something a player can walk into: the sidecar says which parts are solid, and the
    /// space between them is the cavity. Everything else falls back to the model's bounds,
    /// which is right for a rock and a crate.
    ///
    /// A sidecar that is present and will not parse is reported and the asset falls back to
    /// its bounds — the asset is still placeable, and the log says why it is solid.
    pub fn selected_collision(&mut self) -> PlacementCollision {
        let Selection::Entry(index) = self.selection else {
            return PlacementCollision::Simple(default_collision(self.selected_asset()));
        };
        let Some(entry) = self.catalogue.entries().get(index) else {
            return PlacementCollision::Simple(EnvironmentCollision::None);
        };
        let id = entry.id.clone();
        if let Some((_, known)) = self.collision.iter().find(|(name, _)| *name == id) {
            return PlacementCollision::Shape(known.clone());
        }
        let path = entry.path.clone();
        let shape = authored_collision(&path).unwrap_or_else(|| bounds_collision(&path));
        self.collision.push((id, shape.clone()));
        PlacementCollision::Shape(shape)
    }

    /// Where the file behind the chosen asset is, if there is one.
    #[must_use]
    pub fn selected_path(&self) -> Option<&Path> {
        match self.selection {
            Selection::Builtin(_) => None,
            Selection::Entry(index) => self
                .catalogue
                .entries()
                .get(index)
                .map(|entry| entry.path.as_path()),
        }
    }

    /// Chooses the next placeable asset, wrapping. What `TAB` does.
    ///
    /// It walks everything that *exists*, not everything that is on screen, and opens the
    /// folders down to whatever it lands on. Walking the visible rows instead would make TAB
    /// do nothing at all in a browser whose folders are closed, which is how the browser
    /// starts.
    pub fn select_next(&mut self) {
        let placeable = self.placeable_assets();
        if placeable.is_empty() {
            return;
        }
        let current = placeable.iter().position(|item| *item == self.selection);
        let next = current.map_or(0, |index| (index + 1) % placeable.len());
        self.selection = placeable[next];
        self.reveal_selection();
    }

    /// Opens or closes a folder row.
    pub fn toggle(&mut self, row: BrowserRow) {
        match row {
            BrowserRow::Folder(index) => {
                if let Some(open) = self.expanded.get_mut(index) {
                    *open = !*open;
                }
            }
            BrowserRow::BuiltinFolder => self.builtin_expanded = !self.builtin_expanded,
            _ => return,
        }
        self.rebuild_rows();
    }

    /// Chooses an asset row. Returns whether it changed the selection.
    ///
    /// A row that is not a *model* is not chosen: a material and a texture are things to look
    /// at in the browser, and placing one in a level is not a thing that means anything yet.
    pub fn choose(&mut self, row: BrowserRow) -> bool {
        let selection = match row {
            BrowserRow::Builtin(index) => Selection::Builtin(index),
            BrowserRow::Entry(index) => {
                let placeable = self
                    .catalogue
                    .entries()
                    .get(index)
                    .is_some_and(|entry| entry.kind.is_placeable());
                if !placeable {
                    return false;
                }
                Selection::Entry(index)
            }
            _ => return false,
        };
        let changed = selection != self.selection;
        self.selection = selection;
        changed
    }

    /// Chooses the asset a level calls `name`, opens the folders down to it and scrolls it
    /// into view. Returns whether the browser has it.
    pub fn reveal_named(&mut self, name: &str) -> bool {
        let found = LIBRARY
            .iter()
            .position(|asset| asset.name() == name)
            .map(Selection::Builtin)
            .or_else(|| {
                self.catalogue
                    .entries()
                    .iter()
                    .position(|entry| entry.id == name)
                    .map(Selection::Entry)
            });
        let Some(selection) = found else {
            return false;
        };
        if !self.filter.is_empty() {
            self.filter.clear();
        }
        self.selection = selection;
        self.reveal_selection();
        true
    }

    /// Chooses `row` if it is placeable, as a click on it does.
    pub fn choose_row(&mut self, row: BrowserRow) -> bool {
        self.choose(row)
    }

    /// Replaces the filter, as typing into the search field does.
    pub fn set_filter(&mut self, filter: &str) {
        if self.filter != filter {
            self.filter = filter.to_owned();
            self.rebuild_rows();
        }
    }

    /// Starts or stops the filter owning the keyboard.
    pub fn set_typing(&mut self, typing: bool) {
        self.typing = typing;
    }

    /// Whether the built-in folder is open.
    #[must_use]
    pub fn is_builtin_open(&self) -> bool {
        self.builtin_expanded
    }

    /// How far a row is indented in the tree, in levels.
    #[must_use]
    pub fn depth(&self, row: BrowserRow) -> usize {
        if !self.filter.trim().is_empty() {
            // A filtered list is flat: every match at the same level, folders gone.
            return 0;
        }
        match row {
            BrowserRow::BuiltinFolder => 0,
            BrowserRow::Builtin(_) => 1,
            BrowserRow::Folder(index) => self
                .catalogue
                .folders()
                .get(index)
                .map_or(0, |folder| folder.depth.saturating_sub(1)),
            BrowserRow::Entry(index) => self
                .catalogue
                .entries()
                .get(index)
                .and_then(|entry| self.catalogue.folders().get(entry.folder))
                .map_or(0, |folder| folder.depth),
        }
    }

    /// What a row is called, as a person reads it: a folder's name, an asset's.
    #[must_use]
    pub fn name(&self, row: BrowserRow) -> String {
        match row {
            BrowserRow::BuiltinFolder => "Built-in".to_owned(),
            BrowserRow::Builtin(index) => {
                crate::theme::display_name(LIBRARY[index.min(LIBRARY.len() - 1)].name())
            }
            BrowserRow::Folder(index) => self
                .catalogue
                .folders()
                .get(index)
                .map_or_else(String::new, |folder| {
                    crate::theme::display_name(&folder.name)
                }),
            BrowserRow::Entry(index) => self
                .catalogue
                .entries()
                .get(index)
                .map_or_else(String::new, |entry| crate::theme::display_name(&entry.name)),
        }
    }

    /// The selection a row would make, if it is an asset row.
    #[must_use]
    pub fn selection_of(row: BrowserRow) -> Option<Selection> {
        match row {
            BrowserRow::Builtin(index) => Some(Selection::Builtin(index)),
            BrowserRow::Entry(index) => Some(Selection::Entry(index)),
            _ => None,
        }
    }

    /// Whether a row can be placed in a level: a built-in, or a model file.
    #[must_use]
    pub fn is_placeable(&self, row: BrowserRow) -> bool {
        match row {
            BrowserRow::Builtin(_) => true,
            BrowserRow::Entry(index) => self
                .catalogue
                .entries()
                .get(index)
                .is_some_and(|entry| entry.kind.is_placeable()),
            BrowserRow::Folder(_) | BrowserRow::BuiltinFolder => false,
        }
    }

    /// The file behind a row, if the row is a level.
    #[must_use]
    pub fn scene_path(&self, row: BrowserRow) -> Option<&Path> {
        let BrowserRow::Entry(index) = row else {
            return None;
        };
        self.catalogue
            .entries()
            .get(index)
            .filter(|entry| entry.kind == engine_assets::catalogue::AssetKind::Scene)
            .map(|entry| entry.path.as_path())
    }

    /// The row at `index` in the visible list.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<BrowserRow> {
        self.rows.get(index).copied()
    }

    /// What a row says, and how far it is indented.
    #[must_use]
    pub fn label(&self, row: BrowserRow) -> (usize, String) {
        match row {
            BrowserRow::BuiltinFolder => (
                0,
                format!("{} builtin", if self.builtin_expanded { "-" } else { "+" }),
            ),
            BrowserRow::Builtin(index) => {
                (1, LIBRARY[index.min(LIBRARY.len() - 1)].name().to_owned())
            }
            BrowserRow::Folder(index) => match self.catalogue.folders().get(index) {
                Some(folder) => (
                    folder.depth.saturating_sub(1),
                    format!(
                        "{} {}",
                        if self.expanded.get(index).copied().unwrap_or(false) {
                            "-"
                        } else {
                            "+"
                        },
                        folder.name
                    ),
                ),
                None => (0, String::new()),
            },
            BrowserRow::Entry(index) => match self.catalogue.entries().get(index) {
                Some(entry) => {
                    let depth = self
                        .catalogue
                        .folders()
                        .get(entry.folder)
                        .map_or(0, |folder| folder.depth);
                    let label = if entry.kind.is_placeable() {
                        entry.name.clone()
                    } else {
                        format!("{} [{}]", entry.name, entry.kind.as_str())
                    };
                    (depth, label)
                }
                None => (0, String::new()),
            },
        }
    }

    /// Scrolls by whole rows, stopping at both ends.
    pub fn scroll_by(&mut self, rows: i32, visible: usize) {
        let last = self.rows.len().saturating_sub(visible);
        let scrolled = i64::from(rows) + self.scroll as i64;
        self.scroll = scrolled.clamp(0, last as i64) as usize;
    }

    /// Keeps the scroll inside the list after it changed length.
    pub fn clamp_scroll(&mut self, visible: usize) {
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(visible));
    }

    /// One frame of keyboard input aimed at the browser.
    ///
    /// Returns whether the browser answered it, which is what stops a key from also meaning
    /// something to the editor behind it.
    pub fn update_keys(&mut self, input: &InputState) -> bool {
        if input.was_key_pressed(KeyCode::F2) {
            self.rescan();
            return true;
        }
        if !self.typing {
            // CTRL+F, the find every program has. A plain `F` frames the selection.
            if input.modifiers().ctrl() && input.was_key_pressed(KeyCode::F) {
                self.typing = true;
                return true;
            }
            return false;
        }

        // Typing. Everything is the filter's until it is closed, including the keys that
        // would otherwise fly the camera.
        if input.was_key_pressed(KeyCode::Escape) {
            self.typing = false;
            if !self.filter.is_empty() {
                self.filter.clear();
                self.rebuild_rows();
            }
            return true;
        }
        if input.was_key_pressed(KeyCode::Enter)
            || (input.modifiers().ctrl() && input.was_key_pressed(KeyCode::F))
        {
            self.typing = false;
            return true;
        }
        if input.was_key_pressed(KeyCode::Backspace) {
            self.filter.pop();
            self.rebuild_rows();
            return true;
        }
        let mut typed = false;
        for (key, character) in typable() {
            if input.was_key_pressed(key) {
                self.filter.push(character);
                typed = true;
            }
        }
        if typed {
            self.rebuild_rows();
        }
        true
    }

    /// Rebuilds the visible row list from the tree, the expansion state and the filter.
    fn rebuild_rows(&mut self) {
        self.rows.clear();
        if !self.filter.trim().is_empty() {
            let needle = self.filter.trim().to_ascii_lowercase();
            for (index, asset) in LIBRARY.into_iter().enumerate() {
                if asset.name().to_ascii_lowercase().contains(&needle) {
                    self.rows.push(BrowserRow::Builtin(index));
                }
            }
            for index in self.catalogue.search(&needle) {
                self.rows.push(BrowserRow::Entry(index));
            }
            self.scroll = 0;
            return;
        }

        self.rows.push(BrowserRow::BuiltinFolder);
        if self.builtin_expanded {
            for index in 0..LIBRARY.len() {
                self.rows.push(BrowserRow::Builtin(index));
            }
        }
        // The catalogue's root folder is not drawn — it is `assets/` itself, and a row saying
        // so would be a row that can be collapsed to hide everything.
        let children = self
            .catalogue
            .folders()
            .first()
            .map(|root| root.children.clone())
            .unwrap_or_default();
        let assets = self
            .catalogue
            .folders()
            .first()
            .map(|root| root.assets.clone())
            .unwrap_or_default();
        for index in assets {
            self.rows.push(BrowserRow::Entry(index));
        }
        for child in children {
            self.push_folder(child);
        }
    }

    /// Adds a folder row and, if it is open, everything under it.
    fn push_folder(&mut self, index: usize) {
        self.rows.push(BrowserRow::Folder(index));
        if !self.expanded.get(index).copied().unwrap_or(false) {
            return;
        }
        let Some(folder) = self.catalogue.folders().get(index) else {
            return;
        };
        let assets = folder.assets.clone();
        let children = folder.children.clone();
        for asset in assets {
            self.rows.push(BrowserRow::Entry(asset));
        }
        for child in children {
            self.push_folder(child);
        }
    }

    /// Every asset that could be placed, whether or not its folder is open.
    ///
    /// The built-in library first, then the catalogue in the order it lists things, so `TAB`
    /// walks a stable sequence that does not change when a folder is opened or closed.
    fn placeable_assets(&self) -> Vec<Selection> {
        let builtins = (0..LIBRARY.len()).map(Selection::Builtin);
        let entries = self
            .catalogue
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.kind.is_placeable())
            .map(|(index, _)| Selection::Entry(index));
        builtins.chain(entries).collect()
    }

    /// Opens whatever the chosen asset is inside and scrolls it into view.
    ///
    /// A selection that cannot be seen is a selection an author cannot check, and `TAB` is
    /// most useful exactly when they have not opened the folder yet.
    fn reveal_selection(&mut self) {
        match self.selection {
            Selection::Builtin(_) => self.builtin_expanded = true,
            Selection::Entry(index) => {
                let mut folder = self
                    .catalogue
                    .entries()
                    .get(index)
                    .map(|entry| entry.folder);
                while let Some(current) = folder {
                    if let Some(open) = self.expanded.get_mut(current) {
                        *open = true;
                    }
                    folder = self
                        .catalogue
                        .folders()
                        .get(current)
                        .and_then(|folder| folder.parent);
                }
            }
        }
        self.rebuild_rows();

        let Some(position) = self.rows.iter().position(|row| self.is_selected(*row)) else {
            return;
        };
        // Only scrolled when it is above the window; how far *below* it is depends on how many
        // rows the panel can show, which is the painter's business rather than the tree's.
        if position < self.scroll {
            self.scroll = position;
        }
    }

    /// Puts the selection back on something that exists after a rescan.
    fn clamp_selection(&mut self) {
        if let Selection::Entry(index) = self.selection
            && self.catalogue.entries().get(index).is_none()
        {
            self.selection = Selection::Builtin(0);
        }
    }
}

/// Where the browser looks when nothing said otherwise.
///
/// The same search order the model loader uses, so what the editor lists is what the game
/// finds: an explicit `MYGAME_ASSETS`, then the directory beside the executable, then the
/// working directory.
#[must_use]
pub fn default_root() -> PathBuf {
    if let Some(root) = std::env::var_os("MYGAME_ASSETS") {
        return PathBuf::from(root);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(directory) = exe.parent()
        && directory.join("assets").is_dir()
    {
        return directory.join("assets");
    }
    if let Ok(working) = std::env::current_dir()
        && working.join("assets").is_dir()
    {
        return working.join("assets");
    }
    PathBuf::from("assets")
}

/// What the sidecar beside a model says it blocks, if there is one.
///
/// `None` means "nobody said", and the caller falls back to the geometry's bounds. A sidecar
/// that is there and will not parse is reported and treated the same way: the asset stays
/// placeable, and the log says why it came out solid.
fn authored_collision(path: &Path) -> Option<CollisionShape> {
    match engine_assets::collision_file::CollisionFile::beside(path) {
        Ok(shape) => shape,
        Err(error) => {
            log::warn!(
                "editor: the collision sidecar beside {} will not load, so a box around its \
                 geometry is used instead: {error}",
                path.display()
            );
            None
        }
    }
}

/// A box around everything in a model's level-0 geometry.
///
/// An empty shape for a file that will not import, which is reported rather than guessed at:
/// an asset nobody can load gets no collision, and the marker cube standing in its place is
/// what tells the author to look at it.
fn bounds_collision(path: &Path) -> CollisionShape {
    let limits = engine_assets::ValidationLimits::default();
    let name = path
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let model = match engine_assets::disk::load_glb(&name, path, &limits) {
        Ok(model) => model,
        Err(error) => {
            log::warn!("editor: '{name}' has no collision because it did not load: {error}");
            return CollisionShape::default();
        }
    };
    let Some(lod) = model.lod0() else {
        return CollisionShape::default();
    };
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for submesh in &lod.submeshes {
        for position in submesh.geometry.positions() {
            min = min.min(position);
            max = max.max(position);
        }
    }
    if min.x > max.x {
        return CollisionShape::default();
    }
    // A model with no thickness in some axis — a water plane, a decal card — is decoration.
    // `CollisionShape::new` drops the box, which is what makes that true without a branch.
    CollisionShape::new([CollisionBox::new((min + max) * 0.5, (max - min) * 0.5)])
}

/// The keys that put a character into the filter.
///
/// Letters, digits and the underscore, which is what asset names are made of. There is no
/// text-input event in this engine's input state — it reports keys — so this is the mapping,
/// in one place, rather than a `match` at the call site.
fn typable() -> impl Iterator<Item = (KeyCode, char)> {
    const LETTERS: [(KeyCode, char); 26] = [
        (KeyCode::A, 'a'),
        (KeyCode::B, 'b'),
        (KeyCode::C, 'c'),
        (KeyCode::D, 'd'),
        (KeyCode::E, 'e'),
        (KeyCode::F, 'f'),
        (KeyCode::G, 'g'),
        (KeyCode::H, 'h'),
        (KeyCode::I, 'i'),
        (KeyCode::J, 'j'),
        (KeyCode::K, 'k'),
        (KeyCode::L, 'l'),
        (KeyCode::M, 'm'),
        (KeyCode::N, 'n'),
        (KeyCode::O, 'o'),
        (KeyCode::P, 'p'),
        (KeyCode::Q, 'q'),
        (KeyCode::R, 'r'),
        (KeyCode::S, 's'),
        (KeyCode::T, 't'),
        (KeyCode::U, 'u'),
        (KeyCode::V, 'v'),
        (KeyCode::W, 'w'),
        (KeyCode::X, 'x'),
        (KeyCode::Y, 'y'),
        (KeyCode::Z, 'z'),
    ];
    const DIGITS: [(KeyCode, char); 10] = [
        (KeyCode::Digit0, '0'),
        (KeyCode::Digit1, '1'),
        (KeyCode::Digit2, '2'),
        (KeyCode::Digit3, '3'),
        (KeyCode::Digit4, '4'),
        (KeyCode::Digit5, '5'),
        (KeyCode::Digit6, '6'),
        (KeyCode::Digit7, '7'),
        (KeyCode::Digit8, '8'),
        (KeyCode::Digit9, '9'),
    ];
    LETTERS.into_iter().chain(DIGITS)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn tree(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join("mygame_browser_tests").join(name);
        std::fs::remove_dir_all(&root).ok();
        for file in files {
            let path = root.join(file);
            if let Some(directory) = path.parent() {
                std::fs::create_dir_all(directory).expect("a directory");
            }
            std::fs::write(&path, b"not a real asset").expect("a file");
        }
        root
    }

    fn keyed(key: KeyCode) -> InputState {
        let mut input = InputState::new();
        input.apply(engine_core::input::InputEvent::KeyDown { key, repeat: false });
        input
    }

    /// CTRL+F: what opens the filter.
    fn find() -> InputState {
        let mut input = InputState::new();
        input.apply(engine_core::input::InputEvent::KeyDown {
            key: KeyCode::LeftControl,
            repeat: false,
        });
        input.begin_frame();
        input.apply(engine_core::input::InputEvent::KeyDown {
            key: KeyCode::F,
            repeat: false,
        });
        input
    }

    fn browser_over(root: &Path) -> AssetBrowser {
        let mut browser = AssetBrowser::new();
        browser.scan(root);
        browser
    }

    #[test]
    fn an_empty_assets_directory_still_offers_the_builtin_library() {
        let root = tree("empty", &[]);
        std::fs::create_dir_all(&root).expect("the directory");
        let mut browser = browser_over(&root);

        // One row: the closed `builtin` folder. Opening it is the whole library.
        assert_eq!(browser.rows(), [BrowserRow::BuiltinFolder]);
        browser.toggle(BrowserRow::BuiltinFolder);
        assert_eq!(browser.rows().len(), LIBRARY.len() + 1);
        assert_eq!(browser.selected_asset(), LIBRARY[0]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_categories_of_a_content_directory_are_visible_without_a_click() {
        // The shape the generator writes: two levels of grouping over the folders that hold
        // the files. An author opening the editor must see `rocks` and `caves` without
        // hunting for them.
        let root = tree(
            "categories",
            &[
                "generated/environment/rocks/rock_small.glb",
                "generated/environment/caves/cave_entrance.glb",
                "generated/environment/cliffs/cliff_overhang.glb",
                "generated/materials/grass.json",
            ],
        );
        let browser = browser_over(&root);

        let visible: Vec<String> = browser
            .rows()
            .iter()
            .map(|row| browser.label(*row).1)
            .collect();
        for wanted in [
            "- generated",
            "- environment",
            "+ rocks",
            "+ caves",
            "+ cliffs",
        ] {
            assert!(
                visible.iter().any(|label| label == wanted),
                "{wanted} is not on screen: {visible:?}"
            );
        }
        assert!(
            visible.iter().all(|label| label != "rock_small"),
            "a leaf folder opened itself: {visible:?}"
        );
        assert!(
            browser.rows().len() < 20,
            "the tree opened far enough to need scrolling: {visible:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_folder_opens_and_closes_and_takes_its_contents_with_it() {
        let root = tree(
            "expand",
            &[
                "generated/environment/rocks/a.glb",
                "generated/environment/rocks/b.glb",
            ],
        );
        let mut browser = browser_over(&root);

        // The leaf folder that holds the files starts closed.
        let rows = browser.rows().to_vec();
        let rocks = rows
            .iter()
            .find(|row| {
                matches!(row, BrowserRow::Folder(index)
                if browser.catalogue().folders()[*index].name == "rocks")
            })
            .copied()
            .expect("the rocks folder is a row");
        assert!(!browser.is_expanded(rocks));
        assert!(
            !rows.iter().any(|row| matches!(row, BrowserRow::Entry(_))),
            "a closed folder is showing its contents"
        );

        browser.toggle(rocks);
        assert!(browser.is_expanded(rocks));
        assert_eq!(
            browser
                .rows()
                .iter()
                .filter(|row| matches!(row, BrowserRow::Entry(_)))
                .count(),
            2
        );

        browser.toggle(rocks);
        assert!(
            !browser
                .rows()
                .iter()
                .any(|row| matches!(row, BrowserRow::Entry(_))),
            "closing the folder left its contents on screen"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn choosing_an_asset_makes_it_what_the_next_click_places() {
        let root = tree("choose", &["props/pillar_tall.glb"]);
        let mut browser = browser_over(&root);
        let row = browser
            .rows()
            .iter()
            .find(|row| matches!(row, BrowserRow::Entry(_)))
            .copied()
            .expect("the asset is a row");

        assert!(browser.choose(row));
        assert_eq!(browser.selected_name(), "pillar_tall");
        assert_eq!(
            browser.selected_asset(),
            EnvironmentAsset::interned("props/pillar_tall"),
            "a placed asset is named by its path"
        );
        assert!(browser.selected_path().is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_material_can_be_looked_at_but_not_placed() {
        let root = tree("material", &["materials/grass.json", "materials/grass.png"]);
        let mut browser = browser_over(&root);
        let row = browser
            .rows()
            .iter()
            .find(|row| matches!(row, BrowserRow::Entry(_)))
            .copied()
            .expect("the material is a row");

        assert!(!browser.choose(row), "a material was chosen for placement");
        assert!(matches!(browser.selection(), Selection::Builtin(_)));
        let (_, label) = browser.label(row);
        assert!(label.contains("MATERIAL"), "{label}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn typing_filters_the_list_flat_and_clearing_puts_the_tree_back() {
        let root = tree(
            "filter",
            &[
                "generated/rocks/rock_small.glb",
                "generated/cliffs/cliff_overhang.glb",
            ],
        );
        let mut browser = browser_over(&root);
        let before = browser.rows().len();

        assert!(browser.update_keys(&find()));
        assert!(browser.is_typing());
        for key in [KeyCode::R, KeyCode::O, KeyCode::C, KeyCode::K] {
            assert!(browser.update_keys(&keyed(key)), "the filter took the key");
        }
        assert_eq!(browser.filter(), "rock");
        assert!(
            browser
                .rows()
                .iter()
                .all(|row| !matches!(row, BrowserRow::Folder(_) | BrowserRow::BuiltinFolder)),
            "a filtered list still draws folders"
        );
        assert!(
            browser
                .rows()
                .iter()
                .any(|row| *row == BrowserRow::Entry(0) || matches!(row, BrowserRow::Entry(_)))
        );

        assert!(browser.update_keys(&keyed(KeyCode::Escape)));
        assert!(!browser.is_typing());
        assert_eq!(browser.filter(), "");
        assert_eq!(browser.rows().len(), before, "the tree did not come back");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_filter_owns_the_keys_that_would_otherwise_fly_the_camera() {
        let mut browser = AssetBrowser::new();
        assert!(
            !browser.update_keys(&keyed(KeyCode::W)),
            "W is the camera's"
        );
        assert!(
            !browser.update_keys(&keyed(KeyCode::F)),
            "a plain F frames the selection and opens nothing"
        );
        browser.update_keys(&find());
        assert!(
            browser.update_keys(&keyed(KeyCode::W)),
            "W must be the filter's while it is open"
        );
        assert_eq!(browser.filter(), "w");
        browser.update_keys(&keyed(KeyCode::Backspace));
        assert_eq!(browser.filter(), "");
    }

    #[test]
    fn a_rescan_shows_a_new_file_and_keeps_the_selection() {
        let root = tree("rescan", &["props/one.glb"]);
        let mut browser = browser_over(&root);
        let row = browser
            .rows()
            .iter()
            .find(|row| matches!(row, BrowserRow::Entry(_)))
            .copied()
            .expect("the asset is a row");
        browser.choose(row);
        assert_eq!(browser.selected_name(), "one");

        // A file appears while the editor is open, in a folder that did not exist either.
        std::fs::create_dir_all(root.join("props/added")).expect("a new folder");
        std::fs::write(root.join("props/added/two.glb"), b"new").expect("a new file");
        assert!(browser.update_keys(&keyed(KeyCode::F2)), "F2 rescans");

        assert!(browser.catalogue().entry("props/added/two").is_some());
        assert_eq!(
            browser.selected_name(),
            "one",
            "the rescan moved the selection"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tab_walks_every_placeable_asset_and_skips_the_rest() {
        let root = tree(
            "tab",
            &[
                "props/one.glb",
                "materials/grass.json",
                "materials/grass.png",
            ],
        );
        let mut browser = browser_over(&root);

        // Every built-in, then the one model on disk, then round again. The material is never
        // landed on, because placing one means nothing.
        for expected in LIBRARY.into_iter().skip(1) {
            browser.select_next();
            assert_eq!(browser.selected_name(), expected.name());
        }
        browser.select_next();
        assert_eq!(browser.selected_name(), "one");
        browser.select_next();
        assert_eq!(
            browser.selected_name(),
            LIBRARY[0].name(),
            "TAB did not wrap round to the start"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tab_works_in_a_browser_whose_folders_are_all_closed() {
        // How the browser starts: the builtin folder closed and the leaf folders closed. TAB
        // has to walk what exists rather than what is drawn, and open what it lands on.
        let root = tree("tab_closed", &["generated/environment/rocks/only.glb"]);
        let mut browser = browser_over(&root);
        assert!(!browser.is_expanded(BrowserRow::BuiltinFolder));

        browser.select_next();
        assert_eq!(browser.selected_name(), LIBRARY[1].name());
        assert!(
            browser.is_expanded(BrowserRow::BuiltinFolder),
            "TAB chose something inside a folder it did not open"
        );
        assert!(
            browser.rows().iter().any(|row| browser.is_selected(*row)),
            "the chosen asset is not on any row"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tab_onto_a_file_opens_every_folder_above_it() {
        let root = tree("tab_deep", &["generated/environment/rocks/only.glb"]);
        let mut browser = browser_over(&root);
        // Walk past the whole built-in library onto the one file on disk.
        for _ in 0..LIBRARY.len() {
            browser.select_next();
        }
        assert_eq!(browser.selected_name(), "only");
        assert!(
            browser.rows().iter().any(|row| browser.is_selected(*row)),
            "the file's folder was left closed, so the selection is invisible"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_real_directory_shows_its_categories_when_the_editor_opens() {
        let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        if !root.join("generated").is_dir() {
            return;
        }
        let mut browser = AssetBrowser::new();
        browser.scan(&root);
        let labels: Vec<String> = browser
            .rows()
            .iter()
            .map(|row| browser.label(*row).1)
            .collect();
        for wanted in ["+ caves", "+ cliffs", "+ rocks", "+ builtin"] {
            assert!(
                labels.iter().any(|label| label == wanted),
                "{wanted}: {labels:?}"
            );
        }
        assert!(
            labels.len() < 24,
            "the opening view needs scrolling to reach the categories: {labels:?}"
        );
    }

    #[test]
    fn a_placed_file_blocks_the_box_its_geometry_fills() {
        // The generated library, if this checkout has it: a boulder is solid and a water
        // plane is not, and both answers come out of the geometry rather than out of a table.
        let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        if !root.join("generated").is_dir() {
            return;
        }
        let mut browser = AssetBrowser::new();
        browser.scan(&root);

        let boulder = browser
            .catalogue()
            .entries()
            .iter()
            .position(|entry| entry.id.ends_with("rocks/boulder_large"))
            .expect("the generated library has a boulder");
        browser.choose(BrowserRow::Entry(boulder));
        let PlacementCollision::Shape(shape) = browser.selected_collision() else {
            panic!("a file's collision is a shape the level holds");
        };
        assert_eq!(shape.len(), 1, "a boulder is one box");
        assert!(
            shape.boxes[0].half_extents.y > 0.5,
            "{:?} is not boulder-sized",
            shape.boxes[0]
        );

        let water = browser
            .catalogue()
            .entries()
            .iter()
            .position(|entry| entry.id.ends_with("water/water_plane"))
            .expect("the generated library has water");
        browser.choose(BrowserRow::Entry(water));
        let PlacementCollision::Shape(shape) = browser.selected_collision() else {
            panic!("a file's collision is a shape");
        };
        assert!(shape.is_empty(), "a flat sheet of water became a wall");
    }

    #[test]
    fn a_piece_with_a_sidecar_is_placed_with_the_boxes_it_names() {
        // The whole point of Phase 7: the overhang's collision is the wall and the slab, not
        // one box around both — which is what makes the space under it walkable.
        let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        if !root.join("generated/environment/cliffs").is_dir() {
            return;
        }
        let mut browser = AssetBrowser::new();
        browser.scan(&root);
        let overhang = browser
            .catalogue()
            .entries()
            .iter()
            .position(|entry| entry.id.ends_with("cliffs/cliff_overhang"))
            .expect("the generated library has an overhang");
        browser.choose(BrowserRow::Entry(overhang));

        let PlacementCollision::Shape(shape) = browser.selected_collision() else {
            panic!("a file's collision is a shape");
        };
        assert!(
            shape.len() >= 2,
            "the overhang came out as {} box(es), so the space under it is filled in",
            shape.len()
        );
    }

    #[test]
    fn a_builtin_keeps_its_authored_collision() {
        let mut browser = AssetBrowser::new();
        // The pine: a trunk to collide with, not a canopy. No bounding box would say that.
        let pine = LIBRARY
            .into_iter()
            .position(|asset| asset.name() == "pine")
            .expect("the library has a pine");
        browser.choose(BrowserRow::Builtin(pine));
        assert_eq!(
            browser.selected_collision(),
            PlacementCollision::Simple(default_collision(LIBRARY[pine]))
        );
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let mut browser = AssetBrowser::new();
        let visible = 5;
        browser.scroll_by(-100, visible);
        assert_eq!(browser.scroll(), 0);
        browser.scroll_by(1_000, visible);
        assert_eq!(browser.scroll(), browser.rows().len() - visible);
    }

    #[test]
    fn a_missing_assets_directory_is_a_browser_with_the_builtins_in_it() {
        let mut browser = AssetBrowser::new();
        browser.scan(std::env::temp_dir().join("mygame_no_such_assets"));
        assert!(browser.catalogue().report().root_missing);
        assert_eq!(browser.rows(), [BrowserRow::BuiltinFolder]);
        browser.toggle(BrowserRow::BuiltinFolder);
        assert_eq!(browser.rows().len(), LIBRARY.len() + 1);
        assert_eq!(browser.selected_asset(), LIBRARY[0]);
    }
}
