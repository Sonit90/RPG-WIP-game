//! A level as a file: what the editor writes and the game reads.
//!
//! ```text
//!  assets/scenes/x.json ──► load ──► EnvironmentScene ──► the same three readers a
//!                    ▲                                    layout function feeds
//!                    └── save ◄── the in-game editor
//! ```
//!
//! # Why the file has its own types
//!
//! [`SceneDocument`] and its parts are not [`EnvironmentScene`] with `derive(Serialize)` on
//! it. A file is an *interface*: it outlives the build that wrote it, it is edited by hand
//! when something has to be fixed at two in the morning, and it must not change shape because
//! a runtime type gained a field. Keeping the two apart means the runtime struct stays free to
//! change and the schema changes only when somebody decides it should — and it keeps
//! `engine-core`'s math types out of the format, so a `Vec3` is three numbers in an array
//! rather than whatever serde would make of it.
//!
//! # What a broken file does
//!
//! Fails, naming what is wrong and where. This is the opposite of
//! [`engine_settings`](https://docs.rs)-style lenient loading, and deliberately: a HUD config
//! that half-parses costs a misplaced health bar, while a level that half-parses is a dungeon
//! missing a wall the designer thinks is there. An asset name the library does not contain,
//! a malformed number, a truncated file — each is an error that names the object's index.
//!
//! # Only names cross the boundary
//!
//! An [`EnvironmentAsset`] is resolved by looking its name up in [`crate::asset::LIBRARY`], so
//! the file never carries an index into a table whose order might change, and adding an asset
//! to the library does not renumber every scene ever written.

use std::path::Path;

use serde::{Deserialize, Serialize};

use engine_core::math::{Quat, Vec3};
use engine_core::transform::Transform;

use crate::asset::{EnvironmentAsset, LIBRARY};
use crate::lighting::EnvironmentLighting;
use crate::object::{EnvironmentCollision, EnvironmentObject};
use crate::scene::{EnvironmentScene, SpawnPoints};

/// The version this build writes, and the only one it reads.
///
/// One number in the file so that a format change is a message naming the version rather than
/// a confusing parse failure three fields in.
pub const FORMAT_VERSION: u32 = 1;

/// What went wrong with a scene file.
#[derive(Debug)]
pub enum SceneFileError {
    /// The file could not be read or written.
    Io {
        /// Which file.
        path: String,
        /// What the operating system said.
        reason: String,
    },
    /// The bytes are not the JSON this format expects.
    Malformed {
        /// Which file.
        path: String,
        /// What the parser said, including the line.
        reason: String,
    },
    /// The document parsed but says something impossible.
    Invalid {
        /// Which file.
        path: String,
        /// Where inside it: `"objects[17]"`, `"version"`.
        location: String,
        /// What is wrong.
        reason: String,
    },
}

impl std::fmt::Display for SceneFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, reason } => write!(formatter, "scene '{path}': {reason}"),
            Self::Malformed { path, reason } => {
                write!(formatter, "scene '{path}' is not valid JSON: {reason}")
            }
            Self::Invalid {
                path,
                location,
                reason,
            } => write!(formatter, "scene '{path}' at {location}: {reason}"),
        }
    }
}

impl std::error::Error for SceneFileError {}

/// One placement, as the file says it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectRecord {
    /// The asset's name, as [`crate::asset`] spells it.
    pub asset: String,
    /// Where it stands: `[x, y, z]` in metres.
    pub position: [f32; 3],
    /// How far it is turned about +Y, in radians.
    ///
    /// A yaw rather than a quaternion because everything in this game stands on the ground,
    /// and a number a human can read and edit is worth more here than the generality. A
    /// layout that needed pitch would carry a quaternion instead, and that is a format change
    /// with a version bump behind it.
    #[serde(default)]
    pub yaw: f32,
    /// Per-axis scale. One entry means uniform.
    #[serde(default = "unit_scale")]
    pub scale: [f32; 3],
    /// What it blocks. Absent means the library's default for the asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collision: Option<CollisionRecord>,
}

/// What an object blocks, as the file says it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollisionRecord {
    /// Nothing: the character walks through it.
    None,
    /// A box in the model's own space.
    Box {
        /// Centre, in the model's local space.
        center: [f32; 3],
        /// Half-width, half-height, half-depth, in the model's local space.
        half_extents: [f32; 3],
    },
    /// Several boxes, held once in the file's own table and named by index.
    ///
    /// What a cliff overhang and every cave piece carry: the shape is the *solid* parts, and
    /// the space a player walks through is what is left between them.
    Compound {
        /// Which entry of [`SceneDocument::collision_shapes`] this names.
        shape: usize,
    },
}

/// One compound collision shape, as the file says it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollisionShapeRecord {
    /// The boxes, in the model's local space.
    pub boxes: Vec<CollisionBoxRecord>,
}

