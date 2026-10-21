//! Temporary effects that outlive the ability that applied them.
//!
//! A status is what makes Milestone 13's synergy possible: Ignite leaves [`Burning`] on a
//! target, and Detonate reads it without either ability knowing about the other.
//!
//! ```text
//! Ignite -> Burning (6 s, 8 damage a second)
//!               |
//!               +--> ticks damage on its own
//!               |
//!               +--> Detonate consumes what is left for a burst
//! ```
//!
//! [`Burning`]: StatusKind::Burning
//!
//! Statuses do not apply damage themselves. Ticking produces [`StatusOutcome`] values that
//! the simulation turns into damage and into [`crate::event::CombatEvent`]s, which is what
//! keeps a status tick indistinguishable from a direct hit as far as telemetry is
//! concerned — it is attributed to the ability that applied it, and to the caster.

use crate::ability::AbilityId;
use crate::character::Stat;
use crate::combat::DamageType;
use crate::druid::tuning::{
    NATURES_EXPOSURE_PER_STACK, NATURES_EXPOSURE_STACK_CEILING, WILD_RUSH_REDUCTION,
    WILD_RUSH_SPEED,
};
use crate::entity::EntityId;
use crate::warrior::tuning::{
    FERVOR_DAMAGE_PER_STACK, FERVOR_DAMAGE_TAKEN_PER_STACK, FERVOR_DURATION,
    FERVOR_MAX_HEALTH_PER_STACK, FERVOR_MAX_STACKS, WAR_STOMP_ATTACK_SPEED_PER_STACK,
};

/// Seconds between the damage ticks of a lasting status.
///
/// One second, so a burn's remaining duration and its remaining damage are the same number
/// scaled: what is left is readable off the overlay without arithmetic.
pub const STATUS_TICK_INTERVAL: f32 = 1.0;

/// Every lasting effect in the game: the keys of the global effect registry.
///
/// An enum rather than a string id, so a `match` over every effect is exhaustive and adding
/// one is a compile error everywhere it must be handled. What each one *is* — its name, its
/// class, its stacks, its dispel school, what it does to stats — is its row in
/// [`STATUS_DEFS`], read through [`StatusKind::definition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusKind {
    /// Damage over time. Consumed by Detonate.
    Burning,
    /// The Warrior's bleed, from Colossus Strike.
    ///
    /// The one status in the game whose damage is **not** decided when it is applied: every
    /// tick reads the caster's maximum health at the moment of the tick. See
    /// [`Status::source_health_per_tick`].
    Bleeding,
    /// The Warrior's self heal, from Recuperate. Stacks to five.
    Recuperating,
    /// The healing Adrenaline banked, paid back over its recovery.
    Recovering,
    /// War Stomp's attack-speed debuff. Stacks to eight.
    Sundered,
    /// The Druid's Moonfire: Nature damage over time on one enemy.
    ///
    /// The one status in the game that can be made to last forever - see
    /// [`Status::forever`] and the Astral Empowerment that grants it.
    Moonfire,
    /// The Druid's Sunfire: the same burn, spread over a pack.
    Sunfire,
    /// Astral Sunfire: while it is on an enemy, the party is healed for a share of the
    /// damage they deal to it.
    ///
    /// It deals nothing itself. It is a *mark*, and the conversion happens in the one place
    /// damage is applied.
    AstralSunfire,
    /// Wrath's debuff: more Nature and Arcane damage taken, per stack, to ten.
    NaturesExposure,
    /// Rejuvenation's healing over time.
    Rejuvenation,
    /// Regrowth's healing over time, which follows its direct heal.
    Regrowth,
    /// Lifebloom's healing over time, which blooms for a large heal when it expires.
    Lifebloom,
    /// Wild Growth's healing over time.
    WildGrowth,
    /// Astral Wrath: while it is up, Wrath's debuff is applied around the primary target.
    AstralWrath,
    /// Wild Rush: faster, and harder to hurt, for six seconds.
    WildRush,
    /// A poison. Nothing applies it in the shipping roster; it is what a dispel is written
    /// and tested against.
    Poisoned,
    /// The twenty seconds after a respawn during which the player cannot be respawned again.
    ///
    /// The one status in the game that is not applied by an ability and does not belong to a
    /// class: the player lifecycle applies it, and what it does is refuse the next respawn —
    /// see `crate::sim::respawn`. It is here rather than as a flag on the player because a
    /// timed effect the interface has to show is exactly what this module is, and a second
    /// place that counts down seconds would be a second place that can disagree with this one.
    ///
    /// Its clock lives in the respawn system rather than in [`StatusSet::tick`], because
    /// statuses do not tick on a corpse and this one has to run while the player is dead.
    /// This is the *published view* of that clock: everything that asks whether the player is
    /// protected asks the status, and only the respawn system writes it.
    RespawnProtection,
    /// A monster's bleed: Rending Claws, Rending Frenzy, the Steward's Garrote.
    ///
    /// Separate from the Warrior's own [`StatusKind::Bleeding`] because it stacks and that one
    /// does not, and because what a player's heal can see on a party frame is "the Ghoul's
    /// bleed", not "a bleed". How tall it may stack is the ability's to say, per application.
    Rended,
    /// Slower movement: Crippling Arrow, Gouging Riposte.
    ///
    /// How much slower is the status's [`Status::magnitude`], so one kind serves every slow
    /// in the roster and the most recent one applied is the one that holds.
    Crippled,
    /// More Stamina, so more maximum health. Stacks to three.
    Fortitude,
    /// More Strength.
    BattleShout,
    /// More Intellect.
    ArcaneIntellect,
    /// Haste, damage and speed: the burst buff.
    Bloodlust,
    /// Healing every second.
    Regeneration,
    /// A curse: less damage dealt.
    CurseOfWeakness,
    /// A disease: less Stamina and slower, and a little damage, per stack.
    WitheringPlague,
    /// More magic damage taken, and some physical.
    MagicVulnerability,
    /// The Warrior's reward for a full bar of Valor: more damage dealt and less taken, per
    /// stack, to three.
    ///
    /// Applied by the Warrior's own executor when Valor fills, to the Warrior and to nobody
    /// else. Every number it carries is read from [`crate::warrior::tuning`].
    BattleFervor,
    /// The Plague Explosion modifier's debuff: less healing received.
    ///
    /// Applied by the dungeon modifier's cloud and refreshed while the bearer stands in it, so
    /// it lingers a few seconds after they leave. How much healing it takes is the status's own
    /// [`Status::magnitude`] (the modifier's data), read in the one place health is restored.
    /// **Appended last**: statuses travel the wire by their index in [`StatusKind::ALL`].
    Plagued,
    /// Barkskin Instinct: an ally's share of Wild Rush. The damage reduction is the
    /// status's own [`Status::magnitude`], which the talent's data decides.
    Barkskin,
    /// A trinket's group ward: less damage taken, by the status's own [`Status::magnitude`].
    /// **Appended last**: statuses travel the wire by their index in [`StatusKind::ALL`].
    TrinketWard,
    /// A trinket's group absorb. The shield itself is a pool on the player (see
    /// `crate::trinket::TrinketState`); this is what the frames and the aura row show while it
    /// stands, and it goes when the pool is spent.
    TrinketAbsorb,
    /// The Spiteful Casters modifier's curse: less healing received, for whoever one of its
    /// casters' spells landed on. A Magic debuff, so a dispel takes it off.
    /// **Appended last**: statuses travel the wire by their index in [`StatusKind::ALL`].
    Spiteful,
    /// The Drowned Catacombs' brine: slower, and every blow lands harder. A Magic debuff the
    /// Drowned Cultist's Brine Bolt stacks, so a dispel (or killing the cultist) is the answer.
    /// **Appended last**: statuses travel the wire by their index in [`StatusKind::ALL`].
    Waterlogged,
    /// The Bone Foundry's slag ward: a shielder's blessing on its allies, taking a good share
    /// less of every hit. A buff on a monster, so the answer is to stop the cast or to wait it out.
    /// **Appended last**: statuses travel the wire by their index in [`StatusKind::ALL`].
    Warded,
    /// The Bone Foundry's stoked furnace: a foreman's rally, every blow its bearer lands hitting
    /// harder for a while. A buff on a monster, answered by stopping the cast.
    /// **Appended last**: statuses travel the wire by their index in [`StatusKind::ALL`].
    Stoked,
    /// The Sanctum of Ash's mirror ward: a buff on a monster that sends a share of every hit
    /// it takes back at whoever dealt it, so the answer is to stop the cast, to hit something else,
    /// or to wait it out.
    /// **Appended last**: statuses travel the wire by their index in [`StatusKind::ALL`].
    Mirrored,
    /// The Sanctum of Ash's ashen curse: a dispellable Curse that stacks on a player, taking more
    /// damage and moving slower for each stack.
    /// **Appended last**: statuses travel the wire by their index in [`StatusKind::ALL`].
    Ashen,
    /// The Ashen Pontiff's brand: a dispellable stacking mark on whoever he strikes, each stack
    /// making the next blows heavier. Passed to another tank, dispelled or kicked off the cast.
    /// **Appended last**: statuses travel the wire by their index in [`StatusKind::ALL`].
    Branded,
}

