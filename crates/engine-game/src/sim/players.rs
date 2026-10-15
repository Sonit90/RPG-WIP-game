//! More than one player in one simulation.
//!
//! ```text
//! Simulation (active player's state in its own fields)   others: [Some(slot B), Some(slot C)]
//!        │                                                          │
//!        └──── exchange(slot) ◄─────────────────────────────────────┘
//!              the active fields and a slot trade places, field by field
//! ```
//!
//! Everything the game knows about "the player" — the cast, the target, the class state, the
//! character sheet, the respawn clock, the movement intent, the meter — was written for one
//! player and lives in [`Simulation`]'s own fields. Rather than turn every one of the ~150
//! places that read them into a lookup, a second player's state is kept in a [`PlayerSlot`] and
//! *swapped in* whenever that player is the one being simulated: for their per-player phases
//! each tick, and whenever damage or healing lands on them or comes from them. Code that reads
//! `self.player` reads whichever player is active, which is exactly what it meant.
//!
//! A slot is held as `Option` and put back — holding the *previously active* player — while
//! another player is swapped in, so a nested swap (player B's cast lands, an enemy's reflex
//! hits player A) still finds A. With no other players, nothing here runs and the
//! simulation is exactly the single-player one it always was.

use engine_collision::controller::MoveResult;
use engine_core::math::Vec3;

use crate::character::Roster;
use crate::character::spirit::SpiritSurge;
use crate::command::{CommandBuffer, PlayerCommand};
use crate::druid::Druid;
use crate::entity::EntityId;
use crate::firemage::{FireMage, PendingImpact};
use crate::hero::Hero;
use crate::movement::MoveIntent;
use crate::telemetry::CombatMeter;
use crate::warrior::Warrior;

use super::firemage_exec::FireCounters;
use super::{Player, Simulation, respawn};

/// One player's state while another player is active.
#[derive(Debug)]
pub struct PlayerSlot {
    player: Player,
    player_spawn: Option<(Vec3, f32)>,
    respawn: respawn::Respawn,
    void_seconds: f32,
    intent: MoveIntent,
    pending: CommandBuffer,
    hero: Hero,
    roster: Roster,
    spirit: SpiritSurge,
    mage: FireMage,
    stat_bonus: crate::character::StatBlock,
    last_defense: Option<crate::character::DefenseOutcome>,
    warrior: Warrior,
    druid: Druid,
    warrior_talents: crate::talent::TalentGrid,
    warrior_effects: crate::talent::warrior::WarriorTalentEffects,
    converting: bool,
    fire: FireCounters,
    impacts: Vec<PendingImpact>,
    meter: CombatMeter,
    regen_elapsed: f32,
    health_override: Option<f32>,
    effect_stat_bonus: crate::character::StatBlock,
    last_move: MoveResult,
    trinket: crate::trinket::TrinketState,
}

impl PlayerSlot {
    fn new(entity: EntityId, hero: Hero, seed: u64, spawn: Option<(Vec3, f32)>) -> Self {
        Self {
            player: Player {
                entity,
                target: None,
                cast: None,
                queued: None,
                last_error: None,
                last_error_at: 0.0,
                last_ability: None,
            },
            player_spawn: spawn,
            respawn: respawn::Respawn::new(),
            void_seconds: 0.0,
            intent: MoveIntent::NONE,
            pending: CommandBuffer::with_capacity(16),
            hero,
            roster: Roster::starting(),
            spirit: SpiritSurge::new(),
            mage: FireMage::new(seed),
            stat_bonus: crate::character::StatBlock::zero(),
            last_defense: None,
            warrior: Warrior::new(),
            druid: Druid::new(),
            warrior_talents: crate::talent::TalentGrid::new(Hero::Warrior),
            warrior_effects: crate::talent::warrior::WarriorTalentEffects::default(),
            converting: false,
            fire: FireCounters::default(),
            impacts: Vec::with_capacity(8),
            meter: CombatMeter::new(),
            regen_elapsed: 0.0,
            health_override: None,
            effect_stat_bonus: crate::character::StatBlock::default(),
            last_move: MoveResult::default(),
            trinket: crate::trinket::TrinketState::new(),
        }
    }

    /// The entity this slot's player controls.
    #[must_use]
    pub fn entity(&self) -> EntityId {
        self.player.entity
    }
}

