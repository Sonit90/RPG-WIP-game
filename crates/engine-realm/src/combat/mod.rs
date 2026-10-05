//! Server-side combat: the real simulation with the whole party in it, and the wire format
//! its events travel in.

pub mod instance;
pub mod wire;

pub use instance::{
    ActionError, ActionKind, CastState, CombatAction, CombatInstance, CooldownState,
    MonsterRecord, MonsterState, PlayerRecord, PlayerSpec, PlayerState, Resources, Restore,
    StatusState,
};
pub use wire::{AbilityRef, EntityRef, WireEvent};
