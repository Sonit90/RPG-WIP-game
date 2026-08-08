//! The UI's view of the world: everything a combat HUD needs and nothing else.
//!
//! ```text
//! Simulation  --UiSnapshot::build-->  UiSnapshot  -->  UI layout  -->  UiDrawList
//!  (gameplay)        (this module)     (plain data)      (hud.rs)      (renderer)
//! ```
//!
//! ## Why a snapshot rather than `&Simulation`
//!
//! Phase 3's developer overlay borrows the simulation and reads whatever it likes. That is
//! acceptable for a debug readout and wrong for game UI, for three reasons that all become
//! real in this phase:
//!
//! * **Interpolation.** Once extraction interpolates between ticks (Milestone 9), the
//!   geometry on screen is *between* two simulation states. UI that reads the simulation
//!   directly would disagree with the geometry beside it about where a health bar goes.
//! * **A layout pass must not be able to mutate or re-query.** A snapshot is taken once per
//!   frame at a defined point; a borrow invites the UI to read a different answer in two
//!   places in the same frame.
//! * **A future network client has no `Simulation` to borrow.** It has a stream of state,
//!   and this is the shape that state arrives in.
//!
//! ## Allocation
//!
//! Every field is a fixed-size array or a scalar; there is no `Vec` anywhere in the
//! snapshot. Building one writes into storage that already exists, so a snapshot per frame
//! costs no allocation at all — asserted by `building_repeatedly_allocates_nothing`.

use engine_core::math::Vec3;
use engine_game::ability::{ABILITY_SLOTS, AbilityError, AbilityId};
use engine_game::entity::EntityId;
use engine_game::movement::MovementState;
use engine_game::sim::Simulation;
use engine_game::status::StatusKind;
use engine_game::world::{EntityKind, horizontal_distance};

/// Statuses carried per entity in a snapshot.
///
/// Four is what a nameplate has room for and more than the one status kind the game
/// currently has. A fifth would be dropped rather than grow the snapshot.
pub const MAX_SNAPSHOT_STATUSES: usize = 4;

/// One active status, as the UI needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusView {
    /// Which status this is.
    pub kind: StatusKind,
    /// Seconds left before it expires.
    pub remaining: f32,
    /// How much of its original duration is left, in [0, 1].
    pub fraction: f32,
}

/// The statuses on one entity, without a heap allocation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StatusList {
    entries: [Option<StatusView>; MAX_SNAPSHOT_STATUSES],
    count: usize,
}

impl StatusList {
    /// Appends a status, ignoring it once the list is full.
    fn push(&mut self, view: StatusView) {
        if self.count < MAX_SNAPSHOT_STATUSES {
            self.entries[self.count] = Some(view);
            self.count += 1;
        }
    }

    /// The statuses present, in the order they were collected.
    pub fn iter(&self) -> impl Iterator<Item = StatusView> + '_ {
        self.entries[..self.count].iter().filter_map(|entry| *entry)
    }

    /// How many statuses are listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether nothing is listed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// The local player, as the HUD needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerView {
    /// Which entity this describes.
    pub entity: EntityId,
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Current resource pool.
    pub resource: f32,
    /// Maximum resource pool.
    pub max_resource: f32,
    /// Where the character's feet are.
    pub position: Vec3,
    /// Which way the character faces, in radians about +Y.
    pub facing_yaw: f32,
    /// What the character is doing, for a future animation selector.
    pub movement: MovementState,
    /// Whether the character is alive.
    pub alive: bool,
    /// Statuses on the player.
    pub statuses: StatusList,
}

impl PlayerView {
    /// Health as a fraction in [0, 1]. Zero maximum reads as zero rather than dividing.
    #[must_use]
    pub fn health_fraction(&self) -> f32 {
        safe_fraction(self.health, self.max_health)
    }

    /// Resource as a fraction in [0, 1].
    #[must_use]
    pub fn resource_fraction(&self) -> f32 {
        safe_fraction(self.resource, self.max_resource)
    }
}

/// The selected target, as the HUD needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetView {
    /// Which entity is targeted.
    pub entity: EntityId,
    /// Display name.
    pub name: &'static str,
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Where the target's feet are.
    pub position: Vec3,
    /// How tall it is, so a world-space plate can sit above it.
    pub height: f32,
    /// Horizontal distance from the player, in metres.
    pub distance: f32,
    /// Whether the target is alive.
    pub alive: bool,
    /// Whether the target is a valid subject for a hostile ability.
    ///
    /// False for a corpse or an ally: what makes the target frame say "you cannot cast at
    /// this" without the UI re-deriving the rule the simulation already owns.
    pub valid: bool,
    /// Statuses on the target.
    pub statuses: StatusList,
}

