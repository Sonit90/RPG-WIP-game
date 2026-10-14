//! The rotation AIs: what a competent player of each class would press, and when.
//!
//! ```text
//! Rotation (trait)
//!   |-- firemage::RotationAi   Haste / Crit styles, the Detonate decision
//!   |-- warrior::WarriorRotation   damage abilities on cooldown, Recuperate as the filler
//!   `-- druid::DruidRotation       Moonfire/Sunfire upkeep, Wrath filler, Astral Power
//! ```
//!
//! Every rotation is a priority list over *ability presses and cooldown readiness* and
//! nothing else. None of them computes a point of damage, restates a cooldown or reads a
//! class resource by a name that might be reworked: they ask the game whether an ability is
//! ready and press it, and the game decides what that does. That is what keeps a rework of a
//! class's resource from breaking the simulator.
//!
//! A rotation takes **at most one action per tick**, and none while a cast is in progress or
//! the global cooldown is running, exactly as a player's hands would.

use engine_core::math::Vec3;
use engine_game::entity::EntityId;
use engine_game::firemage::FireAbility;
use engine_game::movement::MoveIntent;
use engine_game::sim::Simulation;

pub mod druid;
pub mod firemage;
pub mod warrior;

pub use druid::DruidRotation;
#[allow(unused_imports, reason = "re-exported for the tests and the report")]
pub use firemage::{DetonateRules, RotationAi, Style};
pub use warrior::WarriorRotation;

/// A player: something that looks at the fight each tick and presses a button.
pub trait Rotation {
    /// Takes at most one action this tick against `pack`.
    fn act(&mut self, simulation: &mut Simulation, pack: &[EntityId]);

    /// The abilities with a cooldown that the rotation is meant to use on cooldown.
    ///
    /// What the cooldown-usage section of the report measures: casts against the number the
    /// cooldown would have allowed, and the seconds a charge sat ready.
    fn tracked_cooldowns(&self) -> &'static [FireAbility];

    /// A short name, for the report.
    fn name(&self) -> &'static str;

    /// The priority list in words, in order, for the report.
    ///
    /// Written beside the code that implements it and printed verbatim, so the document a
    /// designer reads and the rotation the numbers came from cannot drift apart.
    fn priority(&self) -> &'static [&'static str];
}

/// Whether the player's hands are free: no cast bar, no global cooldown.
#[must_use]
pub fn ready_to_act(simulation: &Simulation) -> bool {
    simulation.casting().is_none() && simulation.mage().off_global_cooldown()
}

/// The living members of `pack`.
#[must_use]
pub fn living(simulation: &Simulation, pack: &[EntityId]) -> Vec<EntityId> {
    pack.iter()
        .copied()
        .filter(|id| simulation.world().is_alive(*id))
        .collect()
}

/// Whether `ability` has a charge to spend. Cooldown readiness, nothing more.
#[must_use]
pub fn is_ready(simulation: &Simulation, ability: FireAbility) -> bool {
    simulation.mage().abilities.is_ready(ability)
}

/// Selects `target` and turns to face it.
///
/// Facing is a real requirement for a targeted cast, and the AI is not testing the player's
/// mouse hand.
pub fn select_and_face(simulation: &mut Simulation, target: EntityId) {
    simulation.player_mut().target = Some(target);
    if let Some(position) = simulation.world().position(target) {
        simulation.face(position);
    }
}

/// Horizontal distance from the player to `target`.
#[must_use]
pub fn distance_to(simulation: &Simulation, target: EntityId) -> f32 {
    let (Some(me), Some(them)) = (
        simulation.world().position(simulation.player().entity),
        simulation.world().position(target),
    ) else {
        return 0.0;
    };
    engine_game::sim::horizontal_distance(me, them)
}

/// Runs one step towards `target` through the game's own movement path.
///
/// The melee class has to be where its abilities reach; walking there costs the global
/// cooldown nothing but it does cost the time, which is the honest price of a target that
/// wandered off.
pub fn close_in(simulation: &mut Simulation, target: EntityId) {
    let (Some(me), Some(them)) = (
        simulation.world().position(simulation.player().entity),
        simulation.world().position(target),
    ) else {
        return;
    };
    let (dx, dz) = (them.x - me.x, them.z - me.z);
    let length = (dx * dx + dz * dz).sqrt().max(1e-4);
    let direction = Vec3::new(dx / length, 0.0, dz / length);
    let entity = simulation.player().entity;
    simulation.face(them);
    simulation.drive_player(entity, MoveIntent::moving(direction));
}

/// The middle of the living pack: where an area ability is aimed.
#[must_use]
pub fn centroid(simulation: &Simulation, living: &[EntityId]) -> Vec3 {
    if living.is_empty() {
        return Vec3::ZERO;
    }
    let mut sum = Vec3::ZERO;
    let mut count = 0.0;
    for id in living {
        if let Some(position) = simulation.world().position(*id) {
            sum = Vec3::new(sum.x + position.x, sum.y, sum.z + position.z);
            count += 1.0;
        }
    }
    if count == 0.0 {
        return Vec3::ZERO;
    }
    Vec3::new(sum.x / count, 0.0, sum.z / count)
}
