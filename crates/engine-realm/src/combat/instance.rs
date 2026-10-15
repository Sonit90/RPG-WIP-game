//! One dungeon run's authoritative fight: the real [`Simulation`], with every party member
//! in it, driven by networked input.
//!
//! ```text
//! move_input ──► apply_frames ──► drive_player (one 60 Hz step per frame, as predicted)
//! combat_act ──► act ──► SelectTarget + UseAbility… queued for that player's next tick
//! combat_tick ─► advance(n) ──► step_once × n ──► WireEvents
//!                     └──► player_states / changed_monsters ──► table rows
//! ```
//!
//! The server runs exactly the gameplay code the offline game does — the same monsters,
//! brains, threat, abilities, damage, dungeon run and boss gate — with three players instead
//! of one. Nothing a client sends is an outcome: input frames are intents, actions are
//! button presses, and the simulation decides whether either is possible.
//!
//! Entities are named on the wire by [`EntityRef`]: a player by account, a monster by a key
//! that is stable for the life of the instance and — for every monster the level places —
//! across a rebuild from the tables, because placed monsters are keyed by spawn order.

use std::collections::HashMap;

use engine_collision::world::StaticId;
use engine_core::math::Vec3;
use engine_environment::EnvironmentScene;
use engine_game::command::PlayerCommand;
use engine_game::dungeon::DungeonScaling;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::{ABILITY_SLOTS, FireAbility};
use engine_game::hero::Hero;
use engine_game::loot::LootRules;
use engine_game::movement::{Body, MoveIntent};
use engine_game::sim::Simulation;
use engine_game::talent::TalentGrid;
use engine_game::world::EntityDesc;
use serde::{Deserialize, Serialize};

use super::wire::{AbilityRef, EntityRef, WireEvent, encode};
use crate::catalog::StartPoint;
use crate::loadout::{Loadout, sheet_of};
use crate::meter::{InstanceMeter, MeterProjector};
use crate::movement::{Barrier, BarrierKind, InputFrame, barrier_label};

/// Keys at and above this name monsters the fight summoned rather than the level placed.
pub const SUMMONED_KEY_BASE: u64 = 1 << 32;

/// Ticks without an input frame after which the server moves a player itself (gravity,
/// friction): 200 ms.
const IDLE_AFTER_TICKS: u32 = 12;

/// One party member entering the fight.
#[derive(Debug, Clone)]
pub struct PlayerSpec {
    /// Their account.
    pub account_id: String,
    /// Their character's name.
    pub name: String,
    /// Their class.
    pub hero: Hero,
    /// What they wear, from the persistence API. `None` is the class's starting gear.
    pub loadout: Option<Loadout>,
    /// The talents they bought, by key, from the snapshot taken at queue time.
    pub talents: Vec<String>,
    /// Every talent point they had when the snapshot was taken: what the keys must fit in.
    pub talent_points: u32,
    /// Where they start, and respawn.
    pub spawn: StartPoint,
}

/// What a player asked to do, besides moving.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", content = "v")]
pub enum ActionKind {
    /// Only change target.
    Target,
    /// The ability on bar slot `n`.
    Ability(u8),
    /// The ability on bar slot `n`, placed at a point.
    AbilityAt(u8, [f32; 3]),
    /// The ultimate, placed at a point.
    Ultimate([f32; 3]),
    /// The extra button `n`.
    Extra(u8),
    /// Take the player's item from the boss's chest. The simulation checks that the chest
    /// has landed, that they are close enough and that it is not already taken.
    OpenChest,
    /// Steer the channelled Moving Fire Beam towards a ground point.
    ///
    /// Appended last: the variant order is the wire format. Not a targeting press - the
    /// selection is left as it is - and a no-op unless the player is channelling.
    ChannelAim([f32; 3]),
}

/// A button press, with the target the player had selected when they pressed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CombatAction {
    /// The selection: nobody, a party member, or a monster.
    pub target: EntityRef,
    /// What was pressed.
    pub kind: ActionKind,
}

/// Why an action was refused before it reached the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionError {
    /// The account is not a player in this fight.
    NotInFight,
    /// A slot or button that does not exist, or a position that is not a number.
    Malformed,
}

/// A cast in progress.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CastState {
    /// What is being cast.
    pub ability: AbilityRef,
    /// Seconds in.
    pub elapsed: f32,
    /// Seconds in total.
    pub duration: f32,
}

/// One status on an entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusState {
    /// `StatusKind` index.
    pub kind: u16,
    /// Who applied it.
    pub source: EntityRef,
    /// Seconds left.
    pub remaining: f32,
    /// Seconds in total.
    pub duration: f32,
    /// Stacks.
    pub stacks: u8,
}

