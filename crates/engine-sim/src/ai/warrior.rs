//! The Warrior's rotation: every damage ability on cooldown, a free ability in between.
//!
//! The Warrior is a tank, so most of its kit is not damage and a damage rotation uses little
//! of it. What it does have is four damaging buttons on cooldowns and one without, and the
//! whole rotation is "never leave a ready one unpressed".
//!
//! # Priority
//!
//! 1. **Shield Charge** (15 s) - the biggest hit, and it closes the distance. On cooldown.
//! 2. **Colossus Strike** (8 s) - an area smash plus a bleed that scales with maximum
//!    Health. On cooldown; re-pressing it before the bleed ends refreshes the bleed.
//! 3. **War Stomp** (4 s) - the cleave, and the debuff (Sundered) it stacks. On cooldown.
//! 4. **Shield Bash** (15 s) - a short-cooldown hit. On cooldown.
//! 5. **Recuperate** - no cooldown, so it is the filler between the above.
//!
//! The order is by size of hit, which on a shared global cooldown is only a question of which
//! ready ability goes first - all of them are pressed within a few seconds of becoming ready.
//!
//! # What it keeps up
//!
//! * **Sundered** on the target (War Stomp, up to eight stacks) and **Bleeding** (Colossus
//!   Strike), both by pressing their abilities the moment they are ready.
//! * **Recuperating** on the Warrior, stacked by the filler.
//! * Any buff the class's resource grants is a *consequence of pressing the damage abilities*
//!   and needs no decision here, which is why this AI reads no resource: it presses buttons
//!   and checks cooldown readiness, so a rework of the resource cannot break it.
//!
//! # Talents
//!
//! The rotation asks the game what the Warrior's talents mean ([`engine_game::sim::Simulation::warrior_effects`])
//! and never which talents were bought:
//!
//! * **Executioner** - the Warrior fights the weakest enemy in reach, and Colossus Strike goes
//!   first while that enemy is under the threshold: the bonus is the whole point of the talent.
//! * **Unyielding** - Last Stand is pressed first and again whenever it is ready, because its
//!   Battle Fervor stacks are worth a global cooldown.
//! * **Bulwark Rhythm** - Shield Block is pressed whenever it is ready, after the damage
//!   abilities, so the cooldown it cuts is a cooldown that is being used.
//!
//! # What it does not do
//!
//! The other defensive cooldowns (Puncture Blood, Adrenaline, Battle Trance, Spell
//! Reflect) and Taunt do nothing to a target dummy's health bar and the fight has no
//! incoming damage to answer, so a damage rotation leaves them alone. Shield Bash is
//! pressed as damage only; there is nothing to interrupt.

use engine_game::entity::EntityId;
use engine_game::firemage::FireAbility;
use engine_game::sim::Simulation;

use super::{Rotation, close_in, distance_to, is_ready, living, ready_to_act, select_and_face};

/// How near the Warrior stands to a target before it presses a melee ability, in metres.
///
/// Inside the five-metre reach of the shortest-ranged ability with a margin for a target that
/// takes a step before the press lands.
const MELEE_REACH: f32 = 4.0;

/// The damage abilities, in priority order. Recuperate is last and has no cooldown.
const PRIORITY: [FireAbility; 5] = [
    FireAbility::ShieldCharge,
    FireAbility::ColossusStrike,
    FireAbility::WarStomp,
    FireAbility::ShieldBash,
    FireAbility::Recuperate,
];

/// The fraction of its health `target` has left, in `0.0..=1.0`.
fn health_fraction(simulation: &Simulation, target: EntityId) -> f32 {
    simulation
        .world()
        .health(target)
        .map_or(1.0, |health| health.current() / health.max().max(1.0))
}

/// The Warrior, as the simulator plays it.
#[derive(Debug, Default)]
pub struct WarriorRotation;

