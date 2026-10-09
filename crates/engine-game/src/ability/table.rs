//! Every hero's abilities, as data, plus the runtime that cools them down.
//!
//! ```text
//! FireAbility (which)  ->  AbilityDef (numbers)  ->  AbilityBook (cooldowns and charges)
//! ```
//!
//! [`ABILITIES`] is one table assembled from each class's own rows —
//! [`crate::firemage::abilities`], [`crate::warrior::abilities`], [`crate::druid::abilities`]
//! — so a class's numbers live with the class and the table is still indexed by
//! [`FireAbility`]. Cast times, cooldowns, charges, ranges, areas, Heat and the
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
use crate::firemage::stats::Stats;
use crate::firemage::talent::TalentEffects;
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
        self.definition().hero()
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
    /// [`AbilityDef::name`]: that one is an uppercase
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

/// How the caster's body shows an ability: a presentation hint carried as data on the row,
/// so the visual layer never matches on an ability's identity.
///
/// Nothing in the simulation reads it. The visual layer maps it to an animation clip the
/// model may or may not have, and [Motion::Cast] means no motion of its own: the ordinary
/// cast animation, which is what every ability showed before this column existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Motion {
    /// The ordinary cast: a spell flick, or a cast bar's wind-up.
    Cast,
    /// A single slash with the weapon.
    Swing,
    /// A wide sweep that catches everything in front of the caster.
    Cleave,
    /// A heavy overhead blow at the ground.
    Smash,
    /// A shove with the shield.
    Bash,
    /// A forward lunge or charge into the target.
    Lunge,
    /// Raising a guard against what is coming.
    Guard,
    /// Gathering strength: a flex, a clenched fist.
    PowerUp,
    /// A shout.
    Battlecry,
    /// Mending an ally. Unlike the Warrior's motions it is a *flavour of cast*: played through
    /// the heal cast clips and timed to the cast bar like the ordinary cast.
    Heal,
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
/// Carried only by Warrior rows, as [`ClassRow::Warrior`]. The
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
    /// How much Deflection must be banked before the cost is paid, or zero for "the cost".
    ///
    /// Every spender empowers as soon as it can afford itself; Recuperate is the exception
    /// the design makes, and spends only a **full** bank — it is the outlet for a Warrior with
    /// nothing else to spend on, and must never compete with the real spenders below that.
    pub empower_at: f32,
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
    /// Every column at zero: the defaults a row overrides, and what
    /// [`AbilityDef::warrior`] answers for a row that is not the Warrior's.
    pub const NONE: Self = Self {
        deflection_cost: 0.0,
        empower_at: 0.0,
        health_coefficient: 0.0,
        tick_health_coefficient: 0.0,
    };

    /// Whether twenty Deflection buys this ability anything.
    #[must_use]
    pub const fn is_empowerable(&self) -> bool {
        self.deflection_cost > 0.0
    }

    /// How much must be banked for a press to empower: the cost, or more where the row says.
    #[must_use]
    pub fn empower_threshold(&self) -> f32 {
        self.deflection_cost.max(self.empower_at)
    }
}

/// The Druid's columns of the ability table.
///
/// Carried only by Druid rows, as [`ClassRow::Druid`]. What Astral Power *does* to an
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
    /// Every column at zero: the defaults a row overrides, and what [`AbilityDef::druid`]
    /// answers for a row that is not the Druid's.
    pub const NONE: Self = Self {
        mana_cost: 0.0,
        astral_generation: 0.0,
        astral_empowerable: false,
        bloom_coefficient: 0.0,
    };
}

/// The FireMage's columns of the ability table: Heat, Ultimate Energy and Ignite.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FireMageRow {
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
}

impl FireMageRow {
    /// Every column at zero: the defaults a row overrides, and what [`AbilityDef::fire`]
    /// answers for a row that is not the FireMage's.
    pub const NONE: Self = Self {
        ignite_bonus_per_stack: 0.0,
        ignite_stacks: 0,
        ignite_chance: 0.0,
        heat: 0.0,
        energy: 0.0,
        energy_cost: 0.0,
    };
}

