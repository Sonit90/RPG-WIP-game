//! The simulation's half of the dungeon modifiers: asking the world who is standing where,
//! dealing the damage and announcing what happened.
//!
//! ```text
//! tick_modifiers ─┬─ encounters engaged / ended ──▶ BeamField  ──▶ ModifierBeams{Started,Ended}
//!                 ├─ shared 0.5 s clock ──────────▶ players in a live beam ──▶ deal_damage
//!                 └─ PlagueField::advance ────────▶ burst ──▶ players inside ──▶ deal_damage
//!                                                   cloud ticks ──▶ deal_damage + Plagued
//! modifier_on_death ──▶ PlagueArmed (the one hook inside the damage path)
//! ```
//!
//! The pure state and geometry are in [`crate::modifier`]; this file is the glue, in the
//! pattern `enemy::hazard` / `enemy_exec` already follow. A simulation with no modifier in
//! force returns at the first line of every entry point here, so an ordinary dungeon pays
//! nothing for them.
//!
//! # Where the damage comes from
//!
//! Every hit is `deal_damage` with no source entity and no ability (the environment, like a
//! fall), flagged to skip the incoming multipliers: a modifier says "5 % of your health" and
//! means exactly that, not whatever versatility and armour make of it. The number is capped
//! (`max_hit_pct`) so no magnitude can turn a tick into a one-shot.

use engine_core::math::Vec3;

use crate::ability::AbilityId;
use crate::combat::DamageType;
use crate::enemy::brain::AiState;
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::modifier::{
    ActiveModifier, BeamEmitter, BeamParams, EndedModifier, MODIFIER_TICK_INTERVAL,
    ModifierRuntime, PlagueCloud, PlagueFuse, PlagueParams,
};
use crate::status::{Status, StatusKind};
use crate::world::EntityKind;

use super::Simulation;

impl Simulation {
    /// Puts `modifiers` in force for this run, replacing any, and clears the ground the old
    /// ones left. Set once when an instance is built.
    pub fn set_modifiers(&mut self, modifiers: Vec<ActiveModifier>) {
        self.clear_modifier_ground();
        self.modifiers.set_active(modifiers);
    }

    /// The modifiers in force and everything they have put on the ground. For presentation,
    /// which draws the beams and the plague from it, and for tests.
    #[must_use]
    pub fn modifiers(&self) -> &ModifierRuntime {
        &self.modifiers
    }

    /// How much of the dungeon's placed monsters have been killed: `(killed, total)`.
    ///
    /// Counts the monsters the level placed — normal and elite — and **not** the boss and
    /// not anything the boss called (a summoned add is not part of the level). What a reroll
    /// that unlocks at "half the dungeon cleared" reads. `(0, 0)` for a simulation with no
    /// encounters.
    #[must_use]
    pub fn dungeon_kill_progress(&self) -> (u32, u32) {
        let (mut killed, mut total) = (0, 0);
        for encounter in &self.encounters {
            let placed = encounter.len().saturating_sub(encounter.summon_count());
            for member in &encounter.members()[..placed] {
                if self.is_boss_entity(*member) {
                    continue;
                }
                total += 1;
                if !self.world.is_alive(*member) {
                    killed += 1;
                }
            }
        }
        (killed, total)
    }

    /// Slays living placed monsters (never the boss, never summons) until `fraction` of them
    /// are dead, and returns how many it slew. An operator and test tool: the realm's
    /// `admin_clear_dungeon` uses it to reach the reroll's unlock without playing half a dungeon.
    pub fn slay_placed_until(&mut self, fraction: f32) -> u32 {
        let (killed, total) = self.dungeon_kill_progress();
        let wanted = (total as f32 * fraction.clamp(0.0, 1.0)).ceil() as u32;
        let mut to_slay = Vec::new();
        for encounter in &self.encounters {
            let placed = encounter.len().saturating_sub(encounter.summon_count());
            for member in &encounter.members()[..placed] {
                if !self.is_boss_entity(*member) && self.world.is_alive(*member) {
                    to_slay.push(*member);
                }
            }
        }
        let count = wanted.saturating_sub(killed) as usize;
        let slain = count.min(to_slay.len());
        for entity in to_slay.into_iter().take(slain) {
            self.slay(entity);
        }
        slain as u32
    }

