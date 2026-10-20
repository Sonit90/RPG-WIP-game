//! Generates the table of level files a filesystem-less server embeds.
//!
//! The realm module (wasm) cannot read files, so every dungeon's `scene` named in
//! `assets/catalog/dungeons.json` is turned into an `include_str!` arm here. Adding a dungeon is
//! a row in that file plus the scene it names; no Rust changes.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a build script fails the build by panicking"
)]

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"));
    let assets = manifest.join("../../assets");
    let catalog = assets.join("catalog/dungeons.json");
    println!("cargo:rerun-if-changed={}", catalog.display());
    println!("cargo:rerun-if-changed=build.rs");

    let text = fs::read_to_string(&catalog)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", catalog.display()));
    let json: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", catalog.display()));
    let dungeons = json
        .get("dungeons")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("{} has no `dungeons` array", catalog.display()));

    let mut scenes: Vec<String> = dungeons
        .iter()
        .filter_map(|dungeon| dungeon.get("scene").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .collect();
    scenes.sort();
    scenes.dedup();

    let mut out = String::from("match path {\n");
    for scene in &scenes {
        let file = assets.join(scene);
        println!("cargo:rerun-if-changed={}", file.display());
        let absolute = fs::canonicalize(&file).unwrap_or_else(|error| {
            panic!(
                "dungeon scene {scene} ({}) is missing: {error}",
                file.display()
            )
        });
        let literal = absolute
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let literal = literal.strip_prefix("//?/").unwrap_or(&literal);
        writeln!(out, "    {scene:?} => Some(include_str!({literal:?})),")
            .expect("write to string");
    }
    out.push_str("    _ => None,\n}\n");

    let target =
        PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("embedded_scenes.rs");
    fs::write(&target, out)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", target.display()));
}
