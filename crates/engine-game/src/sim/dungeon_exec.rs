//! The simulation's half of a dungeon run: sealing the boss, crediting kills, opening the
//! gate, calling the adds and putting it all back.
//!
//! ```text
//! death ──▶ pending credit ──▶ DungeonRun::credit ──▶ DungeonProgress
//!                                     │ requirement met
//!                                     ▼
//!                           gate removed ──▶ BossUnlocked
//!
//! boss encounter Active ──▶ choose adds ──▶ spawn empowered ──▶ AddSummoned × n
//! boss home and idle    ──▶ adds taken away, boss restored  ──▶ can be pulled again
//! boss encounter done   ──▶ DungeonCompleted
//! ```
//!
//! # A run is optional
//!
//! A simulation with no [`DungeonRun`] installed — the arena, the Ashen Reliquary, every test
//! bed — runs none of this: every entry point here returns at its first line. Nothing the
//! dungeon added costs a scene that is not one.

use engine_core::math::Vec3;

use crate::dungeon_run::{DungeonRules, DungeonRun, choose_adds};
use crate::encounter::EncounterState;
use crate::enemy::brain::AiState;
use crate::enemy::{EnemyKind, level_scale};
use crate::entity::EntityId;
use crate::event::CombatEvent;

use super::Simulation;

/// How one enemy is spawned, beyond what it is and where.
///
/// Everything a level placement can say about a monster that is not in the monster's own
/// table row. The default is the monster exactly as designed.
#[derive(Debug, Clone, PartialEq)]
pub struct EnemySpawnOptions {
    /// Its level, `1..`. Scales health and damage — see [`level_scale`].
    pub level: u8,
    /// What its health is multiplied by on top of its level: the party size the dungeon
    /// was built for.
    pub health_scale: f32,
    /// What its damage is multiplied by on top of its level.
    pub damage_scale: f32,
    /// Its aggro radius, when it is not the monster's own.
    pub aggro_radius: Option<f32>,
    /// What killing it is worth towards completion, when it is not the monster's own.
    pub completion: Option<f32>,
    /// Uniform size multiplier: a larger body is a wider and taller one.
    pub size: f32,
    /// The name the level gave it.
    pub name: Option<String>,
}

impl Default for EnemySpawnOptions {
    fn default() -> Self {
        Self {
            level: 1,
            health_scale: 1.0,
            damage_scale: 1.0,
            aggro_radius: None,
            completion: None,
            size: 1.0,
            name: None,
        }
    }
}

impl Simulation {
    /// Spawns an enemy with everything a level placement can say about it.
    ///
    /// [`Simulation::spawn_enemy`] with the defaults is exactly the monster as designed; this
    /// is the same spawn with its level, its party scaling, its aggro radius, its completion
    /// value, its size and its name applied on top.
    pub fn spawn_enemy_with(
        &mut self,
        kind: EnemyKind,
        position: Vec3,
        yaw: f32,
        options: &EnemySpawnOptions,
    ) -> EntityId {
        let archetype = kind.archetype();
        let (level_health, level_damage) = level_scale(options.level);
        let size = if options.size.is_finite() && options.size > 0.0 {
            options.size
        } else {
            1.0
        };
        let mut desc = crate::world::enemy_desc(kind, position);
        desc.max_health = archetype.max_health * level_health * options.health_scale.max(0.01);
        desc.radius *= size;
        desc.height *= size;
        let entity = self.world.spawn(desc);
        if let Some(body) = self.world.body_mut(entity) {
            body.facing_yaw = yaw;
        }
        let slot = entity.index();
        if self.enemies.len() <= slot {
            self.enemies.resize(slot + 1, None);
        }
        let mut runtime = super::enemy_exec::EnemyRuntime::new(kind, position, yaw);
        runtime.damage_scale = level_damage * options.damage_scale.max(0.0);
        if let Some(completion) = options.completion {
            runtime.completion = completion.max(0.0);
        }
        if let Some(radius) = options.aggro_radius {
            runtime.brain.set_aggro_radius(radius);
        }
        runtime.display_name = options.name.clone();
        self.enemies[slot] = Some(runtime);
        entity
    }

