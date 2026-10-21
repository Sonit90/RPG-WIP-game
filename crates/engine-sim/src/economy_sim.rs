//! The whole-economy simulation (`--economy-sim`): income, sinks and runs to best-in-slot.
//!
//! One simulated player does run after run through the shipped dungeons and spends what they
//! earn the way a sensible player would. Every roll is the game's own and every price is the
//! catalog's; nothing here restates a number.
//!
//! ```text
//! a run   30 ordinary + 6 elite kills   equipment chance, gold, gem  (roll_equipment, roll_mob_gold, roll_gem)
//!         the boss                      gold, gem
//!         the chest                     `equipment_per_completion` boss-table rolls, a gem, a book
//!         the dungeon's reward table    Crypt Dust at the key level            (rewards::run_rewards)
//! between runs, in this order
//!   1. gem upgrades   highest level first, any kind, `gems.json` price and chance
//!   2. sockets        a level-5 gem goes into the first free socket of an owned target piece (free)
//!   3. sharpening     owned target pieces up to the safe ceiling, Crypt Dust (shards -> dust first)
//!   4. reforge        the owned target piece with the lowest quality, while it is under the goal
//! ```
//!
//! # The player
//!
//! The target is the class's `p2_set` loadout (`gear.rs`): the major set's pieces plus the best
//! epic of every other slot, thirteen items. A drop of one of them is kept when it is new or
//! clearly better (quality plus sharpening, replacing a piece pays to take its gems out); every
//! other drop is salvaged for Bone Shards. Money is only ever spent on an owned target piece or
//! on gems, never on gear that is about to be replaced, so gold piles up while the pieces are
//! still being found. That pile is exactly what the report measures.
//!
//! The dungeon of each run is chosen greedily: the one whose boss table has the most
//! still-missing target pieces and books (a player farms the crypt for the Gravewarden set).
//! With nothing missing the player rotates through all of them.
//!
//! # The key curve
//!
//! `+1` key level per two timed runs up to key 20 (every run is assumed timed), then one level
//! per [`ETERNAL_PACE`] runs up to [`KEY_CAP`]. Two runs per level is the pace a group that clears
//! its keys would set, and the cap is where the power-scaling study puts the eternal wall: a
//! player is not assumed to out-climb their gear. The loot tables only read the key level
//! (epic chance, quality buckets, sharpening shift, gem chance).
//!
//! # Not modelled
//!
//! Aether Shards (modifier rerolls), key cleansing (100 gold, optional), experience, talent
//! points, and any sharpening beyond the safe ceiling: every level past it breaks the item
//! 95 % of the time or more (`economy.json`), so a rational player takes the pre-sharpened
//! drops instead.

use std::fmt::Write as _;
use std::sync::OnceLock;

use engine_game::character::{ItemId, ItemQuality, item_by_key};
use engine_game::firemage::Rng;
use engine_game::hero::Hero;
use engine_game::loot::{
    DropSource, LootRules, LootTable, boss_gold, epic_chance_at, roll_book, roll_equipment,
    roll_gem, roll_mob_gold,
};
use engine_game::talent::books::BookKind;
use engine_realm::catalog::{Catalog, Economy};
use engine_realm::gems::GemCatalog;
use engine_realm::reforge::{self, ReforgeTable};
use engine_realm::rewards::run_rewards;

use crate::class::Class;
use crate::gear::{GearProfile, GearTier, major_set_for};
use crate::gem_sim::{ELITES, MOBS, upgrade_gold};

/// Runs a player makes in the study.
pub const HORIZON: u32 = 900;
/// The run count at which unspent gold is judged ("after 300 runs").
pub const GOLD_CHECK: u32 = 300;
/// Unspent gold at [`GOLD_CHECK`] must stay under this many runs of gold income.
pub const GOLD_BAND_RUNS: f64 = 20.0;
/// The key level the player stops climbing at (the eternal wall, see the power-scaling study).
pub const KEY_CAP: u32 = 24;
/// Runs per key level above key 20.
pub const ETERNAL_PACE: u32 = 5;
/// A reforge is made only on a piece whose base quality is under this.
pub const QUALITY_GOAL: u8 = 90;
/// Gold kept back from reforging so gem upgrades are never starved.
const RESERVE: f64 = 300.0;
/// A found piece replaces the worn one when it is this many per cent better.
const REPLACE_MARGIN: u32 = 3;
/// The kind index the first-max-gem milestone calls "the chosen kind" (Ruby, the main stat).
const CHOSEN_KIND: usize = 0;
/// Run counts at which the gold balance is recorded.
pub const CHECKPOINTS: [u32; 4] = [100, 200, 300, HORIZON];

/// The milestones a trial records, in report order.
pub const MILESTONES: [&str; 10] = [
    "first max gem (any kind)",
    "first max gem (Ruby)",
    "full major set",
    "all 6 books",
    "all 13 target pieces",
    "sharpening ceiling (dust only)",
    "sharpening ceiling (13 pieces)",
    "quality 90+ in all 13",
    "quality 100+ in all 13",
    "max gems in every socket",
];

const M_GEM_ANY: usize = 0;
const M_GEM_RUBY: usize = 1;
const M_SET: usize = 2;
const M_BOOKS: usize = 3;
const M_GEAR: usize = 4;
const M_SHARP_DUST: usize = 5;
const M_SHARP: usize = 6;
const M_Q90: usize = 7;
const M_Q100: usize = 8;
const M_GEMS: usize = 9;

/// The key level of run `run` (1-based).
#[must_use]
pub fn key_level(run: u32) -> u32 {
    let timed = run.saturating_sub(1);
    let level = if timed / 2 < 20 {
        1 + timed / 2
    } else {
        20 + (timed - 38) / ETERNAL_PACE
    };
    level.min(KEY_CAP)
}

/// A dungeon the player can run: its loot tables and reward table.
#[derive(Debug, Clone, Copy)]
struct Site {
    /// The dungeon's id.
    id: &'static str,
    /// The final boss, whose table the chest rolls.
    boss: &'static str,
    /// A mid-dungeon guardian with a table of its own (one roll).
    mini: Option<&'static str>,
}