/// One ability's cooldown.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CooldownState {
    /// `FireAbility` index.
    pub ability: u16,
    /// Charges ready.
    pub charges: u8,
    /// Seconds to the next charge.
    pub recharge: f32,
}

/// A class's resources. Fields a class does not use are zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Resources {
    /// Fire mage heat.
    pub heat: f32,
    /// Ultimate energy.
    pub energy: f32,
    /// Druid mana.
    pub mana: f32,
    /// Druid mana maximum.
    pub max_mana: f32,
    /// Warrior Valor stacks.
    pub valor: f32,
    /// Druid astral power.
    pub astral: f32,
    /// Global cooldown left, seconds.
    pub global_cooldown: f32,
    /// Damage the player absorbs before health moves: every shield on them, summed.
    pub absorb: f32,
}

/// Everything about one player a client needs beyond its own prediction.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    /// The account.
    pub account_id: String,
    /// The body.
    pub body: Body,
    /// Health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Alive.
    pub alive: bool,
    /// Their selection.
    pub target: EntityRef,
    /// Their cast.
    pub cast: Option<CastState>,
    /// Their resources.
    pub resources: Resources,
    /// Cooldowns of their class's abilities.
    pub cooldowns: Vec<CooldownState>,
    /// Statuses on them.
    pub statuses: Vec<StatusState>,
    /// The last input frame applied.
    pub last_seq: u32,
}

impl PlayerState {
    /// Whether this state is worth a combat-row write over `older`, written `elapsed`
    /// seconds earlier. Clocks a client runs down by itself (cooldowns, cast progress,
    /// status timers) count only when they jump; the body and health are compared against
    /// the row by the caller, because movement input writes them too.
    #[must_use]
    pub fn combat_differs(&self, older: &Self, elapsed: f32) -> bool {
        let ran_down = |old: f32, new: f32| ((old - elapsed).max(0.0) - new).abs() > 0.2;
        let resource = |a: f32, b: f32| (a - b).abs() > 0.5;
        let (r, o) = (&self.resources, &older.resources);
        self.alive != older.alive
            || self.target != older.target
            || self.cast.map(|c| (c.ability, c.duration))
                != older.cast.map(|c| (c.ability, c.duration))
            || matches!((self.cast, older.cast), (Some(n), Some(c)) if ((c.elapsed + elapsed) - n.elapsed).abs() > 0.2)
            || resource(r.heat, o.heat)
            || resource(r.energy, o.energy)
            || resource(r.mana, o.mana)
            || resource(r.max_mana, o.max_mana)
            || resource(r.valor, o.valor)
            || resource(r.astral, o.astral)
            || resource(r.absorb, o.absorb)
            || ran_down(o.global_cooldown, r.global_cooldown)
            || self.cooldowns.len() != older.cooldowns.len()
            || self.cooldowns.iter().zip(&older.cooldowns).any(|(n, c)| {
                n.ability != c.ability || n.charges != c.charges || ran_down(c.recharge, n.recharge)
            })
            || statuses_differ(&self.statuses, &older.statuses, elapsed)
    }
}

fn statuses_differ(new: &[StatusState], old: &[StatusState], elapsed: f32) -> bool {
    new.len() != old.len()
        || new.iter().zip(old).any(|(n, o)| {
            n.kind != o.kind
                || n.stacks != o.stacks
                || n.source != o.source
                || ((o.remaining - elapsed).max(0.0) - n.remaining).abs() > 0.3
        })
}

/// Everything about one monster a client shows.
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterState {
    /// Its key.
    pub key: u64,
    /// Its kind's id (`skeleton_warrior`).
    pub kind: &'static str,
    /// Its display name.
    pub name: String,
    /// Feet.
    pub position: Vec3,
    /// Facing.
    pub yaw: f32,
    /// Health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Alive.
    pub alive: bool,
    /// Whom it is fighting.
    pub target: EntityRef,
    /// Its cast.
    pub cast: Option<CastState>,
    /// Statuses on it.
    pub statuses: Vec<StatusState>,
}

impl MonsterState {
    /// Whether `other` is worth a row write, `elapsed` seconds after this one was written:
    /// anything but sub-centimetre drift and timers running down as expected.
    fn differs(&self, other: &Self, elapsed: f32) -> bool {
        (self.position - other.position).length() > 0.01
            || (self.yaw - other.yaw).abs() > 0.01
            || self.health != other.health
            || self.max_health != other.max_health
            || self.alive != other.alive
            || self.target != other.target
            || self.cast.map(|c| (c.ability, c.duration))
                != other.cast.map(|c| (c.ability, c.duration))
            || statuses_differ(&other.statuses, &self.statuses, elapsed)
            || self.name != other.name
    }
}

