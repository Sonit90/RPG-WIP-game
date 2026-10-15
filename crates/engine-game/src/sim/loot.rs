//! The boss's chest: dropped when the dungeon is done, opened once by each player.
//!
//! ```text
//! boss encounter completed ──► spawn_loot_chest ──► ChestSpawned   (falls CHEST_FALL_SECONDS)
//!                                   │ rolls each player's items now, from the loot generator
//!                                   ▼
//! PlayerCommand::OpenChest ──► landed? in range? not yet claimed?
//!                                   │ yes
//!                                   ├─► first opener: ChestOpened
//!                                   └─► each item into the bag ──► LootAwarded
//!                                       bag full               ──► mailbox, LootMailed
//!
//! chest cleared / mail_unclaimed_loot ──► every unclaimed roll ──► mailbox, LootMailed
//! PlayerCommand::TakeMail             ──► letter's item into the bag (MailTaken / MailBagFull)
//!
//! normal monster dies (local authority, loot rules set) ──► per player: MobLoot, MobCurrency
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
//! One chest, a share per player present when it landed: with [`LootRules`] set, that is
//! `equipment_per_completion` independent rolls from the boss's table each (plus, with a reward
//! bonus, a `bonus / 100` chance of one more); without, one uniform pick from the built-in pool.
//! Opening is per player: a second `OpenChest` from the same player does nothing, and one
//! player opening never takes another's items. The chest is shared in the world and personal
//! in its contents. A share is claimed whole, exactly once.
//!
//! # Monster drops
//!
//! Killing a normal monster rolls, for every player, a small chance of one equipment piece
//! ([`CombatEvent::MobLoot`]) and the currency drops ([`CombatEvent::MobCurrency`]). They are
//! events only: the realm that owns the character's persistence turns them into rows. Only a
//! simulation with local authority rolls them; a client presenting a server's fight receives
//! the server's events instead. The rolls use the simulation's own loot generator, never the
//! enemies', so a drop cannot change a fight.

use engine_core::math::Vec3;

