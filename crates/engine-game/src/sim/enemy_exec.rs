//! The AI's half of a tick: perceive, decide, move, and resolve what was asked for.
//!
//! ```text
//! for each enemy:
//!    perceive  ── threat table + world  ─▶ AiPercept
//!    think     ── EnemyBrain::think     ─▶ AiDecision { intent, request }
//!    intent    ── stored for movement, which runs later in the same tick
//!    request   ── validated and executed here: casts, telegraphs, damage
//!    charge    ── a body carried along its line instead, until it lands
//!    channel   ── a cast that deals its ticks while it runs
//! ```
//!
//! # This file is the *combat* half, not the AI half
//!
//! [`crate::enemy::brain`] decides what an enemy wants. Everything that actually changes the
//! world — starting a cast, announcing a telegraph, collecting who is inside a shape,
//! dealing damage, ending a fight — happens here, through the same
//! [`Simulation::deal_damage`] every player ability goes through. An enemy has no privileged
//! path to anybody's health.
//!
//! # The brain aims at one; this file aims at the rest
//!
//! A brain sees its target and nothing else, so it starts every cast on one recipient. When
//! the ability asks for more — three players, a scatter of seven zones, a line through each
//! of them — [`Simulation::plan_cast`] fills the cast in from the world at the moment it
//! starts, and everything after that (the telegraphs, the hit test, the delayed strikes)
//! reads the planned zones. One plan, used everywhere, so what is drawn is what lands.
//!
//! # Why the intent is stored rather than applied
//!
//! Movement integration runs once, for every body, later in the tick. Writing a position
//! here would mean two systems moved bodies and the character controller was only one of
//! them. Instead the brain's intent is parked on the runtime and `integrate_bodies` reads
//! it, so an enemy walks with exactly the controller the player walks with — acceleration,
//! ground contact, collision and all. A charge is the one exception, and it goes through the
//! same collided move the player's own Shield Charge does.

use engine_core::math::Vec3;

use crate::ability::AbilityId;
use crate::enemy::brain::{
    AiPercept, AiRequest, AiState, AiTarget, CastStop, EnemyBrain, EnemyCast, MAX_TARGETS,
    MAX_ZONES, aimed_shape, yaw_towards,
};
use crate::enemy::strike::PendingStrike;
use crate::enemy::{
    EnemyAbility, EnemyAim, EnemyEffect, EnemyKind, EnemyRider, EnemyTarget, TelegraphShape,
};
use crate::entity::EntityId;
use crate::event::CombatEvent;
use crate::movement::MoveIntent;
use crate::sim::CastEnd;
use crate::status::{Status, StatusKind};
use crate::world::horizontal_distance;

use crate::targeting::TargetQuery;

use super::Simulation;

/// Metres from its destination at which a charge counts as arrived.
const CHARGE_CONTACT: f32 = 0.6;

/// Seconds a charge may run past the time its distance should take before it gives up.
///
/// A charge into something that stops it — a pillar, a wall, a body — would otherwise run
/// forever against it. The same safety the Warrior's own Shield Charge has.
const CHARGE_SLACK: f32 = 0.6;

/// How far, edge to edge, a charge's target may be when it lands and still be hit, in metres.
const CHARGE_REACH: f32 = 2.0;

/// How far past the range a channel's victim may drift before a tick skips them, in metres.
///
/// A drain is not broken by a step backwards; it is broken by an interrupt. A victim who
/// runs a long way out is spared the tick, which is the other honest answer.
const CHANNEL_LEASH: f32 = 6.0;

/// One enemy's runtime: its brain, and what the last decision asked the body to do.
#[derive(Debug, Clone, PartialEq)]
pub struct EnemyRuntime {
    /// The state machine.
    pub brain: EnemyBrain,
    /// What movement should do with this body this tick.
    pub intent: MoveIntent,
    /// The telegraphs this enemy currently has up, as the first id and how many.
    ///
    /// A cast's zones are announced with consecutive ids, so a range names all of them and
    /// every path that takes a cast down — landing, an interrupt, a death, a reset — ends
    /// exactly the ones it put up.
    pub telegraphs: Option<(u32, u8)>,
    /// The charge the body is being carried along, if it is charging.
    pub dash: Option<EnemyDash>,
    /// What its damage is multiplied by: its level, and the party size the level was built
    /// for.
    pub damage_scale: f32,
    /// What killing it is worth towards dungeon completion.
    pub completion: f32,
    /// Whether its completion has already been counted, so a corpse is credited once.
    pub credited: bool,
    /// The name the level gave it, if the author gave it one.
    pub display_name: Option<String>,
}

impl EnemyRuntime {
    /// A runtime for a `kind` spawned at `home`.
    #[must_use]
    pub fn new(kind: EnemyKind, home: Vec3, facing_yaw: f32) -> Self {
        Self {
            brain: EnemyBrain::new(kind, home, facing_yaw),
            intent: MoveIntent::NONE,
            telegraphs: None,
            dash: None,
            damage_scale: 1.0,
            completion: kind.archetype().completion,
            credited: false,
            display_name: None,
        }
    }
}

/// A charge in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnemyDash {
    /// Which ability is charging.
    pub ability: EnemyAbility,
    /// Where it set off from, for the trail it leaves.
    pub origin: Vec3,
    /// Who it is running at. It follows them: a charge is cast on a player, not at a spot.
    pub target: Option<EntityId>,
    /// Where it is running to: its target's position, refreshed every step while they live.
    pub destination: Vec3,
    /// Seconds it has been running.
    pub elapsed: f32,
    /// Seconds it may run before it gives up.
    pub timeout: f32,
}

