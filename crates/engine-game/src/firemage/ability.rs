//! FireMage's abilities, as data, plus the runtime that cools them down.
//!
//! ```text
//! FireAbility (which)  ->  AbilityDef (numbers)  ->  AbilityBook (cooldowns and charges)
//! ```
//!
//! [`ABILITIES`] is a table: cast times, cooldowns, charges, ranges, areas, Heat and the
//! spell-power coefficients all live in one place a designer edits. Nothing in the executor,
//! the simulator, the UI or the tooltips restates any of them, and the tooltip text is
//! *generated* from the same rows so a description can never drift from the ability it
//! describes.
//!
//! Damage is a **coefficient of spell power**, not a flat number: `2.60` is "260 % spell
//! power". Base spell power is a hundred ([`super::stats::BASE_SPELL_POWER`]), so a
//! coefficient reads directly as damage at base gear and scales with everything later.
//!
//! Charges and cooldowns are one system: an ability with one charge is the ordinary case,
//! and an ability with two is the same rules with a bigger bucket.

use crate::combat::DamageType;
use crate::druid::tuning::{
    LIFEBLOOM_MANA, MOONFIRE_MANA, NATURES_CLEANSE_MANA, NOURISH_MANA, REGROWTH_MANA,
    REJUVENATION_MANA, SUNFIRE_MANA, SWIFTMEND_MANA, TRANQUILITY_MANA, WILD_GROWTH_MANA,
};

use super::stats::Stats;
use super::talent::TalentEffects;
use crate::hero::Hero;

/// Abilities on the regular bar.
pub const ABILITY_SLOTS: usize = 10;

/// Every ability of every hero, including the Ultimate and the passive row Ignite is
/// credited to.
///
/// One table for all three heroes rather than one each: see [`crate::hero`] for why a class
/// is a slice of this table and not a system of its own.
pub const ABILITY_COUNT: usize = 37;

/// The furthest anything the class has can reach, in metres.
///
/// What "in range" means for *selection*, as opposed to for one particular spell. A target
/// further away than this cannot be hit by anything on the bar, so putting it in the cycle
/// is offering the player something to select and then be refused for.
///
/// Derived from the table rather than written down, so an ability with a longer reach extends
/// the cycle by being added rather than by anybody remembering to change a second number.
#[must_use]
pub fn longest_range() -> f32 {
    ABILITIES
        .iter()
        .map(|definition| definition.range)
        .fold(0.0, f32::max)
}

/// Whether the Ultimate can be used.
///
/// **False for this iteration.** FireMage has no Ultimate concept yet, so the button is
/// inert and the cast is refused - but the ability, its energy cost, the resource that pays
/// for it and the whole targeting path stay exactly where they are. Turning the Ultimate
/// back on is this one constant.
pub const ULTIMATE_ENABLED: bool = false;

/// Which ability. The first ten are the bar, in bar order; the eleventh is the Ultimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FireAbility {
    /// The filler: a short cast that keeps Ignite going.
    Firebolt,
    /// The primary single-target Ignite generator.
    FlameLance,
    /// A short frontal cone that spreads Ignite across a pack.
    FlameCone,
    /// A ground-targeted rock that is paid off by an existing burn.
    Meteor,
    /// The big cast, paid off by a full burn.
    Pyroblast,
    /// A line of fire that burns for six seconds.
    Firestorm,
    /// The payoff: converts a full burn into an explosion.
    Detonate,
    /// The specialisation cooldown: haste or critical strike for twenty seconds.
    FireMastery,
    /// A defensive cooldown.
    FlameShield,
    /// A short teleport.
    Blink,
    /// The Ultimate. Not on the bar, and disabled this iteration.
    Cataclysm,
    /// Warrior: the filler. Attacks, and stacks a heal on the caster.
    Recuperate,
    /// Warrior: a targeted 120-degree cleave that slows what it catches.
    WarStomp,
    /// Warrior: a ground smash that leaves a bleed scaled from the Warrior's own health.
    ColossusStrike,
    /// Warrior: a real, animated charge that ends in an area hit and a short stun.
    ShieldCharge,
    /// Warrior: the interrupt.
    ShieldBash,
    /// Warrior: six seconds of Block, and no critical strikes taken.
    ShieldBlock,
    /// Warrior: trades current health for an absorb built from maximum health.
    PunctureBlood,
    /// Warrior: banks a share of the damage taken and gives it back as healing.
    Adrenaline,
    /// Warrior: ten seconds where every defensive event heals.
    BattleTrance,
    /// Warrior: six seconds where enemy spells go back where they came from.
    SpellReflect,
    /// Warrior: forces one enemy to attack the Warrior for a few seconds.
    Taunt,
    /// Warrior: the emergency cooldown. More maximum health, a heal, and less damage taken.
    LastStand,
    /// Druid: the instant that leaves moonlight burning on one enemy.
    Moonfire,
    /// Druid: the same burn, spread across a pack.
    Sunfire,
    /// Druid: the free filler, and the debuff every cast of it stacks.
    Wrath,
    /// Druid: the instant heal over time.
    Rejuvenation,
    /// Druid: a large direct heal and the heal over time that follows it.
    Regrowth,
    /// Druid: a heal over time that blooms when it runs out.
    Lifebloom,
    /// Druid: the party heal over time, on a cooldown.
    WildGrowth,
    /// Druid: eats one of its own heals over time for a large instant heal.
    Swiftmend,
    /// Druid: a direct heal that is larger the more of the Druid's heals are already on the
    /// target.
    Nourish,
    /// Druid: the dispel.
    NaturesCleanse,
    /// Druid: the movement and damage-reduction cooldown.
    WildRush,
    /// Druid: the resource button. Arms the next compatible spell.
    AstralPower,
    /// Druid: the Ultimate. A channelled party heal.
    Tranquility,
    /// Not an ability: the row the burn's damage is credited to.
    ///
    /// Ignite is the class's largest single source of damage and it comes from no cast, so
    /// it needs a name of its own in the meter and in the simulator's report. It has a
    /// definition so tooltips can read it, and it can never be cast.
    Ignite,
}

impl FireAbility {
    /// Every ability, in table order.
    pub const ALL: [Self; ABILITY_COUNT] = [
        Self::Firebolt,
        Self::FlameLance,
        Self::FlameCone,
        Self::Meteor,
        Self::Pyroblast,
        Self::Firestorm,
        Self::Detonate,
        Self::FireMastery,
        Self::FlameShield,
        Self::Blink,
        Self::Cataclysm,
        Self::Recuperate,
        Self::WarStomp,
        Self::ColossusStrike,
        Self::ShieldCharge,
        Self::ShieldBash,
        Self::ShieldBlock,
        Self::PunctureBlood,
        Self::Adrenaline,
        Self::BattleTrance,
        Self::SpellReflect,
        Self::Taunt,
        Self::LastStand,
        Self::Moonfire,
        Self::Sunfire,
        Self::Wrath,
        Self::Rejuvenation,
        Self::Regrowth,
        Self::Lifebloom,
        Self::WildGrowth,
        Self::Swiftmend,
        Self::Nourish,
        Self::NaturesCleanse,
        Self::WildRush,
        Self::AstralPower,
        Self::Tranquility,
        Self::Ignite,
    ];

    /// Dense index into the ability arrays.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Which hero casts it.
    #[must_use]
    pub fn hero(self) -> Hero {
        self.definition().hero
    }

    /// Which slot of *its own hero's* bar this ability sits in, or `None` for the Ultimate
    /// and for Ignite.
    ///
    /// Derived from the owning hero rather than from a global list, and unambiguous because
    /// no two heroes share an ability — the property
    /// `every_castable_ability_belongs_to_exactly_one_heros_bar` pins in [`crate::hero`].
    /// It is what lets everything that draws a button keep asking the ability where it goes,
    /// with no idea that a second hero exists.
    #[must_use]
    pub fn bar_slot(self) -> Option<usize> {
        self.hero()
            .abilities()
            .iter()
            .position(|ability| *ability == self)
            .filter(|slot| *slot < ABILITY_SLOTS)
    }