/// Which hero a row belongs to, and the columns only that hero has.
///
/// The whole of "which class an ability belongs to". A Warrior row cannot carry Heat or a
/// Druid row Deflection: the data a class does not have is not there to be zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClassRow {
    /// A FireMage ability, and what it does with Heat, Energy and Ignite.
    FireMage(FireMageRow),
    /// A Warrior ability: what it spends and how it reads health.
    Warrior(WarriorRow),
    /// A Druid ability: what it costs, what it generates, what Astral Power may do to it.
    Druid(DruidRow),
}

impl ClassRow {
    /// The hero whose row this is.
    #[must_use]
    pub const fn hero(&self) -> Hero {
        match self {
            Self::FireMage(_) => Hero::FireMage,
            Self::Warrior(_) => Hero::Warrior,
            Self::Druid(_) => Hero::Druid,
        }
    }
}

/// One ability, entirely as data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityDef {
    /// Which ability this describes.
    pub id: FireAbility,
    /// What the damage it deals counts as.
    ///
    /// A column rather than a rule about the caster: a Druid's stars are arcane and a
    /// Warrior's swing is physical because their rows say so, which is the same reason the
    /// mage's are fire.
    pub damage_type: DamageType,
    /// Uppercase name, the form the table stores; the interface title-cases it.
    pub name: &'static str,
    /// What it does, in the tooltip's own words.
    ///
    /// Natural sentences, written as a template: `{duration}`, `{cooldown}`, `{range}` and the
    /// other tokens [`crate::ability::text`] knows are filled from this row and from the
    /// class's tuning constants when the tooltip is built, so a balance pass that moves a
    /// number moves the sentence with it. A test holds every token to resolving.
    pub description: &'static str,
    /// What spending the class's resource on it adds, as a template like
    /// [`AbilityDef::description`]. Empty for an ability nothing empowers.
    ///
    /// Shown in its own tooltip section beside what the empowerment costs, which is derived
    /// from the row ([`AbilityDef::resource_uses`]) rather than written here.
    pub empowered: &'static str,
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
    /// Whether a single-target cast turns the caster towards its target instead of being
    /// refused for facing the wrong way.
    ///
    /// Taunt sets it: a taunt is pointed at the selection from up to thirty metres, and
    /// making the tank turn round first would be refusing it for the camera. Everything that
    /// is aimed by the caster's body — a swing, a bolt — keeps the facing rule.
    pub turns_to_target: bool,
    /// How much threat each point of what it does is worth.
    pub threat: ThreatScale,
    /// Whether haste shortens its cooldown.
    ///
    /// True for everything the player *casts*, because going faster is what haste is. False
    /// for Detonate: that ability cashes in a bank whose size is set by Ignite's duration,
    /// and a haste character already fills the bank faster. Letting haste also spend it more
    /// often is the same stat paid twice, and at a geared 45 % it was worth +23 % on a single
    /// target against a balance goal of +5 to +10.
    pub cooldown_hasted: bool,
    /// Whether casting it starts the global cooldown, and whether the global cooldown holds
    /// it back.
    ///
    /// True for nearly everything. False for the few presses that are meant to be woven in
    /// between the others — a Blink out of a cast's aftermath, a Detonate the moment the
    /// burn is full, a cooldown popped on the way — which are paced by their own cooldowns
    /// instead.
    pub on_global_cooldown: bool,
    /// How the caster's body shows it. Presentation only; see [Motion].
    pub motion: Motion,
    /// Which hero casts it, and the columns only that hero's abilities have.
    pub class: ClassRow,
}

