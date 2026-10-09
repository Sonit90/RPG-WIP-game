//! The boss's chest: dropped when the dungeon is done, opened once by each player.
//!
//! ```text
//! boss encounter completed ──► spawn_loot_chest ──► ChestSpawned   (falls CHEST_FALL_SECONDS)
//!                                   │ rolls one item per player now, from the seeded rng
//!                                   ▼
//! PlayerCommand::OpenChest ──► landed? in range? not yet claimed?
//!                                   │ yes
//!                                   ├─► first opener: ChestOpened
//!                                   └─► item into the bag ──► LootAwarded
//!                                       bag full           ──► mailbox, LootMailed
//!
//! chest cleared / mail_unclaimed_loot ──► every unclaimed roll ──► mailbox, LootMailed
//! PlayerCommand::TakeMail             ──► letter's item into the bag (MailTaken / MailBagFull)
//! ```
//!
//! # Why the roll happens at spawn, not at opening
//!
//! The simulation is the authority: online it runs on the server, and the realm persists each
//! player's item when the run ends. Rolling when the chest *appears* makes the whole outcome a
//! function of the seed and the fight, independent of who opens first, who is standing where,
//! or whether somebody never opens it at all - an unopened share is still known, so it can be
//! persisted or discarded by policy rather than by accident. It also means every player's
//! loot is decided once, in one order, from one generator, and never re-rolled.
//!
//! # Personal loot
//!
//! One chest, one item per player present when it landed. Opening is per player: a second
//! `OpenChest` from the same player does nothing, and one player opening never takes
//! another's item. The chest is shared in the world and personal in its contents.

use engine_core::math::Vec3;

use crate::character::{ItemId, ItemStack, MailReason, MailSender, TakeMailError, loot_pool};
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::hero::Hero;

use super::{Simulation, horizontal_distance};

/// How high above its resting place the chest appears, in metres.
///
/// Exported so presentation animates exactly the fall the simulation gates opening on.
pub const CHEST_DROP_HEIGHT: f32 = 7.0;

/// How long the chest takes to land, in seconds. It cannot be opened until it has.
pub const CHEST_FALL_SECONDS: f32 = 1.1;

/// How close, horizontally in metres, a player must stand to open the chest.
pub const CHEST_INTERACT_RANGE: f32 = 4.0;

/// One player's share of a chest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChestClaim {
    /// Who the item was rolled for.
    pub player: EntityId,
    /// What they will receive.
    pub item: ItemId,
    /// Whether it has been put in their bag.
    pub claimed: bool,
    /// The hero the player was when it was rolled. An unclaimed item is mailed to this
    /// character, wherever the player has swapped to since.
    pub hero: Hero,
}

/// The chest a completed dungeon leaves behind.
#[derive(Debug, Clone, PartialEq)]
pub struct LootChest {
    id: u32,
    position: Vec3,
    spawned_tick: u64,
    spawned_time: f64,
    opened: bool,
    claims: Vec<ChestClaim>,
}

impl LootChest {
    /// Its identity, as carried by the chest events.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Where it rests, on the ground: the boss's feet at death. It *appears*
    /// [`CHEST_DROP_HEIGHT`] above this.
    #[must_use]
    pub fn position(&self) -> Vec3 {
        self.position
    }

    /// The tick it was spawned on.
    #[must_use]
    pub fn spawned_tick(&self) -> u64 {
        self.spawned_tick
    }

    /// Whether anyone has opened it yet. Drives the lid animation.
    #[must_use]
    pub fn opened(&self) -> bool {
        self.opened
    }

    /// Each player's rolled item and whether they have taken it.
    #[must_use]
    pub fn claims(&self) -> &[ChestClaim] {
        &self.claims
    }

    /// Whether it has finished falling at simulation time `now` (seconds).
    #[must_use]
    pub fn landed(&self, now: f64) -> bool {
        now - self.spawned_time >= f64::from(CHEST_FALL_SECONDS)
    }

    /// Seconds since it spawned at simulation time `now`; presentation derives the fall from
    /// this against [`CHEST_FALL_SECONDS`].
    #[must_use]
    pub fn age(&self, now: f64) -> f32 {
        (now - self.spawned_time).max(0.0) as f32
    }
}

impl Simulation {
    /// The chest the finished dungeon left, if there is one.
    #[must_use]
    pub fn loot_chest(&self) -> Option<&LootChest> {
        self.chest.as_ref()
    }