/// One box of a compound shape, as the file says it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CollisionBoxRecord {
    /// Centre, in the model's local space.
    pub center: [f32; 3],
    /// Half-width, half-height, half-depth, in the model's local space.
    pub half_extents: [f32; 3],
}

/// Where the scene says things start.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpawnRecord {
    /// Where the player starts.
    pub player: [f32; 3],
    /// Where the single-target dummy stands.
    pub featured_dummy: [f32; 3],
    /// Centre of the dummy group.
    pub group_center: [f32; 3],
    /// Metres between dummies in the group.
    pub group_spacing: f32,
}

/// The sun, the fill and the sky.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LightingRecord {
    /// The direction the sun's light travels.
    pub sun_direction: [f32; 3],
    /// Linear colour of the sun.
    pub sun_color: [f32; 3],
    /// Sun strength.
    pub sun_intensity: f32,
    /// Linear ambient colour.
    pub ambient_color: [f32; 3],
    /// Ambient strength.
    pub ambient_intensity: f32,
    /// Linear colour straight overhead.
    pub sky_zenith: [f32; 3],
    /// Linear colour at the horizon.
    pub sky_horizon: [f32; 3],
}

/// A whole level, as the file says it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneDocument {
    /// Which schema this is. Refused if it is not [`FORMAT_VERSION`].
    pub version: u32,
    /// What the level is called.
    pub name: String,
    /// Half-width of the square a character is confined to, in metres.
    pub half_extent: f32,
    /// The y the floor is at.
    pub ground_height: f32,
    /// Where things start. Absent means the defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawns: Option<SpawnRecord>,
    /// The light. Absent means the arena's afternoon preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting: Option<LightingRecord>,
    /// Every placement, in draw order.
    pub objects: Vec<ObjectRecord>,
    /// The compound collision shapes the objects name by index.
    ///
    /// Written once however many placements refer to it, which is what makes a hillside of
    /// forty cliff pieces one shape and forty references rather than forty copies of the same
    /// four boxes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collision_shapes: Vec<CollisionShapeRecord>,
    /// The shape of the ground, if the level has one.
    ///
    /// Absent in every level written before terrain existed, and absent in a level nobody has
    /// sculpted, which is why it is optional rather than a version bump: a reader of this
    /// format that has never heard of terrain reads such a file unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain: Option<TerrainRecord>,
    /// The bodies of water the level has, if any.
    ///
    /// Absent rather than empty in a level with no water, for the reason the terrain is
    /// optional: a file written before water existed reads unchanged, and a level nobody has
    /// poured water into carries no water key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub water: Vec<WaterRecord>,
    /// Where a player can start, if the level says.
    ///
    /// Absent rather than empty in a level that does not, for the reason the water list is:
    /// a file written before spawn points existed reads unchanged, and a level that leans on
    /// the old `spawns.player` carries no key it does not use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawn_points: Vec<SpawnPointRecord>,
}

/// One spawn point, as the file says it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpawnPointRecord {
    /// Where the player stands, `[x, y, z]`, with `y` their feet.
    pub position: [f32; 3],
    /// Which way they face, radians about +Y. Absent means along -Z.
    #[serde(default)]
    pub yaw: f32,
}

/// One body of water, as the file says it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WaterRecord {
    /// The middle of the surface, `[x, y, z]`, with `y` the height the water sits at.
    pub center: [f32; 3],
    /// Half-width and half-depth of the rectangle, in metres, before rotation.
    pub half_extents: [f32; 2],
    /// Rotation about +Y, radians. Absent means none.
    #[serde(default)]
    pub yaw: f32,
    /// How far the volume reaches below the surface, in metres.
    pub depth: f32,
}

/// The ground's shape, as the file says it.
///
/// The heights are a flat list of numbers, row-major in `z`, which is what they are in memory
/// and what a person editing the file by hand would least mind looking at. A 129×129 terrain
/// is 16 641 numbers; nothing else in this format is remotely that large, and it is still
/// smaller than one texture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainRecord {
    /// The corner the grid starts at, `[x, y, z]`, with `y` the base height.
    pub origin: [f32; 3],
    /// Metres between grid points.
    pub cell_size: f32,
    /// Grid points a side.
    pub resolution: usize,
    /// Height above the origin's `y`, one per grid point, row-major in `z`.
    pub heights: Vec<f32>,
}

/// The default scale of a placement that does not name one.
fn unit_scale() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}

/// The asset in the built-in library called `name`.
///
/// By name rather than by index so that adding an asset does not renumber every scene ever
/// saved, and so a hand-edited file says `"pillar"` rather than `11`.
///
/// Returns `None` for a name the *compiled-in* library does not have, which since assets
/// began arriving from disk is no longer the same question as "is this a real asset" — see
/// [`asset_from_file`].
#[must_use]
pub fn asset_named(name: &str) -> Option<EnvironmentAsset> {
    LIBRARY.into_iter().find(|asset| asset.name() == name)
}

