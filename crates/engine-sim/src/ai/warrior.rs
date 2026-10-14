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
//! # What it does not do
//!
//! The defensive cooldowns (Shield Block, Puncture Blood, Adrenaline, Battle Trance, Spell
//! Reflect, Last Stand) and Taunt do nothing to a target dummy's health bar and the fight
//! has no incoming damage to answer, so a damage rotation leaves them alone. Shield Bash is
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
        // The nearest enemy: a melee character fights what is in front of it.
        let Some(primary) = alive.iter().copied().min_by(|a, b| {
            distance_to(simulation, *a)
                .partial_cmp(&distance_to(simulation, *b))
                .unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            return;
        };
        select_and_face(simulation, primary);

        // Shield Charge crosses the gap by itself; everything else has to be in reach.
        let charge_ready = is_ready(simulation, FireAbility::ShieldCharge);
        if distance_to(simulation, primary) > MELEE_REACH && !charge_ready {
            close_in(simulation, primary);
            return;
        }
        for ability in PRIORITY {
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
            "Shield Charge on cooldown (15 s): the biggest hit, and it closes the distance.",
            "Colossus Strike on cooldown (8 s): area smash and a bleed scaled from maximum Health.",
            "War Stomp on cooldown (4 s): the cleave, stacking Sundered on everything it hits.",
            "Shield Bash on cooldown (15 s): pressed as damage; there is nothing to interrupt.",
            "Recuperate as the filler: no cooldown, and it stacks the Warrior's self heal.",
            "Defensive cooldowns (Shield Block, Puncture Blood, Adrenaline, Battle Trance, Spell Reflect, Last Stand) and Taunt are not part of a damage rotation.",
        ]
    }
}
