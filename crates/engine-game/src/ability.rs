//! The ability framework: definitions, slots, cooldowns and costs.
//!
//! An ability is **data**, not code. The player controller never names Fireball; it emits
//! [`crate::command::PlayerCommand::UseAbility`] with a slot, and the ability system reads
//! the [`AbilityDef`] in that slot and executes what it describes:
//!
//! ```text
//! UseAbility(slot) -> validate (alive, ready, affordable, in range, valid target)
//!                  -> resolve targets
//!                  -> apply effect
//!                  -> emit CombatEvents
//! ```
//!
//! That indirection is what makes ten slots cost the same as three and what lets Milestone
//! 13's synergy exist: Detonate does not know that Ignite applied the burn, only that a
//! burn is there.
//!
//! Execution lives in [`crate::sim`], because applying an effect needs the world. This
//! module owns everything that can be decided without one — which is every rule about
//! whether an ability may be used at all.

use crate::combat::DamageType;
use crate::status::StatusKind;

/// Ability slots a character has.
///
/// Ten, fixed, from the start. Three are mechanically interesting in Phase 3 and seven are
/// placeholders, but they are all real slots with real cooldowns and costs — the point is
/// that filling them in later is a data change, not an architectural one.
pub const ABILITY_SLOTS: usize = 10;

/// Which ability slot. Also the identity of an ability in a damage event.
///
/// A slot index rather than a name, so telemetry can bucket by ability with an array
/// instead of a map, and a future ability database can reassign what lives in a slot
/// without changing the event shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AbilityId(u8);

impl AbilityId {
    /// The id used by damage that came from no ability — a status tick attributes itself to
    /// the ability that applied it, but falling damage or a future environmental effect has
    /// no ability at all.
    pub const NONE: Self = Self(u8::MAX);

    /// The ability in slot `index`, or `None` if there is no such slot.
    #[must_use]
    pub const fn from_slot(index: usize) -> Option<Self> {
        // The cast cannot truncate: the branch above bounds the index by ABILITY_SLOTS.
        if index < ABILITY_SLOTS {
            Some(Self(index as u8))
        } else {
            None
        }
    }

    /// The slot this id names.
    #[must_use]
    pub const fn slot(self) -> usize {
        self.0 as usize
    }

    /// Whether this names a real slot.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        (self.0 as usize) < ABILITY_SLOTS
    }

    /// The 1-based number shown on the ability bar. Slot 9 is printed as `0`, matching the
    /// key that triggers it.
    ///
    /// Widened before the increment because [`AbilityId::NONE`] is `u8::MAX`, and adding
    /// one to it in a debug build would panic rather than produce a digit.
    #[must_use]
    pub const fn hotkey_digit(self) -> u8 {
        ((self.0 as u16 + 1) % 10) as u8
    }
}

/// How an ability chooses what it affects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Targeting {
    /// One hostile entity, which must be the caster's current target.
    SingleTarget,
    /// Everything hostile within `radius` of the current target.
    ///
    /// Target-centred rather than reticle-centred because there is no reticle: the arena is
    /// a training dummy field and the interesting question is whether an area effect hits
    /// the group and misses what is outside it.
    AreaAroundTarget {
        /// Metres from the target an entity may be and still be hit.
        radius: f32,
    },
    /// Everything hostile within `radius` of the caster.
    SelfCentered {
        /// Metres from the caster an entity may be and still be hit.
        radius: f32,
    },
}

impl Targeting {
    /// Whether this mode needs a target entity to be selected.
    #[must_use]
    pub fn needs_target(self) -> bool {
        matches!(self, Self::SingleTarget | Self::AreaAroundTarget { .. })
    }

    /// Whether this mode can affect more than one entity.
    #[must_use]
    pub fn is_area(self) -> bool {
        matches!(
            self,
            Self::AreaAroundTarget { .. } | Self::SelfCentered { .. }
        )
    }
}

