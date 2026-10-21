//! Loot rules and the rolls made from them: quality, epics, equipment, and mob currency.
//!
//! ```text
//! catalog (engine-realm, serde) ──► LootRules (plain data, this module)
//!                                       │ held by the Simulation (set_loot_rules)
//!                                       ▼
//! boss chest ──► roll_equipment(boss table) ──► epic?  ─┬─ yes ► pick among the table's epics
//!                                                       └─ no  ► pick among the rest
//!                                       │
//!                                       └──────────────► roll_quality ──► ItemProps
//! normal mob ──► equipment_chance ► roll_equipment(mob table)
//!            └─► roll_mob_currency ► aether shards, gold
//! ```
//!
//! # Pure rolls
//!
//! Every function here takes the generator it draws from ([`Rng`]) and nothing else: no clock,
//! no thread-local randomness. The same seed and the same sequence of calls give the same
//! loot, which is what lets the server's loot be reproduced from a replay and tested to a
//! tolerance rather than an anecdote.
//!
//! # Why plain data, and why here
//!
//! The balance numbers live in `assets/catalog/loot.json`, read by `engine-realm`. This crate
//! keeps no serde, so `engine-realm` converts the file into [`LootRules`] and hands that to
//! the simulation, which is the only thing that rolls: the client never does.
//!
//! # Key level
//!
//! A dungeon key's level raises the odds of a good drop in two places: the quality buckets at
//! or above [`QualityRules::high_from`] are weighted up by
//! `1 + high_bucket_weight_per_key_level x (level - 1)`, and the epic chance grows by
//! [`EpicRules::per_key_level`] per level up to [`EpicRules::max_chance`].
//! [`epic_chance_at`] and [`quality_odds`] answer both for a tooltip.

use crate::character::{ItemId, ItemProps, ItemQuality, QUALITY_MAX, QUALITY_MIN, item};
use crate::firemage::Rng;
use crate::hero::Hero;
use crate::talent::books::BookKind;
pub use crate::sharpen_drops::{DropSource, SharpeningDrops};

/// One entry of a loot table: an item and how likely it is relative to its siblings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeightedItem {
    /// Which item.
    pub item: ItemId,
    /// Its relative weight. Only the ratio to the other entries matters.
    pub weight: f32,
}

/// A weighted list of items.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LootTable {
    /// The entries, in file order.
    pub entries: Vec<WeightedItem>,
}

impl LootTable {
    /// Whether the table has nothing to give.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries that are epic (`epic`) or not (`!epic`).
    fn of_rarity(&self, epic: bool) -> Vec<WeightedItem> {
        self.entries
            .iter()
            .filter(|entry| {
                item(entry.item).is_some_and(|def| (def.quality == ItemQuality::Epic) == epic)
            })
            .copied()
            .collect()
    }
}

/// A boss's table, named by the boss's stable id (its enemy archetype id, `"lord_ashgrave"`).
#[derive(Debug, Clone, PartialEq)]
pub struct BossTable {
    /// The boss's stable id.
    pub boss: String,
    /// What it drops.
    pub table: LootTable,
}

/// One band of the quality distribution: every whole percentage in `min..=max` is equally
/// likely within it, and the band as a whole has `weight`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityBucket {
    /// Lowest quality in the band.
    pub min: u8,
    /// Highest quality in the band.
    pub max: u8,
    /// Its relative weight at key level 1.
    pub weight: f32,
}

/// How an item's quality is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityRules {
    /// The bands, covering `QUALITY_MIN..=QUALITY_MAX`.
    pub buckets: Vec<QualityBucket>,
    /// Bands whose `min` is at least this are the "high" ones a key level boosts.
    pub high_from: u8,
    /// Their weight is multiplied by `1 + this x (key level - 1)`.
    pub high_bucket_weight_per_key_level: f32,
}

/// The chance that an equipment roll is an epic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EpicRules {
    /// At key level 1.
    pub base_chance: f32,
    /// Added for every key level above the first.
    pub per_key_level: f32,
    /// The ceiling.
    pub max_chance: f32,
}

/// What a normal monster's death can drop as equipment.
#[derive(Debug, Clone, PartialEq)]
pub struct MobLootRules {
    /// The chance, per player per kill, of one equipment drop.
    pub equipment_chance: f32,
    /// What it drops.
    pub table: LootTable,
}

