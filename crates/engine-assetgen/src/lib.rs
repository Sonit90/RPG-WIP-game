//! `asset_gen`: writing the procedural starter library to disk as ordinary content.
//!
//! ```text
//!  asset_gen                       write assets/generated/**
//!  asset_gen --root some/dir       write somewhere else
//!  asset_gen --list                say what would be written, write nothing
//!  asset_gen --force               overwrite files that are already there
//! ```
//!
//! # What it produces
//!
//! ```text
//! assets/generated/
//! ├── environment/
//! │   ├── rocks/      rock_small.glb  rock_medium.glb  boulder_large.glb  rock_spire.glb
//! │   ├── cliffs/     cliff_straight.glb … cliff_overhang.glb  cliff_arch.glb
//! │   ├── ground/     ground_tile.glb  platform_square.glb  ramp_square.glb
//! │   ├── props/      stone_cube.glb  pillar.glb  arch.glb  crate_wooden.glb
//! │   ├── vegetation/ bush_round.glb  tree_simple.glb
//! │   ├── caves/      cave_entrance.glb  cave_tunnel_straight.glb …
//! │   └── water/      water_plane.glb
//! └── materials/      grass.json + grass.png, dirt, rock, cliff, sand, water, simple_test
//! ```
//!
//! Every one of them is a file of the same kind an artist delivers: a `.glb` the engine's own
//! importer reads, and a `.png` beside a small JSON file. Nothing about them is special-cased
//! anywhere in the engine, which is the property that makes them replaceable — drop a
//! sculpted `rock_small.glb` over the generated one and the level that placed it is
//! unchanged.
//!
//! # Why an existing file is not overwritten
//!
//! Because the first thing that happens to starter content is that somebody replaces it. A
//! generator that overwrote whatever it found would delete an artist's work the next time
//! anybody ran it out of habit. `--force` is there for regenerating deliberately.
//!
//! # Why the work lives here and not in `main`
//!
//! So it can be tested: [`run`] takes options and a writer and returns an [`Outcome`], and
//! `main` turns that into an exit code. The tests at the bottom write into a temporary
//! directory and read the result back with the engine's importer.

#![deny(missing_docs)]

pub mod showcase;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use engine_assets::collision_file::CollisionFile;
use engine_assets::gltf_export::{encode_png_file, export_glb};
use engine_assets::material_file::MaterialFile;
use engine_assets::procedural::starter::{STARTER_LIBRARY, STARTER_MATERIALS};

/// Usage text, printed for `--help` and on a parse error.
pub const USAGE: &str = "\
usage: asset_gen [options]

Writes the procedural starter library — blockout rocks, cliffs, cave pieces, ground, props
and their materials — into an assets directory as ordinary .glb and .png files.

options:
  --root <dir>   where to write. Default: the repository's assets/ directory
  --force        overwrite files that are already there. Off by default, so a generated
                 asset somebody has since replaced with real art is never destroyed by
                 running this again
  --list         print what would be written and write nothing
  -h, --help     print this help and exit";

/// What a run was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The assets directory to write under.
    pub root: PathBuf,
    /// Whether an existing file may be overwritten.
    pub force: bool,
    /// Whether to write anything at all.
    pub list_only: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            root: default_root(),
            force: false,
            list_only: false,
        }
    }
}

/// Where the generator writes when nothing said otherwise: the repository's `assets/`.
///
/// The same directory `engine_assets::disk::asset_roots` looks in last, so content written
/// here is content a `cargo run` finds.
#[must_use]
pub fn default_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"))
}

/// What a run wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Outcome {
    /// Files written.
    pub written: usize,
    /// Files left alone because they were already there.
    pub skipped: usize,
    /// Files that could not be written or built.
    pub failed: usize,
}

impl Outcome {
    /// Whether the run should fail the build.
    #[must_use]
    pub fn failing(self) -> bool {
        self.failed > 0
    }
}

