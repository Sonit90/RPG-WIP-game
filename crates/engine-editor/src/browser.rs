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
use engine_environment::EnvironmentAsset;
use engine_environment::asset::LIBRARY;

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
        };
        browser.rebuild_rows();
        browser
    }

    /// Reads `root` and opens the folders down to the first level.
    ///
    /// Everything is open at the top and closed below it: a tree that opens fully shows a
    /// hundred rows the first time it is looked at, and a tree that opens nothing makes an
    /// author click three times to find out whether anything is there at all.
    pub fn scan(&mut self, root: impl Into<PathBuf>) {
        self.catalogue = AssetCatalogue::scan(root);
        self.expanded = self
            .catalogue
            .folders()
            .iter()
            .map(|folder| folder.depth <= 1)
            .collect();
        self.scroll = 0;
        self.clamp_selection();
        self.rebuild_rows();
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
        self.expanded = self
            .catalogue
            .folders()
            .iter()
            .map(|folder| open.contains(&folder.path) || folder.depth <= 1)
            .collect();
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
    pub fn select_next(&mut self) {
        let placeable: Vec<Selection> = self.placeable_rows();
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
                format!(
                    "{} builtin",
                    if self.builtin_expanded { "-" } else { "+" }
                ),
            ),
            BrowserRow::Builtin(index) => (
                1,
                LIBRARY[index.min(LIBRARY.len() - 1)].name().to_owned(),
            ),
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
            if input.was_key_pressed(KeyCode::F) {
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
        if input.was_key_pressed(KeyCode::Enter) || input.was_key_pressed(KeyCode::F) {
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

    /// Every row the next click could place, in the order they are drawn.
    fn placeable_rows(&self) -> Vec<Selection> {
        self.rows
            .iter()
            .filter_map(|row| match row {
                BrowserRow::Builtin(index) => Some(Selection::Builtin(*index)),
                BrowserRow::Entry(index) => self
                    .catalogue
                    .entries()
                    .get(*index)
                    .filter(|entry| entry.kind.is_placeable())
                    .map(|_| Selection::Entry(*index)),
                _ => None,
            })
            .collect()
    }

    /// Scrolls the chosen asset into view, opening nothing.
    fn reveal_selection(&mut self) {
        let Some(position) = self.rows.iter().position(|row| self.is_selected(*row)) else {
            return;
        };
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

    fn browser_over(root: &Path) -> AssetBrowser {
        let mut browser = AssetBrowser::new();
        browser.scan(root);
        browser
    }

    #[test]
    fn an_empty_assets_directory_still_lists_the_builtin_library() {
        let root = tree("empty", &[]);
        std::fs::create_dir_all(&root).expect("the directory");
        let browser = browser_over(&root);

        assert_eq!(
            browser.rows().len(),
            LIBRARY.len() + 1,
            "the builtin folder and its assets, and nothing else"
        );
        assert_eq!(browser.rows()[0], BrowserRow::BuiltinFolder);
        assert_eq!(browser.selected_asset(), LIBRARY[0]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_folder_opens_and_closes_and_takes_its_contents_with_it() {
        let root = tree(
            "expand",
            &["generated/rocks/a.glb", "generated/rocks/b.glb"],
        );
        let mut browser = browser_over(&root);
        browser.toggle(BrowserRow::BuiltinFolder);

        // generated is open (depth 1), rocks is closed (depth 2).
        let rows = browser.rows().to_vec();
        assert!(rows.contains(&BrowserRow::BuiltinFolder));
        let rocks = rows
            .iter()
            .find(|row| matches!(row, BrowserRow::Folder(index)
                if browser.catalogue().folders()[*index].name == "rocks"))
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

        assert!(browser.update_keys(&keyed(KeyCode::F)));
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
        assert!(!browser.update_keys(&keyed(KeyCode::W)), "W is the camera's");
        browser.update_keys(&keyed(KeyCode::F));
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
    fn tab_walks_every_placeable_row_and_skips_the_rest() {
        let root = tree(
            "tab",
            &["props/one.glb", "materials/grass.json", "materials/grass.png"],
        );
        let mut browser = browser_over(&root);
        browser.toggle(BrowserRow::BuiltinFolder);

        // Only the model is placeable, so TAB lands on it and stays there.
        browser.select_next();
        assert_eq!(browser.selected_name(), "one");
        browser.select_next();
        assert_eq!(browser.selected_name(), "one");
        std::fs::remove_dir_all(&root).ok();
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
        assert_eq!(browser.rows().len(), LIBRARY.len() + 1);
        assert_eq!(browser.selected_asset(), LIBRARY[0]);
    }
}