impl AbilityDef {
    /// Every shared column at the value most abilities want, for the class tables to
    /// override.
    ///
    /// Each class file narrows this into its own `BASE` (its [`ClassRow`], damage type and
    /// threat), and a row is written as `AbilityDef { id, name, …, ..BASE }`, saying only
    /// what is particular about the ability. `id` and `name` are placeholders every row
    /// sets; [`ABILITIES`]' assembly fails the build on a row filed under the wrong index.
    pub const BASE: Self = Self {
        id: FireAbility::Ignite,
        damage_type: DamageType::Physical,
        name: "",
        description: "",
        empowered: "",
        target: TargetMode::Single,
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
        requires_line_of_sight: false,
        turns_to_target: false,
        threat: ThreatScale::DAMAGE_DEALER,
        cooldown_hasted: true,
        on_global_cooldown: true,
        motion: Motion::Cast,
        class: ClassRow::FireMage(FireMageRow::NONE),
    };

    /// Which hero casts it.
    #[must_use]
    pub const fn hero(&self) -> Hero {
        self.class.hero()
    }

    /// The FireMage's columns, or [`FireMageRow::NONE`] for another hero's ability.
    #[must_use]
    pub const fn fire(&self) -> &FireMageRow {
        match &self.class {
            ClassRow::FireMage(row) => row,
            _ => &FireMageRow::NONE,
        }
    }

    /// The Warrior's columns, or [`WarriorRow::NONE`] for another hero's ability.
    #[must_use]
    pub const fn warrior(&self) -> &WarriorRow {
        match &self.class {
            ClassRow::Warrior(row) => row,
            _ => &WarriorRow::NONE,
        }
    }

    /// The Druid's columns, or [`DruidRow::NONE`] for another hero's ability.
    #[must_use]
    pub const fn druid(&self) -> &DruidRow {
        match &self.class {
            ClassRow::Druid(row) => row,
            _ => &DruidRow::NONE,
        }
    }

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
        self.base_damage(stats) + self.warrior().health_coefficient * max_health.max(0.0)
    }

    /// One periodic tick at `stats` including its share of `max_health`.
    #[must_use]
    pub fn base_tick_damage_with_health(&self, stats: &Stats, max_health: f32) -> f32 {
        self.base_tick_damage(stats) + self.warrior().tick_health_coefficient * max_health.max(0.0)
    }
}

/// The whole class, in bar order. The Ultimate is last.
/// The whole table, in [`FireAbility`] order: each class's rows, then Ignite.
///
/// Assembled at compile time from the classes' own files. The order is checked here too: a
/// row filed under the wrong index fails the build rather than casting the wrong spell.
pub static ABILITIES: [AbilityDef; ABILITY_COUNT] = assemble();

/// Concatenates the class tables and proves every row sits at its own index.
const fn assemble() -> [AbilityDef; ABILITY_COUNT] {
    let mut table = [crate::firemage::abilities::IGNITE; ABILITY_COUNT];
    let mut at = 0;
    let mut index = 0;
    while index < crate::firemage::abilities::ROWS.len() {
        table[at] = crate::firemage::abilities::ROWS[index];
        assert!(
            matches!(table[at].class, ClassRow::FireMage(_)),
            "a row in the FireMage's table is not a FireMage row"
        );
        at += 1;
        index += 1;
    }
    index = 0;
    while index < crate::warrior::abilities::ROWS.len() {
        table[at] = crate::warrior::abilities::ROWS[index];
        assert!(
            matches!(table[at].class, ClassRow::Warrior(_)),
            "a row in the Warrior's table is not a Warrior row"
        );
        at += 1;
        index += 1;
    }
    index = 0;
    while index < crate::druid::abilities::ROWS.len() {
        table[at] = crate::druid::abilities::ROWS[index];
        assert!(
            matches!(table[at].class, ClassRow::Druid(_)),
            "a row in the Druid's table is not a Druid row"
        );
        at += 1;
        index += 1;
    }
    table[at] = crate::firemage::abilities::IGNITE;
    assert!(
        at + 1 == ABILITY_COUNT,
        "the class tables do not add up to ABILITY_COUNT"
    );
    index = 0;
    while index < ABILITY_COUNT {
        assert!(
            table[index].id as usize == index,
            "an ability row is out of FireAbility order"
        );
        index += 1;
    }
    table
}