impl Simulation {
    /// Trades the active player's state with `slot`'s, field by field.
    fn exchange(&mut self, slot: &mut PlayerSlot) {
        use std::mem::swap;
        swap(&mut self.player, &mut slot.player);
        swap(&mut self.player_spawn, &mut slot.player_spawn);
        swap(&mut self.respawn, &mut slot.respawn);
        swap(&mut self.void_seconds, &mut slot.void_seconds);
        swap(&mut self.intent, &mut slot.intent);
        swap(&mut self.pending, &mut slot.pending);
        swap(&mut self.hero, &mut slot.hero);
        swap(&mut self.roster, &mut slot.roster);
        swap(&mut self.spirit, &mut slot.spirit);
        swap(&mut self.mage, &mut slot.mage);
        swap(&mut self.stat_bonus, &mut slot.stat_bonus);
        swap(&mut self.last_defense, &mut slot.last_defense);
        swap(&mut self.warrior, &mut slot.warrior);
        swap(&mut self.druid, &mut slot.druid);
        swap(&mut self.warrior_talents, &mut slot.warrior_talents);
        swap(&mut self.warrior_effects, &mut slot.warrior_effects);
        swap(&mut self.converting, &mut slot.converting);
        swap(&mut self.fire, &mut slot.fire);
        swap(&mut self.impacts, &mut slot.impacts);
        swap(&mut self.meter, &mut slot.meter);
        swap(&mut self.regen_elapsed, &mut slot.regen_elapsed);
        swap(&mut self.health_override, &mut slot.health_override);
        swap(&mut self.effect_stat_bonus, &mut slot.effect_stat_bonus);
        swap(&mut self.last_move, &mut slot.last_move);
        swap(&mut self.trinket, &mut slot.trinket);
    }

    /// Runs `body` with the player at `index` in `others` active, then restores the active
    /// player. While `body` runs, the previously active player sits in `others[index]`, so a
    /// nested swap can still reach them.
    fn with_slot<R>(&mut self, index: usize, body: impl FnOnce(&mut Self) -> R) -> Option<R> {
        let mut slot = self.others.get_mut(index)?.take()?;
        self.exchange(&mut slot);
        self.others[index] = Some(slot);
        let result = body(self);
        let mut slot = self.others[index].take()?;
        self.exchange(&mut slot);
        self.others[index] = Some(slot);
        Some(result)
    }

    /// Adds another player, controlling `entity`, as `hero`. Their character sheet decides
    /// their health; they join the party; they respawn at the level's spawn point.
    ///
    /// The first player is the one [`Simulation::set_player_entity`] names; every other one
    /// comes through here.
    pub fn add_player(&mut self, entity: EntityId, hero: Hero) {
        if self.is_player_entity(entity) {
            return;
        }
        let index = self.others.len();
        let seed = self
            .config
            .rng_seed
            .wrapping_add(0x9E37_79B9_7F4A_7C15_u64.wrapping_mul(index as u64 + 1));
        self.others
            .push(Some(PlayerSlot::new(entity, hero, seed, self.player_spawn)));
        self.party.join(entity);
        let _ = self.with_slot(index, Simulation::refresh_character);
    }

    /// Takes a player out of the simulation: their state, their body and every trace the rest
    /// of the fight holds of them. Returns whether anyone was removed.
    ///
    /// Any player can go, the active one included: the first slot is *promoted* by trading its
    /// state into the active fields (see [`Simulation::exchange`]), which leaves the leaver's
    /// state in the slot, and the slot is dropped. The last player cannot be removed - the
    /// simulation always has an active one - and `false` says so.
    ///
    /// What goes with them: the party seat, external movement, a beam they channelled, their
    /// seeds, the ground and delayed zones they laid, every enemy's threat row and target for
    /// them, every player's selection of them - enemies re-choose among whoever is left and a
    /// wipe is judged on the remaining players only. Their body is despawned.
    ///
    /// Call it between ticks and from outside [`Simulation::with_player`]: while a player is
    /// swapped in, the slots hold other players' state.
    pub fn remove_player(&mut self, entity: EntityId) -> bool {
        if self.player_count() < 2 || !self.is_player_entity(entity) {
            return false;
        }
        // A channel is announced ended while its owner is still known.
        let _ = self.with_player(entity, |sim| sim.end_beam(crate::firemage::BeamEnd::Reset));
        if self.player.entity == entity {
            // Promote the first other player; the slot now holds the leaver and is dropped.
            let Some(mut slot) = self.others.remove(0) else {
                return false;
            };
            self.exchange(&mut slot);
        } else if let Some(index) = self.slot_index(entity) {
            self.others.remove(index);
        }
        self.party.leave(entity);
        self.externally_moved.retain(|e| *e != entity);
        self.seeds
            .retain(|seed| seed.owner != entity && seed.target != entity);
        self.clear_hazards_of(entity);
        self.clear_strikes_of(entity);
        for runtime in self.enemies.iter_mut().flatten() {
            runtime.brain.forget(entity);
        }
        self.world.despawn(entity);
        self.forget_target_everywhere(entity);
        true
    }

