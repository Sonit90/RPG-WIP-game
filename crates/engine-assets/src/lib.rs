//! Character and prop assets: the runtime model representation, and the two ways to get one.
//!
//! ```text
//!  Source                Import                 Runtime asset            GPU
//!  ------                ------                 -------------            ---
//!  procedural::firemage  ─┐
//!                         ├─►  validate  ─────►  ModelAsset  ─────►  engine-renderer
//!  artist .glb  ─► gltf ─┘                       ├ lods / submeshes    uploads meshes,
//!                                                ├ materials           textures, and the
//!                                                ├ textures            skinning palette
//!                                                ├ skeleton
//!                                                └ animation clips
//! ```
//!
//! # What this crate is for
//!
//! One representation, two producers. A [`ModelAsset`] built by [`procedural`] and a
//! [`ModelAsset`] imported from a `.glb` an artist exported are the same type with the same
//! invariants, so replacing the first with the second changes a line of configuration and
//! nothing else. That is the whole point of the phase: gameplay names a character by
//! [`ModelId`], and what that resolves to is a data decision.
//!
//! # What this crate is not
//!
//! It has no device, no `windows` dependency and no rendering concept. It does not know what
//! a vertex buffer, a descriptor or a pipeline state is. That is what lets a model be built,
//! validated and animated in a unit test with no adapter present — and most of this crate's
//! tests do exactly that.
//!
//! It is equally not a gameplay crate. `engine-game` must never depend on it: a simulation
//! that can name a mesh is a simulation that can be broken by an artist.
//!
//! # Cost model
//!
//! Everything here happens at load, never per frame. Importing a `.glb`, building a
//! procedural character, and validating either are all one-time costs paid while a scene is
//! being assembled. The only per-frame work in this crate is [`AnimationPlayer`] sampling and
//! [`Skeleton::skinning_matrices`], which are both linear in bone count over flat arrays.

#![deny(missing_docs)]

pub mod animation;
pub mod asset_validator;
pub mod bc;
pub mod character;
pub mod disk;
pub mod error;
pub mod gltf_export;
pub mod gltf_import;
pub mod library;
pub mod material;
pub mod material_file;
pub mod mesh;
pub mod model;
pub mod procedural;
pub mod skeleton;
pub mod tangent;
pub mod texture;
pub mod validate;

pub use animation::{AnimationClip, AnimationPlayer, BoneTrack, Playback};
pub use asset_validator::{
    AssetCategory, AssetLimits, AssetSpec, AssetStatistics, AssetValidationResult, Diagnostic,
    Severity, ValidationCode, validate_asset,
};
pub use character::{AnimationSlot, CharacterDef, ClipBindings};
pub use error::AssetError;
pub use library::{ModelId, ModelLibrary};
pub use material::{MaterialAsset, SurfaceClass};
pub use material_file::MaterialFile;
pub use mesh::{DEFAULT_VERTEX_COLOR, Geometry, MeshVertex, SkinVertex, SubMesh};
pub use model::{ModelAsset, ModelLod};
pub use skeleton::{Bone, Pose, Skeleton};
pub use texture::{TextureAsset, TextureFormat};
pub use validate::{ValidationLimits, validate_model};
