//! What the interface is allowed to know about the fight.
//!
//! ```text
//! Simulation --> UiSnapshot --> game_ui / ability bar / tooltips / talent menu
//! ```
//!
//! The UI reads this and never the simulation. That is the rule the whole interface is built
//! on: a widget cannot decide that an ability is ready, cannot recompute a cooldown, and
//! cannot invent a number the game does not have. Everything it draws was measured here,
//! once, from the one system that owns the answer.
//!
//! Rebuilt in place every frame. Nothing here allocates after the first one: the vectors are
//! cleared and refilled, so a snapshot is a copy of a few hundred bytes rather than a fresh
//! object graph.

use engine_game::EntityId;
use engine_game::ability::AbilityId;
use engine_game::firemage::{
    ABILITY_SLOTS, AbilityError, ENERGY_MAXIMUM, FireAbility, FireMastery, HEAT_HOT_THRESHOLD,
    HEAT_MAXIMUM, HEAT_WARM_THRESHOLD, HeatTier, TALENTS, TalentId, ULTIMATE_ENABLED,
};
use engine_game::sim::Simulation;
use engine_game::status::StatusKind;
use engine_game::world::EntityKind;

/// One status on an entity, as the UI sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusView {
    /// Which status.
    pub kind: StatusKind,
    /// Seconds left.
    pub remaining: f32,
    /// How much of its duration is left, in `0.0..=1.0`.
    pub fraction: f32,
}

/// A fixed-capacity list of statuses.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct StatusList {
    entries: [Option<StatusView>; Self::CAPACITY],
}

impl StatusList {
    /// How many statuses one entity can show.
    pub const CAPACITY: usize = 4;

    /// The statuses, in the order they were added.
    pub fn iter(&self) -> impl Iterator<Item = StatusView> + '_ {
        self.entries.iter().filter_map(|entry| *entry)
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds one, if there is room.
    fn push(&mut self, view: StatusView) {
        if let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) {
            *slot = Some(view);
        }
    }
}

/// The player's own state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerView {
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Current Heat.
    pub heat: f32,
    /// Which band Heat is in, which is what the bar colours itself from.
    pub heat_tier: HeatTier,
    /// Seconds of Inferno State left. Zero when it is not running.
    pub inferno_remaining: f32,
    /// Current Ultimate Energy.
    pub energy: f32,
    /// Whether the Ultimate can be cast.
    pub ultimate_ready: bool,
    /// Whether Flame Shield is up.
    pub shielded: bool,
    /// Whether the class is hasted above its base.
    pub hasted: bool,
    /// Which stat Fire Mastery specialises into.
    pub mastery: FireMastery,
    /// Whether Fire Mastery's active is running.
    pub mastery_active: bool,
    /// Statuses on the player.
    pub statuses: StatusList,
}

impl PlayerView {
    /// Health as a fraction, for a bar.
    #[must_use]
    pub fn health_fraction(&self) -> f32 {
        if self.max_health <= 0.0 {
            return 0.0;
        }
        (self.health / self.max_health).clamp(0.0, 1.0)
    }

    /// Heat as a fraction, for a bar.
    #[must_use]
    pub fn heat_fraction(&self) -> f32 {
        (self.heat / HEAT_MAXIMUM).clamp(0.0, 1.0)
    }

    /// Whether Heat has reached the second band, where it hands over a stat.
    #[must_use]
    pub fn heat_is_high(&self) -> bool {
        self.heat >= HEAT_HOT_THRESHOLD
    }

    /// Whether the Inferno State is running.
    #[must_use]
    pub fn in_inferno(&self) -> bool {
        matches!(self.heat_tier, HeatTier::Inferno)
    }

    /// Where the two band markers sit on the bar, as fractions of its width.
    ///
    /// Computed here so the bar cannot draw a threshold the simulation does not have.
    #[must_use]
    pub fn heat_thresholds(&self) -> [f32; 2] {
        [
            HEAT_WARM_THRESHOLD / HEAT_MAXIMUM,
            HEAT_HOT_THRESHOLD / HEAT_MAXIMUM,
        ]
    }

    /// Ultimate Energy as a fraction, for a bar.
    #[must_use]
    pub fn energy_fraction(&self) -> f32 {
        (self.energy / ENERGY_MAXIMUM).clamp(0.0, 1.0)
    }
}

