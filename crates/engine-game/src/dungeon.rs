//! The Ashen Reliquary as gameplay: which pack stands where, and how to put it all back.
//!
//! ```text
//! engine_environment::dungeon::AREAS   ── where there is room ──┐
//!                                                               ▼
//!                                    PACKS  ──▶  Encounter (one per pack)
//!                                                               │
//!                                                     Simulation::add_encounter
//!                                                               │
//!                                                               ▼
//!                                                    Dungeon { progress, reset }
//! ```
//!
//! # What this owns and what it does not
//!
//! It owns the *content*: the composition of each pack, which room it stands in, and the
//! order the rooms come in. It owns no AI, no combat, no health and no telegraph — a pack is
//! an [`Encounter`], an encounter is a list of spawn points, and a spawn point is a kind and
//! a place. Everything that happens after the player walks into a room happens in
//! [`crate::enemy`] and [`crate::sim`], unchanged.
//!
//! That is the answer to "how is a new monster added without modifying the dungeon system":
//! a monster is a row in [`crate::enemy::ARCHETYPES`], and a pack that uses it is a
//! [`EnemyKind`] in a list here. Neither file knows anything about the other's contents.
//!
//! # Every pack exists from the moment the dungeon is built
//!
//! Nothing spawns while the player is walking. All nine encounters are populated up front and
//! their enemies stand at their spawn points being idle, so entering a room is a *pull*
//! rather than a spawn — which is what makes an accidental pull possible, and therefore what
//! makes careful play mean something.

use engine_core::math::Vec3;
use engine_environment::dungeon::{AREAS, DungeonArea, ashen_reliquary};

use crate::arena::{ArenaConfig, add_environment_collision, build_collision};
use crate::dungeon_run::DungeonRules;
use crate::encounter::{Encounter, EncounterState, EnemySpawn};
use crate::enemy::EnemyKind;
use crate::entity::EntityId;
use crate::sim::{SimConfig, Simulation};
use crate::world::EntityDesc;

/// One pack, as content.
///
/// A name, the room it belongs to, and where each of its members stands relative to that
/// room's centre. Offsets rather than world positions, so moving a room in the level moves
/// its pack with it and the two cannot disagree.
#[derive(Debug, Clone, Copy)]
pub struct PackDef {
    /// What it is called, for the readout and the design document.
    pub name: &'static str,
    /// Which room it stands in, indexing [`AREAS`].
    pub area: usize,
    /// Its members: a kind and an offset from the room's centre, in metres.
    pub members: &'static [(EnemyKind, f32, f32)],
}

impl PackDef {
    /// How many enemies it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether it holds nobody.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The room it stands in.
    #[must_use]
    pub fn area(&self) -> &'static DungeonArea {
        // A pack whose area index is out of range is a content error, and standing it in the
        // first room is a great deal more debuggable than not building the dungeon.
        AREAS.get(self.area).unwrap_or(&AREAS[0])
    }

    /// The encounter this pack becomes.
    #[must_use]
    pub fn encounter(&self) -> Encounter {
        let area = self.area();
        let spawns = self
            .members
            .iter()
            .map(|(kind, x, z)| {
                // Facing back towards the entrance, which is +Z: a pack that faced away would
                // read as scenery until it turned round.
                EnemySpawn::new(*kind, area.at(*x, *z)).facing(std::f32::consts::PI)
            })
            .collect();
        Encounter::with_spawns(self.name, spawns)
    }
}

/// How many packs there are, the boss included.
pub const PACK_COUNT: usize = 9;

/// Which pack is the boss.
pub const BOSS_PACK: usize = PACK_COUNT - 1;