/// What a monster row says, for rebuilding a fight from the tables.
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterRecord {
    /// Its key.
    pub key: u64,
    /// Alive.
    pub alive: bool,
    /// Health.
    pub health: f32,
    /// Feet.
    pub position: Vec3,
}

/// What a player row says, for rebuilding a fight from the tables.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerRecord {
    /// The account.
    pub account_id: String,
    /// The body.
    pub body: Body,
    /// Health.
    pub health: f32,
    /// The last input frame applied.
    pub last_seq: u32,
}

/// The tables' view of a fight in progress.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Restore {
    /// The tick the fight had reached.
    pub tick: u64,
    /// Completion credited.
    pub credited: f32,
    /// Whether the boss gate was open.
    pub unlocked: bool,
    /// Monsters.
    pub monsters: Vec<MonsterRecord>,
    /// Players.
    pub players: Vec<PlayerRecord>,
}

struct Member {
    account_id: String,
    entity: EntityId,
    last_seq: u32,
    idle_ticks: u32,
}

/// An item the boss's chest rolled: its stable key and its instance properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RolledItem {
    /// The item's stable key.
    pub key: &'static str,
    /// Its quality, enhancement and broken state.
    pub props: engine_game::character::ItemProps,
}

/// One instance's fight.
pub struct CombatInstance {
    sim: Simulation,
    members: Vec<Member>,
    keys: HashMap<EntityId, u64>,
    entities: HashMap<u64, EntityId>,
    next_summoned: u64,
    start_barrier: Vec<StaticId>,
    tick_base: u64,
    completed: Option<f32>,
    written: HashMap<u64, (MonsterState, u64)>,
    written_players: HashMap<String, (PlayerState, u64)>,
    meter: InstanceMeter,
    projector: MeterProjector,
}

/// The talents a player enters the fight with.
///
/// Checked again here, whatever the API did: a key the hero lacks or a set that costs more than
/// the snapshot's points drops *all* the talents (and says so) rather than half of them.
fn talent_grid_of(spec: &PlayerSpec) -> TalentGrid {
    match TalentGrid::from_keys(spec.hero, &spec.talents, spec.talent_points) {
        Ok(grid) => grid,
        Err(error) => {
            log::warn!(
                "event=talents_dropped account_id={} hero={} reason={}",
                spec.account_id,
                spec.hero.config_name(),
                error.as_str()
            );
            TalentGrid::with_points(spec.hero, spec.talent_points)
        }
    }
}

impl CombatInstance {
    /// The fight for `scene` with `players` in it, the start barrier standing. `key_scaling` is
    /// the dungeon key's `(health, damage)` multipliers (see [`crate::key::scaling`]); `(1.0,
    /// 1.0)` for no key.
    ///
    /// # Panics
    ///
    /// Never for a non-empty party; an empty one is given a fight with nobody to drive.
    #[must_use]
    pub fn new(
        scene: &EnvironmentScene,
        players: &[PlayerSpec],
        barriers: &[Barrier],
        key_scaling: (f32, f32),
    ) -> Self {
        let size = u8::try_from(players.len().max(1)).unwrap_or(u8::MAX);
        let mut sim = engine_game::arena::environment_arena_for_party(
            scene,
            0,
            DungeonScaling::for_party(size).with_key(key_scaling.0, key_scaling.1),
        );
        let mut members = Vec::with_capacity(players.len());
        for (index, spec) in players.iter().enumerate() {
            let spawn = Vec3::new(
                spec.spawn.position[0],
                spec.spawn.position[1],
                spec.spawn.position[2],
            );
            let entity = if index == 0 {
                sim.player().entity
            } else {
                let entity = sim.world_mut().spawn(EntityDesc::player(spawn));
                sim.add_player(entity, spec.hero);
                entity
            };
            if let Some(body) = sim.world_mut().body_mut(entity) {
                *body = Body::at(spawn);
                body.facing_yaw = spec.spawn.yaw;
            }
            let _ = sim.with_player(entity, |sim| {
                sim.set_hero(spec.hero);
                sim.set_player_spawn(spawn, spec.spawn.yaw);
                if let Some(loadout) = &spec.loadout {
                    *sim.roster_mut().sheet_mut(spec.hero) = sheet_of(spec.hero, loadout);
                }
                sim.set_talents(talent_grid_of(spec));
                sim.refresh_character();
                // A selection the arena made for a solo player means nothing here.
                sim.push_command(PlayerCommand::ClearTarget);
            });
            sim.world_mut().set_label(entity, &spec.name);
            sim.set_external_movement(entity, true);
            members.push(Member {
                account_id: spec.account_id.clone(),
                entity,
                last_seq: 0,
                idle_ticks: 0,
            });
        }
        let mut instance = Self {
            sim,
            members,
            keys: HashMap::new(),
            entities: HashMap::new(),
            next_summoned: SUMMONED_KEY_BASE,
            start_barrier: Vec::new(),
            tick_base: 0,
            completed: None,
            written: HashMap::new(),
            written_players: HashMap::new(),
            meter: InstanceMeter::new(players.iter().map(|p| p.account_id.clone())),
            projector: MeterProjector::default(),
        };
        // Placed monsters, keyed by spawn order: the same level gives the same keys.
        let placed: Vec<EntityId> = instance.sim.enemies().map(|(entity, _)| entity).collect();
        for (key, entity) in placed.into_iter().enumerate() {
            instance.bind(entity, key as u64);
        }
        for barrier in barriers.iter().filter(|b| b.kind == BarrierKind::Start) {
            let world = instance.sim.collision_world_mut();
            instance.start_barrier.push(world.add_box(
                barrier.min,
                barrier.max,
                barrier_label(barrier.kind),
            ));
        }
        instance.sim.collision_world_mut().rebuild_broad_phase();
        instance
    }