/// How a harmful effect is classified for dispelling.
///
/// The whole of "what kind of thing is this, for the purposes of removing it". A dispel
/// names schools rather than statuses, so a new debuff becomes dispellable by saying which
/// school it is in and by nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DispelSchool {
    /// Spells: burns, exposures, marks.
    Magic,
    /// Poisons.
    Poison,
    /// Curses: Crippled, Curse of Weakness.
    Curse,
    /// Diseases: Withering Plague.
    Disease,
}

impl DispelSchool {
    /// Short uppercase name, for the developer overlay's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Magic => "MAGIC",
            Self::Poison => "POISON",
            Self::Curse => "CURSE",
            Self::Disease => "DISEASE",
        }
    }
}

/// Which damage types an [`EffectModifier`] reaches, as a small set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Schools(u8);

impl Schools {
    /// Every damage type.
    pub const ALL: Self = Self(0b1111);
    /// Weapons and impacts.
    pub const PHYSICAL: Self = Self(1 << 0);
    /// Fire, Arcane and Nature: everything that is not a weapon.
    pub const MAGIC: Self = Self(0b1110);
    /// Nature and Arcane: what Nature's Exposure amplifies.
    pub const NATURE_ARCANE: Self = Self((1 << 2) | (1 << 3));

    /// Whether `damage_type` is in the set.
    #[must_use]
    pub const fn contains(self, damage_type: DamageType) -> bool {
        let bit = match damage_type {
            DamageType::Physical => 1 << 0,
            DamageType::Fire => 1 << 1,
            DamageType::Arcane => 1 << 2,
            DamageType::Nature => 1 << 3,
        };
        self.0 & bit != 0
    }

    /// Short name for a readout: `"all"`, `"physical"`, `"magic"`, `"nature and arcane"`.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self.0 {
            0b1111 => "all",
            0b0001 => "physical",
            0b1110 => "magic",
            0b1100 => "Nature and Arcane",
            _ => "some",
        }
    }
}

/// What one effect does to whoever carries it, besides ticking.
///
/// **The one place an effect's stat change is written.** Gameplay reads these through
/// [`StatusSet::move_speed_multiplier`], [`StatusSet::damage_taken_multiplier`] and the
/// other evaluators below, for the player and a monster alike, so an effect cannot do one
/// thing to the player and another to a skeleton: there is no second copy of the number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectModifier {
    /// A character-sheet stat, flat, per stack. Only an entity with a character sheet — the
    /// player — has stats for it to change.
    Stat {
        /// Which stat.
        stat: crate::character::Stat,
        /// Points per stack. Negative takes away.
        per_stack: f32,
    },
    /// Movement speed, as a fraction per stack: `0.5` is half as fast again, `-0.3` is thirty
    /// per cent slower.
    MoveSpeed {
        /// Fraction per stack.
        per_stack: f32,
    },
    /// Slower by the status's own [`Status::magnitude`]: one kind for every slow, each
    /// application saying how strong it is.
    Slow,
    /// Attack speed, as a fraction per stack. Negative is slower.
    AttackSpeed {
        /// Fraction per stack.
        per_stack: f32,
    },
    /// More (or, negative, less) damage of `schools` taken, as a fraction per stack. Applied
    /// where damage is applied, whoever dealt it.
    DamageTaken {
        /// Which damage types.
        schools: Schools,
        /// Fraction per stack.
        per_stack: f32,
    },
    /// A flat share taken off every hit, equal to the status's own magnitude. Does not stack.
    DamageReductionMagnitude,
    /// A flat share taken off every hit, where hits are mitigated. Does not stack.
    DamageReduction {
        /// Fraction, `0..1`.
        fraction: f32,
    },
    /// More (or, negative, less) damage dealt, as a fraction per stack.
    DamageDealt {
        /// Fraction per stack.
        per_stack: f32,
    },
    /// More (or, negative, less) maximum health, as a fraction per stack. Multiplies the
    /// health Stamina gives, the same way the Warrior's Last Stand does.
    MaxHealth {
        /// Fraction per stack.
        per_stack: f32,
    },
    /// Less healing received, by the status's own [`Status::magnitude`] as a fraction: `0.2`
    /// is twenty per cent less. Applied where health is restored, whoever healed.
    HealingReduced,
    /// A share of every hit the bearer takes is sent back at whoever dealt it, as a fraction
    /// (`0.5` is half). Read where damage is applied, so no source can slip past it.
    Reflect {
        /// The share sent back, `0..1`.
        fraction: f32,
    },
}

/// Whether an effect is something the bearer wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectClass {
    /// Helpful.
    Buff,
    /// Harmful.
    Debuff,
}

/// Everything about one effect, defined once.
///
/// **The global effect registry is [`STATUS_DEFS`]**: one row per [`StatusKind`], read by the
/// simulation when an effect is applied, ticked, dispelled or evaluated, and by the interface
/// for its name, description and border. A player and a monster carrying the same kind carry
/// a [`Status`] that points at the same row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusDef {
    /// Which effect. A row's kind is its index's kind — a test holds the table to it.
    pub kind: StatusKind,
    /// A stable lower-case id: `"natures_exposure"`. Never renamed.
    pub id: &'static str,
    /// Upper case, for the developer overlay's font.
    pub overlay_name: &'static str,
    /// The name a player reads.
    pub name: &'static str,
    /// What it does, in a sentence, for a tooltip.
    pub description: &'static str,
    /// Buff or debuff.
    pub class: EffectClass,
    /// The school a dispel has to name, if any. Only a debuff in a school is dispellable.
    pub dispel: Option<DispelSchool>,
    /// The tallest it may stack on one bearer. One refreshes rather than stacks.
    pub max_stacks: u8,
    /// The kind of damage its ticks deal.
    pub damage_type: DamageType,
    /// Whether its ticks restore health rather than remove it.
    pub heals: bool,
    /// What it does to the bearer's stats while it is on them.
    pub modifiers: &'static [EffectModifier],
    /// What a generic application — one not made by an ability, such as the test bed's —
    /// gives it.
    pub generic: GenericApplication,
    /// Whether anything but its owning system may apply it. Respawn protection is the
    /// lifecycle's own clock and is never applied from outside.
    pub applicable: bool,
}

/// The numbers an effect gets when it is applied by something other than an ability, which
/// has its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenericApplication {
    /// Seconds it lasts unless told otherwise.
    pub duration: f32,
    /// Damage or healing each tick, per stack. Zero for an effect that does not tick.
    pub per_tick: f32,
    /// Its strength, for an effect that reads one (a slow).
    pub magnitude: f32,
}

impl GenericApplication {
    const fn new(duration: f32, per_tick: f32, magnitude: f32) -> Self {
        Self {
            duration,
            per_tick,
            magnitude,
        }
    }
}

/// How many effects exist.
pub const STATUS_KIND_COUNT: usize = 39;

impl StatusKind {
    /// Every effect, in registry order.
    pub const ALL: [Self; STATUS_KIND_COUNT] = [
        Self::Burning,
        Self::Bleeding,
        Self::Recuperating,
        Self::Recovering,
        Self::Sundered,
        Self::Moonfire,
        Self::Sunfire,
        Self::AstralSunfire,
        Self::NaturesExposure,
        Self::Rejuvenation,
        Self::Regrowth,
        Self::Lifebloom,
        Self::WildGrowth,
        Self::AstralWrath,
        Self::WildRush,
        Self::Poisoned,
        Self::RespawnProtection,
        Self::Rended,
        Self::Crippled,
        Self::Fortitude,
        Self::BattleShout,
        Self::ArcaneIntellect,
        Self::Bloodlust,
        Self::Regeneration,
        Self::CurseOfWeakness,
        Self::WitheringPlague,
        Self::MagicVulnerability,
        Self::BattleFervor,
        Self::Plagued,
        Self::Barkskin,
        Self::TrinketWard,
        Self::TrinketAbsorb,
        Self::Spiteful,
        Self::Waterlogged,
        Self::Warded,
        Self::Stoked,
        Self::Mirrored,
        Self::Ashen,
        Self::Branded,
    ];