use crate::character::{ItemId, ItemStack, MailReason, MailSender, TakeMailError, loot_pool};
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::hero::Hero;
use crate::loot::{self, LootRules};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChestClaim {
    /// Who the items were rolled for.
    pub player: EntityId,
    /// What they will receive, in roll order. Claimed together, exactly once.
    pub items: Vec<ItemStack>,
    /// Whether the items have been handed over (bag or mailbox).
    pub claimed: bool,
    /// The hero the player was when it was rolled. Unclaimed items are mailed to this
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

    /// Each player's rolled items and whether they have taken them.
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

    /// Every item rolled from the chest: `(player, stack, claimed)`, one entry per item, a
    /// player's items in roll order.
    ///
    /// What the realm reads at the end of a run to persist each player's items, including
    /// anything left unclaimed if that is the policy.
    pub fn chest_loot(&self) -> impl Iterator<Item = (EntityId, ItemStack, bool)> + '_ {
        self.chest
            .iter()
            .flat_map(|chest| chest.claims.iter())
            .flat_map(|claim| {
                claim
                    .items
                    .iter()
                    .map(|stack| (claim.player, *stack, claim.claimed))
            })
    }

    /// Hands the simulation the data-driven loot rules, the dungeon key level they are rolled
    /// at, and the run's reward bonus.
    ///
    /// Once set, the boss chest gives each player `equipment_per_completion` rolls from the
    /// boss's table, and normal monsters drop equipment and currency (local authority only).
    /// `reward_bonus_pct` (a later key modifier's `+25%`) raises rewards two ways, both
    /// per cent: every currency drop chance and gold amount is scaled by `1 + bonus/100`, and
    /// each player's chest share has a `bonus/100` chance (at most 1) of one extra equipment
    /// roll. The realm scales the run's resource payout with the same number
    /// (`engine_realm::rewards::RunRewards::with_bonus`).
    pub fn set_loot_rules(&mut self, rules: LootRules, key_level: u32, reward_bonus_pct: u32) {
        self.loot_rules = Some(rules);
        self.loot_key_level = key_level.max(1);
        self.loot_bonus_pct = reward_bonus_pct;
    }

    /// The loot rules, if a host set them.
    #[must_use]
    pub fn loot_rules(&self) -> Option<&LootRules> {
        self.loot_rules.as_ref()
    }

    /// Changes the run's reward bonus (per cent) without touching the rules or the key level.
    /// Takes effect for the next roll; what was already rolled is not redone.
    pub fn set_reward_bonus(&mut self, reward_bonus_pct: u32) {
        self.loot_bonus_pct = reward_bonus_pct;
    }

    /// The run's reward bonus, per cent. Zero unless a host set one.
    #[must_use]
    pub fn reward_bonus(&self) -> u32 {
        self.loot_bonus_pct
    }

    /// The key level the loot is rolled at.
    #[must_use]
    pub fn loot_key_level(&self) -> u32 {
        self.loot_key_level
    }

    /// Reseeds the loot generator, for a test that wants a different chest.
    pub fn reseed_loot(&mut self, seed: u64) {
        self.loot_rng = crate::firemage::Rng::new(seed);
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

    /// The stable id of the dungeon's boss (its enemy archetype id), if there is one.
    fn boss_id(&self) -> Option<&'static str> {
        let index = self.dungeon.as_ref()?.rules().boss_encounter?;
        let boss = self.encounters.get(index)?.members().first().copied()?;
        self.enemies
            .get(boss.index())?
            .as_ref()
            .map(|runtime| runtime.brain.kind().id())
    }

    /// Every player's chest share from the data-driven rules.
    fn roll_chest_shares(&mut self, rules: &LootRules) -> Vec<(EntityId, Vec<ItemStack>)> {
        let boss = self.boss_id();
        // The boss's own table; failing that (a development chest, a boss with no table) the
        // first table there is, then the monsters' table.
        let table = boss
            .and_then(|id| rules.boss_table(id))
            .or_else(|| rules.boss_tables.first().map(|entry| &entry.table))
            .unwrap_or(&rules.mob.table);
        let key_level = self.loot_key_level;
        let bonus = f32::from(u16::try_from(self.loot_bonus_pct).unwrap_or(u16::MAX)) / 100.0;
        let mut shares = Vec::with_capacity(self.player_count());
        for player in self.player_entities() {
            let mut rolls = rules.equipment_per_completion;
            if bonus > 0.0 && self.loot_rng.chance(bonus.min(1.0)) {
                rolls += 1;
            }
            let items = (0..rolls)
                .filter_map(|_| loot::roll_equipment(table, rules, key_level, &mut self.loot_rng))
                .map(|(item, props)| ItemStack::with_props(item, props))
                .collect();
            shares.push((player, items));
        }
        shares
    }

    /// Every player's chest share from the built-in pool: one uniform pick each, default props.
    /// What a simulation without [`LootRules`] rolls.
    fn roll_legacy_shares(&mut self) -> Vec<(EntityId, Vec<ItemStack>)> {
        let pool = loot_pool();
        if pool.is_empty() {
            return Vec::new();
        }
        // The enemies' generator, so the roll is a function of the seed and the fight and a
        // replay of either produces the same chest. Players in `player_entities` order
        // (the active player first, then joiners) so the order is stable too.
        let mut shares = Vec::with_capacity(self.player_count());
        for player in self.player_entities() {
            let roll = self.enemy_random();
            let index = ((roll * pool.len() as f32) as usize).min(pool.len() - 1);
            shares.push((player, vec![ItemStack::one(pool[index].id)]));
        }
        shares
    }

    /// Drops the chest at `position` and rolls every player's items.
    ///
    /// Called once, where the dungeon completes. A second call while a chest stands does
    /// nothing, so a re-entrant completion cannot reroll anyone's loot. A player whose rolls
    /// came up empty (a table with nothing in it) simply has no claim.
    pub(super) fn spawn_loot_chest(&mut self, position: Vec3) {
        if self.chest.is_some() {
            return;
        }
        let shares = match self.loot_rules.take() {
            Some(rules) => {
                let shares = self.roll_chest_shares(&rules);
                self.loot_rules = Some(rules);
                shares
            }
            None => self.roll_legacy_shares(),
        };
        let mut claims = Vec::with_capacity(shares.len());
        for (player, items) in shares {
            if items.is_empty() {
                continue;
            }
            let hero = self
                .with_player(player, |sim| sim.hero)
                .unwrap_or(self.hero);
            claims.push(ChestClaim {
                player,
                items,
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

    /// Takes the active player's items out of the chest, if the rules allow it.
    ///
    /// Silently refuses when there is no chest, it is still falling, the player is dead or
    /// out of range, or they have already taken theirs: a command is a request, and a
    /// request that cannot be honoured is not an error. A full bag is the one refusal the
    /// player needs to hear about: each item that does not fit is then mailed to the
    /// character that tried to take it ([`MailReason::BagFull`], [`CombatEvent::LootMailed`])
    /// and the claim is spent either way.
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
        let items = chest.claims[claim_index].items.clone();
        if first_open {
            self.events.push(CombatEvent::ChestOpened {
                chest: chest_id,
                by: player,
                tick,
            });
        }

        // The claim is spent whole, before anything is handed over: whatever happens below, a
        // second open finds nothing to take.
        if let Some(chest) = self.chest.as_mut() {
            chest.claims[claim_index].claimed = true;
        }
        let mut placed_any = false;
        for (index, stack) in items.into_iter().enumerate() {
            let index = u8::try_from(index).unwrap_or(u8::MAX);
            let placed = self.roster.sheet_mut(self.hero).inventory_mut().add(stack);
            if placed.is_none() {
                self.mail_to_active(stack, MailReason::BagFull);
                self.events.push(CombatEvent::LootMailed {
                    player,
                    item: stack.item,
                    props: stack.props,
                    index,
                    reason: MailReason::BagFull,
                    tick,
                });
                continue;
            }
            placed_any = true;
            self.events.push(CombatEvent::LootAwarded {
                player,
                item: stack.item,
                props: stack.props,
                index,
                tick,
            });
        }
        if placed_any {
            // The same refresh an item move runs; a bag-only change alters no stat today, but
            // anything derived from the sheet must never be a step behind it.
            self.refresh_character();
        }
    }

    /// Removes the chest. A reset or a cleared level leaves nothing to loot, so whatever
    /// nobody took is mailed first.
    pub(super) fn clear_loot_chest(&mut self) {
        self.mail_unclaimed_loot();
        self.chest = None;
    }

    /// **Mails every unclaimed roll to its player** ([`MailReason::Unclaimed`]) and marks it
    /// claimed, emitting [`CombatEvent::LootMailed`] for each item.
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
        let open: Vec<(usize, EntityId, Vec<ItemStack>, Hero)> = self
            .chest
            .iter()
            .flat_map(|chest| chest.claims.iter().enumerate())
            .filter(|(_, claim)| !claim.claimed)
            .map(|(index, claim)| (index, claim.player, claim.items.clone(), claim.hero))
            .collect();
        for (index, player, items, hero) in open {
            let stamp = self.mail_epoch + self.time;
            let sent = self.with_player(player, |sim| {
                for stack in &items {
                    sim.roster.sheet_mut(hero).mailbox_mut().send_stack(
                        *stack,
                        MailSender::DungeonChest,
                        MailReason::Unclaimed,
                        stamp,
                    );
                }
            });
            if sent.is_none() {
                log::warn!("loot: {player:?} is gone; their unclaimed items stay on the chest");
                continue;
            }
            if let Some(chest) = self.chest.as_mut() {
                chest.claims[index].claimed = true;
            }
            for (item_index, stack) in items.iter().enumerate() {
                self.events.push(CombatEvent::LootMailed {
                    player,
                    item: stack.item,
                    props: stack.props,
                    index: u8::try_from(item_index).unwrap_or(u8::MAX),
                    reason: MailReason::Unclaimed,
                    tick,
                });
            }
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

    /// Files `stack` in the active player's current character's mailbox.
    fn mail_to_active(&mut self, stack: ItemStack, reason: MailReason) {
        let stamp = self.mail_epoch + self.time;
        self.roster.sheet_mut(self.hero).mailbox_mut().send_stack(
            stack,
            MailSender::DungeonChest,
            reason,
            stamp,
        );
    }

    /// Rolls what a dead monster drops, for every player.
    ///
    /// A boss pays its gold; any other monster rolls the equipment chance, the currency chance
    /// (`elite` uses the elite one) and its gold. Nothing without [`LootRules`] and nothing
    /// under a remote authority, whose server rolls and sends the events. Every roll comes
    /// from the loot generator, in player order, so it is a function of the seed and the fight.
    pub(super) fn drop_kill_loot(&mut self, boss: bool, elite: bool) {
        if self.remote_authority {
            return;
        }
        let Some(rules) = self.loot_rules.take() else {
            return;
        };
        let (tick, key_level, bonus) = (self.tick, self.loot_key_level, self.loot_bonus_pct);
        for player in self.player_entities() {
            if boss {
                if let Some(gold) = loot::boss_gold(&rules, bonus) {
                    self.events.push(CombatEvent::MobCurrency {
                        player,
                        resource: gold.resource,
                        amount: gold.amount,
                        tick,
                    });
                }
                continue;
            }
            if self.loot_rng.chance(rules.mob.equipment_chance)
                && let Some((item, props)) =
                    loot::roll_equipment(&rules.mob.table, &rules, key_level, &mut self.loot_rng)
            {
                self.events.push(CombatEvent::MobLoot {
                    player,
                    item,
                    props,
                    tick,
                });
            }
            let drops = [
                loot::roll_mob_currency(&rules, elite, bonus, &mut self.loot_rng),
                loot::roll_mob_gold(&rules, bonus, &mut self.loot_rng),
            ];
            for drop in drops.into_iter().flatten() {
                self.events.push(CombatEvent::MobCurrency {
                    player,
                    resource: drop.resource,
                    amount: drop.amount,
                    tick,
                });
            }
        }
        self.loot_rules = Some(rules);
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