/// The modifier-reroll currency monsters drop.
#[derive(Debug, Clone, PartialEq)]
pub struct CurrencyRules {
    /// The resource's id in the economy catalog.
    pub resource: &'static str,
    /// The chance, per player per ordinary kill, of one drop.
    pub per_mob_chance: f32,
    /// The same chance for an elite.
    pub elite_chance: f32,
    /// How many one drop is worth.
    pub amount: u32,
}

/// The gold monsters drop.
#[derive(Debug, Clone, PartialEq)]
pub struct GoldRules {
    /// The resource's id in the economy catalog.
    pub resource: &'static str,
    /// The least a kill pays, per player.
    pub per_mob_min: u32,
    /// The most a kill pays, per player.
    pub per_mob_max: u32,
    /// What the boss pays, per player.
    pub boss_amount: u32,
}

/// Every loot number the simulation needs, as plain data.
#[derive(Debug, Clone, PartialEq)]
pub struct LootRules {
    /// How many independent equipment rolls each player gets from the boss chest.
    pub equipment_per_completion: u32,
    /// How quality is drawn.
    pub quality: QualityRules,
    /// How often an equipment roll is epic.
    pub epic: EpicRules,
    /// Per-boss tables.
    pub boss_tables: Vec<BossTable>,
    /// What ordinary monsters drop.
    pub mob: MobLootRules,
    /// The modifier-reroll currency.
    pub currency: CurrencyRules,
    /// Gold.
    pub gold: GoldRules,
    /// How a drop is pre-sharpened.
    pub sharpening: SharpeningDrops,
    /// Book drops from a boss chest. `None`: bosses drop no books.
    pub books: Option<BookRules>,
}

/// The chance a boss chest holds a book, per player, by kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BookChances {
    /// A talent-point book.
    pub talent: f32,
    /// An enhancement book.
    pub enhancement: f32,
}

/// One book a boss can drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookDrop {
    /// The book's item.
    pub item: ItemId,
    /// Its kind.
    pub kind: BookKind,
    /// The lowest key level it drops at.
    pub min_key_level: u32,
}

/// Book drops: a chance per boss, and each hero's books.
#[derive(Debug, Clone, PartialEq)]
pub struct BookRules {
    /// The chances of a boss with no row of its own.
    pub default: BookChances,
    /// Per-boss chances, by the boss's stable id.
    pub bosses: Vec<(String, BookChances)>,
    /// Every hero's books, indexed by [`Hero::index`].
    pub per_hero: [Vec<BookDrop>; 3],
}

impl BookRules {
    /// The chances for `boss`.
    #[must_use]
    pub fn chances_for(&self, boss: Option<&str>) -> BookChances {
        boss.and_then(|id| self.bosses.iter().find(|(name, _)| name == id))
            .map_or(self.default, |(_, chances)| *chances)
    }
}

/// **One boss-chest book roll** for a player of `hero` at `key_level`: at most one book.
///
/// A talent book is tried first, then an enhancement book; either needs a book of the hero's whose
/// key-level gate is met, and the book is picked uniformly among those (a hit with nothing eligible
/// falls through to the other kind). Draws exactly three numbers
/// from `rng` whatever the outcome.
pub fn roll_book(
    rules: &BookRules,
    boss: Option<&str>,
    hero: Hero,
    key_level: u32,
    rng: &mut Rng,
) -> Option<ItemId> {
    let chances = rules.chances_for(boss);
    let talent_hit = rng.chance(chances.talent);
    let enhancement_hit = rng.chance(chances.enhancement);
    let pick = rng.next_f32();
    let eligible_of = |kind: BookKind| -> Vec<&BookDrop> {
        rules.per_hero[hero.index()]
            .iter()
            .filter(|book| book.kind == kind && book.min_key_level <= key_level)
            .collect()
    };
    let mut eligible = if talent_hit {
        eligible_of(BookKind::TalentPoint)
    } else {
        Vec::new()
    };
    if eligible.is_empty() && enhancement_hit {
        eligible = eligible_of(BookKind::Enhancement);
    }
    if eligible.is_empty() {
        return None;
    }
    let index = ((pick * eligible.len() as f32) as usize).min(eligible.len() - 1);
    Some(eligible[index].item)
}

