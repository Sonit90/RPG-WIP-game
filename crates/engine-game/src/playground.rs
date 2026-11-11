//! The Monster Combat Playground: choose monsters, stand them in front of the player, fight
//! them with everything the game has, take them away, choose again.
//!
//! ```text
//! MonsterPlayground
//!     ├── MonsterLibrary        every monster the game registers, read from its table
//!     ├── MonsterSelection      which of them the next spawn brings
//!     ├── PlaygroundEncounter   what the playground has standing in the world, by entity
//!     ├── formation()           where a group of them stands
//!     ├── HealthMode            the player's test or normal health
//!     └── effect target         who effects from the global registry are tried on
//! ```
//!
//! ## An orchestration layer, not a second game
//!
//! Nothing here fights. A spawned monster is [`Simulation::spawn_enemy_with`]'s monster — the
//! same brain, the same abilities, the same threat table and the same death a dungeon pack
//! has — and from the moment it stands up the simulation alone decides what it does. Taking
//! the encounter away is [`Simulation::dismiss_enemies`]; resetting the player is
//! [`Simulation::reset_player_state`]; test health is
//! [`Simulation::set_player_health_override`]. What this module owns is *which* entities are
//! the playground's, where they are put, and what the tester has chosen — state the
//! simulation has no reason to know.
//!
//! The library is discovered from [`EnemyKind::ALL`], not listed: a monster added to the
//! table is in the playground the moment it compiles, abilities and all. The effects are
//! likewise [`StatusKind::ALL`], applied and removed through
//! [`Simulation::apply_effect`] and [`Simulation::remove_effect`] — the registry's own path.

use engine_core::math::Vec3;

use crate::enemy::brain::yaw_towards;
use crate::enemy::{EnemyAbility, EnemyArchetype, EnemyKind, MonsterRole, MonsterTier};
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::sim::Simulation;
use crate::sim::dungeon_exec::EnemySpawnOptions;
use crate::status::StatusKind;

/// The playground's numbers. Configuration rather than constants, so a scene or a command
/// line can choose them and nothing in gameplay hard-codes a test value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaygroundConfig {
    /// The maximum health [`HealthMode::Test`] gives the player.
    pub test_health: f32,
    /// The level spawned monsters are, which scales their health and damage exactly as a
    /// level placement's does.
    pub monster_level: u8,
    /// Metres between the player's edge and the nearest monster's edge.
    pub distance: f32,
    /// Metres kept clear between any two bodies in a formation, edge to edge.
    pub gap: f32,
    /// The widest a formation row may be, in monsters.
    pub max_columns: usize,
}

impl PlaygroundConfig {
    /// A million: a boss can be watched through every phase without the tester dying.
    pub const DEFAULT_TEST_HEALTH: f32 = 1_000_000.0;
}

impl Default for PlaygroundConfig {
    fn default() -> Self {
        Self {
            test_health: Self::DEFAULT_TEST_HEALTH,
            monster_level: 1,
            distance: 5.0,
            gap: 1.2,
            max_columns: 4,
        }
    }
}

/// One monster the playground offers.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryEntry {
    /// Which monster.
    pub kind: EnemyKind,
    /// Every ability it can use in any phase, each once, in the order the table first
    /// lists it.
    pub abilities: Vec<EnemyAbility>,
}

impl LibraryEntry {
    /// Everything the table says about it.
    #[must_use]
    pub fn archetype(&self) -> &'static EnemyArchetype {
        self.kind.archetype()
    }

    /// Its name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.archetype().name
    }

    /// What it does in a fight.
    #[must_use]
    pub fn role(&self) -> MonsterRole {
        self.archetype().role
    }

    /// Whether it is ordinary, an empowered add or a boss.
    #[must_use]
    pub fn tier(&self) -> MonsterTier {
        self.archetype().tier
    }

    /// Whether `query` finds it: every word of the query, ignoring case, somewhere in its
    /// name, its id, its role or its tier. An empty query finds everything.
    #[must_use]
    pub fn matches(&self, query: &str) -> bool {
        let archetype = self.archetype();
        query.split_whitespace().all(|word| {
            contains_ignoring_case(archetype.name, word)
                || contains_ignoring_case(archetype.id, word)
                || contains_ignoring_case(archetype.role.as_str(), word)
                || contains_ignoring_case(tier_name(archetype.tier), word)
        })
    }
}