    /// Hands the fight the loot rules it rolls with, the key level they are rolled at and the
    /// run's reward bonus (per cent). Called once, before the first tick, by whoever builds the
    /// fight (and again on a rebuild, which loses it with everything else the tables do not
    /// hold). Without it the boss's chest is the legacy one-item chest and monsters drop nothing.
    pub fn set_loot(&mut self, rules: LootRules, key_level: u32, reward_bonus_pct: u32) {
        self.sim.set_loot_rules(rules, key_level, reward_bonus_pct);
    }

    /// Changes the run's reward bonus, per cent: what a key modifier grants. It scales what is
    /// rolled from now on (currency chances and gold, the chest's chance of an extra item); the
    /// realm scales the run's resource payout with the same number
    /// ([`crate::rewards::RunRewards::with_bonus`]).
    pub fn set_reward_bonus(&mut self, reward_bonus_pct: u32) {
        self.sim.set_reward_bonus(reward_bonus_pct);
    }

    /// Operator and test tool: kills every living monster at once, the first player credited,
    /// and returns the events the kills announced (their drops among them). The run then
    /// completes through the ordinary ticks, chest and all - unlike an admin finish, which ends
    /// a run without one.
    pub fn admin_kill_all(&mut self) -> Vec<WireEvent> {
        let Some(killer) = self.members.first().map(|member| member.entity) else {
            return Vec::new();
        };
        let living: Vec<EntityId> = self
            .sim
            .enemies()
            .map(|(entity, _)| entity)
            .filter(|entity| self.sim.world().is_alive(*entity))
            .collect();
        for entity in living {
            self.sim.apply_damage(
                killer,
                entity,
                1.0e7,
                engine_game::ability::AbilityId::from_fire(FireAbility::Firebolt),
                engine_game::combat::DamageType::Fire,
            );
        }
        // A kill made between steps announces itself into the current stream, which the next
        // step clears: it is read here.
        let announced: Vec<CombatEvent> = self.sim.events().to_vec();
        let name = |entity: EntityId| self.name(entity);
        announced
            .iter()
            .filter_map(|event| encode(event, &name))
            .collect()
    }

    /// The run's reward bonus, per cent. Zero by default.
    #[must_use]
    pub fn reward_bonus(&self) -> u32 {
        self.sim.reward_bonus()
    }

    /// Puts the run's dungeon modifiers in force (see `ModifierCatalog::resolve`). Called once,
    /// before the first tick, with what the key resolved to; a keyless run sets nothing.
    pub fn set_modifiers(&mut self, modifiers: Vec<engine_game::modifier::ActiveModifier>) {
        self.sim.set_modifiers(modifiers);
    }

    /// How much of the dungeon's placed monsters are dead: `(killed, total)`, boss and its adds
    /// excluded. What unlocks a key reroll (`Simulation::dungeon_kill_progress`).
    #[must_use]
    pub fn kill_progress(&self) -> (u32, u32) {
        self.sim.dungeon_kill_progress()
    }

    /// Slays placed monsters until `fraction` of them are dead; returns how many. The realm's
    /// operator tool for reaching the reroll's unlock (see `Simulation::slay_placed_until`).
    pub fn slay_placed_until(&mut self, fraction: f32) -> u32 {
        self.sim.slay_placed_until(fraction)
    }