/// What an ability does to each entity it affects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbilityEffect {
    /// Immediate damage.
    DirectDamage {
        /// Points of damage.
        amount: f32,
    },
    /// Damage now and a lasting status afterwards.
    ApplyStatus {
        /// Damage on impact. May be zero for a pure status.
        impact: f32,
        /// Which status to apply.
        status: StatusKind,
        /// How long it lasts, seconds.
        duration: f32,
        /// Damage each status tick deals.
        damage_per_tick: f32,
    },
    /// Consumes a status for a burst of damage scaled by what was left of it.
    ///
    /// This is the synergy: on its own it is a weak area hit, and against a burning target
    /// it is the payoff for having spent a global cooldown on the status first.
    Detonate {
        /// Damage dealt whether or not the status was there.
        base: f32,
        /// Which status is consumed.
        consumes: StatusKind,
        /// Extra damage per second of status remaining.
        bonus_per_second_remaining: f32,
    },
}

/// One ability, entirely as data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityDef {
    /// Which slot it lives in.
    pub id: AbilityId,
    /// Uppercase name, because the overlay font has no lowercase glyphs.
    pub name: &'static str,
    /// Seconds before it can be used again.
    pub cooldown: f32,
    /// Resource points it costs.
    pub cost: f32,
    /// Metres the target may be away. Ignored by self-centred abilities.
    pub range: f32,
    /// How it chooses what to affect.
    pub targeting: Targeting,
    /// What it does.
    pub effect: AbilityEffect,
    /// What kind of damage it deals.
    pub damage_type: DamageType,
    /// Whether this slot is a Phase 5 placeholder rather than designed content.
    ///
    /// Placeholders are fully functional — they validate, cost, cooldown and damage like
    /// anything else. The flag exists so the overlay can say so and so a test can assert
    /// that exactly three slots are real.
    pub placeholder: bool,
}

impl AbilityDef {
    /// Whether the ability can affect more than one entity.
    #[must_use]
    pub fn is_area(&self) -> bool {
        self.targeting.is_area()
    }

    /// The radius an area ability affects, or zero for a single-target one.
    #[must_use]
    pub fn radius(&self) -> f32 {
        match self.targeting {
            Targeting::SingleTarget => 0.0,
            Targeting::AreaAroundTarget { radius } | Targeting::SelfCentered { radius } => radius,
        }
    }
}

/// Why an ability could not be used.
///
/// Every refusal is named. A silently dropped ability press is the single most confusing
/// thing a combat prototype can do, and the overlay prints these back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityError {
    /// The slot does not exist.
    UnknownSlot,
    /// The caster is dead.
    CasterDead,
    /// The ability is still cooling down.
    OnCooldown,
    /// The caster cannot pay the cost.
    NotEnoughResource,
    /// The ability needs a target and none is selected.
    NoTarget,
    /// The selected target is dead or no longer exists.
    TargetInvalid,
    /// The target is further away than the ability reaches.
    OutOfRange,
}

impl AbilityError {
    /// Short uppercase description, for the developer overlay's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnknownSlot => "NO SUCH ABILITY",
            Self::CasterDead => "YOU ARE DEAD",
            Self::OnCooldown => "ON COOLDOWN",
            Self::NotEnoughResource => "NOT ENOUGH ENERGY",
            Self::NoTarget => "NO TARGET",
            Self::TargetInvalid => "TARGET IS GONE",
            Self::OutOfRange => "OUT OF RANGE",
        }
    }
}

/// Radius of the two area abilities, in metres.
///
/// Chosen against the arena layout: the dummy group is spread over roughly six metres, so
/// eight metres hits the group and leaves the single dummy — twelve metres away — out of
/// it. That is what makes "targets outside the radius take no damage" observable by playing
/// rather than only by a test.
const AREA_RADIUS: f32 = 8.0;