impl LootRules {
    /// The table of the boss with this id.
    #[must_use]
    pub fn boss_table(&self, boss: &str) -> Option<&LootTable> {
        self.boss_tables
            .iter()
            .find(|entry| entry.boss == boss)
            .map(|entry| &entry.table)
    }
}

/// One currency drop from a kill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrencyDrop {
    /// The resource's id.
    pub resource: &'static str,
    /// How many.
    pub amount: u32,
}

/// What a key level does to the quality odds, for a tooltip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityOdds {
    /// The mean quality, in per cent.
    pub mean: f32,
    /// The chance of 100 or better, `0..=1`.
    pub at_least_100: f32,
    /// The chance of the maximum, `0..=1`.
    pub jackpot: f32,
}

/// The weight of `bucket` at `key_level`.
fn bucket_weight(rules: &QualityRules, bucket: &QualityBucket, key_level: u32) -> f32 {
    let boost = if bucket.min >= rules.high_from {
        1.0 + rules.high_bucket_weight_per_key_level * key_level.saturating_sub(1) as f32
    } else {
        1.0
    };
    (bucket.weight * boost).max(0.0)
}

/// **Draws an item's quality**: a band by weight, then a whole percentage within it.
///
/// Always within `QUALITY_MIN..=QUALITY_MAX`, whatever the rules say. Low qualities are
/// common and the top is a jackpot; a higher `key_level` boosts the high bands.
pub fn roll_quality(rules: &LootRules, key_level: u32, rng: &mut Rng) -> u8 {
    let quality = &rules.quality;
    let weights: Vec<f32> = quality
        .buckets
        .iter()
        .map(|bucket| bucket_weight(quality, bucket, key_level))
        .collect();
    let total: f32 = weights.iter().sum();
    let band_roll = rng.next_f32();
    let offset_roll = rng.next_f32();
    if total <= 0.0 || quality.buckets.is_empty() {
        return QUALITY_MIN;
    }
    let mut target = band_roll * total;
    let mut chosen = quality.buckets.len() - 1;
    for (index, weight) in weights.iter().enumerate() {
        if target < *weight {
            chosen = index;
            break;
        }
        target -= weight;
    }
    let bucket = quality.buckets[chosen];
    let (low, high) = (bucket.min.min(bucket.max), bucket.max.max(bucket.min));
    let span = u32::from(high - low) + 1;
    let value = u32::from(low) + ((offset_roll * span as f32) as u32).min(span - 1);
    (value as u8).clamp(QUALITY_MIN, QUALITY_MAX)
}

/// The chance that an equipment roll at `key_level` is an epic, `0..=1`.
#[must_use]
pub fn epic_chance_at(rules: &LootRules, key_level: u32) -> f32 {
    let epic = &rules.epic;
    (epic.base_chance + epic.per_key_level * key_level.saturating_sub(1) as f32)
        .clamp(0.0, epic.max_chance.clamp(0.0, 1.0))
}

/// Whether an equipment roll at `key_level` is an epic.
pub fn roll_epic(rules: &LootRules, key_level: u32, rng: &mut Rng) -> bool {
    rng.chance(epic_chance_at(rules, key_level))
}

/// The quality odds at `key_level`, summarised: mean, chance of 100 or better, chance of the
/// maximum.
#[must_use]
pub fn quality_odds(rules: &LootRules, key_level: u32) -> QualityOdds {
    let quality = &rules.quality;
    let weights: Vec<f32> = quality
        .buckets
        .iter()
        .map(|bucket| bucket_weight(quality, bucket, key_level))
        .collect();
    let total: f32 = weights.iter().sum();
    if total <= 0.0 {
        return QualityOdds {
            mean: f32::from(QUALITY_MIN),
            at_least_100: 0.0,
            jackpot: 0.0,
        };
    }
    let (mut mean, mut at_least_100, mut jackpot) = (0.0, 0.0, 0.0);
    for (bucket, weight) in quality.buckets.iter().zip(&weights) {
        let share = weight / total;
        let (low, high) = (bucket.min.min(bucket.max), bucket.max.max(bucket.min));
        let count = f32::from(high - low) + 1.0;
        mean += share * (f32::from(low) + f32::from(high)) / 2.0;
        let at_or_above = if high >= 100 {
            f32::from(high - low.max(100)) + 1.0
        } else {
            0.0
        };
        at_least_100 += share * at_or_above / count;
        if high >= QUALITY_MAX {
            jackpot += share / count;
        }
    }
    QualityOdds {
        mean,
        at_least_100,
        jackpot,
    }
}

