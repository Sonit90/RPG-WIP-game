//! Using a trinket: the button, its refusals and what each effect does.
//!
//! ```text
//! UseTrinket(n) ─► use_trinket ─► alive? stunned? worn? cooling? ─► TrinketUsed
//!                                      │
//!        ┌─────────────────┬───────────┴──────┬─────────────────────┐
//!   TrinketWard status   dispel (allies,   AbilityBook::reduce_   absorb pool +
//!   on each ally         enemy buffs)      remaining_fraction     TrinketAbsorb status
//! ```
//!
//! Every effect goes through the systems that already exist: a status for the ward, the
//! status table's own dispel classification for Mass Dispel, the ability book for the cooldown
//! cut and the player's absorb pool for the shield. The party is [`Simulation::party`], so on a
//! server it is every player of the instance and offline it is the one player.
//!
//! A trinket is **off the global cooldown**: it neither waits for it nor starts it.

use super::Simulation;
use crate::ability::AbilityId;
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::firemage::AbilityError;
use crate::status::{Status, StatusEnd, StatusKind};
use crate::trinket::{EQUIP_COOLDOWN, TRINKET_SLOTS, TrinketActive, TrinketEffect, slot_of};

impl Simulation {
    /// The active of the trinket worn in button `index`, if there is one.
    #[must_use]
    pub fn trinket_active(&self, index: usize) -> Option<TrinketActive> {
        let slot = slot_of(index)?;
        self.sheet().equipment().get(slot)?.def()?.active
    }

    /// Seconds left on button `index`'s cooldown.
    #[must_use]
    pub fn trinket_remaining(&self, index: usize) -> f32 {
        self.trinket.remaining(index)
    }

    /// Whether the player is stunned. Nothing can stun a player yet; the day something does,
    /// it answers here and a trinket is refused.
    fn player_stunned(&self) -> bool {
        false
    }

    /// Uses the trinket in button `index` for the active player.
    ///
    /// # Errors
    ///
    /// [`AbilityError`] with the reason: dead, stunned, nothing to use, or cooling down. The
    /// refusal is also kept on the player for the interface, like a refused ability.
    pub fn use_trinket(&mut self, index: usize) -> Result<(), AbilityError> {
        let result = self.try_use_trinket(index);
        match result {
            Ok(()) => self.player.last_error = None,
            Err(error) => {
                self.player.last_error = Some(error);
                self.player.last_error_at = self.time;
            }
        }
        result
    }

    fn try_use_trinket(&mut self, index: usize) -> Result<(), AbilityError> {
        if !self.world.is_alive(self.player.entity) {
            return Err(AbilityError::CasterDead);
        }
        if self.player_stunned() {
            return Err(AbilityError::Stunned);
        }
        let active = self
            .trinket_active(index)
            .filter(TrinketActive::is_valid)
            .ok_or(AbilityError::NoTrinketActive)?;
        if self.trinket.remaining(index) > 0.0 {
            return Err(AbilityError::TrinketNotReady);
        }
        self.trinket.start(index, active.cooldown);
        let user = self.player.entity;
        let tick = self.tick;
        self.events.push(CombatEvent::TrinketUsed {
            entity: user,
            slot: u8::try_from(index).unwrap_or(u8::MAX),
            effect: active.effect,
            tick,
        });
        let allies = self.trinket_allies(active.radius);
        match active.effect {
            TrinketEffect::GroupDamageReduction => self.trinket_ward(&allies, active),
            TrinketEffect::MassDispel => self.trinket_mass_dispel(&allies, active),
            TrinketEffect::GroupCooldownReduction => self.trinket_cooldown_cut(&allies, active),
            TrinketEffect::GroupAbsorb => self.trinket_shield(&allies, active),
        }
        self.sync_effect_stats();
        Ok(())
    }

    /// The living party members within `radius` of the user, the user included.
    fn trinket_allies(&self, radius: f32) -> Vec<EntityId> {
        let user = self.player.entity;
        let mut allies: Vec<EntityId> = self
            .party
            .members()
            .iter()
            .copied()
            .filter(|member| {
                self.world.is_alive(*member)
                    && (*member == user
                        || self
                            .world
                            .distance_between(user, *member)
                            .is_some_and(|distance| distance <= radius))
            })
            .collect();
        if !allies.contains(&user) && self.world.is_alive(user) {
            allies.insert(0, user);
        }
        allies
    }

    fn trinket_ward(&mut self, allies: &[EntityId], active: TrinketActive) {
        let user = self.player.entity;
        for &ally in allies {
            let status = Status::new(
                StatusKind::TrinketWard,
                user,
                AbilityId::NONE,
                active.duration,
                0.0,
            )
            .with_magnitude(active.magnitude);
            self.apply_status(ally, status, active.duration);
        }
    }