/// The dungeons with a boss table of their own. The Ashen Reliquary's boss has none and falls
/// back to the first table, so it drops what the crypt drops and adds nothing here.
const SITES: [Site; 4] = [
    Site {
        id: "graveyard_crypt",
        boss: "lord_ashgrave",
        mini: None,
    },
    Site {
        id: "drowned_catacombs",
        boss: "drowned_abbess",
        mini: None,
    },
    Site {
        id: "bone_foundry",
        boss: "foundry_overseer",
        mini: None,
    },
    Site {
        id: "sanctum_of_ash",
        boss: "ashen_pontiff",
        mini: Some("censer_warden"),
    },
];

/// Everything shared by all players: the catalogs and numbers derived from them.
pub struct World {
    rules: &'static LootRules,
    gems: &'static GemCatalog,
    catalog: &'static Catalog,
    table: ReforgeTable,
    /// The highest sharpening level reachable with no risk of breaking.
    safe_sharpening: u8,
    /// Crypt Dust to take an item to level `i + 1` (index `i`), for the safe levels.
    sharpen_dust: Vec<u64>,
    /// `(item, kind, level)` of every gem item.
    gem_items: Vec<(ItemId, usize, usize)>,
}

impl World {
    fn economy(&self) -> &Economy {
        self.catalog.economy()
    }

    /// The Crypt Dust a full set of `pieces` pieces needs to reach the safe ceiling.
    #[must_use]
    pub fn sharpen_dust_for(&self, pieces: usize) -> u64 {
        self.sharpen_dust.iter().sum::<u64>() * pieces as u64
    }

    /// The highest sharpening level with no break chance.
    #[must_use]
    pub fn safe_sharpening(&self) -> u8 {
        self.safe_sharpening
    }

    /// The key-level progression and eternal rules.
    #[must_use]
    pub fn modifiers(&self) -> &engine_realm::modifiers::ModifierCatalog {
        self.catalog.modifiers()
    }
}

/// The shared world, built once. `None` if a shipped catalog does not build.
#[must_use]
pub fn world() -> Option<&'static World> {
    static WORLD: OnceLock<Option<World>> = OnceLock::new();
    WORLD
        .get_or_init(|| {
            static CATALOG: OnceLock<Option<Catalog>> = OnceLock::new();
            let catalog = CATALOG.get_or_init(|| Catalog::builtin().ok()).as_ref()?;
            let rules = engine_realm::loot::builtin_rules()?;
            let gems = engine_realm::gems::builtin()?;
            let economy = catalog.economy();
            let rule = &economy.sharpening;
            let safe = (1..=rule.max_level)
                .take_while(|level| rule.break_chance(*level) <= 0.0)
                .last()
                .unwrap_or(0);
            let sharpen_dust = (1..=safe)
                .map(|level| {
                    rule.costs_for(level)
                        .filter(|cost| cost.resource == "crypt_dust")
                        .map(|cost| cost.amount)
                        .sum()
                })
                .collect();
            let gem_items = rules
                .gems
                .items
                .iter()
                .enumerate()
                .flat_map(|(kind, levels)| {
                    levels
                        .iter()
                        .enumerate()
                        .map(move |(level, id)| (*id, kind, level + 1))
                })
                .collect();
            Some(World {
                rules,
                gems,
                catalog,
                table: reforge::reforge_odds(economy),
                safe_sharpening: safe,
                sharpen_dust,
                gem_items,
            })
        })
        .as_ref()
}

/// One of the thirteen pieces the player is building towards.
#[derive(Debug, Clone, Copy)]
struct Target {
    id: ItemId,
    item_level: u16,
    sockets: u8,
    in_major_set: bool,
}

/// A target piece the player owns.
#[derive(Debug, Clone, Copy)]
struct Piece {
    quality: u8,
    sharpening: u8,
    /// Level-5 gems socketed.
    gems: u8,
}

impl Piece {
    fn total(&self, floor: u8) -> u32 {
        u32::from(self.quality) + u32::from(self.sharpening.max(floor))
    }
}

/// What a player had earned and spent at one point.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Snapshot {
    /// Runs done.
    pub run: u32,
    /// Gold in hand.
    pub gold: f64,
    /// Gold earned so far.
    pub income: f64,
    /// Gold spent on gem upgrades.
    pub gem_gold: f64,
    /// Gold spent taking gems out of sockets.
    pub unsocket_gold: f64,
    /// Gold spent on reforges.
    pub reforge_gold: f64,
    /// Reforges made.
    pub reforges: u32,
    /// Crypt Dust earned (run rewards and exchanged shards).
    pub dust_income: f64,
    /// Crypt Dust spent on sharpening.
    pub dust_spent: f64,
    /// Runs in which a gem pair waited for gold.
    pub gold_blocked: u32,
}

/// Income per run by key-level band, for the report.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BandIncome {
    /// Runs in the band.
    pub runs: u32,
    /// Gold earned.
    pub gold: f64,
    /// Crypt Dust from the reward table.
    pub dust: f64,
    /// Bone Shards from salvage.
    pub shards: f64,
    /// Equipment drops (target or not).
    pub equipment: f64,
    /// Gems dropped.
    pub gems: f64,
}

/// The key bands of [`BandIncome`]: below 10, 10 to 19, 20 and above.
pub const BANDS: [&str; 3] = ["key 1-9", "key 10-19", "key 20+"];

fn band_of(key: u32) -> usize {
    match key {
        0..=9 => 0,
        10..=19 => 1,
        _ => 2,
    }
}

/// One simulated player.
#[derive(Debug, Clone)]
pub struct Trial {
    /// The run each milestone was first met at, if it was.
    pub milestones: [Option<u32>; MILESTONES.len()],
    /// The balance at each of [`CHECKPOINTS`].
    pub snapshots: Vec<Snapshot>,
    /// Income by key band.
    pub bands: [BandIncome; 3],
}