    /// Whether `entity` is the dungeon's boss, or any boss-tier monster.
    fn is_boss_entity(&self, entity: EntityId) -> bool {
        // The enemy table is out during the AI's own step, so the dungeon's rules answer
        // too: they name the boss encounter, whose first member is the boss.
        if let Some(Some(runtime)) = self.enemies.get(entity.index())
            && runtime.brain.kind().is_boss()
        {
            return true;
        }
        self.dungeon
            .as_ref()
            .and_then(|run| run.rules().boss_encounter)
            .and_then(|index| self.encounters.get(index))
            .and_then(|encounter| encounter.members().first())
            .is_some_and(|boss| *boss == entity)
    }

    /// One tick of every modifier. Run once a tick from the simulation's step, after the AI
    /// has decided who is fighting.
    pub(super) fn tick_modifiers(&mut self, step: f32) {
        if self.modifiers.is_empty() {
            return;
        }
        // A finished dungeon has nothing left to fight: put the ground out, once.
        if self.dungeon.as_ref().is_some_and(|run| run.is_completed()) {
            if !self.modifiers.beams().is_empty()
                || !self.modifiers.fuses().is_empty()
                || !self.modifiers.clouds().is_empty()
            {
                self.clear_modifier_ground();
            }
            return;
        }
        if let Some((params, magnitude)) = self.modifiers.beam_rules() {
            self.sync_beam_emitters(&params);
            self.tick_beam_damage(step, &params, magnitude);
        }
        if let Some((params, magnitude)) = self.modifiers.plague_rules() {
            self.tick_plague(step, &params, magnitude);
        }
    }

    /// Puts every modifier's ground out and announces it. What a reset, a wipe, a new
    /// modifier set and a finished dungeon run.
    pub(super) fn clear_modifier_ground(&mut self) {
        let mut ended = Vec::new();
        self.modifiers.clear_into(&mut ended);
        let tick = self.tick;
        for gone in ended {
            self.events.push(match gone {
                EndedModifier::Beams(id) => CombatEvent::ModifierBeamsEnded { id, tick },
                EndedModifier::Plague(id) => CombatEvent::PlagueCloudEnded { id, tick },
            });
        }
    }

    /// Files an authority's modifier event away so presentation can draw it. Only a client
    /// under a remote authority does this; a simulation that runs the modifiers made the
    /// event itself.
    pub(super) fn observe_modifier_event(&mut self, event: &CombatEvent) {
        if self.remote_authority {
            self.modifiers.observe(event, self.time);
        }
    }

    /// Drops mirrored ground whose end event was lost. Once a tick on a client.
    pub(super) fn prune_modifier_mirror(&mut self) {
        let now = self.time;
        self.modifiers.prune_mirror(now);
    }

    // ------------------------------------------------------------------------- the beams

    /// Whether encounter `index` is fighting: pulled, and somebody in it is awake.
    ///
    /// A pack that leashed and walked home is no longer engaged: when its last member is
    /// idle again the beams go out, and a second pull lights a new emitter.
    fn encounter_engaged(&self, index: usize) -> bool {
        let Some(encounter) = self.encounters.get(index) else {
            return false;
        };
        encounter.state().is_active()
            && encounter.members().iter().any(|member| {
                self.world.is_alive(*member)
                    && matches!(
                        self.enemies.get(member.index()),
                        Some(Some(runtime))
                            if !matches!(runtime.brain.state(), AiState::Idle | AiState::Dead)
                    )
            })
    }

    /// Lights an emitter for every encounter that has just engaged and puts out the ones
    /// whose encounter has ended or been reset.
    fn sync_beam_emitters(&mut self, params: &BeamParams) {
        for index in 0..self.encounters.len() {
            let engaged = self.encounter_engaged(index);
            let existing = self
                .modifiers
                .beams()
                .iter()
                .find(|emitter| emitter.encounter == index)
                .map(|emitter| emitter.id);
            match (engaged, existing) {
                (true, None) => self.start_beams(index, params),
                (false, Some(id)) => {
                    self.modifiers.beam_field().remove(id);
                    let tick = self.tick;
                    self.events
                        .push(CombatEvent::ModifierBeamsEnded { id, tick });
                }
                _ => {}
            }
        }
    }

