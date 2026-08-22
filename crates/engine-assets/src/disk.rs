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

use engine_environment::object::default_collision;
use engine_environment::scene_file::asset_named;
use engine_environment::{EnvironmentObject, EnvironmentScene};

use crate::error::AssetError;
use crate::gltf_import::{LayoutNode, import_glb, import_layout};
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

/// What a layout export contained, for the line the loader logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LayoutReport {
    /// Nodes that became placements.
    pub placed: usize,
    /// Named nodes that matched no asset and no spawn point.
    pub skipped: usize,
}

/// Reads a Blender layout `.glb` as a scene.
///
/// # How a node becomes a placement
///
/// By its **name**. `pillar`, `pillar.001` and `Pillar.042` are all the `pillar` asset —
/// Blender's duplicate suffix is cut at the first dot and the comparison ignores case, which
/// is the whole of the naming convention an artist has to learn. A node named
/// `spawn_player`, `spawn_featured_dummy` or `spawn_group` moves that spawn point instead of
/// placing anything.
///
/// Collision comes from [`default_collision`], so a wall built in Blender blocks without the
/// author saying so. A layout that needs something else says so in the scene file the editor
/// writes; this is the fast path from a modelling package, not the last word.
///
/// A name that matches nothing is **skipped and counted**, never guessed at: an export always
/// carries cameras, lights and collections, and treating an unrecognised name as an error
/// would make the common export fail.
///
/// # Errors
///
/// [`AssetError::Malformed`] if the file cannot be read or is not a glTF document, and
/// [`AssetError::Invalid`] if nothing in it named an asset — which means the naming
/// convention was not followed, and is worth failing over rather than opening an empty level.
pub fn load_layout(
    name: &str,
    path: &Path,
) -> Result<(EnvironmentScene, LayoutReport), AssetError> {
    let bytes = std::fs::read(path).map_err(|error| AssetError::Malformed {
        model: name.to_owned(),
        reason: format!("{} could not be read: {error}", path.display()),
    })?;
    let nodes = import_layout(name, &bytes)?;
    scene_from_layout(name, &nodes)
}

/// The scene a layout export's nodes describe.
///
/// Separate from [`load_layout`] so the mapping is testable without a file.
///
/// # Errors
///
/// [`AssetError::Invalid`] if no node named an asset.
pub fn scene_from_layout(
    name: &str,
    nodes: &[LayoutNode],
) -> Result<(EnvironmentScene, LayoutReport), AssetError> {
    let mut scene = EnvironmentScene::with_capacity(name.to_owned(), nodes.len());
    let mut report = LayoutReport::default();

    for node in nodes {
        let key = layout_key(&node.name);
        if let Some(asset) = asset_named(&key) {
            scene.push(EnvironmentObject {
                asset,
                transform: node.transform,
                collision: default_collision(asset),
            });
            report.placed += 1;
            continue;
        }
        match key.as_str() {
            "spawn_player" => scene.spawns.player = node.transform.translation,
            "spawn_featured_dummy" => scene.spawns.featured_dummy = node.transform.translation,
            "spawn_group" => scene.spawns.group_center = node.transform.translation,
            _ => report.skipped += 1,
        }
    }

    if report.placed == 0 {
        return Err(AssetError::invalid(
            name,
            "layout",
            "no node named an environment asset; name each object after the asset it stands \
             for, as in 'pillar.001'",
        ));
    }

    // Room to walk, taken from what was actually placed. A layout that says nothing about its
    // bounds should not confine the player to a square smaller than the level.
    scene.half_extent = scene.half_extent.max(scene.horizontal_reach() + 4.0);
    Ok((scene, report))
}

/// The asset name a node name means.
///
/// Blender's `.001` duplicate suffix is cut, an exporter's `Collection:` prefix is cut, and
/// case is ignored. Everything else is left alone, because a name that has been mangled
/// further than this should be fixed in the export rather than guessed at here.
fn layout_key(node_name: &str) -> String {
    let after_prefix = node_name.rsplit(':').next().unwrap_or(node_name);
    let before_suffix = after_prefix.split('.').next().unwrap_or(after_prefix);
    before_suffix.trim().to_lowercase()
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

    fn node(name: &str, x: f32, z: f32) -> LayoutNode {
        LayoutNode {
            name: name.to_owned(),
            transform: engine_core::transform::Transform::from_translation(
                engine_core::math::Vec3::new(x, 0.0, z),
            ),
        }
    }

    #[test]
    fn a_layout_places_what_its_nodes_are_named_after_and_counts_what_it_skipped() {
        let nodes = vec![
            node("pillar", 4.0, 0.0),
            node("Pillar.001", -4.0, 0.0),
            node("wall_segment.042", 0.0, 8.0),
            node("Collection:pine.003", 20.0, 20.0),
            node("spawn_player", 1.0, 2.0),
            node("Camera", 0.0, 0.0),
            node("Light.001", 0.0, 0.0),
        ];
        let Ok((scene, report)) = scene_from_layout("test_layout", &nodes) else {
            panic!("a layout with assets in it loads");
        };
        assert_eq!(report.placed, 4, "four nodes named an asset");
        assert_eq!(report.skipped, 2, "the camera and the light are not assets");
        assert_eq!(
            scene.spawns.player,
            engine_core::math::Vec3::new(1.0, 0.0, 2.0)
        );

        let Some(pillar) = asset_named("pillar") else {
            panic!("the library has a pillar");
        };
        assert_eq!(
            scene.count_of(pillar),
            2,
            "the .001 duplicate is a pillar too"
        );
        assert!(
            scene.objects.iter().all(|object| object.collision.blocks()),
            "a wall built in Blender blocks without anyone saying so"
        );
    }

    #[test]
    fn a_layout_that_follows_no_naming_convention_fails_rather_than_opening_empty() {
        let nodes = vec![node("Cube", 0.0, 0.0), node("Cube.001", 1.0, 0.0)];
        let Err(error) = scene_from_layout("nameless", &nodes) else {
            panic!("a layout with no recognisable node must not load as an empty level");
        };
        assert!(error.to_string().contains("pillar.001"), "{error}");
    }

    #[test]
    fn the_playable_square_grows_to_hold_what_was_placed() {
        let nodes = vec![node("pillar", 300.0, 0.0)];
        let Ok((scene, _)) = scene_from_layout("wide", &nodes) else {
            panic!("it loads");
        };
        assert!(
            scene.half_extent > 300.0,
            "the player would be walled off from half the level: {}",
            scene.half_extent
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