struct Player {
    world: &'static World,
    hero: Hero,
    targets: Vec<Target>,
    pieces: Vec<Option<Piece>>,
    learned: Vec<ItemId>,
    /// Gems in the bag: `[kind][level]` (index 0 unused).
    bag: Vec<Vec<u32>>,
    /// Level-5 gems waiting for a free socket.
    banked: u32,
    gold: f64,
    dust: u64,
    shards: u64,
    rng: Rng,
    flow: Snapshot,
    milestones: [Option<u32>; MILESTONES.len()],
    snapshots: Vec<Snapshot>,
    bands: [BandIncome; 3],
    /// The band of the run in progress, for the income counters.
    band: usize,
    /// Runs per step of the key curve: 1 is the study's pace, 2 climbs half as fast.
    pace: u32,
}

impl Player {
    fn new(world: &'static World, class: Class, seed: u64) -> Self {
        let hero = class.hero();
        let major = major_set_for(hero).map(|set| set.id.clone());
        let targets: Vec<Target> = GearProfile::headline(GearTier::P2Set)
            .loadout(hero)
            .iter()
            .filter_map(|(_, key, _)| item_by_key(key))
            .map(|def| Target {
                id: def.id,
                item_level: def.item_level,
                sockets: def.sockets,
                in_major_set: def.set.is_some() && def.set.map(str::to_owned) == major,
            })
            .collect();
        let kinds = world.gems.kinds.len();
        let levels = usize::from(world.gems.max_level) + 1;
        Self {
            world,
            hero,
            pieces: vec![None; targets.len()],
            targets,
            learned: Vec::new(),
            bag: vec![vec![0; levels]; kinds],
            banked: 0,
            gold: 0.0,
            dust: 0,
            shards: 0,
            rng: Rng::new(seed),
            flow: Snapshot::default(),
            milestones: [None; MILESTONES.len()],
            snapshots: Vec::new(),
            bands: [BandIncome::default(); 3],
            band: 0,
            pace: 1,
        }
    }

    fn max_level(&self) -> usize {
        usize::from(self.world.gems.max_level)
    }

    fn earn_gold(&mut self, amount: u32) {
        self.gold += f64::from(amount);
        self.flow.income += f64::from(amount);
        self.bands[self.band].gold += f64::from(amount);
    }

    /// An item arrives: a target piece is kept or compared, anything else is salvaged.
    fn receive(&mut self, id: ItemId, quality: u8, sharpening: u8) {
        self.bands[self.band].equipment += 1.0;
        let Some(index) = self.targets.iter().position(|target| target.id == id) else {
            self.salvage(id);
            return;
        };
        let floor = self.world.safe_sharpening;
        let found = Piece {
            quality,
            sharpening,
            gems: 0,
        };
        match self.pieces[index] {
            None => self.pieces[index] = Some(found),
            Some(worn) if found.total(floor) >= worn.total(floor) + REPLACE_MARGIN => {
                let cost = f64::from(worn.gems)
                    * self.world.gems.unsocket_cost(self.world.gems.max_level) as f64;
                if self.gold >= cost {
                    self.gold -= cost;
                    self.flow.unsocket_gold += cost;
                    self.banked += u32::from(worn.gems);
                    self.pieces[index] = Some(found);
                }
                // The worn piece or the new one, whichever is not kept, is broken down.
                self.salvage(id);
            }
            Some(_) => self.salvage(id),
        }
    }

    fn salvage(&mut self, id: ItemId) {
        let rule = &self.world.economy().salvage;
        let level = engine_game::character::item(id).map_or(0, |def| u32::from(def.item_level));
        let shards = u64::from(rule.base) + u64::from(rule.per_item_level) * u64::from(level);
        self.shards += shards;
        self.bands[self.band].shards += shards as f64;
    }

    fn receive_gem(&mut self, id: ItemId, run: u32) {
        let Some(&(_, kind, level)) = self.world.gem_items.iter().find(|entry| entry.0 == id)
        else {
            return;
        };
        self.bands[self.band].gems += 1.0;
        self.add_gem(kind, level, run);
    }

    fn add_gem(&mut self, kind: usize, level: usize, run: u32) {
        if level >= self.max_level() {
            self.banked += 1;
            self.mark(M_GEM_ANY, run);
            if kind == CHOSEN_KIND {
                self.mark(M_GEM_RUBY, run);
            }
        } else {
            self.bag[kind][level] += 1;
        }
    }

    fn mark(&mut self, milestone: usize, run: u32) {
        if self.milestones[milestone].is_none() {
            self.milestones[milestone] = Some(run);
        }
    }

    fn learn(&mut self, id: ItemId) {
        if !self.learned.contains(&id) {
            self.learned.push(id);
        }
    }

    /// The dungeon that promises the most still-missing pieces and books.
    fn choose_site(&self, key: u32, run: u32) -> Site {
        let rules = self.world.rules;
        let epic = f64::from(epic_chance_at(rules, key));
        let rolls = f64::from(rules.equipment_per_completion);
        let mut best = (f64::MIN, SITES[0]);
        for (offset, site) in SITES.iter().enumerate() {
            let mut score = 0.0;
            if let Some(table) = rules.boss_table(site.boss) {
                score += rolls * self.missing_share(table, epic);
            }
            if let Some(table) = site.mini.and_then(|mini| rules.boss_table(mini)) {
                score += self.missing_share(table, epic);
            }
            score += self.book_need(site.boss, key);
            // Rotate between equal scores so a finished player visits every dungeon.
            let rotation = ((offset as u32 + run) % SITES.len() as u32) as f64 * 1e-9;
            if score + rotation > best.0 {
                best = (score + rotation, *site);
            }
        }
        best.1
    }

    /// The chance that one roll of `table` gives a target piece the player does not have.
    fn missing_share(&self, table: &LootTable, epic: f64) -> f64 {
        let weight = |want_epic: bool| -> (f64, f64) {
            let mut total = 0.0;
            let mut missing = 0.0;
            for entry in &table.entries {
                let Some(def) = engine_game::character::item(entry.item) else {
                    continue;
                };
                if (def.quality == ItemQuality::Epic) != want_epic {
                    continue;
                }
                total += f64::from(entry.weight);
                let wanted = self
                    .targets
                    .iter()
                    .position(|target| target.id == entry.item)
                    .is_some_and(|index| self.pieces[index].is_none());
                if wanted {
                    missing += f64::from(entry.weight);
                }
            }
            (total, missing)
        };
        let (epic_total, epic_missing) = weight(true);
        let (rare_total, rare_missing) = weight(false);
        let epic_share = if epic_total > 0.0 {
            epic_missing / epic_total
        } else {
            0.0
        };
        let rare_share = if rare_total > 0.0 {
            rare_missing / rare_total
        } else {
            0.0
        };
        epic * epic_share + (1.0 - epic) * rare_share
    }