/// Picks one entry of `entries` by weight.
fn pick_weighted(entries: &[WeightedItem], rng: &mut Rng) -> Option<ItemId> {
    let total: f32 = entries.iter().map(|entry| entry.weight.max(0.0)).sum();
    let roll = rng.next_f32();
    if entries.is_empty() || total <= 0.0 {
        return entries.first().map(|entry| entry.item);
    }
    let mut target = roll * total;
    for entry in entries {
        let weight = entry.weight.max(0.0);
        if target < weight {
            return Some(entry.item);
        }
        target -= weight;
    }
    entries.last().map(|entry| entry.item)
}

/// **One equipment roll from `table`**: epic or not by [`roll_epic`], an item of that rarity by
/// weight, a quality by [`roll_quality`], then a sharpening level by
/// [`SharpeningDrops::roll`] for the drop's `source`, `dungeon` and key level. The item comes
/// back unbroken.
///
/// Draws exactly five numbers from `rng` (epic, pick, quality band, quality offset, sharpening)
/// whatever the outcome, so a roll's position in the sequence never depends on its result; the
/// sharpening draw is independent of which item was picked. `None` only for an empty table. A
/// table with no epics still gives its other items on an epic roll rather than nothing, and the
/// reverse.
pub fn roll_equipment(
    table: &LootTable,
    rules: &LootRules,
    source: DropSource,
    dungeon: &str,
    key_level: u32,
    rng: &mut Rng,
) -> Option<(ItemId, ItemProps)> {
    let epic = roll_epic(rules, key_level, rng);
    let mut pool = table.of_rarity(epic);
    if pool.is_empty() {
        pool = table.entries.clone();
    }
    let picked = pick_weighted(&pool, rng);
    let quality = roll_quality(rules, key_level, rng);
    let sharpening = rules.sharpening.roll(source, dungeon, key_level, rng);
    picked.map(|id| (id, ItemProps::new(quality, sharpening, false)))
}

/// The modifier-reroll currency one ordinary or elite kill drops for one player.
///
/// `bonus_pct` is the run's reward bonus: it multiplies the drop chance by `1 + bonus/100`.
/// Draws one number whatever the outcome.
pub fn roll_mob_currency(
    rules: &LootRules,
    elite: bool,
    bonus_pct: u32,
    rng: &mut Rng,
) -> Option<CurrencyDrop> {
    let base = if elite {
        rules.currency.elite_chance
    } else {
        rules.currency.per_mob_chance
    };
    let chance = (base * bonus_factor(bonus_pct)).min(1.0);
    rng.chance(chance).then_some(CurrencyDrop {
        resource: rules.currency.resource,
        amount: rules.currency.amount.max(1),
    })
}

/// The gold one kill pays one player: a whole number in `per_mob_min..=per_mob_max`, scaled by
/// the reward bonus. Draws one number. `None` when the rules pay no gold.
pub fn roll_mob_gold(rules: &LootRules, bonus_pct: u32, rng: &mut Rng) -> Option<CurrencyDrop> {
    let gold = &rules.gold;
    let (low, high) = (
        gold.per_mob_min.min(gold.per_mob_max),
        gold.per_mob_max.max(gold.per_mob_min),
    );
    let roll = rng.next_f32();
    let span = high - low + 1;
    let base = low + ((roll * span as f32) as u32).min(span - 1);
    let amount = scaled(base, bonus_pct);
    (amount > 0).then_some(CurrencyDrop {
        resource: gold.resource,
        amount,
    })
}

/// What the boss pays each player in gold, scaled by the reward bonus.
#[must_use]
pub fn boss_gold(rules: &LootRules, bonus_pct: u32) -> Option<CurrencyDrop> {
    let amount = scaled(rules.gold.boss_amount, bonus_pct);
    (amount > 0).then_some(CurrencyDrop {
        resource: rules.gold.resource,
        amount,
    })
}

