//! Remote authority: the simulation as a networked client runs it.
//!
//! ```text
//! offline:  commands ─► tick (AI, abilities, damage, statuses, respawn) ─► events
//! remote:   commands ─► targeting + own movement only ─► events
//!               └─ ability presses ─► take_remote_requests() ─► the server
//!           server rows ─► present_* (health, casts, cooldowns, statuses, brains)
//!           server events ─► inject_event ─► the same presentation observers
//! ```
//!
//! Online, the server runs the fight. The client keeps the same simulation for what only it
//! can do — predict its own movement, choose targets, hold the world presentation reads — and
//! nothing else: no brain thinks, no ability fires, no damage, healing, regeneration, death
//! or respawn happens locally. What the server decided arrives as rows and events and is laid
//! into the world here, so every widget, nameplate, effect and number reads it the way it
//! reads the offline game.

use engine_core::math::Vec3;

use crate::ability::AbilityId;
use crate::command::PlayerCommand;
use crate::enemy::EnemyAbility;
use crate::enemy::brain::{EnemyCast, MAX_TARGETS, MAX_ZONES};
use crate::entity::EntityId;
use crate::firemage::{AbilityState, FireAbility};
use crate::status::{Status, StatusKind};

use super::{ActiveCast, SimStats, Simulation};

/// One status as the server described it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentedStatus {
    /// Which.
    pub kind: StatusKind,
    /// Who applied it.
    pub source: EntityId,
    /// Seconds left now.
    pub remaining: f32,
    /// Seconds in total.
    pub duration: f32,
    /// Stacks.
    pub stacks: u8,
}

/// The local player's class state as the server described it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PresentedResources {
    /// Heat.
    pub heat: f32,
    /// Ultimate energy.
    pub energy: f32,
    /// Mana.
    pub mana: f32,
    /// Deflection.
    pub deflection: f32,
    /// Astral power.
    pub astral: f32,
    /// Global cooldown left now.
    pub global_cooldown: f32,
}

impl Simulation {
    /// Hands the fight to a remote authority (or takes it back). See the module docs.
    pub fn set_remote_authority(&mut self, remote: bool) {
        self.remote_authority = remote;
        if !remote {
            self.remote_requests.clear();
        }
    }

    /// Whether a remote authority runs the fight.
    #[must_use]
    pub fn is_remote_authority(&self) -> bool {
        self.remote_authority
    }

    /// Whether `command` is a request the remote authority decides.
    pub(super) fn is_remote_request(&self, command: &PlayerCommand) -> bool {
        self.remote_authority
            && matches!(
                command,
                PlayerCommand::UseAbility(_)
                    | PlayerCommand::UseAbilityAt { .. }
                    | PlayerCommand::UseUltimate { .. }
                    | PlayerCommand::UseExtra(_)
            )
    }

    /// The ability presses made since the last call, oldest first, for the server.
    pub fn take_remote_requests(&mut self) -> Vec<PlayerCommand> {
        std::mem::take(&mut self.remote_requests)
    }

    /// One tick under a remote authority: commands (targeting) and movement, nothing else.
    pub(super) fn tick_presented(&mut self, stats: &mut SimStats) {
        let step = self.timestep.step_seconds();
        self.world.begin_tick();
        self.apply_pending_commands();
        let movement_started = super::Stopwatch::start();
        self.integrate_bodies(step);
        stats.movement += movement_started.elapsed();
        // The presented cast keeps moving between rows, so a bar fills smoothly.
        if let Some(cast) = self.player.cast.as_mut() {
            cast.elapsed = (cast.elapsed + step).min(cast.duration);
        }
        self.flush_observations();
    }

    /// Shows `entity`'s health as the server has it.
    pub fn present_health(&mut self, entity: EntityId, current: f32, max: f32) {
        if let Some(health) = self.world.health_mut(entity) {
            if (health.max() - max).abs() > f32::EPSILON && max > 0.0 {
                health.set_max_keeping_current(max);
            }
            health.set_current(current);
        }
    }

    /// Shows the statuses on `entity` as the server has them.
    pub fn present_statuses(&mut self, entity: EntityId, statuses: &[PresentedStatus]) {
        let Some(set) = self.world.statuses_mut(entity) else {
            return;
        };
        set.clear();
        for presented in statuses {
            let mut status = Status::new(
                presented.kind,
                presented.source,
                AbilityId::NONE,
                presented.duration,
                0.0,
            );
            status.remaining = presented.remaining.max(0.0);
            status.stacks = presented.stacks.max(1);
            set.apply(status);
        }
    }

    /// Shows the local player's cast as the server has it.
    pub fn present_cast(&mut self, cast: Option<(FireAbility, f32, f32)>) {
        self.player.cast = cast.map(|(ability, elapsed, duration)| ActiveCast {
            ability,
            elapsed: elapsed.clamp(0.0, duration.max(0.0)),
            duration: duration.max(0.0),
            target: self.player.target,
            ground: None,
        });
    }

    /// Shows the local player's cooldowns and resources as the server has them.
    pub fn present_class_state(
        &mut self,
        cooldowns: &[(FireAbility, u8, f32)],
        resources: PresentedResources,
    ) {
        for (ability, charges, recharge) in cooldowns {
            self.mage.abilities.set_state(
                *ability,
                AbilityState {
                    charges: *charges,
                    recharge: *recharge,
                },
            );
        }
        self.mage.heat.set(resources.heat);
        self.mage.energy.set(resources.energy);
        self.druid.mana.set_current(resources.mana);
        self.warrior.deflection.set(resources.deflection);
        self.druid.astral.set(resources.astral);
        if resources.global_cooldown > 0.0 {
            self.mage.global_cooldown.start(resources.global_cooldown);
        } else {
            self.mage.global_cooldown.clear();
        }
    }

    /// Shows what an enemy is doing as the server has it.
    pub fn present_enemy(
        &mut self,
        entity: EntityId,
        target: Option<EntityId>,
        cast: Option<(EnemyAbility, f32, f32)>,
        interruptible: bool,
    ) {
        let alive = self.world.is_alive(entity);
        let anchor = self.world.position(entity).unwrap_or(Vec3::ZERO);
        let Some(Some(runtime)) = self.enemies.get_mut(entity.index()) else {
            return;
        };
        let cast = cast.map(|(ability, elapsed, duration)| EnemyCast {
            ability,
            elapsed,
            duration,
            target: target.unwrap_or(EntityId::INVALID),
            anchor,
            interruptible,
            shape: crate::enemy::TelegraphShape::Single,
            zone_count: 0,
            zones: [Vec3::ZERO; MAX_ZONES],
            aims: [Vec3::ZERO; MAX_ZONES],
            yaws: [0.0; MAX_ZONES],
            zone_targets: [None; MAX_ZONES],
            targets: [None; MAX_TARGETS],
            ticks: 0,
        });
        runtime.brain.present(target, cast, alive);
    }
}
