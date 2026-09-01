//! What an asset blocks, as a file beside it.
//!
//! ```text
//!  cliff_overhang.glb            the geometry
//!  cliff_overhang.collision.json the boxes: the wall and the slab, and the space between
//! ```
//!
//! # Why a sidecar
//!
//! A box around the geometry is the right answer for a rock and the worst possible answer for
//! a tunnel: it fills the cavity the piece exists to provide. Something has to say what the
//! solid parts of a hollow asset are, and there are three places it could live.
//!
//! Inside the `.glb`, as a convention about node names, which every exporter would have to
//! honour and every artist would have to remember. In the engine, as a table keyed by asset
//! name, which stops working the moment somebody adds an asset the build was not compiled
//! with — the thing the whole asset catalogue exists to allow. Or beside the file.
//!
//! Beside the file: an artist delivering `arch.glb` can deliver `arch.collision.json` with it,
//! the editor reads it when the asset is placed, and an asset with no sidecar keeps the
//! automatic bounding box. No engine change, no exporter convention, nothing to remember for
//! the assets that do not need it.
//!
//! # It is read once, at placement
//!
//! What ends up in the level is the boxes, written into the scene file. The sidecar is an
//! *authoring input*, not something the game loads: a level built today keeps working if the
//! sidecar changes tomorrow, and the game has no idea these files exist.

use std::path::{Path, PathBuf};

use engine_core::math::Vec3;
use engine_environment::object::{CollisionBox, CollisionShape};
use serde::{Deserialize, Serialize};

use crate::error::AssetError;

/// The version this build writes, and the only one it reads.
pub const FORMAT_VERSION: u32 = 1;

/// The suffix that turns a model's path into its collision sidecar's path.
pub const SUFFIX: &str = ".collision.json";

/// What an asset blocks, as the file says it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollisionFile {
    /// Which version of this format the file is.
    pub version: u32,
    /// The solid parts, in the model's own space. What is *not* here is what a player can
    /// walk through.
    pub boxes: Vec<CollisionBoxRecord>,
}

/// One box, as the file says it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CollisionBoxRecord {
    /// Centre, in the model's local space.
    pub center: [f32; 3],
    /// Half-width, half-height, half-depth, in the model's local space.
    pub half_extents: [f32; 3],
}

impl CollisionFile {
    /// The file a shape would be written as.
    #[must_use]
    pub fn from_shape(shape: &CollisionShape) -> Self {
        Self {
            version: FORMAT_VERSION,
            boxes: shape
                .boxes
                .iter()
                .map(|shape| CollisionBoxRecord {
                    center: [shape.center.x, shape.center.y, shape.center.z],
                    half_extents: [
                        shape.half_extents.x,
                        shape.half_extents.y,
                        shape.half_extents.z,
                    ],
                })
                .collect(),
        }
    }

    /// The shape this file describes.
    ///
    /// Boxes with no volume are dropped by [`CollisionShape::new`]: a flat box is a shape
    /// every query tests and none can touch, and a file that names one is a mistake worth
    /// ignoring rather than carrying into the level.
    #[must_use]
    pub fn to_shape(&self) -> CollisionShape {
        CollisionShape::new(self.boxes.iter().map(|record| {
            CollisionBox::new(
                Vec3::new(record.center[0], record.center[1], record.center[2]),
                Vec3::new(
                    record.half_extents[0],
                    record.half_extents[1],
                    record.half_extents[2],
                ),
            )
        }))
    }

    /// Where the sidecar for a model file would be.
    ///
    /// `environment/cliffs/cliff_overhang.glb` → `environment/cliffs/cliff_overhang.collision.json`.
    #[must_use]
    pub fn path_for(model: &Path) -> PathBuf {
        let stem = model
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        model.with_file_name(format!("{stem}{SUFFIX}"))
    }

