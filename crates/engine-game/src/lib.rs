//! The gameplay simulation.
//!
//! ```text
//! Input -> Commands -> Simulation -> (state + events) -> extraction -> RenderWorld
//!                      ^^^^^^^^^^
//!                      this crate
//! ```
//!
//! ## What makes this crate the seam
//!
//! It depends on `engine-core` and nothing else. Not `windows`, not `engine-platform`, not
//! `engine-renderer`, and deliberately not even `engine-render-data`: the simulation cannot
//! name a `MeshHandle`, so it cannot accidentally start owning one. Gameplay therefore runs
//! with no device, no window and no GPU, which is what
//! `crates/engine-game/tests/headless_simulation.rs` asserts and what makes a determinism
//! test and a future dedicated server possible.
//!
//! ```text
//! cargo tree -p engine-game
//! engine-game
//! ├── engine-core
//! │   └── log
//! └── log
//! ```
//!
//! ## The shape of a tick
//!
//! ```text
//! commands -> movement -> cooldowns -> abilities -> statuses -> deaths -> telemetry
//!                                          |            |         |
//!                                          +------> CombatEvent <-+
//! ```
//!
//! Time arrives as a real frame delta and is spent in whole fixed 60 Hz ticks
//! ([`engine_core::time::FixedTimestep`]), so gameplay behaviour does not change with the
//! frame rate. Everything a system does to the world is announced as a
//! [`event::CombatEvent`], so telemetry — and later VFX, a combat log, or network
//! replication — observes rather than hooks.
//!
//! ## Module map
//!
//! | Module | Owns |
//! | --- | --- |
//! | [`entity`] | Entity identity and the slot allocator |
//! | [`world`] | Every entity's state, in parallel arrays |
//! | [`movement`] | The character controller |
//! | [`combat`] | Health, resource, factions, damage types |
//! | [`status`] | Lasting effects such as burning |
//! | [`ability`] | Ability definitions, slots and cooldowns |
//! | [`targeting`] | Who an ability affects |
//! | [`los`] | Whether a targeted ability can reach its target through the level |
//! | [`event`] | What happened, as data |
//! | [`telemetry`] | The combat meter |
//! | [`command`] | What the player asked for |
//! | [`input_map`] | Keys to commands — the only module that knows a key |
//! | [`character`] | Stats, items, the fifteen equipment slots and the bag |
//! | [`arena`] | The training arena |
//! | [`demo`] | The collision demo scene: geometry, no combat |
//! | [`enemy`] | Enemy archetypes, their attacks, their threat and their state machine |
//! | [`encounter`] | A fight: who is in it, whether it is running, how to reset it |
//! | [`sim`] | The tick, and everything that owns state across one |

/// Seconds below which a countdown is treated as finished.
///
/// A fixed step of 1/60 s is not exactly representable in `f32`, so subtracting it sixty
/// times from 1.0 leaves a residue of a few millionths rather than zero. Without a
/// tolerance a six-second burn ticks five times instead of six and an ability whose
/// cooldown has "expired" is still refused — both of which look like logic bugs and are
/// arithmetic. A tenth of a millisecond is far below anything gameplay can express and far
/// above the accumulated error.
pub const TIME_EPSILON: f32 = 1e-4;

pub mod ability;
pub mod arena;
/// The character sheet: stats, items, equipment, the bag and Spirit.
pub mod character;
pub mod combat;
pub mod command;
pub mod demo;
/// The first dungeon: which pack stands where, and how to put the whole thing back.
pub mod dungeon;
/// A fight as an object: members, lifecycle and reset.
pub mod encounter;
/// What an enemy is, what it wants, and who it is angry at.
pub mod enemy;
pub mod entity;
pub mod event;
pub mod firemage;
/// Which playable character the player is, and which abilities that gives them.
pub mod hero;
pub mod input_map;
/// Line of sight for targeted abilities, and nothing else.
pub mod los;
pub mod los_course;
pub mod movement;
/// Who the player is grouped with: the roster the party frames read.
pub mod party;
pub mod sim;
pub mod status;
pub mod targeting;
pub mod telemetry;
pub mod world;

pub use ability::{ABILITY_SLOTS, AbilityId};
pub use arena::{Arena, ArenaConfig, training_arena};
pub use character::{
    CharacterSheet, EquipSlot, Equipment, Inventory, ItemDef, ItemFamily, ItemId, ItemQuality,
    ItemSlot, ItemStack, MoveError, Roster, Stat, StatBlock, StatGroup,
};
pub use combat::{DamageType, Faction, Health, Resource};
pub use command::{CommandBuffer, PlayerCommand};
pub use demo::{DemoScene, Station, collision_demo};
pub use encounter::{Encounter, EncounterState, EnemySpawn};
pub use enemy::brain::{AiState, EnemyBrain};
pub use enemy::{EnemyAbility, EnemyKind, TelegraphShape};
pub use entity::EntityId;
pub use event::{CombatEvent, DamageEvent};
pub use firemage::{AbilityError, FireAbility, FireMage, TalentId};
pub use hero::Hero;
pub use input_map::Bindings;
pub use los::{CombatPoint, LineOfSightCheck, has_line_of_sight, line_of_sight};
pub use movement::{Body, MovementState};
pub use party::{MAX_PARTY, Party};
pub use sim::{Player, SimConfig, SimStats, Simulation};
pub use status::{StatusKind, StatusSet};
pub use telemetry::CombatMeter;
pub use world::{EntityDesc, EntityKind, GameWorld};