/// `1 + bonus_pct / 100`.
fn bonus_factor(bonus_pct: u32) -> f32 {
    1.0 + bonus_pct as f32 / 100.0
}

/// `amount` raised by `bonus_pct` per cent, rounded to nearest.
fn scaled(amount: u32, bonus_pct: u32) -> u32 {
    (amount as f32 * bonus_factor(bonus_pct)).round() as u32
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::cast_precision_loss,
    reason = "test code may assert on known-good values"
)]
pub(crate) mod tests {
    use super::*;
    use crate::character::{CatalogItem, ItemFamily, ItemIcon, Stat, item_by_key};

    /// Registers a small catalog (two rares, two epics) once per test process and answers the
    /// ids, in order: rare ring, rare helm, epic ring, epic helm.
    pub(crate) fn test_items() -> [ItemId; 4] {
        let mk = |key: &str, family, quality, value| CatalogItem {
            key: key.to_owned(),
            name: key.to_owned(),
            family,
            quality,
            item_level: 10,
            icon: ItemIcon::of_family(family),
            stats: vec![(Stat::Stamina, value)],
            active: None,
            set: None,
        };
        crate::character::register_catalog_items(vec![
            mk("test_rare_ring", ItemFamily::Ring, ItemQuality::Rare, 10.0),
            mk("test_rare_helm", ItemFamily::Head, ItemQuality::Rare, 10.0),
            mk("test_epic_ring", ItemFamily::Ring, ItemQuality::Epic, 15.0),
            mk("test_epic_helm", ItemFamily::Head, ItemQuality::Epic, 15.0),
        ]);
        [
            "test_rare_ring",
            "test_rare_helm",
            "test_epic_ring",
            "test_epic_helm",
        ]
        .map(|key| item_by_key(key).expect("registered").id)
    }

    /// Rules shaped like the shipped catalog, over the test items.
    pub(crate) fn rules() -> LootRules {
        let [rare_ring, rare_helm, epic_ring, epic_helm] = test_items();
        let table = LootTable {
            entries: vec![
                WeightedItem {
                    item: rare_ring,
                    weight: 1.0,
                },
                WeightedItem {
                    item: rare_helm,
                    weight: 1.0,
                },
                WeightedItem {
                    item: epic_ring,
                    weight: 1.0,
                },
                WeightedItem {
                    item: epic_helm,
                    weight: 1.0,
                },
            ],
        };
        let bucket = |min, max, weight| QualityBucket { min, max, weight };
        LootRules {
            equipment_per_completion: 2,
            quality: QualityRules {
                buckets: vec![
                    bucket(50, 59, 22.0),
                    bucket(60, 69, 24.0),
                    bucket(70, 79, 22.0),
                    bucket(80, 89, 16.0),
                    bucket(90, 94, 7.0),
                    bucket(95, 99, 5.55),
                    bucket(100, 104, 2.5),
                    bucket(105, 109, 0.9),
                    bucket(110, 110, 0.05),
                ],
                high_from: 95,
                high_bucket_weight_per_key_level: 0.1,
            },
            epic: EpicRules {
                base_chance: 0.025,
                per_key_level: 0.002,
                max_chance: 0.08,
            },
            boss_tables: vec![BossTable {
                boss: "test_boss".to_owned(),
                table: table.clone(),
            }],
            mob: MobLootRules {
                equipment_chance: 0.002,
                table,
            },
            currency: CurrencyRules {
                resource: "aether_shard",
                per_mob_chance: 0.033,
                elite_chance: 0.25,
                amount: 1,
            },
            gold: GoldRules {
                resource: "gold",
                per_mob_min: 1,
                per_mob_max: 4,
                boss_amount: 50,
            },
            sharpening: SharpeningDrops::none(),
            books: None,
        }
    }