    fn trinket_mass_dispel(&mut self, allies: &[EntityId], active: TrinketActive) {
        let user = self.player.entity;
        for &ally in allies {
            self.strip_statuses(ally, StatusKind::is_dispellable);
        }
        let Some(origin) = self.world.position(user) else {
            return;
        };
        let mut enemies = Vec::new();
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if !self.world.is_alive(id) || !self.is_attackable(id) {
                continue;
            }
            let near = self
                .world
                .position(id)
                .is_some_and(|at| (at - origin).length() <= active.radius);
            if near {
                enemies.push(id);
            }
        }
        for enemy in enemies {
            self.strip_statuses(enemy, StatusKind::is_purgeable);
        }
    }

    /// Takes every status of `target` that `picks` accepts, announcing each as a dispel.
    fn strip_statuses(&mut self, target: EntityId, picks: impl Fn(StatusKind) -> bool) {
        let source = self.player.entity;
        let picked: Vec<StatusKind> = self
            .world
            .statuses(target)
            .map(|statuses| {
                statuses
                    .iter()
                    .map(|status| status.kind)
                    .filter(|kind| picks(*kind))
                    .collect()
            })
            .unwrap_or_default();
        let tick = self.tick;
        for kind in picked {
            let Some(school) = kind.dispel_school() else {
                continue;
            };
            if let Some(statuses) = self.world.statuses_mut(target) {
                statuses.consume(kind);
            }
            self.events.push(CombatEvent::Dispelled {
                source,
                target,
                status: kind,
                school,
                ability: AbilityId::NONE,
                tick,
            });
            self.events.push(CombatEvent::StatusRemoved {
                target,
                status: kind,
                reason: StatusEnd::Consumed,
                tick,
            });
        }
    }

    fn trinket_cooldown_cut(&mut self, allies: &[EntityId], active: TrinketActive) {
        for &ally in allies {
            let _ = self.with_player(ally, |sim| {
                // The hero's own abilities only: not the global cooldown, which is a clock
                // beside the book, and not the trinkets, which are not in it.
                for &ability in sim.hero.abilities() {
                    sim.mage
                        .abilities
                        .reduce_remaining_fraction(ability, active.magnitude);
                }
            });
        }
    }

    fn trinket_shield(&mut self, allies: &[EntityId], active: TrinketActive) {
        let user = self.player.entity;
        let tick = self.tick;
        for &ally in allies {
            let Some(max) = self.world.health(ally).map(crate::combat::Health::max) else {
                continue;
            };
            let amount = max * active.magnitude;
            let granted = self
                .with_player(ally, |sim| {
                    sim.trinket.absorb.grant(amount, active.duration)
                })
                .is_some();
            if !granted {
                continue;
            }
            let status = Status::new(
                StatusKind::TrinketAbsorb,
                user,
                AbilityId::NONE,
                active.duration,
                0.0,
            )
            .with_magnitude(active.magnitude);
            self.apply_status(ally, status, active.duration);
            self.events.push(CombatEvent::AbsorbApplied {
                target: ally,
                amount,
                duration: active.duration,
                ability: AbilityId::NONE,
                tick,
            });
        }
    }

    /// The shield was spent: the aura that showed it goes too.
    pub(super) fn end_trinket_absorb_status(&mut self, entity: EntityId) {
        let removed = self
            .world
            .statuses_mut(entity)
            .and_then(|statuses| statuses.consume(StatusKind::TrinketAbsorb))
            .is_some();
        if removed {
            let tick = self.tick;
            self.events.push(CombatEvent::StatusRemoved {
                target: entity,
                status: StatusKind::TrinketAbsorb,
                reason: StatusEnd::Consumed,
                tick,
            });
        }
    }

    /// One tick of the active player's trinkets: a freshly worn trinket starts a short
    /// cooldown, the cooldowns run down, and a shield whose aura is gone (expired, or cleared
    /// with the player's statuses) goes with it.
    pub(super) fn tick_trinket(&mut self, step: f32) {
        let mut worn = [crate::character::ItemId::NONE; TRINKET_SLOTS];
        for (index, id) in worn.iter_mut().enumerate() {
            if let Some(stack) = slot_of(index).and_then(|slot| self.sheet().equipment().get(slot))
            {
                *id = stack.item;
            }
        }
        let changed = self.trinket.note_worn(worn);
        for (index, changed) in changed.into_iter().enumerate() {
            if changed && let Some(active) = self.trinket_active(index) {
                self.trinket
                    .start(index, active.cooldown.min(EQUIP_COOLDOWN));
            }
        }
        self.trinket.tick(step);
        if self.trinket.absorb.is_active()
            && !self
                .world
                .statuses(self.player.entity)
                .is_some_and(|statuses| statuses.has(StatusKind::TrinketAbsorb))
        {
            self.trinket.absorb.clear();
        }
    }
}