impl TargetView {
    /// Health as a fraction in [0, 1].
    #[must_use]
    pub fn health_fraction(&self) -> f32 {
        safe_fraction(self.health, self.max_health)
    }

    /// Health as a whole percentage that never reads zero while the target lives.
    ///
    /// A dummy on its last point of health reading `0%` looks dead when it is not, which is
    /// the one thing a health readout must never say.
    #[must_use]
    pub fn health_percent(&self) -> u32 {
        health_percent(self.health, self.max_health, self.alive)
    }
}

/// Why an ability cannot be used right now.
///
/// A reason rather than a boolean, because the ability bar shows a different thing for
/// "cooling down" than for "you cannot afford it".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotAvailability {
    /// Usable now.
    Ready,
    /// Still cooling down.
    OnCooldown,
    /// Cooled down, but the resource cost cannot be paid.
    NoResource,
    /// Cooled down and affordable, but nothing valid is targeted.
    NoTarget,
}

impl SlotAvailability {
    /// Whether the ability can be used right now.
    #[must_use]
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// One ability slot, as the ability bar needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilitySlotView {
    /// Which ability sits in this slot.
    pub id: AbilityId,
    /// Its display name.
    pub name: &'static str,
    /// The digit that triggers it.
    pub hotkey: u8,
    /// Seconds of cooldown left, or zero.
    pub cooldown_remaining: f32,
    /// The slot's full cooldown, so a sweep knows what fraction to draw.
    pub cooldown_total: f32,
    /// What this costs to use.
    pub cost: f32,
    /// Why it is or is not usable.
    pub availability: SlotAvailability,
    /// Whether this slot holds a working placeholder rather than a designed ability.
    pub placeholder: bool,
}

impl AbilitySlotView {
    /// How much of the cooldown is still to run, in [0, 1].
    ///
    /// 1.0 the instant the ability is used, falling to 0.0 when it is ready. A cooldown
    /// sweep draws exactly this much of the slot.
    #[must_use]
    pub fn cooldown_fraction(&self) -> f32 {
        safe_fraction(self.cooldown_remaining, self.cooldown_total)
    }
}

/// What the player is casting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastView {
    /// Which ability is being cast.
    pub id: AbilityId,
    /// Its display name.
    pub name: &'static str,
    /// Seconds spent casting so far.
    pub elapsed: f32,
    /// Seconds the cast takes in total.
    pub duration: f32,
}

impl CastView {
    /// Cast progress in [0, 1].
    #[must_use]
    pub fn fraction(&self) -> f32 {
        safe_fraction(self.elapsed, self.duration)
    }

    /// Seconds left before the cast completes.
    #[must_use]
    pub fn remaining(&self) -> f32 {
        (self.duration - self.elapsed).max(0.0)
    }
}

/// The combat session, as the meter readout needs it.
///
/// `f64` because the meter accumulates in `f64`: a long session's total is a running sum,
/// and narrowing it here would make the HUD disagree with the meter it is reporting.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CombatView {
    /// Damage dealt this session.
    pub total_damage: f64,
    /// Damage per second of simulated combat.
    pub dps: f64,
    /// Seconds of combat the session covers.
    pub seconds: f64,
    /// Enemies killed this session.
    pub deaths: u32,
}

/// Transient messages for the player.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NotificationView {
    /// Why the last ability attempt was refused, if it was.
    pub last_error: Option<AbilityError>,
}

/// Everything the UI is allowed to know.
#[derive(Debug, Clone)]
pub struct UiSnapshot {
    /// The local player.
    pub player: PlayerView,
    /// The selected target, if there is one.
    pub target: Option<TargetView>,
    /// The ten ability slots, in slot order.
    pub abilities: [AbilitySlotView; ABILITY_SLOTS],
    /// The cast in progress, if any.
    pub cast: Option<CastView>,
    /// The combat session.
    pub combat: CombatView,
    /// Transient messages.
    pub notifications: NotificationView,
    /// Simulated seconds since the world was created.
    ///
    /// Every animated piece of UI is driven from this rather than from a frame counter, so
    /// a fade or a sweep runs at the same rate at 60 and 1,400 FPS.
    pub time: f32,
}