    /// Takes an enemy out of the world entirely: its body, its mind, its warnings and its
    /// ground.
    ///
    /// What a boss's add goes through when the fight it was called into is abandoned: an add
    /// is not *restored* like a pack member, because it was never part of the level.
    pub fn despawn_enemy(&mut self, entity: EntityId) {
        if let Some(Some(runtime)) = self.enemies.get_mut(entity.index()) {
            let telegraphs = runtime.telegraphs.take();
            let cast = runtime.brain.die();
            let anchor = self.world.position(entity).unwrap_or(Vec3::ZERO);
            if let Some((first, count)) = telegraphs {
                self.end_telegraphs(first, count, anchor, false);
            }
            if let Some(cast) = cast {
                self.events.push(CombatEvent::CastEnded {
                    source: entity,
                    ability: crate::ability::AbilityId::from_enemy(cast.ability),
                    end: super::CastEnd::CasterDied,
                    tick: self.tick,
                });
            }
        }
        self.clear_hazards_of(entity);
        self.clear_strikes_of(entity);
        if let Some(slot) = self.enemies.get_mut(entity.index()) {
            *slot = None;
        }
        self.world.despawn(entity);
        self.forget_target_everywhere(entity);
    }

    /// Takes every enemy, every encounter and the dungeon run out of the simulation.
    ///
    /// What a level edited in the editor goes through before its monsters are built again:
    /// the player, the dummies and the collision world are left alone, and every warning,
    /// pool and mark an enemy left behind comes down with it.
    pub fn clear_enemies(&mut self) {
        self.dismiss_summons();
        let enemies: Vec<EntityId> = self.enemies().map(|(entity, _)| entity).collect();
        for enemy in enemies {
            self.despawn_enemy(enemy);
        }
        self.enemies.clear();
        self.encounters.clear();
        self.clear_all_strikes();
        self.clear_all_hazards();
        self.pending_credit.clear();
        if let Some(id) = self.gate_static.take() {
            let world = self.collision_world_mut();
            if world.remove_static(id) {
                world.rebuild_broad_phase();
            }
        }
        self.dungeon = None;
        self.clear_loot_chest();
    }

    /// The name an enemy is shown under: the one its level gave it, or its monster's.
    #[must_use]
    pub fn enemy_name(&self, entity: EntityId) -> Option<&str> {
        let runtime = self.enemies.get(entity.index())?.as_ref()?;
        Some(
            runtime
                .display_name
                .as_deref()
                .unwrap_or(runtime.brain.archetype().name),
        )
    }

    /// Gives an enemy the name its level placement carried.
    pub fn set_enemy_name(&mut self, entity: EntityId, name: String) {
        if let Some(Some(runtime)) = self.enemies.get_mut(entity.index()) {
            runtime.display_name = (!name.trim().is_empty()).then_some(name);
        }
    }

    /// Gives an enemy the loop its level placement asked it to walk while idle.
    pub fn set_enemy_patrol(&mut self, entity: EntityId, waypoints: Vec<Vec3>) {
        if let Some(Some(runtime)) = self.enemies.get_mut(entity.index()) {
            runtime.brain.set_patrol(waypoints);
        }
    }

    /// What killing an enemy is worth towards dungeon completion.
    #[must_use]
    pub fn completion_value(&self, entity: EntityId) -> f32 {
        self.enemies
            .get(entity.index())
            .and_then(|slot| slot.as_ref())
            .map_or(0.0, |runtime| runtime.completion)
    }

    /// Makes this simulation a dungeon run of `rules`.
    ///
    /// Seals the boss encounter's members behind the gate — they notice nobody and take no
    /// damage — and puts the gate's collision in the world, until the requirement is met.
    pub fn set_dungeon(&mut self, rules: DungeonRules) {
        self.dungeon = Some(DungeonRun::new(rules));
        self.seal_boss();
    }

    /// The dungeon run, if this simulation is one.
    #[must_use]
    pub fn dungeon_run(&self) -> Option<&DungeonRun> {
        self.dungeon.as_ref()
    }

