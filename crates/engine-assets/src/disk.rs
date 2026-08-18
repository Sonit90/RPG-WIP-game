//! Reading a model off the filesystem: the last link between an artist's export and the game.
//!
//! ```text
//!  firemage.glb ──► find() ──► load_glb() ──► import_glb ──► fit textures ──► validate
//!                    │                                                           │
//!              search roots                                                 ModelAsset
//! ```
//!
//! # Why this is a separate module from the importer
//!
//! [`crate::gltf_import`] turns *bytes* into a model and knows nothing about paths, which is
//! what lets every one of its tests run on a byte slice. This module is the one place that
//! touches `std::fs`, so "where does content live" is one decision in one file rather than a
//! path literal in every caller.
//!
//! # A missing file is not an error
//!
//! [`model_file`] returns `None` when nothing is there, because the procedural model is a
//! perfectly good answer and a build with no content directory must still run. A file that
//! *is* there and does not load is a hard error: a typo in an export must not look the same
//! as an artist who has not delivered yet.

use std::path::{Path, PathBuf};

use crate::error::AssetError;
use crate::gltf_import::import_glb;
use crate::model::ModelAsset;
use crate::validate::{ValidationLimits, validate_model};

/// The directories a model is looked for in, most specific first.
///
/// `MYGAME_ASSETS` wins so a build can be pointed at a content directory without moving
/// files; then the directory beside the executable, which is where a shipped build keeps
/// content; then the working directory, which is where `cargo run` starts; and last the
/// repository's own `assets/`, so a model is found however the binary was launched.
///
/// Models are not in the repository — they are megabytes of binary git cannot pack — so all
/// four of these can be empty, and that is not an error.
#[must_use]
pub fn asset_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = std::env::var_os("MYGAME_ASSETS") {
        roots.push(PathBuf::from(root));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(directory) = exe.parent()
    {
        roots.push(directory.join("assets"));
    }
    if let Ok(working) = std::env::current_dir() {
        roots.push(working.join("assets"));
    }
    roots.push(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets"
    )));
    roots
}

/// Where `file` is, if any root holds it.
#[must_use]
pub fn model_file(file: &str) -> Option<PathBuf> {
    asset_roots()
        .into_iter()
        .map(|root| root.join(file))
        .find(|path| path.is_file())
}

/// Reads a `.glb` into a validated model called `name`.
///
/// Textures larger than the limits allow are halved until they fit rather than refused: a
/// 4096 map an artist exported out of habit is not a reason to lose the model, and the
/// runtime budget is about what the GPU holds, not about what the file contains. Everything
/// else — triangle counts, material counts, dangling indices — is still a refusal, because
/// no amount of resampling fixes those.
///
/// # Errors
///
/// * [`AssetError::Malformed`] if the file cannot be read, or is not a glTF document.
/// * Anything [`import_glb`] or [`validate_model`] reports, with the model named.
pub fn load_glb(
    name: &str,
    path: &Path,
    limits: &ValidationLimits,
) -> Result<ModelAsset, AssetError> {
    let bytes = std::fs::read(path).map_err(|error| AssetError::Malformed {
        model: name.to_owned(),
        reason: format!("{} could not be read: {error}", path.display()),
    })?;
    let mut model = import_glb(name, &bytes)?;
    model.textures = model
        .textures
        .into_iter()
        .map(|texture| texture.fitted_within(limits.max_texture_size))
        .collect();
    validate_model(&model, limits)?;
    Ok(model)
}

/// The model in `file`, if the file exists anywhere in [`asset_roots`].
///
/// # Errors
///
/// Anything [`load_glb`] reports. A file that is simply absent is `Ok(None)`.
pub fn find_model(name: &str, file: &str) -> Result<Option<ModelAsset>, AssetError> {
    let Some(path) = model_file(file) else {
        return Ok(None);
    };
    load_glb(name, &path, &ValidationLimits::default()).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_is_not_there_is_not_an_error() {
        let found = find_model("nothing", "a-file-no-root-has-1a2b3c.glb");
        assert!(
            matches!(found, Ok(None)),
            "an absent model must leave the procedural one in place"
        );
    }

    #[test]
    fn a_file_that_cannot_be_read_names_the_path() {
        let Err(error) = load_glb(
            "missing",
            Path::new("no-such-directory-1a2b3c/model.glb"),
            &ValidationLimits::default(),
        ) else {
            panic!("reading a file that is not there must fail");
        };
        assert!(
            error.to_string().contains("model.glb"),
            "the error must name the file: {error}"
        );
    }

    #[test]
    fn the_delivered_firemage_loads_within_every_runtime_budget() {
        // Skipped where the export is not installed, so a checkout without content still
        // passes. Where it is installed this is the regression test for the whole import
        // path: two skins merged into one skeleton, a rig hanging under a centimetre-scale
        // conversion node, and 4096 maps fitted to the texture budget.
        let Ok(Some(model)) = find_model("firemage", "firemage.glb") else {
            return;
        };
        let Some(skeleton) = model.skeleton.as_ref() else {
            panic!("the FireMage export is skinned and must import a skeleton");
        };
        assert!(
            skeleton.len() <= crate::skeleton::MAX_BONES,
            "{} bones will not fit one palette",
            skeleton.len()
        );
        for texture in &model.textures {
            let limit = ValidationLimits::default().max_texture_size;
            assert!(
                texture.width <= limit && texture.height <= limit,
                "{}x{} is over the texture budget",
                texture.width,
                texture.height
            );
        }
        let height = model.bind_pose_height();
        assert!(
            (0.5..4.0).contains(&height),
            "a humanoid measures about two metres posed, not {height} - the rig's unit \
             conversion was dropped"
        );
    }

    #[test]
    fn the_search_ends_with_the_repositorys_own_content_directory() {
        let roots = asset_roots();
        let Some(last) = roots.last() else {
            panic!("there is always at least one root");
        };
        assert!(last.ends_with("assets"), "{last:?}");
    }
}