    /// Whether this is its hero's Ultimate, which lives outside the bar on the `R` key.
    ///
    /// Asked of the owning hero rather than written down here, so the one place that
    /// decides which ability sits beside the bar is [`Hero::ultimate`]. The FireMage's is
    /// Cataclysm and is switched off ([`ULTIMATE_ENABLED`]); the Warrior's is Last Stand and
    /// is a real button, which is what lets an eleven-ability class fit a ten-key bar
    /// without renumbering the keyboard for the other two heroes.
    #[must_use]
    pub fn is_ultimate(self) -> bool {
        self.hero().ultimate() == Some(self)
    }

    /// Whether this ability is disabled outright, whatever its cooldown says.
    ///
    /// One ability is: Cataclysm, which has no design yet. Kept as a question about the
    /// ability rather than about "the Ultimate", because the Ultimate *slot* now holds a
    /// working ability for the Warrior.
    #[must_use]
    pub fn is_disabled(self) -> bool {
        matches!(self, Self::Cataclysm) && !ULTIMATE_ENABLED
    }

    /// Whether this is a row in the meter rather than something that can be cast.
    #[must_use]
    pub fn is_passive(self) -> bool {
        matches!(self, Self::Ignite)
    }

    /// Whether this ability can still be used while a friendly target is selected.
    ///
    /// The per-ability form of [`TargetMode::usable_on_friendly_target`], which is where the
    /// rule and the reasoning live. What the interface asks so it can dim a button the
    /// player is about to be refused for.
    #[must_use]
    pub fn usable_on_friendly_target(self) -> bool {
        self.definition().target.usable_on_friendly_target()
    }

    /// This ability's definition.
    #[must_use]
    pub fn definition(self) -> &'static AbilityDef {
        &ABILITIES[self.index()]
    }

    /// This ability's stable written name: what a configuration file stores.
    ///
    /// Lowercase and stable across builds, and deliberately not
    /// [`AbilityDef::name`](crate::firemage::AbilityDef::name): that one is an uppercase
    /// display string the interface prints, and renaming an ability on screen must not
    /// silently empty every action bar slot holding it. It is not the discriminant either,
    /// for the same reason reordering this enum must not rebind anything.
    #[must_use]
    pub fn config_name(self) -> &'static str {
        match self {
            Self::Firebolt => "firebolt",
            Self::FlameLance => "flame_lance",
            Self::FlameCone => "flame_cone",
            Self::Meteor => "meteor",
            Self::Pyroblast => "pyroblast",
            Self::Firestorm => "firestorm",
            Self::Detonate => "detonate",
            Self::FireMastery => "fire_mastery",
            Self::FlameShield => "flame_shield",
            Self::Blink => "blink",
            Self::Cataclysm => "cataclysm",
            Self::Recuperate => "recuperate",
            Self::WarStomp => "war_stomp",
            Self::ColossusStrike => "colossus_strike",
            Self::ShieldCharge => "shield_charge",
            Self::ShieldBash => "shield_bash",
            Self::ShieldBlock => "shield_block",
            Self::PunctureBlood => "puncture_blood",
            Self::Adrenaline => "adrenaline",
            Self::BattleTrance => "battle_trance",
            Self::SpellReflect => "spell_reflect",
            Self::Taunt => "taunt",
            Self::LastStand => "last_stand",
            Self::Moonfire => "moonfire",
            Self::Sunfire => "sunfire",
            Self::Wrath => "wrath",
            Self::Rejuvenation => "rejuvenation",
            Self::Regrowth => "regrowth",
            Self::Lifebloom => "lifebloom",
            Self::WildGrowth => "wild_growth",
            Self::Swiftmend => "swiftmend",
            Self::Nourish => "nourish",
            Self::NaturesCleanse => "natures_cleanse",
            Self::WildRush => "wild_rush",
            Self::AstralPower => "astral_power",
            Self::Tranquility => "tranquility",
            Self::Ignite => "ignite",
        }
    }

    /// The ability with this written name, if there is one.
    ///
    /// Total: a name from a newer build, or a typo in a hand-edited file, gives `None`
    /// rather than a guess, and the slot that asked for it stays empty.
    #[must_use]
    pub fn from_config_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|ability| ability.config_name() == name)
    }

    /// Whether the player can put this in an action bar slot and cast it.
    ///
    /// Ignite is a row in the damage meter rather than a button, and the Ultimate has a
    /// place of its own beside the bar. Everything else is assignable, which is exactly the
    /// list the spell picker offers.
    #[must_use]
    pub fn is_assignable(self) -> bool {
        !self.is_passive() && !self.is_ultimate()
    }

    /// The key that casts it: 1-0 across the bar, R for the Ultimate.
    #[must_use]
    pub fn key_hint(self) -> &'static str {
        if let Some(slot) = self.bar_slot() {
            return ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"][slot];
        }
        // Beside the bar: the Ultimate on `R`, and a hero with more abilities than keys puts
        // the rest on the two buttons next to it. Asked of the hero rather than written down
        // per ability, for the reason `bar_slot` is.
        match self.hero().extra_index(self) {
            Some(index) => ["F", "G"][index.min(1)],
            None => "R",
        }
    }
}

/// How an ability picks what it hits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TargetMode {
    /// The current target, within `range`.
    Single,
    /// A cone of `range` metres in front of the caster.
    Cone,
    /// A cone of `range` metres, pointed at the **current target**.
    ///
    /// The player picks an enemy; the game decides the direction from that, and everything
    /// inside [`TARGET_CLEAVE_HALF_ANGLE`] of it is hit. Deliberately not a freeform aimed
    /// cone: War Stomp is pressed in a pack while the camera is somewhere else, and an
    /// ability that has to be drawn before it fires is an ability that cannot be pressed
    /// every four seconds.
    TargetCone,
    /// A circle the player places with the mouse.
    Ground,
    /// A rectangle aimed from the caster, `range` long and `width` wide.
    Line,
    /// A friendly target: the selection when it is one, and the caster when it is not.
    ///
    /// **Never refused for the selection.** A healer with an enemy selected is a healer
    /// mid-fight, and a heal that answered "THAT IS NOT AN ALLY" would be a heal that is
    /// unusable exactly when it is needed. The fallback is the caster, which is what every
    /// game in the genre does.
    ///
    /// With a radius it is an area heal, centred on whoever it resolved to.
    Friendly,
    /// The caster only.
    SelfCast,
    /// A movement, not an attack.
    Movement,
}

impl TargetMode {
    /// Whether the player has to have something selected.
    #[must_use]
    pub fn needs_target(self) -> bool {
        matches!(self, Self::Single | Self::TargetCone)
    }

    /// Whether the player places this ability with the mouse.
    #[must_use]
    pub fn is_ground_targeted(self) -> bool {
        matches!(self, Self::Ground)
    }

    /// Whether the player aims this ability with the mouse before it is cast.
    ///
    /// A placed ability picks a position, a cone and a line pick a direction; all three show
    /// a preview and wait for a click, so all three must be kept off the key that would
    /// otherwise cast them outright.
    #[must_use]
    pub fn is_aimed(self) -> bool {
        matches!(self, Self::Ground | Self::Cone | Self::Line)
    }

    /// Whether an ability aimed this way can still be used while a friendly target — an
    /// ally, or the player themselves — is selected.
    ///
    /// Every mode except [`TargetMode::Single`] can, and each for its own reason:
    ///
    /// ```text
    /// Single    no   it is pointed at the selection, and the selection is not an enemy
    /// TargetCone no  likewise: the cone is aimed by the selection
    /// Cone      yes  aimed with the mouse; the selection is not consulted
    /// Ground    yes  the key opens the placement circle rather than casting
    /// Line      yes  placed, like Ground
    /// SelfCast  yes  it is used on the caster whatever is selected
    /// Movement  yes  it moves the caster and touches nobody
    /// ```
    ///
    /// Derived from the mode rather than written down per ability. A second column saying
    /// "usable on an ally" would be a fact that can contradict the mode beside it, and the
    /// mode is already the thing that decides who an ability is pointed at. A test pins the
    /// per-ability answers so the derivation cannot quietly change what the bar does.
    #[must_use]
    pub fn usable_on_friendly_target(self) -> bool {
        !matches!(self, Self::Single | Self::TargetCone)
    }