    /// The chance that a chest of `boss` holds a book the player has not read.
    fn book_need(&self, boss: &str, key: u32) -> f64 {
        let Some(books) = &self.world.rules.books else {
            return 0.0;
        };
        let chances = books.chances_for(Some(boss));
        let mut need = 0.0;
        for (kind, chance) in [
            (BookKind::TalentPoint, chances.talent),
            (BookKind::Enhancement, chances.enhancement),
        ] {
            let eligible: Vec<_> = books.per_hero[self.hero.index()]
                .iter()
                .filter(|book| book.kind == kind && book.min_key_level <= key)
                .collect();
            if eligible.is_empty() {
                continue;
            }
            let unread = eligible
                .iter()
                .filter(|book| !self.learned.contains(&book.item))
                .count();
            need += f64::from(chance) * unread as f64 / eligible.len() as f64;
        }
        need
    }

    /// One run: the drops of the dungeon, then the spending.
    fn run_once(&mut self, run: u32) {
        let world = self.world;
        let rules = world.rules;
        let key = key_level(1 + (run - 1) / self.pace.max(1));
        self.band = band_of(key);
        self.bands[self.band].runs += 1;
        let site = self.choose_site(key, run);
        let dust_before = self.flow.dust_income;

        for kill in 0..MOBS + ELITES {
            let source = if kill < MOBS {
                DropSource::Mob
            } else {
                DropSource::Elite
            };
            if self.rng.chance(rules.mob.equipment_chance)
                && let Some((id, props)) =
                    roll_equipment(&rules.mob.table, rules, source, site.id, key, &mut self.rng)
            {
                self.receive(id, props.quality, props.sharpening);
            }
            if let Some(gold) = roll_mob_gold(rules, 0, &mut self.rng) {
                self.earn_gold(gold.amount);
            }
            if let Some(id) = roll_gem(rules, source, key, 0, &mut self.rng) {
                self.receive_gem(id, run);
            }
        }
        if let Some(table) = site.mini.and_then(|mini| rules.boss_table(mini))
            && let Some((id, props)) =
                roll_equipment(table, rules, DropSource::Elite, site.id, key, &mut self.rng)
        {
            self.receive(id, props.quality, props.sharpening);
        }
        if let Some(gold) = boss_gold(rules, 0) {
            self.earn_gold(gold.amount);
        }
        if let Some(id) = roll_gem(rules, DropSource::Boss, key, 0, &mut self.rng) {
            self.receive_gem(id, run);
        }
        if let Some(table) = rules.boss_table(site.boss) {
            for _ in 0..rules.equipment_per_completion {
                if let Some((id, props)) =
                    roll_equipment(table, rules, DropSource::Boss, site.id, key, &mut self.rng)
                {
                    self.receive(id, props.quality, props.sharpening);
                }
            }
        }
        if let Some(id) = roll_gem(rules, DropSource::Chest, key, 0, &mut self.rng) {
            self.receive_gem(id, run);
        }
        if let Some(books) = &rules.books
            && let Some(id) = roll_book(books, Some(site.boss), self.hero, key, &mut self.rng)
        {
            self.learn(id);
        }
        if let Some(dungeon) = world.catalog.dungeon(site.id) {
            for (resource, amount) in run_rewards(dungeon, key, true, true).resources {
                if resource == "crypt_dust" {
                    self.dust += u64::from(amount);
                    self.flow.dust_income += f64::from(amount);
                }
            }
        }
        self.exchange_shards();
        self.bands[self.band].dust += self.flow.dust_income - dust_before;

        self.spend(run);
        self.check(run);
    }

    /// Bone Shards into Crypt Dust at the catalog's rate.
    fn exchange_shards(&mut self) {
        let Some(trade) = self.world.economy().exchange("shards_to_dust") else {
            return;
        };
        let from = u64::from(trade.from_amount).max(1);
        let times = self.shards / from;
        let gained = times * u64::from(trade.to_amount);
        self.shards -= times * from;
        self.dust += gained;
        self.flow.dust_income += gained as f64;
    }

    fn spend(&mut self, run: u32) {
        self.upgrade_gems(run);
        self.socket_gems();
        self.sharpen();
        self.reforge();
    }

