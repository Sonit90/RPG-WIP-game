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

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::enemy::EnemyKind;

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
        assert_eq!(simulation.world().len(), 1 + enemies);
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