    /// Lights the emitter for encounter `index` at its living members' centroid.
    fn start_beams(&mut self, index: usize, params: &BeamParams) {
        let mut sum = Vec3::ZERO;
        let mut count = 0.0_f32;
        if let Some(encounter) = self.encounters.get(index) {
            for member in encounter.members() {
                if self.world.is_alive(*member)
                    && let Some(position) = self.world.position(*member)
                {
                    sum += position;
                    count += 1.0;
                }
            }
        }
        if count <= 0.0 {
            return;
        }
        let center = sum / count;
        // Two draws from the enemies' own generator: where beam zero starts, and which way
        // they turn. A replay of the seed is a replay of the beams.
        let start_angle = self.enemy_random() * std::f32::consts::TAU;
        let direction = if self.enemy_random() < 0.5 { 1.0 } else { -1.0 };
        let now = self.time;
        let id = self.modifiers.fresh_id();
        let emitter = BeamEmitter {
            id,
            encounter: index,
            center,
            start_angle,
            angular_speed: direction * params.angular_speed_deg.to_radians(),
            radius: params.radius,
            width: params.width,
            beams: params.beams.max(1),
            start_time: now,
            warmup_end_time: now + f64::from(params.warmup.max(0.0)),
        };
        let tick = self.tick;
        if let Some(dropped) = self.modifiers.beam_field().insert(emitter) {
            self.events
                .push(CombatEvent::ModifierBeamsEnded { id: dropped, tick });
        }
        self.events.push(CombatEvent::ModifierBeamsStarted {
            id,
            center,
            start_angle,
            angular_speed: emitter.angular_speed,
            radius: emitter.radius,
            width: emitter.width,
            beams: emitter.beams,
            warmup_end_time: emitter.warmup_end_time,
            start_time: emitter.start_time,
            tick,
        });
    }

    /// Hurts every player standing in a live beam, once every [`MODIFIER_TICK_INTERVAL`].
    ///
    /// One shared clock for every emitter, so a player caught by two of them still takes one
    /// tick: "no more than one tick per interval per player" is the clock's property and not
    /// something the loop has to remember.
    fn tick_beam_damage(&mut self, step: f32, params: &BeamParams, magnitude: f32) {
        if self.modifiers.beams().is_empty() || !self.modifiers.beam_tick_due(step) {
            return;
        }
        let now = self.time;
        let mut victims = std::mem::take(&mut self.modifiers.scratch_hits);
        victims.clear();
        for player in self.player_entities() {
            if !self.world.is_alive(player) {
                continue;
            }
            let Some(position) = self.world.position(player) else {
                continue;
            };
            let radius = self.world.radius(player);
            let hit =
                self.modifiers.beams().iter().any(|emitter| {
                    !emitter.is_warming(now) && emitter.contains(now, position, radius)
                });
            if hit {
                victims.push((player, params.damage_pct_max_hp));
            }
        }
        for (victim, pct) in victims.drain(..) {
            self.modifier_hit(
                victim,
                pct,
                magnitude,
                params.max_hit_pct,
                DamageType::Fire,
                true,
            );
        }
        self.modifiers.scratch_hits = victims;
    }

    // ------------------------------------------------------------------------- the plague

    /// Arms the plague on a monster that has just died. The one hook inside the damage path.
    ///
    /// Players do not fester, and neither does the boss: its death ends the dungeon.
    pub(super) fn modifier_on_death(&mut self, entity: EntityId, position: Vec3) {
        let Some((params, _)) = self.modifiers.plague_rules() else {
            return;
        };
        if self.world.kind(entity) != Some(EntityKind::Enemy) || self.is_boss_entity(entity) {
            return;
        }
        let now = self.time;
        let id = self.modifiers.fresh_id();
        let fuse = PlagueFuse {
            id,
            position,
            radius: params.explosion_radius,
            armed_time: now,
            explode_time: now + f64::from(params.fuse.max(0.0)),
        };
        let tick = self.tick;
        if let Some(dropped) = self.modifiers.plague_field().insert_fuse(fuse) {
            self.events
                .push(CombatEvent::PlagueCloudEnded { id: dropped, tick });
        }
        self.events.push(CombatEvent::PlagueArmed {
            id,
            position,
            radius: fuse.radius,
            armed_time: fuse.armed_time,
            explode_time: fuse.explode_time,
            tick,
        });
    }