    /// Upgrade attempts, highest level first, while a pair and the gold are there.
    fn upgrade_gems(&mut self, run: u32) {
        let consume = u32::from(self.world.gems.upgrade.consume);
        let max = self.max_level();
        let mut waiting = false;
        loop {
            let mut attempted = false;
            'search: for level in (1..max).rev() {
                for kind in 0..self.bag.len() {
                    if self.bag[kind][level] < consume {
                        continue;
                    }
                    let Some(step) = self.world.gems.step(u8::try_from(level).unwrap_or(u8::MAX))
                    else {
                        continue;
                    };
                    if self.gold < step.gold as f64 {
                        waiting = true;
                        continue;
                    }
                    self.gold -= step.gold as f64;
                    self.flow.gem_gold += step.gold as f64;
                    self.bag[kind][level] -= consume;
                    if f64::from(self.rng.next_f32()) < step.success_chance {
                        self.add_gem(kind, level + 1, run);
                    } else {
                        // `keep_one`: a failure hands one of the consumed gems back.
                        self.bag[kind][level] += 1;
                    }
                    attempted = true;
                    break 'search;
                }
            }
            if !attempted {
                break;
            }
        }
        if waiting {
            self.flow.gold_blocked += 1;
        }
    }

    /// Banked level-5 gems into free sockets of owned pieces (free of charge).
    fn socket_gems(&mut self) {
        for index in 0..self.pieces.len() {
            let sockets = self.targets[index].sockets;
            while self.banked > 0
                && let Some(piece) = self.pieces[index].as_mut()
                && piece.gems < sockets
            {
                piece.gems += 1;
                self.banked -= 1;
            }
        }
    }

    /// Sharpening up to the safe ceiling, the lowest piece first.
    fn sharpen(&mut self) {
        let safe = self.world.safe_sharpening;
        loop {
            let next = self
                .pieces
                .iter()
                .enumerate()
                .filter_map(|(index, piece)| piece.map(|piece| (index, piece)))
                .filter(|(_, piece)| piece.sharpening < safe)
                .min_by_key(|(_, piece)| piece.sharpening);
            let Some((index, piece)) = next else { break };
            let cost = self.world.sharpen_dust[usize::from(piece.sharpening)];
            if self.dust < cost {
                break;
            }
            self.dust -= cost;
            self.flow.dust_spent += cost as f64;
            if let Some(piece) = self.pieces[index].as_mut() {
                piece.sharpening += 1;
            }
        }
    }

    /// Reforges the lowest-quality owned piece while it is under the goal and gold allows.
    fn reforge(&mut self) {
        loop {
            let next = self
                .pieces
                .iter()
                .enumerate()
                .filter_map(|(index, piece)| piece.map(|piece| (index, piece)))
                .filter(|(_, piece)| piece.quality < QUALITY_GOAL)
                .min_by_key(|(_, piece)| piece.quality);
            let Some((index, piece)) = next else { break };
            let cost = reforge::cost(
                self.world.economy(),
                self.targets[index].item_level,
                piece.sharpening,
            ) as f64;
            if self.gold < cost + RESERVE {
                break;
            }
            self.gold -= cost;
            self.flow.reforge_gold += cost;
            self.flow.reforges += 1;
            let rng = &mut self.rng;
            let quality = reforge::roll(&mut || rng.next_u64(), &self.world.table);
            if let Some(piece) = self.pieces[index].as_mut() {
                piece.quality = quality;
            }
        }
    }

    /// Milestones and checkpoints after a run.
    fn check(&mut self, run: u32) {
        let owned: Vec<(Target, Piece)> = self
            .targets
            .iter()
            .zip(&self.pieces)
            .filter_map(|(target, piece)| piece.map(|piece| (*target, piece)))
            .collect();
        let all = owned.len() == self.targets.len();
        let set_pieces = self.targets.iter().filter(|t| t.in_major_set).count();
        if set_pieces > 0
            && owned
                .iter()
                .filter(|(target, _)| target.in_major_set)
                .count()
                == set_pieces
        {
            self.mark(M_SET, run);
        }
        if let Some(books) = &self.world.rules.books
            && books.per_hero[self.hero.index()]
                .iter()
                .all(|book| self.learned.contains(&book.item))
        {
            self.mark(M_BOOKS, run);
        }
        let dust_needed = self.world.sharpen_dust_for(self.targets.len()) as f64;
        if self.flow.dust_income >= dust_needed {
            self.mark(M_SHARP_DUST, run);
        }
        if all {
            self.mark(M_GEAR, run);
            let safe = self.world.safe_sharpening;
            if owned.iter().all(|(_, piece)| piece.sharpening >= safe) {
                self.mark(M_SHARP, run);
            }
            if owned.iter().all(|(_, piece)| piece.quality >= 90) {
                self.mark(M_Q90, run);
            }
            if owned.iter().all(|(_, piece)| piece.quality >= 100) {
                self.mark(M_Q100, run);
            }
            if owned
                .iter()
                .all(|(target, piece)| piece.gems >= target.sockets)
            {
                self.mark(M_GEMS, run);
            }
        }
        if CHECKPOINTS.contains(&run) {
            self.snapshots.push(Snapshot {
                run,
                gold: self.gold,
                ..self.flow
            });
        }
    }
}

/// One player's `horizon` runs, the key curve taking `pace` runs for every one of the study's.
#[must_use]
pub fn trial(world: &'static World, class: Class, horizon: u32, seed: u64, pace: u32) -> Trial {
    let mut player = Player::new(world, class, seed);
    player.pace = pace;
    for run in 1..=horizon {
        player.run_once(run);
    }
    Trial {
        milestones: player.milestones,
        snapshots: player.snapshots,
        bands: player.bands,
    }
}

/// Many players of one class.
#[derive(Debug, Clone)]
pub struct Study {
    /// The class.
    pub class: Class,
    /// The players.
    pub trials: Vec<Trial>,
}

impl Study {
    /// The `p`-quantile of the run a milestone was met at, counting players who never met it as
    /// infinitely late. `None` when that quantile is a player who did not finish.
    #[must_use]
    pub fn quantile(&self, milestone: usize, p: f64) -> Option<u32> {
        let mut runs: Vec<u32> = self
            .trials
            .iter()
            .map(|trial| trial.milestones[milestone].unwrap_or(u32::MAX))
            .collect();
        runs.sort_unstable();
        let last = runs.len().saturating_sub(1);
        let value = *runs.get(((last as f64) * p).round() as usize)?;
        (value != u32::MAX).then_some(value)
    }

    /// The median run of a milestone.
    #[must_use]
    pub fn median(&self, milestone: usize) -> Option<u32> {
        self.quantile(milestone, 0.5)
    }

    /// The share of players who met a milestone within `runs`.
    #[must_use]
    pub fn finished_by(&self, milestone: usize, runs: u32) -> f64 {
        let hits = self
            .trials
            .iter()
            .filter(|trial| trial.milestones[milestone].is_some_and(|run| run <= runs))
            .count();
        hits as f64 / self.trials.len().max(1) as f64
    }

