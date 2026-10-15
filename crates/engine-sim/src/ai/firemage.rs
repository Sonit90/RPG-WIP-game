//! The FireMage's rotation: what a competent player would press, and when they would deviate.
//!
//! Two priority systems, one per build, plus the decisions both share. It is deliberately
//! *not* a fixed script: the Detonate decision in particular is evaluated against the state
//! of the fight, because "detonate at maximum stacks" and "detonate when the pack is about
//! to die anyway" are different rotations and a simulator that only knew the first would
//! overstate the Ignite-maintenance build.
//!
//! # Haste (Detonation) priority
//!
//! 1. **Fire Mastery** on cooldown, once there is a burn (two stacks) or 30 Heat to amplify.
//! 2. **Detonate** when the burn is full and (it would kill the target anyway, or it is about
//!    to expire, or it is fresh enough to be worth the cycle).
//! 3. **Flame Cone** / **Firestorm** when two or more enemies stand in the pack.
//! 4. **Flame Lance** while the burn is below maximum stacks.
//! 5. **Meteor** and **Pyroblast** from three stacks.
//! 6. **Flame Lance** (a charge to spare), then **Firebolt** as the filler.
//!
//! # Crit (Ignite maintenance) priority
//!
//! 1. **Fire Mastery** as above. 2. **Detonate** only when holding the burn is worthless.
//! 3. At a full burn: **Pyroblast**, then **Meteor**. 4. Below a full burn: **Flame Lance**.
//! 5. Area abilities, then the payoff spells, then **Flame Lance** and **Firebolt**.
//!
//! Buffs and debuffs kept up: Ignite on the primary target (and as many of the pack as the
//! area spells reach), Fire Mastery's haste or crit window, and Heat.
//!
//! Nothing here computes damage. It presses buttons; the game decides what they do.

use engine_core::math::Vec3;
use engine_game::entity::EntityId;
use engine_game::firemage::FireAbility;
use engine_game::sim::Simulation;

use super::{Rotation, centroid, ready_to_act};

/// Which rotation is being played.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Build the burn, cash it in, rebuild. Wants Heat cycles and cooldown resets.
    #[default]
    Haste,
    /// Build the burn and *keep* it, hitting a fully ignited target with everything.
    Crit,
}

impl Style {
    /// Uppercase name, for the report.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Haste => "HASTE",
            Self::Crit => "CRIT",
        }
    }
}

/// How eagerly the AI spends a burn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetonateRules {
    /// Detonate when the burn would kill the target anyway, times this margin.
    ///
    /// One means "detonate when the remaining burn is already lethal", which is the
    /// execute heuristic: a player does not wait fifteen seconds for damage the corpse will
    /// not be there to take.
    pub execute_margin: f32,
    /// Detonate a full burn whose duration is at least this fraction of its maximum.
    ///
    /// The Detonation build's engine. Zero switches it off, which is the maintenance build.
    pub freshness: f32,
    /// Detonate rather than let a burn fall off with fewer than this many seconds left.
    pub salvage_seconds: f32,
}

impl DetonateRules {
    /// The Detonation build: spend a fresh full burn as soon as there is one.
    #[must_use]
    pub fn eager() -> Self {
        Self {
            execute_margin: 1.0,
            freshness: 0.8,
            salvage_seconds: 4.0,
        }
    }

    /// The maintenance build: hold the burn, and spend it only when holding it is worthless.
    #[must_use]
    pub fn patient() -> Self {
        Self {
            execute_margin: 1.0,
            freshness: 0.0,
            salvage_seconds: 4.0,
        }
    }
}

/// The player, as far as the simulator is concerned.
pub struct RotationAi {
    style: Style,
    detonate: DetonateRules,
}

impl RotationAi {
    /// An AI playing `style` with that style's Detonate discipline.
    #[must_use]
    pub fn new(style: Style) -> Self {
        Self {
            style,
            detonate: match style {
                Style::Haste => DetonateRules::eager(),
                Style::Crit => DetonateRules::patient(),
            },
        }
    }

