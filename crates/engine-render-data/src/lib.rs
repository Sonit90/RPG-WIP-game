//! What the game hands the renderer, and nothing else.
//!
//! This crate is the seam between simulation and rendering:
//!
//! ```text
//! Input -> Commands -> Simulation -> RenderWorld -> Renderer -> GPU
//!                                    ^^^^^^^^^^^
//!                                    this crate
//! ```
//!
//! ## Why it is separate from `engine-renderer`
//!
//! Everything here is plain data over [`engine_core::math`] types. None of it needs a
//! device, a COM pointer or a `windows` type. Until Milestone 15 it lived in
//! `engine-renderer` next to the D3D12 pipeline objects, which meant anything that wanted
//! to *build* a `RenderWorld` had to link D3D12 — including a future gameplay crate, which
//! must not.
//!
//! The dependency arrow is therefore:
//!
//! ```text
//! engine-core
//!     ^        ^
//!     |        |
//! engine-render-data <- engine-renderer
//!     ^
//!     |
//! (future) gameplay
//! ```
//!
//! **This crate must never depend on `windows`.** That is not a style preference: it is the
//! single property that makes the seam real, and it is checkable with `cargo tree`.
//!
//! ## What does *not* live here
//!
//! * **`Culler`** — visibility is the renderer's policy, not data the game authors. It is
//!   pure CPU code and could compile here, but moving it would make this crate "the
//!   renderer minus D3D12" rather than "what the game produces".
//! * **`FrameConstants`, `ObjectConstants`** — plain data, but they are the CPU-side mirror
//!   of an HLSL `cbuffer`. Shader ABI belongs next to the shader.
//! * **`MeshData`, `MaterialDesc`** — asset-authoring data rather than scene data. Gameplay
//!   spawns entities with handles; it does not build vertex buffers. These are candidates
//!   for a future asset crate, not for this one.

pub mod camera;
pub mod error;
pub mod font;
pub mod lighting;
pub mod particles;
pub mod skinning;
pub mod ui;
pub mod world;

pub use camera::{Camera, DEFAULT_FAR, DEFAULT_FOV_Y, DEFAULT_NEAR, Projection};
pub use error::RenderDataError;
pub use font::UiFont;
pub use lighting::{Lighting, LocalLight, LocalLightKind};
pub use particles::{DEFAULT_PARTICLE_CAPACITY, ParticleBlend, ParticleDrawList, ParticleInstance};
pub use skinning::{
    MAX_PALETTE_BONES, ModelBinding, ModelLodBinding, ModelPart, PartGeometry, SkinnedDrawList,
    SkinnedInstance, SkinnedMeshHandle,
};
pub use ui::{
    MAX_ICON_PIXELS, TextAlign, TextRange, UiColor, UiCommand, UiDrawList, UiIcon, UiRect, UiScale,
};
pub use world::{
    CameraFade, Handle, InstanceStyle, MaterialHandle, MeshHandle, RenderEntity, RenderEntityDesc,
    RenderWorld, SHADOW_LOD_MESHES, ShadowCasting, world_bounds,
};