/// The selected enemy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetView {
    /// Which entity.
    pub entity: EntityId,
    /// What it is.
    pub kind: EntityKind,
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Metres away.
    pub distance: f32,
    /// Whether it is alive.
    pub alive: bool,
    /// Ignite stacks on it.
    pub ignite_stacks: u8,
    /// Seconds of Ignite left.
    pub ignite_remaining: f32,
    /// Whether it is at the current maximum - the Detonate requirement.
    pub ignite_at_maximum: bool,
    /// Statuses on it.
    pub statuses: StatusList,
}

impl TargetView {
    /// Health as a fraction, for a bar.
    #[must_use]
    pub fn health_fraction(&self) -> f32 {
        if self.max_health <= 0.0 {
            return 0.0;
        }
        (self.health / self.max_health).clamp(0.0, 1.0)
    }

    /// Health as a whole percentage, for a readout.
    #[must_use]
    pub fn health_percent(&self) -> u32 {
        (self.health_fraction() * 100.0).round() as u32
    }
}

/// Why an ability button looks the way it does.
///
/// The UI picks a colour from this and never works it out itself, which is what stops a
/// button from claiming an ability is ready when the simulation would refuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotAvailability {
    /// Ready to cast.
    Ready,
    /// Cooling down, or out of charges.
    Cooling,
    /// Not enough Ultimate Energy.
    NoResource,
    /// The target cannot be hit, or is not there.
    InvalidTarget,
    /// Ready *and* worth pressing right now: Detonate on a full burn.
    Empowered,
    /// Switched off entirely: the Ultimate, which has no design yet.
    Disabled,
}

impl SlotAvailability {
    /// Whether the ability can be cast right now.
    #[must_use]
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready | Self::Empowered)
    }
}

/// One ability button's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilitySlotView {
    /// Which ability.
    pub ability: FireAbility,
    /// Its identity in the event stream.
    pub id: AbilityId,
    /// Its name.
    pub name: &'static str,
    /// The key that casts it.
    pub key: &'static str,
    /// Why it looks the way it does.
    pub availability: SlotAvailability,
    /// Charges ready now.
    pub charges: u8,
    /// Charges it holds when full.
    pub max_charges: u8,
    /// Seconds until the next charge.
    pub cooldown_remaining: f32,
    /// How far through the recharge it is, in `0.0..=1.0`. One is ready.
    pub cooldown_fraction: f32,
    /// How far through the global cooldown we are, in `0.0..=1.0`. One means it is not
    /// running.
    ///
    /// The same value on every slot, because the global cooldown *is* the same on every
    /// slot: starting any ability locks the whole bar. It is carried per-slot rather than
    /// once on the snapshot so that a button has everything it needs to draw itself, which
    /// is the rule the rest of this view already follows.
    pub global_cooldown_fraction: f32,
    /// Whether this is the ability currently being cast.
    pub casting: bool,
    /// Whether this is the ability whose area the player is placing.
    ///
    /// Unlike everything else on this view, aiming is not a fact about the simulation — a
    /// placement that has not been confirmed has changed nothing and costs nothing. It is
    /// carried here anyway, because a button that has to be told separately what it is
    /// showing is a button that can be told wrong.
    pub aiming: bool,
    /// Whether the player has to place it with the mouse.
    pub ground_targeted: bool,
    /// What the cast takes right now, haste included. Zero for an instant.
    pub cast_time: f32,
    /// What it hits for right now, before Heat, critical strikes and Ignite scaling.
    ///
    /// Computed from the ability's coefficient and the mage's current spell power, so a
    /// tooltip prints a number the ability will actually deal rather than a percentage the
    /// player has to do arithmetic on.
    pub damage: f32,
    /// Extra damage per Ignite stack on the target, as a fraction. Zero if it has none.
    pub ignite_scaling: f32,
    /// Ignite stacks it applies to everything it hits.
    pub ignite_stacks: u8,
    /// Chance those stacks land, for the abilities that roll for it.
    pub ignite_chance: f32,
}

impl AbilitySlotView {
    /// Whether the button should show a charge counter.
    #[must_use]
    pub fn shows_charges(&self) -> bool {
        self.max_charges > 1
    }

    /// Whether a radial cooldown should be drawn over it.
    #[must_use]
    pub fn is_cooling(&self) -> bool {
        self.cooldown_remaining > 0.0 && self.charges < self.max_charges
    }

    /// Whether the global cooldown is running.
    #[must_use]
    pub fn is_on_global_cooldown(&self) -> bool {
        self.global_cooldown_fraction < 1.0
    }