    /// The dungeon run, for a test or a developer control to force the adds.
    pub fn dungeon_run_mut(&mut self) -> Option<&mut DungeonRun> {
        self.dungeon.as_mut()
    }

    /// Whether `entity` is waiting behind the boss gate.
    #[must_use]
    pub fn is_sealed(&self, entity: EntityId) -> bool {
        self.dungeon.as_ref().is_some_and(|run| {
            !run.is_unlocked()
                && run
                    .rules()
                    .boss_encounter
                    .and_then(|index| self.encounters.get(index))
                    .is_some_and(|encounter| encounter.contains(entity))
        })
    }

    /// Opens the boss gate whatever the count says. A development control.
    pub fn unlock_dungeon(&mut self) {
        let opened = self.dungeon.as_mut().is_some_and(DungeonRun::unlock);
        if opened {
            self.open_gate();
        }
    }

    /// Queues a kill for completion credit.
    ///
    /// Queued rather than credited on the spot because a death can happen while the AI loop
    /// has the enemy table taken out; [`Simulation::flush_credits`] pays what it can reach,
    /// and the dungeon tick pays the rest.
    pub(super) fn queue_credit(&mut self, entity: EntityId) {
        if self.dungeon.is_some() {
            self.pending_credit.push(entity);
        }
    }

    /// Credits every queued kill whose enemy can be reached right now.
    pub(super) fn flush_credits(&mut self) {
        if self.pending_credit.is_empty() || self.dungeon.is_none() {
            return;
        }
        let mut pending = std::mem::take(&mut self.pending_credit);
        let mut open = false;
        // What died and may drop loot, as `(boss, elite)`; rolled once the table is back.
        let drops_loot = self.loot_rules.is_some() && !self.remote_authority;
        let mut kills = Vec::new();
        pending.retain(|entity| {
            let Some(Some(runtime)) = self.enemies.get_mut(entity.index()) else {
                // Not reachable yet: the AI loop has the table out. Kept for the next flush.
                return self.enemies.is_empty();
            };
            if drops_loot {
                let archetype = runtime.brain.archetype();
                kills.push((
                    archetype.tier == crate::enemy::MonsterTier::Boss,
                    archetype.role == crate::enemy::MonsterRole::Elite,
                ));
            }
            if runtime.credited || runtime.completion <= 0.0 {
                return false;
            }
            runtime.credited = true;
            let value = runtime.completion;
            let Some(run) = self.dungeon.as_mut() else {
                return false;
            };
            run.note_started(self.time);
            let met = run.credit(value);
            let (credited, fraction) = (run.credited(), run.fraction());
            let tick = self.tick;
            self.events.push(CombatEvent::DungeonProgress {
                credited,
                fraction,
                tick,
            });
            log::info!("dungeon: +{value} completion, {:.0} %", fraction * 100.0);
            open |= met;
            false
        });
        self.pending_credit = pending;
        for (boss, elite) in kills {
            self.drop_kill_loot(boss, elite);
        }
        if open {
            self.open_gate();
        }
    }

