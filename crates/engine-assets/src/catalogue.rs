//! What is in `assets/`: a directory, read recursively, as a tree of folders and assets.
//!
//! ```text
//!  assets/                       AssetCatalogue
//!  ├── generated/                ├─ folder generated
//!  │   ├── environment/          │   ├─ folder environment
//!  │   │   └── rocks/            │   │   └─ folder rocks
//!  │   │       └── rock_01.glb   │   │       └─ asset generated/environment/rocks/rock_01
//!  │   └── materials/            │   └─ folder materials
//!  │       ├── grass.json        │       └─ asset generated/materials/grass
//!  │       └── grass.png         │          (the image belongs to the material beside it)
//!  └── characters/               └─ folder characters
//! ```
//!
//! # No fixed structure
//!
//! Nothing here knows what `environment` or `rocks` mean. A folder is a folder at any depth,
//! an asset is a file with a recognised extension, and both are discovered by walking. Adding
//! `assets/environment/trees/oak_03.glb` makes an oak appear in the editor after a rescan and
//! requires no code change — which is the whole requirement.
//!
//! # An asset's id is its path
//!
//! `generated/environment/rocks/rock_small`: the path under the assets root with the
//! extension removed, slashes forward on every platform. It is unique by construction, it
//! says where the file is, and it is what a scene file stores — so a level written on Windows
//! opens on anything else.
//!
//! # A material is one asset, not two files
//!
//! `grass.json` and `grass.png` are one material. The `.json` is the asset and the `.png` is
//! its image; an image with no material file beside it is listed on its own as a texture,
//! because an artist dropping a folder of maps in should see them.
//!
//! # What a broken file does
//!
//! Nothing, loudly. Scanning does not open a single file: it reads names and extensions, so a
//! truncated `.glb` is listed and fails when something asks for its model. A directory that
//! cannot be read is one entry in [`ScanReport::unreadable`] and the rest of the scan
//! continues — an editor that shows nothing because one folder is locked is worse than an
//! editor that shows everything else.

use std::path::{Path, PathBuf};

/// What kind of thing an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetKind {
    /// A model: `.glb` or `.gltf`. The only kind that can be placed in a level.
    Model,
    /// A material: a `.json` beside its image.
    Material,
    /// An image with no material file beside it.
    Texture,
    /// A level.
    Scene,
}

impl AssetKind {
    /// Short uppercase name, for the editor's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "MODEL",
            Self::Material => "MATERIAL",
            Self::Texture => "TEXTURE",
            Self::Scene => "SCENE",
        }
    }

    /// What kind a file with this extension is, if the catalogue lists it at all.
    #[must_use]
    pub fn of_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "glb" | "gltf" => Some(Self::Model),
            "json" => Some(Self::Material),
            "png" | "jpg" | "jpeg" => Some(Self::Texture),
            _ => None,
        }
    }

    /// Whether an object of this kind can be placed in a level.
    #[must_use]
    pub fn is_placeable(self) -> bool {
        matches!(self, Self::Model)
    }
}

/// One asset in the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueEntry {
    /// What a person calls it: the file's stem, `"rock_small"`.
    pub name: String,
    /// What a level calls it: `"generated/environment/rocks/rock_small"`.
    pub id: String,
    /// Where the file is.
    pub path: PathBuf,
    /// Which folder holds it, as an index into [`AssetCatalogue::folders`].
    pub folder: usize,
    /// What kind of thing it is.
    pub kind: AssetKind,
}

/// One folder in the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueFolder {
    /// The folder's own name: `"rocks"`. Empty for the root.
    pub name: String,
    /// The path under the assets root: `"generated/environment/rocks"`. Empty for the root.
    pub path: String,
    /// How deep it is. The root is 0.
    pub depth: usize,
    /// The folder above it, or `None` for the root.
    pub parent: Option<usize>,
    /// Folders inside it, in name order.
    pub children: Vec<usize>,
    /// Assets inside it, in name order.
    pub assets: Vec<usize>,
}

impl CatalogueFolder {
    /// Whether the folder holds nothing at all, at any depth.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.children.is_empty() && self.assets.is_empty()
    }
}