    /// Its row in [`STATUS_DEFS`].
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Everything about it: the registry row.
    #[must_use]
    pub fn definition(self) -> &'static StatusDef {
        &STATUS_DEFS[self.index()]
    }

    /// The kind whose id is `id`, ignoring case.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim();
        Self::ALL
            .into_iter()
            .find(|kind| kind.definition().id.eq_ignore_ascii_case(id))
    }

    /// Short uppercase name, for the developer overlay's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.definition().overlay_name
    }

    /// The name this effect is shown to a player under.
    ///
    /// [`StatusKind::as_str`] is the *developer overlay's* name: uppercase, because that is
    /// the font the overlay and the unit frames are drawn in. This is the effect's actual
    /// name, in the case it is written in, for anything that needs to print it as prose.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        self.definition().name
    }

    /// What it does, in a sentence.
    #[must_use]
    pub fn description(self) -> &'static str {
        self.definition().description
    }

    /// Which school a dispel has to name to remove this, or `None` if nothing can.
    ///
    /// A bleed is not dispellable, and neither is a buff the bearer wants: what Nature's
    /// Cleanse removes is a *harmful* effect that belongs to a school. Both halves are asked
    /// here - see [`StatusKind::is_harmful`].
    #[must_use]
    pub fn dispel_school(self) -> Option<DispelSchool> {
        self.definition().dispel
    }

    /// Whether the bearer would rather not have this.
    #[must_use]
    pub fn is_harmful(self) -> bool {
        self.definition().class == EffectClass::Debuff
    }

    /// Whether a dispel that names its school may remove it.
    #[must_use]
    pub fn is_dispellable(self) -> bool {
        self.is_harmful() && self.dispel_school().is_some()
    }

    /// Whether a dispel aimed at an *enemy* may strip it: a helpful effect the registry puts
    /// in a school (Mass Dispel's other half).
    ///
    /// The counterpart of [`StatusKind::is_dispellable`], and the same table answers it: a
    /// buff with no school cannot be stripped, and neither can the lifecycle's own
    /// bookkeeping.
    #[must_use]
    pub fn is_purgeable(self) -> bool {
        let definition = self.definition();
        definition.class == EffectClass::Buff
            && definition.dispel.is_some()
            && definition.applicable
    }

    /// Whether this is one of the Druid's own healing-over-time effects.
    ///
    /// What Nourish counts and what Swiftmend is allowed to consume. Asked of the kind
    /// rather than listed at each call site, so the two abilities cannot disagree about what
    /// a Druid heal over time is.
    #[must_use]
    pub fn is_druid_hot(self) -> bool {
        matches!(
            self,
            Self::Rejuvenation | Self::Regrowth | Self::Lifebloom | Self::WildGrowth
        )
    }

    /// The kind of damage this status deals when it ticks.
    #[must_use]
    pub fn damage_type(self) -> DamageType {
        self.definition().damage_type
    }

    /// How many times this status may stack on one bearer.
    ///
    /// One for everything that refreshes rather than stacks, which is most of them. A
    /// property of the kind rather than of each application, so two callers cannot disagree
    /// about how tall a stack of War Stomp is.
    #[must_use]
    pub fn max_stacks(self) -> u8 {
        self.definition().max_stacks
    }

    /// Whether a tick of this status restores health rather than removes it.
    #[must_use]
    pub fn heals(self) -> bool {
        self.definition().heals
    }
}

/// A buff with nothing particular about it, for the rows below to override.
const BUFF: StatusDef = StatusDef {
    kind: StatusKind::Burning,
    id: "",
    overlay_name: "",
    name: "",
    description: "",
    class: EffectClass::Buff,
    dispel: None,
    max_stacks: 1,
    damage_type: DamageType::Nature,
    heals: false,
    modifiers: &[],
    generic: GenericApplication::new(30.0, 0.0, 0.0),
    applicable: true,
};

/// A debuff with nothing particular about it.
const DEBUFF: StatusDef = StatusDef {
    class: EffectClass::Debuff,
    ..BUFF
};