    /// Reads the sidecar beside `model`, if there is one.
    ///
    /// A missing sidecar is `Ok(None)`, because most assets do not need one — that is the
    /// whole point of the design. A sidecar that *is* there and will not parse is an error:
    /// an author who wrote one meant it, and silently falling back to a bounding box would
    /// hide the typo behind a cave nobody can walk into.
    ///
    /// # Errors
    ///
    /// [`AssetError::Malformed`] naming the file if it cannot be read or is not this format,
    /// and [`AssetError::Invalid`] if it is from another version.
    pub fn beside(model: &Path) -> Result<Option<CollisionShape>, AssetError> {
        let path = Self::path_for(model);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|error| AssetError::Malformed {
            model: path.display().to_string(),
            reason: error.to_string(),
        })?;
        let file: Self = serde_json::from_str(&text).map_err(|error| AssetError::Malformed {
            model: path.display().to_string(),
            reason: error.to_string(),
        })?;
        if file.version != FORMAT_VERSION {
            return Err(AssetError::invalid(
                path.display().to_string(),
                "version",
                format!(
                    "this build reads collision format {FORMAT_VERSION}, the file is {}",
                    file.version
                ),
            ));
        }
        Ok(Some(file.to_shape()))
    }

    /// Writes this file, creating the directory if it is not there.
    ///
    /// # Errors
    ///
    /// [`AssetError::Malformed`] naming the file if it cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), AssetError> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory).map_err(|error| AssetError::Malformed {
                model: path.display().to_string(),
                reason: error.to_string(),
            })?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| AssetError::Malformed {
            model: path.display().to_string(),
            reason: error.to_string(),
        })?;
        std::fs::write(path, text).map_err(|error| AssetError::Malformed {
            model: path.display().to_string(),
            reason: error.to_string(),
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::procedural::starter::starter_collision;

    fn temporary(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join("mygame_collision_file_tests");
        std::fs::create_dir_all(&directory).expect("a directory");
        directory.join(name)
    }

    #[test]
    fn a_shape_survives_being_written_and_read_back() {
        let model = temporary("cliff_overhang.glb");
        let original = starter_collision("cliff_overhang").expect("the overhang is authored");
        CollisionFile::from_shape(&original)
            .save(&CollisionFile::path_for(&model))
            .expect("it writes");

        let restored = CollisionFile::beside(&model)
            .expect("it reads")
            .expect("the sidecar is there");
        assert_eq!(restored, original);
        std::fs::remove_file(CollisionFile::path_for(&model)).ok();
    }

    #[test]
    fn the_sidecar_sits_beside_the_model_and_is_named_after_it() {
        let path = CollisionFile::path_for(Path::new("assets/environment/cliffs/x.glb"));
        assert!(path.ends_with("x.collision.json"), "{path:?}");
        assert_eq!(
            path.parent(),
            Path::new("assets/environment/cliffs/x.glb").parent()
        );
    }

    #[test]
    fn an_asset_with_no_sidecar_says_so_rather_than_failing() {
        let model = temporary("no_sidecar_here.glb");
        std::fs::remove_file(CollisionFile::path_for(&model)).ok();
        assert_eq!(CollisionFile::beside(&model).expect("no sidecar is fine"), None);
    }

    #[test]
    fn a_sidecar_that_will_not_parse_is_an_error_rather_than_a_bounding_box() {
        // The failure mode this rule exists for: a typo in a hand-written sidecar must not
        // quietly become a cave filled in with a solid box.
        let model = temporary("broken.glb");
        std::fs::write(CollisionFile::path_for(&model), b"{ not json").expect("it writes");
        assert!(CollisionFile::beside(&model).is_err());
        std::fs::remove_file(CollisionFile::path_for(&model)).ok();
    }

    #[test]
    fn a_file_from_another_version_says_so() {
        let model = temporary("future.glb");
        let mut file = CollisionFile::from_shape(&CollisionShape::new([CollisionBox::standing(
            1.0, 2.0, 1.0,
        )]));
        file.version = FORMAT_VERSION + 1;
        file.save(&CollisionFile::path_for(&model)).expect("it writes");
        let error = CollisionFile::beside(&model).expect_err("a future version is refused");
        assert!(format!("{error}").contains("collision format"), "{error}");
        std::fs::remove_file(CollisionFile::path_for(&model)).ok();
    }

    #[test]
    fn a_box_with_no_volume_is_dropped_rather_than_carried_into_the_level() {
        let file = CollisionFile {
            version: FORMAT_VERSION,
            boxes: vec![
                CollisionBoxRecord {
                    center: [0.0, 1.0, 0.0],
                    half_extents: [1.0, 1.0, 1.0],
                },
                CollisionBoxRecord {
                    center: [0.0, 0.0, 0.0],
                    half_extents: [1.0, 0.0, 1.0],
                },
            ],
        };
        assert_eq!(file.to_shape().len(), 1);
    }
}
