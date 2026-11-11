//! Effects as data, and the runtime that plays them.
//!
//! ```text
//!            gameplay events                (engine-game, elsewhere)
//!                   │
//!                   ▼
//!            a director decides             (engine-presentation, elsewhere)
//!                   │  VfxSpawn
//!                   ▼
//!   ┌─────────────────────────────────┐
//!   │  VfxSystem                      │   this crate
//!   │    instances ── lifecycle       │
//!   │    emitters  ── ParticlePool    │
//!   │    layers    ── MeshDraw list   │
//!   └─────────────────────────────────┘
//!         │                   │
//!         ▼                   ▼
//!  ParticleDrawList      MeshDraw            (drawn by engine-renderer, elsewhere)
//! ```
//!
//! # Why this is its own crate
//!
//! It depends on `engine-core` and `engine-render-data`, and on nothing else. That absence
//! is the design:
//!
//! * **No `engine-game`.** An effect physically cannot reach into the simulation that
//!   announced it, ask a target for its health, or consume the gameplay random source. VFX
//!   observe; they never decide.
//! * **No `engine-renderer` and no `windows`.** The whole runtime — spawning, ageing,
//!   emitting, pooling, expiring, and the particle integration itself — is unit tested with
//!   no device, no window and no adapter, which is what makes the tests in this crate
//!   assertions about behaviour rather than about a screenshot.
//! * **No `engine-assets`.** A layer names a *shape*, not a mesh. Binding shapes to
//!   uploaded geometry is presentation's job, exactly as it is for the environment.
//!
//! ```text
//! cargo tree -p engine-vfx
//! engine-vfx
//! ├── engine-core
//! ├── engine-render-data
//! └── log
//! ```
//!
//! # What is here
//!
//! | Module | Owns |
//! | --- | --- |
//! | [`asset`] | The catalogue: what each effect is made of, as a static table |
//! | [`system`] | Instances, their lifecycle, and this frame's output |
//! | [`particle`] | The fixed-capacity CPU particle simulation |
//! | [`rng`] | The random source a scatter is drawn from — not gameplay's |
//!
//! # Gameplay is authoritative
//!
//! Nothing in this crate can change a hit, a position or a number. An effect is handed the
//! radius an ability hits with and draws that radius; if the whole system were deleted, the
//! same fight would resolve identically with nothing to look at. Every cap in here —
//! instances, particles — degrades by *dropping visuals* and counting the drop.

pub mod asset;
pub mod particle;
pub mod rng;
pub mod system;

pub use asset::{
    EFFECT_COUNT, EFFECTS, EmitterDesc, LayerAlign, MAX_EMITTERS, MAX_LAYERS, MeshLayer,
    SpawnShape, VfxAsset, VfxId, VfxShape, Window,
};
pub use particle::{NO_OWNER, ParticlePool, ParticleSpawn};
pub use rng::Rng;
pub use system::{
    DEFAULT_MAX_INSTANCES, DEFAULT_MAX_PARTICLES, MeshDraw, VfxHandle, VfxSpawn, VfxStats,
    VfxSystem,
};