/// Parses a command line.
///
/// # Errors
///
/// A message naming the option that is wrong, ready to print above [`USAGE`].
pub fn parse<S: AsRef<str>>(arguments: &[S]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut iter = arguments.iter().map(AsRef::as_ref);
    while let Some(argument) = iter.next() {
        match argument {
            "--root" => {
                let value = iter.next().ok_or("--root needs a directory")?;
                options.root = PathBuf::from(value);
            }
            "--force" => options.force = true,
            "--list" => options.list_only = true,
            "-h" | "--help" => return Err("help".to_owned()),
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(options)
}

/// Writes the starter library, reporting into `out`.
///
/// Never panics and never stops on the first failure: an asset that will not build is one
/// line in the report and a non-zero exit, and the other twenty-three are still written.
pub fn run(options: &Options, out: &mut String) -> Outcome {
    let mut outcome = Outcome::default();

    for asset in STARTER_LIBRARY {
        let path = options.root.join(asset.relative_path());
        if options.list_only {
            let _ = writeln!(out, "would write {}", path.display());
            outcome.written += 1;
        } else if path.exists() && !options.force {
            let _ = writeln!(out, "kept    {}", path.display());
            outcome.skipped += 1;
        } else {
            let model = (asset.build)();
            let triangles = model.triangle_count();
            match export_glb(&model)
                .map_err(|error| error.to_string())
                .and_then(|bytes| write_file(&path, &bytes).map(|()| bytes.len()))
            {
                Ok(bytes) => {
                    let _ = writeln!(
                        out,
                        "wrote   {} ({triangles} triangles, {:.1} KiB)",
                        path.display(),
                        bytes as f64 / 1024.0
                    );
                    outcome.written += 1;
                }
                Err(reason) => {
                    let _ = writeln!(out, "FAILED  {}: {reason}", path.display());
                    outcome.failed += 1;
                }
            }
        }

        // The collision sidecar, for the pieces a box around the geometry gets wrong: the
        // overhang, the arch and every cave piece. Written beside the model, so an artist
        // replacing the model can replace this with it — see `engine_assets::collision_file`.
        //
        // Handled whatever happened to the model above: a run that kept an artist's `.glb`
        // must still be able to write the sidecar that was never there, or replacing a model
        // would silently lose its collision.
        let Some(shape) = asset.collision() else {
            continue;
        };
        let sidecar = options.root.join(asset.collision_path());
        if options.list_only {
            let _ = writeln!(out, "would write {}", sidecar.display());
            outcome.written += 1;
            continue;
        }
        if sidecar.exists() && !options.force {
            let _ = writeln!(out, "kept    {}", sidecar.display());
            outcome.skipped += 1;
            continue;
        }
        match CollisionFile::from_shape(&shape).save(&sidecar) {
            Ok(()) => {
                let _ = writeln!(
                    out,
                    "wrote   {} ({} box(es))",
                    sidecar.display(),
                    shape.len()
                );
                outcome.written += 1;
            }
            Err(error) => {
                let _ = writeln!(out, "FAILED  {}: {error}", sidecar.display());
                outcome.failed += 1;
            }
        }
    }

    for material in STARTER_MATERIALS {
        let directory = options.root.join("generated/materials");
        let image_name = format!("{}.png", material.name);
        let image = directory.join(&image_name);
        let description = directory.join(format!("{}.json", material.name));

        if options.list_only {
            let _ = writeln!(out, "would write {}", image.display());
            let _ = writeln!(out, "would write {}", description.display());
            outcome.written += 2;
            continue;
        }

        if image.exists() && !options.force {
            let _ = writeln!(out, "kept    {}", image.display());
            outcome.skipped += 1;
        } else {
            match encode_png_file(&material.texture())
                .map_err(|error| error.to_string())
                .and_then(|bytes| write_file(&image, &bytes))
            {
                Ok(()) => {
                    let _ = writeln!(out, "wrote   {}", image.display());
                    outcome.written += 1;
                }
                Err(reason) => {
                    let _ = writeln!(out, "FAILED  {}: {reason}", image.display());
                    outcome.failed += 1;
                }
            }
        }

        if description.exists() && !options.force {
            let _ = writeln!(out, "kept    {}", description.display());
            outcome.skipped += 1;
            continue;
        }
        let file = MaterialFile::from_material(&material.material(), Some(&image_name));
        match file.save(&description) {
            Ok(()) => {
                let _ = writeln!(out, "wrote   {}", description.display());
                outcome.written += 1;
            }
            Err(error) => {
                let _ = writeln!(out, "FAILED  {}: {error}", description.display());
                outcome.failed += 1;
            }
        }
    }

    let _ = writeln!(
        out,
        "\n{} written, {} kept, {} failed",
        outcome.written, outcome.skipped, outcome.failed
    );
    outcome
}

/// Writes `bytes` to `path`, creating the directories above it.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_assets::gltf_import::import_glb;

    fn temporary(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join("mygame_assetgen_tests").join(name);
        std::fs::remove_dir_all(&path).ok();
        path
    }

    fn options(root: PathBuf) -> Options {
        Options {
            root,
            force: false,
            list_only: false,
        }
    }

    #[test]
    fn a_run_writes_every_asset_and_every_material() {
        let root = temporary("full_run");
        let mut out = String::new();
        let outcome = run(&options(root.clone()), &mut out);

        assert_eq!(outcome.failed, 0, "{out}");
        let sidecars = STARTER_LIBRARY
            .iter()
            .filter(|asset| asset.collision().is_some())
            .count();
        assert!(sidecars >= 5, "{sidecars} pieces carry authored collision");
        assert_eq!(
            outcome.written,
            STARTER_LIBRARY.len() + sidecars + STARTER_MATERIALS.len() * 2,
            "one .glb per asset, a sidecar per hollow one, and two files per material\n{out}"
        );
        for asset in STARTER_LIBRARY {
            let path = root.join(asset.relative_path());
            assert!(path.is_file(), "{} was not written", path.display());
        }
        for material in STARTER_MATERIALS {
            let directory = root.join("generated/materials");
            assert!(directory.join(format!("{}.png", material.name)).is_file());
            assert!(directory.join(format!("{}.json", material.name)).is_file());
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn what_it_writes_is_what_the_engine_reads_back() {
        let root = temporary("readable");
        let mut out = String::new();
        run(&options(root.clone()), &mut out);

        for asset in STARTER_LIBRARY {
            let path = root.join(asset.relative_path());
            let bytes = std::fs::read(&path).expect("the file is there");
            let model = import_glb(asset.name, &bytes)
                .unwrap_or_else(|error| panic!("{} does not import: {error}", asset.name));
            assert!(model.triangle_count() > 0, "{} is empty", asset.name);
        }
        // What the hollow pieces are for: a sidecar the editor can read back.
        for asset in STARTER_LIBRARY {
            let Some(authored) = asset.collision() else {
                continue;
            };
            let model = root.join(asset.relative_path());
            let read = engine_assets::collision_file::CollisionFile::beside(&model)
                .expect("the sidecar reads")
                .expect("the sidecar is there");
            assert_eq!(read, authored, "{} lost its collision", asset.name);
        }

        for material in STARTER_MATERIALS {
            let path = root.join(format!("generated/materials/{}.json", material.name));
            let (file, texture) =
                MaterialFile::load_with_texture(&path).expect("the material loads");
            assert_eq!(file.name, material.name);
            assert!(texture.is_some(), "{} has no image", material.name);
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_second_run_keeps_what_is_already_there() {
        let root = temporary("idempotent");
        let mut out = String::new();
        run(&options(root.clone()), &mut out);

        // Somebody replaces a generated rock with their own.
        let replaced = root.join("generated/environment/rocks/rock_small.glb");
        std::fs::write(&replaced, b"an artist's file").expect("it writes");

        let mut second = String::new();
        let outcome = run(&options(root.clone()), &mut second);
        assert_eq!(outcome.written, 0, "{second}");
        let sidecars = STARTER_LIBRARY
            .iter()
            .filter(|asset| asset.collision().is_some())
            .count();
        assert_eq!(
            outcome.skipped,
            STARTER_LIBRARY.len() + sidecars + STARTER_MATERIALS.len() * 2
        );
        assert_eq!(
            std::fs::read(&replaced).expect("still there"),
            b"an artist's file",
            "the generator overwrote content it did not write"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn force_overwrites_and_list_writes_nothing() {
        let root = temporary("force_and_list");
        let mut out = String::new();
        run(&options(root.clone()), &mut out);
        let replaced = root.join("generated/environment/rocks/rock_small.glb");
        std::fs::write(&replaced, b"stale").expect("it writes");

        let forced = Options {
            force: true,
            ..options(root.clone())
        };
        let mut second = String::new();
        let outcome = run(&forced, &mut second);
        assert_eq!(outcome.skipped, 0);
        assert_ne!(
            std::fs::read(&replaced).expect("still there"),
            b"stale",
            "--force did not overwrite"
        );

        let listing_root = temporary("listing");
        let listing = Options {
            list_only: true,
            ..options(listing_root.clone())
        };
        let mut third = String::new();
        run(&listing, &mut third);
        assert!(!listing_root.exists(), "--list wrote files");
        assert!(third.contains("would write"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_command_line_says_what_it_means() {
        assert_eq!(parse::<&str>(&[]).expect("no arguments"), Options::default());
        let parsed = parse(&["--root", "somewhere", "--force"]).expect("valid");
        assert_eq!(parsed.root, PathBuf::from("somewhere"));
        assert!(parsed.force);
        assert!(!parsed.list_only);
        assert!(parse(&["--root"]).is_err(), "--root needs a value");
        assert!(parse(&["--nonsense"]).is_err());
    }
}