    /// The dungeon's own step: credits, the boss's adds, a fight abandoned, a dungeon done.
    ///
    /// Run once a tick, after the AI and the encounters have been brought up to date.
    pub(super) fn tick_dungeon(&mut self) {
        if self.dungeon.is_none() {
            return;
        }
        self.flush_credits();
        let Some(run) = self.dungeon.as_ref() else {
            return;
        };
        let Some(index) = run.rules().boss_encounter else {
            return;
        };
        let Some(encounter) = self.encounters.get(index) else {
            return;
        };
        let state = encounter.state();
        let boss = encounter.members().first().copied();
        let engaged = run.is_engaged();
        let unlocked = run.is_unlocked();
        let completed = run.is_completed();

        // Any pack pulled starts the clock.
        if self
            .encounters
            .iter()
            .any(|encounter| encounter.state().is_active())
            && let Some(run) = self.dungeon.as_mut()
        {
            run.note_started(self.time);
        }

        if !engaged && unlocked && state == EncounterState::Active {
            self.call_adds(index);
            return;
        }

        // The fight was abandoned: the boss walked home and forgot. Its adds go, it is made
        // whole, and the encounter can be pulled again — a wipe on the boss is a retry of
        // the boss, not of the dungeon.
        if engaged
            && state == EncounterState::Active
            && let Some(boss) = boss
            && self.world.is_alive(boss)
            && self
                .brain(boss)
                .is_some_and(|brain| brain.state() == AiState::Idle)
        {
            self.abandon_boss_fight(index);
            return;
        }

        if state == EncounterState::Completed && !completed {
            let now = self.time;
            if let Some(seconds) = self.dungeon.as_mut().and_then(|run| run.complete(now)) {
                let tick = self.tick;
                self.events
                    .push(CombatEvent::DungeonCompleted { seconds, tick });
                log::info!("dungeon completed in {seconds:.1} s");
                // The boss's chest drops where it fell; failing that, where the party stands.
                let at = boss
                    .and_then(|boss| self.world.position(boss))
                    .or_else(|| self.world.position(self.player.entity))
                    .unwrap_or(Vec3::ZERO);
                self.spawn_loot_chest(at);
            }
        }
    }

    /// Calls the boss's adds into the fight that has just started.
    fn call_adds(&mut self, index: usize) {
        let Some(run) = self.dungeon.as_ref() else {
            return;
        };
        let rules = run.rules().clone();
        let chosen = match run.forced_adds() {
            Some(forced) => forced.to_vec(),
            None => {
                let mut random = Vec::with_capacity(rules.add_count);
                for _ in 0..rules.add_count {
                    random.push(self.enemy_random());
                }
                let mut draws = random.into_iter();
                choose_adds(
                    &rules.add_pool,
                    rules.add_count,
                    rules.allow_duplicate_adds,
                    || draws.next().unwrap_or(0.0),
                )
            }
        };
        let Some(boss) = self
            .encounters
            .get(index)
            .and_then(|e| e.members().first().copied())
        else {
            return;
        };
        let boss_position = self.world.position(boss).unwrap_or(Vec3::ZERO);
        let target = self.brain(boss).and_then(|brain| brain.target());
        // The adds share the boss's scaling: a boss built for a party of one calls adds built
        // for a party of one.
        let (health_scale, damage_scale) = self
            .enemies
            .get(boss.index())
            .and_then(|slot| slot.as_ref())
            .map_or((1.0, 1.0), |runtime| {
                let designed = runtime.brain.archetype().max_health;
                let actual = self
                    .world
                    .health(boss)
                    .map_or(designed, |health| health.max());
                (actual / designed.max(1.0), runtime.damage_scale)
            });

        let mut adds = Vec::with_capacity(chosen.len());
        for (slot, kind) in chosen.iter().enumerate() {
            let empowered = kind.empowered().unwrap_or(*kind);
            let (position, yaw) = rules
                .add_spawns
                .get(slot % rules.add_spawns.len().max(1))
                .copied()
                .unwrap_or_else(|| {
                    let side = if slot % 2 == 0 { -1.0 } else { 1.0 };
                    (boss_position + Vec3::new(side * 6.0, 0.0, 2.0), 0.0)
                });
            let add = self.spawn_enemy_with(
                empowered,
                position,
                yaw,
                &EnemySpawnOptions {
                    health_scale,
                    damage_scale,
                    completion: Some(0.0),
                    ..EnemySpawnOptions::default()
                },
            );
            if let Some(target) = target
                && let Some(Some(runtime)) = self.enemies.get_mut(add.index())
            {
                runtime.brain.provoke(target);
            }
            if let Some(encounter) = self.encounters.get_mut(index) {
                encounter.add_summon(add);
            }
            let tick = self.tick;
            self.events.push(CombatEvent::AddSummoned {
                boss,
                add,
                position,
                tick,
            });
            log::info!("boss called {}", empowered.name());
            adds.push(add);
        }
        if let Some(run) = self.dungeon.as_mut() {
            run.engage(adds, chosen);
        }
    }

