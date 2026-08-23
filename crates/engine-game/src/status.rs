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
use crate::combat::DamageType;
use crate::entity::EntityId;

/// Seconds between the damage ticks of a lasting status.
///
/// One second, so a burn's remaining duration and its remaining damage are the same number
/// scaled: what is left is readable off the overlay without arithmetic.
pub const STATUS_TICK_INTERVAL: f32 = 1.0;

/// Statuses the simulation knows.
///
/// One kind so far, because one is what proves the mechanism. The set is an enum rather
/// than a string or a registry id so a `match` over every status is exhaustive and adding
/// one is a compile error everywhere it must be handled.
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
    /// Wrath's debuff: more Nature and Arcane damage taken, per stack, to fifteen.
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
    /// Curses. Nothing applies one yet.
    Curse,
    /// Diseases. Nothing applies one yet.
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

impl StatusKind {
    /// Short uppercase name, for the developer overlay's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Burning => "BURNING",
            Self::Bleeding => "BLEEDING",
            Self::Recuperating => "RECUPERATING",
            Self::Recovering => "RECOVERING",
            Self::Sundered => "SUNDERED",
            Self::Moonfire => "MOONFIRE",
            Self::Sunfire => "SUNFIRE",
            Self::AstralSunfire => "ASTRAL SUNFIRE",
            Self::NaturesExposure => "NATURES EXPOSURE",
            Self::Rejuvenation => "REJUVENATION",
            Self::Regrowth => "REGROWTH",
            Self::Lifebloom => "LIFEBLOOM",
            Self::WildGrowth => "WILD GROWTH",
            Self::AstralWrath => "ASTRAL WRATH",
            Self::WildRush => "WILD RUSH",
            Self::Poisoned => "POISONED",
        }
    }

    /// Which school a dispel has to name to remove this, or `None` if nothing can.
    ///
    /// A bleed is not dispellable, and neither is a buff the bearer wants: what Nature's
    /// Cleanse removes is a *harmful* effect that belongs to a school. Both halves are asked
    /// here - see [`StatusKind::is_harmful`].
    #[must_use]
    pub fn dispel_school(self) -> Option<DispelSchool> {
        match self {
            Self::Burning
            | Self::Moonfire
            | Self::Sunfire
            | Self::AstralSunfire
            | Self::NaturesExposure
            | Self::Sundered => Some(DispelSchool::Magic),
            Self::Poisoned => Some(DispelSchool::Poison),
            // A bleed is a wound. Nothing in the game removes one with a spell, which is the
            // point of the exception: a dispel that removed everything would be a dispel
            // nothing had to be chosen for.
            Self::Bleeding
            | Self::Recuperating
            | Self::Recovering
            | Self::Rejuvenation
            | Self::Regrowth
            | Self::Lifebloom
            | Self::WildGrowth
            | Self::AstralWrath
            | Self::WildRush => None,
        }
    }

    /// Whether the bearer would rather not have this.
    #[must_use]
    pub fn is_harmful(self) -> bool {
        !matches!(
            self,
            Self::Recuperating
                | Self::Recovering
                | Self::Rejuvenation
                | Self::Regrowth
                | Self::Lifebloom
                | Self::WildGrowth
                | Self::AstralWrath
                | Self::WildRush
        )
    }

    /// Whether a dispel that names its school may remove it.
    #[must_use]
    pub fn is_dispellable(self) -> bool {
        self.is_harmful() && self.dispel_school().is_some()
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
        match self {
            Self::Burning => DamageType::Fire,
            Self::Bleeding | Self::Recuperating | Self::Recovering | Self::Sundered => {
                DamageType::Physical
            }
            Self::Moonfire
            | Self::Sunfire
            | Self::AstralSunfire
            | Self::NaturesExposure
            | Self::Rejuvenation
            | Self::Regrowth
            | Self::Lifebloom
            | Self::WildGrowth
            | Self::AstralWrath
            | Self::WildRush
            | Self::Poisoned => DamageType::Nature,
        }
    }

    /// How many times this status may stack on one bearer.
    ///
    /// One for everything that refreshes rather than stacks, which is most of them. A
    /// property of the kind rather than of each application, so two callers cannot disagree
    /// about how tall a stack of War Stomp is.
    #[must_use]
    pub fn max_stacks(self) -> u8 {
        match self {
            Self::Burning | Self::Bleeding | Self::Recovering => 1,
            Self::Recuperating => 5,
            Self::Sundered => 8,
            // Fifteen, which is the forty-five per cent ceiling the design states divided by
            // the three per cent one stack is worth. One number, in one place.
            Self::NaturesExposure => 15,
            Self::Moonfire
            | Self::Sunfire
            | Self::AstralSunfire
            | Self::Rejuvenation
            | Self::Regrowth
            | Self::Lifebloom
            | Self::WildGrowth
            | Self::AstralWrath
            | Self::WildRush
            | Self::Poisoned => 1,
        }
    }

    /// Whether a tick of this status restores health rather than removing it.
    #[must_use]
    pub fn heals(self) -> bool {
        matches!(
            self,
            Self::Recuperating
                | Self::Recovering
                | Self::Rejuvenation
                | Self::Regrowth
                | Self::Lifebloom
                | Self::WildGrowth
        )
    }
}

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
            timer: STATUS_TICK_INTERVAL,
        }
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
    fn a_zero_length_step_changes_nothing() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();
        statuses.tick(0.0, 1.0, &mut outcomes);
        assert!(outcomes.is_empty());
        assert_eq!(statuses.remaining(StatusKind::Burning), 6.0);
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