    /// An AI with the Detonate rules overridden, for a balance experiment.
    #[must_use]
    #[allow(
        dead_code,
        reason = "a balance experiment's entry point; used by the tests"
    )]
    pub fn with_rules(style: Style, detonate: DetonateRules) -> Self {
        Self { style, detonate }
    }

    /// Takes at most one action this tick.
    fn act_once(&mut self, simulation: &mut Simulation, pack: &[EntityId]) {
        // Moving Fire Beam: while it burns the player does nothing else (casting would break
        // it) but keep it on the pack, which is what steering with the cursor amounts to.
        if simulation.is_channelling_beam() {
            let living: Vec<EntityId> = pack
                .iter()
                .copied()
                .filter(|id| simulation.world().is_alive(*id))
                .collect();
            if !living.is_empty() {
                let centre = centroid(simulation, &living);
                simulation.steer_beam(centre);
            }
            return;
        }
        if !ready_to_act(simulation) {
            return;
        }
        let living: Vec<EntityId> = pack
            .iter()
            .copied()
            .filter(|id| simulation.world().is_alive(*id))
            .collect();
        // The Ultimate on cooldown: a full bar and a ready button, aimed at the pack's
        // centre - which for a single enemy is the enemy.
        if !living.is_empty()
            && simulation.mage().energy.is_full()
            && simulation.mage().abilities.is_ready(FireAbility::FireBeam)
        {
            let centre = centroid(simulation, &living);
            if simulation.cast_at(FireAbility::FireBeam, centre).is_ok() {
                return;
            }
        }
        let Some(primary) = self.choose_target(simulation, &living) else {
            return;
        };
        simulation.player_mut().target = Some(primary);
        // Facing is a real requirement for a targeted cast, and the AI is not testing the
        // player's mouse hand.
        if let Some(position) = simulation.world().position(primary) {
            simulation.face(position);
        }

        let centre = centroid(simulation, &living);
        match self.style {
            Style::Haste => self.haste_priority(simulation, primary, &living, centre),
            Style::Crit => self.crit_priority(simulation, primary, &living, centre),
        }
    }

    /// Who to hit: the most-ignited living enemy, then the weakest.
    ///
    /// Concentrating on one burn is what makes a burn worth having; hitting whichever
    /// target happens to be nearest would spread stacks across a pack and never fill one.
    fn choose_target(&self, simulation: &Simulation, living: &[EntityId]) -> Option<EntityId> {
        let mage = simulation.mage();
        let maximum = mage.ignite_max();
        let mut best: Option<(u8, f32, EntityId)> = None;
        for id in living {
            let stacks = mage.ignite.stacks(*id);
            let health = simulation
                .world()
                .health(*id)
                .map_or(f32::MAX, engine_game::combat::Health::current);
            // Prefer a target that is already burning hardest; break ties on the one
            // closest to dying, because that is the one a Detonate would be spent on.
            let key = (stacks.min(maximum), health, *id);
            if best.is_none_or(|(stacks, current, _)| {
                key.0 > stacks || (key.0 == stacks && key.1 < current)
            }) {
                best = Some(key);
            }
        }
        best.map(|(_, _, id)| id)
    }

    /// Build, spend, rebuild.
    fn haste_priority(
        &mut self,
        simulation: &mut Simulation,
        primary: EntityId,
        living: &[EntityId],
        centre: Vec3,
    ) {
        if self.use_mastery(simulation) {
            return;
        }
        if self.should_detonate(simulation, primary)
            && simulation.cast(FireAbility::Detonate).is_ok()
        {
            return;
        }
        if self.use_area(simulation, living, centre) {
            return;
        }
        let stacks = simulation.mage().ignite.stacks(primary);
        let maximum = simulation.mage().ignite_max();

        if stacks < maximum && simulation.cast(FireAbility::FlameLance).is_ok() {
            return;
        }
        if stacks >= 3 && self.use_meteor(simulation, centre) {
            return;
        }
        if stacks >= 3 && simulation.cast(FireAbility::Pyroblast).is_ok() {
            return;
        }
        if simulation.cast(FireAbility::FlameLance).is_ok() {
            return;
        }
        let _ = simulation.cast(FireAbility::Firebolt);
    }

    /// Fill the burn, then hit it with everything that is paid off by it.
    fn crit_priority(
        &mut self,
        simulation: &mut Simulation,
        primary: EntityId,
        living: &[EntityId],
        centre: Vec3,
    ) {
        if self.use_mastery(simulation) {
            return;
        }
        if self.should_detonate(simulation, primary)
            && simulation.cast(FireAbility::Detonate).is_ok()
        {
            return;
        }

        let mage = simulation.mage();
        let stacks = mage.ignite.stacks(primary);
        let maximum = mage.ignite_max();
        let full = stacks >= maximum;

        // At the cap, the payoff spells come first: that is the whole build.
        if full && simulation.cast(FireAbility::Pyroblast).is_ok() {
            return;
        }
        if full && self.use_meteor(simulation, centre) {
            return;
        }
        if !full && simulation.cast(FireAbility::FlameLance).is_ok() {
            return;
        }
        if self.use_area(simulation, living, centre) {
            return;
        }
        if simulation.cast(FireAbility::Pyroblast).is_ok() {
            return;
        }
        if self.use_meteor(simulation, centre) {
            return;
        }
        // Flame Lance is the filler even at the cap: it is worth two and a half Firebolts
        // per cast and it refreshes the thirty seconds the whole build depends on. Holding
        // its charges for a burn that is already full would be holding them forever.
        if simulation.cast(FireAbility::FlameLance).is_ok() {
            return;
        }
        let _ = simulation.cast(FireAbility::Firebolt);
    }

    /// Fire Mastery, at the start of a burst window.
    fn use_mastery(&self, simulation: &mut Simulation) -> bool {
        if !simulation
            .mage()
            .abilities
            .is_ready(FireAbility::FireMastery)
        {
            return false;
        }
        // Held until there is a burn worth amplifying, then used on cooldown.
        let heat = simulation.mage().heat.value();
        let burning = simulation.mage().ignite.stacks(
            simulation
                .player()
                .target
                .unwrap_or(engine_game::entity::EntityId::INVALID),
        );
        if burning < 2 && heat < 30.0 {
            return false;
        }
        simulation.cast(FireAbility::FireMastery).is_ok()
    }

    /// The area abilities, in the order that keeps a pack burning.
    fn use_area(&self, simulation: &mut Simulation, living: &[EntityId], centre: Vec3) -> bool {
        let many = living.len() >= 2;
        if simulation.mage().abilities.is_ready(FireAbility::FlameCone)
            && (many || self.wants_stacks(simulation, living))
            && simulation.cast_at(FireAbility::FlameCone, centre).is_ok()
        {
            return true;
        }
        if simulation.mage().abilities.is_ready(FireAbility::Firestorm)
            && simulation.cast_at(FireAbility::Firestorm, centre).is_ok()
        {
            return true;
        }
        false
    }

    /// Meteor, placed on the pack.
    fn use_meteor(&self, simulation: &mut Simulation, centre: Vec3) -> bool {
        simulation.mage().abilities.is_ready(FireAbility::Meteor)
            && simulation.cast_at(FireAbility::Meteor, centre).is_ok()
    }

    /// Whether anything in the pack still has room for stacks.
    fn wants_stacks(&self, simulation: &Simulation, living: &[EntityId]) -> bool {
        let mage = simulation.mage();
        let maximum = mage.ignite_max();
        living.iter().any(|id| mage.ignite.stacks(*id) < maximum)
    }

    /// **The decision the two builds differ on.**
    ///
    /// A full burn is spent when the explosion would land on a target that is about to die
    /// anyway, when the burn is about to fall off and cannot be held, or - for the
    /// Detonation build only - whenever it is fresh enough to be worth the whole cycle.
    fn should_detonate(&self, simulation: &Simulation, primary: EntityId) -> bool {
        let mage = simulation.mage();
        let maximum = mage.ignite_max();
        if !mage.ignite.is_at_maximum(primary, maximum) {
            return false;
        }
        if !mage.abilities.is_ready(FireAbility::Detonate) {
            return false;
        }

        let burn = mage.ignite.get(primary);
        let remaining = mage.remaining_ignite_damage(primary);
        let health = simulation
            .world()
            .health(primary)
            .map_or(f32::MAX, engine_game::combat::Health::current);

        // Execute: the burn is already lethal, so the sixty per cent it converts is damage
        // that would otherwise be dealt to a corpse.
        if remaining >= health * self.detonate.execute_margin {
            return true;
        }
        // Salvage: a burn that is about to expire is worth more spent than lost.
        if burn.remaining <= self.detonate.salvage_seconds {
            return true;
        }
        // The cycle: only the Detonation build takes this branch.
        self.detonate.freshness > 0.0
            && burn.remaining >= mage.ignite_duration() * self.detonate.freshness
    }
}