/// The asset id a scene file's name becomes.
///
/// A built-in library name resolves to its constant; anything else is interned, because it
/// names a file under `assets/` that this build was not compiled with any knowledge of.
///
/// # Why an unknown name is no longer a refusal
///
/// It used to be, and the reason was good: a typo in a hand-edited level should not become a
/// missing wall the designer thinks is there. But the library stopped being the set of assets
/// that exist the moment assets came from a directory — a build compiled last week does not
/// know about a rock added this morning, and refusing to load a level because of that would
/// make every new asset a code change.
///
/// The check did not disappear; it moved to where the answer is actually known. Whatever
/// resolves an id to a model reports a name it cannot find, and the level shows the "missing
/// asset" marker standing where the object should be — which is louder than a log line and
/// impossible to walk past.
///
/// An empty name is still refused here, because it names nothing that could ever exist.
///
/// # Errors
///
/// A message naming the problem, for a name that is empty or only whitespace.
pub fn asset_from_file(name: &str) -> Result<EnvironmentAsset, String> {
    if name.trim().is_empty() {
        return Err("an object must name an asset".to_owned());
    }
    Ok(EnvironmentAsset::interned(name))
}

impl SceneDocument {
    /// The document a scene would be written as.
    #[must_use]
    pub fn from_scene(scene: &EnvironmentScene) -> Self {
        Self {
            version: FORMAT_VERSION,
            name: scene.name.to_string(),
            half_extent: scene.half_extent,
            ground_height: scene.ground_height,
            spawns: Some(SpawnRecord {
                player: array(scene.spawns.player),
                featured_dummy: array(scene.spawns.featured_dummy),
                group_center: array(scene.spawns.group_center),
                group_spacing: scene.spawns.group_spacing,
            }),
            collision_shapes: scene
                .collision_shapes
                .iter()
                .map(|shape| CollisionShapeRecord {
                    boxes: shape
                        .boxes
                        .iter()
                        .map(|shape| CollisionBoxRecord {
                            center: array(shape.center),
                            half_extents: array(shape.half_extents),
                        })
                        .collect(),
                })
                .collect(),
            spawn_points: scene
                .spawn_points
                .iter()
                .map(|spawn| SpawnPointRecord {
                    position: array(spawn.position),
                    yaw: spawn.yaw,
                })
                .collect(),
            water: scene
                .water
                .iter()
                .map(|body| WaterRecord {
                    center: array(body.center),
                    half_extents: [body.half_extents.x, body.half_extents.y],
                    yaw: body.yaw,
                    depth: body.depth,
                })
                .collect(),
            terrain: scene.terrain.as_ref().map(|terrain| TerrainRecord {
                origin: array(terrain.origin),
                cell_size: terrain.cell_size,
                resolution: terrain.resolution,
                heights: terrain.heights.clone(),
            }),
            lighting: Some(LightingRecord {
                sun_direction: array(scene.lighting.sun_direction),
                sun_color: array(scene.lighting.sun_color),
                sun_intensity: scene.lighting.sun_intensity,
                ambient_color: array(scene.lighting.ambient_color),
                ambient_intensity: scene.lighting.ambient_intensity,
                sky_zenith: array(scene.lighting.sky_zenith),
                sky_horizon: array(scene.lighting.sky_horizon),
            }),
            objects: scene
                .objects
                .iter()
                .map(ObjectRecord::from_object)
                .collect(),
        }
    }