/// The tier's name, for search and for a group heading.
#[must_use]
pub fn tier_name(tier: MonsterTier) -> &'static str {
    match tier {
        MonsterTier::Normal => "MONSTERS",
        MonsterTier::Empowered => "EMPOWERED",
        MonsterTier::Boss => "BOSSES",
    }
}

fn contains_ignoring_case(haystack: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

/// Every monster the game registers, grouped by tier.
///
/// Built once, from the table: ordinary monsters first, then the empowered adds, then the
/// bosses, each group in table order.
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterLibrary {
    entries: Vec<LibraryEntry>,
}

impl MonsterLibrary {
    /// Reads every [`EnemyKind`] out of the monster table.
    #[must_use]
    pub fn discover() -> Self {
        let mut entries: Vec<LibraryEntry> = EnemyKind::ALL
            .into_iter()
            .map(|kind| {
                let archetype = kind.archetype();
                let mut abilities: Vec<EnemyAbility> = Vec::new();
                for phase in 0..archetype.phase_count() {
                    for ability in archetype.phase_abilities(phase) {
                        if !abilities.contains(&ability) {
                            abilities.push(ability);
                        }
                    }
                }
                LibraryEntry { kind, abilities }
            })
            .collect();
        // Stable, so table order survives inside each group.
        entries.sort_by_key(|entry| tier_order(entry.tier()));
        Self { entries }
    }

    /// Every entry, in display order.
    #[must_use]
    pub fn entries(&self) -> &[LibraryEntry] {
        &self.entries
    }

    /// The entry for `kind`.
    #[must_use]
    pub fn get(&self, kind: EnemyKind) -> Option<&LibraryEntry> {
        self.entries.iter().find(|entry| entry.kind == kind)
    }

    /// How many monsters there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn tier_order(tier: MonsterTier) -> u8 {
    match tier {
        MonsterTier::Normal => 0,
        MonsterTier::Empowered => 1,
        MonsterTier::Boss => 2,
    }
}

/// Which monsters the next spawn brings, in the order they were chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonsterSelection {
    kinds: Vec<EnemyKind>,
}

impl MonsterSelection {
    /// Nothing chosen.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `kind`. Returns whether it was not already chosen.
    pub fn select(&mut self, kind: EnemyKind) -> bool {
        if self.contains(kind) {
            return false;
        }
        self.kinds.push(kind);
        true
    }

    /// Removes `kind`. Returns whether it was chosen.
    pub fn deselect(&mut self, kind: EnemyKind) -> bool {
        let before = self.kinds.len();
        self.kinds.retain(|chosen| *chosen != kind);
        self.kinds.len() != before
    }

    /// Chooses `kind` if it was not, and un-chooses it if it was. Returns whether it is
    /// chosen now.
    pub fn toggle(&mut self, kind: EnemyKind) -> bool {
        if self.deselect(kind) {
            false
        } else {
            self.kinds.push(kind);
            true
        }
    }

    /// Chooses nothing.
    pub fn clear(&mut self) {
        self.kinds.clear();
    }

    /// Whether `kind` is chosen.
    #[must_use]
    pub fn contains(&self, kind: EnemyKind) -> bool {
        self.kinds.contains(&kind)
    }

    /// How many are chosen.
    #[must_use]
    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    /// Whether nothing is chosen.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// The chosen monsters, in the order they were chosen.
    #[must_use]
    pub fn kinds(&self) -> &[EnemyKind] {
        &self.kinds
    }
}

/// Where one monster of a formation stands, and which way it faces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormationSlot {
    /// Its feet, at the height of the formation's origin.
    pub position: Vec3,
    /// Its facing: towards the origin.
    pub yaw: f32,
}