    fn slot_index(&self, entity: EntityId) -> Option<usize> {
        self.others.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|slot| slot.player.entity == entity)
        })
    }

    /// Whether `entity` is controlled by a player: the active one or any other.
    #[must_use]
    pub fn is_player_entity(&self, entity: EntityId) -> bool {
        self.player.entity == entity || self.slot_index(entity).is_some()
    }

    /// Every player's entity, the active one first.
    #[must_use]
    pub fn player_entities(&self) -> Vec<EntityId> {
        let mut entities = Vec::with_capacity(self.others.len() + 1);
        entities.push(self.player.entity);
        entities.extend(self.others.iter().flatten().map(PlayerSlot::entity));
        entities
    }

    /// How many players the simulation has.
    #[must_use]
    pub fn player_count(&self) -> usize {
        1 + self.others.len()
    }

    /// Runs `body` with the player controlling `entity` active. `None` if no player does.
    pub fn with_player<R>(
        &mut self,
        entity: EntityId,
        body: impl FnOnce(&mut Self) -> R,
    ) -> Option<R> {
        if self.player.entity == entity {
            return Some(body(self));
        }
        let index = self.slot_index(entity)?;
        self.with_slot(index, body)
    }

    /// How much the player controlling `entity` absorbs while another player is active.
    pub(super) fn inactive_absorb(&self, entity: EntityId) -> f32 {
        self.others
            .iter()
            .flatten()
            .find(|slot| slot.entity() == entity)
            .map_or(0.0, |slot| {
                slot.warrior.absorb.remaining() + slot.trinket.absorb.remaining()
            })
    }

    /// Runs `body` once with each player active, the active one first.
    pub(super) fn for_each_player(&mut self, mut body: impl FnMut(&mut Self)) {
        body(self);
        for index in 0..self.others.len() {
            let _ = self.with_slot(index, &mut body);
        }
    }

    /// Queues a command for the player controlling `entity`. Returns false if no player does.
    pub fn push_command_for(&mut self, entity: EntityId, command: PlayerCommand) -> bool {
        self.with_player(entity, |sim| sim.push_command(command))
            .is_some()
    }

    /// Has `entity`'s body moved from outside the tick rather than by its intent. A server
    /// does this for networked players: each input frame is applied as it arrives, with the
    /// same function the client predicted it with, so a frame is never applied twice or
    /// stretched over a tick it did not belong to.
    pub fn set_external_movement(&mut self, entity: EntityId, external: bool) {
        self.externally_moved.retain(|e| *e != entity);
        if external {
            self.externally_moved.push(entity);
        }
    }

    /// Whether every player is dead: what a party wipe is.
    #[must_use]
    pub fn all_players_dead(&self) -> bool {
        !self.world.is_alive(self.player.entity)
            && self
                .others
                .iter()
                .flatten()
                .all(|slot| !self.world.is_alive(slot.player.entity))
    }

    /// The player who should be active while `source` deals damage to or heals `target`: the
    /// target if a player is being hit, else the source if a player is acting. `None` when
    /// neither is a player other than the active one.
    pub(super) fn context_player(&self, source: EntityId, target: EntityId) -> Option<EntityId> {
        if target == self.player.entity
            || source == self.player.entity && self.slot_index(target).is_none()
        {
            return None;
        }
        if self.slot_index(target).is_some() {
            return Some(target);
        }
        if self.slot_index(source).is_some() {
            return Some(source);
        }
        None
    }

    /// Clears `entity` from every player's target.
    pub(super) fn forget_target_everywhere(&mut self, entity: EntityId) {
        if self.player.target == Some(entity) {
            self.player.target = None;
        }
        for slot in self.others.iter_mut().flatten() {
            if slot.player.target == Some(entity) {
                slot.player.target = None;
            }
        }
    }
}