/// What a scan could not read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Directories that could not be listed, and why.
    pub unreadable: Vec<String>,
    /// Whether the root itself is not there.
    ///
    /// Not an error: a project with no `assets/` directory is a project nobody has put
    /// anything in yet, and the editor has to open anyway.
    pub root_missing: bool,
}

/// Every asset under one root, as a tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetCatalogue {
    root: PathBuf,
    folders: Vec<CatalogueFolder>,
    entries: Vec<CatalogueEntry>,
    report: ScanReport,
}

impl AssetCatalogue {
    /// An empty catalogue rooted at `root`, with the root folder and nothing in it.
    #[must_use]
    pub fn empty(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            folders: vec![CatalogueFolder {
                name: String::new(),
                path: String::new(),
                depth: 0,
                parent: None,
                children: Vec::new(),
                assets: Vec::new(),
            }],
            entries: Vec::new(),
            report: ScanReport::default(),
        }
    }

    /// Reads `root` recursively.
    ///
    /// Never fails: a missing root is an empty catalogue that says so, and a directory that
    /// cannot be listed is one line in the report.
    #[must_use]
    pub fn scan(root: impl Into<PathBuf>) -> Self {
        let mut catalogue = Self::empty(root);
        if !catalogue.root.is_dir() {
            catalogue.report.root_missing = true;
            return catalogue;
        }
        let root = catalogue.root.clone();
        catalogue.walk(&root, 0);
        catalogue.sort();
        catalogue
    }

    /// Reads the same root again, in place.
    ///
    /// What the editor's refresh does. It rebuilds rather than diffs: a rescan is a few
    /// hundred directory entries and happens when a person presses a key, and a diff is a
    /// second description of the tree that can disagree with the first.
    pub fn rescan(&mut self) {
        *self = Self::scan(self.root.clone());
    }

    /// Where this catalogue was read from.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every folder. Index 0 is the root.
    #[must_use]
    pub fn folders(&self) -> &[CatalogueFolder] {
        &self.folders
    }

    /// Every asset.
    #[must_use]
    pub fn entries(&self) -> &[CatalogueEntry] {
        &self.entries
    }

    /// What the scan could not read.
    #[must_use]
    pub fn report(&self) -> &ScanReport {
        &self.report
    }

    /// How many assets there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no assets at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The asset with this id, if there is one.
    #[must_use]
    pub fn entry(&self, id: &str) -> Option<&CatalogueEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Every asset whose id contains `needle`, case-insensitively.
    ///
    /// The id rather than the name, so typing `rocks` finds a folder's worth of them.
    #[must_use]
    pub fn search(&self, needle: &str) -> Vec<usize> {
        let needle = needle.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return (0..self.entries.len()).collect();
        }
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.id.to_ascii_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect()
    }

    /// How many assets of each kind there are, in [`AssetKind`] order.
    #[must_use]
    pub fn counts(&self) -> [usize; 4] {
        let mut counts = [0; 4];
        for entry in &self.entries {
            let slot = match entry.kind {
                AssetKind::Model => 0,
                AssetKind::Material => 1,
                AssetKind::Texture => 2,
                AssetKind::Scene => 3,
            };
            counts[slot] += 1;
        }
        counts
    }

    /// Reads one directory into `folder`, then everything under it.
    fn walk(&mut self, directory: &Path, folder: usize) {
        let listing = match std::fs::read_dir(directory) {
            Ok(listing) => listing,
            Err(error) => {
                self.report
                    .unreadable
                    .push(format!("{}: {error}", directory.display()));
                return;
            }
        };

        let mut subdirectories: Vec<(String, PathBuf)> = Vec::new();
        let mut files: Vec<(String, String, PathBuf)> = Vec::new();
        for item in listing.flatten() {
            let path = item.path();
            let Some(name) = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            // A hidden or tool-owned directory is not content: `.git`, `.import`, `__pycache__`.
            if name.starts_with('.') || name.starts_with("__") {
                continue;
            }
            if path.is_dir() {
                subdirectories.push((name, path));
                continue;
            }
            let Some(extension) = path
                .extension()
                .map(|extension| extension.to_string_lossy().into_owned())
            else {
                continue;
            };
            let stem = path
                .file_stem()
                .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
            files.push((stem, extension, path));
        }

        // An image is only listed on its own when no material file beside it claims it.
        let claimed: Vec<String> = files
            .iter()
            .filter(|(_, extension, _)| extension.eq_ignore_ascii_case("json"))
            .map(|(stem, _, _)| stem.clone())
            .collect();

        for (stem, extension, path) in files {
            let Some(mut kind) = AssetKind::of_extension(&extension) else {
                continue;
            };
            if kind == AssetKind::Texture && claimed.contains(&stem) {
                continue;
            }
            // A `.json` under `scenes/` is a level rather than a material. The folder decides,
            // because both are JSON and only their location says which — and a level is worth
            // showing in the browser that opens levels.
            if kind == AssetKind::Material && self.folders[folder].path.starts_with("scenes") {
                kind = AssetKind::Scene;
            }
            let id = if self.folders[folder].path.is_empty() {
                stem.clone()
            } else {
                format!("{}/{stem}", self.folders[folder].path)
            };
            let index = self.entries.len();
            self.entries.push(CatalogueEntry {
                name: stem,
                id,
                path,
                folder,
                kind,
            });
            self.folders[folder].assets.push(index);
        }

        for (name, path) in subdirectories {
            let parent_path = self.folders[folder].path.clone();
            let child_path = if parent_path.is_empty() {
                name.clone()
            } else {
                format!("{parent_path}/{name}")
            };
            let child = self.folders.len();
            let depth = self.folders[folder].depth + 1;
            self.folders.push(CatalogueFolder {
                name,
                path: child_path,
                depth,
                parent: Some(folder),
                children: Vec::new(),
                assets: Vec::new(),
            });
            self.folders[folder].children.push(child);
            self.walk(&path, child);
        }
    }

    /// Puts every folder's children and assets in name order.
    ///
    /// `read_dir` makes no promise about order, and a browser whose rows move between two
    /// scans of an unchanged directory is a browser nobody can point at.
    fn sort(&mut self) {
        for index in 0..self.folders.len() {
            let mut children = std::mem::take(&mut self.folders[index].children);
            children.sort_by(|a, b| self.folders[*a].name.cmp(&self.folders[*b].name));
            self.folders[index].children = children;

            let mut assets = std::mem::take(&mut self.folders[index].assets);
            assets.sort_by(|a, b| {
                self.entries[*a]
                    .kind
                    .cmp(&self.entries[*b].kind)
                    .then_with(|| self.entries[*a].name.cmp(&self.entries[*b].name))
            });
            self.folders[index].assets = assets;
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// Builds a directory tree under a fresh temporary root.
    fn tree(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir()
            .join("mygame_catalogue_tests")
            .join(name);
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

    #[test]
    fn a_missing_root_is_an_empty_catalogue_rather_than_a_failure() {
        let catalogue = AssetCatalogue::scan(std::env::temp_dir().join("mygame_nothing_here"));
        assert!(catalogue.is_empty());
        assert!(catalogue.report().root_missing);
        assert_eq!(catalogue.folders().len(), 1, "the root folder still exists");
    }

    #[test]
    fn folders_nest_to_any_depth_and_ids_are_paths() {
        let root = tree(
            "nested",
            &[
                "generated/environment/rocks/rock_01.glb",
                "generated/environment/rocks/rock_02.glb",
                "generated/environment/cliffs/deep/deeper/cliff_x.glb",
                "characters/hero.glb",
            ],
        );
        let catalogue = AssetCatalogue::scan(&root);

        assert_eq!(catalogue.len(), 4);
        assert!(
            catalogue
                .entry("generated/environment/rocks/rock_01")
                .is_some(),
            "an id is the path with the extension off"
        );
        let deep = catalogue
            .entry("generated/environment/cliffs/deep/deeper/cliff_x")
            .expect("depth is not limited");
        assert_eq!(catalogue.folders()[deep.folder].depth, 5);
        assert_eq!(catalogue.folders()[deep.folder].name, "deeper");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_material_claims_the_image_beside_it_and_a_loose_image_is_listed() {
        let root = tree(
            "materials",
            &[
                "generated/materials/grass.json",
                "generated/materials/grass.png",
                "textures/loose_map.png",
            ],
        );
        let catalogue = AssetCatalogue::scan(&root);

        let grass = catalogue
            .entry("generated/materials/grass")
            .expect("the material is listed");
        assert_eq!(grass.kind, AssetKind::Material);
        assert_eq!(
            catalogue.len(),
            2,
            "grass.png was listed separately from the material it belongs to"
        );
        assert_eq!(
            catalogue
                .entry("textures/loose_map")
                .expect("a loose image is still content")
                .kind,
            AssetKind::Texture
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_json_under_scenes_is_a_level_and_not_a_material() {
        let root = tree("scenes", &["scenes/first_level.json"]);
        let catalogue = AssetCatalogue::scan(&root);
        assert_eq!(
            catalogue.entry("scenes/first_level").expect("listed").kind,
            AssetKind::Scene
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unknown_extensions_and_hidden_directories_are_skipped() {
        let root = tree(
            "skipping",
            &[
                "notes.txt",
                "model.blend",
                ".git/objects/thing.glb",
                "__pycache__/cached.glb",
                "keep/real.glb",
            ],
        );
        let catalogue = AssetCatalogue::scan(&root);
        assert_eq!(catalogue.len(), 1, "{:?}", catalogue.entries());
        assert_eq!(catalogue.entries()[0].id, "keep/real");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_rescan_sees_a_file_that_was_added_after_the_first_one() {
        let root = tree("refresh", &["props/one.glb"]);
        let mut catalogue = AssetCatalogue::scan(&root);
        assert_eq!(catalogue.len(), 1);

        std::fs::write(root.join("props/two.glb"), b"new").expect("a second file");
        std::fs::create_dir_all(root.join("props/deeper")).expect("a new folder");
        std::fs::write(root.join("props/deeper/three.glb"), b"new").expect("a third file");

        catalogue.rescan();
        assert_eq!(catalogue.len(), 3);
        assert!(catalogue.entry("props/deeper/three").is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_search_matches_anywhere_in_the_id_and_ignores_case() {
        let root = tree(
            "search",
            &[
                "generated/environment/rocks/rock_small.glb",
                "generated/environment/cliffs/cliff_overhang.glb",
                "generated/materials/rock.json",
            ],
        );
        let catalogue = AssetCatalogue::scan(&root);

        assert_eq!(catalogue.search("").len(), 3, "an empty search matches all");
        assert_eq!(catalogue.search("ROCK").len(), 2, "case is ignored");
        assert_eq!(
            catalogue.search("cliffs/").len(),
            1,
            "a folder can be typed"
        );
        assert!(catalogue.search("nothing_like_this").is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_order_of_a_folder_does_not_depend_on_the_filesystem() {
        let root = tree(
            "order",
            &[
                "props/zeta.glb",
                "props/alpha.glb",
                "props/mid.glb",
                "props/zzz_folder/inner.glb",
                "props/aaa_folder/inner.glb",
            ],
        );
        let first = AssetCatalogue::scan(&root);
        let second = AssetCatalogue::scan(&root);
        assert_eq!(first, second, "two scans of one directory disagree");

        let props = first
            .folders()
            .iter()
            .position(|folder| folder.path == "props")
            .expect("the folder is there");
        let names: Vec<&str> = first.folders()[props]
            .assets
            .iter()
            .map(|index| first.entries()[*index].name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "mid", "zeta"]);
        let folders: Vec<&str> = first.folders()[props]
            .children
            .iter()
            .map(|index| first.folders()[*index].name.as_str())
            .collect();
        assert_eq!(folders, ["aaa_folder", "zzz_folder"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_real_generated_library_is_discovered_by_a_scan() {
        // The assets the generator wrote, if this checkout has run it. Skipped rather than
        // failed when it has not: the catalogue's own rules are covered by the tests above,
        // and this one is about the real directory.
        let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let catalogue = AssetCatalogue::scan(&root);
        if catalogue
            .entry("generated/environment/rocks/rock_small")
            .is_none()
        {
            return;
        }
        let [models, materials, _, _] = catalogue.counts();
        assert!(models >= 24, "{models} models found");
        assert!(materials >= 7, "{materials} materials found");
    }
}