/// Where `radii.len()` bodies stand in front of something at `origin` looking along `yaw`.
///
/// Rows across the line of sight, nearest first; the first rows are full and the last takes
/// what is left, so three are two in front and one behind them, and six are two rows of
/// three. Each row is as deep as its widest member and each monster as wide as itself, with
/// `config.gap` clear between any two bodies and `config.distance` between the nearest row
/// and the `player_radius` around the origin. Everybody faces the origin.
///
/// Pure arithmetic on a flat plane: [`PlaygroundEncounter::spawn`] then puts each slot on
/// the ground and moves it if the level is in the way.
#[must_use]
pub fn formation(
    origin: Vec3,
    yaw: f32,
    player_radius: f32,
    radii: &[f32],
    config: &PlaygroundConfig,
) -> Vec<FormationSlot> {
    let count = radii.len();
    if count == 0 {
        return Vec::new();
    }
    let columns = (count as f32)
        .sqrt()
        .ceil()
        .clamp(1.0, config.max_columns.max(1) as f32) as usize;
    let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    let gap = config.gap.max(0.0);

    let mut slots = Vec::with_capacity(count);
    let mut depth = player_radius.max(0.0) + config.distance.max(0.0);
    let mut previous_half_depth = 0.0;
    for (row_index, row) in radii.chunks(columns).enumerate() {
        let half_depth = row.iter().copied().fold(0.0_f32, f32::max).max(0.0);
        depth += if row_index == 0 {
            half_depth
        } else {
            previous_half_depth + gap + half_depth
        };
        previous_half_depth = half_depth;
        let width: f32 = row.iter().map(|radius| radius.max(0.0) * 2.0).sum::<f32>()
            + gap * (row.len() - 1) as f32;
        let mut cursor = -width * 0.5;
        for radius in row {
            let radius = radius.max(0.0);
            let across = cursor + radius;
            cursor += radius * 2.0 + gap;
            let position = origin + forward * depth + right * across;
            slots.push(FormationSlot {
                position,
                yaw: yaw_towards(position, origin),
            });
        }
    }
    slots
}

/// One monster the playground put in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncounterMember {
    /// Its entity.
    pub entity: EntityId,
    /// What it is.
    pub kind: EnemyKind,
    /// Whether one of the encounter's own monsters called it, rather than the tester.
    pub summoned: bool,
}

/// What the playground has standing in the world.
///
/// Membership by entity, so "the playground's monsters" is never a guess from the world:
/// a dummy, a level's pack or anything else in the scene is not in it and is never touched
/// by a reset.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaygroundEncounter {
    members: Vec<EncounterMember>,
    /// Reused for the id list a reset hands the simulation.
    scratch: Vec<EntityId>,
}

impl PlaygroundEncounter {
    /// Nobody.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Everybody, in the order they were put in the world.
    #[must_use]
    pub fn members(&self) -> &[EncounterMember] {
        &self.members
    }

    /// How many members there are, alive or dead.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Whether `entity` is one of them.
    #[must_use]
    pub fn contains(&self, entity: EntityId) -> bool {
        self.members.iter().any(|member| member.entity == entity)
    }

    /// How many are alive.
    #[must_use]
    pub fn living(&self, simulation: &Simulation) -> usize {
        self.members
            .iter()
            .filter(|member| simulation.world().is_alive(member.entity))
            .count()
    }

