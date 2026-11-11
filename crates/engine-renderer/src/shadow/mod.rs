//! Shadow rendering: cascades for the sun, atlas tiles for local lights, and the depth
//! passes that fill them.
//!
//! ```text
//!            settings          what the system is asked for      (settings.rs)
//!               |
//!  shadow generation           cascades, local views             (cascade.rs, local.rs)
//!               |              caster culling and LOD            (caster.rs)
//!               v
//!  shadow resources            one atlas, its views, its layout  (atlas.rs, resources.rs)
//!               |
//!  shadow sampling             a factor in [0, 1]                (shaders/shadow_sampling.hlsl)
//!               |
//!  lighting                    multiplies the direct term        (shaders/mesh*.hlsl)
//! ```
//!
//! The separation §3 asks for is the arrow in the middle: the lighting shader consumes a
//! *shadow factor* and knows nothing about how it was produced, and this module produces one
//! and knows nothing about how it will be used. Nothing in here mentions a character, a
//! monster or a prop — the shadow passes read the same [`RenderWorld`] arrays the camera
//! cull reads, which is what lets anything the game spawns cast a shadow without a line of
//! code being written for it.
//!
//! # What lives where
//!
//! | Module | Needs a device | What it decides |
//! | --- | --- | --- |
//! | [`settings`] | no | quality presets, biases, budgets |
//! | [`cascade`] | no | splits, bounding spheres, stable light matrices, LOD |
//! | [`local`] | no | which lights get maps, and their projections |
//! | [`atlas`] | no | which texels each view owns |
//! | [`plan`] | no | one frame's cascades, caster lists and light selection |
//! | [`caster`] | no | which entities are drawn into a view |
//! | [`resources`] | yes | the atlas texture, its DSV and SRV, the frame constants |
//! | [`renderer`] | yes | recording the depth passes |
//!
//! Everything above the line is plain CPU code with unit tests, for the same reason
//! [`crate::culling`] is: a cascade matrix that is subtly wrong looks like a lighting bug,
//! and the cheapest place to catch it is an assertion.

pub mod atlas;
pub mod cascade;
pub mod caster;
#[cfg(windows)]
pub mod contact;
pub mod local;
pub mod pipeline;
pub mod plan;
#[cfg(windows)]
pub mod renderer;
pub mod resources;
pub mod settings;
pub mod static_cache;
pub mod stats;

pub use atlas::{AtlasRect, ShadowAtlasLayout};
pub use cascade::{
    CascadeSplit, CascadeView, build_cascades, cascade_blend, select_cascade, shadow_lod_for,
    split_cascades,
};
pub use caster::{CasterSlots, ShadowCasterCollector, ShadowCullStats};
#[cfg(windows)]
pub use contact::{CONTACT_FORMAT, ContactShadowPipeline, ContactShadowTarget};
pub use local::{LocalShadowView, SelectedLight, select_shadow_lights};
pub use plan::ShadowPlan;
#[cfg(windows)]
pub use renderer::{ShadowRenderer, ShadowScene};
#[cfg(windows)]
pub use resources::{SHADOW_FORMAT, ShadowResources};
pub use settings::{
    ContactShadowSettings, LocalShadowSettings, MAX_CASCADES, MAX_LOCAL_LIGHTS,
    MAX_LOCAL_SHADOW_VIEWS, PcfKernel, ShadowBias, ShadowLod, ShadowLodSettings, ShadowPreset,
    ShadowSettings,
};
pub use static_cache::{StaticCacheStats, StaticShadowCache, StaticView};
pub use stats::{ShadowPassStats, ShadowStats};