    /// Ends an abandoned boss fight: adds gone, boss whole, encounter ready to pull again.
    fn abandon_boss_fight(&mut self, index: usize) {
        let adds = self
            .dungeon
            .as_mut()
            .map(DungeonRun::disengage)
            .unwrap_or_default();
        for add in adds {
            self.despawn_enemy(add);
        }
        let summons = self
            .encounters
            .get_mut(index)
            .map(crate::encounter::Encounter::take_summons)
            .unwrap_or_default();
        for summon in summons {
            if self.world.contains(summon) {
                self.despawn_enemy(summon);
            }
        }
        let members = self
            .encounters
            .get(index)
            .map(|encounter| encounter.members().to_vec())
            .unwrap_or_default();
        for member in members {
            self.restore_enemy(member);
        }
        if let Some(encounter) = self.encounters.get_mut(index) {
            encounter.finish_reset();
        }
        log::info!("boss fight abandoned; the boss is ready to be pulled again");
    }

    /// Takes every add out of every encounter and out of the world. What a reset runs.
    pub(super) fn dismiss_summons(&mut self) {
        let adds = self
            .dungeon
            .as_mut()
            .map(DungeonRun::disengage)
            .unwrap_or_default();
        let mut gone = adds;
        for encounter in &mut self.encounters {
            gone.extend(encounter.take_summons());
        }
        gone.sort_by_key(|entity| entity.index());
        gone.dedup();
        for add in gone {
            if self.world.contains(add) {
                self.despawn_enemy(add);
            }
        }
    }

    /// Puts the run back to its start: nothing credited, gate shut, boss sealed.
    pub(super) fn reset_dungeon_run(&mut self) {
        self.clear_loot_chest();
        self.dismiss_summons();
        self.pending_credit.clear();
        let Some(run) = self.dungeon.as_mut() else {
            return;
        };
        run.reset();
        for runtime in self.enemies.iter_mut().flatten() {
            runtime.credited = false;
        }
        self.seal_boss();
    }

    /// Puts the gate's collision in the world if the run is still locked.
    fn seal_boss(&mut self) {
        let Some(run) = self.dungeon.as_ref() else {
            return;
        };
        if run.is_unlocked() || self.gate_static.is_some() {
            return;
        }
        let Some(gate) = run.rules().gate else {
            return;
        };
        let (min, max) = gate_bounds(&gate);
        let world = self.collision_world_mut();
        let id = world.add_box(min, max, "BOSS GATE");
        world.rebuild_broad_phase();
        self.gate_static = Some(id);
    }

    /// Takes the gate out of the world and announces it.
    fn open_gate(&mut self) {
        let position = self
            .dungeon
            .as_ref()
            .and_then(|run| run.rules().gate)
            .map_or(Vec3::ZERO, |gate| gate.center);
        if let Some(id) = self.gate_static.take() {
            let world = self.collision_world_mut();
            world.remove_static(id);
            world.rebuild_broad_phase();
        }
        let tick = self.tick;
        self.events
            .push(CombatEvent::BossUnlocked { position, tick });
        log::info!("dungeon: the path to the boss is open");
    }

    /// Whether the gate's collision is standing.
    #[must_use]
    pub fn gate_is_closed(&self) -> bool {
        self.gate_static.is_some()
    }
}

/// Developer controls for a dungeon run. Compiled only with the `dev-tools` feature, which the
/// shipping build never enables.
#[cfg(feature = "dev-tools")]
impl Simulation {
    /// The dungeon's boss, if this is a dungeon run whose boss is alive and not yet beaten.
    #[must_use]
    pub fn dev_boss(&self) -> Option<EntityId> {
        let run = self.dungeon.as_ref()?;
        if run.is_completed() {
            return None;
        }
        let index = run.rules().boss_encounter?;
        let boss = self.encounters.get(index)?.members().first().copied()?;
        self.world.is_alive(boss).then_some(boss)
    }