    /// The mean snapshot at the checkpoint `run`.
    #[must_use]
    pub fn mean_at(&self, run: u32) -> Snapshot {
        let snaps: Vec<&Snapshot> = self
            .trials
            .iter()
            .filter_map(|trial| trial.snapshots.iter().find(|snap| snap.run == run))
            .collect();
        let n = snaps.len().max(1) as f64;
        let mean = |f: &dyn Fn(&Snapshot) -> f64| snaps.iter().map(|s| f(s)).sum::<f64>() / n;
        Snapshot {
            run,
            gold: mean(&|s| s.gold),
            income: mean(&|s| s.income),
            gem_gold: mean(&|s| s.gem_gold),
            unsocket_gold: mean(&|s| s.unsocket_gold),
            reforge_gold: mean(&|s| s.reforge_gold),
            reforges: mean(&|s| f64::from(s.reforges)).round() as u32,
            dust_income: mean(&|s| s.dust_income),
            dust_spent: mean(&|s| s.dust_spent),
            gold_blocked: mean(&|s| f64::from(s.gold_blocked)).round() as u32,
        }
    }

    /// The mean income per run in a key band.
    #[must_use]
    pub fn band(&self, band: usize) -> BandIncome {
        let n = self.trials.len().max(1) as f64;
        let sum = |f: &dyn Fn(&BandIncome) -> f64| {
            self.trials.iter().map(|t| f(&t.bands[band])).sum::<f64>() / n
        };
        BandIncome {
            runs: self
                .trials
                .first()
                .map_or(0, |trial| trial.bands[band].runs),
            gold: sum(&|b| b.gold),
            dust: sum(&|b| b.dust),
            shards: sum(&|b| b.shards),
            equipment: sum(&|b| b.equipment),
            gems: sum(&|b| b.gems),
        }
    }

    /// Mean gold earned per run over the first [`GOLD_CHECK`] runs.
    #[must_use]
    pub fn income_per_run(&self) -> f64 {
        self.mean_at(GOLD_CHECK).income / f64::from(GOLD_CHECK)
    }

    /// Unspent gold at [`GOLD_CHECK`], in runs of income.
    #[must_use]
    pub fn unspent_runs(&self) -> f64 {
        self.mean_at(GOLD_CHECK).gold / self.income_per_run().max(1e-9)
    }
}

/// Simulates `players` of every class for `horizon` runs.
#[must_use]
pub fn run(world: &'static World, players: u32, horizon: u32, seed: u64) -> Vec<Study> {
    run_paced(world, players, horizon, seed, 1)
}

/// [`run`] with the key curve slowed by `pace`.
#[must_use]
pub fn run_paced(
    world: &'static World,
    players: u32,
    horizon: u32,
    seed: u64,
    pace: u32,
) -> Vec<Study> {
    Class::ALL
        .into_iter()
        .map(|class| Study {
            class,
            trials: (0..players)
                .map(|n| {
                    let mix = (class as u64 + 1) * 0x85EB_CA6B + u64::from(n) * 0x9E37_79B9;
                    trial(world, class, horizon, seed ^ mix, pace)
                })
                .collect(),
        })
        .collect()
}

fn runs_text(value: Option<u32>) -> String {
    value.map_or_else(|| format!("> {HORIZON}"), |runs| runs.to_string())
}

fn percent(value: f64) -> String {
    format!("{:.0} %", value * 100.0)
}

/// The headline targets: label, milestone, lowest and highest acceptable median run.
const TARGETS: [(&str, usize, u32, u32); 3] = [
    ("First max-level gem (Ruby)", M_GEM_RUBY, 60, 150),
    ("Full major set", M_SET, 40, 120),
    ("All 6 books", M_BOOKS, 80, 200),
];

/// The expected gold one piece costs to bring up to [`QUALITY_GOAL`] by reforging alone (each
/// attempt replaces the roll, so it is a geometric wait for one that lands at or above it).
fn reforge_gold_per_piece(world: &World, item_level: u16) -> f64 {
    let chance = world.table.chance_at_least(QUALITY_GOAL).max(1e-9);
    let price = reforge::cost(world.economy(), item_level, world.safe_sharpening) as f64;
    price / chance
}

