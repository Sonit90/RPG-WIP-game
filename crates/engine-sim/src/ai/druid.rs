//! The Druid's damage rotation: two damage-over-time spells kept up, a free filler between.
//!
//! The Druid is a healer, and this is the rotation of a Druid with nobody to heal: it plays
//! the three damage spells and spends Astral Power on them.
//!
//! # Priority
//!
//! 1. **Astral Power** - the moment the bar is full (100) and not already armed, press the
//!    button. Nothing is spent until the next compatible spell is cast, so this costs the
//!    global cooldown and nothing else.
//! 2. **Moonfire** - when it is missing, or has three seconds or less left. When Astral
//!    Power is armed it is cast *regardless*, as long as the burn is not already permanent:
//!    an empowered Moonfire deals half as much again and never expires, so it is cast once
//!    and the slot is free for good.
//! 3. **Sunfire** - when anything inside its radius of the best target is not burning, or has
//!    three seconds or less left (nine when armed, so the empowerment is not wasted on a
//!    Wrath while a Sunfire is about to lapse). The target is the enemy nearest the middle of
//!    the pack, so the eight-metre radius covers as many as possible.
//! 4. **Wrath** - the filler. It costs no Mana, stacks Nature's Exposure (more Nature and
//!    Arcane damage taken, to fifteen stacks), and an empowered one spreads the debuff around
//!    its target.
//!
//! # What it keeps up
//!
//! * **Moonfire** and **Sunfire** on the target(s), refreshed before they lapse.
//! * **Nature's Exposure** on the target, by casting Wrath whenever nothing better is ready.
//! * Astral Power spent as fast as it fills, so none of it overflows unused.
//!
//! The healing spells, Wild Rush and Tranquility are not part of a damage rotation: there is
//! no one to heal and nothing to run from.

use engine_game::entity::EntityId;
use engine_game::firemage::FireAbility;
use engine_game::sim::Simulation;
use engine_game::status::StatusKind;

use super::{Rotation, centroid, living, ready_to_act, select_and_face};

/// Seconds left on a damage-over-time at which it is refreshed.
const REFRESH_AT: f32 = 3.0;

/// The same, for Sunfire while an empowerment is waiting to be spent.
const REFRESH_AT_ARMED: f32 = 9.0;

/// The Druid, as the simulator plays it.
#[derive(Debug, Default)]
pub struct DruidRotation;

impl DruidRotation {
    /// A Druid who keeps its burns up and spends its Astral Power.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Seconds left on `kind` on `target`, `None` if absent, infinity if it never ends.
    fn remaining(simulation: &Simulation, target: EntityId, kind: StatusKind) -> Option<f32> {
        simulation
            .world()
            .statuses(target)
            .and_then(|statuses| statuses.get(kind))
            .map(|status| {
                if status.indefinite {
                    f32::INFINITY
                } else {
                    status.remaining
                }
            })
    }

    /// Whether `kind` on `target` is missing or has `threshold` seconds or less left.
    fn needs(simulation: &Simulation, target: EntityId, kind: StatusKind, threshold: f32) -> bool {
        Self::remaining(simulation, target, kind).is_none_or(|left| left <= threshold)
    }
}

impl Rotation for DruidRotation {
    fn act(&mut self, simulation: &mut Simulation, pack: &[EntityId]) {
        if !ready_to_act(simulation) {
            return;
        }
        let alive = living(simulation, pack);
        let middle = centroid(simulation, &alive);
        // The enemy nearest the middle of the pack, so an area spell centred on it covers
        // the most.
        let Some(best) = alive.iter().copied().min_by(|a, b| {
            let spread = |id: &EntityId| {
                simulation.world().position(*id).map_or(f32::MAX, |p| {
                    engine_game::sim::horizontal_distance(p, middle)
                })
            };
            spread(a)
                .partial_cmp(&spread(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            return;
        };
        select_and_face(simulation, best);

        // 1. The button.
        if simulation.druid().astral.can_arm() && simulation.cast(FireAbility::AstralPower).is_ok()
        {
            return;
        }
        let armed = simulation.astral_armed();

        // 2. Moonfire. An empowered one is permanent, so an armed Druid casts it whenever the
        //    one on the target is not.
        let moonfire = Self::remaining(simulation, best, StatusKind::Moonfire);
        let moonfire_permanent = moonfire == Some(f32::INFINITY);
        let moonfire_due = if armed {
            !moonfire_permanent
        } else {
            moonfire.is_none_or(|left| left <= REFRESH_AT)
        };
        if moonfire_due && simulation.cast(FireAbility::Moonfire).is_ok() {
            return;
        }

        // 3. Sunfire, for whatever it will actually reach.
        let radius = FireAbility::Sunfire.definition().radius;
        let threshold = if armed { REFRESH_AT_ARMED } else { REFRESH_AT };
        let centre = simulation.world().position(best);
        let reached_and_due = alive.iter().any(|id| {
            let near = centre
                .zip(simulation.world().position(*id))
                .is_some_and(|(c, p)| engine_game::sim::horizontal_distance(c, p) <= radius);
            near && Self::needs(simulation, *id, StatusKind::Sunfire, threshold)
        });
        if reached_and_due && simulation.cast(FireAbility::Sunfire).is_ok() {
            return;
        }

        // 4. The filler.
        let _ = simulation.cast(FireAbility::Wrath);
    }

    fn tracked_cooldowns(&self) -> &'static [FireAbility] {
        // The Druid's damage spells have no cooldown; Astral Power is a resource button, not
        // a timed one, so it appears under resources instead.
        &[]
    }

    fn name(&self) -> &'static str {
        "Druid / damage over time and Wrath"
    }

    fn priority(&self) -> &'static [&'static str] {
        &[
            "Astral Power: press the button as soon as the bar holds 100 and nothing is armed (arming spends nothing; the next compatible spell does).",
            "Moonfire: when missing or at 3 s or less. When armed, cast it unless the burn is already permanent (empowered Moonfire: +50% damage, never expires).",
            "Sunfire: when anything within its 8 m radius of the most central enemy is missing it or at 3 s or less (9 s when armed).",
            "Wrath: the free filler; stacks Nature's Exposure on the target (empowered, on everything around it).",
            "Healing spells, Wild Rush and Tranquility are not part of a damage rotation.",
        ]
    }
}