    /// **Teleports the player beside the boss and kills it, completing the run.**
    ///
    /// Everything goes through the paths a real run uses, so everything downstream fires as
    /// it would for a real kill (progress, `BossUnlocked`, the boss encounter completing,
    /// `DungeonCompleted`, the chest and its rolls):
    ///
    /// 1. every other living enemy is slain with [`Simulation::slay`], which pays the
    ///    completion credit that opens the boss gate;
    /// 2. if the credit did not open the gate (a requirement above what the level offers) it
    ///    is opened with [`Simulation::unlock_dungeon`];
    /// 3. the player is put down on walkable ground beside the boss;
    /// 4. the boss encounter is marked as pulled, and the boss slain. The next tick sees the
    ///    encounter completed and completes the run.
    ///
    /// Answers whether there was a boss to kill.
    pub fn dev_teleport_and_kill_boss(&mut self) -> bool {
        let Some(boss) = self.dev_boss() else {
            return false;
        };
        let Some(boss_index) = self
            .dungeon
            .as_ref()
            .and_then(|run| run.rules().boss_encounter)
        else {
            return false;
        };
        let boss_members: Vec<EntityId> = self
            .encounters
            .get(boss_index)
            .map(|encounter| encounter.members().to_vec())
            .unwrap_or_default();

        // 1. Everything that stands between the party and the boss.
        let others: Vec<EntityId> = self
            .enemies()
            .map(|(entity, _)| entity)
            .filter(|entity| *entity != boss && !boss_members.contains(entity))
            .collect();
        for entity in others {
            self.slay(entity);
        }
        self.flush_credits();

        // 2. The gate, if the credit was not enough to open it.
        self.unlock_dungeon();

        // 3. Beside the boss, on the ground.
        let boss_at = self.world.position(boss).unwrap_or(Vec3::ZERO);
        self.dev_stand_beside(boss_at, self.world.radius(boss));

        // 4. The boss: pulled, then killed through the damage path. Its adds, if any had
        // been called, go too, so the encounter has nobody left alive.
        if let Some(run) = self.dungeon.as_mut() {
            run.note_started(self.time);
        }
        if let Some(encounter) = self.encounters.get_mut(boss_index) {
            encounter.activate();
        }
        for member in boss_members {
            if member != boss {
                self.slay(member);
            }
        }
        self.slay(boss);
        log::info!("dev-tools: teleported to the boss and killed it");
        true
    }

    /// Puts the player on walkable ground a few metres from `target`, trying a ring of
    /// positions and keeping the first where the controller finds floor at the boss's height.
    fn dev_stand_beside(&mut self, target: Vec3, boss_radius: f32) {
        let player = self.player.entity;
        let distance = boss_radius + 3.0;
        let mut chosen = None;
        for step in 0..16u8 {
            let angle = f32::from(step) * std::f32::consts::TAU / 16.0;
            let candidate = Vec3::new(
                target.x + angle.sin() * distance,
                target.y + 0.1,
                target.z + angle.cos() * distance,
            );
            self.teleport_player(candidate);
            let landed = self
                .world
                .body(player)
                .is_some_and(|body| body.grounded && (body.position.y - target.y).abs() < 1.5);
            if landed {
                chosen = Some(candidate);
                break;
            }
        }
        if chosen.is_none() {
            // Nowhere on the ring has floor: fall back to the boss's own spot.
            self.teleport_player(target + Vec3::new(0.0, 0.1, 0.0));
        }
    }
}

/// The axis-aligned box a gate's rotated extents cover.
///
/// A gate is placed across a doorway and doorways here run along X or Z, so the box is
/// exact for every gate this game has; one set at an angle is covered conservatively.
fn gate_bounds(gate: &engine_environment::BossGate) -> (Vec3, Vec3) {
    let (sin, cos) = gate.yaw.sin_cos();
    let half = gate.half_extents;
    let x = (half.x * cos).abs() + (half.z * sin).abs();
    let z = (half.x * sin).abs() + (half.z * cos).abs();
    (
        Vec3::new(gate.center.x - x, gate.center.y, gate.center.z - z),
        Vec3::new(
            gate.center.x + x,
            gate.center.y + half.y * 2.0,
            gate.center.z + z,
        ),
    )
}