/// The default ten-slot loadout.
///
/// Slots 1 to 3 are the designed abilities and demonstrate three different mechanics:
/// direct damage, a status over time, and a synergy that consumes that status. Slots 4 to
/// 10 are placeholders with distinct numbers so the bar and the meter have something real
/// to show.
#[must_use]
pub fn default_loadout() -> [AbilityDef; ABILITY_SLOTS] {
    /// Builds a slot, panicking only if `ABILITY_SLOTS` and this table disagree.
    const fn id(slot: usize) -> AbilityId {
        match AbilityId::from_slot(slot) {
            Some(id) => id,
            // Unreachable for a slot below ABILITY_SLOTS, and this table is exactly that
            // long; a mismatch is a compile-time-visible bug rather than a runtime one.
            None => AbilityId::NONE,
        }
    }

    [
        AbilityDef {
            id: id(0),
            name: "FIREBALL",
            cooldown: 1.5,
            cost: 12.0,
            range: 30.0,
            targeting: Targeting::SingleTarget,
            effect: AbilityEffect::DirectDamage { amount: 45.0 },
            damage_type: DamageType::Fire,
            placeholder: false,
        },
        AbilityDef {
            id: id(1),
            name: "IGNITE",
            cooldown: 6.0,
            cost: 18.0,
            range: 30.0,
            targeting: Targeting::SingleTarget,
            effect: AbilityEffect::ApplyStatus {
                impact: 10.0,
                status: StatusKind::Burning,
                duration: 6.0,
                damage_per_tick: 8.0,
            },
            damage_type: DamageType::Fire,
            placeholder: false,
        },
        AbilityDef {
            id: id(2),
            name: "DETONATE",
            cooldown: 8.0,
            cost: 25.0,
            range: 30.0,
            targeting: Targeting::AreaAroundTarget {
                radius: AREA_RADIUS,
            },
            effect: AbilityEffect::Detonate {
                base: 20.0,
                consumes: StatusKind::Burning,
                // Six seconds of burn remaining is 90 bonus damage, which is twice a
                // Fireball: spending the setup has to be worth more than casting the
                // filler, or the synergy is decoration.
                bonus_per_second_remaining: 15.0,
            },
            damage_type: DamageType::Fire,
            placeholder: false,
        },
        placeholder(id(3), "STRIKE", 2.0, 8.0, 6.0, 30.0, DamageType::Physical),
        placeholder(id(4), "CLEAVE", 5.0, 15.0, 6.0, 22.0, DamageType::Physical),
        placeholder(id(5), "SHOCK", 4.0, 14.0, 30.0, 35.0, DamageType::Arcane),
        placeholder(id(6), "LANCE", 3.0, 11.0, 30.0, 28.0, DamageType::Arcane),
        placeholder(id(7), "QUAKE", 12.0, 30.0, 6.0, 55.0, DamageType::Physical),
        placeholder(id(8), "NOVA", 15.0, 35.0, 6.0, 60.0, DamageType::Arcane),
        placeholder(id(9), "SMITE", 20.0, 40.0, 30.0, 120.0, DamageType::Arcane),
    ]
}

/// One placeholder slot: a working ability with no interesting mechanic.
///
/// A range of six metres means self-centred, which is how the two melee-shaped placeholders
/// are expressed without a separate concept.
const fn placeholder(
    id: AbilityId,
    name: &'static str,
    cooldown: f32,
    cost: f32,
    range: f32,
    damage: f32,
    damage_type: DamageType,
) -> AbilityDef {
    AbilityDef {
        id,
        name,
        cooldown,
        cost,
        range,
        targeting: if range <= 6.0 {
            Targeting::SelfCentered { radius: range }
        } else {
            Targeting::SingleTarget
        },
        effect: AbilityEffect::DirectDamage { amount: damage },
        damage_type,
        placeholder: true,
    }
}

/// A character's abilities and their cooldowns.
///
/// Fixed-size arrays: ten slots is the design, the cooldown array is forty bytes, and a
/// `HashMap<AbilityId, f32>` would allocate to store what fits in a cache line.
#[derive(Debug, Clone)]
pub struct AbilityBook {
    definitions: [AbilityDef; ABILITY_SLOTS],
    /// Seconds remaining on each slot. Zero means ready.
    cooldowns: [f32; ABILITY_SLOTS],
}