    /// Every player's roll from the chest: `(player, item, claimed)`.
    ///
    /// What the realm reads at the end of a run to persist each player's item, including
    /// anything left unclaimed if that is the policy.
    pub fn chest_loot(&self) -> impl Iterator<Item = (EntityId, ItemId, bool)> + '_ {
        self.chest
            .iter()
            .flat_map(|chest| chest.claims.iter())
            .map(|claim| (claim.player, claim.item, claim.claimed))
    }

    /// Whether `player` could open the chest right now: it has landed, they are alive and
    /// close enough, and they have not already taken their item. For an interact prompt.
    #[must_use]
    pub fn can_open_chest(&self, player: EntityId) -> bool {
        let Some(chest) = self.chest.as_ref() else {
            return false;
        };
        chest.landed(self.time)
            && self.world.is_alive(player)
            && self
                .world
                .position(player)
                .is_some_and(|at| horizontal_distance(at, chest.position) <= CHEST_INTERACT_RANGE)
            && chest
                .claims
                .iter()
                .any(|claim| claim.player == player && !claim.claimed)
    }

    /// Drops the chest at `position` and rolls every player's item.
    ///
    /// Called once, where the dungeon completes. A second call while a chest stands does
    /// nothing, so a re-entrant completion cannot reroll anyone's loot.
    pub(super) fn spawn_loot_chest(&mut self, position: Vec3) {
        if self.chest.is_some() {
            return;
        }
        let pool = loot_pool();
        if pool.is_empty() {
            return;
        }
        // The enemies' generator, so the roll is a function of the seed and the fight and a
        // replay of either produces the same chest. Players in `player_entities` order
        // (the active player first, then joiners) so the order is stable too.
        let mut claims = Vec::with_capacity(self.player_count());
        for player in self.player_entities() {
            let roll = self.enemy_random();
            let index = ((roll * pool.len() as f32) as usize).min(pool.len() - 1);
            let hero = self
                .with_player(player, |sim| sim.hero)
                .unwrap_or(self.hero);
            claims.push(ChestClaim {
                player,
                item: pool[index].id,
                claimed: false,
                hero,
            });
        }
        self.next_chest += 1;
        let id = self.next_chest;
        let tick = self.tick;
        self.chest = Some(LootChest {
            id,
            position,
            spawned_tick: tick,
            spawned_time: self.time,
            opened: false,
            claims,
        });
        self.events.push(CombatEvent::ChestSpawned {
            chest: id,
            position,
            drop_height: CHEST_DROP_HEIGHT,
            fall_seconds: CHEST_FALL_SECONDS,
            tick,
        });
        log::info!("loot chest {id} dropped");
    }

    /// Takes the active player's item out of the chest, if the rules allow it.
    ///
    /// Silently refuses when there is no chest, it is still falling, the player is dead or
    /// out of range, or they have already taken theirs: a command is a request, and a
    /// request that cannot be honoured is not an error. A full bag is the one refusal the
    /// player needs to hear about: the item is then mailed to the character that tried to take
    /// it ([`MailReason::BagFull`], [`CombatEvent::LootMailed`]) and the claim is spent.
    pub(super) fn open_chest(&mut self) {
        if self.remote_authority {
            return;
        }
        let player = self.player.entity;
        if !self.can_open_chest(player) {
            return;
        }
        let tick = self.tick;
        let Some(chest) = self.chest.as_mut() else {
            return;
        };
        let chest_id = chest.id;
        let first_open = !chest.opened;
        chest.opened = true;
        let Some(claim_index) = chest
            .claims
            .iter()
            .position(|claim| claim.player == player && !claim.claimed)
        else {
            return;
        };
        let item = chest.claims[claim_index].item;
        if first_open {
            self.events.push(CombatEvent::ChestOpened {
                chest: chest_id,
                by: player,
                tick,
            });
        }

        let placed = self
            .roster
            .sheet_mut(self.hero)
            .inventory_mut()
            .add(ItemStack::one(item));
        if let Some(chest) = self.chest.as_mut() {
            chest.claims[claim_index].claimed = true;
        }
        if placed.is_none() {
            self.mail_to_active(item, MailReason::BagFull);
            self.events.push(CombatEvent::LootMailed {
                player,
                item,
                reason: MailReason::BagFull,
                tick,
            });
            return;
        }
        self.events
            .push(CombatEvent::LootAwarded { player, item, tick });
        // The same refresh an item move runs; a bag-only change alters no stat today, but
        // anything derived from the sheet must never be a step behind it.
        self.refresh_character();
    }

    /// Removes the chest. A reset or a cleared level leaves nothing to loot, so whatever
    /// nobody took is mailed first.
    pub(super) fn clear_loot_chest(&mut self) {
        self.mail_unclaimed_loot();
        self.chest = None;
    }

    /// **Mails every unclaimed roll to its player** ([`MailReason::Unclaimed`]) and marks it
    /// claimed, emitting [`CombatEvent::LootMailed`] for each.
    ///
    /// Idempotent: a spent claim is never mailed twice, so calling this on every exit path is
    /// safe. The host calls it before saving on exit or leaving the scene; the chest-clearing
    /// paths call it themselves. Does nothing under a remote authority, whose server owns the
    /// chest (and mails through its own persistence, reading [`Simulation::chest_loot`]). A
    /// roll whose player has since left stays unclaimed rather than vanishing.
    pub fn mail_unclaimed_loot(&mut self) {
        if self.remote_authority {
            return;
        }
        let tick = self.tick;
        let open: Vec<(usize, EntityId, ItemId, Hero)> = self
            .chest
            .iter()
            .flat_map(|chest| chest.claims.iter().enumerate())
            .filter(|(_, claim)| !claim.claimed)
            .map(|(index, claim)| (index, claim.player, claim.item, claim.hero))
            .collect();
        for (index, player, item, hero) in open {
            let stamp = self.mail_epoch + self.time;
            let sent = self.with_player(player, |sim| {
                sim.roster.sheet_mut(hero).mailbox_mut().send(
                    item,
                    MailSender::DungeonChest,
                    MailReason::Unclaimed,
                    stamp,
                );
            });
            if sent.is_none() {
                log::warn!("loot: {player:?} is gone; their unclaimed item stays on the chest");
                continue;
            }
            if let Some(chest) = self.chest.as_mut() {
                chest.claims[index].claimed = true;
            }
            self.events.push(CombatEvent::LootMailed {
                player,
                item,
                reason: MailReason::Unclaimed,
                tick,
            });
        }
    }

    /// Sets the clock letters are stamped with: `epoch + simulation time`, in seconds.
    ///
    /// The offline host passes the unix time at launch so a letter carries a real date; left
    /// at zero, letters carry simulation time, which keeps a replay deterministic.
    pub fn set_mail_epoch(&mut self, epoch: f64) {
        self.mail_epoch = epoch;
    }

    /// Development: drops the loot chest at `position` exactly as a completed dungeon does
    /// (same roll, same events), for visual QA of the chest without clearing a dungeon.
    ///
    /// Like the completion path it does nothing while a chest already stands.
    pub fn spawn_loot_chest_at(&mut self, position: Vec3) {
        self.spawn_loot_chest(position);
    }

    /// Development: files `item` in the active player's mailbox as if it had arrived
    /// `age_seconds` ago, for visual QA of the mail window. Emits no event.
    pub fn mail_item_aged(&mut self, item: ItemId, reason: MailReason, age_seconds: f64) {
        if self.remote_authority {
            return;
        }
        let stamp = self.mail_epoch + self.time - age_seconds;
        self.roster.sheet_mut(self.hero).mailbox_mut().send(
            item,
            MailSender::DungeonChest,
            reason,
            stamp,
        );
    }

    /// The active player's letters, for the mail window.
    #[must_use]
    pub fn mailbox(&self) -> &crate::character::Mailbox {
        self.sheet().mailbox()
    }

    /// Files `item` in the active player's current character's mailbox.
    fn mail_to_active(&mut self, item: ItemId, reason: MailReason) {
        let stamp = self.mail_epoch + self.time;
        self.roster.sheet_mut(self.hero).mailbox_mut().send(
            item,
            MailSender::DungeonChest,
            reason,
            stamp,
        );
    }

    /// Moves a letter's item into the active player's bag.
    ///
    /// [`CombatEvent::MailTaken`] on success, [`CombatEvent::MailBagFull`] (letter kept) when
    /// there is no room; an unknown id does nothing. The server owns mail online, so a client
    /// under a remote authority does nothing.
    pub(super) fn take_mail(&mut self, id: u64) {
        if self.remote_authority {
            return;
        }
        let player = self.player.entity;
        let tick = self.tick;
        let Some(item) = self.sheet().mailbox().get(id).map(|letter| letter.item) else {
            return;
        };
        match self.roster.sheet_mut(self.hero).take_mail(id) {
            Ok(_) => {
                self.events
                    .push(CombatEvent::MailTaken { player, item, tick });
                self.refresh_character();
            }
            Err(TakeMailError::BagFull) => {
                self.events.push(CombatEvent::MailBagFull { player, tick });
            }
            Err(TakeMailError::NotFound) => {}
        }
    }

    /// Marks a letter in the active player's mailbox read.
    pub(super) fn mark_mail_read(&mut self, id: u64) {
        if self.remote_authority {
            return;
        }
        self.roster.sheet_mut(self.hero).mark_read(id);
    }
}