    /// Stands every monster in `kinds` in front of `origin`, facing it, and adds them to
    /// the encounter. Returns how many were spawned.
    ///
    /// The formation is [`formation`]'s, with melee in the front rows and ranged behind
    /// them. Each slot is then put on walkable ground the player could walk to; one that
    /// is not — inside a wall, over a pit, behind cover — is moved outwards in rings until
    /// it is, never closer than `config.gap` to the player or to a monster already placed.
    /// A level with no ground to ask about keeps the formation at the origin's height.
    ///
    /// Passive monsters wait to be attacked; the rest notice the player at once, exactly as
    /// a pack does when it is walked into.
    pub fn spawn(
        &mut self,
        simulation: &mut Simulation,
        kinds: &[EnemyKind],
        origin: Vec3,
        yaw: f32,
        config: &PlaygroundConfig,
        passive: bool,
    ) -> usize {
        if kinds.is_empty() {
            return 0;
        }
        // Melee in front, ranged and support behind, the tester's order kept within each.
        let mut ordered = kinds.to_vec();
        ordered.sort_by_key(|kind| front_rank(kind.archetype().role));
        let radii: Vec<f32> = ordered.iter().map(|kind| kind.archetype().radius).collect();
        let player_radius = simulation
            .world()
            .radius(simulation.player().entity)
            .max(0.0);
        let slots = formation(origin, yaw, player_radius, &radii, config);

        let options = EnemySpawnOptions {
            level: config.monster_level.max(1),
            aggro_radius: passive.then_some(0.0),
            ..EnemySpawnOptions::default()
        };
        let mut placed: Vec<(Vec3, f32)> = Vec::with_capacity(ordered.len());
        let mut spawned = 0;
        for ((kind, slot), radius) in ordered.iter().zip(&slots).zip(&radii) {
            let position = settle(
                simulation,
                slot.position,
                *radius,
                origin,
                player_radius,
                &placed,
                config.gap,
            );
            placed.push((position, *radius));
            let facing = yaw_towards(position, origin);
            let entity = simulation.spawn_enemy_with(*kind, position, facing, &options);
            self.members.push(EncounterMember {
                entity,
                kind: *kind,
                summoned: false,
            });
            spawned += 1;
        }
        spawned
    }

    /// Adopts anything the encounter's own monsters called into the world, and forgets
    /// members that something else took out of it.
    ///
    /// Run once a frame with that frame's events. A boss's adds are then the playground's
    /// too, so a reset takes them with it. The forgetting is what keeps the encounter from
    /// holding an id for a slot the world has since given to somebody else.
    pub fn observe(&mut self, simulation: &Simulation, events: &[CombatEvent]) {
        for event in events {
            if let CombatEvent::AddSummoned { boss, add, .. } = *event
                && self.contains(boss)
                && !self.contains(add)
                && let Some(brain) = simulation.brain(add)
            {
                self.members.push(EncounterMember {
                    entity: add,
                    kind: brain.kind(),
                    summoned: true,
                });
            }
        }
        let world = simulation.world();
        self.members.retain(|member| world.contains(member.entity));
    }

    /// Kills every living member through the ordinary damage path, so each death plays.
    /// Returns how many died.
    pub fn kill(&self, simulation: &mut Simulation) -> usize {
        let mut killed = 0;
        for member in &self.members {
            if simulation.world().is_alive(member.entity) {
                simulation.slay(member.entity);
                killed += 1;
            }
        }
        killed
    }

    /// Takes every member out of the world with everything it left behind, and starts the
    /// meter again. Returns how many were taken.
    ///
    /// Safe whatever they are doing — see [`Simulation::dismiss_enemies`]. Nothing outside
    /// the encounter is touched.
    pub fn reset(&mut self, simulation: &mut Simulation) -> usize {
        if self.members.is_empty() {
            return 0;
        }
        self.scratch.clear();
        self.scratch
            .extend(self.members.iter().map(|member| member.entity));
        let dismissed = simulation.dismiss_enemies(&self.scratch);
        self.members.clear();
        self.scratch.clear();
        simulation.reset_combat_meter();
        dismissed
    }
}

/// Which formation rows a role prefers: lower is nearer the player.
fn front_rank(role: MonsterRole) -> u8 {
    match role {
        MonsterRole::Melee | MonsterRole::Elite | MonsterRole::Boss => 0,
        MonsterRole::Ranged | MonsterRole::Support => 1,
    }
}

/// How many rings of alternatives a blocked formation slot tries.
const SETTLE_RINGS: usize = 6;

/// How many alternatives one ring holds.
const SETTLE_RING_POINTS: usize = 8;