impl Default for AbilityBook {
    fn default() -> Self {
        Self::new(default_loadout())
    }
}

impl AbilityBook {
    /// A book with the given definitions, all off cooldown.
    #[must_use]
    pub fn new(definitions: [AbilityDef; ABILITY_SLOTS]) -> Self {
        Self {
            definitions,
            cooldowns: [0.0; ABILITY_SLOTS],
        }
    }

    /// Every definition, in slot order.
    #[must_use]
    pub fn definitions(&self) -> &[AbilityDef; ABILITY_SLOTS] {
        &self.definitions
    }

    /// The definition in a slot.
    #[must_use]
    pub fn get(&self, id: AbilityId) -> Option<&AbilityDef> {
        self.definitions.get(id.slot())
    }

    /// Seconds until the ability is usable again. Zero means ready.
    #[must_use]
    pub fn cooldown_remaining(&self, id: AbilityId) -> f32 {
        self.cooldowns.get(id.slot()).copied().unwrap_or(0.0)
    }

    /// How far through its cooldown an ability is, in [0, 1]. One means ready.
    #[must_use]
    pub fn cooldown_fraction(&self, id: AbilityId) -> f32 {
        let Some(definition) = self.get(id) else {
            return 1.0;
        };
        if definition.cooldown <= 0.0 {
            return 1.0;
        }
        1.0 - (self.cooldown_remaining(id) / definition.cooldown).clamp(0.0, 1.0)
    }

    /// Whether the cooldown has expired. Says nothing about cost, range or targets.
    #[must_use]
    pub fn is_ready(&self, id: AbilityId) -> bool {
        id.is_valid() && self.cooldown_remaining(id) <= 0.0
    }

    /// Puts an ability on its full cooldown.
    pub fn start_cooldown(&mut self, id: AbilityId) {
        let Some(definition) = self.definitions.get(id.slot()) else {
            return;
        };
        let cooldown = definition.cooldown;
        if let Some(slot) = self.cooldowns.get_mut(id.slot()) {
            *slot = cooldown.max(0.0);
        }
    }

    /// Advances every cooldown by one simulation step.
    ///
    /// Driven by the fixed timestep, never by the render delta: a cooldown that ticked per
    /// frame would be six times faster at 360 FPS than at 60.
    pub fn tick(&mut self, seconds: f32) {
        for remaining in &mut self.cooldowns {
            if *remaining > 0.0 {
                *remaining = (*remaining - seconds).max(0.0);
                // Snapped rather than left as a residue: see `TIME_EPSILON`. Without this a
                // 1.5 s cooldown ticked ninety times is still 0.0000003 s from ready.
                if *remaining < crate::TIME_EPSILON {
                    *remaining = 0.0;
                }
            }
        }
    }

    /// Clears every cooldown.
    pub fn reset_cooldowns(&mut self) {
        self.cooldowns = [0.0; ABILITY_SLOTS];
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn every_slot_exists_and_names_itself() {
        let book = AbilityBook::default();
        assert_eq!(book.definitions().len(), ABILITY_SLOTS);
        for slot in 0..ABILITY_SLOTS {
            let id = AbilityId::from_slot(slot).expect("a slot below the count exists");
            let definition = book.get(id).expect("every slot is filled");
            assert_eq!(definition.id, id, "slot {slot} names another slot");
            assert!(!definition.name.is_empty());
            assert!(definition.cooldown > 0.0, "{}", definition.name);
        }
        assert_eq!(AbilityId::from_slot(ABILITY_SLOTS), None);
        assert!(!AbilityId::NONE.is_valid());
    }

    #[test]
    fn the_hotkey_digits_run_one_to_zero() {
        let digits: Vec<u8> = (0..ABILITY_SLOTS)
            .filter_map(AbilityId::from_slot)
            .map(AbilityId::hotkey_digit)
            .collect();
        assert_eq!(digits, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 0]);
    }