/// The report body.
pub fn write_body(out: &mut String, world: &'static World, studies: &[Study], seed: u64) {
    let players = studies.first().map_or(0, |study| study.trials.len());
    let _ = writeln!(
        out,
        "{players} players per class, {HORIZON} runs each, seed {seed:#x}. A run is {MOBS} ordinary \
         kills, {ELITES} elites, the boss and its chest (see `engine-sim/src/economy_sim.rs`). Key \
         levels: +1 per two runs up to key 20, then +1 per {ETERNAL_PACE} runs up to key {KEY_CAP} \
         (run 39 is the first at key 20, run {} the first at the cap). Every figure below is a mean \
         over the players unless it says median.\n",
        39 + (KEY_CAP - 20) * ETERNAL_PACE
    );

    let _ = writeln!(out, "### Income per run\n");
    let _ = writeln!(
        out,
        "| Class | Key band | Gold | Crypt Dust (rewards) | Bone Shards (salvage) | Equipment drops | Gems |"
    );
    let _ = writeln!(out, "|---|---|---:|---:|---:|---:|---:|");
    for study in studies {
        for (index, name) in BANDS.iter().enumerate() {
            let band = study.band(index);
            let runs = f64::from(band.runs.max(1));
            let _ = writeln!(
                out,
                "| {} | {} | {:.0} | {:.1} | {:.1} | {:.2} | {:.2} |",
                study.class.title(),
                name,
                band.gold / runs,
                band.dust / runs,
                band.shards / runs,
                band.equipment / runs,
                band.gems / runs
            );
        }
    }

    let _ = writeln!(
        out,
        "\nGold is flat in the key level (`loot.json` pays per kill and for the boss only); Crypt \
         Dust grows with it (`dungeons.json` `per_level`). Gold has three sinks and no other use: \
         gem upgrades, taking a gem out, and reforging.\n"
    );

    let _ = writeln!(out, "### Gold: income against sinks\n");
    let _ = writeln!(
        out,
        "| Class | Runs | Income | Gem upgrades | Unsocket | Reforge (made) | Unspent | Unspent, runs of income | Gem pair waited on gold |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for study in studies {
        for run in CHECKPOINTS {
            let snap = study.mean_at(run);
            let per_run = (snap.income / f64::from(run)).max(1e-9);
            let _ = writeln!(
                out,
                "| {} | {} | {:.0} | {:.0} | {:.0} | {:.0} ({}) | {:.0} | {:.1} | {} runs |",
                study.class.title(),
                run,
                snap.income,
                snap.gem_gold,
                snap.unsocket_gold,
                snap.reforge_gold,
                snap.reforges,
                snap.gold,
                snap.gold / per_run,
                snap.gold_blocked
            );
        }
    }

    let _ = writeln!(out, "\n### Crypt Dust: income against sharpening\n");
    let _ = writeln!(
        out,
        "Sharpening is paid in Crypt Dust, not gold. Only levels up to +{} are safe (the next break \
         the item 95 % of the time or more), so that is the ceiling a player crafts to; anything \
         higher comes from a pre-sharpened drop (`docs/balance/loot-sharpening.md`).\n",
        world.safe_sharpening()
    );
    let _ = writeln!(out, "| Class | Runs | Dust earned | Dust spent | Left |");
    let _ = writeln!(out, "|---|---:|---:|---:|---:|");
    for study in studies {
        for run in CHECKPOINTS {
            let snap = study.mean_at(run);
            let _ = writeln!(
                out,
                "| {} | {} | {:.0} | {:.0} | {:.0} |",
                study.class.title(),
                run,
                snap.dust_income,
                snap.dust_spent,
                snap.dust_income - snap.dust_spent
            );
        }
    }
    if let Some(study) = studies.first() {
        let needed = world.sharpen_dust_for(13) as f64;
        let earned = study.mean_at(GOLD_CHECK).dust_income;
        let _ = writeln!(
            out,
            "\nBringing all 13 pieces to +{} costs {needed:.0} Crypt Dust once; the player earns \
             {earned:.0} by run {GOLD_CHECK}, {:.0} times that. Dust never gates sharpening: \
             owning the pieces does, and past the safe levels the break chance. It also has no other \
             use (Steward's Ember, 20 dust each, is only asked for by the unsafe levels).",
            world.safe_sharpening(),
            earned / needed.max(1.0)
        );
    }

    let _ = writeln!(out, "\n### Runs to best-in-slot, per system\n");
    let _ = writeln!(
        out,
        "Median run at which each milestone is met (10th to 90th percentile in brackets; \
         `> {HORIZON}` means the median player had not finished). The last column is the share \
         who had finished by run {GOLD_CHECK}.\n"
    );
    let _ = writeln!(out, "| Milestone | Warrior | Druid | Mage |");
    let _ = writeln!(out, "|---|---|---|---|");
    for (index, name) in MILESTONES.iter().enumerate() {
        let cells: Vec<String> = studies
            .iter()
            .map(|study| {
                format!(
                    "{} ({}-{}), {} by {GOLD_CHECK}",
                    runs_text(study.median(index)),
                    runs_text(study.quantile(index, 0.1)),
                    runs_text(study.quantile(index, 0.9)),
                    percent(study.finished_by(index, GOLD_CHECK))
                )
            })
            .collect();
        let _ = writeln!(out, "| {} | {} |", name, cells.join(" | "));
    }

    let _ = writeln!(out, "\n### Targets\n");
    let _ = writeln!(out, "| Target | Band | Warrior | Druid | Mage |");
    let _ = writeln!(out, "|---|---|---:|---:|---:|");
    for (label, milestone, low, high) in TARGETS {
        let cells: Vec<String> = studies
            .iter()
            .map(|study| {
                let median = study.median(milestone);
                let ok = median.is_some_and(|runs| (low..=high).contains(&runs));
                format!("{} {}", runs_text(median), if ok { "ok" } else { "MISS" })
            })
            .collect();
        let _ = writeln!(
            out,
            "| {label} (median runs) | {low}-{high} | {} |",
            cells.join(" | ")
        );
    }
    let cells: Vec<String> = studies
        .iter()
        .map(|study| {
            let unspent = study.unspent_runs();
            let verdict = if unspent < GOLD_BAND_RUNS {
                "ok"
            } else {
                "MISS"
            };
            format!("{unspent:.1} {verdict}")
        })
        .collect();
    let _ = writeln!(
        out,
        "| Unspent gold after {GOLD_CHECK} runs (runs of income) | under {GOLD_BAND_RUNS:.0} | {} |",
        cells.join(" | ")
    );

    let _ = writeln!(out, "\n### Sink demand\n");
    let demand = demand_rows(world, studies);
    let _ = writeln!(
        out,
        "| Class | Sockets | Gold to max every gem | Gold to reforge 13 pieces to {QUALITY_GOAL}+ | Total | Runs of income |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|");
    for row in &demand {
        let _ = writeln!(
            out,
            "| {} | {} | {:.0} | {:.0} | {:.0} | {:.0} |",
            row.class.title(),
            row.sockets,
            row.gem_gold,
            row.reforge_gold,
            row.gem_gold + row.reforge_gold,
            (row.gem_gold + row.reforge_gold) / row.income_per_run.max(1e-9)
        );
    }
    let _ = writeln!(
        out,
        "\nGem gold is the chain's expected price per max-level gem ({:.0} gold, from the success \
         chances alone) times the sockets; reforge gold is the expected attempts to land \
         {QUALITY_GOAL}+ ({:.1} % per attempt) times the price of one at +{} for every one of the \
         13 pieces, which overstates it (a drop often arrives above the goal). The player also \
         pays {:.0} gold per maxed gem moved to a better piece.\n",
        upgrade_gold(world.gems),
        world.table.chance_at_least(QUALITY_GOAL) * 100.0,
        world.safe_sharpening,
        world.gems.unsocket_cost(world.gems.max_level) as f64
    );
}

/// The analytic sink demand of one class's build.
struct Demand {
    class: Class,
    sockets: u32,
    gem_gold: f64,
    reforge_gold: f64,
    income_per_run: f64,
}

fn demand_rows(world: &'static World, studies: &[Study]) -> Vec<Demand> {
    studies
        .iter()
        .map(|study| {
            let player = Player::new(world, study.class, 1);
            let sockets: u32 = player
                .targets
                .iter()
                .map(|target| u32::from(target.sockets))
                .sum();
            let reforge_gold = player
                .targets
                .iter()
                .map(|target| reforge_gold_per_piece(world, target.item_level))
                .sum();
            Demand {
                class: study.class,
                sockets,
                gem_gold: upgrade_gold(world.gems) * f64::from(sockets),
                reforge_gold,
                income_per_run: study.income_per_run(),
            }
        })
        .collect()
}