    /// Whether this ability is pointed at an ally rather than at an enemy.
    #[must_use]
    pub fn is_friendly(self) -> bool {
        matches!(self, Self::Friendly)
    }
}

/// Half the angle of a target-pointed cleave, in radians.
///
/// A third of pi: sixty degrees either side of the line to the target, which is the
/// hundred-and-twenty-degree cleave War Stomp is specified as. Derived from the target mode
/// rather than carried as a column, because there is exactly one kind of cleave and a second
/// number per row would be a second thing to disagree with the mode beside it.
pub const TARGET_CLEAVE_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_3;

/// What one point of an ability's damage or healing is worth in an enemy's threat table.
///
/// A column on every row rather than a rule about the caster, so a tank's threat tool, a
/// damage dealer's quiet spell and a healer's big heal are each tuned where they are
/// defined. The three presets are the role defaults:
///
/// ```text
///                  per damage   per healing
/// DAMAGE_DEALER       1.0          0.5
/// TANK                2.0          0.5
/// HEALER              1.0          0.5
/// ```
///
/// Healing threat lands on every enemy already fighting whoever was healed — see
/// [`crate::enemy::threat`] for the rules the table applies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThreatScale {
    /// Threat per point of damage dealt.
    pub damage: f32,
    /// Threat per point of healing actually restored. Overhealing generates none.
    pub healing: f32,
}

impl ThreatScale {
    /// One threat a point of damage: the FireMage.
    pub const DAMAGE_DEALER: Self = Self {
        damage: 1.0,
        healing: 0.5,
    };

    /// Two a point: the Warrior, whose whole job is to be the one the enemy wants.
    pub const TANK: Self = Self {
        damage: 2.0,
        healing: 0.5,
    };

    /// Half a point of threat a point of healing: the Druid.
    pub const HEALER: Self = Self {
        damage: 1.0,
        healing: 0.5,
    };

    /// The same scale with a different damage rate: a threat tool.
    #[must_use]
    pub const fn with_damage(self, damage: f32) -> Self {
        Self {
            damage,
            healing: self.healing,
        }
    }
}

/// The Warrior's columns of the ability table.
///
/// Nested rather than eight more fields on every row, so the eighteen rows that are not the
/// Warrior's say [`WarriorRow::NONE`] once instead of repeating eight zeroes. The
/// **magnitudes** of what twenty Deflection buys are not here: they differ in kind from one
/// ability to the next — a silence, five per cent of Block, half again on a bleed — and they
/// live as named constants in [`crate::warrior::tuning`], which is one place a designer
/// reads rather than a column that would be a different quantity in every row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarriorRow {
    /// Deflection this ability spends to be empowered, or zero if it has no empowered form.
    ///
    /// Twenty for every spender in the kit, as one number rather than nine.
    pub deflection_cost: f32,
    /// Impact damage as a fraction of the **caster's current** maximum health.
    ///
    /// Added to whatever [`AbilityDef::coefficient`] produced. `0.01` is "and 1 % of your
    /// maximum health".
    pub health_coefficient: f32,
    /// One periodic tick as a fraction of the caster's maximum health **at the moment the
    /// tick happens**.
    ///
    /// Never snapshotted. The bleed a Colossus Strike leaves reads the Warrior's maximum
    /// health on every tick, so a Last Stand cast after the bleed was applied makes the rest
    /// of that bleed hit harder — see [`crate::status::Status::source_health_per_tick`].
    pub tick_health_coefficient: f32,
}

impl WarriorRow {
    /// What a row that is not the Warrior's says.
    pub const NONE: Self = Self {
        deflection_cost: 0.0,
        health_coefficient: 0.0,
        tick_health_coefficient: 0.0,
    };

    /// Whether twenty Deflection buys this ability anything.
    #[must_use]
    pub const fn is_empowerable(&self) -> bool {
        self.deflection_cost > 0.0
    }
}

/// The Druid's columns of the ability table.
///
/// Nested for the same reason [`WarriorRow`] is: the rows that are not the Druid's say
/// [`DruidRow::NONE`] once instead of repeating four zeroes. What Astral Power *does* to an
/// ability is not here — a fifty per cent larger Moonfire and an indefinite one are
/// different quantities from a thirty per cent larger heal, and they live as named constants
/// in [`crate::druid::tuning`], which is the one place a designer reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DruidRow {
    /// Mana one cast costs. Zero for Wrath, which is the whole point of Wrath.
    pub mana_cost: f32,
    /// Astral Power one successful cast generates.
    ///
    /// The whole of "how Astral Power is generated", as a column rather than a rule, so the
    /// rate is tuned per ability without touching the resource.
    pub astral_generation: f32,
    /// Whether an armed Astral Power is spent on this ability and empowers it.
    ///
    /// False for Nature's Cleanse, Wild Rush and the Astral Power button itself, which is
    /// exactly the exclusion list the design states.
    pub astral_empowerable: bool,
    /// Healing paid when this ability's own effect expires naturally, as a fraction of spell
    /// power. Lifebloom's bloom, and nothing else.
    pub bloom_coefficient: f32,
}

impl DruidRow {
    /// What a row that is not the Druid's says.
    pub const NONE: Self = Self {
        mana_cost: 0.0,
        astral_generation: 0.0,
        astral_empowerable: false,
        bloom_coefficient: 0.0,
    };
}

/// One ability, entirely as data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityDef {
    /// Which ability this describes.
    pub id: FireAbility,
    /// Which hero casts it.
    ///
    /// The whole of "which class an ability belongs to". Nothing branches on it: it decides
    /// which bar the row appears on and nothing else.
    pub hero: Hero,
    /// What the damage it deals counts as.
    ///
    /// A column rather than a rule about the caster: a Druid's stars are arcane and a
    /// Warrior's swing is physical because their rows say so, which is the same reason the
    /// mage's are fire.
    pub damage_type: DamageType,
    /// Uppercase name; the UI font has no lowercase glyphs.
    pub name: &'static str,
    /// One line of what it does, for the tooltip.
    pub description: &'static str,
    /// How it picks what it hits.
    pub target: TargetMode,
    /// Seconds of casting before it lands. Zero is instant.
    pub cast_time: f32,
    /// Seconds before a charge comes back.
    pub cooldown: f32,
    /// How many charges it holds. One is the ordinary case.
    pub charges: u8,
    /// Metres it reaches. For a cone or a line, how far it extends.
    pub range: f32,
    /// Metres of effect radius, or zero if it hits one thing.
    pub radius: f32,
    /// Metres across, for a line.
    pub width: f32,
    /// Seconds the effect lasts after it lands, or zero if it is instant.
    pub duration: f32,
    /// Seconds between the cast landing and the effect, for Meteor and the Ultimate.
    pub impact_delay: f32,
    /// Impact damage as a fraction of spell power. `2.6` is 260 %.
    pub coefficient: f32,
    /// Damage each periodic tick deals, as a fraction of spell power.
    pub tick_coefficient: f32,
    /// Seconds between periodic ticks.
    pub tick_interval: f32,
    /// Extra damage per Ignite stack on the target. `0.10` is +10 % a stack.
    pub ignite_bonus_per_stack: f32,
    /// Ignite stacks applied to everything hit.
    pub ignite_stacks: u8,
    /// Chance those stacks are applied. One means always.
    pub ignite_chance: f32,
    /// Heat generated by one cast.
    pub heat: f32,
    /// Ultimate Energy generated by one cast.
    pub energy: f32,
    /// Ultimate Energy it costs. Only the Ultimate costs any.
    pub energy_cost: f32,
    /// Whether the caster must have an unobstructed line to the target.
    ///
    /// **Opt-in, and never inferred.** An ability that does not set this casts no ray at
    /// all, which is both the backwards-compatible answer and the reason the check costs
    /// nothing for the abilities that do not want it. Only meaningful together with
    /// [`TargetMode::Single`]: an aimed, placed, self or movement ability has no target to
    /// have a line to, and [`AbilityDef::checks_line_of_sight`] is where the two are
    /// combined.
    ///
    /// See `line_of_sight.md` for which abilities set it and why.
    pub requires_line_of_sight: bool,
    /// How much threat each point of what it does is worth.
    pub threat: ThreatScale,
    /// The Warrior's columns: what it spends and how it reads health.
    pub warrior: WarriorRow,
    /// The Druid's columns: what it costs, what it generates, what Astral Power may do to
    /// it.
    pub druid: DruidRow,
    /// Whether haste shortens its cooldown.
    ///
    /// True for everything the player *casts*, because going faster is what haste is. False
    /// for Detonate: that ability cashes in a bank whose size is set by Ignite's duration,
    /// and a haste character already fills the bank faster. Letting haste also spend it more
    /// often is the same stat paid twice, and at a geared 45 % it was worth +23 % on a single
    /// target against a balance goal of +5 to +10.
    pub cooldown_hasted: bool,
}