/// **The global effect registry.** One row per [`StatusKind`], in [`StatusKind::ALL`] order.
///
/// The last eight rows are applied by nothing in the shipping roster yet. Like Poisoned
/// before them, they exist so the rules every effect follows — stat changes, curses and
/// diseases, dispels — have something to be written and tested against, and so content can
/// use them by naming them.
pub static STATUS_DEFS: [StatusDef; STATUS_KIND_COUNT] = [
    StatusDef {
        kind: StatusKind::Burning,
        id: "burning",
        overlay_name: "BURNING",
        name: "Burning",
        description: "Taking fire damage every second.",
        dispel: Some(DispelSchool::Magic),
        damage_type: DamageType::Fire,
        generic: GenericApplication::new(10.0, 8.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Bleeding,
        id: "bleeding",
        overlay_name: "BLEEDING",
        name: "Bleeding",
        description: "Bleeding for a share of the Warrior's maximum health every second.",
        damage_type: DamageType::Physical,
        generic: GenericApplication::new(10.0, 8.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Recuperating,
        id: "recuperating",
        overlay_name: "RECUPERATE",
        // The ability's own name, so the buff the button gives reads as the button.
        name: "Recuperate",
        description: "Healing a share of maximum health every second.",
        max_stacks: 5,
        damage_type: DamageType::Physical,
        heals: true,
        generic: GenericApplication::new(10.0, 5.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Recovering,
        id: "recovering",
        overlay_name: "RECOVERING",
        name: "Recovering",
        description: "Healing back the damage Adrenaline banked.",
        damage_type: DamageType::Physical,
        heals: true,
        generic: GenericApplication::new(6.0, 5.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Sundered,
        id: "sundered",
        overlay_name: "SUNDERED",
        name: "Sundered",
        description: "Attacking 2% slower per stack.",
        dispel: Some(DispelSchool::Magic),
        max_stacks: 8,
        damage_type: DamageType::Physical,
        modifiers: &[EffectModifier::AttackSpeed {
            per_stack: -WAR_STOMP_ATTACK_SPEED_PER_STACK,
        }],
        generic: GenericApplication::new(12.0, 0.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Moonfire,
        id: "moonfire",
        overlay_name: "MOONFIRE",
        name: "Moonfire",
        description: "Taking Nature damage every second.",
        dispel: Some(DispelSchool::Magic),
        generic: GenericApplication::new(16.0, 8.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Sunfire,
        id: "sunfire",
        overlay_name: "SUNFIRE",
        name: "Sunfire",
        description: "Taking Nature damage every second.",
        dispel: Some(DispelSchool::Magic),
        generic: GenericApplication::new(12.0, 8.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::AstralSunfire,
        id: "astral_sunfire",
        overlay_name: "ASTRAL SUNFIRE",
        name: "Astral Sunfire",
        description: "Damage dealt to this target heals the party for a share of it.",
        dispel: Some(DispelSchool::Magic),
        generic: GenericApplication::new(12.0, 0.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::NaturesExposure,
        id: "natures_exposure",
        overlay_name: "NATURES EXPOSURE",
        name: "Nature's Exposure",
        description: "Taking 2% more Nature and Arcane damage per stack.",
        dispel: Some(DispelSchool::Magic),
        // The table's ceiling: the base maximum plus everything Deep Exposure can add. Each
        // application narrows it to the caster's own (`Status::stacking_to`).
        max_stacks: NATURES_EXPOSURE_STACK_CEILING,
        modifiers: &[EffectModifier::DamageTaken {
            schools: Schools::NATURE_ARCANE,
            per_stack: NATURES_EXPOSURE_PER_STACK,
        }],
        generic: GenericApplication::new(15.0, 0.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Rejuvenation,
        id: "rejuvenation",
        overlay_name: "REJUVENATION",
        name: "Rejuvenation",
        description: "Healing every second.",
        heals: true,
        generic: GenericApplication::new(12.0, 10.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Regrowth,
        id: "regrowth",
        overlay_name: "REGROWTH",
        name: "Regrowth",
        description: "Healing every second.",
        heals: true,
        generic: GenericApplication::new(12.0, 10.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Lifebloom,
        id: "lifebloom",
        overlay_name: "LIFEBLOOM",
        name: "Lifebloom",
        description: "Healing every second, and blooming when it ends.",
        heals: true,
        generic: GenericApplication::new(10.0, 6.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::WildGrowth,
        id: "wild_growth",
        overlay_name: "WILD GROWTH",
        name: "Wild Growth",
        description: "Healing every second.",
        heals: true,
        generic: GenericApplication::new(7.0, 10.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::AstralWrath,
        id: "astral_wrath",
        overlay_name: "ASTRAL WRATH",
        name: "Astral Wrath",
        description: "Wrath spreads Nature's Exposure around its target.",
        generic: GenericApplication::new(10.0, 0.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::WildRush,
        id: "wild_rush",
        overlay_name: "WILD RUSH",
        name: "Wild Rush",
        description: "50% faster and taking 30% less damage.",
        modifiers: &[
            EffectModifier::MoveSpeed {
                per_stack: WILD_RUSH_SPEED,
            },
            EffectModifier::DamageReduction {
                fraction: WILD_RUSH_REDUCTION,
            },
        ],
        generic: GenericApplication::new(6.0, 0.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Poisoned,
        id: "poisoned",
        overlay_name: "POISONED",
        name: "Poisoned",
        description: "Taking Nature damage every second.",
        dispel: Some(DispelSchool::Poison),
        generic: GenericApplication::new(12.0, 6.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::RespawnProtection,
        id: "respawn_protection",
        overlay_name: "CANNOT BE RESPAWNED",
        name: "Cannot be respawned",
        description: "Recently respawned: cannot be respawned again yet.",
        // Not harmful: it is the thing standing between the player and being killed
        // repeatedly, and it is over in twenty seconds. Not dispellable: it is the
        // lifecycle's own bookkeeping, and a dispel that removed it could delete a player.
        generic: GenericApplication::new(20.0, 0.0, 0.0),
        applicable: false,
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Rended,
        id: "rended",
        overlay_name: "RENDED",
        name: "Rended",
        description: "Bleeding every second. Stacks.",
        // The ceiling a single application may raise; each ability narrows it to its own
        // with `Status::max_stacks`, so the Ghoul's eight and the empowered Ghoul's ten are
        // the table's numbers, not this one.
        max_stacks: 10,
        damage_type: DamageType::Physical,
        generic: GenericApplication::new(10.0, 4.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Crippled,
        id: "crippled",
        overlay_name: "CRIPPLED",
        name: "Crippled",
        description: "Moving slower.",
        // A crippling shot is a curse on the legs: the Druid can take it off. A bleed is a
        // wound, and nothing removes a wound with a spell.
        dispel: Some(DispelSchool::Curse),
        damage_type: DamageType::Physical,
        modifiers: &[EffectModifier::Slow],
        generic: GenericApplication::new(8.0, 0.0, 0.4),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Fortitude,
        id: "fortitude",
        overlay_name: "FORTITUDE",
        name: "Fortitude",
        description: "20 more Stamina per stack.",
        // The buffs below are spells: a Mass Dispel strips them from an enemy.
        dispel: Some(DispelSchool::Magic),
        max_stacks: 3,
        modifiers: &[EffectModifier::Stat {
            stat: Stat::Stamina,
            per_stack: 20.0,
        }],
        generic: GenericApplication::new(60.0, 0.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::BattleShout,
        id: "battle_shout",
        overlay_name: "BATTLE SHOUT",
        name: "Battle Shout",
        description: "40 more Strength.",
        dispel: Some(DispelSchool::Magic),
        modifiers: &[EffectModifier::Stat {
            stat: Stat::Strength,
            per_stack: 40.0,
        }],
        generic: GenericApplication::new(60.0, 0.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::ArcaneIntellect,
        id: "arcane_intellect",
        overlay_name: "ARCANE INTELLECT",
        name: "Arcane Intellect",
        description: "40 more Intellect.",
        dispel: Some(DispelSchool::Magic),
        modifiers: &[EffectModifier::Stat {
            stat: Stat::Intellect,
            per_stack: 40.0,
        }],
        generic: GenericApplication::new(60.0, 0.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Bloodlust,
        id: "bloodlust",
        overlay_name: "BLOODLUST",
        name: "Bloodlust",
        description: "30% more haste, 20% more damage dealt and 10% faster.",
        dispel: Some(DispelSchool::Magic),
        modifiers: &[
            EffectModifier::Stat {
                stat: Stat::Haste,
                per_stack: crate::character::stat::rating_for_fraction(0.30),
            },
            EffectModifier::DamageDealt { per_stack: 0.20 },
            EffectModifier::MoveSpeed { per_stack: 0.10 },
        ],
        generic: GenericApplication::new(40.0, 0.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Regeneration,
        id: "regeneration",
        overlay_name: "REGENERATION",
        name: "Regeneration",
        description: "Healing every second.",
        dispel: Some(DispelSchool::Magic),
        heals: true,
        generic: GenericApplication::new(20.0, 12.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::CurseOfWeakness,
        id: "curse_of_weakness",
        overlay_name: "CURSE OF WEAKNESS",
        name: "Curse of Weakness",
        description: "Dealing 25% less damage.",
        dispel: Some(DispelSchool::Curse),
        modifiers: &[EffectModifier::DamageDealt { per_stack: -0.25 }],
        generic: GenericApplication::new(30.0, 0.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::WitheringPlague,
        id: "withering_plague",
        overlay_name: "WITHERING PLAGUE",
        name: "Withering Plague",
        description: "15 less Stamina and 10% slower per stack, and Nature damage every second.",
        dispel: Some(DispelSchool::Disease),
        max_stacks: 3,
        modifiers: &[
            EffectModifier::Stat {
                stat: Stat::Stamina,
                per_stack: -15.0,
            },
            EffectModifier::MoveSpeed { per_stack: -0.10 },
        ],
        generic: GenericApplication::new(24.0, 3.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::MagicVulnerability,
        id: "magic_vulnerability",
        overlay_name: "MAGIC VULNERABILITY",
        name: "Magic Vulnerability",
        description: "Taking 15% more magic and 5% more physical damage per stack.",
        dispel: Some(DispelSchool::Magic),
        max_stacks: 3,
        modifiers: &[
            EffectModifier::DamageTaken {
                schools: Schools::MAGIC,
                per_stack: 0.15,
            },
            EffectModifier::DamageTaken {
                schools: Schools::PHYSICAL,
                per_stack: 0.05,
            },
        ],
        generic: GenericApplication::new(24.0, 0.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::BattleFervor,
        id: "battle_fervor",
        overlay_name: "BATTLE FERVOR",
        name: "Battle Fervor",
        description: "10% more damage dealt and 3% less damage taken per stack.",
        max_stacks: FERVOR_MAX_STACKS,
        modifiers: &[
            EffectModifier::DamageDealt {
                per_stack: FERVOR_DAMAGE_PER_STACK,
            },
            EffectModifier::DamageTaken {
                schools: Schools::ALL,
                per_stack: -FERVOR_DAMAGE_TAKEN_PER_STACK,
            },
            EffectModifier::MaxHealth {
                per_stack: FERVOR_MAX_HEALTH_PER_STACK,
            },
        ],
        generic: GenericApplication::new(FERVOR_DURATION, 0.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Plagued,
        id: "plagued",
        overlay_name: "PLAGUED",
        name: "Plagued",
        description: "Healing received is reduced while the plague clings to you.",
        dispel: Some(DispelSchool::Disease),
        modifiers: &[EffectModifier::HealingReduced],
        generic: GenericApplication::new(3.0, 0.0, 0.2),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Barkskin,
        id: "barkskin",
        overlay_name: "BARKSKIN",
        name: "Barkskin",
        description: "Taking less damage, shared by a Druid's Wild Rush.",
        modifiers: &[EffectModifier::DamageReductionMagnitude],
        generic: GenericApplication::new(4.0, 0.0, 0.25),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::TrinketWard,
        id: "trinket_ward",
        overlay_name: "WARD",
        name: "Rallying Ward",
        description: "Taking less damage from every source.",
        modifiers: &[EffectModifier::DamageReductionMagnitude],
        generic: GenericApplication::new(8.0, 0.0, 0.15),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::TrinketAbsorb,
        id: "trinket_absorb",
        overlay_name: "AEGIS",
        name: "Aegis",
        description: "A shield takes damage before health does.",
        // The pool is the player's (`TrinketState::absorb`); this only says it stands.
        generic: GenericApplication::new(10.0, 0.0, 0.20),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Spiteful,
        id: "spiteful",
        overlay_name: "SPITE",
        name: "Spite",
        description: "Healing received is reduced by a caster's spite. Dispel it.",
        dispel: Some(DispelSchool::Magic),
        modifiers: &[EffectModifier::HealingReduced],
        generic: GenericApplication::new(10.0, 0.0, 0.3),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Waterlogged,
        id: "waterlogged",
        overlay_name: "WATERLOGGED",
        name: "Waterlogged",
        description: "Moving 15% slower and taking 8% more damage per stack. Dispel it.",
        // A drowning spell, not a wound: a dispel takes every stack at once.
        dispel: Some(DispelSchool::Magic),
        max_stacks: 3,
        modifiers: &[
            EffectModifier::MoveSpeed { per_stack: -0.15 },
            EffectModifier::DamageTaken {
                schools: Schools::ALL,
                per_stack: 0.08,
            },
        ],
        generic: GenericApplication::new(12.0, 0.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Warded,
        id: "warded",
        overlay_name: "WARDED",
        name: "Slag Ward",
        description: "Taking 40% less damage from every source.",
        modifiers: &[EffectModifier::DamageTaken {
            schools: Schools::ALL,
            per_stack: -0.40,
        }],
        generic: GenericApplication::new(8.0, 0.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Stoked,
        id: "stoked",
        overlay_name: "STOKED",
        name: "Stoked",
        description: "Dealing 30% more damage.",
        modifiers: &[EffectModifier::DamageDealt { per_stack: 0.30 }],
        generic: GenericApplication::new(10.0, 0.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Mirrored,
        id: "mirrored",
        overlay_name: "MIRRORED",
        name: "Mirror Ward",
        description: "Sending half of all damage taken back at whoever dealt it.",
        modifiers: &[EffectModifier::Reflect { fraction: 0.5 }],
        generic: GenericApplication::new(6.0, 0.0, 0.0),
        ..BUFF
    },
    StatusDef {
        kind: StatusKind::Ashen,
        id: "ashen",
        overlay_name: "ASHEN",
        name: "Ashen Curse",
        description: "Taking 6% more damage and moving 3% slower per stack. Dispel it.",
        // A curse, not a wound: a dispel takes every stack at once.
        dispel: Some(DispelSchool::Curse),
        max_stacks: 5,
        modifiers: &[
            EffectModifier::DamageTaken {
                schools: Schools::ALL,
                per_stack: 0.06,
            },
            EffectModifier::MoveSpeed { per_stack: -0.03 },
        ],
        generic: GenericApplication::new(20.0, 0.0, 0.0),
        ..DEBUFF
    },
    StatusDef {
        kind: StatusKind::Branded,
        id: "branded",
        overlay_name: "BRANDED",
        name: "Pontiff's Brand",
        description: "Taking 12% more damage per stack. Dispel it, or pass the Pontiff to another tank.",
        dispel: Some(DispelSchool::Magic),
        max_stacks: 8,
        modifiers: &[EffectModifier::DamageTaken {
            schools: Schools::ALL,
            per_stack: 0.12,
        }],
        generic: GenericApplication::new(20.0, 0.0, 0.0),
        ..DEBUFF
    },
];

/// Why a status ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusEnd {
    /// Its duration ran out.
    Expired,
    /// An ability consumed it.
    Consumed,
    /// It was removed wholesale — a dummy reset, or the bearer dying.
    Cleared,
}

/// One active status on one entity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Status {
    /// Which status this is.
    pub kind: StatusKind,
    /// Who applied it. Damage it deals is credited to them.
    pub source: EntityId,
    /// Which ability applied it. Damage it deals is attributed to that ability, so the
    /// meter shows "IGNITE 58" rather than an anonymous damage-over-time bucket.
    pub ability: AbilityId,
    /// Seconds left.
    pub remaining: f32,
    /// Full duration, kept so the overlay can draw a fraction.
    pub duration: f32,
    /// Damage each tick deals, per stack, as a flat number decided when it was applied.
    pub damage_per_tick: f32,
    /// Damage each tick deals, per stack, as a fraction of the **source's maximum health at
    /// the moment the tick happens**.
    ///
    /// The whole of "this effect does not snapshot". The status carries a *coefficient*, not
    /// a number of points, so there is structurally nowhere for a stale maximum health to be
    /// stored — the simulation multiplies it out against the live value when it applies the
    /// outcome. Colossus Strike's bleed is the reason, and a Last Stand cast while it is
    /// running makes the rest of it hit harder because of this field and nothing else.
    pub source_health_per_tick: f32,
    /// How many stacks are on the bearer. At least one.
    pub stacks: u8,
    /// How many it may reach. From [`StatusKind::max_stacks`] unless overridden.
    pub max_stacks: u8,
    /// Seconds between ticks. [`STATUS_TICK_INTERVAL`] unless the ability says otherwise.
    pub interval: f32,
    /// Whether haste shortens the gap between its ticks.
    ///
    /// Opt-in, and read from the *current* stats on every step rather than folded into
    /// [`Status::interval`] when it was applied: a Druid who gains haste while a Moonfire is
    /// running ticks faster for the rest of it, which is the same no-snapshot rule Ignite
    /// already follows.
    pub hasted: bool,
    /// Whether its duration runs at all.
    ///
    /// **The whole of "this effect does not expire".** An indefinite status is removed by
    /// the bearer dying, by the encounter resetting or by something consuming it, and by
    /// nothing else - which is exactly the list Astral Moonfire is specified with. It is a
    /// flag rather than a very large duration so that "forever" cannot be reached by
    /// accident and so the interface can say so instead of printing a number.
    pub indefinite: bool,
    /// Whether the ability that applied it spent Astral Power.
    ///
    /// Carried for the interface and for the reset rules; the *magnitude* of the
    /// empowerment is already in the numbers beside it.
    pub empowered: bool,
    /// Healing paid out when it runs out of time on its own.
    ///
    /// Lifebloom's bloom, and the only thing in the game that uses it. Paid on
    /// [`StatusEnd::Expired`] and on nothing else: a bloom that also paid when the status
    /// was consumed or cleared would make dispelling a Lifebloom a heal.
    pub expiry_heal: f32,
    /// A strength that is neither damage nor healing: how much a slow slows, as a fraction.
    ///
    /// Zero for every status that does not read it.
    pub magnitude: f32,
    /// Seconds until the next tick.
    timer: f32,
}

impl Status {
    /// A freshly applied status.
    #[must_use]
    pub fn new(
        kind: StatusKind,
        source: EntityId,
        ability: AbilityId,
        duration: f32,
        damage_per_tick: f32,
    ) -> Self {
        let duration = duration.max(0.0);
        Self {
            kind,
            source,
            ability,
            remaining: duration,
            duration,
            damage_per_tick: damage_per_tick.max(0.0),
            source_health_per_tick: 0.0,
            stacks: 1,
            max_stacks: kind.max_stacks(),
            interval: STATUS_TICK_INTERVAL,
            hasted: false,
            indefinite: false,
            empowered: false,
            expiry_heal: 0.0,
            magnitude: 0.0,
            timer: STATUS_TICK_INTERVAL,
        }
    }

    /// The same, with a strength: how much a slow slows.
    #[must_use]
    pub fn with_magnitude(mut self, magnitude: f32) -> Self {
        self.magnitude = if magnitude.is_finite() {
            magnitude.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self
    }

    /// The same, allowed to stack only to `max` rather than to the kind's ceiling.
    #[must_use]
    pub fn stacking_to(mut self, max: u8) -> Self {
        self.max_stacks = max.clamp(1, self.kind.max_stacks().max(1));
        self.stacks = self.stacks.min(self.max_stacks);
        self
    }

    /// The same, ticking faster the more haste the caster has.
    #[must_use]
    pub fn with_haste(mut self) -> Self {
        self.hasted = true;
        self
    }

    /// The same, lasting until something removes it.
    ///
    /// Death, an encounter reset and a consumer are the only three things that can. See
    /// [`Status::indefinite`].
    #[must_use]
    pub fn forever(mut self) -> Self {
        self.indefinite = true;
        self
    }

    /// The same, marked as having been paid for with Astral Power.
    #[must_use]
    pub fn empowered_by_astral(mut self, empowered: bool) -> Self {
        self.empowered = empowered;
        self
    }

    /// The same, healing for `amount` when it expires naturally. Lifebloom's bloom.
    #[must_use]
    pub fn blooming(mut self, amount: f32) -> Self {
        self.expiry_heal = amount.max(0.0);
        self
    }

    /// The same, ticking every `interval` seconds instead of every second.
    #[must_use]
    pub fn every(mut self, interval: f32) -> Self {
        if interval > 0.0 {
            self.interval = interval;
            self.timer = interval;
        }
        self
    }

    /// The same, with each tick also worth `fraction` of the source's **current** maximum
    /// health.
    #[must_use]
    pub fn scaling_with_source_health(mut self, fraction: f32) -> Self {
        self.source_health_per_tick = fraction.max(0.0);
        self
    }

    /// The same, applied `stacks` deep rather than one.
    ///
    /// Clamped to the kind's maximum here rather than at the call site, so an empowered
    /// application that grants two stacks cannot overshoot a cap it does not know about.
    #[must_use]
    pub fn with_stacks(mut self, stacks: u8) -> Self {
        self.stacks = stacks.clamp(1, self.max_stacks.max(1));
        self
    }

    /// What one tick is worth against a source whose maximum health is `source_max_health`.
    ///
    /// Stacks included. The one place the two halves of a tick — the flat part and the part
    /// that reads live maximum health — are added together.
    #[must_use]
    pub fn tick_amount(&self, source_max_health: f32) -> f32 {
        let per_stack =
            self.damage_per_tick + self.source_health_per_tick * source_max_health.max(0.0);
        per_stack * f32::from(self.stacks.max(1))
    }

    /// How much of the duration is left, in [0, 1].
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.indefinite {
            return 1.0;
        }
        if self.duration <= 0.0 {
            return 0.0;
        }
        (self.remaining / self.duration).clamp(0.0, 1.0)
    }
}

/// Something a status did during one simulation step.
///
/// Returned rather than applied, so the status system stays a pure function of time and
/// the simulation remains the only place that changes health.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StatusOutcome {
    /// The status ticked.
    Tick {
        /// Which status ticked.
        kind: StatusKind,
        /// Who is credited.
        source: EntityId,
        /// Which ability is credited.
        ability: AbilityId,
        /// The flat part of the tick, stacks already multiplied in.
        damage: f32,
        /// The part that has still to be multiplied by the **source's** current maximum
        /// health, stacks already multiplied in.
        ///
        /// Handed out rather than resolved here because a [`StatusSet`] is a pure function
        /// of time and knows nothing about who is in the world. The simulation multiplies it
        /// against the live value on the tick it is applied, which is what makes the bleed
        /// unsnapshotted.
        source_health_fraction: f32,
        /// How many stacks produced it, for the meter and the log.
        stacks: u8,
        /// Whether this restores health instead of removing it.
        heals: bool,
    },
    /// The status ended.
    Ended {
        /// Which status ended.
        kind: StatusKind,
        /// Why.
        reason: StatusEnd,
        /// Who applied it. Whatever its ending pays out is credited to them.
        source: EntityId,
        /// Which ability applied it.
        ability: AbilityId,
        /// Healing its natural expiry owes, already zero unless it expired.
        ///
        /// Handed out rather than applied here for the same reason a tick's damage is: a
        /// [`StatusSet`] is a pure function of time and knows nothing about who is in the
        /// world.
        expiry_heal: f32,
    },
}

/// The statuses on one entity.
///
/// A `Vec` with a reused allocation rather than a fixed array: the set is empty for most
/// entities most of the time, one entry deep in practice, and growing it later must not be
/// a structural change. Clearing keeps the capacity, so an arena that has been reset a
/// hundred times allocates as much as one that has never been.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StatusSet {
    entries: Vec<Status>,
}

impl StatusSet {
    /// No statuses.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a status, refreshing one of the same kind rather than stacking it.
    ///
    /// Refresh, not stack: two burns from the same caster ticking independently doubles the
    /// damage of spamming one button, which is a balance decision this prototype has no
    /// reason to make. Returns `true` if the status was newly applied.
    pub fn apply(&mut self, status: Status) -> bool {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|entry| entry.kind == status.kind)
        {
            // The timer is *not* reset: a refresh extends the burn, it does not delay the
            // next tick, which would let a refresh every 0.9 s produce no damage at all.
            let timer = existing.timer;
            // A stacking status adds what the new application brought and keeps its own
            // ceiling; everything else replaces, which is the old refresh behaviour written
            // as the one-stack case of the same rule.
            let stacks = existing
                .stacks
                .saturating_add(status.stacks)
                .min(status.max_stacks.max(1));
            *existing = status;
            existing.timer = timer;
            existing.stacks = stacks;
            return false;
        }
        self.entries.push(status);
        true
    }

    /// The active status of a kind, if there is one.
    #[must_use]
    pub fn get(&self, kind: StatusKind) -> Option<&Status> {
        self.entries.iter().find(|entry| entry.kind == kind)
    }

    /// Whether a status of this kind is active.
    #[must_use]
    pub fn has(&self, kind: StatusKind) -> bool {
        self.get(kind).is_some()
    }

    /// How many stacks of a status the bearer has, or zero if it is not there.
    #[must_use]
    pub fn stacks(&self, kind: StatusKind) -> u8 {
        self.get(kind).map_or(0, |status| status.stacks)
    }

    /// Seconds left on a status, or zero if it is not there.
    #[must_use]
    pub fn remaining(&self, kind: StatusKind) -> f32 {
        self.get(kind).map_or(0.0, |status| status.remaining)
    }

    /// Removes a status and returns it, for an ability that consumes one.
    pub fn consume(&mut self, kind: StatusKind) -> Option<Status> {
        let index = self.entries.iter().position(|entry| entry.kind == kind)?;
        Some(self.entries.remove(index))
    }

    /// Active statuses.
    pub fn iter(&self) -> impl Iterator<Item = &Status> {
        self.entries.iter()
    }

    /// How many statuses are active.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is active.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes everything, keeping the allocation, and reports each removal.
    pub fn clear_into(&mut self, out: &mut Vec<StatusOutcome>) {
        for entry in self.entries.drain(..) {
            out.push(StatusOutcome::Ended {
                kind: entry.kind,
                reason: StatusEnd::Cleared,
                source: entry.source,
                ability: entry.ability,
                // Cleared, not expired: a Lifebloom that went with a corpse does not bloom.
                expiry_heal: 0.0,
            });
        }
    }

    /// Removes everything without reporting it. For arena setup and tests.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    // ------------------------------------------------------------------------------------
    // What the effects on one bearer do to it, read from the registry. Every one of these is
    // what gameplay multiplies by, for the player and for a monster alike.

    /// Every modifier of every active status, with the status it belongs to.
    fn modifiers(&self) -> impl Iterator<Item = (&Status, &'static EffectModifier)> + '_ {
        self.entries.iter().flat_map(|status| {
            status
                .kind
                .definition()
                .modifiers
                .iter()
                .map(move |modifier| (status, modifier))
        })
    }

    /// What the bearer's movement speed is multiplied by. One when nothing changes it.
    #[must_use]
    pub fn move_speed_multiplier(&self) -> f32 {
        let mut multiplier = 1.0;
        for (status, modifier) in self.modifiers() {
            match *modifier {
                EffectModifier::MoveSpeed { per_stack } => {
                    multiplier *= (1.0 + per_stack * f32::from(status.stacks)).max(0.0);
                }
                // A slow is the status's own strength, so every slow in the roster is one
                // status with one number on it.
                EffectModifier::Slow => multiplier *= 1.0 - status.magnitude,
                _ => {}
            }
        }
        multiplier.clamp(0.1, 4.0)
    }

    /// What the bearer's attack speed is multiplied by: below one is slower.
    #[must_use]
    pub fn attack_speed_scale(&self) -> f32 {
        let mut scale = 1.0;
        for (status, modifier) in self.modifiers() {
            if let EffectModifier::AttackSpeed { per_stack } = *modifier {
                scale *= (1.0 + per_stack * f32::from(status.stacks)).max(0.0);
            }
        }
        scale.clamp(0.1, 4.0)
    }

    /// What a hit of `damage_type` on the bearer is multiplied by.
    #[must_use]
    pub fn damage_taken_multiplier(&self, damage_type: DamageType) -> f32 {
        let mut multiplier = 1.0;
        for (status, modifier) in self.modifiers() {
            if let EffectModifier::DamageTaken { schools, per_stack } = *modifier
                && schools.contains(damage_type)
            {
                multiplier *= (1.0 + per_stack * f32::from(status.stacks)).max(0.0);
            }
        }
        multiplier
    }

    /// The share every hit on the bearer loses where hits are mitigated, `0..1`.
    #[must_use]
    pub fn damage_reduction(&self) -> f32 {
        let mut kept = 1.0;
        for (status, modifier) in self.modifiers() {
            match *modifier {
                EffectModifier::DamageReduction { fraction } => {
                    kept *= 1.0 - fraction.clamp(0.0, 1.0);
                }
                EffectModifier::DamageReductionMagnitude => {
                    kept *= 1.0 - status.magnitude.clamp(0.0, 1.0);
                }
                _ => {}
            }
        }
        1.0 - kept
    }

    /// What healing the bearer receives is multiplied by. One when nothing reduces it.
    ///
    /// Read by [`crate::world::GameWorld::heal`], the one place health is restored, so a
    /// reduction cannot be bypassed by a heal that forgot to ask.
    #[must_use]
    pub fn healing_received_multiplier(&self) -> f32 {
        let mut multiplier = 1.0;
        for (status, modifier) in self.modifiers() {
            if matches!(modifier, EffectModifier::HealingReduced) {
                multiplier *= 1.0 - status.magnitude.clamp(0.0, 1.0);
            }
        }
        multiplier
    }

    /// The share of every hit on the bearer that is sent back at its author, `0..1`.
    #[must_use]
    pub fn reflect_fraction(&self) -> f32 {
        let mut kept = 1.0;
        for (_, modifier) in self.modifiers() {
            if let EffectModifier::Reflect { fraction } = *modifier {
                kept *= 1.0 - fraction.clamp(0.0, 1.0);
            }
        }
        1.0 - kept
    }

    /// What the bearer's damage is multiplied by.
    #[must_use]
    pub fn damage_dealt_multiplier(&self) -> f32 {
        let mut multiplier = 1.0;
        for (status, modifier) in self.modifiers() {
            if let EffectModifier::DamageDealt { per_stack } = *modifier {
                multiplier *= (1.0 + per_stack * f32::from(status.stacks)).max(0.0);
            }
        }
        multiplier
    }

    /// What the bearer's maximum health is multiplied by. One when nothing changes it.
    #[must_use]
    pub fn max_health_multiplier(&self) -> f32 {
        let mut multiplier = 1.0;
        for (status, modifier) in self.modifiers() {
            if let EffectModifier::MaxHealth { per_stack } = *modifier {
                multiplier *= (1.0 + per_stack * f32::from(status.stacks)).max(0.0);
            }
        }
        multiplier
    }

    /// The character-sheet stats the bearer's effects add (or take away).
    #[must_use]
    pub fn stat_bonus(&self) -> crate::character::StatBlock {
        let mut bonus = crate::character::StatBlock::default();
        for (status, modifier) in self.modifiers() {
            if let EffectModifier::Stat { stat, per_stack } = *modifier {
                bonus.add(stat, per_stack * f32::from(status.stacks));
            }
        }
        bonus
    }

    /// Removes every status `source` applied, without reporting it, and returns how many
    /// went.
    ///
    /// What taking an entity out of the world entirely leaves to clean up: a bleed whose
    /// author no longer exists is not a death the game is simulating but a leftover, and a
    /// test bed that despawns a monster must not leave its poison ticking on the player.
    pub fn remove_from(&mut self, source: EntityId) -> usize {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.source != source);
        before - self.entries.len()
    }

    /// Advances every status by one simulation step, appending what happened to `out`.
    ///
    /// A status that expires on the same step it ticks does both, in that order: the last
    /// second of a six-second burn is a second the target was burning, and dropping its
    /// damage would make the total depend on where the tick boundary fell.
    pub fn tick(&mut self, seconds: f32, haste_scale: f32, out: &mut Vec<StatusOutcome>) {
        if seconds <= 0.0 || self.entries.is_empty() {
            return;
        }
        // A scale of one is "no haste", and anything nonsensical is treated as that: a NaN
        // here would stop every hasted effect in the world from ever ticking again.
        let haste_scale = if haste_scale.is_finite() && haste_scale > 0.0 {
            haste_scale
        } else {
            1.0
        };

        let mut index = 0;
        while index < self.entries.len() {
            let entry = &mut self.entries[index];
            // An indefinite status's clock does not run. Its ticks still do: what "forever"
            // takes away is the expiry, not the damage.
            if !entry.indefinite {
                entry.remaining = (entry.remaining - seconds).max(0.0);
                // Snapped rather than left as a residue: see `crate::TIME_EPSILON`. Without
                // this a six-second burn ticks five times, because the sixty-first
                // subtraction of an inexact 1/60 leaves a few millionths of a second behind.
                if entry.remaining < crate::TIME_EPSILON {
                    entry.remaining = 0.0;
                }
            }
            // **Where haste goes.** The interval is left alone and the *clock* is run faster,
            // against the stats of this step rather than the ones the status was applied
            // with. Doing it the other way round — shortening the interval — would leave the
            // first tick of an already-running effect scheduled at the old gap, because the
            // countdown to it was started before the haste existed.
            entry.timer -= if entry.hasted {
                seconds / haste_scale
            } else {
                seconds
            };

            let interval = if entry.interval > 0.0 {
                entry.interval
            } else {
                STATUS_TICK_INTERVAL
            };
            let stacks = entry.stacks.max(1);
            while entry.timer <= crate::TIME_EPSILON {
                entry.timer += interval;
                let damage = entry.damage_per_tick * f32::from(stacks);
                let fraction = entry.source_health_per_tick * f32::from(stacks);
                if damage > 0.0 || fraction > 0.0 {
                    out.push(StatusOutcome::Tick {
                        kind: entry.kind,
                        source: entry.source,
                        ability: entry.ability,
                        damage,
                        source_health_fraction: fraction,
                        stacks,
                        heals: entry.kind.heals(),
                    });
                }
            }

            if !entry.indefinite && entry.remaining <= 0.0 {
                let ended = *entry;
                self.entries.remove(index);
                out.push(StatusOutcome::Ended {
                    kind: ended.kind,
                    reason: StatusEnd::Expired,
                    source: ended.source,
                    ability: ended.ability,
                    expiry_heal: ended.expiry_heal,
                });
            } else {
                index += 1;
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const STEP: f32 = 1.0 / 60.0;

    fn burn(duration: f32, damage: f32) -> Status {
        Status::new(
            StatusKind::Burning,
            EntityId::new(0, 1),
            AbilityId::from_slot(1).expect("slot 1 exists"),
            duration,
            damage,
        )
    }

    fn total_damage(outcomes: &[StatusOutcome]) -> f32 {
        outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                StatusOutcome::Tick { damage, .. } => Some(*damage),
                StatusOutcome::Ended { .. } => None,
            })
            .sum()
    }

    #[test]
    fn applying_a_status_makes_it_active_with_its_full_duration() {
        let mut statuses = StatusSet::new();
        assert!(statuses.apply(burn(6.0, 8.0)));

        assert!(statuses.has(StatusKind::Burning));
        assert_eq!(statuses.remaining(StatusKind::Burning), 6.0);
        assert_eq!(statuses.len(), 1);
        assert_eq!(
            statuses
                .get(StatusKind::Burning)
                .map(super::Status::fraction),
            Some(1.0)
        );
    }

    #[test]
    fn a_status_ticks_once_a_second_for_its_whole_duration() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();

        for _ in 0..(6 * 60) {
            statuses.tick(STEP, 1.0, &mut outcomes);
        }

        // Six seconds, one tick a second, eight damage a tick.
        assert!(
            (total_damage(&outcomes) - 48.0).abs() < 1e-3,
            "{}",
            total_damage(&outcomes)
        );
        assert!(!statuses.has(StatusKind::Burning), "the burn expired");
        assert!(outcomes.iter().any(|outcome| matches!(
            outcome,
            StatusOutcome::Ended {
                kind: StatusKind::Burning,
                reason: StatusEnd::Expired,
                ..
            }
        )));
    }

    #[test]
    fn the_last_tick_is_not_lost_to_the_expiry_on_the_same_step() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(1.0, 5.0));
        let mut outcomes = Vec::new();
        for _ in 0..60 {
            statuses.tick(STEP, 1.0, &mut outcomes);
        }
        assert!((total_damage(&outcomes) - 5.0).abs() < 1e-3);
    }

    #[test]
    fn reapplying_refreshes_the_duration_without_stacking() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();
        for _ in 0..(3 * 60) {
            statuses.tick(STEP, 1.0, &mut outcomes);
        }
        assert!((statuses.remaining(StatusKind::Burning) - 3.0).abs() < 1e-3);

        assert!(
            !statuses.apply(burn(6.0, 8.0)),
            "a refresh is not a new application"
        );
        assert_eq!(statuses.len(), 1, "statuses refresh rather than stack");
        assert!((statuses.remaining(StatusKind::Burning) - 6.0).abs() < 1e-3);
    }

    #[test]
    fn a_refresh_does_not_postpone_the_next_damage_tick() {
        // Refreshing every 0.9 s while resetting the timer would deal no damage at all.
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();

        for _ in 0..5 {
            for _ in 0..54 {
                statuses.tick(STEP, 1.0, &mut outcomes);
            }
            statuses.apply(burn(6.0, 8.0));
        }
        assert!(
            total_damage(&outcomes) > 0.0,
            "a refresh loop dealt nothing"
        );
    }

    #[test]
    fn consuming_removes_the_status_and_hands_back_what_was_left() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();
        for _ in 0..(2 * 60) {
            statuses.tick(STEP, 1.0, &mut outcomes);
        }

        let consumed = statuses
            .consume(StatusKind::Burning)
            .expect("the burn was still active");
        assert!((consumed.remaining - 4.0).abs() < 1e-3, "{consumed:?}");
        assert!(!statuses.has(StatusKind::Burning));
        assert_eq!(statuses.consume(StatusKind::Burning), None);
    }

    #[test]
    fn clearing_reports_every_removal() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();
        statuses.clear_into(&mut outcomes);

        assert!(statuses.is_empty());
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            outcomes[0],
            StatusOutcome::Ended {
                kind: StatusKind::Burning,
                reason: StatusEnd::Cleared,
                ..
            }
        ));
    }

    #[test]
    fn a_stacking_status_climbs_to_its_ceiling_and_stops() {
        let mut statuses = StatusSet::new();
        let stack = || {
            Status::new(
                StatusKind::Recuperating,
                EntityId::new(0, 1),
                AbilityId::from_slot(1).expect("slot 1 exists"),
                10.0,
                1.0,
            )
        };
        assert!(statuses.apply(stack()));
        assert_eq!(statuses.stacks(StatusKind::Recuperating), 1);
        for _ in 0..10 {
            statuses.apply(stack());
        }
        assert_eq!(statuses.stacks(StatusKind::Recuperating), 5);
        assert_eq!(statuses.len(), 1, "a stack is one entry, not five");
    }

    #[test]
    fn an_empowered_application_may_grant_two_stacks_at_once() {
        let mut statuses = StatusSet::new();
        let two = Status::new(
            StatusKind::Sundered,
            EntityId::new(0, 1),
            AbilityId::from_slot(1).expect("slot 1 exists"),
            10.0,
            0.0,
        )
        .with_stacks(2);
        statuses.apply(two);
        assert_eq!(statuses.stacks(StatusKind::Sundered), 2);
        statuses.apply(two);
        assert_eq!(statuses.stacks(StatusKind::Sundered), 4);
        for _ in 0..10 {
            statuses.apply(two);
        }
        assert_eq!(statuses.stacks(StatusKind::Sundered), 8);
    }

    #[test]
    fn a_health_scaled_tick_carries_a_fraction_rather_than_a_number() {
        // The structural half of "the bleed does not snapshot": what comes out of a tick is
        // a coefficient, and there is nowhere in the status for a maximum health to live.
        let mut statuses = StatusSet::new();
        statuses.apply(
            Status::new(
                StatusKind::Bleeding,
                EntityId::new(0, 1),
                AbilityId::from_slot(1).expect("slot 1 exists"),
                8.0,
                0.0,
            )
            .every(2.0)
            .scaling_with_source_health(0.01),
        );
        let mut outcomes = Vec::new();
        for _ in 0..(8 * 60) {
            statuses.tick(STEP, 1.0, &mut outcomes);
        }
        let ticks: Vec<f32> = outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                StatusOutcome::Tick {
                    source_health_fraction,
                    damage,
                    ..
                } => {
                    assert_eq!(*damage, 0.0);
                    Some(*source_health_fraction)
                }
                StatusOutcome::Ended { .. } => None,
            })
            .collect();
        assert_eq!(ticks.len(), 4, "eight seconds, one tick every two");
        for fraction in ticks {
            assert!((fraction - 0.01).abs() < 1e-6);
        }
    }

    #[test]
    fn a_healing_status_says_so_on_every_tick() {
        let mut statuses = StatusSet::new();
        statuses.apply(Status::new(
            StatusKind::Recovering,
            EntityId::new(0, 1),
            AbilityId::from_slot(1).expect("slot 1 exists"),
            6.0,
            10.0,
        ));
        let mut outcomes = Vec::new();
        for _ in 0..(6 * 60) {
            statuses.tick(STEP, 1.0, &mut outcomes);
        }
        let heals = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, StatusOutcome::Tick { heals: true, .. }))
            .count();
        assert_eq!(heals, 6);
    }

    #[test]
    fn an_indefinite_status_ticks_for_ever_and_never_expires() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(18.0, 4.0).forever());
        let mut outcomes = Vec::new();
        for _ in 0..(600 * 60) {
            statuses.tick(STEP, 1.0, &mut outcomes);
        }
        assert!(statuses.has(StatusKind::Burning), "forever ran out");
        assert!((statuses.remaining(StatusKind::Burning) - 18.0).abs() < 1e-3);
        assert!(total_damage(&outcomes) > 2_000.0, "it stopped ticking");
        assert_eq!(
            statuses
                .get(StatusKind::Burning)
                .map(super::Status::fraction),
            Some(1.0)
        );
    }

    #[test]
    fn haste_shortens_the_gap_between_ticks_of_a_hasted_status_only() {
        // Twelve seconds of a two-second burn is six ticks; at a scale of a half - what a
        // hundred per cent haste produces - the same twelve seconds is twelve, because the
        // interval is one second rather than two.
        let mut hasted = StatusSet::new();
        hasted.apply(burn(12.0, 1.0).every(2.0).with_haste());
        let mut plain = StatusSet::new();
        plain.apply(burn(12.0, 1.0).every(2.0));

        let scale = 0.5;
        let mut hasted_out = Vec::new();
        let mut plain_out = Vec::new();
        for _ in 0..(12 * 60) {
            hasted.tick(STEP, scale, &mut hasted_out);
            plain.tick(STEP, scale, &mut plain_out);
        }
        assert_eq!(total_damage(&plain_out), 6.0);
        assert_eq!(total_damage(&hasted_out), 12.0);
    }

    #[test]
    fn only_a_natural_expiry_pays_out_a_bloom() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(1.0, 0.0).blooming(500.0));
        let mut outcomes = Vec::new();
        for _ in 0..60 {
            statuses.tick(STEP, 1.0, &mut outcomes);
        }
        let bloom = outcomes.iter().find_map(|outcome| match outcome {
            StatusOutcome::Ended { expiry_heal, .. } => Some(*expiry_heal),
            StatusOutcome::Tick { .. } => None,
        });
        assert_eq!(bloom, Some(500.0));

        // Cleared instead: the bloom is not paid.
        let mut statuses = StatusSet::new();
        statuses.apply(burn(10.0, 0.0).blooming(500.0));
        let mut outcomes = Vec::new();
        statuses.clear_into(&mut outcomes);
        assert!(matches!(
            outcomes[0],
            StatusOutcome::Ended {
                expiry_heal: 0.0,
                ..
            }
        ));
    }

    #[test]
    fn a_dispel_names_a_school_rather_than_a_status() {
        assert_eq!(
            StatusKind::Poisoned.dispel_school(),
            Some(DispelSchool::Poison)
        );
        assert_eq!(
            StatusKind::NaturesExposure.dispel_school(),
            Some(DispelSchool::Magic)
        );
        assert!(StatusKind::Burning.is_dispellable());
        // A wound is not a spell, and a buff is not a thing to be rid of.
        assert!(!StatusKind::Bleeding.is_dispellable());
        assert!(!StatusKind::Rejuvenation.is_dispellable());
        assert!(!StatusKind::Rejuvenation.is_harmful());
        assert!(StatusKind::Rejuvenation.is_druid_hot());
        assert!(!StatusKind::AstralWrath.is_druid_hot());
    }

    #[test]
    fn only_a_buff_in_a_school_can_be_purged_and_only_a_debuff_in_one_dispelled() {
        for kind in StatusKind::ALL {
            // The two halves of Mass Dispel never claim the same effect.
            assert!(
                !(kind.is_purgeable() && kind.is_dispellable()),
                "{kind:?} is both"
            );
        }
        assert!(StatusKind::Bloodlust.is_purgeable());
        assert!(StatusKind::Fortitude.is_purgeable());
        assert!(!StatusKind::Burning.is_purgeable());
        // Lifecycle bookkeeping, a bare buff and the trinkets' own effects are not purged.
        assert!(!StatusKind::RespawnProtection.is_purgeable());
        assert!(!StatusKind::Rejuvenation.is_purgeable());
        assert!(!StatusKind::TrinketWard.is_purgeable());
        assert!(!StatusKind::TrinketAbsorb.is_purgeable());
    }

    #[test]
    fn a_zero_length_step_changes_nothing() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();
        statuses.tick(0.0, 1.0, &mut outcomes);
        assert!(outcomes.is_empty());
        assert_eq!(statuses.remaining(StatusKind::Burning), 6.0);
    }

    #[test]
    fn the_registry_has_one_row_per_kind_in_order_with_unique_ids() {
        assert_eq!(STATUS_DEFS.len(), StatusKind::ALL.len());
        let mut ids = Vec::new();
        for (index, kind) in StatusKind::ALL.into_iter().enumerate() {
            let row = kind.definition();
            assert_eq!(kind.index(), index, "{kind:?}");
            assert_eq!(row.kind, kind, "row {index} is not {kind:?}");
            assert!(
                !row.name.is_empty() && !row.description.is_empty(),
                "{kind:?}"
            );
            assert!(
                row.id.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{}",
                row.id
            );
            assert!(!ids.contains(&row.id), "{} twice", row.id);
            ids.push(row.id);
            assert!(row.max_stacks >= 1, "{kind:?}");
            assert!(row.generic.duration > 0.0, "{kind:?}");
            // A buff is never something a dispel takes off.
            if row.class == EffectClass::Buff {
                assert!(!kind.is_dispellable(), "{kind:?}");
            }
        }
        // Every dispel school has something in it to dispel.
        for school in [
            DispelSchool::Magic,
            DispelSchool::Poison,
            DispelSchool::Curse,
            DispelSchool::Disease,
        ] {
            assert!(
                StatusKind::ALL
                    .into_iter()
                    .any(|kind| kind.is_dispellable() && kind.dispel_school() == Some(school)),
                "{school:?}"
            );
        }
    }

    #[test]
    fn the_evaluators_read_the_registry() {
        let effect = |kind: StatusKind, stacks: u8| {
            Status::new(kind, EntityId::new(0, 1), AbilityId::NONE, 10.0, 0.0).with_stacks(stacks)
        };
        let mut statuses = StatusSet::new();
        assert_eq!(statuses.move_speed_multiplier(), 1.0);
        statuses.apply(effect(StatusKind::Sundered, 8));
        assert!((statuses.attack_speed_scale() - 0.84).abs() < 1e-6);
        statuses.apply(effect(StatusKind::NaturesExposure, 10));
        assert!((statuses.damage_taken_multiplier(DamageType::Nature) - 1.20).abs() < 1e-6);
        assert_eq!(statuses.damage_taken_multiplier(DamageType::Fire), 1.0);
        statuses.apply(effect(StatusKind::Crippled, 1).with_magnitude(0.4));
        statuses.apply(effect(StatusKind::WildRush, 1));
        assert!(
            (statuses.move_speed_multiplier() - 0.9).abs() < 1e-6,
            "1.5 × 0.6"
        );
        assert!((statuses.damage_reduction() - 0.30).abs() < 1e-6);
        statuses.apply(effect(StatusKind::Fortitude, 2));
        statuses.apply(effect(StatusKind::WitheringPlague, 1));
        assert_eq!(statuses.stat_bonus().get(Stat::Stamina), 40.0 - 15.0);
        statuses.apply(effect(StatusKind::CurseOfWeakness, 1));
        assert!((statuses.damage_dealt_multiplier() - 0.75).abs() < 1e-6);
    }

    #[test]
    fn removing_a_source_takes_only_what_it_applied() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut bleed = burn(6.0, 4.0);
        bleed.kind = StatusKind::Bleeding;
        bleed.source = EntityId::new(7, 1);
        statuses.apply(bleed);
        assert_eq!(statuses.remove_from(EntityId::new(7, 1)), 1);
        assert!(statuses.has(StatusKind::Burning));
        assert!(!statuses.has(StatusKind::Bleeding));
        assert_eq!(statuses.remove_from(EntityId::new(7, 1)), 0);
    }

    #[test]
    fn a_status_with_no_damage_still_expires() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(1.0, 0.0));
        let mut outcomes = Vec::new();
        for _ in 0..60 {
            statuses.tick(STEP, 1.0, &mut outcomes);
        }
        assert!(!statuses.has(StatusKind::Burning));
        assert_eq!(total_damage(&outcomes), 0.0);
    }
}