    fn book_rules(talent: f32, enhancement: f32) -> BookRules {
        let book = |id, kind, min_key_level| BookDrop {
            item: ItemId(id),
            kind,
            min_key_level,
        };
        BookRules {
            default: BookChances {
                talent,
                enhancement,
            },
            bosses: vec![(
                "quiet_boss".to_owned(),
                BookChances {
                    talent: 0.0,
                    enhancement: 0.0,
                },
            )],
            per_hero: [
                vec![],
                vec![
                    book(1, BookKind::TalentPoint, 8),
                    book(2, BookKind::TalentPoint, 12),
                    book(3, BookKind::Enhancement, 6),
                ],
                vec![],
            ],
        }
    }

    #[test]
    fn books_drop_by_class_kind_and_key_level_gate() {
        let rules = book_rules(1.0, 1.0);
        let roll = |hero, key| roll_book(&rules, Some("b"), hero, key, &mut Rng::new(7));
        assert_eq!(roll(Hero::Warrior, 5), None, "below every gate");
        assert_eq!(roll(Hero::Warrior, 6), Some(ItemId(3)), "falls through to enhancement");
        assert_eq!(roll(Hero::FireMage, 20), None, "no books for this class");
        let hit = roll(Hero::Warrior, 8).expect("a talent book");
        assert!(hit == ItemId(1));
        let both = roll(Hero::Warrior, 12).expect("a talent book");
        assert!(both == ItemId(1) || both == ItemId(2));
        let only_enhancement = book_rules(0.0, 1.0);
        let got = roll_book(&only_enhancement, None, Hero::Warrior, 6, &mut Rng::new(1));
        assert_eq!(got, Some(ItemId(3)));
    }