impl AbilityDef {
    /// Whether casting this actually runs a line-of-sight check.
    ///
    /// The two conditions together, in one place: it must be pointed at a target, and it
    /// must have asked. Everything that decides whether to cast a ray asks this rather than
    /// reading the flag, so a `requires_line_of_sight` accidentally set on a ground ability
    /// cannot start firing rays at nothing.
    #[must_use]
    pub fn checks_line_of_sight(&self) -> bool {
        self.requires_line_of_sight && self.target.needs_target()
    }

    /// Whether it can hit more than one thing.
    #[must_use]
    pub fn is_area(&self) -> bool {
        self.radius > 0.0
            || matches!(
                self.target,
                TargetMode::Cone | TargetMode::Line | TargetMode::TargetCone
            )
    }

    /// Whether it holds more than one charge, with talents applied.
    #[must_use]
    pub fn has_charges(&self, effects: &TalentEffects) -> bool {
        self.charges_with(effects) > 1
    }

    /// Its cast time at the current stats.
    #[must_use]
    pub fn cast_time_with(&self, stats: &Stats) -> f32 {
        stats.cast_time(self.cast_time)
    }

    /// Its cooldown with talents applied.
    #[must_use]
    pub fn cooldown_with(&self, _effects: &TalentEffects) -> f32 {
        self.cooldown
    }

    /// How many charges it holds with talents applied.
    #[must_use]
    pub fn charges_with(&self, effects: &TalentEffects) -> u8 {
        match self.id {
            FireAbility::Pyroblast => self.charges + effects.pyroblast_extra_charges,
            _ => self.charges,
        }
    }

    /// Its impact damage at `stats`, before Heat, crits and Ignite scaling.
    #[must_use]
    pub fn base_damage(&self, stats: &Stats) -> f32 {
        self.coefficient * stats.spell_power
    }

    /// One periodic tick's damage at `stats`, before Heat and crits.
    #[must_use]
    pub fn base_tick_damage(&self, stats: &Stats) -> f32 {
        self.tick_coefficient * stats.spell_power
    }

    /// Its impact damage at `stats` **including** what it takes from maximum health.
    ///
    /// The one entry point for an ability that scales from both. `max_health` is the
    /// caster's current maximum, read at the moment of the hit; nothing stores it.
    #[must_use]
    pub fn base_damage_with_health(&self, stats: &Stats, max_health: f32) -> f32 {
        self.base_damage(stats) + self.warrior.health_coefficient * max_health.max(0.0)
    }

    /// One periodic tick at `stats` including its share of `max_health`.
    #[must_use]
    pub fn base_tick_damage_with_health(&self, stats: &Stats, max_health: f32) -> f32 {
        self.base_tick_damage(stats) + self.warrior.tick_health_coefficient * max_health.max(0.0)
    }
}