    /// The fight rebuilt from the tables after the server lost it (a restart, a fresh module
    /// instance). Placed monsters keep their keys; ones the tables say are dead are removed;
    /// summoned ones are gone; casts, threat and cooldowns start over.
    #[must_use]
    pub fn restore(
        scene: &EnvironmentScene,
        players: &[PlayerSpec],
        barriers: &[Barrier],
        key_scaling: (f32, f32),
        barrier_open: bool,
        state: &Restore,
    ) -> Self {
        let mut instance = Self::new(scene, players, barriers, key_scaling);
        instance.tick_base = state.tick;
        if barrier_open {
            instance.open_start_barrier();
        }
        let records: HashMap<u64, &MonsterRecord> =
            state.monsters.iter().map(|m| (m.key, m)).collect();
        let placed: Vec<(u64, EntityId)> =
            instance.entities.iter().map(|(k, e)| (*k, *e)).collect();
        for (key, entity) in placed {
            match records.get(&key) {
                Some(record) if record.alive => {
                    if let Some(health) = instance.sim.world_mut().health_mut(entity) {
                        health.set_current(record.health.max(1.0));
                    }
                    if let Some(body) = instance.sim.world_mut().body_mut(entity) {
                        body.position = record.position;
                    }
                }
                _ => {
                    instance.sim.despawn_enemy(entity);
                    instance.unbind(entity);
                }
            }
        }
        if let Some(run) = instance.sim.dungeon_run_mut() {
            let _ = run.credit(state.credited);
        }
        if state.unlocked {
            instance.sim.unlock_dungeon();
        }
        for record in &state.players {
            let Some(member) = instance
                .members
                .iter_mut()
                .find(|m| m.account_id == record.account_id)
            else {
                continue;
            };
            member.last_seq = record.last_seq;
            let entity = member.entity;
            if let Some(body) = instance.sim.world_mut().body_mut(entity) {
                *body = record.body;
            }
            if let Some(health) = instance.sim.world_mut().health_mut(entity) {
                health.set_current(record.health);
            }
        }
        // Whatever the rebuild emitted (the gate opening) was already seen by the clients.
        instance.sim.retain_events(|_| false);
        instance
    }

    fn bind(&mut self, entity: EntityId, key: u64) {
        self.keys.insert(entity, key);
        self.entities.insert(key, entity);
    }

    fn unbind(&mut self, entity: EntityId) {
        if let Some(key) = self.keys.remove(&entity) {
            self.entities.remove(&key);
        }
    }

    /// The simulation, for reading.
    #[must_use]
    pub fn simulation(&self) -> &Simulation {
        &self.sim
    }