/// Every pack in the dungeon, in the order the player meets them.
///
/// Compositions, not counts. Each one adds exactly one thing to what the player has to be
/// doing at once — see `docs/dungeon/README.md` for the axis each pack introduces.
pub static PACKS: [PackDef; PACK_COUNT] = [
    // Room 1, near corner. Two bodies and nothing else: this pack is about where the player
    // stands, and there is nothing else in the room to make that question harder.
    PackDef {
        name: "GATEHOUSE WATCH",
        area: 1,
        members: &[
            (EnemyKind::Melee, -11.0, 10.0),
            (EnemyKind::Melee, -6.0, 12.0),
        ],
    },
    // Room 1, far corner. A second threat range. The Adepts stand far enough apart that one
    // telegraph does not cover the answer to the other.
    PackDef {
        name: "GATE CROSSFIRE",
        area: 1,
        members: &[
            (EnemyKind::Melee, 10.0, -8.0),
            (EnemyKind::Caster, 4.0, -12.0),
            (EnemyKind::Caster, 13.0, -12.0),
        ],
    },
    // Room 2, near corner. The interrupt lesson, with two Brutes making it awkward rather
    // than free.
    PackDef {
        name: "BONE CHOIR",
        area: 2,
        members: &[
            (EnemyKind::Ritualist, -12.0, 13.0),
            (EnemyKind::Melee, -13.0, 8.0),
            (EnemyKind::Melee, -7.0, 9.0),
        ],
    },
    // Room 2, far corner. Target priority: the Warden stands behind the things it keeps
    // alive, so reaching it is a decision rather than an accident.
    PackDef {
        name: "WARDENS CIRCLE",
        area: 2,
        members: &[
            (EnemyKind::Warden, 12.0, -13.0),
            (EnemyKind::Melee, 8.0, -7.0),
            (EnemyKind::Caster, 13.0, -8.0),
        ],
    },
    // Room 3, near corner. Two Weavers, so the floor is never entirely safe and never
    // entirely lost.
    PackDef {
        name: "CINDER LOOM",
        area: 3,
        members: &[
            (EnemyKind::Weaver, -13.0, 11.0),
            (EnemyKind::Weaver, -6.0, 12.0),
        ],
    },
    // Room 3, far corner. Move and interrupt: the first pack that asks for both inside the
    // same window.
    PackDef {
        name: "WEAVE AND WORD",
        area: 3,
        members: &[
            (EnemyKind::Weaver, 12.0, -13.0),
            (EnemyKind::Ritualist, 6.0, -13.0),
            (EnemyKind::Melee, 10.0, -7.0),
        ],
    },
    // Room 4, near corner. The elite, with one Brute so it is not fought in a vacuum.
    PackDef {
        name: "VAULT SENTINEL",
        area: 4,
        members: &[
            (EnemyKind::Revenant, -11.0, 11.0),
            (EnemyKind::Melee, -6.0, 13.0),
        ],
    },
    // Room 4, far corner. Everything the dungeon has taught, in one pull.
    PackDef {
        name: "LAST RITE",
        area: 4,
        members: &[
            (EnemyKind::Warden, 13.0, -13.0),
            (EnemyKind::Ritualist, 6.0, -13.0),
            (EnemyKind::Weaver, 13.0, -7.0),
            (EnemyKind::Melee, 8.0, -7.0),
        ],
    },
    // Room 5. The boss, alone, in the middle of its arena.
    PackDef {
        name: "WARDEN OF THE ASHEN GATE",
        area: 5,
        members: &[(EnemyKind::Boss, 0.0, 0.0)],
    },
];

/// A dungeon run: which packs exist, where the player is, and what has been cleared.
///
/// Holds no entities and no state the simulation also holds. Everything it reports is read
/// back out of the simulation's encounters, so there is one answer to "is this pack dead"
/// rather than two that can disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dungeon {
    name: &'static str,
    /// How many times the whole dungeon has been reset.
    resets: u32,
}

impl Default for Dungeon {
    fn default() -> Self {
        Self::new()
    }
}

impl Dungeon {
    /// The Ashen Reliquary.
    #[must_use]
    pub fn new() -> Self {
        Self {
            name: "THE ASHEN RELIQUARY",
            resets: 0,
        }
    }

    /// Its name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// How many times it has been reset.
    #[must_use]
    pub fn resets(&self) -> u32 {
        self.resets
    }