/// The whole class, in bar order. The Ultimate is last.
pub static ABILITIES: [AbilityDef; ABILITY_COUNT] = [
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::Firebolt,
        name: "FIREBOLT",
        description: "FILLER. ALWAYS IGNITES AN UNBURNT TARGET, 50% CHANCE OTHERWISE",
        target: TargetMode::Single,
        cast_time: 1.0,
        cooldown: 0.0,
        charges: 1,
        range: 35.0,
        radius: 0.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 1.00,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 1,
        ignite_chance: 0.50,
        heat: 2.0,
        energy: 1.0,
        requires_line_of_sight: true,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::FlameLance,
        name: "FLAME LANCE",
        description: "APPLIES 2 IGNITE. A CRITICAL STRIKE REDUCES COOLDOWN OF PYROBLAST BY 2 SECONDS",
        target: TargetMode::Single,
        cast_time: 1.5,
        cooldown: 8.0,
        charges: 2,
        range: 35.0,
        radius: 0.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 2.60,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 2,
        ignite_chance: 1.0,
        heat: 4.0,
        energy: 2.0,
        requires_line_of_sight: true,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::FlameCone,
        name: "FLAME CONE",
        description: "A CONE OF FIRE. APPLIES 2 IGNITE TO EVERY ENEMY HIT",
        target: TargetMode::Cone,
        cast_time: 0.0,
        cooldown: 10.0,
        charges: 1,
        range: 12.0,
        radius: 0.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 1.40,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 2,
        ignite_chance: 1.0,
        heat: 3.0,
        energy: 2.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::Meteor,
        name: "METEOR",
        description: "A ROCK FALLS ON THE CHOSEN GROUND. +10% DAMAGE PER IGNITE STACK",
        target: TargetMode::Ground,
        cast_time: 0.0,
        cooldown: 20.0,
        charges: 1,
        range: 35.0,
        radius: 5.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 1.0,
        coefficient: 5.00,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.15,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 5.0,
        energy: 5.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::Pyroblast,
        name: "PYROBLAST",
        description: "VERY HIGH DAMAGE. +10% PER IGNITE STACK ON THE TARGET",
        target: TargetMode::Single,
        cast_time: 3.0,
        cooldown: 30.0,
        charges: 1,
        range: 35.0,
        radius: 0.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 6.50,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.15,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 5.0,
        energy: 10.0,
        requires_line_of_sight: true,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::Firestorm,
        name: "FIRESTORM",
        description: "A 20 X 10 M LINE OF FIRE FOR 6 S. EACH TICK CAN IGNITE",
        target: TargetMode::Line,
        cast_time: 0.0,
        cooldown: 24.0,
        charges: 1,
        range: 20.0,
        radius: 0.0,
        width: 10.0,
        duration: 6.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.70,
        tick_interval: 0.5,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 1,
        ignite_chance: 0.15,
        heat: 5.0,
        energy: 8.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::Detonate,
        name: "DETONATE",
        description: "CONSUMES A FULL BURN FOR 60% OF ITS REMAINING DAMAGE, AT ONCE",
        target: TargetMode::Single,
        cast_time: 0.0,
        cooldown: 0.0,
        charges: 1,
        range: 35.0,
        radius: 10.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 3.0,
        requires_line_of_sight: true,
        cooldown_hasted: false,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::FireMastery,
        name: "FIRE MASTERY",
        description: "20 S OF HASTE OR CRITICAL STRIKE, WHICHEVER YOU SPECIALISE IN",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 60.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 20.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::FlameShield,
        name: "FLAME SHIELD",
        description: "TAKE 30% LESS DAMAGE FOR 5 SECONDS",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 30.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 5.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::Blink,
        name: "BLINK",
        description: "TELEPORT 10 M FORWARD",
        target: TargetMode::Movement,
        cast_time: 0.0,
        cooldown: 20.0,
        charges: 1,
        range: 10.0,
        radius: 0.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::Cataclysm,
        name: "CATACLYSM",
        description: "NOT YET DESIGNED. THE ULTIMATE IS DISABLED",
        target: TargetMode::Ground,
        cast_time: 1.0,
        cooldown: 0.0,
        charges: 1,
        range: 35.0,
        radius: 10.0,
        width: 0.0,
        duration: 8.0,
        impact_delay: 0.5,
        coefficient: 12.00,
        tick_coefficient: 1.50,
        tick_interval: 1.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 3,
        ignite_chance: 1.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 100.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::Recuperate,
        name: "RECUPERATE",
        description: "A SWING THAT STACKS A HEAL ON YOU. 5 STACKS, 0.2% MAX HP EACH A SECOND",
        target: TargetMode::Single,
        cast_time: 0.0,
        cooldown: 0.0,
        charges: 1,
        range: 5.0,
        radius: 0.0,
        width: 0.0,
        duration: 10.0,
        impact_delay: 0.0,
        coefficient: 1.20,
        tick_coefficient: 0.0,
        tick_interval: 1.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: true,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK,
        warrior: WarriorRow {
            deflection_cost: 0.0,
            health_coefficient: 0.0,
            tick_health_coefficient: 0.0,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::WarStomp,
        name: "WAR STOMP",
        description: "A 120 DEGREE CLEAVE AT YOUR TARGET. -2% ATTACK SPEED A STACK, UP TO 8",
        target: TargetMode::TargetCone,
        cast_time: 0.0,
        cooldown: 4.0,
        charges: 1,
        range: 8.0,
        radius: 0.0,
        width: 0.0,
        duration: 10.0,
        impact_delay: 0.0,
        coefficient: 1.10,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK.with_damage(3.0),
        warrior: WarriorRow {
            deflection_cost: 20.0,
            health_coefficient: 0.0,
            tick_health_coefficient: 0.0,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::ColossusStrike,
        name: "COLOSSUS STRIKE",
        description: "SMASHES THE GROUND. +1% MAX HP, AND AN 8 S BLEED THAT READS YOUR HEALTH",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 8.0,
        charges: 1,
        range: 0.0,
        radius: 8.0,
        width: 0.0,
        duration: 8.0,
        impact_delay: 0.0,
        coefficient: 2.00,
        tick_coefficient: 1.50,
        tick_interval: 2.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK.with_damage(3.0),
        warrior: WarriorRow {
            deflection_cost: 20.0,
            health_coefficient: 0.01,
            tick_health_coefficient: 0.0075,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::ShieldCharge,
        name: "SHIELD CHARGE",
        description: "RUN AT YOUR TARGET. AREA DAMAGE AND A 0.5 S STUN WHEN YOU ARRIVE",
        target: TargetMode::Single,
        cast_time: 0.0,
        cooldown: 15.0,
        charges: 1,
        range: 25.0,
        radius: 5.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 2.50,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: true,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK.with_damage(4.0),
        warrior: WarriorRow {
            deflection_cost: 20.0,
            health_coefficient: 0.0,
            tick_health_coefficient: 0.0,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::ShieldBash,
        name: "SHIELD BASH",
        description: "INTERRUPTS THE CAST. WITH DEFLECTION, SILENCES FOR 4 SECONDS",
        target: TargetMode::Single,
        cast_time: 0.0,
        cooldown: 15.0,
        charges: 1,
        range: 5.0,
        radius: 0.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 1.50,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: true,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK.with_damage(3.0),
        warrior: WarriorRow {
            deflection_cost: 20.0,
            health_coefficient: 0.0,
            tick_health_coefficient: 0.0,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::ShieldBlock,
        name: "SHIELD BLOCK",
        description: "6 S OF EXTRA BLOCK, SCALED BY MAX HP. NO CRITICAL STRIKES TAKEN",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 15.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 6.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK,
        warrior: WarriorRow {
            deflection_cost: 20.0,
            health_coefficient: 0.0,
            tick_health_coefficient: 0.0,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::PunctureBlood,
        name: "PUNCTURE BLOOD",
        description: "SPEND 30% OF CURRENT HP FOR AN ABSORB OF 30% OF MAXIMUM HP, 10 S",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 30.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 10.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK,
        warrior: WarriorRow {
            deflection_cost: 20.0,
            health_coefficient: 0.0,
            tick_health_coefficient: 0.0,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::Adrenaline,
        name: "ADRENALINE",
        description: "BANKS 30% OF THE DAMAGE TAKEN FOR 6 S, THEN HEALS IT BACK OVER 6 S",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 30.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 6.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 1.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK,
        warrior: WarriorRow {
            deflection_cost: 20.0,
            health_coefficient: 0.0,
            tick_health_coefficient: 0.0,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::BattleTrance,
        name: "BATTLE TRANCE",
        description: "10 S. EVERY BLOCK, PARRY OR DODGE HEALS 1% MAX HP AND PAYS DOUBLE",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 45.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 10.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK,
        warrior: WarriorRow {
            deflection_cost: 0.0,
            health_coefficient: 0.0,
            tick_health_coefficient: 0.0,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::SpellReflect,
        name: "SPELL REFLECT",
        description: "6 S. ENEMY SPELLS GO BACK TO WHOEVER CAST THEM",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 45.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 6.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK,
        warrior: WarriorRow {
            deflection_cost: 20.0,
            health_coefficient: 0.0,
            tick_health_coefficient: 0.0,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::Taunt,
        name: "TAUNT",
        description: "THE TARGET ATTACKS YOU FOR 3 S, AND YOUR THREAT RISES TO ITS HIGHEST",
        target: TargetMode::Single,
        cast_time: 0.0,
        cooldown: 8.0,
        charges: 1,
        range: 25.0,
        radius: 0.0,
        width: 0.0,
        duration: 3.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: true,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Warrior,
        damage_type: DamageType::Physical,
        id: FireAbility::LastStand,
        name: "LAST STAND",
        description: "MORE MAXIMUM HP, A HEAL FOR THE SAME SHARE, AND 10% LESS DAMAGE TAKEN",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 90.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 15.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::TANK,
        warrior: WarriorRow {
            deflection_cost: 20.0,
            health_coefficient: 0.0,
            tick_health_coefficient: 0.0,
        },
        druid: DruidRow::NONE,
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::Moonfire,
        name: "MOONFIRE",
        description: "MOONLIGHT, AND 18 S OF IT BURNING. ASTRAL: +50% AND IT NEVER ENDS",
        target: TargetMode::Single,
        cast_time: 0.0,
        cooldown: 0.0,
        charges: 1,
        range: 30.0,
        radius: 0.0,
        width: 0.0,
        duration: 18.0,
        impact_delay: 0.0,
        coefficient: 1.20,
        tick_coefficient: 0.35,
        tick_interval: 2.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: true,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: MOONFIRE_MANA,
            astral_generation: 12.0,
            astral_empowerable: true,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::Sunfire,
        name: "SUNFIRE",
        description: "THE SAME BURN, ON EVERYTHING AROUND THE TARGET. ASTRAL: +30% AND THE PACK HEALS YOU",
        target: TargetMode::Single,
        cast_time: 0.0,
        cooldown: 0.0,
        charges: 1,
        range: 30.0,
        radius: 8.0,
        width: 0.0,
        duration: 18.0,
        impact_delay: 0.0,
        coefficient: 1.00,
        tick_coefficient: 0.35,
        tick_interval: 2.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: true,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: SUNFIRE_MANA,
            astral_generation: 12.0,
            astral_empowerable: true,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::Wrath,
        name: "WRATH",
        description: "FREE. STACKS NATURES EXPOSURE. ASTRAL: 15 S OF STACKING THE WHOLE PACK",
        target: TargetMode::Single,
        cast_time: 1.5,
        cooldown: 0.0,
        charges: 1,
        range: 30.0,
        radius: 0.0,
        width: 0.0,
        duration: 15.0,
        impact_delay: 0.0,
        coefficient: 2.20,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: true,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: 0.0,
            astral_generation: 15.0,
            astral_empowerable: true,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::Rejuvenation,
        name: "REJUVENATION",
        description: "HEALS OVER 18 S. ASTRAL: +30% HEALING",
        target: TargetMode::Friendly,
        cast_time: 0.0,
        cooldown: 0.0,
        charges: 1,
        range: 40.0,
        radius: 0.0,
        width: 0.0,
        duration: 18.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.45,
        tick_interval: 3.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: REJUVENATION_MANA,
            astral_generation: 8.0,
            astral_empowerable: true,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::Regrowth,
        name: "REGROWTH",
        description: "A LARGE HEAL AND 12 S OF IT AFTERWARDS. ASTRAL: +30% HEALING",
        target: TargetMode::Friendly,
        cast_time: 1.5,
        cooldown: 0.0,
        charges: 1,
        range: 40.0,
        radius: 0.0,
        width: 0.0,
        duration: 12.0,
        impact_delay: 0.0,
        coefficient: 3.00,
        tick_coefficient: 0.30,
        tick_interval: 3.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: REGROWTH_MANA,
            astral_generation: 10.0,
            astral_empowerable: true,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::Lifebloom,
        name: "LIFEBLOOM",
        description: "HEALS FOR 15 S AND BLOOMS WHEN IT ENDS. ASTRAL: +30% HEALING",
        target: TargetMode::Friendly,
        cast_time: 0.0,
        cooldown: 0.0,
        charges: 1,
        range: 40.0,
        radius: 0.0,
        width: 0.0,
        duration: 15.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.25,
        tick_interval: 1.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: LIFEBLOOM_MANA,
            astral_generation: 8.0,
            astral_empowerable: true,
            bloom_coefficient: 2.50,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::WildGrowth,
        name: "WILD GROWTH",
        description: "HEALS THE TARGET AND HURT ALLIES NEAR THEM FOR 7 S. ASTRAL: +30% HEALING",
        target: TargetMode::Friendly,
        cast_time: 0.0,
        cooldown: 15.0,
        charges: 1,
        range: 40.0,
        radius: 15.0,
        width: 0.0,
        duration: 7.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.30,
        tick_interval: 1.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: WILD_GROWTH_MANA,
            astral_generation: 12.0,
            astral_empowerable: true,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::Swiftmend,
        name: "SWIFTMEND",
        description: "EATS ONE OF YOUR HEALS OVER TIME FOR A LARGE ONE NOW. ASTRAL: +30% HEALING",
        target: TargetMode::Friendly,
        cast_time: 0.0,
        cooldown: 12.0,
        charges: 1,
        range: 40.0,
        radius: 0.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 4.00,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: SWIFTMEND_MANA,
            astral_generation: 10.0,
            astral_empowerable: true,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::Nourish,
        name: "NOURISH",
        description: "+10% PER HEAL OVER TIME ALREADY ON THE TARGET, WITH NO CAP. ASTRAL: +30% HEALING",
        target: TargetMode::Friendly,
        cast_time: 2.0,
        cooldown: 0.0,
        charges: 1,
        range: 40.0,
        radius: 0.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 2.80,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: NOURISH_MANA,
            astral_generation: 8.0,
            astral_empowerable: true,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::NaturesCleanse,
        name: "NATURES CLEANSE",
        description: "REMOVES MAGIC, POISONS, CURSES AND DISEASES FROM AN ALLY. NOT EMPOWERABLE",
        target: TargetMode::Friendly,
        cast_time: 0.0,
        cooldown: 8.0,
        charges: 1,
        range: 40.0,
        radius: 0.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: NATURES_CLEANSE_MANA,
            astral_generation: 5.0,
            astral_empowerable: false,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::WildRush,
        name: "WILD RUSH",
        description: "6 S OF +50% SPEED AND -30% DAMAGE TAKEN. NOT EMPOWERABLE",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 25.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 6.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: 0.0,
            astral_generation: 0.0,
            astral_empowerable: false,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::AstralPower,
        name: "ASTRAL POWER",
        description: "ARMS THE NEXT COMPATIBLE SPELL. IT IS SPENT WHEN THAT SPELL IS CAST",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 0.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.0,
        tick_interval: 0.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: 0.0,
            astral_generation: 0.0,
            astral_empowerable: false,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::Druid,
        damage_type: DamageType::Nature,
        id: FireAbility::Tranquility,
        name: "TRANQUILITY",
        description: "CHANNELLED. HEALS EVERY ALLY WITHIN 20 M ONCE A SECOND. ASTRAL: +30% HEALING",
        target: TargetMode::Friendly,
        cast_time: 6.0,
        cooldown: 120.0,
        charges: 1,
        range: 40.0,
        radius: 20.0,
        width: 0.0,
        duration: 6.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 1.20,
        tick_interval: 1.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::HEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow {
            mana_cost: TRANQUILITY_MANA,
            astral_generation: 0.0,
            astral_empowerable: true,
            bloom_coefficient: 0.0,
        },
    },
    AbilityDef {
        hero: Hero::FireMage,
        damage_type: DamageType::Fire,
        id: FireAbility::Ignite,
        name: "IGNITE",
        description: "A STACKING BURN. 20% SPELL POWER PER STACK EVERY 2 SECONDS",
        target: TargetMode::Single,
        cast_time: 0.0,
        cooldown: 0.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        width: 0.0,
        duration: 30.0,
        impact_delay: 0.0,
        coefficient: 0.0,
        tick_coefficient: 0.20,
        tick_interval: 2.0,
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        requires_line_of_sight: false,
        cooldown_hasted: true,
        energy_cost: 0.0,
        threat: ThreatScale::DAMAGE_DEALER,
        warrior: WarriorRow::NONE,
        druid: DruidRow::NONE,
    },
];

/// Why an ability could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityError {
    /// The caster is dead.
    CasterDead,
    /// Every charge is spent.
    OnCooldown,
    /// Another ability was started less than a global cooldown ago.
    GlobalCooldown,
    /// Not enough Ultimate Energy.
    NotEnoughEnergy,
    /// It needs a target and none is selected.
    NoTarget,
    /// The selected target is dead or gone.
    TargetInvalid,
    /// The selected target is not something to attack — the player themselves, or an ally.
    ///
    /// Selecting yourself is legal and useful; setting yourself on fire is not, and the
    /// refusal has to say which of the two happened.
    TargetFriendly,
    /// The target is further away than it reaches.
    OutOfRange,
    /// The target is behind the caster.
    FacingWrongWay,
    /// Something in the level stands between the caster and the target.
    NoLineOfSight,
    /// Detonate without a full burn on the target.
    NotEnoughIgnite,
    /// A ground-targeted ability was cast without a placement.
    NeedsGroundTarget,
    /// The chosen ground is out of range.
    GroundOutOfRange,
    /// The Ultimate has no design yet and is switched off.
    UltimateDisabled,
    /// Not enough Mana.
    NotEnoughMana,
    /// The Astral Power button was pressed without a full bar, or while already armed.
    NotEnoughAstralPower,
    /// Swiftmend was cast on a target with none of the Druid's heals over time on it.
    NoHealOverTime,
    /// A friendly ability was cast with nothing friendly to point it at.
    NoFriendlyTarget,
}

impl AbilityError {
    /// Short uppercase description, for the UI font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CasterDead => "YOU ARE DEAD",
            Self::OnCooldown => "NOT READY",
            Self::GlobalCooldown => "TOO SOON",
            Self::NotEnoughEnergy => "NOT ENOUGH ENERGY",
            Self::NoTarget => "NO TARGET",
            Self::TargetInvalid => "TARGET IS GONE",
            Self::TargetFriendly => "THAT IS NOT AN ENEMY",
            Self::OutOfRange => "OUT OF RANGE",
            Self::FacingWrongWay => "YOU ARE FACING THE WRONG DIRECTION.",
            Self::NoLineOfSight => "TARGET IS NOT IN LINE OF SIGHT",
            Self::NotEnoughIgnite => "NEEDS MAXIMUM IGNITE",
            Self::NeedsGroundTarget => "CHOOSE A LOCATION",
            Self::GroundOutOfRange => "TOO FAR AWAY",
            Self::UltimateDisabled => "NOT AVAILABLE YET",
            Self::NotEnoughMana => "NOT ENOUGH MANA",
            Self::NotEnoughAstralPower => "NOT ENOUGH ASTRAL POWER",
            Self::NoHealOverTime => "NO HEAL OVER TIME ON THE TARGET",
            Self::NoFriendlyTarget => "NO FRIENDLY TARGET",
        }
    }
}

/// One ability's cooldown and charges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityState {
    /// Charges ready to spend.
    pub charges: u8,
    /// Seconds until the next charge returns. Zero when the bucket is full.
    pub recharge: f32,
}

impl AbilityState {
    /// A full bucket.
    #[must_use]
    pub fn full(charges: u8) -> Self {
        Self {
            charges: charges.max(1),
            recharge: 0.0,
        }
    }
}

/// Every ability's runtime state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityBook {
    states: [AbilityState; ABILITY_COUNT],
}

impl Default for AbilityBook {
    fn default() -> Self {
        Self::new(&TalentEffects::default())
    }
}

impl AbilityBook {
    /// Every ability ready, with the charges the talents allow.
    #[must_use]
    pub fn new(effects: &TalentEffects) -> Self {
        let mut states = [AbilityState::full(1); ABILITY_COUNT];
        for ability in FireAbility::ALL {
            states[ability.index()] =
                AbilityState::full(ability.definition().charges_with(effects));
        }
        Self { states }
    }

    /// Everything about `ability`'s cooldown: charges ready and time to the next one.
    #[must_use]
    pub fn state(&self, ability: FireAbility) -> AbilityState {
        self.states[ability.index()]
    }

    /// Replaces `ability`'s cooldown state: how a networked client is brought back in line
    /// with the authoritative simulation.
    pub fn set_state(&mut self, ability: FireAbility, state: AbilityState) {
        self.states[ability.index()] = AbilityState {
            charges: state.charges,
            recharge: if state.recharge.is_finite() { state.recharge.max(0.0) } else { 0.0 },
        };
    }

    /// Charges ready on `ability`.
    #[must_use]
    pub fn charges(&self, ability: FireAbility) -> u8 {
        self.states[ability.index()].charges
    }

    /// Seconds until the next charge of `ability` returns.
    #[must_use]
    pub fn recharge_remaining(&self, ability: FireAbility) -> f32 {
        self.states[ability.index()].recharge
    }

    /// How far through the current recharge `ability` is, in `0.0..=1.0`.
    #[must_use]
    pub fn recharge_fraction(&self, ability: FireAbility, effects: &TalentEffects) -> f32 {
        let state = self.states[ability.index()];
        if state.recharge <= 0.0 {
            return 1.0;
        }
        let cooldown = ability.definition().cooldown_with(effects);
        if cooldown <= 0.0 {
            return 1.0;
        }
        (1.0 - state.recharge / cooldown).clamp(0.0, 1.0)
    }

    /// Whether `ability` has a charge to spend.
    #[must_use]
    pub fn is_ready(&self, ability: FireAbility) -> bool {
        self.states[ability.index()].charges > 0
    }

    /// Spends a charge and starts the recharge if it was not already running.
    pub fn spend(&mut self, ability: FireAbility, effects: &TalentEffects) -> bool {
        let cooldown = ability.definition().cooldown_with(effects);
        if cooldown <= 0.0 {
            return true;
        }
        let state = &mut self.states[ability.index()];
        if state.charges == 0 {
            return false;
        }
        state.charges -= 1;
        if state.recharge <= 0.0 && cooldown > 0.0 {
            state.recharge = cooldown;
        }
        true
    }

    /// Takes `seconds` off the recharge of `ability`, possibly returning a charge.
    ///
    /// Returns the seconds that were *not* used, which is what a cooldown reduction wastes
    /// when the ability was already ready. The simulator reports that number; nothing in
    /// gameplay depends on it.
    pub fn reduce_cooldown(
        &mut self,
        ability: FireAbility,
        seconds: f32,
        effects: &TalentEffects,
    ) -> f32 {
        if seconds <= 0.0 {
            return 0.0;
        }
        let maximum = ability.definition().charges_with(effects);
        let cooldown = ability.definition().cooldown_with(effects);
        let state = &mut self.states[ability.index()];
        if state.recharge <= 0.0 {
            return seconds;
        }
        state.recharge -= seconds;
        while state.recharge <= 0.0 && state.charges < maximum {
            state.charges += 1;
            if state.charges < maximum && cooldown > 0.0 {
                state.recharge += cooldown;
            } else {
                let wasted = -state.recharge;
                state.recharge = 0.0;
                return wasted.max(0.0);
            }
        }
        0.0
    }

    /// Hands `ability` one charge, up to its maximum.
    pub fn grant_charge(&mut self, ability: FireAbility, effects: &TalentEffects) {
        let maximum = ability.definition().charges_with(effects);
        let state = &mut self.states[ability.index()];
        if state.charges < maximum {
            state.charges += 1;
        }
        if state.charges >= maximum {
            state.recharge = 0.0;
        }
    }

    /// Advances every recharge by `seconds`, scaled by haste where the ability allows it.
    pub fn tick(&mut self, seconds: f32, haste: f32, effects: &TalentEffects) {
        for ability in FireAbility::ALL {
            let definition = ability.definition();
            let scaled = if definition.cooldown_hasted {
                seconds * (1.0 + haste)
            } else {
                seconds
            };
            let maximum = definition.charges_with(effects);
            let cooldown = definition.cooldown_with(effects);
            let state = &mut self.states[ability.index()];
            state.charges = state.charges.min(maximum);
            if state.recharge <= 0.0 {
                continue;
            }
            state.recharge -= scaled;
            while state.recharge <= 0.0 && state.charges < maximum {
                state.charges += 1;
                if state.charges < maximum && cooldown > 0.0 {
                    state.recharge += cooldown;
                } else {
                    state.recharge = 0.0;
                }
            }
            if state.charges >= maximum {
                state.recharge = 0.0;
            }
        }
    }

    /// Refills everything. For a reset.
    pub fn refill(&mut self, effects: &TalentEffects) {
        *self = Self::new(effects);
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::firemage::talent::{TalentGrid, TalentId};

    fn effects() -> TalentEffects {
        TalentEffects::default()
    }

    #[test]
    fn the_mages_bar_holds_the_ten_abilities_in_the_designed_order() {
        let names: Vec<&str> = Hero::FireMage
            .abilities()
            .iter()
            .map(|ability| ability.definition().name)
            .collect();
        assert_eq!(
            names,
            [
                "FIREBOLT",
                "FLAME LANCE",
                "FLAME CONE",
                "METEOR",
                "PYROBLAST",
                "FIRESTORM",
                "DETONATE",
                "FIRE MASTERY",
                "FLAME SHIELD",
                "BLINK",
            ]
        );
    }

    #[test]
    fn the_removed_abilities_are_gone_from_the_class() {
        // Inferno, Flame Wave and the old Fire Detonation were replaced outright; a stray
        // row would be an ability the player could still find.
        for ability in FireAbility::ALL {
            let name = ability.definition().name;
            assert_ne!(name, "INFERNO");
            assert_ne!(name, "FLAME WAVE");
            assert_ne!(name, "FIRE DETONATION");
            assert_ne!(name, "COMBUSTION");
        }
    }

    #[test]
    fn the_ultimate_is_not_on_the_bar_and_is_disabled() {
        assert!(FireAbility::Cataclysm.is_ultimate());
        assert_eq!(FireAbility::Cataclysm.bar_slot(), None);
        assert_eq!(Hero::FireMage.abilities().len(), ABILITY_SLOTS);
        #[allow(
            clippy::assertions_on_constants,
            reason = "the constant is the subject"
        )]
        {
            assert!(!ULTIMATE_ENABLED, "the ultimate has no design yet");
        }
    }

    #[test]
    fn the_table_matches_the_design_document() {
        // The values after the balance pass recorded in docs/firemage-design.md; the two
        // Ignite scalings opened at 10% and were raised to 15% because the maintenance
        // build's payoff spells were not worth holding a burn for.
        let firebolt = FireAbility::Firebolt.definition();
        assert_eq!(
            (firebolt.cast_time, firebolt.cooldown, firebolt.coefficient),
            (1.0, 0.0, 1.00)
        );
        assert_eq!((firebolt.heat, firebolt.ignite_chance), (2.0, 0.50));

        let lance = FireAbility::FlameLance.definition();
        assert_eq!(
            (lance.cast_time, lance.cooldown, lance.charges),
            (1.5, 8.0, 2)
        );
        assert_eq!(
            (lance.coefficient, lance.ignite_stacks, lance.heat),
            (2.60, 2, 4.0)
        );

        let cone = FireAbility::FlameCone.definition();
        assert_eq!(
            (cone.cast_time, cone.cooldown, cone.range),
            (0.0, 10.0, 12.0)
        );
        assert_eq!(
            (cone.coefficient, cone.ignite_stacks, cone.heat),
            (1.40, 2, 3.0)
        );

        let meteor = FireAbility::Meteor.definition();
        assert_eq!(
            (meteor.cooldown, meteor.radius, meteor.cast_time),
            (20.0, 5.0, 0.0)
        );
        assert_eq!(meteor.ignite_stacks, 0, "meteor applies no ignite");
        assert_eq!(
            (meteor.coefficient, meteor.ignite_bonus_per_stack),
            (5.00, 0.15)
        );

        let pyroblast = FireAbility::Pyroblast.definition();
        assert_eq!((pyroblast.cast_time, pyroblast.cooldown), (3.0, 30.0));
        assert_eq!(
            (pyroblast.coefficient, pyroblast.ignite_bonus_per_stack),
            (6.50, 0.15)
        );

        let firestorm = FireAbility::Firestorm.definition();
        assert_eq!((firestorm.duration, firestorm.tick_interval), (6.0, 0.5));
        // The linear zone: twenty metres ahead, ten across.
        assert_eq!((firestorm.range, firestorm.width), (20.0, 10.0));
        assert_eq!(firestorm.target, TargetMode::Line);
        assert_eq!(
            (firestorm.tick_coefficient, firestorm.ignite_chance),
            (0.70, 0.15)
        );
        assert_eq!(
            (firestorm.duration / firestorm.tick_interval) as i32,
            12,
            "twelve ticks, which the simulator assumes"
        );

        let detonate = FireAbility::Detonate.definition();
        assert_eq!((detonate.cooldown, detonate.radius), (0.0, 10.0));
        assert_eq!(detonate.cast_time, 0.0, "detonate is instant");
        assert!(
            !detonate.cooldown_hasted,
            "haste both fills the bank and spends it"
        );
        for ability in FireAbility::ALL {
            if ability != FireAbility::Detonate {
                assert!(
                    ability.definition().cooldown_hasted,
                    "{ability:?} stopped scaling with haste"
                );
            }
        }

        let mastery = FireAbility::FireMastery.definition();
        assert_eq!((mastery.cooldown, mastery.duration), (60.0, 20.0));
    }

    #[test]
    fn only_the_ultimate_costs_a_resource() {
        for ability in Hero::FireMage.abilities() {
            assert_eq!(ability.definition().energy_cost, 0.0);
        }
        assert_eq!(FireAbility::Cataclysm.definition().energy_cost, 100.0);
    }

    #[test]
    fn a_charge_is_spent_and_comes_back_after_the_cooldown() {
        let effects = effects();
        let mut book = AbilityBook::new(&effects);
        assert!(book.spend(FireAbility::Meteor, &effects));
        assert!(!book.is_ready(FireAbility::Meteor));

        book.tick(19.0, 0.0, &effects);
        assert!(!book.is_ready(FireAbility::Meteor), "back early");
        book.tick(1.5, 0.0, &effects);
        assert!(book.is_ready(FireAbility::Meteor));
    }

    #[test]
    fn two_charges_are_spent_one_at_a_time_and_return_one_at_a_time() {
        let effects = effects();
        let mut book = AbilityBook::new(&effects);
        assert_eq!(book.charges(FireAbility::FlameLance), 2);

        book.spend(FireAbility::FlameLance, &effects);
        assert!(book.is_ready(FireAbility::FlameLance));
        book.spend(FireAbility::FlameLance, &effects);
        assert_eq!(book.charges(FireAbility::FlameLance), 0);

        book.tick(8.0, 0.0, &effects);
        assert_eq!(book.charges(FireAbility::FlameLance), 1);
        book.tick(8.0, 0.0, &effects);
        assert_eq!(book.charges(FireAbility::FlameLance), 2);
    }

    #[test]
    fn volatile_ignition_hands_pyroblast_a_second_charge() {
        let mut grid = TalentGrid::with_points(100);
        grid.purchase(TalentId::VolatileIgnition)
            .expect("affordable");
        let effects = grid.effects();
        assert_eq!(
            FireAbility::Pyroblast.definition().charges_with(&effects),
            2
        );
        assert_eq!(
            FireAbility::Pyroblast
                .definition()
                .charges_with(&TalentEffects::default()),
            1
        );
    }

    #[test]
    fn haste_shortens_cooldowns_and_casts() {
        let effects = effects();
        let mut fast = AbilityBook::new(&effects);
        fast.spend(FireAbility::Meteor, &effects);
        fast.tick(10.0, 1.0, &effects);
        assert!(fast.is_ready(FireAbility::Meteor));

        let stats = Stats {
            haste: 1.0,
            ..Stats::base()
        };
        assert!((FireAbility::Pyroblast.definition().cast_time_with(&stats) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn a_cooldown_reduction_reports_what_it_wasted() {
        let effects = effects();
        let mut book = AbilityBook::new(&effects);

        // Nothing running: the whole reduction is wasted, and none of it is banked.
        assert!((book.reduce_cooldown(FireAbility::Meteor, 4.0, &effects) - 4.0).abs() < 1e-6);
        book.spend(FireAbility::Meteor, &effects);
        assert!(
            book.recharge_remaining(FireAbility::Meteor) > 19.0,
            "banked"
        );

        assert_eq!(
            book.reduce_cooldown(FireAbility::Meteor, 3.0, &effects),
            0.0
        );
        let wasted = book.reduce_cooldown(FireAbility::Meteor, 100.0, &effects);
        assert!(wasted > 0.0, "an overshoot reported nothing");
        assert!(book.is_ready(FireAbility::Meteor));
    }

    #[test]
    fn a_zero_cooldown_ability_never_becomes_unready() {
        let effects = effects();
        let mut book = AbilityBook::new(&effects);
        for _ in 0..10 {
            assert!(book.spend(FireAbility::Firebolt, &effects));
            book.tick(0.016, 0.0, &effects);
            assert!(book.is_ready(FireAbility::Firebolt));
        }
    }

    #[test]
    fn every_ability_round_trips_through_its_written_name() {
        let mut seen: Vec<&str> = Vec::new();
        for ability in FireAbility::ALL {
            let name = ability.config_name();
            assert_eq!(
                FireAbility::from_config_name(name),
                Some(ability),
                "{ability:?} did not survive its own name"
            );
            assert!(!seen.contains(&name), "{name} names two abilities");
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name} is not a written name"
            );
            seen.push(name);
        }
        assert_eq!(FireAbility::from_config_name("frostbolt"), None);
        assert_eq!(FireAbility::from_config_name(""), None);
        // The display name is not the written one, and must not be usable as it.
        assert_eq!(FireAbility::from_config_name("FIREBOLT"), None);
    }

    #[test]
    fn everything_on_the_bar_can_be_assigned_and_nothing_else_can() {
        for ability in Hero::FireMage.abilities().iter().copied() {
            assert!(
                ability.is_assignable(),
                "{ability:?} cannot be put in a slot"
            );
        }
        assert!(
            !FireAbility::Ignite.is_assignable(),
            "a damage meter row is not a button"
        );
        assert!(
            !FireAbility::Cataclysm.is_assignable(),
            "the Ultimate has a place of its own"
        );
    }
}