impl Rotation for RotationAi {
    fn act(&mut self, simulation: &mut Simulation, pack: &[EntityId]) {
        self.act_once(simulation, pack);
    }

    fn tracked_cooldowns(&self) -> &'static [FireAbility] {
        &[
            FireAbility::FlameLance,
            FireAbility::FlameCone,
            FireAbility::Meteor,
            FireAbility::Pyroblast,
            FireAbility::Firestorm,
            FireAbility::FireMastery,
        ]
    }

    fn name(&self) -> &'static str {
        match self.style {
            Style::Haste => "FireMage / Haste (Detonation)",
            Style::Crit => "FireMage / Crit (Ignite maintenance)",
        }
    }

    fn priority(&self) -> &'static [&'static str] {
        match self.style {
            Style::Haste => &[
                "Moving Fire Beam on a full Ultimate bar, aimed at the pack and kept on it for the whole channel.",
                "Fire Mastery on cooldown, once there is a burn (2 stacks) or 30 Heat to amplify.",
                "Detonate a full burn when it would kill the target anyway, is about to expire, or is fresh (80% of its duration left).",
                "Flame Cone / Firestorm when two or more enemies stand in the pack.",
                "Flame Lance while the burn is below maximum stacks.",
                "Meteor, then Pyroblast, from 3 Ignite stacks.",
                "Flame Lance with a spare charge, then Firebolt as the filler.",
            ],
            Style::Crit => &[
                "Moving Fire Beam on a full Ultimate bar, aimed at the pack and kept on it for the whole channel.",
                "Fire Mastery on cooldown, once there is a burn (2 stacks) or 30 Heat to amplify.",
                "Detonate only when holding the burn is worthless (lethal anyway, or about to expire).",
                "At a full burn: Pyroblast, then Meteor.",
                "Below a full burn: Flame Lance to rebuild it.",
                "Flame Cone / Firestorm for a pack, then the payoff spells, then Flame Lance and Firebolt.",
            ],
        }
    }
}