/// Where a monster of `radius` meant for `wanted` actually stands.
fn settle(
    simulation: &mut Simulation,
    wanted: Vec3,
    radius: f32,
    origin: Vec3,
    player_radius: f32,
    placed: &[(Vec3, f32)],
    gap: f32,
) -> Vec3 {
    let clear = |at: Vec3| {
        let flat = |a: Vec3, b: Vec3| Vec3::new(a.x - b.x, 0.0, a.z - b.z).length();
        flat(at, origin) >= player_radius + radius + gap - 1e-3
            && placed
                .iter()
                .all(|(other, other_radius)| flat(at, *other) >= other_radius + radius + gap - 1e-3)
    };
    let step = radius * 2.0 + gap;
    let mut found_ground = false;
    for ring in 0..=SETTLE_RINGS {
        let points = if ring == 0 { 1 } else { SETTLE_RING_POINTS };
        for point in 0..points {
            let angle = std::f32::consts::TAU * point as f32 / points as f32;
            let distance = step * ring as f32;
            let candidate = Vec3::new(
                wanted.x + angle.cos() * distance,
                origin.y,
                wanted.z + angle.sin() * distance,
            );
            if !clear(candidate) {
                continue;
            }
            let Some(ground) = simulation.standing_ground(candidate) else {
                continue;
            };
            found_ground = true;
            if simulation.walkable_between(origin, ground) {
                return ground;
            }
        }
    }
    if found_ground {
        log::warn!(
            "playground: no clear ground near ({:.1}, {:.1}); standing it where the formation put it",
            wanted.x,
            wanted.z
        );
    }
    Vec3::new(wanted.x, origin.y, wanted.z)
}

/// The player's health in the playground.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HealthMode {
    /// [`PlaygroundConfig::test_health`], held through gear changes and hero swaps.
    #[default]
    Test,
    /// What the character sheet gives them: the game's own number.
    Normal,
}

/// The whole playground: what is offered, what is chosen, what is standing in the world,
/// and how the player is set up.
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterPlayground {
    config: PlaygroundConfig,
    library: MonsterLibrary,
    selection: MonsterSelection,
    encounter: PlaygroundEncounter,
    health: HealthMode,
    passive: bool,
    /// Who the effect controls apply to. `None`, or somebody no longer alive, is the player.
    effect_target: Option<EntityId>,
}

impl Default for MonsterPlayground {
    fn default() -> Self {
        Self::new(PlaygroundConfig::default())
    }
}

impl MonsterPlayground {
    /// A playground with `config`, the library read, nothing chosen and test health.
    #[must_use]
    pub fn new(config: PlaygroundConfig) -> Self {
        Self {
            config,
            library: MonsterLibrary::discover(),
            selection: MonsterSelection::new(),
            encounter: PlaygroundEncounter::new(),
            health: HealthMode::Test,
            passive: false,
            effect_target: None,
        }
    }

    /// Sets the player up for the playground: applies the current health mode.
    pub fn install(&mut self, simulation: &mut Simulation) {
        self.set_health_mode(simulation, self.health);
    }

    /// Its numbers.
    #[must_use]
    pub fn config(&self) -> &PlaygroundConfig {
        &self.config
    }

    /// Every monster on offer.
    #[must_use]
    pub fn library(&self) -> &MonsterLibrary {
        &self.library
    }

    /// What is chosen.
    #[must_use]
    pub fn selection(&self) -> &MonsterSelection {
        &self.selection
    }

    /// What is chosen, to change.
    pub fn selection_mut(&mut self) -> &mut MonsterSelection {
        &mut self.selection
    }

    /// What is standing in the world.
    #[must_use]
    pub fn encounter(&self) -> &PlaygroundEncounter {
        &self.encounter
    }

    /// The player's health mode.
    #[must_use]
    pub fn health_mode(&self) -> HealthMode {
        self.health
    }

    /// Whether new spawns wait to be attacked.
    #[must_use]
    pub fn is_passive(&self) -> bool {
        self.passive
    }

    /// Makes new spawns wait to be attacked, or not.
    pub fn set_passive(&mut self, passive: bool) {
        self.passive = passive;
    }

    /// The level new spawns are.
    #[must_use]
    pub fn monster_level(&self) -> u8 {
        self.config.monster_level
    }

    /// Sets the level new spawns are, `1..=60`.
    pub fn set_monster_level(&mut self, level: u8) {
        self.config.monster_level = level.clamp(1, 60);
    }