impl UiSnapshot {
    /// Builds a snapshot from the simulation's current state.
    ///
    /// Allocation: none. Every field is a fixed-size array or a scalar.
    #[must_use]
    pub fn build(simulation: &Simulation) -> Self {
        let world = simulation.world();
        let player = simulation.player();
        let entity = player.entity;

        let player_view = PlayerView {
            entity,
            health: world.health(entity).map_or(0.0, |health| health.current()),
            max_health: world.health(entity).map_or(0.0, |health| health.max()),
            resource: player.resource.current(),
            max_resource: player.resource.max(),
            position: world.position(entity).unwrap_or(Vec3::ZERO),
            facing_yaw: world.body(entity).map_or(0.0, |body| body.facing_yaw),
            movement: world
                .body(entity)
                .map_or(MovementState::Idle, |body| body.state),
            alive: world.is_alive(entity),
            statuses: status_list(simulation, entity),
        };

        let target = player.target.and_then(|id| target_view(simulation, id));

        let mut abilities = [ability_slot_view(simulation, 0); ABILITY_SLOTS];
        for (slot, view) in abilities.iter_mut().enumerate() {
            *view = ability_slot_view(simulation, slot);
        }

        let meter = simulation.meter();

        Self {
            player: player_view,
            target,
            abilities,
            cast: player.cast.map(|cast| CastView {
                id: cast.ability,
                name: player
                    .abilities
                    .get(cast.ability)
                    .map_or("", |definition| definition.name),
                elapsed: cast.elapsed,
                duration: cast.duration,
            }),
            combat: CombatView {
                total_damage: meter.total_damage(),
                dps: meter.dps(),
                seconds: meter.combat_seconds(),
                deaths: meter.deaths(),
            },
            notifications: NotificationView {
                last_error: player.last_error,
            },
            time: simulation.time() as f32,
        }
    }

    /// Rewrites this snapshot in place from the simulation.
    ///
    /// Equivalent to [`UiSnapshot::build`] but reuses the existing value, which is what the
    /// per-frame path calls.
    pub fn refresh(&mut self, simulation: &Simulation) {
        *self = Self::build(simulation);
    }
}

/// The statuses on one entity, capped at [`MAX_SNAPSHOT_STATUSES`].
fn status_list(simulation: &Simulation, id: EntityId) -> StatusList {
    let mut list = StatusList::default();
    let Some(statuses) = simulation.world().statuses(id) else {
        return list;
    };
    for status in statuses.iter() {
        list.push(StatusView {
            kind: status.kind,
            remaining: status.remaining,
            fraction: status.fraction(),
        });
    }
    list
}

/// One target's view, or `None` if the entity has gone away entirely.
fn target_view(simulation: &Simulation, id: EntityId) -> Option<TargetView> {
    let world = simulation.world();
    if !world.contains(id) {
        return None;
    }
    let player = simulation.player().entity;
    let alive = world.is_alive(id);
    let hostile = world
        .faction(player)
        .zip(world.faction(id))
        .is_some_and(|(mine, theirs)| mine.is_hostile_to(theirs));
    let position = world.position(id).unwrap_or(Vec3::ZERO);
    let player_position = world.position(player).unwrap_or(Vec3::ZERO);

    Some(TargetView {
        entity: id,
        name: world.kind(id).map_or("UNKNOWN", EntityKind::as_str),
        health: world.health(id).map_or(0.0, |health| health.current()),
        max_health: world.health(id).map_or(0.0, |health| health.max()),
        position,
        height: world.height(id),
        distance: horizontal_distance(player_position, position),
        alive,
        valid: alive && hostile,
        statuses: status_list(simulation, id),
    })
}

/// One ability slot's view.
fn ability_slot_view(simulation: &Simulation, slot: usize) -> AbilitySlotView {
    let player = simulation.player();
    let id = AbilityId::from_slot(slot).unwrap_or(AbilityId::NONE);
    let book = &player.abilities;
    let Some(definition) = book.get(id) else {
        return AbilitySlotView {
            id,
            name: "",
            hotkey: 0,
            cooldown_remaining: 0.0,
            cooldown_total: 0.0,
            cost: 0.0,
            availability: SlotAvailability::OnCooldown,
            placeholder: true,
        };
    };

    // The order matches the simulation's own validation order in `use_ability`, so the bar
    // shows the same reason the cast would report. Any other order would put the bar and
    // the error message out of step with each other.
    let remaining = book.cooldown_remaining(id);
    let availability = if remaining > 0.0 {
        SlotAvailability::OnCooldown
    } else if !player.resource.can_pay(definition.cost) {
        SlotAvailability::NoResource
    } else if definition.targeting.needs_target()
        && !simulation
            .player()
            .target
            .is_some_and(|target| simulation.world().is_alive(target))
    {
        SlotAvailability::NoTarget
    } else {
        SlotAvailability::Ready
    };

    AbilitySlotView {
        id,
        name: definition.name,
        hotkey: id.hotkey_digit(),
        cooldown_remaining: remaining,
        cooldown_total: definition.cooldown,
        cost: definition.cost,
        availability,
        placeholder: definition.placeholder,
    }
}