    /// Its rooms.
    #[must_use]
    pub fn areas(&self) -> &'static [DungeonArea] {
        &AREAS
    }

    /// Its packs.
    #[must_use]
    pub fn packs(&self) -> &'static [PackDef] {
        &PACKS
    }

    /// Which room `position` is in, if it is in one.
    #[must_use]
    pub fn area_of(&self, position: Vec3) -> Option<usize> {
        AREAS.iter().position(|area| area.contains(position))
    }

    /// How many packs have been cleared.
    #[must_use]
    pub fn cleared(&self, simulation: &Simulation) -> usize {
        simulation
            .encounters()
            .iter()
            .filter(|encounter| encounter.state() == EncounterState::Completed)
            .count()
    }

    /// Whether every pack including the boss is dead.
    #[must_use]
    pub fn is_complete(&self, simulation: &Simulation) -> bool {
        self.cleared(simulation) == PACK_COUNT
    }

    /// Whether the boss is dead.
    #[must_use]
    pub fn boss_defeated(&self, simulation: &Simulation) -> bool {
        simulation
            .encounter_at(BOSS_PACK)
            .is_some_and(|encounter| encounter.state() == EncounterState::Completed)
    }

    /// The boss's entity, if the dungeon built one.
    #[must_use]
    pub fn boss(&self, simulation: &Simulation) -> Option<EntityId> {
        simulation
            .encounter_at(BOSS_PACK)
            .and_then(|encounter| encounter.members().first().copied())
    }

    /// The pack that is being fought right now, if one is.
    ///
    /// The first active one, which in a linear dungeon is the only one unless the player has
    /// pulled two rooms at once — in which case the first is the one they started.
    #[must_use]
    pub fn active_pack(&self, simulation: &Simulation) -> Option<usize> {
        simulation
            .encounters()
            .iter()
            .position(|encounter| encounter.state().is_active())
    }

    /// Puts the entire dungeon back: every pack, the boss, the player and the floor.
    ///
    /// Complete by construction rather than by listing: it runs the simulation's own
    /// whole-world reset, which restores every enemy, revives the player, puts out every
    /// hazard and finishes every encounter's reset. A field added to an enemy later is
    /// covered without this function changing.
    pub fn reset(&mut self, simulation: &mut Simulation) {
        simulation.reset_encounter();
        self.resets += 1;
        log::info!("dungeon '{}' reset ({})", self.name, self.resets);
    }

    /// Asks one pack to reset itself on the next tick, leaving the rest of the dungeon alone.
    pub fn reset_pack(&self, simulation: &mut Simulation, pack: usize) {
        simulation.request_encounter_reset(pack);
    }
}

/// Builds the dungeon: its level, its collision, its player and all nine encounters.
///
/// The whole of "load the dungeon". The scene supplies the layout, the collision and the
/// player's spawn; [`PACKS`] supplies who stands where. Adding a second dungeon is another
/// scene and another pack table, and no change to anything below this function.
#[must_use]
pub fn build_dungeon() -> (Simulation, Dungeon) {
    let scene = ashen_reliquary();
    // No dummies: a dungeon with training dummies in it is a training arena with corridors.
    let config = ArenaConfig {
        group_count: 0,
        ..ArenaConfig::from_environment(&scene, 0)
    };

    let mut simulation = Simulation::new(SimConfig {
        movement: config.movement(),
        ..SimConfig::default()
    });
    let player = simulation
        .world_mut()
        .spawn(EntityDesc::player(config.player_spawn));
    simulation.set_player_entity(player);
    // The one friendly body in the dungeon, standing in the entrance hall where nothing is
    // fighting: a healer with no ally to practise on has half a kit. It is not a pack member
    // and no encounter names it, so nothing here can pull it.
    let healing_dummy = crate::arena::spawn_healing_dummy(
        simulation.world_mut(),
        AREAS[0].at(6.0, 0.0),
        config.healing_dummy_health,
    );
    simulation.party_mut().join(healing_dummy);

    let world = simulation.collision_world_mut();
    // The outer bound and the floor first, then every wall, pillar and arch the level brought.
    // The outer bound is far outside the dungeon: the dungeon's own walls are what confine
    // the player, and an invisible wall the player can reach is a wall they cannot see.
    build_collision(world, &config);
    let statics = add_environment_collision(world, &scene);
    world.rebuild_broad_phase();

    for pack in &PACKS {
        simulation.add_encounter(pack.encounter());
    }

    log::info!(
        "dungeon '{}': {} objects, {statics} collision shapes, {} packs, {} enemies",
        scene.name,
        scene.len(),
        PACK_COUNT,
        PACKS.iter().map(PackDef::len).sum::<usize>(),
    );
    (simulation, Dungeon::new())
}