    #[test]
    fn exactly_three_slots_are_designed_and_the_rest_are_placeholders() {
        let loadout = default_loadout();
        let designed: Vec<&str> = loadout
            .iter()
            .filter(|definition| !definition.placeholder)
            .map(|definition| definition.name)
            .collect();
        assert_eq!(designed, vec!["FIREBALL", "IGNITE", "DETONATE"]);
        assert_eq!(loadout.iter().filter(|d| d.placeholder).count(), 7);
    }

    #[test]
    fn the_three_designed_abilities_demonstrate_three_mechanics() {
        let loadout = default_loadout();
        assert!(matches!(
            loadout[0].effect,
            AbilityEffect::DirectDamage { .. }
        ));
        assert!(matches!(
            loadout[1].effect,
            AbilityEffect::ApplyStatus {
                status: StatusKind::Burning,
                ..
            }
        ));
        assert!(matches!(
            loadout[2].effect,
            AbilityEffect::Detonate {
                consumes: StatusKind::Burning,
                ..
            }
        ));
        assert!(!loadout[0].is_area(), "fireball is single target");
        assert!(loadout[2].is_area(), "detonate is an area effect");
        assert_eq!(loadout[2].radius(), AREA_RADIUS);
    }

    #[test]
    fn an_ability_is_ready_until_it_is_used() {
        let mut book = AbilityBook::default();
        let fireball = AbilityId::from_slot(0).expect("slot 0 exists");

        assert!(book.is_ready(fireball));
        assert_eq!(book.cooldown_remaining(fireball), 0.0);
        assert_eq!(book.cooldown_fraction(fireball), 1.0);

        book.start_cooldown(fireball);
        assert!(!book.is_ready(fireball));
        assert!((book.cooldown_remaining(fireball) - 1.5).abs() < 1e-6);
        assert_eq!(book.cooldown_fraction(fireball), 0.0);
    }

    #[test]
    fn a_cooldown_runs_down_at_the_simulation_rate_and_stops_at_zero() {
        let mut book = AbilityBook::default();
        let fireball = AbilityId::from_slot(0).expect("slot 0 exists");
        book.start_cooldown(fireball);

        // Half the cooldown: 45 ticks of 1/60 s.
        for _ in 0..45 {
            book.tick(1.0 / 60.0);
        }
        assert!((book.cooldown_remaining(fireball) - 0.75).abs() < 1e-3);
        assert!(!book.is_ready(fireball));
        assert!((book.cooldown_fraction(fireball) - 0.5).abs() < 1e-2);

        for _ in 0..60 {
            book.tick(1.0 / 60.0);
        }
        assert_eq!(
            book.cooldown_remaining(fireball),
            0.0,
            "a cooldown does not go negative"
        );
        assert!(book.is_ready(fireball));
    }

    #[test]
    fn one_cooldown_does_not_affect_another() {
        let mut book = AbilityBook::default();
        let fireball = AbilityId::from_slot(0).expect("slot 0 exists");
        let ignite = AbilityId::from_slot(1).expect("slot 1 exists");

        book.start_cooldown(fireball);
        assert!(book.is_ready(ignite));
        assert_eq!(book.cooldown_remaining(ignite), 0.0);
    }

    #[test]
    fn resetting_clears_every_cooldown() {
        let mut book = AbilityBook::default();
        for slot in 0..ABILITY_SLOTS {
            if let Some(id) = AbilityId::from_slot(slot) {
                book.start_cooldown(id);
            }
        }
        book.reset_cooldowns();
        for slot in 0..ABILITY_SLOTS {
            let id = AbilityId::from_slot(slot).expect("a slot below the count exists");
            assert!(book.is_ready(id), "slot {slot} stayed on cooldown");
        }
    }

    #[test]
    fn every_refusal_has_a_message() {
        for error in [
            AbilityError::UnknownSlot,
            AbilityError::CasterDead,
            AbilityError::OnCooldown,
            AbilityError::NotEnoughResource,
            AbilityError::NoTarget,
            AbilityError::TargetInvalid,
            AbilityError::OutOfRange,
        ] {
            assert!(!error.as_str().is_empty(), "{error:?}");
        }
    }
}