impl Simulation {
    /// Runs every enemy's brain and carries out what it asked for.
    ///
    /// Called once a tick, before movement, so an intent decided this tick is walked this
    /// tick rather than next.
    pub(super) fn tick_ai(&mut self, step: f32) {
        // Taken out of `self` so the loop can call `&mut self` methods; put back at the end,
        // keeping every allocation. The same pattern `apply_pending_commands` uses.
        let mut enemies = std::mem::take(&mut self.enemies);
        // Indexed rather than iterated: the body needs `&mut self` *and* a second look at
        // this slot after executing, and an iterator over `enemies` would hold a borrow
        // across both.
        #[allow(
            clippy::needless_range_loop,
            reason = "the slot index is used again inside the body"
        )]
        for slot in 0..enemies.len() {
            let Some(runtime) = enemies[slot].as_mut() else {
                continue;
            };
            let Some(entity) = self.world.id_at(slot) else {
                // The slot was freed under it: nothing to drive.
                continue;
            };
            // A corpse is not tracked. Its brain has already been through `die` — which
            // dropped its target, its threat and its cast — and everything downstream of it
            // has been cleaned up by the death itself: its burn, its statuses, its telegraph
            // and the ground it lit. Thinking again would produce `AiDecision::IDLE` at the
            // cost of a full perception pass over the world, which in a cleared dungeon room
            // is several scans a tick for enemies nobody is fighting.
            //
            // Deliberately keyed on the *brain* rather than on the world: an entity killed
            // outside the combat path has a body at zero health and a brain that has not
            // noticed, and that one still has to think once so it can die properly.
            if runtime.brain.state() == AiState::Dead {
                continue;
            }
            // A boss behind a sealed gate does not notice anybody: it waits for the dungeon to
            // be complete enough, however close the player stands to the bars.
            if self.is_sealed(entity) {
                runtime.intent = MoveIntent::NONE;
                continue;
            }

            // Mid-charge: the body is carried, the mind waits.
            if let Some(dash) = runtime.dash
                && self.world.is_alive(entity)
            {
                runtime.brain.hold(step);
                let scale = runtime.damage_scale;
                runtime.dash = self.advance_dash(entity, dash, step, scale);
                runtime.intent = MoveIntent {
                    direction: Vec3::ZERO,
                    jump: false,
                    facing: None,
                };
                continue;
            }
            runtime.dash = None;

            let percept = self.perceive(entity, &runtime.brain);
            // The cast as it was *before* thinking: a cast that completes this tick is taken
            // off the brain by `think`, and the anchor and shape it was started with are
            // exactly what has to resolve. Reading them afterwards would find nothing and
            // fall back to the table's neutral shape - a cone pointing wherever the table
            // said, which is not where the telegraph was drawn.
            let running = runtime.brain.cast();
            let decision = runtime.brain.think(&percept, step);
            runtime.intent = decision.intent;

            let telegraphs = runtime.telegraphs;
            let request = decision.request;
            let brain_state = runtime.brain.state();
            let scale = runtime.damage_scale;

            if let Some(phase) = decision.phase {
                self.announce_phase(entity, phase, runtime.brain.phase_name());
            }

            match request {
                AiRequest::None => {}
                AiRequest::Instant(ability) => {
                    if let EnemyEffect::Frenzy { duration, haste } = ability.definition().effect {
                        runtime.brain.frenzy(duration, haste);
                        self.announce_self_ability(entity, ability);
                    } else {
                        let victim = recipient_of(ability, &percept);
                        let aim = cast_anchor(ability, entity, victim, self);
                        self.execute_enemy_ability(entity, ability, victim, aim, scale);
                    }
                }
                AiRequest::BeginCast(ability) => {
                    if let Some(cast) = runtime.brain.cast_mut() {
                        self.plan_cast(entity, cast, &percept);
                    }
                    if let Some(cast) = runtime.brain.cast() {
                        self.announce_cast_start(entity, ability, cast.duration);
                        runtime.telegraphs = self.announce_cast_telegraphs(entity, &cast);
                    }
                }
                AiRequest::CompleteCast(ability) => {
                    if let Some((first, count)) = telegraphs {
                        let anchor = running.map_or(Vec3::ZERO, |cast| cast.anchor);
                        self.end_telegraphs(first, count, anchor, true);
                        runtime.telegraphs = None;
                    }
                    self.announce_cast_end(entity, ability, CastEnd::Completed);
                    match running {
                        Some(cast) => {
                            if let Some(dash) = self.resolve_cast(entity, &cast, &percept, scale) {
                                runtime.dash = Some(dash);
                            }
                        }
                        // A cast with no record of how it started: resolve the table's shape on
                        // whoever the percept names. Nothing produces this today; it is the
                        // safe answer if something ever does.
                        None => {
                            let victim = recipient_of(ability, &percept);
                            let aim = cast_anchor(ability, entity, victim, self);
                            self.execute_enemy_ability(entity, ability, victim, aim, scale);
                        }
                    }
                }
                AiRequest::CancelCast(ability) => {
                    if let Some((first, count)) = telegraphs {
                        let anchor = running.map_or_else(
                            || self.world.position(entity).unwrap_or(Vec3::ZERO),
                            |cast| cast.anchor,
                        );
                        self.end_telegraphs(first, count, anchor, false);
                        runtime.telegraphs = None;
                    }
                    let reason = if brain_state == AiState::Dead {
                        CastEnd::CasterDied
                    } else {
                        CastEnd::Cancelled
                    };
                    self.announce_cast_end(entity, ability, reason);
                }
            }

            // A channel deals its ticks while it runs. After the decision, so a channel that
            // started this tick has not ticked yet and one that completed this tick paid its
            // last ticks out in `resolve_cast`.
            if let Some(cast) = runtime.brain.cast()
                && let EnemyEffect::Channel { interval, .. } = cast.ability.definition().effect
            {
                let due = crate::enemy::channel_ticks(cast.elapsed, interval);
                if due > usize::from(cast.ticks) {
                    let done = self.pay_channel(entity, &cast, due, scale);
                    if let Some(cast) = runtime.brain.cast_mut() {
                        cast.ticks = done;
                    }
                }
            }
        }
        self.enemies = enemies;
        self.sync_encounter();
    }

    /// Fills in the targets and zones a cast asks for, from the world as it is now.
    ///
    /// The brain started the cast on one recipient; this is where "three players", "seven
    /// zones around the target" and "a line to each of them" become positions. Random picks
    /// come from the simulation's seeded generator, so a replay plans the same cast.
    fn plan_cast(&mut self, entity: EntityId, cast: &mut EnemyCast, percept: &AiPercept) {
        let definition = cast.ability.definition();
        let caster = percept.position;
        let wanted = usize::from(definition.targets.max(1)).min(MAX_TARGETS);

        // Who it is aimed at: the recipient the brain chose, or a random hostile for an
        // ability that reaches past the tank, then the nearest others in range.
        let mut targets = [None; MAX_TARGETS];
        if definition.select == EnemyTarget::Random {
            let exclude = percept.target.map(|target| target.entity);
            let mut picked = 0;
            while picked < wanted {
                let Some(choice) = self.random_hostile(
                    entity,
                    (definition.min_range, definition.range),
                    exclude,
                    &targets,
                ) else {
                    break;
                };
                targets[picked] = Some(choice);
                picked += 1;
            }
            if picked == 0 {
                targets[0] = Some(cast.target);
            }
        } else {
            targets[0] = Some(cast.target);
            if wanted > 1 {
                self.nearest_hostiles_into(entity, definition.range, &mut targets);
            }
        }

        // Where each zone goes. Zone zero of each target is the target (or the caster, for
        // something centred on itself); the rest are scattered on a ring around it.
        let per_target = usize::from(definition.zones.max(1));
        let turn = self.enemy_random() * std::f32::consts::TAU;
        let mut count = 0_usize;
        for target in targets.iter().flatten() {
            let target_position = self.world.position(*target).unwrap_or(cast.anchor);
            let base = match definition.aim {
                EnemyAim::Target => target_position,
                EnemyAim::Caster => caster,
            };
            let yaw = yaw_towards(caster, target_position);
            for index in 0..per_target {
                if count >= MAX_ZONES {
                    break;
                }
                let anchor = if index == 0 || definition.spread <= 0.0 {
                    base
                } else {
                    let ring = (per_target - 1).max(1) as f32;
                    let angle = turn + std::f32::consts::TAU * (index - 1) as f32 / ring;
                    // Alternating radii, so a seven-zone barrage is two rings rather than one
                    // circle with a gap in the middle a player would learn to stand in.
                    let reach = if index % 2 == 0 { 1.0 } else { 0.6 } * definition.spread;
                    Vec3::new(
                        base.x + angle.sin() * reach,
                        base.y,
                        base.z + angle.cos() * reach,
                    )
                };
                cast.zones[count] = anchor;
                cast.aims[count] = target_position;
                cast.yaws[count] = yaw;
                cast.zone_targets[count] = Some(*target);
                count += 1;
            }
        }
        if count == 0 {
            count = 1;
        }
        cast.zone_count = count as u8;
        cast.targets = targets;
        cast.target = targets[0].unwrap_or(cast.target);
        cast.anchor = cast.zones[0];

        cast.shape = aimed_shape(definition.shape, cast.yaws[0]);
    }

    /// Puts a cast's warnings on the floor, and returns the range of ids it used.
    ///
    /// One telegraph per zone for an area that lands with the cast. None for a delayed
    /// ability — its zones are drawn when the cast finishes and they start counting — and
    /// none for a single-target cast, whose cast bar is its warning.
    fn announce_cast_telegraphs(
        &mut self,
        entity: EntityId,
        cast: &EnemyCast,
    ) -> Option<(u32, u8)> {
        let definition = cast.ability.definition();
        // A charge is cast on a player and draws nothing on the floor: its cast bar is the
        // warning, and there is no line to step off.
        if definition.delay > 0.0 || !cast.shape.is_area() || definition.effect.is_charge() {
            return None;
        }
        let zones = usize::from(cast.zone_count).clamp(1, MAX_ZONES);
        let first = self.next_telegraph;
        for index in 0..zones {
            let id = self.next_telegraph_id();
            let shape = cast.zone_shape(index);
            self.announce_telegraph(
                id,
                entity,
                cast.ability,
                cast.zones[index],
                shape,
                cast.duration,
                None,
            );
        }
        Some((first, zones as u8))
    }

    /// Resolves a cast that has finished. Returns the charge to start, if it was a charge.
    fn resolve_cast(
        &mut self,
        entity: EntityId,
        cast: &EnemyCast,
        percept: &AiPercept,
        scale: f32,
    ) -> Option<EnemyDash> {
        let definition = cast.ability.definition();
        match definition.effect {
            EnemyEffect::Heal { .. } => {
                // Whoever the cast was started against — for a heal that is an ally, and
                // re-asking the percept here would find the thing the enemy is fighting.
                let recipient = self
                    .describe_target(entity, cast.target)
                    .or_else(|| recipient_of(cast.ability, percept));
                self.execute_enemy_ability(entity, cast.ability, recipient, cast.anchor, scale);
                None
            }
            EnemyEffect::Channel { interval, .. } => {
                // The last ticks, paid on the tick it finished rather than one late.
                let total = crate::enemy::channel_ticks(cast.duration, interval);
                self.pay_channel(entity, cast, total, scale);
                None
            }
            EnemyEffect::Charge { speed, .. } => {
                let origin = self.world.position(entity).unwrap_or(cast.anchor);
                let target = cast.targets[0].filter(|target| self.world.is_alive(*target));
                let destination = target
                    .and_then(|target| self.world.position(target))
                    .unwrap_or(cast.aims[0]);
                let distance = horizontal_distance(origin, destination);
                Some(EnemyDash {
                    ability: cast.ability,
                    target,
                    origin,
                    destination,
                    elapsed: 0.0,
                    timeout: distance / speed.max(1.0) + CHARGE_SLACK,
                })
            }
            EnemyEffect::Frenzy { .. } => None,
            EnemyEffect::Damage | EnemyEffect::Hazard { .. } => {
                if definition.delay > 0.0 {
                    // Every zone starts its own clock now, drawn from this moment.
                    for (anchor, shape, _, laid_on) in cast.zone_list() {
                        let follow = if definition.follow { laid_on } else { None };
                        self.lay_strike(
                            entity,
                            cast.ability,
                            anchor,
                            shape,
                            follow,
                            definition.delay,
                            scale,
                            false,
                        );
                    }
                    return None;
                }
                if !cast.shape.is_area() {
                    // Single target, once for each target it was aimed at.
                    for target in cast.target_list() {
                        let recipient = self.describe_target(entity, target);
                        self.execute_enemy_ability(
                            entity,
                            cast.ability,
                            recipient,
                            cast.anchor,
                            scale,
                        );
                    }
                    return None;
                }
                for (anchor, shape, aim, _) in cast.zone_list() {
                    self.resolve_zone(entity, cast.ability, anchor, shape, aim, scale);
                }
                None
            }
        }
    }

    /// Pays a channel's ticks up to `due`, and returns how many have now been paid.
    ///
    /// Each tick hits every zone the channel planned — a circle, or each of the players a
    /// drain is draining — and grows by the ability's ramp over the one before.
    fn pay_channel(&mut self, entity: EntityId, cast: &EnemyCast, due: usize, scale: f32) -> u8 {
        let definition = cast.ability.definition();
        let EnemyEffect::Channel { interval, ramp } = definition.effect else {
            return cast.ticks;
        };
        let total = crate::enemy::channel_ticks(cast.duration, interval);
        let due = due.min(total);
        let mut done = usize::from(cast.ticks);
        while done < due {
            let growth = 1.0 + ramp * done as f32;
            let amount = definition.damage * growth * scale;
            if cast.shape.is_area() {
                for (anchor, shape, _, _) in cast.zone_list() {
                    self.hit_shape(entity, cast.ability, anchor, shape, amount);
                }
            } else {
                for target in cast.target_list() {
                    let Some(described) = self.describe_target(entity, target) else {
                        continue;
                    };
                    if described.distance > definition.range + CHANNEL_LEASH {
                        continue;
                    }
                    self.strike_one(entity, cast.ability, target, amount);
                }
            }
            done += 1;
        }
        done.min(usize::from(u8::MAX)) as u8
    }

    /// Carries a charging body one step along its line, and lands it when it arrives.
    ///
    /// Returns the charge still running, or `None` once it has landed.
    fn advance_dash(
        &mut self,
        entity: EntityId,
        mut dash: EnemyDash,
        step: f32,
        scale: f32,
    ) -> Option<EnemyDash> {
        dash.elapsed += step;
        // Following its target: a charge is cast on a player, and running away from one
        // only makes it longer.
        if let Some(target) = dash.target
            && self.world.is_alive(target)
            && let Some(position) = self.world.position(target)
        {
            dash.destination = position;
        }
        let origin = self.world.position(entity).unwrap_or(dash.destination);
        let offset = Vec3::new(
            dash.destination.x - origin.x,
            0.0,
            dash.destination.z - origin.z,
        );
        let distance = offset.length();
        let EnemyEffect::Charge { speed, .. } = dash.ability.definition().effect else {
            return None;
        };
        // Arrived means touching: edge to edge with its target, or on the spot with none.
        let contact = CHARGE_CONTACT
            + dash.target.map_or(0.0, |target| {
                self.world.radius(target) + self.world.radius(entity)
            });
        if distance <= contact || dash.elapsed >= dash.timeout {
            self.land_dash(entity, &dash, scale);
            return None;
        }
        let travel = (speed * step).min(distance);
        let direction = offset * (1.0 / distance);
        self.slide_body(entity, direction * travel);
        if let Some(body) = self.world.body_mut(entity) {
            body.facing_yaw = yaw_towards(origin, dash.destination);
        }
        Some(dash)
    }

    /// The end of a charge: the impact, the trail it leaves and the shockwave that follows.
    fn land_dash(&mut self, entity: EntityId, dash: &EnemyDash, scale: f32) {
        let definition = dash.ability.definition();
        let EnemyEffect::Charge {
            impact_radius,
            shockwave_radius,
            trail_damage_per_second,
            trail_duration,
            slow,
            slow_duration,
            ..
        } = definition.effect
        else {
            return;
        };
        let landing = self.world.position(entity).unwrap_or(dash.destination);
        let amount = definition.damage * scale;
        // The player it was cast on takes the hit and the slow — if the run reached them. One
        // that outran the timeout, or put a pillar between them, is spared.
        let reached = dash.target.filter(|target| {
            self.describe_target(entity, *target)
                .is_some_and(|described| described.distance <= CHARGE_REACH)
        });
        if let Some(target) = reached {
            self.strike_one(entity, dash.ability, target, amount);
            if slow > 0.0 && slow_duration > 0.0 && self.world.is_alive(target) {
                let status = Status::new(
                    StatusKind::Crippled,
                    entity,
                    AbilityId::from_enemy(dash.ability),
                    slow_duration,
                    0.0,
                )
                .with_magnitude(slow);
                self.apply_status(target, status, slow_duration);
            }
        } else {
            self.announce_self_ability(entity, dash.ability);
        }
        // A pounce's landing blast: everyone else around where it came down.
        if impact_radius > 0.0 {
            let impact = TelegraphShape::Circle {
                radius: impact_radius,
            };
            self.hit_shape_except(entity, dash.ability, landing, impact, amount, reached);
        }

        // The ground it ran over burns, a pool every few metres from where it set off to
        // where it stopped: the path of the charge is a place the party has to leave.
        if trail_damage_per_second > 0.0 && trail_duration > 0.0 {
            let run = horizontal_distance(dash.origin, landing);
            let pools = (run / crate::enemy::TRAIL_SPACING).floor() as usize;
            for index in 0..=pools {
                let along = if run <= f32::EPSILON {
                    0.0
                } else {
                    (index as f32 * crate::enemy::TRAIL_SPACING / run).min(1.0)
                };
                let center = Vec3::new(
                    dash.origin.x + (landing.x - dash.origin.x) * along,
                    landing.y,
                    dash.origin.z + (landing.z - dash.origin.z) * along,
                );
                self.light_hazard(
                    entity,
                    dash.ability,
                    center,
                    crate::enemy::TRAIL_RADIUS,
                    trail_duration,
                    trail_damage_per_second * scale,
                );
            }
        }

        // A second ring, a moment later, around where it stopped.
        if shockwave_radius > 0.0 {
            self.lay_strike(
                entity,
                dash.ability,
                landing,
                TelegraphShape::Circle {
                    radius: shockwave_radius,
                },
                None,
                crate::enemy::SHOCKWAVE_DELAY,
                scale * crate::enemy::SHOCKWAVE_SHARE,
                true,
            );
        }
    }

    /// Lays one delayed zone, drawn from now until it lands.
    #[allow(
        clippy::too_many_arguments,
        reason = "a strike is this many facts, and naming a struct for one call site hides them"
    )]
    fn lay_strike(
        &mut self,
        owner: EntityId,
        ability: EnemyAbility,
        anchor: Vec3,
        shape: TelegraphShape,
        follow: Option<EntityId>,
        delay: f32,
        damage_scale: f32,
        chained: bool,
    ) {
        let telegraph = self.next_telegraph_id();
        self.announce_telegraph(telegraph, owner, ability, anchor, shape, delay, follow);
        let mut dropped = std::mem::take(&mut self.strike_scratch);
        dropped.clear();
        self.strikes.push(
            PendingStrike {
                telegraph,
                owner,
                ability,
                anchor,
                follow,
                shape,
                remaining: delay,
                damage_scale,
                chained,
            },
            &mut dropped,
        );
        for strike in dropped.drain(..) {
            self.announce_telegraph_end(strike.telegraph, strike.anchor, false);
        }
        self.strike_scratch = dropped;
    }

    /// Counts every delayed zone down and resolves the ones that land.
    ///
    /// Run once a tick from the simulation's own step, after the AI, so a zone laid this tick
    /// does not also land this tick.
    pub(super) fn tick_enemy_strikes(&mut self, step: f32) {
        if self.strikes.is_empty() {
            return;
        }
        let world = &self.world;
        self.strikes.follow(|victim| {
            world
                .is_alive(victim)
                .then(|| world.position(victim))
                .flatten()
        });
        let mut landed = std::mem::take(&mut self.strike_scratch);
        landed.clear();
        self.strikes.tick(step, &mut landed);
        for strike in &landed {
            self.announce_telegraph_end(strike.telegraph, strike.anchor, true);
            if !self.world.contains(strike.owner) {
                continue;
            }
            let definition = strike.ability.definition();
            let amount = definition.damage * strike.damage_scale;
            self.hit_shape(
                strike.owner,
                strike.ability,
                strike.anchor,
                strike.shape,
                amount,
            );
            // A chain: everybody the detonation caught who was not its own victim carries a
            // mark of their own now. Once — a chained mark does not chain again.
            if definition.chain && !strike.chained {
                let mut caught = std::mem::take(&mut self.chain_scratch);
                caught.clear();
                caught.extend(self.hits_last.iter().copied());
                for victim in &caught {
                    if Some(*victim) == strike.follow || !self.world.is_alive(*victim) {
                        continue;
                    }
                    let anchor = self.world.position(*victim).unwrap_or(strike.anchor);
                    self.lay_strike(
                        strike.owner,
                        strike.ability,
                        anchor,
                        strike.shape,
                        Some(*victim),
                        crate::enemy::CHAIN_DELAY,
                        strike.damage_scale,
                        true,
                    );
                }
                caught.clear();
                self.chain_scratch = caught;
            }
        }
        landed.clear();
        self.strike_scratch = landed;
    }

    /// Takes away every delayed zone `owner` laid, and their warnings with them.
    pub(super) fn clear_strikes_of(&mut self, owner: EntityId) {
        let mut removed = std::mem::take(&mut self.strike_scratch);
        removed.clear();
        self.strikes.clear_owner(owner, &mut removed);
        for strike in removed.drain(..) {
            self.announce_telegraph_end(strike.telegraph, strike.anchor, false);
        }
        self.strike_scratch = removed;
    }

    /// Takes every delayed zone away. What a reset runs.
    pub(super) fn clear_all_strikes(&mut self) {
        let mut removed = std::mem::take(&mut self.strike_scratch);
        removed.clear();
        self.strikes.clear_into(&mut removed);
        for strike in removed.drain(..) {
            self.announce_telegraph_end(strike.telegraph, strike.anchor, false);
        }
        self.strike_scratch = removed;
    }

    /// One entity described the way the AI sees it, from `entity`'s point of view.
    ///
    /// The same edge-to-edge distance [`Simulation::perceive`] measures, so a range check
    /// against this and a range check against a percept cannot disagree.
    fn describe_target(&self, entity: EntityId, candidate: EntityId) -> Option<AiTarget> {
        if !self.world.is_alive(candidate) {
            return None;
        }
        let position = self.world.position(entity)?;
        let target_position = self.world.position(candidate)?;
        let gap = horizontal_distance(position, target_position)
            - self.world.radius(candidate)
            - self.world.radius(entity);
        Some(AiTarget {
            entity: candidate,
            position: target_position,
            distance: gap.max(0.0),
        })
    }

    /// Whether `candidate` is a living combatant `entity` may attack.
    fn is_hostile_combatant(&self, entity: EntityId, candidate: EntityId) -> bool {
        if candidate == entity
            || !self.world.is_alive(candidate)
            || !TargetQuery::is_combatant(&self.world, candidate)
        {
            return false;
        }
        let Some(mine) = self.world.faction(entity) else {
            return false;
        };
        self.world
            .faction(candidate)
            .is_some_and(|theirs| mine.is_hostile_to(theirs))
    }

    /// A random hostile within `range`, not already in `taken`, preferring anyone who is not
    /// `exclude`.
    ///
    /// Two passes rather than a weighted pick: the tank is chosen only when there is nobody
    /// else to choose, which is the whole promise of an ability that reaches past the tank.
    fn random_hostile(
        &mut self,
        entity: EntityId,
        range: (f32, f32),
        exclude: Option<EntityId>,
        taken: &[Option<EntityId>; MAX_TARGETS],
    ) -> Option<EntityId> {
        for allow_excluded in [false, true] {
            let mut count = 0_usize;
            for candidate in self.world.entities() {
                if self.eligible(entity, candidate, range, exclude, taken, allow_excluded) {
                    count += 1;
                }
            }
            if count == 0 {
                continue;
            }
            let pick = ((self.enemy_random() * count as f32) as usize).min(count - 1);
            let chosen = self
                .world
                .entities()
                .filter(|candidate| {
                    self.eligible(entity, *candidate, range, exclude, taken, allow_excluded)
                })
                .nth(pick);
            if chosen.is_some() {
                return chosen;
            }
        }
        None
    }

    /// Whether `candidate` may be picked by [`Simulation::random_hostile`].
    fn eligible(
        &self,
        entity: EntityId,
        candidate: EntityId,
        range: (f32, f32),
        exclude: Option<EntityId>,
        taken: &[Option<EntityId>; MAX_TARGETS],
        allow_excluded: bool,
    ) -> bool {
        if taken.contains(&Some(candidate)) {
            return false;
        }
        if !allow_excluded && Some(candidate) == exclude {
            return false;
        }
        if !self.is_hostile_combatant(entity, candidate) {
            return false;
        }
        // Between the ability's minimum and maximum: a leap is never aimed at somebody
        // already standing in the monster's reach.
        self.describe_target(entity, candidate)
            .is_some_and(|described| described.distance >= range.0 && described.distance <= range.1)
    }

    /// Fills the empty slots of `targets` with the nearest other hostiles in `range`.
    fn nearest_hostiles_into(
        &self,
        entity: EntityId,
        range: f32,
        targets: &mut [Option<EntityId>; MAX_TARGETS],
    ) {
        for slot in 1..MAX_TARGETS {
            let mut best: Option<(f32, EntityId)> = None;
            for candidate in self.world.entities() {
                if targets.contains(&Some(candidate))
                    || !self.is_hostile_combatant(entity, candidate)
                {
                    continue;
                }
                let Some(described) = self.describe_target(entity, candidate) else {
                    continue;
                };
                if described.distance > range {
                    continue;
                }
                if best.is_none_or(|(distance, _)| described.distance < distance) {
                    best = Some((described.distance, candidate));
                }
            }
            match best {
                Some((_, candidate)) => targets[slot] = Some(candidate),
                None => break,
            }
        }
    }

    /// The hostile furthest from `entity` it could still reach with anything.
    fn farthest_hostile(&self, entity: EntityId, range: f32) -> Option<AiTarget> {
        let mut best: Option<AiTarget> = None;
        for candidate in self.world.entities() {
            if !self.is_hostile_combatant(entity, candidate) {
                continue;
            }
            let Some(described) = self.describe_target(entity, candidate) else {
                continue;
            };
            if described.distance > range {
                continue;
            }
            if best.is_none_or(|current| described.distance > current.distance) {
                best = Some(described);
            }
        }
        best
    }

    /// The most wounded living ally of `entity` within `range`, itself included.
    ///
    /// Linear over the world, like every other targeting query in this crate. A pack is a
    /// handful of entities and a map keyed by faction would cost more to maintain than the
    /// scan costs to run.
    fn most_wounded_ally(&self, entity: EntityId, range: f32) -> Option<AiTarget> {
        let faction = self.world.faction(entity)?;
        let mut best: Option<(f32, AiTarget)> = None;
        for candidate in self.world.entities() {
            if !self.world.is_alive(candidate) {
                continue;
            }
            if self.world.faction(candidate) != Some(faction) {
                continue;
            }
            let Some(health) = self.world.health(candidate) else {
                continue;
            };
            let fraction = health.fraction();
            if fraction >= 1.0 {
                continue;
            }
            let Some(described) = self.describe_target(entity, candidate) else {
                continue;
            };
            if described.distance > range {
                continue;
            }
            // Ties break towards the lower entity slot, which `<` gives for free: the scan
            // is in slot order, so the same pack in the same state always heals the same
            // target.
            if best.is_none_or(|(best_fraction, _)| fraction < best_fraction) {
                best = Some((fraction, described));
            }
        }
        best.map(|(_, target)| target)
    }

    /// What one enemy can see this tick.
    fn perceive(&mut self, entity: EntityId, brain: &EnemyBrain) -> AiPercept {
        let position = self.world.position(entity).unwrap_or(Vec3::ZERO);
        let alive = self.world.is_alive(entity);
        let health_fraction = self
            .world
            .health(entity)
            .map_or(0.0, crate::combat::Health::fraction);
        // Only asked for by an archetype that can actually heal. The scan is cheap, but it is
        // a scan per enemy per tick, and most of the roster has no use for the answer.
        let heal_range = brain
            .abilities()
            .filter(|ability| ability.definition().effect.is_healing())
            .map(|ability| ability.definition().range)
            .fold(0.0_f32, f32::max);
        let wounded_ally = (heal_range > 0.0)
            .then(|| self.most_wounded_ally(entity, heal_range))
            .flatten();
        // Likewise the furthest hostile: only a monster with a charge ready asks.
        let charge_range = brain
            .abilities()
            .filter(|ability| {
                ability.definition().select == EnemyTarget::Farthest
                    && brain.cooldown(*ability) <= 0.0
            })
            .map(|ability| ability.definition().range)
            .fold(0.0_f32, f32::max);
        let farthest = (charge_range > 0.0 && brain.state().is_combat())
            .then(|| self.farthest_hostile(entity, charge_range))
            .flatten();

        // The target is the highest threat that is still a legal victim. "Legal" is asked of
        // the world here rather than inside the threat table, which is what keeps the table
        // free of any dependency on the world and testable with a closure.
        let world = &self.world;
        let faction = world.faction(entity);
        let target = brain.threat().highest(|candidate| {
            world.is_alive(candidate)
                && faction.is_some_and(|mine| {
                    world
                        .faction(candidate)
                        .is_some_and(|theirs| mine.is_hostile_to(theirs))
                })
        });

        let describe = |candidate: EntityId| -> Option<AiTarget> {
            let target_position = self.world.position(candidate)?;
            let gap = horizontal_distance(position, target_position)
                - self.world.radius(candidate)
                - self.world.radius(entity);
            Some(AiTarget {
                entity: candidate,
                position: target_position,
                distance: gap.max(0.0),
            })
        };

        // The nearest hostile is a separate question from the angriest: it is what pulls an
        // idle enemy that nobody has attacked yet.
        let nearest = self
            .targets
            .nearest_hostile(&self.world, entity)
            .and_then(describe);
        let target = target.and_then(describe);
        let victim = target.or(nearest);
        let victim_bleeding = victim.is_some_and(|victim| {
            self.world
                .statuses(victim.entity)
                .is_some_and(|statuses| statuses.has(StatusKind::Rended))
        });

        AiPercept {
            position,
            alive,
            target,
            nearest,
            health_fraction,
            wounded_ally,
            farthest,
            victim_bleeding,
        }
    }

    /// Uses an instant enemy ability, or resolves a single-target one on its recipient.
    fn execute_enemy_ability(
        &mut self,
        source: EntityId,
        ability: EnemyAbility,
        target: Option<AiTarget>,
        anchor: Vec3,
        scale: f32,
    ) {
        let definition = ability.definition();
        // A heal is resolved on whoever it selected and never through the damage path: the
        // shape of a heal is "one ally", and running it through `resolve_zone` would collect
        // everything hostile to the caster, which is precisely the wrong list.
        if let EnemyEffect::Heal { amount } = definition.effect {
            let Some(target) = target else {
                return;
            };
            if target.distance > definition.range {
                return;
            }
            self.heal_entity(
                source,
                target.entity,
                amount,
                AbilityId::from_enemy(ability),
            );
            return;
        }

        match definition.shape {
            TelegraphShape::Single => {
                self.events.push(CombatEvent::AbilityUsed {
                    source,
                    ability: AbilityId::from_enemy(ability),
                    targets: 1,
                    facing: self.body_yaw(source),
                    tick: self.tick,
                });
                let Some(target) = target else {
                    return;
                };
                // Re-checked at the moment it lands, not when it was chosen: a player who
                // walked out of range between the decision and the swing is out of range. A
                // cast gets a little slack, because a caster that finished a two-second
                // arrow does not miss a player who took one step back.
                let slack = if definition.is_cast() { 2.0 } else { 0.0 };
                if target.distance > definition.range + slack {
                    return;
                }
                self.strike_one(source, ability, target.entity, definition.damage * scale);
            }
            _ => {
                let shape = instant_shape(ability, source, self);
                self.resolve_zone(source, ability, anchor, shape, anchor, scale);
            }
        }
    }

    /// Resolves one zone of an area ability: the pool it leaves, then everyone it catches.
    ///
    /// The shape it hits with is the shape the telegraph announced — the same value, passed
    /// through. That is the whole guarantee behind "the damage area matches the telegraph".
    fn resolve_zone(
        &mut self,
        source: EntityId,
        ability: EnemyAbility,
        anchor: Vec3,
        shape: TelegraphShape,
        aim: Vec3,
        scale: f32,
    ) {
        let definition = ability.definition();
        // Burning ground is lit before the impact is resolved, so the pool and the hit that
        // created it are one event pair rather than two things that happened to coincide.
        if let EnemyEffect::Hazard {
            duration,
            damage_per_second,
            grow_to,
            radius,
        } = definition.effect
        {
            // A line leaves its pool where it was aimed; everything else covers its own shape.
            let (center, pool) = match shape {
                TelegraphShape::Line { .. } => (aim, radius.max(1.0)),
                _ if radius > 0.0 => (anchor, radius),
                _ => (anchor, shape.reach()),
            };
            let id = self.light_hazard(
                source,
                ability,
                center,
                pool,
                duration,
                damage_per_second * scale,
            );
            if grow_to > pool {
                self.hazards.set_growth(id, grow_to);
            }
        }
        if shape.is_area() {
            self.hit_shape(source, ability, anchor, shape, definition.damage * scale);
        }
    }

    /// Deals `amount` to every hostile inside `shape`, and remembers who that was.
    fn hit_shape(
        &mut self,
        source: EntityId,
        ability: EnemyAbility,
        anchor: Vec3,
        shape: TelegraphShape,
        amount: f32,
    ) {
        self.hit_shape_except(source, ability, anchor, shape, amount, None);
    }

    /// [`Simulation::hit_shape`], leaving out one entity that was already hit.
    fn hit_shape_except(
        &mut self,
        source: EntityId,
        ability: EnemyAbility,
        anchor: Vec3,
        shape: TelegraphShape,
        amount: f32,
        except: Option<EntityId>,
    ) {
        self.hits.clear();
        let mut hits = std::mem::take(&mut self.hits);
        if self.world.faction(source).is_some() {
            for candidate in self.world.entities() {
                if Some(candidate) == except || !self.is_hostile_combatant(source, candidate) {
                    continue;
                }
                let Some(position) = self.world.position(candidate) else {
                    continue;
                };
                if shape.contains(anchor, position, self.world.radius(candidate)) {
                    hits.push(candidate);
                }
            }
        }

        self.events.push(CombatEvent::AbilityUsed {
            source,
            ability: AbilityId::from_enemy(ability),
            targets: hits.len() as u32,
            // The shape's own yaw, which for a cast was decided when the cast started: the
            // attack is announced pointing where it actually swept.
            facing: match shape {
                TelegraphShape::Cone { yaw, .. } | TelegraphShape::Line { yaw, .. } => yaw,
                TelegraphShape::Circle { .. } | TelegraphShape::Single => self.body_yaw(source),
            },
            tick: self.tick,
        });
        self.hits_last.clear();
        self.hits_last.extend(hits.iter().copied());
        for victim in &hits {
            self.strike_one(source, ability, *victim, amount);
        }
        hits.clear();
        self.hits = hits;
    }

    /// One hit on one victim: the damage, then whatever it leaves behind.
    fn strike_one(
        &mut self,
        source: EntityId,
        ability: EnemyAbility,
        victim: EntityId,
        amount: f32,
    ) {
        let definition = ability.definition();
        self.deal_damage(
            source,
            victim,
            amount,
            AbilityId::from_enemy(ability),
            definition.damage_type,
            false,
        );
        if !self.world.is_alive(victim) {
            return;
        }
        match definition.rider {
            EnemyRider::None => {}
            EnemyRider::Bleed {
                damage_per_stack,
                duration,
                stacks,
                max_stacks,
            } => {
                let scale = amount / definition.damage.max(1.0);
                let status = Status::new(
                    StatusKind::Rended,
                    source,
                    AbilityId::from_enemy(ability),
                    duration,
                    damage_per_stack * scale,
                )
                .stacking_to(max_stacks)
                .with_stacks(stacks);
                self.apply_status(victim, status, duration);
            }
            EnemyRider::Slow { fraction, duration } => {
                let status = Status::new(
                    StatusKind::Crippled,
                    source,
                    AbilityId::from_enemy(ability),
                    duration,
                    0.0,
                )
                .with_magnitude(fraction);
                self.apply_status(victim, status, duration);
            }
        }
    }

    /// Announces an ability an enemy used on itself: a frenzy.
    fn announce_self_ability(&mut self, source: EntityId, ability: EnemyAbility) {
        self.events.push(CombatEvent::AbilityUsed {
            source,
            ability: AbilityId::from_enemy(ability),
            targets: 0,
            facing: self.body_yaw(source),
            tick: self.tick,
        });
    }

    /// Which way an entity is pointing, or zero if it has no body.
    fn body_yaw(&self, entity: EntityId) -> f32 {
        self.world.body(entity).map_or(0.0, |body| body.facing_yaw)
    }

    /// Restores health to one entity and announces it.
    ///
    /// The mirror of [`Simulation::deal_damage`]: the one place an enemy ability puts health
    /// back, so "every health change is observable" holds in both directions.
    fn heal_entity(&mut self, source: EntityId, target: EntityId, amount: f32, ability: AbilityId) {
        if !self.world.is_alive(target) {
            return;
        }
        let restored = self.world.heal(target, amount).applied;
        self.events.push(CombatEvent::AbilityUsed {
            source,
            ability,
            targets: 1,
            facing: self.body_yaw(source),
            tick: self.tick,
        });
        if restored <= 0.0 {
            return;
        }
        self.events.push(CombatEvent::Healing {
            source,
            target,
            amount: restored,
            ability,
            tick: self.tick,
        });
    }

    /// Lights burning ground, announces it as an area effect, and returns its id.
    fn light_hazard(
        &mut self,
        source: EntityId,
        ability: EnemyAbility,
        anchor: Vec3,
        radius: f32,
        duration: f32,
        damage_per_second: f32,
    ) -> u32 {
        let mut expired = std::mem::take(&mut self.hazard_expired);
        let id = self.hazards.spawn(
            source,
            ability,
            anchor,
            radius,
            duration,
            damage_per_second,
            &mut expired,
        );
        self.hazard_expired = expired;
        self.flush_expired_hazards();
        // The gameplay radius, carried as the shape a visual is built from: what burns and
        // what is drawn are one number.
        self.events.push(CombatEvent::AreaEffectStarted {
            id,
            source,
            ability: AbilityId::from_enemy(ability),
            position: anchor,
            shape: crate::firemage::zone::ZoneShape::Circle { radius },
            duration,
            tick: self.tick,
        });
        id
    }

    /// Announces every hazard that has stopped burning since the last flush.
    fn flush_expired_hazards(&mut self) {
        let tick = self.tick;
        let mut expired = std::mem::take(&mut self.hazard_expired);
        for (id, position) in expired.drain(..) {
            self.events
                .push(CombatEvent::AreaEffectEnded { id, position, tick });
        }
        self.hazard_expired = expired;
    }

    /// Advances every hazard and applies what it burned.
    ///
    /// Run once a tick from the simulation's own step, after the AI has had its turn, so a
    /// pool lit this tick does not also tick this tick.
    pub(super) fn tick_enemy_hazards(&mut self, step: f32) {
        // A spreading pool grows first, and is redrawn at its new size: the end of the old
        // area and the start of the new one, under the same id, so the effect that draws it
        // is replaced rather than duplicated.
        let mut grown = std::mem::take(&mut self.hazard_grown);
        grown.clear();
        self.hazards.grow(step, &mut grown);
        for hazard in &grown {
            let tick = self.tick;
            self.events.push(CombatEvent::AreaEffectEnded {
                id: hazard.id,
                position: hazard.center,
                tick,
            });
            self.events.push(CombatEvent::AreaEffectStarted {
                id: hazard.id,
                source: hazard.owner,
                ability: AbilityId::from_enemy(hazard.ability),
                position: hazard.center,
                shape: crate::firemage::zone::ZoneShape::Circle {
                    radius: hazard.radius,
                },
                duration: hazard.remaining,
                tick,
            });
        }
        grown.clear();
        self.hazard_grown = grown;

        let mut ticks = std::mem::take(&mut self.hazard_ticks);
        let mut expired = std::mem::take(&mut self.hazard_expired);
        ticks.clear();
        self.hazards.tick(step, &mut ticks, &mut expired);
        self.hazard_expired = expired;

        for hazard in &ticks {
            if self.world.faction(hazard.owner).is_none() {
                continue;
            }
            let ability = AbilityId::from_enemy(hazard.ability);
            // Collected first: dealing damage can kill, and killing frees a slot the
            // iteration is standing on.
            self.hits.clear();
            let mut hits = std::mem::take(&mut self.hits);
            for candidate in self.world.entities() {
                if !self.is_hostile_combatant(hazard.owner, candidate) {
                    continue;
                }
                let Some(position) = self.world.position(candidate) else {
                    continue;
                };
                let inside = {
                    let (dx, dz) = (position.x - hazard.center.x, position.z - hazard.center.z);
                    (dx * dx + dz * dz).sqrt() - self.world.radius(candidate) <= hazard.radius
                };
                if inside {
                    hits.push(candidate);
                }
            }
            for victim in &hits {
                self.deal_damage(
                    hazard.owner,
                    *victim,
                    hazard.damage,
                    ability,
                    hazard.ability.definition().damage_type,
                    false,
                );
            }
            hits.clear();
            self.hits = hits;
        }

        ticks.clear();
        self.hazard_ticks = ticks;
        self.flush_expired_hazards();
    }

    /// Puts out everything an entity lit, and announces every removal.
    pub(super) fn clear_hazards_of(&mut self, owner: EntityId) {
        let mut expired = std::mem::take(&mut self.hazard_expired);
        self.hazards.clear_owner(owner, &mut expired);
        self.hazard_expired = expired;
        self.flush_expired_hazards();
    }

    /// Puts every hazard out. What a reset runs.
    pub(super) fn clear_all_hazards(&mut self) {
        let mut expired = std::mem::take(&mut self.hazard_expired);
        self.hazards.clear_into(&mut expired);
        self.hazard_expired = expired;
        self.flush_expired_hazards();
    }

    /// Announces a phase transition.
    fn announce_phase(&mut self, entity: EntityId, phase: usize, name: &'static str) {
        log::info!("{} entered {name}", entity.index());
        self.events.push(CombatEvent::PhaseChanged {
            entity,
            phase,
            name,
            tick: self.tick,
        });
    }

    /// Announces that an enemy started casting.
    fn announce_cast_start(&mut self, source: EntityId, ability: EnemyAbility, duration: f32) {
        self.events.push(CombatEvent::CastStarted {
            source,
            ability: AbilityId::from_enemy(ability),
            duration,
            tick: self.tick,
        });
    }

    /// Announces that an enemy's cast ended, however it ended.
    fn announce_cast_end(&mut self, source: EntityId, ability: EnemyAbility, end: CastEnd) {
        self.events.push(CombatEvent::CastEnded {
            source,
            ability: AbilityId::from_enemy(ability),
            end,
            tick: self.tick,
        });
    }

    /// Announces a telegraph going up.
    #[allow(
        clippy::too_many_arguments,
        reason = "a telegraph is this many facts, and the event it becomes names every one"
    )]
    fn announce_telegraph(
        &mut self,
        id: u32,
        source: EntityId,
        ability: EnemyAbility,
        anchor: Vec3,
        shape: TelegraphShape,
        delay: f32,
        follow: Option<EntityId>,
    ) {
        self.events.push(CombatEvent::TelegraphStarted {
            id,
            source,
            ability: AbilityId::from_enemy(ability),
            anchor,
            shape,
            delay,
            follow,
            tick: self.tick,
        });
    }

    /// Announces a telegraph coming down.
    fn announce_telegraph_end(&mut self, id: u32, anchor: Vec3, resolved: bool) {
        self.events.push(CombatEvent::TelegraphEnded {
            id,
            anchor,
            resolved,
            tick: self.tick,
        });
    }

    /// Takes down `count` telegraphs starting at `first`.
    pub(super) fn end_telegraphs(&mut self, first: u32, count: u8, anchor: Vec3, resolved: bool) {
        for offset in 0..u32::from(count) {
            self.announce_telegraph_end(first.wrapping_add(offset), anchor, resolved);
        }
    }

    /// The next telegraph identity.
    fn next_telegraph_id(&mut self) -> u32 {
        let id = self.next_telegraph;
        self.next_telegraph = self.next_telegraph.wrapping_add(1);
        id
    }

    /// The next number in `[0, 1)` from the enemies' own generator.
    ///
    /// Seeded from the simulation's configuration, so a replay of the same seed marks the same
    /// player, scatters the same barrage and calls the same adds. Xorshift: a few instructions
    /// and no allocation, which is all a target pick needs.
    pub(super) fn enemy_random(&mut self) -> f32 {
        let mut state = self.enemy_rng;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        self.enemy_rng = state;
        (state >> 40) as f32 / (1_u64 << 24) as f32
    }

    /// Reseeds the enemies' generator: which player a mark finds, how a barrage scatters and
    /// which adds a boss calls.
    ///
    /// For a test that wants to see many rolls, and for a developer who wants a different
    /// pair of adds without rebuilding the level.
    pub fn reseed_enemies(&mut self, seed: u64) {
        self.enemy_rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    }

    /// Interrupts an enemy's cast, if it has one and it can be interrupted.
    ///
    /// Returns whether anything was stopped. This is the whole interrupt foundation: a
    /// player ability that wants to interrupt calls it, and the enemy loses its cast, its
    /// telegraph and a chunk of time.
    pub fn interrupt(&mut self, entity: EntityId) -> bool {
        let slot = entity.index();
        let Some(Some(runtime)) = self.enemies.get_mut(slot) else {
            return false;
        };
        if !runtime.brain.is_interruptible() {
            return false;
        }
        let Some(cast) = runtime.brain.stop_cast(CastStop::Interrupted) else {
            return false;
        };
        let telegraphs = runtime.telegraphs.take();
        if let Some((first, count)) = telegraphs {
            self.end_telegraphs(first, count, cast.anchor, false);
        }
        self.announce_cast_end(entity, cast.ability, CastEnd::Interrupted);
        true
    }

    /// Whether an entity is an enemy with an interruptible cast running.
    #[must_use]
    pub fn is_interruptible(&self, entity: EntityId) -> bool {
        self.enemies
            .get(entity.index())
            .and_then(|slot| slot.as_ref())
            .is_some_and(|runtime| runtime.brain.is_interruptible())
    }

    /// Whether an enemy is charging right now.
    #[must_use]
    pub fn is_charging(&self, entity: EntityId) -> bool {
        self.enemies
            .get(entity.index())
            .and_then(|slot| slot.as_ref())
            .is_some_and(|runtime| runtime.dash.is_some())
    }

    /// The delayed zones waiting to land: marks, rains, quakes, shockwaves.
    #[must_use]
    pub fn enemy_strikes(&self) -> &crate::enemy::strike::StrikeField {
        &self.strikes
    }

    /// Keeps every encounter's state in step with how many of its members are alive.
    ///
    /// Taken out of `self` and put back so the loop can ask the world questions. A dungeon's
    /// encounters are cheap passes over a handful of entity ids each.
    fn sync_encounter(&mut self) {
        let mut encounters = std::mem::take(&mut self.encounters);
        for encounter in &mut encounters {
            // Who is awake, and what they are fighting. A pack is pulled *by* something, and
            // the rest of it has to be told what.
            let mut awake = false;
            let mut puller = None;
            for member in encounter.members() {
                let Some(Some(runtime)) = self.enemies.get(member.index()) else {
                    continue;
                };
                // A corpse is not awake and cannot pull anybody. The last member of a pack
                // dying must leave the pack at rest, not permanently pulled.
                if runtime.brain.state() == AiState::Idle || runtime.brain.state() == AiState::Dead
                {
                    continue;
                }
                awake = true;
                puller = puller.or_else(|| runtime.brain.target());
            }
            // A member killed outright never left Idle, so nobody in the pack is awake and
            // there is no live target to read a puller off. Whoever landed the killing blow
            // is that puller: a pack that slept through one of its own being deleted could be
            // taken apart one member at a time.
            let puller = puller.or_else(|| encounter.alarm());
            // One member pulled is the whole pack pulled, once. Done here rather than in the
            // brain because membership is the encounter's: an enemy knows its own aggro
            // radius and its own leash, and has no idea it is the second of five.
            let pull = puller
                .filter(|_| !encounter.pack_pulled())
                .filter(|target| self.world.is_alive(*target));

            if awake {
                encounter.activate();
            } else if pull.is_none() {
                // Everybody is home, or everybody is dead. Either way the pack can be pulled
                // as a whole again if anything is left to pull — and an alarm nobody is left
                // to answer goes quiet with it.
                encounter.relax();
            }

            if let Some(target) = pull {
                encounter.note_pack_pull();
                encounter.activate();
                for member in encounter.members() {
                    // A corpse is not provoked into anything. Its brain has not necessarily
                    // been told it is dead yet — that happens on its next think — and waking
                    // it would make the pack look awake for a tick.
                    if !self.world.is_alive(*member) {
                        continue;
                    }
                    if let Some(Some(runtime)) = self.enemies.get_mut(member.index()) {
                        runtime.brain.provoke(target);
                    }
                }
            }
            let alive = encounter
                .members()
                .iter()
                .filter(|member| self.world.is_alive(**member))
                .count();
            if encounter.observe_alive(alive) {
                log::info!("encounter '{}' completed", encounter.name());
            }
        }
        self.encounters = encounters;
    }
}