    /// Whether the button should be framed as the one the player is in the middle of.
    ///
    /// Casting it, or choosing where to put it. Both are "this is the ability happening
    /// right now", and the player has no reason to tell the two states apart on the bar:
    /// aiming a Meteor and casting a Pyroblast are the same answer to "what am I doing".
    #[must_use]
    pub fn shows_active_frame(&self) -> bool {
        self.casting || self.aiming
    }

    /// Whether the button should draw the global cooldown's sweep.
    ///
    /// Only when its own cooldown is not already being drawn. Both are a translucent wash
    /// over the same button, so drawing them together would darken the overlap twice and
    /// make a slot that is merely waiting look different from its neighbours for no reason.
    /// The longer sweep is the one that matters, and it is the one that survives.
    #[must_use]
    pub fn shows_global_cooldown(&self) -> bool {
        self.is_on_global_cooldown() && !self.is_cooling()
    }
}

/// The cast in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastView {
    /// Which ability.
    pub ability: FireAbility,
    /// Its name.
    pub name: &'static str,
    /// Seconds spent.
    pub elapsed: f32,
    /// Seconds it takes.
    pub duration: f32,
}

impl CastView {
    /// How far through it is, in `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        (self.elapsed / self.duration).clamp(0.0, 1.0)
    }

    /// Seconds left.
    #[must_use]
    pub fn remaining(&self) -> f32 {
        (self.duration - self.elapsed).max(0.0)
    }
}

/// The fight so far.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CombatView {
    /// Damage dealt.
    pub damage: f32,
    /// Damage per second.
    pub dps: f32,
    /// Seconds in combat.
    pub seconds: f32,
}

/// Something the interface should say.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct NotificationView {
    /// The last refusal, if there was one.
    pub error: Option<AbilityError>,
}

/// The talent grid, as the menu sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TalentView {
    /// Which talent.
    pub id: TalentId,
    /// Its name.
    pub name: &'static str,
    /// Which tier it sits in.
    pub tier: u8,
    /// What it costs.
    pub cost: u32,
    /// What it does.
    pub description: &'static str,
    /// Whether it is owned.
    pub purchased: bool,
    /// Whether it could be bought right now.
    pub affordable: bool,
}

/// Everything the interface may read, rebuilt every frame.
#[derive(Debug, Clone)]
pub struct UiSnapshot {
    /// The player.
    pub player: PlayerView,
    /// The selected enemy, if any.
    pub target: Option<TargetView>,
    /// The ability bar, in bar order.
    pub abilities: [AbilitySlotView; ABILITY_SLOTS],
    /// The Ultimate, which is not on the bar.
    pub ultimate: AbilitySlotView,
    /// The cast in progress.
    pub cast: Option<CastView>,
    /// The fight so far.
    pub combat: CombatView,
    /// Anything the interface should say.
    pub notification: NotificationView,
    /// Talent points left to spend.
    pub talent_points: u32,
    /// The whole talent grid.
    pub talents: Vec<TalentView>,
    /// The mage's current spell power, which every tooltip number is a percentage of.
    pub spell_power: f32,
    /// Current haste, as a fraction.
    pub haste: f32,
    /// Current critical strike chance, as a fraction.
    pub crit_chance: f32,
    /// The current maximum Ignite stacks. Read by every tooltip that mentions it.
    pub ignite_max: u8,
}

impl UiSnapshot {
    /// Reads the simulation.
    #[must_use]
    pub fn build(simulation: &Simulation) -> Self {
        Self::build_aiming(simulation, None)
    }

    /// Reads the simulation, with `aiming` naming the ability whose area is being placed.
    #[must_use]
    pub fn build_aiming(simulation: &Simulation, aiming: Option<FireAbility>) -> Self {
        let mut snapshot = Self {
            player: PlayerView {
                health: 0.0,
                max_health: 0.0,
                heat: 0.0,
                heat_tier: HeatTier::Cold,
                inferno_remaining: 0.0,
                energy: 0.0,
                ultimate_ready: false,
                shielded: false,
                hasted: false,
                mastery: FireMastery::Haste,
                mastery_active: false,
                statuses: StatusList::default(),
            },
            target: None,
            abilities: [ability_view(simulation, FireAbility::Firebolt, aiming); ABILITY_SLOTS],
            ultimate: ability_view(simulation, FireAbility::Cataclysm, aiming),
            cast: None,
            combat: CombatView::default(),
            notification: NotificationView::default(),
            talent_points: 0,
            talents: Vec::with_capacity(TALENTS.len()),
            spell_power: 0.0,
            haste: 0.0,
            crit_chance: 0.0,
            ignite_max: 5,
        };
        snapshot.refresh(simulation, aiming);
        snapshot
    }

