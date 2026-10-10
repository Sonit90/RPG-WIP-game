//! Embeds the game's content into the executable for the `standalone` feature.
//!
//! Without the feature this writes an empty table and watches nothing, so an ordinary build
//! is unaffected. With it, every file the game reads at runtime is listed with an
//! `include_bytes!`, and a hash of the whole set names the folder it is unpacked into — see
//! `src/standalone.rs`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a build script reports a broken input by failing the build"
)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What the game reads from `assets/` at runtime. Source art (`.blend`, `.fbx`), backups
/// and the sound shortlist are not here: nothing loads them.
const ROOT_FILES: [&str; 2] = ["firemage.glb", "warrior.glb"];
/// Directories shipped whole, minus editor autosaves.
const DIRECTORIES: [&str; 4] = ["catalog", "scenes", "generated", "audio"];

fn main() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let mut table =
        String::from("/// Every embedded file: its path under `assets/` and its bytes.\n");
    table.push_str("pub const FILES: &[(&str, &[u8])] = &[\n");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

    if std::env::var_os("CARGO_FEATURE_STANDALONE").is_some() {
        let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set"));
        let assets = manifest.join("../../assets");
        let mut files: Vec<PathBuf> = ROOT_FILES.iter().map(|f| assets.join(f)).collect();
        for directory in DIRECTORIES {
            println!(
                "cargo:rerun-if-changed={}",
                assets.join(directory).display()
            );
            collect(&assets.join(directory), &mut files);
        }
        files.sort();
        for file in &files {
            println!("cargo:rerun-if-changed={}", file.display());
            let bytes = std::fs::read(file)
                .unwrap_or_else(|error| panic!("cannot embed {}: {error}", file.display()));
            let relative = file
                .strip_prefix(&assets)
                .expect("under assets")
                .to_string_lossy()
                .replace('\\', "/");
            for byte in relative.bytes().chain(bytes.iter().copied()) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
            let absolute = file.canonicalize().expect("exists");
            let _ = writeln!(
                table,
                "    (\"{relative}\", include_bytes!(r\"{}\")),",
                absolute.display()
            );
        }
    }
    table.push_str("];\n");
    let _ = writeln!(
        table,
        "/// Names the unpack folder: changes whenever any embedded byte does.\npub const HASH: u64 = {hash:#018x};"
    );
    std::fs::write(out.join("embedded_assets.rs"), table).expect("OUT_DIR is writable");
    println!("cargo:rerun-if-changed=build.rs");
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if !path.to_string_lossy().contains(".autosave") {
            files.push(path);
        }
    }
}
