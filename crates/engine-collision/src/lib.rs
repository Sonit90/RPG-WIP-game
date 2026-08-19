//! Deterministic collision and character movement.
//!
//! ```text
//! Input
//!   |
//!   v
//! Character Controller      crate::controller
//!   |
//!   v
//! Collision World           crate::world
//!   +-- Broad Phase         crate::broad     (which objects are worth testing)
//!   +-- Narrow Phase        crate::narrow    (are they actually touching)
//!   |
//!   v
//! Resolved position + velocity + ground state
//! ```
//!
//! ## What this crate is for
//!
//! Moving one capsule through static geometry, correctly and repeatably. That is the whole
//! scope. There are no rigid bodies, no joints, no ragdolls, no vehicles, no cloth and no
//! solver, and none of them are planned here: this is the collision a third-person action
//! game needs to be playable, not a general-purpose physics engine.
//!
//! ## What it depends on
//!
//! `engine-core`, for [`engine_core::math`]. Nothing else, ever. No `windows`, no D3D12, no
//! renderer, no render data, no UI, no window. Collision is a simulation concern, and a
//! headless test — or a future dedicated server — has to be able to resolve a character
//! move with no device in the process.
//!
//! ## The shape vocabulary
//!
//! Small on purpose:
//!
//! | Shape | What it is for |
//! | --- | --- |
//! | [`shape::Capsule`] | the character, and only the character |
//! | [`engine_core::math::Aabb`] | walls, steps, platforms |
//! | [`shape::Obb`] | ramps |
//! | [`engine_core::math::Plane`] | the floor |
//!
//! The character's capsule is **not** derived from its mesh. A model with ten thousand
//! triangles collides as a 0.4 m capsule, and that separation is deliberate: collision
//! behaviour must not change when an artist changes a silhouette.
//!
//! ## Example
//!
//! ```
//! use engine_collision::prelude::*;
//! use engine_core::math::Vec3;
//!
//! let mut world = CollisionWorld::new();
//! world.add_ground(0.0, "FLOOR");
//! world.add_box(Vec3::new(2.0, 0.0, -5.0), Vec3::new(3.0, 4.0, 5.0), "WALL");
//! world.rebuild_broad_phase();
//!
//! let config = CharacterConfig::default();
//! let mut state = CharacterState::at(Vec3::new(0.0, 2.0, 0.0));
//! let mut buffers = QueryBuffers::new();
//!
//! // A tick of falling.
//! state.velocity.y = -10.0;
//! let displacement = state.velocity / 60.0;
//! move_character(&world, &config, &mut state, displacement, &mut buffers);
//! ```

pub mod broad;
pub mod contact;
pub mod controller;
pub mod layer;
pub mod narrow;
pub mod ray;
pub mod shape;
pub mod world;

pub use broad::{BroadPhase, UniformGrid};
pub use contact::{Contact, QueryStats, StaticContact};
pub use controller::{
    CharacterConfig, CharacterState, MoveResult, move_character, resolve_overlaps,
};
pub use layer::{CollisionFilter, CollisionLayer, CollisionMask};
pub use ray::{Ray, RayHit, ray_vs_shape};
pub use shape::{Capsule, Obb, Segment, Shape};
pub use world::{CollisionWorld, QueryBuffers, StaticDesc, StaticId, StaticRayHit};

/// Everything a caller normally wants, in one `use`.
///
/// A prelude rather than a flat re-export at the crate root alone, because the names here
/// are generic enough — `Shape`, `Contact` — that a caller may reasonably want to import
/// them explicitly instead.
pub mod prelude {
    pub use crate::contact::{Contact, QueryStats, StaticContact};
    pub use crate::controller::{
        CharacterConfig, CharacterState, MoveResult, move_character, resolve_overlaps,
    };
    pub use crate::layer::{CollisionFilter, CollisionLayer, CollisionMask};
    pub use crate::ray::{Ray, RayHit};
    pub use crate::shape::{Capsule, Obb, Shape};
    pub use crate::world::{CollisionWorld, QueryBuffers, StaticDesc, StaticId, StaticRayHit};
}