    /// Rebuilds in place, keeping every allocation.
    ///
    /// `aiming` is the ability whose area the player is currently placing, which is the one
    /// piece of this the simulation cannot answer: an unconfirmed placement has changed
    /// nothing. It is a parameter rather than a field the caller sets afterwards, so it
    /// cannot be left behind from the frame before.
    pub fn refresh(&mut self, simulation: &Simulation, aiming: Option<FireAbility>) {
        let mage = simulation.mage();
        let player = simulation.player();
        let world = simulation.world();

        let health = world.health(player.entity);
        self.player = PlayerView {
            health: health.map_or(0.0, |health| health.current()),
            max_health: health.map_or(0.0, |health| health.max()),
            heat: mage.heat.value(),
            heat_tier: mage.heat.tier(),
            inferno_remaining: mage.heat.inferno_remaining(),
            energy: mage.energy.value(),
            ultimate_ready: mage.energy.is_full() && ULTIMATE_ENABLED,
            shielded: mage.flame_shield.is_active(),
            hasted: mage.haste() > engine_game::firemage::BASE_HASTE,
            mastery: mage.mastery,
            mastery_active: mage.mastery_buff.is_active(),
            statuses: statuses_of(simulation, player.entity),
        };

        self.target = player.target.and_then(|entity| {
            let health = world.health(entity)?;
            let burn = mage.ignite.get(entity);
            Some(TargetView {
                entity,
                kind: world.kind(entity)?,
                health: health.current(),
                max_health: health.max(),
                // No distance means the target is gone; a large number reads as "out of
                // range" everywhere it is used rather than as "right next to you".
                distance: world
                    .distance_between(player.entity, entity)
                    .unwrap_or(f32::MAX),
                alive: world.is_alive(entity),
                ignite_stacks: burn.stacks,
                ignite_remaining: burn.remaining,
                ignite_at_maximum: burn.is_at_maximum(mage.ignite_max()),
                statuses: statuses_of(simulation, entity),
            })
        });

        for (slot, ability) in FireAbility::BAR.into_iter().enumerate() {
            self.abilities[slot] = ability_view(simulation, ability, aiming);
        }
        self.ultimate = ability_view(simulation, FireAbility::Cataclysm, aiming);

        self.cast = simulation.casting().map(|cast| CastView {
            ability: cast.ability,
            name: cast.ability.definition().name,
            elapsed: cast.elapsed,
            duration: cast.duration,
        });

        let meter = simulation.meter();
        self.combat = CombatView {
            damage: meter.total_damage() as f32,
            dps: meter.dps() as f32,
            seconds: meter.combat_seconds() as f32,
        };
        // The simulation decides how long a refusal is worth showing, not the interface.
        self.notification = NotificationView {
            error: simulation.recent_error(),
        };

        self.talent_points = mage.talents.available_points();
        self.talents.clear();
        for talent in &TALENTS {
            self.talents.push(TalentView {
                id: talent.id,
                name: talent.name,
                tier: talent.tier,
                cost: talent.cost,
                description: talent.description,
                purchased: mage.talents.has(talent.id),
                affordable: mage.talents.can_purchase(talent.id),
            });
        }
        let stats = mage.stats();
        self.spell_power = stats.spell_power;
        self.haste = stats.haste;
        self.crit_chance = stats.crit();
        self.ignite_max = mage.ignite_max();
    }
}