/// The dungeon's level, for a caller that has to draw it.
///
/// Presentation needs the same scene gameplay was built from, and building it twice is how
/// the picture and the collision end up describing two different rooms. Both come from this
/// one deterministic function.
#[must_use]
pub fn dungeon_scene() -> engine_environment::EnvironmentScene {
    ashen_reliquary()
}

// ------------------------------------------------------------------------------------------
// Dungeons authored as levels
// ------------------------------------------------------------------------------------------

/// How a dungeon's monsters are scaled for the party that runs it.
///
/// The tables are tuned for [`DungeonScaling::DESIGNED_PARTY`]: a tank, a damage dealer and
/// a healer. The game has one player today, so a smaller party is how a solo developer walks
/// the same dungeon with the same mechanics and less of every number — every mechanic, every
/// telegraph and every timing is unchanged, because none of those are what scales.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DungeonScaling {
    /// How many players the dungeon is being run by.
    pub party_size: u8,
    /// What a dungeon key multiplies monster health by on top of the party size. 1.0 without one.
    pub key_health: f32,
    /// What a dungeon key multiplies monster damage by on top of the party size. 1.0 without one.
    pub key_damage: f32,
}

impl Default for DungeonScaling {
    fn default() -> Self {
        Self::for_party(Self::DESIGNED_PARTY)
    }
}

impl DungeonScaling {
    /// The party the numbers are tuned for: a tank, a damage dealer, a healer.
    pub const DESIGNED_PARTY: u8 = 3;

    /// Scaling for a party of `size`, clamped to one to five.
    #[must_use]
    pub fn for_party(size: u8) -> Self {
        Self {
            party_size: size.clamp(1, 5),
            key_health: 1.0,
            key_damage: 1.0,
        }
    }

    /// The same scaling with a dungeon key's multipliers applied on top.
    #[must_use]
    pub fn with_key(mut self, health_mult: f32, damage_mult: f32) -> Self {
        self.key_health = health_mult;
        self.key_damage = damage_mult;
        self
    }

    /// What monster health is multiplied by.
    ///
    /// Health scales roughly with the damage the party brings. A solo player brings a little
    /// under half of a party's, because a party's tank and healer deal little.
    #[must_use]
    pub fn health(&self) -> f32 {
        let party = match self.party_size {
            1 => 0.45,
            2 => 0.7,
            3 => 1.0,
            4 => 1.25,
            _ => 1.5,
        };
        party * self.key_health
    }

    /// What monster damage is multiplied by.
    ///
    /// Only below the designed party: a solo player has no healer and takes every hit a tank
    /// would have taken, so the hits are smaller — but no smaller than the point where
    /// standing in a telegraph stops mattering.
    #[must_use]
    pub fn damage(&self) -> f32 {
        let party = match self.party_size {
            1 => 0.6,
            2 => 0.8,
            _ => 1.0,
        };
        party * self.key_damage
    }
}

/// Everything a level placement says about a monster beyond kind and place.
///
/// `Copy`, so an [`EnemySpawn`] stays a small value; the author's name for the monster is
/// applied separately, after it is spawned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnDetail {
    /// Its level.
    pub level: u8,
    /// What its health is multiplied by on top of its level.
    pub health_scale: f32,
    /// What its damage is multiplied by on top of its level.
    pub damage_scale: f32,
    /// Its aggro radius, when it is not the monster's own.
    pub aggro_radius: Option<f32>,
    /// What killing it is worth towards completion, when it is not the monster's own.
    pub completion: Option<f32>,
    /// Uniform size multiplier.
    pub size: f32,
}