impl WarriorRotation {
    /// A Warrior who presses every ready damage ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Rotation for WarriorRotation {
    fn act(&mut self, simulation: &mut Simulation, pack: &[EntityId]) {
        if !ready_to_act(simulation) {
            return;
        }
        let alive = living(simulation, pack);
        let effects = *simulation.warrior_effects();
        // The nearest enemy: a melee character fights what is in front of it. With
        // Executioner it fights the weakest one within reach instead.
        let nearest = |a: &EntityId, b: &EntityId| {
            distance_to(simulation, *a)
                .partial_cmp(&distance_to(simulation, *b))
                .unwrap_or(std::cmp::Ordering::Equal)
        };
        let primary = if effects.executioner_bonus > 0.0 {
            alive
                .iter()
                .copied()
                .filter(|id| distance_to(simulation, *id) <= MELEE_REACH)
                .min_by(|a, b| {
                    health_fraction(simulation, *a)
                        .partial_cmp(&health_fraction(simulation, *b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .or_else(|| alive.iter().copied().min_by(nearest))
        } else {
            alive.iter().copied().min_by(nearest)
        };
        let Some(primary) = primary else {
            return;
        };
        select_and_face(simulation, primary);

        // Shield Charge crosses the gap by itself; everything else has to be in reach.
        let charge_ready = is_ready(simulation, FireAbility::ShieldCharge);
        if distance_to(simulation, primary) > MELEE_REACH && !charge_ready {
            close_in(simulation, primary);
            return;
        }
        // Unyielding: the rally comes before anything else, and again whenever it is ready.
        if effects.last_stand_fervor_stacks > 0
            && is_ready(simulation, FireAbility::LastStand)
            && simulation.cast(FireAbility::LastStand).is_ok()
        {
            return;
        }
        // Executioner: the smash is held for the moment it hits hardest.
        if effects.executioner_bonus > 0.0
            && health_fraction(simulation, primary) < effects.executioner_threshold
            && is_ready(simulation, FireAbility::ColossusStrike)
            && simulation.cast(FireAbility::ColossusStrike).is_ok()
        {
            return;
        }
        for ability in PRIORITY {
            // Relentless Charge: with a second charge Shield Charge is no longer limited by
            // its own cooldown, so it yields to Colossus Strike, which is: a charge pressed
            // ahead of a ready smash would only push the smash's cooldown back.
            if ability == FireAbility::ShieldCharge
                && effects.shield_charge_extra_charges > 0
                && is_ready(simulation, FireAbility::ColossusStrike)
                && simulation.cast(FireAbility::ColossusStrike).is_ok()
            {
                return;
            }
            if ability == FireAbility::Recuperate
                && effects.bulwark_cooldown_cut > 0.0
                && is_ready(simulation, FireAbility::ShieldBlock)
                && simulation.cast(FireAbility::ShieldBlock).is_ok()
            {
                return;
            }
            if is_ready(simulation, ability) && simulation.cast(ability).is_ok() {
                return;
            }
        }
        if distance_to(simulation, primary) > MELEE_REACH {
            close_in(simulation, primary);
        }
    }

    fn tracked_cooldowns(&self) -> &'static [FireAbility] {
        &[
            FireAbility::ShieldCharge,
            FireAbility::ColossusStrike,
            FireAbility::WarStomp,
            FireAbility::ShieldBash,
        ]
    }

    fn name(&self) -> &'static str {
        "Warrior / damage on cooldown"
    }

    fn priority(&self) -> &'static [&'static str] {
        &[
            "Last Stand when ready, only with Unyielding: its Battle Fervor stacks are worth the global cooldown.",
            "Colossus Strike first while the target is under Executioner's threshold, only with Executioner (the Warrior also picks the weakest enemy in reach).",
            "Shield Charge on cooldown (15 s): the biggest hit, and it closes the distance. With Relentless Charge's second charge it yields to a ready Colossus Strike.",
            "Colossus Strike on cooldown (8 s): area smash and a bleed scaled from maximum Health.",
            "War Stomp on cooldown (4 s): the cleave, stacking Sundered on everything it hits.",
            "Shield Bash on cooldown (15 s): pressed as damage; there is nothing to interrupt.",
            "Shield Block when ready, only with Bulwark Rhythm: the cooldown it cuts is then a cooldown in use.",
            "Recuperate as the filler: no cooldown, and it stacks the Warrior's self heal.",
            "Other defensive cooldowns (Puncture Blood, Adrenaline, Battle Trance, Spell Reflect) and Taunt are not part of a damage rotation.",
        ]
    }
}
