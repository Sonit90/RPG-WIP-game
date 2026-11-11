//! A single-file build: the content travels inside the executable.
//!
//! ```text
//! mygame.exe ── FILES (include_bytes!) ──► %LOCALAPPDATA%\mygame\assets-<HASH>\
//!                                          (macOS: ~/Library/Caches/mygame/assets-<HASH>/)
//!                                                   ▲
//!                         MYGAME_ASSETS ────────────┘  (asset_roots() looks here first)
//! ```
//!
//! Unpacked once per build: the folder is named by a hash of every embedded byte, so a new
//! build unpacks beside an old one instead of mixing with it, and a second launch of the same
//! build only checks a marker. Every loader already searches `MYGAME_ASSETS` first, so
//! nothing downstream knows the files came out of the executable.

use std::path::{Path, PathBuf};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));
}

/// The marker written last, so a half-finished unpack is never mistaken for a whole one.
const MARKER: &str = ".unpacked";

/// Unpacks the embedded content and points the asset search at it.
///
/// Must run before any other thread starts: it sets an environment variable. A
/// `MYGAME_ASSETS` the user set themselves wins, and a failure to unpack is logged and
/// leaves the search as it was — the game then runs on procedural stand-ins.
pub fn install() {
    if std::env::var_os("MYGAME_ASSETS").is_some() {
        return;
    }
    let Some(root) = unpack_root() else {
        log::warn!("standalone: no writable folder to unpack content into");
        return;
    };
    if let Err(error) = unpack(&root) {
        log::warn!(
            "standalone: unpacking to {} failed: {error}",
            root.display()
        );
        return;
    }
    log::info!(
        "standalone: {} embedded file(s) at {}",
        embedded::FILES.len(),
        root.display()
    );
    // SAFETY: called at the top of `main`, before the engine, the audio device or any other
    // thread that could read the environment is started.
    unsafe { std::env::set_var("MYGAME_ASSETS", &root) };
}

/// `--scene graveyard` unless the command line names a scene of its own.
pub fn default_arguments(arguments: Vec<String>) -> Vec<String> {
    let names_scene = arguments
        .iter()
        .any(|argument| argument == "--scene" || argument == "--scene-file");
    if names_scene {
        return arguments;
    }
    let mut arguments = arguments;
    arguments.extend(["--scene".to_owned(), "graveyard".to_owned()]);
    arguments
}

fn unpack_root() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library").join("Caches"))
        .or_else(|| Some(std::env::temp_dir()))?;
    #[cfg(not(target_os = "macos"))]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| Some(std::env::temp_dir()))?;
    Some(
        base.join("mygame")
            .join(format!("assets-{:016x}", embedded::HASH)),
    )
}

fn unpack(root: &Path) -> std::io::Result<()> {
    if root.join(MARKER).is_file() {
        return Ok(());
    }
    for (relative, bytes) in embedded::FILES {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    std::fs::write(root.join(MARKER), b"")
}