impl SpawnDetail {
    /// The spawn options this detail becomes.
    #[must_use]
    pub fn options(&self) -> crate::sim::dungeon_exec::EnemySpawnOptions {
        crate::sim::dungeon_exec::EnemySpawnOptions {
            level: self.level,
            health_scale: self.health_scale,
            damage_scale: self.damage_scale,
            aggro_radius: self.aggro_radius,
            completion: self.completion,
            size: self.size,
            name: None,
        }
    }
}

/// What building a level's monsters produced, for the log and for a test to assert on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PopulateReport {
    /// How many encounters the placements became.
    pub packs: usize,
    /// How many monsters were spawned.
    pub monsters: usize,
    /// How many placements were switched off.
    pub disabled: usize,
    /// Placements whose monster id the table does not know, by index and id.
    ///
    /// Reported rather than refused: a level naming a monster this build has never heard of
    /// is a level whose other hundred placements still work, and the log says which one is
    /// missing and where.
    pub unknown: Vec<(usize, String)>,
    /// Which encounter is the boss, if the level has one.
    pub boss_encounter: Option<usize>,
    /// The completion every placed monster together is worth.
    pub total_completion: f32,
}

/// Spawns a level's monsters, grouped into packs, and makes it a dungeon run if it says so.
///
/// Placements sharing a pack name become one encounter; a placement with no pack is a pack
/// of one. Every monster is scaled by its own level and by `scaling`. If the level carries
/// [`engine_environment::DungeonSettings`], the run is installed: completion, the sealed
/// boss, its gate and its adds.
pub fn populate_from_scene(
    simulation: &mut Simulation,
    scene: &engine_environment::EnvironmentScene,
    scaling: DungeonScaling,
) -> PopulateReport {
    let mut report = PopulateReport::default();
    // Packs in the order their first member appears in the level, so the encounter list is
    // stable across saves and a pack's index means the same thing on every load.
    let mut order: Vec<String> = Vec::new();
    let mut packs: Vec<Vec<usize>> = Vec::new();
    for (index, placement) in scene.monsters.iter().enumerate() {
        if !placement.enabled {
            report.disabled += 1;
            continue;
        }
        if EnemyKind::from_id(&placement.monster).is_none() {
            log::warn!(
                "level '{}': monsters[{index}] names '{}', which is not a monster",
                scene.name,
                placement.monster
            );
            report.unknown.push((index, placement.monster.clone()));
            continue;
        }
        let key = placement.pack_key(index);
        match order.iter().position(|existing| *existing == key) {
            Some(pack) => packs[pack].push(index),
            None => {
                order.push(key);
                packs.push(vec![index]);
            }
        }
    }

    let boss_pack = scene
        .dungeon
        .as_ref()
        .and_then(|settings| settings.boss_pack.clone());
    for (key, members) in order.iter().zip(&packs) {
        let spawns: Vec<EnemySpawn> = members
            .iter()
            .filter_map(|index| {
                let placement = &scene.monsters[*index];
                let kind = EnemyKind::from_id(&placement.monster)?;
                let position = Vec3::new(
                    placement.position.x,
                    placement
                        .position
                        .y
                        .max(scene.ground_at(placement.position)),
                    placement.position.z,
                );
                Some(
                    EnemySpawn::new(kind, position)
                        .facing(placement.yaw)
                        .detailed(SpawnDetail {
                            level: placement.level,
                            health_scale: scaling.health(),
                            damage_scale: scaling.damage(),
                            aggro_radius: placement.aggro_radius,
                            completion: placement.completion,
                            size: placement.scale,
                        }),
                )
            })
            .collect();
        let encounter_index = simulation.encounters().len();
        let is_boss = boss_pack.as_deref() == Some(key.as_str())
            || (boss_pack.is_none() && spawns.iter().any(|spawn| spawn.kind.is_boss()));
        simulation.add_encounter(Encounter::with_spawns(key.clone(), spawns));
        if is_boss && report.boss_encounter.is_none() {
            report.boss_encounter = Some(encounter_index);
        }
        // The author's names, now that there are entities to give them to.
        let spawned = simulation
            .encounter_at(encounter_index)
            .map(|encounter| encounter.members().to_vec())
            .unwrap_or_default();
        for (entity, index) in spawned.iter().zip(members) {
            if let Some(name) = scene.monsters[*index].name.clone() {
                simulation.set_enemy_name(*entity, name);
            }
            let patrol = &scene.monsters[*index].patrol;
            if !patrol.is_empty() {
                let waypoints = patrol
                    .iter()
                    .map(|[x, z]| Vec3::new(*x, scene.ground_at(Vec3::new(*x, 0.0, *z)), *z))
                    .collect();
                simulation.set_enemy_patrol(*entity, waypoints);
            }
            report.total_completion += simulation.completion_value(*entity);
        }
        report.monsters += spawned.len();
        report.packs += 1;
    }

    if let Some(settings) = &scene.dungeon {
        let pool: Vec<EnemyKind> = settings
            .add_pool
            .iter()
            .filter_map(|id| {
                let kind = EnemyKind::from_id(id);
                if kind.is_none() {
                    log::warn!(
                        "level '{}': add pool names unknown monster '{id}'",
                        scene.name
                    );
                }
                kind
            })
            .collect();
        simulation.set_dungeon(DungeonRules {
            name: settings.name.clone(),
            required: settings.required_completion.max(f32::EPSILON),
            boss_encounter: report.boss_encounter,
            gate: settings.boss_gate,
            add_spawns: settings
                .add_spawns
                .iter()
                .map(|spawn| (spawn.position, spawn.yaw))
                .collect(),
            add_count: usize::from(settings.add_count),
            add_pool: pool,
            allow_duplicate_adds: settings.allow_duplicate_adds,
        });
    }
    log::info!(
        "level '{}': {} monsters in {} packs ({} disabled, {} unknown), {:.0} completion placed",
        scene.name,
        report.monsters,
        report.packs,
        report.disabled,
        report.unknown.len(),
        report.total_completion
    );
    report
}

