//! The seam between gameplay and rendering.
//!
//! ```text
//! engine-game            engine-presentation           engine-render-data
//! ------------           -------------------           ------------------
//! Simulation      -->    RenderExtractor        -->    RenderWorld
//! player body     -->    ThirdPersonCamera      -->    Camera
//! meter, stats    -->    hud::write_lines       -->    [String]
//! enemies, camera -->    NameplateSet           -->    screen-space plates
//! ```
//!
//! This crate exists because the two things it joins must not know about each other.
//! `engine-game` cannot name a `MeshHandle`; `engine-render-data` cannot name a `Health`.
//! Something has to decide that a dummy is a box of a certain size in a certain colour, and
//! that something is presentation.
//!
//! It links no D3D12 either — `engine-render-data` does not — so every rule about where a
//! character's mesh ends up, how the camera follows it, and what the overlay prints is unit
//! tested without a device, a window or an adapter.

pub mod camera;
pub mod extract;
pub mod hud;
pub mod nameplate;

pub use camera::ThirdPersonCamera;
pub use extract::{RenderExtractor, VisualBindings, VisualStats};
pub use hud::{HudModel, write_lines};
pub use nameplate::{
    MAX_NAMEPLATES, NAMEPLATE_MAX_DISTANCE, Nameplate, NameplateConfig, NameplateSet,
    NameplateStatus, project,
};