    /// The scene this document describes.
    ///
    /// # Errors
    ///
    /// [`SceneFileError::Invalid`] naming the object's index for an unknown asset, a
    /// non-finite number or a scale of zero, and naming `version` for a schema this build
    /// does not read.
    pub fn to_scene(&self, path: &str) -> Result<EnvironmentScene, SceneFileError> {
        if self.version != FORMAT_VERSION {
            return Err(SceneFileError::Invalid {
                path: path.to_owned(),
                location: "version".to_owned(),
                reason: format!(
                    "this build reads version {FORMAT_VERSION}, and the file is version {}",
                    self.version
                ),
            });
        }

        let mut scene = EnvironmentScene::with_capacity(self.name.clone(), self.objects.len());
        scene.half_extent = self.half_extent;
        scene.ground_height = self.ground_height;
        if let Some(spawns) = self.spawns {
            scene.spawns = SpawnPoints {
                player: vector(spawns.player),
                featured_dummy: vector(spawns.featured_dummy),
                group_center: vector(spawns.group_center),
                group_spacing: spawns.group_spacing,
            };
        }
        if let Some(lighting) = self.lighting {
            scene.lighting = EnvironmentLighting {
                sun_direction: vector(lighting.sun_direction),
                sun_color: vector(lighting.sun_color),
                sun_intensity: lighting.sun_intensity,
                ambient_color: vector(lighting.ambient_color),
                ambient_intensity: lighting.ambient_intensity,
                sky_zenith: vector(lighting.sky_zenith),
                sky_horizon: vector(lighting.sky_horizon),
            };
        }

        // The shape table first: an object that names a shape is checked against it below, and
        // a table that arrived after the objects would be a table nothing could be checked
        // against.
        scene.collision_shapes = self
            .collision_shapes
            .iter()
            .map(|record| crate::object::CollisionShape {
                boxes: record
                    .boxes
                    .iter()
                    .map(|shape| crate::object::CollisionBox {
                        center: vector(shape.center),
                        half_extents: vector(shape.half_extents),
                    })
                    .collect(),
            })
            .collect();

        if let Some(record) = &self.terrain {
            let terrain = crate::terrain::Terrain {
                origin: vector(record.origin),
                cell_size: record.cell_size,
                resolution: record.resolution,
                heights: record.heights.clone(),
            };
            // Checked rather than trusted, because everything that reads a terrain indexes
            // into its heights by its resolution: a file whose two numbers disagree would be
            // a panic somewhere far away from the file that caused it.
            if !terrain.is_valid() {
                return Err(SceneFileError::Invalid {
                    path: path.to_owned(),
                    location: "terrain".to_owned(),
                    reason: format!(
                        "{}x{} points at {} m needs {} finite heights, and the file has {}",
                        record.resolution,
                        record.resolution,
                        record.cell_size,
                        record.resolution * record.resolution,
                        record.heights.len()
                    ),
                });
            }
            scene.terrain = Some(terrain);
        }

        for (index, record) in self.water.iter().enumerate() {
            let body = crate::water::WaterBody {
                center: vector(record.center),
                half_extents: engine_core::math::Vec2::new(
                    record.half_extents[0],
                    record.half_extents[1],
                ),
                yaw: record.yaw,
                depth: record.depth,
            };
            // A body with a zero extent or a zero depth is a volume nothing can be inside and
            // a query that can never be true — a file that says so was edited by hand and got
            // it wrong, and the loader is where that is worth saying.
            if !body.is_valid() {
                return Err(SceneFileError::Invalid {
                    path: path.to_owned(),
                    location: format!("water[{index}]"),
                    reason: format!(
                        "a body needs finite numbers, positive half-extents and a positive                          depth; this one is {} x {} at depth {}",
                        record.half_extents[0], record.half_extents[1], record.depth
                    ),
                });
            }
            scene.push_water(body);
        }

        for (index, record) in self.spawn_points.iter().enumerate() {
            let spawn = crate::spawn::SpawnPoint {
                position: vector(record.position),
                yaw: record.yaw,
            };
            // Everything that reads one puts a character at it, and a character at a position
            // that is not a number is a character nothing can find again.
            if !spawn.is_valid() {
                return Err(SceneFileError::Invalid {
                    path: path.to_owned(),
                    location: format!("spawn_points[{index}]"),
                    reason: "a spawn point needs a finite position and a finite yaw".to_owned(),
                });
            }
            scene.push_spawn(spawn);
        }

        for (index, record) in self.objects.iter().enumerate() {
            let invalid = |reason: String| SceneFileError::Invalid {
                path: path.to_owned(),
                location: format!("objects[{index}]"),
                reason,
            };
            let asset = asset_from_file(&record.asset).map_err(&invalid)?;
            if let Some(CollisionRecord::Compound { shape }) = record.collision
                && shape >= scene.collision_shapes.len()
            {
                return Err(invalid(format!(
                    "collision shape {shape} was named, and the level has {}",
                    scene.collision_shapes.len()
                )));
            }
            let finite = record.position.iter().all(|value| value.is_finite())
                && record.scale.iter().all(|value| value.is_finite())
                && record.yaw.is_finite();
            if !finite {
                return Err(invalid(
                    "a position, scale or yaw is not a number".to_owned(),
                ));
            }
            if record.scale.iter().any(|value| value.abs() < 1e-6) {
                return Err(invalid(
                    "a scale of zero collapses the mesh and makes its bounds meaningless"
                        .to_owned(),
                ));
            }
            scene.push(record.to_object(asset));
        }

        Ok(scene)
    }
}