    /// The fight's tick: survives a rebuild.
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick_base + self.sim.tick_count()
    }

    /// Seconds the run took, once its last boss is dead.
    #[must_use]
    pub fn completed(&self) -> Option<f32> {
        self.completed
    }

    /// Completion credited so far.
    #[must_use]
    pub fn credited(&self) -> f32 {
        self.sim
            .dungeon_run()
            .map_or(0.0, engine_game::dungeon_run::DungeonRun::credited)
    }

    /// Whether the boss gate is open.
    #[must_use]
    pub fn unlocked(&self) -> bool {
        self.sim
            .dungeon_run()
            .is_some_and(engine_game::dungeon_run::DungeonRun::is_unlocked)
    }

    /// The first item the boss's chest rolled for `account`, as its stable key, whether or not
    /// they have opened the chest. `None` when there is no chest or no roll for them.
    ///
    /// What the realm persists at the end of the run today: one item per player. A run with
    /// loot rules set rolls several; read them all with [`CombatInstance::loot_items_of`].
    #[must_use]
    pub fn loot_of(&self, account: &str) -> Option<&'static str> {
        self.loot_items_of(account).first().map(|item| item.key)
    }

    /// Every item the boss's chest rolled for `account`, in roll order, whether or not they
    /// have opened the chest. Empty when there is no chest or no roll for them.
    #[must_use]
    pub fn loot_items_of(&self, account: &str) -> Vec<RolledItem> {
        crate::loot::ensure_items_registered();
        let Some(member) = self.member(account) else {
            return Vec::new();
        };
        self.sim
            .chest_loot()
            .filter(|(player, _, _)| *player == member.entity)
            .filter_map(|(_, stack, _)| {
                engine_game::character::item(stack.item).map(|def| RolledItem {
                    key: def.key,
                    props: stack.props,
                })
            })
            .collect()
    }

    /// Takes `account` out of the fight: they left the run for good (a reconnect grace that
    /// ran out is that too; a dropped connection alone is not). Returns whether they were here.
    ///
    /// Their body is despawned and the simulation forgets them (see
    /// [`Simulation::remove_player`]): monsters re-target the others, a wipe needs only the
    /// remaining players down, and later chest rolls and completion give them nothing. The
    /// meter keeps what they did so far but stops listing them. A chest share already rolled
    /// for them stays the realm's to settle. The last member cannot be removed.
    pub fn remove_member(&mut self, account: &str) -> bool {
        let Some(index) = self.members.iter().position(|m| m.account_id == account) else {
            return false;
        };
        let entity = self.members[index].entity;
        if !self.sim.remove_player(entity) {
            return false;
        }
        self.members.remove(index);
        self.written_players.remove(account);
        self.meter.depart(account);
        true
    }

    /// Whether every player is down.
    #[must_use]
    pub fn wiped(&self) -> bool {
        self.sim.all_players_dead()
    }

    /// Lifts the start barrier.
    pub fn open_start_barrier(&mut self) {
        // The run's clock for the statistics starts when the barrier lifts.
        let now = self.time();
        self.meter.activate(now);
        if self.start_barrier.is_empty() {
            return;
        }
        let world = self.sim.collision_world_mut();
        for id in self.start_barrier.drain(..) {
            world.remove_static(id);
        }
        world.rebuild_broad_phase();
    }

    /// The entity `reference` names here.
    #[must_use]
    pub fn resolve(&self, reference: &EntityRef) -> Option<EntityId> {
        match reference {
            EntityRef::None => None,
            EntityRef::Player(account) => self.member(account).map(|m| m.entity),
            EntityRef::Monster(key) => self.entities.get(key).copied(),
        }
    }

    /// How the wire names `entity`.
    #[must_use]
    pub fn name(&self, entity: EntityId) -> EntityRef {
        if let Some(key) = self.keys.get(&entity) {
            return EntityRef::Monster(*key);
        }
        self.members
            .iter()
            .find(|m| m.entity == entity)
            .map_or(EntityRef::None, |m| EntityRef::Player(m.account_id.clone()))
    }

    fn member(&self, account: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.account_id == account)
    }

    /// Applies a player's accepted input frames, one fixed step each, in order. Returns the
    /// body afterwards and the last frame applied.
    pub fn apply_frames(&mut self, account: &str, frames: &[InputFrame]) -> Option<(Body, u32)> {
        let index = self.members.iter().position(|m| m.account_id == account)?;
        let entity = self.members[index].entity;
        for frame in frames {
            self.sim.drive_player(entity, frame.intent());
            self.members[index].last_seq = frame.seq;
        }
        if !frames.is_empty() {
            self.members[index].idle_ticks = 0;
        }
        let body = *self.sim.world().body(entity)?;
        Some((body, self.members[index].last_seq))
    }

    /// Queues a button press for `account`'s next tick. The simulation decides whether it
    /// can happen: range, cooldown, resource, line of sight, being alive.
    ///
    /// # Errors
    ///
    /// [`ActionError`] if the account is not here or the action is malformed.
    pub fn act(&mut self, account: &str, action: &CombatAction) -> Result<(), ActionError> {
        let entity = self.member(account).ok_or(ActionError::NotInFight)?.entity;
        let finite = |p: &[f32; 3]| p.iter().all(|v| v.is_finite());
        let command = match action.kind {
            ActionKind::Target => None,
            ActionKind::Ability(slot) if usize::from(slot) < ABILITY_SLOTS => {
                Some(PlayerCommand::UseAbility(usize::from(slot)))
            }
            ActionKind::AbilityAt(slot, p) if usize::from(slot) < ABILITY_SLOTS && finite(&p) => {
                Some(PlayerCommand::UseAbilityAt {
                    slot: usize::from(slot),
                    position: Vec3::new(p[0], p[1], p[2]),
                })
            }
            ActionKind::Ultimate(p) if finite(&p) => Some(PlayerCommand::UseUltimate {
                position: Vec3::new(p[0], p[1], p[2]),
            }),
            ActionKind::Extra(index) if index < 2 => {
                Some(PlayerCommand::UseExtra(usize::from(index)))
            }
            ActionKind::OpenChest => {
                // Not a targeting press: the selection is left as it is.
                self.sim.push_command_for(entity, PlayerCommand::OpenChest);
                return Ok(());
            }
            ActionKind::ChannelAim(p) if finite(&p) => {
                // Steering an existing channel is not a targeting press either.
                self.sim.push_command_for(
                    entity,
                    PlayerCommand::ChannelAim {
                        position: Vec3::new(p[0], p[1], p[2]),
                    },
                );
                return Ok(());
            }
            _ => return Err(ActionError::Malformed),
        };
        let select = match self.resolve(&action.target) {
            Some(target) if target == entity => PlayerCommand::SelectSelf,
            Some(target) => PlayerCommand::SelectTarget { entity: target },
            None => PlayerCommand::ClearTarget,
        };
        self.sim.push_command_for(entity, select);
        if let Some(command) = command {
            self.sim.push_command_for(entity, command);
        }
        Ok(())
    }

    /// Runs `ticks` fixed ticks and returns what happened, in wire form.
    pub fn advance(&mut self, ticks: u32) -> Vec<WireEvent> {
        let mut out = Vec::new();
        for _ in 0..ticks {
            // Players who stopped sending input still fall and slide to a stop.
            for index in 0..self.members.len() {
                let member = &mut self.members[index];
                member.idle_ticks = member.idle_ticks.saturating_add(1);
                if member.idle_ticks > IDLE_AFTER_TICKS {
                    let entity = member.entity;
                    self.sim.drive_player(entity, MoveIntent::NONE);
                }
            }
            self.sim.step_once();
            self.key_new_monsters();
            if self.completed.is_none() {
                self.completed = self
                    .sim
                    .dungeon_run()
                    .filter(|run| run.is_completed())
                    .map(|run| run.finished_after().unwrap_or(0.0));
            }
            let events: Vec<CombatEvent> = self.sim.events().to_vec();
            let name = |entity: EntityId| self.name(entity);
            let first = out.len();
            out.extend(events.iter().filter_map(|event| encode(event, &name)));
            // The statistics read the same events, on the fight's own clock.
            let now = self.time();
            let engaged = self
                .sim
                .dungeon_run()
                .is_some_and(engine_game::dungeon_run::DungeonRun::is_engaged);
            self.meter.set_boss_engaged(now, engaged);
            self.meter.observe(now, &out[first..]);
        }
        out
    }

    /// The fight's clock in seconds: ticks at 60 Hz, surviving a rebuild.
    #[must_use]
    pub fn time(&self) -> f64 {
        self.tick() as f64 / 60.0
    }

    /// The run's statistics.
    #[must_use]
    pub fn meter(&self) -> &InstanceMeter {
        &self.meter
    }

    /// The run's statistics, mutably: the realm marks them finished with this.
    pub fn meter_mut(&mut self) -> &mut InstanceMeter {
        &mut self.meter
    }

    /// The statistics rows worth writing at `now` (server microseconds): `(account, JSON)`.
    /// Rate-limited to once a second and limited to rows whose text changed; `force` is for
    /// the end of the run.
    pub fn meter_rows(&mut self, now: crate::Micros, force: bool) -> Vec<(String, String)> {
        self.projector.project(now, &mut self.meter, force)
    }

    fn key_new_monsters(&mut self) {
        let fresh: Vec<EntityId> = self
            .sim
            .enemies()
            .map(|(entity, _)| entity)
            .filter(|entity| !self.keys.contains_key(entity))
            .collect();
        for entity in fresh {
            let key = self.next_summoned;
            self.next_summoned += 1;
            self.bind(entity, key);
        }
        let gone: Vec<EntityId> = self
            .keys
            .keys()
            .copied()
            .filter(|entity| self.sim.brain(*entity).is_none())
            .collect();
        for entity in gone {
            self.unbind(entity);
        }
    }

    fn statuses(&self, entity: EntityId) -> Vec<StatusState> {
        self.sim
            .world()
            .statuses(entity)
            .map(|set| {
                set.iter()
                    .map(|status| StatusState {
                        kind: u16::try_from(
                            engine_game::status::StatusKind::ALL
                                .iter()
                                .position(|k| *k == status.kind)
                                .unwrap_or(usize::from(u16::MAX)),
                        )
                        .unwrap_or(u16::MAX),
                        source: self.name(status.source),
                        remaining: status.remaining,
                        duration: status.duration,
                        stacks: status.stacks,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every player's state.
    #[must_use]
    pub fn player_states(&mut self) -> Vec<PlayerState> {
        let members: Vec<(String, EntityId, u32)> = self
            .members
            .iter()
            .map(|m| (m.account_id.clone(), m.entity, m.last_seq))
            .collect();
        let mut states = Vec::with_capacity(members.len());
        for (account_id, entity, last_seq) in members {
            let statuses = self.statuses(entity);
            let Some((target, cast, resources, cooldowns)) = self.sim.with_player(entity, |sim| {
                let hero = sim.hero();
                let mage = sim.mage();
                let resources = Resources {
                    heat: mage.heat.value(),
                    energy: mage.energy.value(),
                    mana: sim.druid().mana.current(),
                    max_mana: sim.druid().mana.max(),
                    valor: f32::from(sim.warrior().valor.stacks()),
                    astral: sim.druid().astral.value(),
                    global_cooldown: mage.global_cooldown.remaining(),
                    absorb: sim.absorb_of(entity),
                };
                let cooldowns = hero
                    .abilities()
                    .iter()
                    .map(|ability| {
                        let state = mage.abilities.state(*ability);
                        CooldownState {
                            ability: u16::try_from(ability.index()).unwrap_or(u16::MAX),
                            charges: state.charges,
                            recharge: state.recharge,
                        }
                    })
                    .collect::<Vec<_>>();
                let cast = sim.casting().map(|cast| CastState {
                    ability: AbilityRef::of(engine_game::ability::AbilityId::from_fire(
                        cast.ability,
                    )),
                    elapsed: cast.elapsed,
                    duration: cast.duration,
                });
                (sim.player().target, cast, resources, cooldowns)
            }) else {
                continue;
            };
            let world = self.sim.world();
            let Some(body) = world.body(entity).copied() else {
                continue;
            };
            let health = world.health(entity);
            states.push(PlayerState {
                account_id,
                body,
                health: health.map_or(0.0, engine_game::combat::Health::current),
                max_health: health.map_or(1.0, engine_game::combat::Health::max),
                alive: world.is_alive(entity),
                target: target.map_or(EntityRef::None, |t| self.name(t)),
                cast,
                resources,
                cooldowns,
                statuses,
                last_seq,
            });
        }
        states
    }

    fn monster_state(&self, key: u64, entity: EntityId) -> Option<MonsterState> {
        let brain = self.sim.brain(entity)?;
        let world = self.sim.world();
        let body = world.body(entity)?;
        let health = world.health(entity);
        Some(MonsterState {
            key,
            kind: brain.kind().id(),
            name: self
                .sim
                .enemy_name(entity)
                .map_or_else(|| brain.kind().name().to_owned(), str::to_owned),
            position: body.position,
            yaw: body.facing_yaw,
            health: health.map_or(0.0, engine_game::combat::Health::current),
            max_health: health.map_or(1.0, engine_game::combat::Health::max),
            alive: world.is_alive(entity),
            target: brain.target().map_or(EntityRef::None, |t| self.name(t)),
            cast: brain.cast().map(|cast| CastState {
                ability: AbilityRef::of(engine_game::ability::AbilityId::from_enemy(cast.ability)),
                elapsed: cast.elapsed,
                duration: cast.duration,
            }),
            statuses: self.statuses(entity),
        })
    }

    /// Every monster's state, whether or not it changed.
    #[must_use]
    pub fn monster_states(&self) -> Vec<MonsterState> {
        let mut keyed: Vec<(u64, EntityId)> = self.entities.iter().map(|(k, e)| (*k, *e)).collect();
        keyed.sort_unstable_by_key(|(key, _)| *key);
        keyed
            .into_iter()
            .filter_map(|(key, entity)| self.monster_state(key, entity))
            .collect()
    }

    /// The monsters that changed since the last call, and the keys of those that are gone.
    /// The first call returns every monster.
    pub fn changed_monsters(&mut self) -> (Vec<MonsterState>, Vec<u64>) {
        let current = self.monster_states();
        let tick = self.tick();
        let mut changed = Vec::new();
        for state in current {
            let differs = self
                .written
                .get(&state.key)
                .is_none_or(|(old, at)| old.differs(&state, seconds_between(*at, tick)));
            if differs {
                self.written.insert(state.key, (state.clone(), tick));
                changed.push(state);
            }
        }
        let gone: Vec<u64> = self
            .written
            .keys()
            .copied()
            .filter(|key| !self.entities.contains_key(key))
            .collect();
        for key in &gone {
            self.written.remove(key);
        }
        (changed, gone)
    }

    /// Every player's state, each with whether its combat row needs writing (see
    /// [`PlayerState::combat_differs`]). The first call marks every player.
    pub fn player_updates(&mut self) -> Vec<(PlayerState, bool)> {
        let tick = self.tick();
        let states = self.player_states();
        states
            .into_iter()
            .map(|state| {
                let differs = self
                    .written_players
                    .get(&state.account_id)
                    .is_none_or(|(old, at)| state.combat_differs(old, seconds_between(*at, tick)));
                if differs {
                    self.written_players
                        .insert(state.account_id.clone(), (state.clone(), tick));
                }
                (state, differs)
            })
            .collect()
    }

    /// Forgets what was written, so the next diff returns everything.
    pub fn forget_written(&mut self) {
        self.written.clear();
        self.written_players.clear();
    }

    /// The ability `index` names, for logging.
    #[must_use]
    pub fn ability_name(index: u16) -> &'static str {
        FireAbility::ALL
            .get(usize::from(index))
            .map_or("?", |ability| ability.definition().name)
    }
}

/// Seconds of fight time from tick `from` to tick `to`.
fn seconds_between(from: u64, to: u64) -> f32 {
    to.saturating_sub(from) as f32 / 60.0
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests;
