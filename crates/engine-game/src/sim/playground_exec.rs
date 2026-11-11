//! The development controls a test bed drives: put the player back, take a chosen set of
//! enemies out of the world, and find ground to stand a monster on.
//!
//! Each is the simulation's own path with nothing bypassed. A dismissed enemy goes through
//! [`Simulation::despawn_enemy`] — the same call that takes a boss's adds away — so its cast,
//! its telegraphs, its hazards and its marks come down exactly as they do in the dungeon. What
//! is added here is only what a *despawn* (rather than a death) leaves behind: statuses its
//! author no longer exists to own, and a player cast aimed at something that is gone.

use engine_core::math::Vec3;

use super::Simulation;
use crate::entity::EntityId;

impl Simulation {
    /// Puts the player back to how a fresh session starts them, where they stand.
    ///
    /// Full health, a full resource and every cooldown ready; no buffs, no debuffs, no cast,
    /// no queued cast, no death owed. A dead player is brought back first — through the
    /// ordinary revive, which also stands them at their spawn. The health override, if a test
    /// scene set one, is kept: it is the scene's choice of maximum, not part of the state
    /// being reset.
    ///
    /// Enemies are not touched. A monster that was fighting the player is still fighting
    /// them; resetting the fight is [`Simulation::dismiss_enemies`]'s job, or
    /// [`Simulation::reset_encounter`]'s.
    pub fn reset_player_state(&mut self) {
        let player = self.player.entity;
        if !self.world.contains(player) {
            return;
        }
        if !self.player_alive() {
            self.revive_player();
        }
        self.respawn.clear();
        // Cooldowns, charges, Heat, Ultimate Energy, Ignite, zones, the Warrior's pools and
        // the Druid's Mana and Astral Power: the same three resets the dummy reset runs.
        self.mage.reset();
        self.warrior.reset();
        self.druid.reset();
        self.fire = super::FireCounters::default();
        if self.player.cast.is_some() {
            self.cancel_cast();
        }
        self.player.cast = None;
        self.player.queued = None;
        self.player.last_error = None;
        if let Some(statuses) = self.world.statuses_mut(player) {
            statuses.clear();
        }
        // After the Warrior reset, which may have taken a health multiplier away with Last
        // Stand, and after the statuses, whose stats went with them: the maximum is
        // recomputed before the player is filled to it.
        self.sync_effect_stats();
        self.refresh_character();
        if let Some(health) = self.world.health(player) {
            let missing = health.max() - health.current();
            if missing > 0.0 {
                self.world.heal(player, missing);
            }
        }
    }

    /// Takes every enemy in `enemies` out of the world, with everything it left behind.
    ///
    /// Alive, dead, casting, stunned or mid-charge makes no difference: each goes through
    /// [`Simulation::despawn_enemy`], which ends its cast with an event, takes its
    /// telegraphs, hazards and marks down and drops the player's target if it was the one.
    /// Then whatever it had applied to anybody else — a bleed on the player, a curse on the
    /// ground's victims — is removed, and a player cast still aimed at one of them is
    /// cancelled rather than left to land on nothing.
    ///
    /// Ids that are stale, or that name something other than an enemy, are skipped. Returns
    /// how many enemies were taken away.
    ///
    /// Meant for enemies that belong to no encounter — a test bed's own spawns. An
    /// encounter's members are put back with [`Simulation::reset_encounter`] instead.
    pub fn dismiss_enemies(&mut self, enemies: &[EntityId]) -> usize {
        let mut dismissed = 0;
        for &enemy in enemies {
            if self.brain(enemy).is_none() {
                continue;
            }
            self.despawn_enemy(enemy);
            dismissed += 1;
        }
        if dismissed == 0 {
            return 0;
        }
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if let Some(statuses) = self.world.statuses_mut(id) {
                for &enemy in enemies {
                    statuses.remove_from(enemy);
                }
            }
        }
        let aimed_at_one = self
            .player
            .cast
            .as_ref()
            .and_then(|cast| cast.target)
            .is_some_and(|target| enemies.contains(&target));
        if aimed_at_one {
            self.cancel_cast();
        }
        if self
            .player
            .target
            .is_some_and(|target| enemies.contains(&target))
        {
            self.player.target = None;
        }
        dismissed
    }

    /// Starts a new combat session on the meter: nothing dealt, nothing taken, from now.
    pub fn reset_combat_meter(&mut self) {
        self.meter.reset(self.time);
    }

    /// The walkable ground under `position`, if a body can stand there.
    ///
    /// Looked for from about `position`'s own height, so a spot on a balcony finds the
    /// balcony rather than the floor below it. `None` where there is no ground, where the
    /// ground is too steep, or where a body standing on it would be inside something — the
    /// same judgement monster navigation makes of every cell it walks.
    pub fn standing_ground(&mut self, position: Vec3) -> Option<Vec3> {
        let (ground, walkable) = self.nav.ground_at(&self.collision, position)?;
        walkable.then_some(Vec3::new(position.x, ground, position.z))
    }

    /// Whether a body could walk the straight line from `from` to `to`.
    ///
    /// The navigator's own test: what stops a test bed standing a monster on the far side
    /// of a wall from the player who asked for it.
    pub fn walkable_between(&mut self, from: Vec3, to: Vec3) -> bool {
        self.nav.walkable_line(&self.collision, from, to)
    }
}