impl ObjectRecord {
    /// The record a placement would be written as.
    ///
    /// The rotation is written as a yaw, which is what every placement in this game actually
    /// has. A transform carrying pitch or roll loses it here — nothing authors one, and the
    /// alternative is a quaternion in the file that no human can read.
    #[must_use]
    pub fn from_object(object: &EnvironmentObject) -> Self {
        Self {
            asset: object.asset.name().to_owned(),
            position: array(object.transform.translation),
            yaw: yaw_of(object.transform.rotation),
            scale: array(object.transform.scale),
            collision: Some(match object.collision {
                EnvironmentCollision::None => CollisionRecord::None,
                EnvironmentCollision::Box {
                    center,
                    half_extents,
                } => CollisionRecord::Box {
                    center: array(center),
                    half_extents: array(half_extents),
                },
                EnvironmentCollision::Compound(shape) => CollisionRecord::Compound {
                    shape: shape as usize,
                },
            }),
        }
    }

    /// The placement this record describes, given the asset its name resolved to.
    #[must_use]
    pub fn to_object(&self, asset: EnvironmentAsset) -> EnvironmentObject {
        EnvironmentObject {
            asset,
            transform: Transform {
                translation: vector(self.position),
                rotation: Quat::from_yaw_pitch_roll(self.yaw, 0.0, 0.0),
                scale: vector(self.scale),
            },
            collision: match self.collision {
                Some(CollisionRecord::None) => EnvironmentCollision::None,
                Some(CollisionRecord::Box {
                    center,
                    half_extents,
                }) => EnvironmentCollision::Box {
                    center: vector(center),
                    half_extents: vector(half_extents),
                },
                // Range-checked where the document is turned into a scene, which is the one
                // place that knows how many shapes the file brought; here it is carried
                // across as written.
                Some(CollisionRecord::Compound { shape }) => {
                    EnvironmentCollision::Compound(u16::try_from(shape).unwrap_or(u16::MAX))
                }
                // An object that says nothing about collision gets what the library says the
                // asset blocks. That is what makes a hand-written or Blender-authored file
                // short: `{"asset": "pillar", "position": [4, 0, 0]}` is a pillar you cannot
                // walk through.
                None => crate::object::default_collision(asset),
            },
        }
    }
}

/// Writes `scene` to `path`, creating the directory if it is not there.
///
/// # Errors
///
/// [`SceneFileError::Io`] if the directory cannot be created or the file cannot be written.
pub fn save(scene: &EnvironmentScene, path: impl AsRef<Path>) -> Result<(), SceneFileError> {
    let path = path.as_ref();
    let display = path.display().to_string();
    let document = SceneDocument::from_scene(scene);
    let text = serde_json::to_string_pretty(&document).map_err(|error| SceneFileError::Io {
        path: display.clone(),
        reason: error.to_string(),
    })?;
    if let Some(directory) = path.parent()
        && !directory.as_os_str().is_empty()
    {
        std::fs::create_dir_all(directory).map_err(|error| SceneFileError::Io {
            path: display.clone(),
            reason: format!("the directory could not be created: {error}"),
        })?;
    }
    std::fs::write(path, text).map_err(|error| SceneFileError::Io {
        path: display,
        reason: error.to_string(),
    })
}

/// Reads the scene in `path`.
///
/// # Errors
///
/// [`SceneFileError::Io`] if the file cannot be read, [`SceneFileError::Malformed`] if it is
/// not JSON of this shape, and [`SceneFileError::Invalid`] for anything the document says that
/// cannot be built — each naming the place inside the file.
pub fn load(path: impl AsRef<Path>) -> Result<EnvironmentScene, SceneFileError> {
    let path = path.as_ref();
    let display = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|error| SceneFileError::Io {
        path: display.clone(),
        reason: error.to_string(),
    })?;
    let document: SceneDocument =
        serde_json::from_str(&text).map_err(|error| SceneFileError::Malformed {
            path: display.clone(),
            reason: error.to_string(),
        })?;
    document.to_scene(&display)
}

/// A vector as three numbers.
fn array(vector: Vec3) -> [f32; 3] {
    [vector.x, vector.y, vector.z]
}

/// Three numbers as a vector.
fn vector(values: [f32; 3]) -> Vec3 {
    Vec3::new(values[0], values[1], values[2])
}