    /// Replaces the current encounter with every chosen monster, in front of `origin`
    /// looking along `yaw`. The selection is kept. Returns how many were spawned.
    pub fn spawn_selected(&mut self, simulation: &mut Simulation, origin: Vec3, yaw: f32) -> usize {
        self.encounter.reset(simulation);
        let kinds = self.selection.kinds().to_vec();
        self.encounter
            .spawn(simulation, &kinds, origin, yaw, &self.config, self.passive)
    }

    /// Takes the current encounter away. The selection is kept. Returns how many went.
    pub fn reset_encounter(&mut self, simulation: &mut Simulation) -> usize {
        self.encounter.reset(simulation)
    }

    /// Kills every living monster of the encounter, so their deaths can be watched.
    pub fn kill_encounter(&self, simulation: &mut Simulation) -> usize {
        self.encounter.kill(simulation)
    }

    /// Gives the player test health or their own.
    ///
    /// Either way they are filled to the new maximum: switching to normal health puts them at
    /// the character sheet's full health, not at a million minus what they had lost.
    pub fn set_health_mode(&mut self, simulation: &mut Simulation, mode: HealthMode) {
        self.health = mode;
        simulation.set_player_health_override(match mode {
            HealthMode::Test => Some(self.config.test_health),
            HealthMode::Normal => None,
        });
    }

    /// Puts the player back to a fresh start: see [`Simulation::reset_player_state`]. The
    /// health mode is kept.
    pub fn reset_player(&mut self, simulation: &mut Simulation) {
        simulation.reset_player_state();
    }

    /// Follows this frame's events: see [`PlaygroundEncounter::observe`]. An effect target
    /// that died or left the world falls back to the player.
    pub fn observe(&mut self, simulation: &Simulation) {
        self.encounter.observe(simulation, simulation.events());
        if self
            .effect_target
            .is_some_and(|target| !simulation.world().is_alive(target))
        {
            self.effect_target = None;
        }
    }

    // ------------------------------------------------------------------------- effects

    /// Who the effect controls apply to: the chosen entity while it is alive, else the
    /// player.
    #[must_use]
    pub fn effect_target(&self, simulation: &Simulation) -> EntityId {
        self.effect_target
            .filter(|target| simulation.world().is_alive(*target))
            .unwrap_or(simulation.player().entity)
    }

    /// Makes `target` the one the effect controls apply to.
    pub fn set_effect_target(&mut self, target: EntityId) {
        self.effect_target = Some(target);
    }

    /// Everybody the effect controls can be pointed at, in the order they are offered: the
    /// player, the party, the arena's dummies, then every living monster of the encounter.
    /// `out` is cleared first and its storage reused.
    pub fn effect_targets(&self, simulation: &Simulation, out: &mut Vec<EffectTarget>) {
        out.clear();
        let world = simulation.world();
        let player = simulation.player().entity;
        out.push(EffectTarget {
            entity: player,
            role: TargetRole::Player,
        });
        for member in simulation.party().members() {
            if *member != player && world.is_alive(*member) {
                out.push(EffectTarget {
                    entity: *member,
                    role: TargetRole::Friendly,
                });
            }
        }
        for entity in world.entities() {
            if world.kind(entity) == Some(crate::world::EntityKind::Dummy) && world.is_alive(entity)
            {
                out.push(EffectTarget {
                    entity,
                    role: TargetRole::Dummy,
                });
            }
        }
        for member in self.encounter.members() {
            if world.is_alive(member.entity) {
                out.push(EffectTarget {
                    entity: member.entity,
                    role: TargetRole::Monster(member.kind),
                });
            }
        }
    }

    /// Applies `kind` to the effect target for `duration` seconds with `stacks` stacks,
    /// through the simulation's ordinary application. Returns whether it took.
    pub fn apply_effect(
        &self,
        simulation: &mut Simulation,
        kind: StatusKind,
        duration: f32,
        stacks: u8,
    ) -> bool {
        let target = self.effect_target(simulation);
        // Applied by nobody: the test bed is not a combatant, so nothing it applies credits
        // the player or a monster with damage, healing or threat.
        simulation.apply_effect(target, kind, duration, stacks, EntityId::INVALID)
    }