/// One ability's button state, worked out once here rather than in the widget.
fn ability_view(
    simulation: &Simulation,
    ability: FireAbility,
    aiming: Option<FireAbility>,
) -> AbilitySlotView {
    let mage = simulation.mage();
    let definition = ability.definition();
    let effects = mage.effects();
    let stats = mage.stats();
    let charges = mage.abilities.charges(ability);
    let max_charges = definition.charges_with(effects);

    let target = simulation.player().target;
    let target_alive = target.is_some_and(|id| simulation.world().is_alive(id));
    let in_range = target.and_then(|id| {
        simulation
            .world()
            .distance_between(simulation.player().entity, id)
    });

    let availability = if ability.is_ultimate() && !ULTIMATE_ENABLED {
        // The Ultimate has no gameplay concept yet. The button stays where it is and says
        // so, rather than disappearing and taking its architecture with it.
        SlotAvailability::Disabled
    } else if definition.energy_cost > 0.0 && !mage.energy.is_full() {
        SlotAvailability::NoResource
    } else if charges == 0 {
        SlotAvailability::Cooling
    } else if definition.target.needs_target()
        && (!target_alive || in_range.is_some_and(|distance| distance > definition.range))
    {
        SlotAvailability::InvalidTarget
    } else if ability == FireAbility::Detonate {
        // The one ability whose readiness is about the *target*: a full burn is what it is
        // for, and the bar says so rather than making the player count stacks.
        match target {
            Some(id) if mage.ignite.is_at_maximum(id, mage.ignite_max()) => {
                SlotAvailability::Empowered
            }
            _ => SlotAvailability::InvalidTarget,
        }
    } else {
        SlotAvailability::Ready
    };

    AbilitySlotView {
        ability,
        id: AbilityId::from_fire(ability),
        name: definition.name,
        key: ability.key_hint(),
        availability,
        charges,
        max_charges,
        cooldown_remaining: mage.abilities.recharge_remaining(ability),
        cooldown_fraction: mage.abilities.recharge_fraction(ability, effects),
        global_cooldown_fraction: mage.global_cooldown_fraction(),
        casting: simulation
            .casting()
            .is_some_and(|cast| cast.ability == ability),
        aiming: aiming == Some(ability),
        ground_targeted: definition.target.is_ground_targeted(),
        cast_time: definition.cast_time_with(&stats),
        damage: if definition.coefficient > 0.0 {
            definition.base_damage(&stats)
        } else {
            definition.base_tick_damage(&stats)
        },
        ignite_scaling: definition.ignite_bonus_per_stack,
        ignite_stacks: definition.ignite_stacks,
        ignite_chance: definition.ignite_chance,
    }
}