/// How far a rotation turns about +Y.
///
/// Everything in this game stands on the ground, so a placement's rotation is a yaw and this
/// recovers it exactly. A quaternion carrying pitch or roll returns the yaw part of it, which
/// is the closest thing the format can say.
fn yaw_of(rotation: Quat) -> f32 {
    let siny = 2.0 * (rotation.w * rotation.y + rotation.z * rotation.x);
    let cosy = 1.0 - 2.0 * (rotation.x * rotation.x + rotation.y * rotation.y);
    siny.atan2(cosy)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::asset::{FERN, PILLAR, WALL_SEGMENT};

    fn scene() -> EnvironmentScene {
        let mut scene = EnvironmentScene::with_capacity("test_level", 3);
        scene.half_extent = 33.0;
        scene.ground_height = 0.5;
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::new(4.0, 0.0, -8.0))
                .with_collision(EnvironmentCollision::standing(0.62, 4.6, 0.62)),
        );
        scene.push(
            EnvironmentObject::decoration(WALL_SEGMENT, Vec3::new(-12.0, 0.0, 3.0))
                .with_yaw(std::f32::consts::FRAC_PI_2)
                .with_collision(EnvironmentCollision::standing(4.0, 2.4, 0.45)),
        );
        scene.push(EnvironmentObject::decoration(FERN, Vec3::new(1.0, 0.0, 1.0)).with_scale(1.4));
        scene
    }

    /// Asserts two scenes describe the same level, to the precision a file has.
    ///
    /// Not [`EnvironmentScene::signature`], which hashes float *bits*: a rotation written as
    /// a yaw and rebuilt from it is the same rotation and rarely the same bit pattern, and a
    /// bit-exact assertion here would be a test that fails on a correct round trip. Every
    /// number is compared to a tolerance far tighter than anything a level cares about.
    fn assert_same_level(restored: &EnvironmentScene, original: &EnvironmentScene) {
        assert_eq!(restored.name, original.name);
        assert!((restored.half_extent - original.half_extent).abs() < 1e-5);
        assert!((restored.ground_height - original.ground_height).abs() < 1e-5);
        assert_eq!(restored.spawns, original.spawns);
        assert_eq!(restored.lighting, original.lighting);
        assert_eq!(
            restored.objects.len(),
            original.objects.len(),
            "a placement was lost"
        );

        for (index, (restored, original)) in
            restored.objects.iter().zip(&original.objects).enumerate()
        {
            assert_eq!(restored.asset, original.asset, "objects[{index}]");
            assert!(
                (restored.position() - original.position()).length() < 1e-4,
                "objects[{index}] moved"
            );
            assert!(
                (restored.transform.scale - original.transform.scale).length() < 1e-4,
                "objects[{index}] changed size"
            );
            // The rotation compared by what it does to a vector, not by its components: a
            // quaternion and its negation are the same rotation.
            let axis = Vec3::new(1.0, 0.0, 0.0);
            assert!(
                (restored.transform.rotation.rotate(axis)
                    - original.transform.rotation.rotate(axis))
                .length()
                    < 1e-4,
                "objects[{index}] turned"
            );
            assert_eq!(
                restored.collision, original.collision,
                "objects[{index}] blocks differently"
            );
        }
    }

    #[test]
    fn a_scene_survives_the_round_trip_placement_for_placement() {
        let original = scene();
        let document = SceneDocument::from_scene(&original);
        let restored = document.to_scene("memory").expect("the document is valid");
        assert_same_level(&restored, &original);
    }

    #[test]
    fn a_file_round_trips_through_the_disk_too() {
        let path = std::env::temp_dir()
            .join("mygame_scene_tests")
            .join("round_trip.json");
        let original = scene();
        save(&original, &path).expect("the scene saves");
        let restored = load(&path).expect("the scene loads");
        assert_same_level(&restored, &original);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_terrain_survives_the_round_trip_height_for_height() {
        let mut original = scene();
        let mut terrain = crate::terrain::Terrain::flat(17, 2.0, 0.5);
        terrain.apply_brush(Vec3::new(4.0, 0.0, 4.0), 8.0, 3.0, crate::BrushMode::Raise);
        original.terrain = Some(terrain.clone());

        let restored = SceneDocument::from_scene(&original)
            .to_scene("memory")
            .expect("the document is valid");
        assert_eq!(restored.terrain, Some(terrain));
    }

    #[test]
    fn water_survives_the_round_trip_and_an_empty_level_writes_none_of_it() {
        use engine_core::math::Vec2;

        let mut original = scene();
        original.push_water(crate::water::WaterBody::new(
            Vec3::new(4.0, -0.5, -6.0),
            Vec2::new(9.0, 3.5),
            2.5,
        ));
        original.push_water(
            crate::water::WaterBody::new(Vec3::new(-20.0, 3.0, 8.0), Vec2::new(4.0, 4.0), 1.25)
                .with_yaw(0.75),
        );

        let document = SceneDocument::from_scene(&original);
        let text = serde_json::to_string(&document).expect("a level with water is writable");
        assert!(text.contains("\"water\""), "the key is in the file");
        let restored: SceneDocument = serde_json::from_str(&text).expect("and readable again");
        let restored = restored.to_scene("memory").expect("and valid");
        assert_eq!(restored.water, original.water);

        // And a level with none of it carries no key at all, so a file written before water
        // existed and a file written by this build of a level without any are the same file.
        let dry = serde_json::to_string(&SceneDocument::from_scene(&scene())).expect("writable");
        assert!(
            !dry.contains("water"),
            "an absent water list was written anyway"
        );
    }

    #[test]
    fn spawn_points_survive_the_round_trip_and_an_empty_level_writes_none_of_them() {
        let mut original = scene();
        original.push_spawn(crate::spawn::SpawnPoint::new(Vec3::new(2.0, 1.0, -3.0)).with_yaw(0.5));
        original.push_spawn(crate::spawn::SpawnPoint::new(Vec3::new(-8.0, 0.0, 12.0)));

        let text = serde_json::to_string(&SceneDocument::from_scene(&original)).expect("writable");
        assert!(text.contains("spawn_points"), "the key is in the file");
        let restored: SceneDocument = serde_json::from_str(&text).expect("readable");
        let restored = restored.to_scene("memory").expect("and valid");
        assert_eq!(restored.spawn_points, original.spawn_points);

        let dry = serde_json::to_string(&SceneDocument::from_scene(&scene())).expect("writable");
        assert!(
            !dry.contains("spawn_points"),
            "an absent list was written anyway"
        );
    }

    #[test]
    fn a_spawn_point_that_is_not_a_place_is_refused_and_the_message_names_it() {
        let mut document = SceneDocument::from_scene(&scene());
        document.spawn_points = vec![
            SpawnPointRecord {
                position: [0.0, 0.0, 0.0],
                yaw: 0.0,
            },
            SpawnPointRecord {
                position: [0.0, f32::NAN, 0.0],
                yaw: 0.0,
            },
        ];
        let error = document
            .to_scene("memory")
            .expect_err("a spawn point at nowhere is refused");
        assert!(error.to_string().contains("spawn_points[1]"), "{error}");
    }

    #[test]
    fn a_water_body_with_no_size_is_refused_and_the_message_names_it() {
        let mut document = SceneDocument::from_scene(&scene());
        document.water = vec![
            WaterRecord {
                center: [0.0, 0.0, 0.0],
                half_extents: [4.0, 4.0],
                yaw: 0.0,
                depth: 2.0,
            },
            WaterRecord {
                center: [0.0, 0.0, 0.0],
                half_extents: [0.0, 4.0],
                yaw: 0.0,
                depth: 2.0,
            },
        ];
        let error = document
            .to_scene("memory")
            .expect_err("a body with no width holds nothing");
        let message = error.to_string();
        assert!(message.contains("water[1]"), "{message}");
    }

    #[test]
    fn a_level_with_no_terrain_writes_no_terrain_at_all() {
        // The field is optional so that a file written before terrain existed still reads, and
        // so that a flat level does not carry sixteen thousand zeroes for nothing.
        let document = SceneDocument::from_scene(&scene());
        assert!(document.terrain.is_none());
        let text = serde_json::to_string(&document).expect("it serializes");
        assert!(
            !text.contains("terrain"),
            "an absent terrain was written anyway"
        );
    }

    #[test]
    fn a_terrain_whose_numbers_disagree_is_refused_rather_than_indexed_into() {
        let mut document = SceneDocument::from_scene(&scene());
        document.terrain = Some(TerrainRecord {
            origin: [0.0, 0.0, 0.0],
            cell_size: 2.0,
            resolution: 9,
            heights: vec![0.0; 4],
        });
        let error = document
            .to_scene("broken.json")
            .expect_err("81 points and 4 heights is not a surface");
        let message = error.to_string();
        assert!(message.contains("terrain"), "{message}");
    }

    #[test]
    fn a_new_empty_level_saves_and_reopens_as_an_empty_level() {
        // What the editor writes the first time somebody makes a level and saves it before
        // placing anything: a file with no objects in it must be a valid level, not a
        // malformed one, or "new level" would be a state that cannot be saved.
        let path = std::env::temp_dir()
            .join("mygame_scene_tests")
            .join("empty_level.json");
        let original = EnvironmentScene::empty("new_level");
        save(&original, &path).expect("an empty scene saves");
        let restored = load(&path).expect("an empty scene loads");
        assert!(restored.is_empty());
        assert_eq!(
            restored.name, "new_level",
            "the level keeps its name, not the file stem"
        );
        assert_same_level(&restored, &original);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_dungeon_itself_survives_the_round_trip() {
        // The real test of the format: 800-odd placements of eight assets, every one of them
        // carrying an authored collision box, written and read back.
        let original = crate::dungeon::ashen_reliquary();
        let restored = SceneDocument::from_scene(&original)
            .to_scene("memory")
            .expect("the dungeon is describable");
        assert_same_level(&restored, &original);
    }

    #[test]
    fn an_asset_the_compiled_library_does_not_have_loads_as_an_interned_id() {
        let document = SceneDocument {
            version: FORMAT_VERSION,
            name: "broken".to_owned(),
            half_extent: 40.0,
            ground_height: 0.0,
            spawns: None,
            lighting: None,
            collision_shapes: Vec::new(),
            terrain: None,
            water: Vec::new(),
            spawn_points: Vec::new(),
            objects: vec![
                ObjectRecord {
                    asset: "pillar".to_owned(),
                    position: [0.0; 3],
                    yaw: 0.0,
                    scale: unit_scale(),
                    collision: None,
                },
                ObjectRecord {
                    // A name this build was not compiled with: a rock somebody added to
                    // assets/ this morning, or a typo. The file cannot tell the two apart and
                    // no longer tries — whatever resolves an id to a model reports the one it
                    // cannot find, and the level shows a marker standing where it should be.
                    asset: "generated/environment/rocks/added_this_morning".to_owned(),
                    position: [0.0; 3],
                    yaw: 0.0,
                    scale: unit_scale(),
                    collision: None,
                },
            ],
        };
        let scene = document
            .to_scene("broken.json")
            .expect("an unknown name still loads");
        assert_eq!(scene.len(), 2);
        assert_eq!(
            scene.objects[1].asset.name(),
            "generated/environment/rocks/added_this_morning"
        );
        assert_eq!(scene.objects[0].asset, crate::asset::PILLAR);
    }

    #[test]
    fn an_object_that_names_no_asset_at_all_is_still_refused() {
        let document = SceneDocument {
            version: FORMAT_VERSION,
            name: "broken".to_owned(),
            half_extent: 40.0,
            ground_height: 0.0,
            spawns: None,
            lighting: None,
            collision_shapes: Vec::new(),
            terrain: None,
            water: Vec::new(),
            spawn_points: Vec::new(),
            objects: vec![ObjectRecord {
                asset: "   ".to_owned(),
                position: [0.0; 3],
                yaw: 0.0,
                scale: unit_scale(),
                collision: None,
            }],
        };
        let error = document
            .to_scene("broken.json")
            .expect_err("an object with no asset names nothing that could exist");
        let message = error.to_string();
        assert!(message.contains("objects[0]"), "{message}");
    }

    #[test]
    fn an_object_that_says_nothing_about_collision_gets_the_library_default() {
        let record = ObjectRecord {
            asset: "pillar".to_owned(),
            position: [1.0, 0.0, 2.0],
            yaw: 0.0,
            scale: unit_scale(),
            collision: None,
        };
        assert!(
            record.to_object(PILLAR).collision.blocks(),
            "a pillar nobody described still stops a character"
        );
    }

    #[test]
    fn a_file_from_another_version_says_so_rather_than_half_loading() {
        let mut document = SceneDocument::from_scene(&scene());
        document.version = FORMAT_VERSION + 7;
        let Err(error) = document.to_scene("future.json") else {
            panic!("a future version must not load");
        };
        assert!(error.to_string().contains("version"), "{error}");
    }

    #[test]
    fn a_truncated_file_is_an_error_and_not_a_panic() {
        let path = std::env::temp_dir()
            .join("mygame_scene_tests")
            .join("truncated.json");
        std::fs::create_dir_all(path.parent().expect("it has a parent")).expect("the directory");
        std::fs::write(&path, "{\"version\": 1, \"name\": \"half").expect("the file");
        assert!(matches!(load(&path), Err(SceneFileError::Malformed { .. })));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_names_itself() {
        let Err(error) = load("no-such-directory-4f2a/level.json") else {
            panic!("a file that is not there cannot load");
        };
        assert!(error.to_string().contains("level.json"), "{error}");
    }

    #[test]
    fn a_degenerate_scale_is_refused_because_its_bounds_would_be_meaningless() {
        let document = SceneDocument {
            version: FORMAT_VERSION,
            name: "flat".to_owned(),
            half_extent: 40.0,
            ground_height: 0.0,
            spawns: None,
            lighting: None,
            collision_shapes: Vec::new(),
            terrain: None,
            water: Vec::new(),
            spawn_points: Vec::new(),
            objects: vec![ObjectRecord {
                asset: "pillar".to_owned(),
                position: [0.0; 3],
                yaw: 0.0,
                scale: [1.0, 0.0, 1.0],
                collision: None,
            }],
        };
        assert!(document.to_scene("flat.json").is_err());
    }

    #[test]
    fn a_yaw_survives_being_written_as_a_number_and_read_back_as_a_rotation() {
        for turns in [0.0_f32, 0.25, 0.5, -0.75] {
            let yaw = turns * std::f32::consts::TAU;
            let object = EnvironmentObject::decoration(PILLAR, Vec3::ZERO).with_yaw(yaw);
            let restored = ObjectRecord::from_object(&object).to_object(PILLAR);
            let turned = restored.transform.rotation.rotate(Vec3::new(1.0, 0.0, 0.0));
            let expected = object.transform.rotation.rotate(Vec3::new(1.0, 0.0, 0.0));
            assert!(
                (turned - expected).length() < 1e-4,
                "{yaw} came back as a different rotation"
            );
        }
    }
}