    /// Bursts the fuses that are due, ticks the clouds, and takes the thin ones away.
    fn tick_plague(&mut self, step: f32, params: &PlagueParams, magnitude: f32) {
        let now = self.time;
        let tick = self.tick;

        let mut advanced = std::mem::take(&mut self.modifiers.scratch_step);
        advanced.clear();
        self.modifiers.plague_field().advance(now, &mut advanced);

        for id in advanced.ended.drain(..) {
            self.events.push(CombatEvent::PlagueCloudEnded { id, tick });
        }

        // Every burst this tick is summed per player *before* anything lands, so a stampede
        // of corpses bursting together is still one capped hit and not a chain of them.
        let mut victims = std::mem::take(&mut self.modifiers.scratch_hits);
        victims.clear();
        for fuse in advanced.burst.drain(..) {
            self.events.push(CombatEvent::PlagueExploded {
                id: fuse.id,
                position: fuse.position,
                radius: fuse.radius,
                tick,
            });
            let cloud = PlagueCloud {
                id: fuse.id,
                position: fuse.position,
                radius: params.cloud_radius,
                end_time: now + f64::from(params.cloud_duration.max(0.0)),
            };
            if let Some(dropped) = self.modifiers.plague_field().insert_cloud(cloud) {
                self.events
                    .push(CombatEvent::PlagueCloudEnded { id: dropped, tick });
            }
            for player in self.player_entities() {
                if !self.world.is_alive(player) {
                    continue;
                }
                let Some(position) = self.world.position(player) else {
                    continue;
                };
                if crate::modifier::plague::inside(
                    fuse.position,
                    fuse.radius,
                    position,
                    self.world.radius(player),
                ) {
                    match victims.iter_mut().find(|(who, _)| *who == player) {
                        Some((_, pct)) => *pct += params.explosion_pct_max_hp,
                        None => victims.push((player, params.explosion_pct_max_hp)),
                    }
                }
            }
        }
        self.modifiers.scratch_step = advanced;
        for (victim, pct) in victims.drain(..) {
            self.modifier_hit(
                victim,
                pct,
                magnitude,
                params.max_hit_pct,
                DamageType::Nature,
                false,
            );
        }
        self.modifiers.scratch_hits = victims;

        self.tick_clouds(step, params, magnitude);
    }

    /// Hurts and infects everyone standing in a cloud, once every interval.
    fn tick_clouds(&mut self, step: f32, params: &PlagueParams, magnitude: f32) {
        if self.modifiers.clouds().is_empty() || !self.modifiers.cloud_tick_due(step) {
            return;
        }
        let mut victims = std::mem::take(&mut self.modifiers.scratch_hits);
        victims.clear();
        for player in self.player_entities() {
            if !self.world.is_alive(player) {
                continue;
            }
            let Some(position) = self.world.position(player) else {
                continue;
            };
            let radius = self.world.radius(player);
            let hit = self.modifiers.clouds().iter().any(|cloud| {
                crate::modifier::plague::inside(cloud.position, cloud.radius, position, radius)
            });
            if hit {
                victims.push((
                    player,
                    params.cloud_pct_max_hp_per_sec * MODIFIER_TICK_INTERVAL,
                ));
            }
        }
        for (victim, pct) in victims.drain(..) {
            self.infect(victim, params);
            self.modifier_hit(
                victim,
                pct,
                magnitude,
                params.max_hit_pct,
                DamageType::Nature,
                true,
            );
        }
        self.modifiers.scratch_hits = victims;
    }