/// `value / max` clamped to [0, 1], with a zero maximum reading zero rather than dividing.
fn safe_fraction(value: f32, max: f32) -> f32 {
    if max <= 0.0 || !max.is_finite() {
        return 0.0;
    }
    (value / max).clamp(0.0, 1.0)
}

/// A whole percentage that never reads zero while the subject is alive.
fn health_percent(current: f32, max: f32, alive: bool) -> u32 {
    let fraction = safe_fraction(current, max);
    let percent = (fraction * 100.0).round() as u32;
    if alive { percent.max(1) } else { percent }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::command::PlayerCommand;

    fn sim() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation
    }

    #[test]
    fn the_snapshot_reports_the_player_the_ui_would_draw() {
        let simulation = sim();
        let snapshot = UiSnapshot::build(&simulation);

        assert_eq!(snapshot.player.entity, simulation.player().entity);
        assert!(snapshot.player.alive);
        assert!(snapshot.player.max_health > 0.0);
        assert!((snapshot.player.health_fraction() - 1.0).abs() < 1e-4);
        assert!((snapshot.player.resource_fraction() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn the_nearest_dummy_is_the_target_and_carries_its_health() {
        let simulation = sim();
        let snapshot = UiSnapshot::build(&simulation);
        let target = snapshot.target.expect("the arena starts with a target");

        assert_eq!(target.name, "DUMMY");
        assert!(target.valid, "a live hostile is a valid target");
        assert!(target.distance > 0.0);
        assert_eq!(target.health_percent(), 100);
    }

    #[test]
    fn a_health_readout_never_says_zero_while_the_target_lives() {
        // The one thing a health bar must not do: report a living enemy as dead.
        let mut simulation = sim();
        let target = simulation.player().target.expect("a dummy is targeted");
        let max = simulation
            .world()
            .health(target)
            .expect("the dummy has health")
            .max();
        simulation.world_mut().damage(target, max - 0.5);

        let snapshot = UiSnapshot::build(&simulation);
        let view = snapshot.target.expect("still targeted");
        assert!(view.alive);
        assert_eq!(view.health_percent(), 1, "0% would read as dead");
        assert!(view.health_fraction() > 0.0);
    }

    #[test]
    fn a_dead_target_is_reported_as_invalid_rather_than_dropped() {
        // Dropping it would make the target frame vanish the instant something dies, which
        // is exactly when the player wants to see that it died.
        let mut simulation = sim();
        let target = simulation.player().target.expect("a dummy is targeted");
        simulation.world_mut().damage(target, 1_000_000.0);

        let snapshot = UiSnapshot::build(&simulation);
        let view = snapshot.target.expect("the corpse is still described");
        assert!(!view.alive);
        assert!(!view.valid);
        assert_eq!(view.health_percent(), 0);
    }

    #[test]
    fn every_ability_slot_is_described_with_its_hotkey() {
        let simulation = sim();
        let snapshot = UiSnapshot::build(&simulation);

        assert_eq!(snapshot.abilities.len(), ABILITY_SLOTS);
        for (slot, view) in snapshot.abilities.iter().enumerate() {
            assert!(!view.name.is_empty(), "slot {slot} has no name");
            assert_eq!(view.hotkey, ((slot + 1) % 10) as u8);
            assert!(view.cooldown_total > 0.0);
        }
        assert_eq!(snapshot.abilities[0].name, "FIREBALL");
        assert_eq!(snapshot.abilities[0].availability, SlotAvailability::Ready);
    }

    #[test]
    fn a_used_ability_reports_its_cooldown_as_a_fraction_that_falls_to_zero() {
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(
            AbilityId::from_slot(0).expect("slot 0"),
        ));
        simulation.step_once();

        let fresh = UiSnapshot::build(&simulation);
        let slot = fresh.abilities[0];
        assert_eq!(slot.availability, SlotAvailability::OnCooldown);
        assert!(slot.cooldown_remaining > 0.0);
        assert!(
            slot.cooldown_fraction() > 0.9,
            "a just-used ability is nearly a full sweep: {}",
            slot.cooldown_fraction()
        );

        // Run past the cooldown; the sweep must stop at zero rather than going negative.
        simulation.step_many(240);
        let later = UiSnapshot::build(&simulation);
        assert_eq!(later.abilities[0].availability, SlotAvailability::Ready);
        assert_eq!(later.abilities[0].cooldown_fraction(), 0.0);
    }

    #[test]
    fn an_unaffordable_ability_is_distinguished_from_a_cooling_one() {
        // The bar draws these differently, so the snapshot has to tell them apart.
        let mut simulation = sim();
        let expensive = AbilityId::from_slot(2).expect("slot 3 is detonate");
        let cost = simulation
            .player()
            .abilities
            .get(expensive)
            .expect("slot 3 is filled")
            .cost;
        let pool = simulation.player().resource.current();
        assert!(
            simulation.player_mut().resource.pay(pool - (cost - 1.0)),
            "draining the pool to just under the cost must succeed"
        );

        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            snapshot.abilities[2].availability,
            SlotAvailability::NoResource
        );
    }

    #[test]
    fn statuses_reach_the_snapshot_with_their_remaining_time() {
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(
            AbilityId::from_slot(1).expect("slot 2 is ignite"),
        ));
        simulation.step_once();

        let snapshot = UiSnapshot::build(&simulation);
        let target = snapshot.target.expect("still targeted");
        let burning = target
            .statuses
            .iter()
            .find(|status| status.kind == StatusKind::Burning)
            .expect("ignite applies burning");
        assert!(burning.remaining > 0.0);
        assert!(burning.fraction > 0.0 && burning.fraction <= 1.0);
    }

    #[test]
    fn the_status_list_is_bounded_rather_than_growing() {
        let mut list = StatusList::default();
        for _ in 0..MAX_SNAPSHOT_STATUSES + 3 {
            list.push(StatusView {
                kind: StatusKind::Burning,
                remaining: 1.0,
                fraction: 1.0,
            });
        }
        assert_eq!(list.len(), MAX_SNAPSHOT_STATUSES);
        assert_eq!(list.iter().count(), MAX_SNAPSHOT_STATUSES);
    }

    #[test]
    fn the_combat_session_reaches_the_snapshot() {
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(
            AbilityId::from_slot(0).expect("slot 1"),
        ));
        simulation.step_once();

        let snapshot = UiSnapshot::build(&simulation);
        assert!(snapshot.combat.total_damage > 0.0);
        assert_eq!(snapshot.combat.deaths, 0);
        assert!(snapshot.time > 0.0);
    }

    #[test]
    fn a_refused_cast_reaches_the_notifications() {
        let mut simulation = sim();
        let fireball = AbilityId::from_slot(0).expect("slot 1");
        simulation.push_command(PlayerCommand::UseAbility(fireball));
        simulation.step_once();
        simulation.push_command(PlayerCommand::UseAbility(fireball));
        simulation.step_once();

        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            snapshot.notifications.last_error,
            Some(AbilityError::OnCooldown)
        );
    }

    #[test]
    fn building_repeatedly_allocates_nothing() {
        // The snapshot is fixed-size by construction: no Vec, no String, no Box. This test
        // asserts the property that makes a per-frame snapshot free.
        let mut simulation = sim();
        let mut snapshot = UiSnapshot::build(&simulation);
        for _ in 0..1_000 {
            simulation.step_once();
            snapshot.refresh(&simulation);
        }
        assert_eq!(snapshot.abilities.len(), ABILITY_SLOTS);
        assert_eq!(
            std::mem::size_of::<UiSnapshot>(),
            std::mem::size_of_val(&snapshot),
            "the snapshot is a value, not a handle to heap storage"
        );
    }

    #[test]
    fn a_snapshot_of_a_world_without_a_player_does_not_panic() {
        // A stale player entity is what a reset or a future respawn produces mid-frame.
        let mut simulation = sim();
        let player = simulation.player().entity;
        simulation.world_mut().despawn(player);

        let snapshot = UiSnapshot::build(&simulation);
        assert!(!snapshot.player.alive);
        assert_eq!(snapshot.player.health_fraction(), 0.0);
    }
}