    /// Takes `kind` off the effect target through the simulation's ordinary removal.
    pub fn remove_effect(&self, simulation: &mut Simulation, kind: StatusKind) -> bool {
        let target = self.effect_target(simulation);
        simulation.remove_effect(target, kind)
    }

    /// Takes every buff (`buffs`) or every debuff off the effect target.
    pub fn clear_effects(&self, simulation: &mut Simulation, buffs: bool) -> usize {
        let target = self.effect_target(simulation);
        simulation.remove_effects_where(target, |kind| kind.is_harmful() != buffs)
    }

    /// Puts every applicable effect in the registry on the effect target at once, each
    /// with its own duration and the stackable ones at their full stack: the interface's
    /// stress test. Returns how many took.
    pub fn add_test_effects(&self, simulation: &mut Simulation) -> usize {
        // Spread from seconds to minutes, so they expire one after another while watched.
        const DURATIONS: [f32; 9] = [6.0, 12.0, 20.0, 30.0, 45.0, 60.0, 90.0, 120.0, 300.0];
        let mut applied = 0;
        for (index, kind) in StatusKind::ALL
            .into_iter()
            .filter(|kind| kind.definition().applicable)
            .enumerate()
        {
            let duration = DURATIONS[index % DURATIONS.len()];
            if self.apply_effect(simulation, kind, duration, kind.max_stacks()) {
                applied += 1;
            }
        }
        applied
    }
}

/// One entity the effect controls can be pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectTarget {
    /// Who.
    pub entity: EntityId,
    /// What they are to the player.
    pub role: TargetRole,
}

/// What an effect target is to the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetRole {
    /// The player themselves.
    Player,
    /// A friendly party member: what a healer dispels.
    Friendly,
    /// One of the arena's training dummies.
    Dummy,
    /// A monster of the encounter.
    Monster(EnemyKind),
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn the_library_holds_every_registered_monster_once_grouped_by_tier() {
        let library = MonsterLibrary::discover();
        assert_eq!(library.len(), EnemyKind::ALL.len());
        for kind in EnemyKind::ALL {
            let count = library
                .entries()
                .iter()
                .filter(|entry| entry.kind == kind)
                .count();
            assert_eq!(count, 1, "{kind:?}");
        }
        let tiers: Vec<u8> = library
            .entries()
            .iter()
            .map(|entry| tier_order(entry.tier()))
            .collect();
        assert!(tiers.windows(2).all(|pair| pair[0] <= pair[1]), "{tiers:?}");
    }

    #[test]
    fn a_library_entry_lists_its_own_abilities_from_every_phase_once() {
        let library = MonsterLibrary::discover();
        for entry in library.entries() {
            let archetype = entry.archetype();
            for ability in archetype.abilities() {
                assert!(entry.abilities.contains(&ability), "{:?}", entry.kind);
            }
            for phase in 0..archetype.phase_count() {
                for ability in archetype.phase_abilities(phase) {
                    assert!(entry.abilities.contains(&ability), "{:?}", entry.kind);
                }
            }
            let mut unique = entry.abilities.clone();
            unique.sort_by_key(|ability| ability.index());
            unique.dedup();
            assert_eq!(unique.len(), entry.abilities.len(), "{:?}", entry.kind);
        }
    }

    #[test]
    fn search_matches_name_id_role_and_tier_ignoring_case() {
        let library = MonsterLibrary::discover();
        let archer = library
            .get(EnemyKind::CryptArcher)
            .expect("the archer is registered");
        assert!(archer.matches(""));
        assert!(archer.matches("archer"));
        assert!(archer.matches("CRYPT arch"));
        assert!(archer.matches("ranged"));
        assert!(archer.matches("crypt_archer"));
        assert!(!archer.matches("brute"));
        let boss = library
            .get(EnemyKind::CryptSteward)
            .expect("the boss is registered");
        assert!(boss.matches("boss"));
    }
}