    /// Gives `target` the Plagued debuff, refreshing it while it has little time left.
    ///
    /// Refreshed only when it is running low, so standing in a cloud is one status event
    /// every second or so rather than one every tick of the cloud.
    fn infect(&mut self, target: EntityId, params: &PlagueParams) {
        let linger = params.debuff_linger.max(MODIFIER_TICK_INTERVAL);
        let remaining = self
            .world
            .statuses(target)
            .map_or(0.0, |statuses| statuses.remaining(StatusKind::Plagued));
        if remaining > linger * 0.75 {
            return;
        }
        let status = Status::new(
            StatusKind::Plagued,
            EntityId::INVALID,
            AbilityId::NONE,
            linger,
            0.0,
        )
        .with_magnitude(params.heal_reduction);
        self.apply_status(target, status, linger);
    }

    // ------------------------------------------------------------------------- the damage

    /// One modifier hit on `target`: `pct` per cent of its maximum health, times
    /// `magnitude`, never more than `cap_pct` per cent.
    ///
    /// The one place a modifier hurts anybody. Credited to nobody and to no ability,
    /// skipping the multipliers `deal_damage` would apply (see the module docs), and
    /// flagged periodic when it is a tick rather than a burst.
    fn modifier_hit(
        &mut self,
        target: EntityId,
        pct: f32,
        magnitude: f32,
        cap_pct: f32,
        damage_type: DamageType,
        periodic: bool,
    ) {
        let Some(health) = self.world.health(target) else {
            return;
        };
        let max = health.max();
        let percent = (pct * magnitude).min(cap_pct).max(0.0);
        let amount = max * percent / 100.0;
        if amount <= 0.0 {
            return;
        }
        self.slaying = true;
        self.periodic_damage = periodic;
        self.deal_damage(
            EntityId::INVALID,
            target,
            amount,
            AbilityId::NONE,
            damage_type,
            false,
        );
        self.periodic_damage = false;
        self.slaying = false;
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use crate::arena::{self, ArenaConfig};
    use crate::encounter::{Encounter, EnemySpawn};
    use crate::enemy::EnemyKind;

    use super::*;

    fn dungeon() -> Simulation {
        let config = ArenaConfig::with_group(0);
        let at = |x: f32| config.player_spawn + Vec3::new(x, 0.0, -40.0);
        let mut simulation = arena::encounter_arena(
            &config,
            Encounter::with_spawns(
                "PACK",
                vec![
                    EnemySpawn::new(EnemyKind::Melee, at(0.0)),
                    EnemySpawn::new(EnemyKind::Caster, at(3.0)),
                ],
            ),
        );
        simulation.add_encounter(Encounter::with_spawns(
            "BOSS",
            vec![EnemySpawn::new(EnemyKind::Boss, at(20.0))],
        ));
        simulation
    }

    #[test]
    fn kill_progress_counts_placed_monsters_but_neither_the_boss_nor_summons() {
        let mut simulation = dungeon();
        assert_eq!(simulation.dungeon_kill_progress(), (0, 2));

        let first = simulation.encounters[0].members()[0];
        simulation.slay(first);
        assert_eq!(simulation.dungeon_kill_progress(), (1, 2));

        // A boss's add is called into the fight, not placed by the level: it neither adds to
        // the total nor to what has been killed.
        let add = simulation.spawn_enemy(EnemyKind::Melee, Vec3::new(0.0, 0.0, -60.0), 0.0);
        simulation.encounters[1].add_summon(add);
        assert_eq!(simulation.dungeon_kill_progress(), (1, 2));
        simulation.slay(add);
        assert_eq!(simulation.dungeon_kill_progress(), (1, 2));

        // Nor does the boss's own death.
        let boss = simulation.encounters[1].members()[0];
        simulation.slay(boss);
        assert_eq!(simulation.dungeon_kill_progress(), (1, 2));

        let second = simulation.encounters[0].members()[1];
        simulation.slay(second);
        assert_eq!(simulation.dungeon_kill_progress(), (2, 2));
    }

    #[test]
    fn an_empty_simulation_has_no_progress() {
        assert_eq!(
            Simulation::new(super::super::SimConfig::default()).dungeon_kill_progress(),
            (0, 0)
        );
    }
}