/// The statuses on one entity, capped at what a plate can show.
fn statuses_of(simulation: &Simulation, entity: EntityId) -> StatusList {
    let mut list = StatusList::default();
    if let Some(statuses) = simulation.world().statuses(entity) {
        for status in statuses.iter() {
            list.push(StatusView {
                kind: status.kind,
                remaining: status.remaining,
                fraction: status.fraction(),
            });
        }
    }
    list
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};

    fn arena() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation.select_nearest_target();
        simulation
    }

    #[test]
    fn the_bar_is_the_ten_designed_abilities_and_the_ultimate_is_separate() {
        let snapshot = UiSnapshot::build(&arena());
        let names: Vec<&str> = snapshot.abilities.iter().map(|slot| slot.name).collect();
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
        assert_eq!(snapshot.ultimate.name, "CATACLYSM");
        assert!(
            !names.contains(&"CATACLYSM"),
            "the ultimate is on the regular bar"
        );
    }

    #[test]
    fn charges_are_reported_only_where_there_are_several() {
        let snapshot = UiSnapshot::build(&arena());
        let lance = snapshot.abilities[1];
        assert_eq!((lance.charges, lance.max_charges), (2, 2));
        assert!(lance.shows_charges());

        let firebolt = snapshot.abilities[0];
        assert!(!firebolt.shows_charges(), "firebolt has one charge");
    }

    #[test]
    fn a_spent_ability_reports_its_cooldown_for_the_overlay() {
        let mut simulation = arena();
        simulation
            .cast(FireAbility::Blink)
            .expect("blink is instant");
        simulation.step_many(60);

        let snapshot = UiSnapshot::build(&simulation);
        let blink = snapshot.abilities[9];
        assert_eq!(blink.charges, 0);
        assert!(blink.is_cooling());
        assert!(blink.cooldown_remaining > 0.0);
        assert!((0.0..1.0).contains(&blink.cooldown_fraction));
        assert_eq!(blink.availability, SlotAvailability::Cooling);
    }

    #[test]
    fn detonate_lights_up_only_against_a_full_burn() {
        let mut simulation = arena();
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            snapshot.abilities[6].availability,
            SlotAvailability::InvalidTarget,
            "it offered itself against a cold target"
        );

        let target = simulation.player().target.expect("a dummy");
        let maximum = simulation.mage().ignite_max();
        let duration = simulation.mage().ignite_duration();
        simulation
            .mage_mut()
            .ignite
            .apply(target, maximum, maximum, duration);

        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            snapshot.abilities[6].availability,
            SlotAvailability::Empowered
        );
        assert!(snapshot.abilities[6].availability.is_ready());
        assert!(snapshot.target.expect("a target").ignite_at_maximum);
    }

    #[test]
    fn the_ultimate_reports_its_energy_while_it_is_switched_off() {
        let mut simulation = arena();
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.ultimate.availability, SlotAvailability::Disabled);
        assert!(!snapshot.player.ultimate_ready);

        simulation.mage_mut().gain_energy(100.0);
        let snapshot = UiSnapshot::build(&simulation);
        assert!((snapshot.player.energy_fraction() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_heat_bar_reports_its_bands_and_the_inferno_state() {
        let mut simulation = arena();
        simulation.mage_mut().gain_heat(40.0);
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.player.heat_tier, HeatTier::Warm);
        assert!(!snapshot.player.heat_is_high());
        assert!((snapshot.player.heat_fraction() - 0.4).abs() < 1e-6);

        simulation.mage_mut().gain_heat(40.0);
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.player.heat_tier, HeatTier::Hot);
        assert!(snapshot.player.heat_is_high());

        // The top of the bar spends it: the bar then reports the state, not the value.
        simulation.mage_mut().gain_heat(20.0);
        let snapshot = UiSnapshot::build(&simulation);
        assert!(snapshot.player.in_inferno());
        assert_eq!(snapshot.player.heat, 0.0);
        assert!(snapshot.player.inferno_remaining > 0.0);

        // Both band markers are on the bar, in order, and neither is at an end of it.
        let [warm, hot] = snapshot.player.heat_thresholds();
        assert!(0.0 < warm && warm < hot && hot < 1.0);
    }

    #[test]
    fn the_ultimate_button_is_present_and_switched_off() {
        // The design keeps the architecture and takes away the button's use, so the slot
        // must still exist and must never read as ready.
        let mut simulation = arena();
        simulation.mage_mut().gain_energy(100.0);
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.ultimate.ability, FireAbility::Cataclysm);
        assert_eq!(snapshot.ultimate.availability, SlotAvailability::Disabled);
        assert!(!snapshot.ultimate.availability.is_ready());
        assert!(!snapshot.player.ultimate_ready);
    }

    #[test]
    fn the_target_view_carries_the_ignite_stacks_and_the_current_maximum() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy");
        let duration = simulation.mage().ignite_duration();
        simulation.mage_mut().ignite.apply(target, 3, 5, duration);

        let snapshot = UiSnapshot::build(&simulation);
        let view = snapshot.target.expect("a target");
        assert_eq!(view.ignite_stacks, 3);
        assert!(!view.ignite_at_maximum);
        assert_eq!(snapshot.ignite_max, 5);

        // And the maximum follows the talent, which is what the tooltips print.
        simulation
            .mage_mut()
            .purchase_talent(engine_game::firemage::TalentId::InfernalCapacity)
            .expect("testing mode pays");
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.ignite_max, 6);
    }

    #[test]
    fn the_talent_grid_reports_what_is_owned_and_what_can_be_bought() {
        let mut simulation = arena();
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.talents.len(), TALENTS.len());
        assert!(snapshot.talents.iter().all(|talent| !talent.purchased));
        assert_eq!(
            snapshot.talent_points,
            simulation.mage().talents.available_points()
        );

        let first = snapshot.talents[0].id;
        simulation
            .mage_mut()
            .purchase_talent(first)
            .expect("affordable");
        let snapshot = UiSnapshot::build(&simulation);
        assert!(snapshot.talents[0].purchased);
    }

    #[test]
    fn a_cast_in_progress_is_reported_with_its_progress() {
        let mut simulation = arena();
        simulation.cast(FireAbility::Firebolt).expect("in range");
        simulation.step_many(30);

        let snapshot = UiSnapshot::build(&simulation);
        let cast = snapshot.cast.expect("a cast is running");
        assert_eq!(cast.name, "FIREBOLT");
        assert!(cast.fraction() > 0.0 && cast.fraction() < 1.0);
        assert!(cast.remaining() > 0.0);
    }

    #[test]
    fn a_refusal_reaches_the_interface() {
        let mut simulation = arena();
        let _ = simulation.cast(FireAbility::Detonate);
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            snapshot.notification.error,
            Some(AbilityError::NotEnoughIgnite)
        );
    }

    #[test]
    fn refreshing_reuses_the_snapshot_rather_than_rebuilding_it() {
        let simulation = arena();
        let mut snapshot = UiSnapshot::build(&simulation);
        let capacity = snapshot.talents.capacity();
        for _ in 0..100 {
            snapshot.refresh(&simulation, None);
        }
        assert_eq!(snapshot.talents.capacity(), capacity, "the snapshot grew");
        assert_eq!(snapshot.talents.len(), TALENTS.len());
    }
}