/// Who an ability is used on, given what its caster can see.
///
/// The same question [`crate::enemy::brain::EnemyBrain`] asked when it chose the ability, so
/// what the brain aimed at and what combat resolves cannot be two different things.
fn recipient_of(ability: EnemyAbility, percept: &AiPercept) -> Option<AiTarget> {
    match ability.definition().select {
        EnemyTarget::Victim | EnemyTarget::Random => percept.victim(),
        EnemyTarget::WoundedAlly => percept.wounded_ally,
        EnemyTarget::Farthest => percept.farthest.or_else(|| percept.victim()),
    }
}

/// Where an instant ability is anchored.
fn cast_anchor(
    ability: EnemyAbility,
    source: EntityId,
    target: Option<AiTarget>,
    simulation: &Simulation,
) -> Vec3 {
    match ability.definition().aim {
        EnemyAim::Caster => simulation.world().position(source).unwrap_or(Vec3::ZERO),
        EnemyAim::Target => target
            .map(|target| target.position)
            .or_else(|| simulation.world().position(source))
            .unwrap_or(Vec3::ZERO),
    }
}

/// The shape an *instant* ability hits with, aimed the way its caster faces now.
///
/// Casts do not use this: theirs was resolved when the cast started and travels on the cast.
fn instant_shape(
    ability: EnemyAbility,
    source: EntityId,
    simulation: &Simulation,
) -> TelegraphShape {
    let yaw = simulation
        .world()
        .body(source)
        .map_or(0.0, |body| body.facing_yaw);
    aimed_shape(ability.definition().shape, yaw)
}