    #[test]
    fn book_rolls_are_deterministic_and_use_the_boss_row() {
        let rules = book_rules(0.3, 0.3);
        let run = |seed| {
            let mut rng = Rng::new(seed);
            (0..200)
                .map(|_| roll_book(&rules, Some("b"), Hero::Warrior, 10, &mut rng))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(42), run(42));
        let drops = run(42).iter().flatten().count();
        assert!((70..=135).contains(&drops), "{drops} of 200 at about 51%");
        let mut rng = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(
                roll_book(&rules, Some("quiet_boss"), Hero::Warrior, 20, &mut rng),
                None
            );
        }
    }

    #[test]
    fn quality_is_always_within_the_legal_range() {
        let rules = rules();
        let mut rng = Rng::new(7);
        for level in [1, 5, 20, 1000] {
            for _ in 0..20_000 {
                let quality = roll_quality(&rules, level, &mut rng);
                assert!((QUALITY_MIN..=QUALITY_MAX).contains(&quality), "{quality}");
            }
        }
    }

    #[test]
    fn low_quality_is_common_and_the_top_is_a_jackpot() {
        let rules = rules();
        let mut rng = Rng::new(11);
        let n = 100_000_u32;
        let rolls: Vec<u8> = (0..n).map(|_| roll_quality(&rules, 1, &mut rng)).collect();
        let mean = rolls.iter().map(|q| f32::from(*q)).sum::<f32>() / n as f32;
        assert!(
            mean < 80.0,
            "the mean {mean} is not in the lower half of 50..=110"
        );
        let high = rolls.iter().filter(|q| **q >= 100).count() as f32 / n as f32;
        assert!(high < 0.06, "{high} of rolls were 100 or better");
        let jackpots = rolls.iter().filter(|q| **q == QUALITY_MAX).count() as f32 / n as f32;
        assert!(jackpots < 0.003, "{jackpots} of rolls were the maximum");
    }

    #[test]
    fn a_higher_key_level_raises_the_share_of_high_quality() {
        let rules = rules();
        let share = |level: u32| {
            let mut rng = Rng::new(99);
            let n = 50_000_u32;
            (0..n)
                .filter(|_| roll_quality(&rules, level, &mut rng) >= 100)
                .count() as f32
                / n as f32
        };
        assert!(share(20) > share(1) * 1.4, "{} vs {}", share(20), share(1));
        // The analytic summary agrees with the direction, and with the sampled share.
        let (low, high) = (quality_odds(&rules, 1), quality_odds(&rules, 20));
        assert!(high.at_least_100 > low.at_least_100);
        assert!(high.mean > low.mean);
        assert!((share(1) - low.at_least_100).abs() < 0.01);
        assert!((share(20) - high.at_least_100).abs() < 0.01);
        assert!(low.jackpot > 0.0 && low.jackpot < 0.002);
    }

    #[test]
    fn the_epic_rate_tracks_the_configured_chance_and_rises_with_level() {
        let rules = rules();
        let rate = |level: u32| {
            let mut rng = Rng::new(5);
            let n = 200_000_u32;
            (0..n)
                .filter(|_| roll_epic(&rules, level, &mut rng))
                .count() as f32
                / n as f32
        };
        assert!((rate(1) - 0.025).abs() < 0.003, "{}", rate(1));
        assert!((rate(11) - 0.045).abs() < 0.004, "{}", rate(11));
        assert!(rate(11) > rate(1));
        // Capped.
        assert!((epic_chance_at(&rules, 100) - 0.08).abs() < f32::EPSILON);
        assert!((epic_chance_at(&rules, 1) - 0.025).abs() < f32::EPSILON);
        assert!((epic_chance_at(&rules, 0) - 0.025).abs() < f32::EPSILON);
    }

    #[test]
    fn an_equipment_roll_picks_by_rarity_then_rolls_quality() {
        let rules = rules();
        let [rare_ring, rare_helm, epic_ring, epic_helm] = test_items();
        let table = &rules.boss_tables[0].table;
        let mut rng = Rng::new(3);
        let (mut epics, n) = (0_u32, 40_000_u32);
        for _ in 0..n {
            let (id, props) = roll_equipment(table, &rules, DropSource::Chest, "", 1, &mut rng)
                .expect("a non-empty table");
            assert!([rare_ring, rare_helm, epic_ring, epic_helm].contains(&id));
            assert!((QUALITY_MIN..=QUALITY_MAX).contains(&props.quality));
            assert_eq!((props.sharpening, props.broken), (0, false));
            epics += u32::from(item(id).expect("known").quality == ItemQuality::Epic);
        }
        let rate = epics as f32 / n as f32;
        assert!((rate - 0.025).abs() < 0.006, "{rate}");
        assert!(
            roll_equipment(
                &LootTable::default(),
                &rules,
                DropSource::Chest,
                "",
                1,
                &mut rng
            )
            .is_none()
        );
    }

    #[test]
    fn a_table_without_epics_still_gives_something_on_an_epic_roll() {
        let mut rules = rules();
        rules.epic.base_chance = 1.0;
        rules.epic.max_chance = 1.0;
        let [rare_ring, ..] = test_items();
        let table = LootTable {
            entries: vec![WeightedItem {
                item: rare_ring,
                weight: 1.0,
            }],
        };
        let mut rng = Rng::new(1);
        assert_eq!(
            roll_equipment(&table, &rules, DropSource::Chest, "", 1, &mut rng).map(|(id, _)| id),
            Some(rare_ring)
        );
    }

    #[test]
    fn the_same_seed_rolls_the_same_loot() {
        let rules = rules();
        let table = &rules.boss_tables[0].table;
        let run = |seed| {
            let mut rng = Rng::new(seed);
            (0..200)
                .map(|_| roll_equipment(table, &rules, DropSource::Chest, "", 7, &mut rng))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(42), run(42));
        assert_ne!(run(42), run(43));
    }

    #[test]
    fn mob_currency_follows_its_chances_and_the_reward_bonus() {
        let rules = rules();
        let count = |elite: bool, bonus: u32| {
            let mut rng = Rng::new(21);
            let n = 100_000_u32;
            (0..n)
                .filter(|_| roll_mob_currency(&rules, elite, bonus, &mut rng).is_some())
                .count() as f32
                / n as f32
        };
        assert!((count(false, 0) - 0.033).abs() < 0.004);
        assert!((count(true, 0) - 0.25).abs() < 0.01);
        assert!((count(false, 100) - 0.066).abs() < 0.006);
        let mut rng = Rng::new(2);
        for _ in 0..1_000 {
            let gold = roll_mob_gold(&rules, 0, &mut rng).expect("gold pays");
            assert!((1..=4).contains(&gold.amount));
            assert_eq!(gold.resource, "gold");
        }
        assert_eq!(boss_gold(&rules, 0).map(|g| g.amount), Some(50));
        assert_eq!(boss_gold(&rules, 25).map(|g| g.amount), Some(63));
    }
}