/// Where the Graveyard Crypt's level lives, under the assets root.
///
/// The crypt is *content*, not code: its walls, graves, lights, every monster placement and
/// its dungeon settings are one level file the editor opens, edits and saves. Nothing in
/// Rust describes its layout.
pub const GRAVEYARD_CRYPT_FILE: &str = "scenes/graveyard_crypt.json";

/// Builds a playable level from a level file: layout, collision, monsters, completion, boss.
///
/// The one path every authored level goes through —
/// [`crate::arena::environment_arena_for_party`] over the scene the file describes — so a
/// test, the game and the editor's "play" all build the same dungeon from the same bytes.
/// Returns the scene too, for the caller that has to draw it.
///
/// # Errors
///
/// The scene file's own error: the file is missing, is not JSON, or says something
/// impossible — each naming where.
pub fn build_level_file(
    path: impl AsRef<std::path::Path>,
    scaling: DungeonScaling,
) -> Result<(Simulation, engine_environment::EnvironmentScene), engine_environment::SceneFileError>
{
    let scene = engine_environment::scene_file::load(path)?;
    let simulation = crate::arena::environment_arena_for_party(&scene, 0, scaling);
    Ok((simulation, scene))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::enemy::EnemyKind;

    #[test]
    fn a_key_multiplies_health_and_damage_on_top_of_the_party() {
        let plain = DungeonScaling::for_party(1);
        assert!((plain.health() - 0.45).abs() < 1e-6);
        let keyed = plain.with_key(2.0, 1.5);
        assert!((keyed.health() - 0.9).abs() < 1e-6);
        assert!((keyed.damage() - 0.9).abs() < 1e-6);
        assert!((DungeonScaling::default().health() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_dungeon_has_the_content_the_design_says_it_has() {
        assert_eq!(PACKS.len(), 9);
        // Four to six monster types was the brief, and the boss is not one of them.
        let mut kinds: Vec<EnemyKind> = PACKS
            .iter()
            .flat_map(|pack| pack.members.iter().map(|(kind, _, _)| *kind))
            .filter(|kind| !kind.is_boss())
            .collect();
        kinds.sort_by_key(|kind| kind.index());
        kinds.dedup();
        assert_eq!(kinds.len(), 6, "{kinds:?}");
    }

    #[test]
    fn every_pack_stands_inside_the_room_it_belongs_to() {
        // The one way a pack can be wrong that no amount of playing finds quickly: a member
        // standing inside a wall, or in the corridor where it pulls the next room.
        for pack in &PACKS {
            let area = pack.area();
            for spawn in pack.encounter().spawns() {
                assert!(
                    area.contains(spawn.position),
                    "{} has a member outside {}: {:?}",
                    pack.name,
                    area.name,
                    spawn.position
                );
                // And not against a wall, where a body would be half inside it.
                let margin = 2.0;
                assert!(
                    (spawn.position.x - area.center.x).abs() <= area.half_x - margin
                        && (spawn.position.z - area.center.z).abs() <= area.half_z - margin,
                    "{} has a member in the wall of {}",
                    pack.name,
                    area.name
                );
            }
        }
    }

    #[test]
    fn the_packs_are_in_room_order_and_the_boss_is_last() {
        let mut previous = 0;
        for pack in &PACKS {
            assert!(pack.area >= previous, "{} goes backwards", pack.name);
            previous = pack.area;
        }
        assert_eq!(PACKS[BOSS_PACK].area, AREAS.len() - 1);
        assert_eq!(PACKS[BOSS_PACK].members.len(), 1);
        assert!(PACKS[BOSS_PACK].members[0].0.is_boss());
        // And nothing else is a boss.
        for pack in &PACKS[..BOSS_PACK] {
            assert!(pack.members.iter().all(|(kind, _, _)| !kind.is_boss()));
        }
    }

    #[test]
    fn two_packs_in_one_room_cannot_pull_each_other() {
        // Aggro radius is the largest any monster in the roster has. Two packs closer than
        // that are one pack, and the room they share is a room the player cannot fight in.
        for (index, outer) in PACKS.iter().enumerate() {
            for inner in PACKS.iter().skip(index + 1) {
                if outer.area != inner.area {
                    continue;
                }
                for a in outer.encounter().spawns() {
                    for b in inner.encounter().spawns() {
                        let reach = a
                            .kind
                            .archetype()
                            .aggro_radius
                            .max(b.kind.archetype().aggro_radius);
                        let gap = crate::world::horizontal_distance(a.position, b.position);
                        assert!(
                            gap > reach,
                            "{} and {} stand {gap:.1} m apart, inside a {reach:.1} m aggro radius",
                            outer.name,
                            inner.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn building_it_produces_a_player_every_pack_and_nothing_else() {
        let (simulation, dungeon) = build_dungeon();
        let enemies: usize = PACKS.iter().map(PackDef::len).sum();
        // The player, every pack, and the one friendly body in the entrance hall.
        assert_eq!(simulation.world().len(), 2 + enemies);
        assert_eq!(simulation.encounters().len(), PACK_COUNT);
        assert_eq!(dungeon.cleared(&simulation), 0);
        assert!(!dungeon.is_complete(&simulation));
        assert!(dungeon.boss(&simulation).is_some());
        assert!(
            !simulation.collision_world().is_dirty(),
            "the dungeon must not hand over a world that still has to be rebuilt"
        );
        // Every pack starts asleep.
        for encounter in simulation.encounters() {
            assert_eq!(encounter.state(), EncounterState::NotStarted);
        }
    }

    #[test]
    fn the_player_starts_in_the_first_room_and_out_of_everything_reach() {
        let (simulation, dungeon) = build_dungeon();
        let position = simulation.player_position();
        assert_eq!(dungeon.area_of(position), Some(0));
        for pack in &PACKS {
            for spawn in pack.encounter().spawns() {
                let gap = crate::world::horizontal_distance(position, spawn.position);
                assert!(
                    gap > spawn.kind.archetype().aggro_radius,
                    "{} notices the player before they have moved",
                    pack.name
                );
            }
        }
    }

    #[test]
    fn the_dungeon_is_built_identically_every_time() {
        let describe = || {
            let (simulation, _) = build_dungeon();
            simulation
                .world()
                .entities()
                .filter_map(|id| simulation.world().position(id))
                .collect::<Vec<_>>()
        };
        assert_eq!(describe(), describe());
    }
}
