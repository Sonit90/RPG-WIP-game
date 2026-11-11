//! Effects from the registry, applied and removed by anything: the generic half of the
//! status system.
//!
//! An ability applies its own status with its own numbers — Wrath's stack of Nature's
//! Exposure, the Ghoul's bleed. What lives here is the path for an effect applied by name
//! with the registry's numbers ([`crate::status::GenericApplication`]): a test bed, a future
//! scripted encounter, a consumable. It goes through the same [`StatusSet`] and announces
//! itself with the same events, so nothing downstream can tell the two apart — the frames,
//! the plates, the meter, the dispel and the stat evaluation all see one status.
//!
//! [`StatusSet`]: crate::status::StatusSet

use super::Simulation;
use crate::ability::AbilityId;
use crate::character::StatBlock;
use crate::combat::DamageType;
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::status::{Status, StatusEnd, StatusKind};

/// What an entity's effects make of it right now, read from the same evaluations gameplay
/// multiplies by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectStats {
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Movement speed, metres per second, after every effect.
    pub move_speed: f32,
    /// What movement speed is multiplied by.
    pub move_speed_multiplier: f32,
    /// What attack speed is multiplied by. Below one is slower.
    pub attack_speed: f32,
    /// What the damage it deals is multiplied by.
    pub damage_dealt: f32,
    /// What a hit on it is multiplied by, per damage type, before flat reductions.
    pub damage_taken: [(DamageType, f32); 4],
    /// The flat share its effects take off every hit.
    pub damage_reduction: f32,
    /// Its character-sheet stats, if it has a sheet: the player does, a monster does not.
    pub sheet: Option<StatBlock>,
}

impl Simulation {
    /// Applies `kind` to `target` for `duration` seconds with `stacks` stacks, with the
    /// registry's numbers, and announces it. Returns whether it was applied.
    ///
    /// The ordinary application rules hold: an effect already there is refreshed to the new
    /// duration and gains the stacks, up to its ceiling; a dead target takes nothing; an
    /// effect the registry marks as not applicable from outside (respawn protection) is
    /// refused. `source` is credited with whatever it deals; [`EntityId::INVALID`] is "nobody".
    pub fn apply_effect(
        &mut self,
        target: EntityId,
        kind: StatusKind,
        duration: f32,
        stacks: u8,
        source: EntityId,
    ) -> bool {
        let definition = kind.definition();
        if !definition.applicable || !self.world.is_alive(target) {
            return false;
        }
        let duration = if duration.is_finite() && duration > 0.0 {
            duration
        } else {
            definition.generic.duration
        };
        let status = Status::new(
            kind,
            source,
            AbilityId::NONE,
            duration,
            definition.generic.per_tick,
        )
        .with_magnitude(definition.generic.magnitude)
        .with_stacks(stacks.max(1));
        self.apply_status(target, status, duration);
        self.sync_effect_stats();
        true
    }

    /// Takes `kind` off `target`, as removing it by hand does, and announces it. Returns
    /// whether it was there.
    pub fn remove_effect(&mut self, target: EntityId, kind: StatusKind) -> bool {
        let removed = self
            .world
            .statuses_mut(target)
            .and_then(|statuses| statuses.consume(kind))
            .is_some();
        if removed {
            let tick = self.tick;
            self.events.push(CombatEvent::StatusRemoved {
                target,
                status: kind,
                reason: StatusEnd::Cleared,
                tick,
            });
            self.sync_effect_stats();
        }
        removed
    }

    /// Takes every effect off `target` that `remove` picks, announcing each. Effects the
    /// registry does not let anything else apply (respawn protection) are never picked.
    /// Returns how many went.
    pub fn remove_effects_where(
        &mut self,
        target: EntityId,
        mut remove: impl FnMut(StatusKind) -> bool,
    ) -> usize {
        let kinds: Vec<StatusKind> = self
            .world
            .statuses(target)
            .map(|statuses| {
                statuses
                    .iter()
                    .map(|status| status.kind)
                    .filter(|kind| kind.definition().applicable && remove(*kind))
                    .collect()
            })
            .unwrap_or_default();
        for kind in &kinds {
            self.remove_effect(target, *kind);
        }
        kinds.len()
    }

    /// Recomputes what depends on the player's effect stats when they have changed: their
    /// maximum health and their class's derived numbers.
    ///
    /// Run once a tick, after statuses have ticked and expired, and straight after an effect
    /// is applied or removed. Compares a handful of numbers on the ticks nothing changed.
    pub(super) fn sync_effect_stats(&mut self) {
        let bonus = self
            .world
            .statuses(self.player.entity)
            .map(crate::status::StatusSet::stat_bonus)
            .unwrap_or_default();
        if bonus != self.effect_stat_bonus {
            self.effect_stat_bonus = bonus;
            self.refresh_character();
        }
    }

    /// What `entity`'s effects make of it: health, speed, attack speed, damage dealt and
    /// taken, and, for the player, the character sheet.
    #[must_use]
    pub fn effect_stats(&self, entity: EntityId) -> Option<EffectStats> {
        let health = self.world.health(entity)?;
        let statuses = self.world.statuses(entity)?;
        let player = entity == self.player.entity;
        let move_speed_multiplier = statuses.move_speed_multiplier();
        let base_speed = match self.brain(entity) {
            Some(brain) => brain.speed(),
            None => self.config.movement.max_speed,
        };
        let attack_speed = match self.brain(entity) {
            Some(brain) => brain.attack_speed_scale(),
            None => statuses.attack_speed_scale(),
        };
        let damage_reduction = if player {
            self.defense_profile().damage_reduction
        } else {
            statuses.damage_reduction()
        };
        let (dealt_correction, taken_correction) = self.fervor_corrections(entity);
        Some(EffectStats {
            health: health.current(),
            max_health: health.max(),
            move_speed: base_speed * move_speed_multiplier,
            move_speed_multiplier,
            attack_speed,
            damage_dealt: statuses.damage_dealt_multiplier() * dealt_correction,
            damage_taken: [
                DamageType::Physical,
                DamageType::Fire,
                DamageType::Arcane,
                DamageType::Nature,
            ]
            .map(|damage_type| {
                (
                    damage_type,
                    statuses.damage_taken_multiplier(damage_type) * taken_correction,
                )
            }),
            damage_reduction,
            sheet: player.then(|| self.character_stats()),
        })
    }
}