/// How the headline milestones move when the key curve climbs `pace` times slower (the player
/// clears fewer keys in time), for the Warrior.
fn write_pace(out: &mut String, world: &'static World, players: u32, seed: u64) {
    let _ = writeln!(
        out,
        "\n### If the player climbs the keys more slowly\n\n\
         The key curve is an assumption and loot reads the key level (epic chance, quality, \
         gems). The Warrior's medians with the whole curve stretched (`pace` runs per step of \
         the study's curve), {players} players:\n"
    );
    let _ = writeln!(
        out,
        "| Pace | Key at run 100 | Key at run 300 | First max gem (Ruby) | Full major set | All 6 books | All 13 pieces | Quality 90+ |"
    );
    let _ = writeln!(out, "|---:|---:|---:|---:|---:|---:|---:|---:|");
    for pace in [1_u32, 2, 4] {
        let study = Study {
            class: Class::Warrior,
            trials: (0..players)
                .map(|n| {
                    trial(
                        world,
                        Class::Warrior,
                        HORIZON,
                        seed ^ (u64::from(n) * 0x9E37_79B9 + u64::from(pace)),
                        pace,
                    )
                })
                .collect(),
        };
        let _ = writeln!(
            out,
            "| x{pace} | {} | {} | {} | {} | {} | {} | {} |",
            key_level(1 + 99 / pace),
            key_level(1 + 299 / pace),
            runs_text(study.median(M_GEM_RUBY)),
            runs_text(study.median(M_SET)),
            runs_text(study.median(M_BOOKS)),
            runs_text(study.median(M_GEAR)),
            runs_text(study.median(M_Q90)),
        );
    }
}

/// The main report's section: a modest sample of the study.
pub fn write_section(out: &mut String, seed: u64) {
    let _ = writeln!(out, "## 10. Economy\n");
    let Some(world) = world() else {
        let _ = writeln!(out, "Not run: the shipped catalogs did not build.\n");
        return;
    };
    let studies = run(world, 100, HORIZON, seed);
    write_body(out, world, &studies, seed);
    write_pace(out, world, 60, seed);
    let _ = writeln!(
        out,
        "\n`engine-sim --economy-sim` writes the full report (`docs/balance/economy.md`).\n"
    );
}

/// The standalone markdown report (`--economy-sim`).
#[must_use]
pub fn report(world: &'static World, studies: &[Study], seed: u64) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Economy simulation\n");
    let _ = writeln!(
        out,
        "Generated by `engine-sim --economy-sim` (regenerate after editing anything in \
         `assets/catalog`). The model, the player's policy and the key curve are documented at the \
         top of `crates/engine-sim/src/economy_sim.rs`.\n"
    );
    write_body(&mut out, world, studies, seed);
    write_pace(
        &mut out,
        world,
        (studies.first().map_or(60, |s| s.trials.len()) as u32).min(100),
        seed,
    );
    out
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    /// Players per class: enough for a stable median, few enough to stay quick. Debug builds
    /// run fewer and the bands are wider.
    fn players() -> u32 {
        if cfg!(debug_assertions) { 40 } else { 150 }
    }

    fn studies() -> &'static (&'static World, Vec<Study>) {
        static STUDIES: OnceLock<(&'static World, Vec<Study>)> = OnceLock::new();
        STUDIES.get_or_init(|| {
            let world = world().expect("the shipped catalogs build");
            (world, run(world, players(), 400, 0xEC0_5EED))
        })
    }

    /// `(low, high)` widened for the smaller debug sample.
    fn band(low: u32, high: u32) -> (u32, u32) {
        if cfg!(debug_assertions) {
            (low * 3 / 4, high * 5 / 4)
        } else {
            (low, high)
        }
    }

    fn median_in(study: &Study, milestone: usize, low: u32, high: u32) {
        let (low, high) = band(low, high);
        let median = study.median(milestone).unwrap_or(u32::MAX);
        assert!(
            (low..=high).contains(&median),
            "{:?}: median runs to {} is {median}, expected {low}..={high}",
            study.class,
            MILESTONES[milestone]
        );
    }

    #[test]
    fn the_key_curve_climbs_two_runs_a_level_then_slows_and_caps() {
        assert_eq!(key_level(1), 1);
        assert_eq!(key_level(2), 1);
        assert_eq!(key_level(3), 2);
        assert_eq!(key_level(39), 20);
        assert_eq!(key_level(40), 20);
        assert_eq!(key_level(39 + ETERNAL_PACE), 21);
        assert_eq!(key_level(10_000), KEY_CAP);
        assert!((1..2_000).all(|run| key_level(run) <= key_level(run + 1)));
    }

    #[test]
    fn a_max_gem_arrives_in_about_a_hundred_runs() {
        for study in &studies().1 {
            median_in(study, M_GEM_RUBY, 60, 150);
        }
    }

    #[test]
    fn the_full_major_set_takes_forty_to_a_hundred_and_twenty_runs() {
        for study in &studies().1 {
            median_in(study, M_SET, 40, 120);
        }
    }

    #[test]
    fn all_the_books_take_eighty_to_two_hundred_runs() {
        for study in &studies().1 {
            median_in(study, M_BOOKS, 80, 200);
        }
    }

    #[test]
    fn gold_does_not_pile_up() {
        let limit = if cfg!(debug_assertions) {
            GOLD_BAND_RUNS * 1.5
        } else {
            GOLD_BAND_RUNS
        };
        for study in &studies().1 {
            let unspent = study.unspent_runs();
            assert!(
                unspent < limit,
                "{:?} holds {unspent:.1} runs of income unspent after {GOLD_CHECK} runs",
                study.class
            );
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_report() {
        let world = world().expect("the shipped catalogs build");
        let a = report(world, &run(world, 6, 120, 9), 9);
        let b = report(world, &run(world, 6, 120, 9), 9);
        assert_eq!(a, b);
        assert!(a.contains("Runs to best-in-slot"));
    }
}
