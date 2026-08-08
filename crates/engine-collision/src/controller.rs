//! The character controller: a capsule that walks, slides, climbs steps and falls.
//!
//! ```text
//! displacement (metres this tick)
//!        |
//!        +-- horizontal --> sweep in substeps --> depenetrate --> slide
//!        |                        |
//!        |                        +-- blocked and grounded? --> step up, retry, drop
//!        |
//!        +-- vertical ----> sweep in substeps --> depenetrate --> land / hit a ceiling
//!        |
//!        +-- still grounded? --> no: probe down and snap, so a downhill walk is a walk
//!                                    and not a series of small falls
//! ```
//!
//! ## What this is not
//!
//! Not a rigid body. There is no mass, no inertia tensor, no restitution and no solver. The
//! character is moved, not simulated: gameplay says "go this far this tick" and this module
//! answers "here is how far you actually got, and what you are standing on". That is the
//! whole contract, and it is what keeps movement predictable enough to build combat on.
//!
//! ## Determinism
//!
//! Every loop here has a fixed iteration bound, every buffer is supplied by the caller, and
//! nothing reads a clock or a random number. The same inputs produce bit-identical outputs,
//! which is what `tests/determinism.rs` asserts and what a replay or a future dedicated
//! server would depend on.
//!
//! ## Tunnelling
//!
//! Movement is advanced in substeps no longer than the capsule's radius, so a capsule can
//! never cross a wall between two overlap tests: after any substep, a wall it has passed is
//! still within a radius of its spine and is therefore still a contact. This is conservative
//! advancement rather than an exact swept test, and it is why a 200 m/s character does not
//! pass through a 5 cm barrier.

use engine_core::math::Vec3;

use crate::contact::{Contact, QueryStats, StaticContact};
use crate::layer::CollisionMask;
use crate::shape::{Capsule, Segment};
use crate::world::{CollisionWorld, QueryBuffers, StaticId};

/// Below this a displacement is not a movement.
const EPSILON: f32 = 1e-6;

/// How the character controller behaves. Tuning, not state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterConfig {
    /// Capsule radius, in metres.
    ///
    /// The **collision** radius. It has nothing to do with the character's mesh and must not
    /// be derived from one: a model with a cape is not wider to walk with.
    pub radius: f32,
    /// Total capsule height, feet to crown, in metres.
    pub height: f32,
    /// The tallest obstacle the character walks over without jumping, in metres.
    pub max_step_height: f32,
    /// The steepest surface the character can stand on, in radians from horizontal.
    pub max_slope_angle: f32,
    /// How far outside a surface the character is kept, in metres.
    ///
    /// Resolution pushes out by the penetration *plus* this. Without it a character resting
    /// against a wall alternates between touching and one ulp inside it, which shows up as
    /// a shudder along the wall.
    pub skin_width: f32,
    /// How far below the feet the controller looks for ground it has just walked off.
    ///
    /// This is what makes walking down a ramp a walk instead of a sequence of small falls.
    /// Too large and the character is glued to the floor over ledges; too small and it
    /// leaves the ground on every slope.
    pub ground_snap_distance: f32,
    /// How many push-out iterations one resolution is allowed.
    ///
    /// Bounded rather than "until clear": an unbounded loop in a corner between three
    /// surfaces is a frame that never ends.
    pub max_resolution_iterations: u32,
    /// The longest a single movement substep may be, in metres.
    ///
    /// Zero derives it from the radius, which is what callers should use — see the module
    /// documentation on tunnelling.
    pub max_substep_distance: f32,
    /// What the character collides with.
    pub mask: CollisionMask,
    /// Whether step-up is attempted at all.
    pub step_up_enabled: bool,
}

impl Default for CharacterConfig {
    fn default() -> Self {
        Self {
            // A 0.4 m radius and 1.8 m height is a person: wide enough not to slip through
            // a door frame's collision, narrow enough to walk down a corridor.
            radius: 0.4,
            height: 1.8,
            // A curb, a low platform edge and one stair tread. Above this a jump is the
            // answer, which is what makes jumping worth having.
            max_step_height: 0.45,
            // 50 degrees: comfortably above the 45-degree ramp a level author will build and
            // comfortably below a wall.
            max_slope_angle: 50.0_f32.to_radians(),
            skin_width: 0.005,
            ground_snap_distance: 0.30,
            max_resolution_iterations: 6,
            max_substep_distance: 0.0,
            mask: CollisionMask::WORLD,
            step_up_enabled: true,
        }
    }
}

impl CharacterConfig {
    /// The capsule this character is, standing with its feet at `feet`.
    #[must_use]
    pub fn capsule_at(&self, feet: Vec3) -> Capsule {
        Capsule::upright(feet, self.radius, self.height)
    }

    /// The smallest `normal · up` that still counts as ground.
    #[must_use]
    pub fn ground_threshold(&self) -> f32 {
        self.max_slope_angle
            .clamp(0.0, std::f32::consts::FRAC_PI_2)
            .cos()
    }

    /// Whether a surface with this normal can be stood on.
    #[must_use]
    pub fn is_walkable(&self, normal: Vec3) -> bool {
        normal.dot(Vec3::Y) >= self.ground_threshold() - 1e-4
    }

    /// The longest substep this configuration allows.
    #[must_use]
    pub fn substep_distance(&self) -> f32 {
        if self.max_substep_distance > 0.0 {
            self.max_substep_distance
        } else {
            // Half a radius. A full radius is the theoretical bound for never skipping a
            // surface; half of it leaves room for the skin width and for a wall the capsule
            // approaches at a glancing angle.
            (self.radius * 0.5).max(0.01)
        }
    }
}

/// Where the character is and what it is doing.
///
/// Owned by the simulation, not by this module: the controller is a function over this
/// state, which is what makes it testable without a game and replayable with one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterState {
    /// Position of the **feet** in world space.
    pub position: Vec3,
    /// Velocity in world space, metres per second.
    pub velocity: Vec3,
    /// Whether the character is standing on something walkable.
    pub grounded: bool,
    /// The normal of what it is standing on. `+Y` when airborne.
    pub ground_normal: Vec3,
    /// Which static object it is standing on, if any.
    pub ground_id: Option<StaticId>,
}

impl Default for CharacterState {
    fn default() -> Self {
        Self::at(Vec3::ZERO)
    }
}

impl CharacterState {
    /// A character standing still at `position`.
    #[must_use]
    pub fn at(position: Vec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            grounded: false,
            ground_normal: Vec3::Y,
            ground_id: None,
        }
    }

    /// The angle of the surface under the character, in radians. Zero when airborne.
    #[must_use]
    pub fn slope_angle(&self) -> f32 {
        if !self.grounded {
            return 0.0;
        }
        self.ground_normal.dot(Vec3::Y).clamp(-1.0, 1.0).acos()
    }

    /// Horizontal speed, ignoring rise and fall.
    #[must_use]
    pub fn horizontal_speed(&self) -> f32 {
        self.velocity.x.hypot(self.velocity.z)
    }
}

/// What one move did.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct MoveResult {
    /// How far the character actually moved.
    pub moved: Vec3,
    /// Whether it ended the move on walkable ground.
    pub grounded: bool,
    /// What it ended the move standing on, if anything.
    ///
    /// Carried out of the move so a developer readout can name the surface without running
    /// the query a second time and possibly getting a different answer.
    pub ground: Option<StaticId>,
    /// Whether it touched something too steep to stand on.
    pub hit_wall: bool,
    /// Whether it touched something above it.
    pub hit_ceiling: bool,
    /// How far a step-up lifted it, in metres. Zero when no step was taken.
    pub stepped_up: f32,
    /// Whether the downward probe reattached it to the ground.
    pub snapped_to_ground: bool,
    /// Movement substeps taken.
    pub substeps: u32,
    /// Push-out iterations performed.
    pub iterations: u32,
    /// What the queries cost.
    pub stats: QueryStats,
}

/// Moves a character by `displacement`, resolving whatever is in the way.
///
/// `displacement` is metres for this step — the caller multiplies velocity by the fixed
/// timestep, because the controller has no opinion about how long a tick is.
///
/// The character's velocity is clipped against every surface it meets, so a run into a wall
/// stops rather than accumulating speed against it, and a diagonal run slides.
pub fn move_character(
    world: &CollisionWorld,
    config: &CharacterConfig,
    state: &mut CharacterState,
    displacement: Vec3,
    buffers: &mut QueryBuffers,
) -> MoveResult {
    let start = state.position;
    let was_grounded = state.grounded;
    let mut resolver = Resolver {
        world,
        config,
        buffers,
        result: MoveResult::default(),
        ground: None,
        walls: 0,
    };

    state.grounded = false;
    state.ground_id = None;

    // Anything already overlapping is pushed out before the move, not after: starting a
    // step inside a wall and then moving would resolve the move against geometry the
    // character should never have been inside.
    resolver.resolve_penetration(state);

    let horizontal = Vec3::new(displacement.x, 0.0, displacement.z);
    if horizontal.length_squared() > EPSILON {
        resolver.move_horizontally(state, horizontal, was_grounded);
    }

    let vertical = Vec3::new(0.0, displacement.y, 0.0);
    if vertical.length_squared() > EPSILON {
        resolver.sweep(state, vertical);
    }

    // Walking off a ramp must not launch the character. Only when it was already on the
    // ground and is not moving upwards: a jump has to be allowed to leave.
    if resolver.ground.is_none() && was_grounded && state.velocity.y <= 0.0 {
        resolver.snap_to_ground(state);
    }

    if let Some(ground) = resolver.ground {
        state.grounded = true;
        state.ground_normal = ground.contact.normal;
        state.ground_id = Some(ground.id);
    } else {
        state.ground_normal = Vec3::Y;
    }

    let mut result = resolver.result;
    result.moved = state.position - start;
    result.grounded = state.grounded;
    result.ground = state.ground_id;
    result
}

/// Pushes a character out of anything it is inside, without moving it otherwise.
///
/// For a teleport, a spawn or a debug reset: a character placed inside geometry should be
/// standing beside it on the next step rather than falling through the world.
pub fn resolve_overlaps(
    world: &CollisionWorld,
    config: &CharacterConfig,
    state: &mut CharacterState,
    buffers: &mut QueryBuffers,
) -> MoveResult {
    let start = state.position;
    let mut resolver = Resolver {
        world,
        config,
        buffers,
        result: MoveResult::default(),
        ground: None,
        walls: 0,
    };
    state.grounded = false;
    state.ground_id = None;
    resolver.resolve_penetration(state);

    if let Some(ground) = resolver.ground {
        state.grounded = true;
        state.ground_normal = ground.contact.normal;
        state.ground_id = Some(ground.id);
    } else {
        state.ground_normal = Vec3::Y;
    }

    let mut result = resolver.result;
    result.moved = state.position - start;
    result.grounded = state.grounded;
    result.ground = state.ground_id;
    result
}

/// The working state of one move.
///
/// A struct rather than a dozen `&mut` parameters, and it is what keeps the borrow of the
/// query buffers in one place.
struct Resolver<'a> {
    world: &'a CollisionWorld,
    config: &'a CharacterConfig,
    buffers: &'a mut QueryBuffers,
    result: MoveResult,
    /// The most level walkable contact seen during this move.
    ground: Option<StaticContact>,
    /// How many unwalkable contacts have been resolved so far this move.
    ///
    /// Counted rather than flagged, so a caller can tell whether a *particular* stretch of
    /// the move ran into a wall — which is what decides whether a step is worth trying.
    walls: u32,
}

impl Resolver<'_> {
    /// Moves horizontally, attempting a step-up if the move was blocked.
    fn move_horizontally(
        &mut self,
        state: &mut CharacterState,
        horizontal: Vec3,
        was_grounded: bool,
    ) {
        let before = *state;
        let ground_before = self.ground;
        let walls_before = self.walls;
        self.sweep(state, horizontal);

        if !self.config.step_up_enabled || !(was_grounded || state.grounded) {
            return;
        }

        // Only something unwalkable is worth stepping over. A move slowed by a ramp is not
        // blocked — it is walking uphill — and probing for a step every tick of a climb
        // would lift the character off the surface it is already walking on.
        if self.walls == walls_before {
            return;
        }

        // "Blocked" is measured along the direction asked for. A slide is not blocked: it
        // made progress, just not the progress requested.
        let wanted = horizontal.length();
        let achieved = (state.position - before.position).dot(horizontal / wanted);
        if achieved >= wanted - self.config.skin_width {
            return;
        }

        let slid = *state;
        let slid_ground = self.ground;
        *state = before;
        self.ground = ground_before;

        if !self.try_step_up(state, horizontal, before.position) {
            // The step failed; keep the ordinary slide, which is always a valid outcome.
            *state = slid;
            self.ground = slid_ground;
        }
    }

    /// Finds the surface just ahead, lifts onto it, and continues the move.
    ///
    /// ```text
    ///                probe: a small sphere, dropped from step height
    ///                   |
    ///                   v
    ///        (o)      .---.                      (o)
    ///         |       |   |        ==>            |   .---.
    ///     ####|#######|   |####            #######|###|   |####
    /// ```
    ///
    /// **Not** the classic lift-move-drop of the capsule itself. That technique comes from
    /// controllers whose shape is a box, and it is unsound for a capsule: a capsule put down
    /// against a step comes to rest on the step's *edge*, at whatever height it happens to
    /// be lifted to. Accepting that as a step means the character gains `max_step_height`
    /// against any obstacle at all and climbs a wall a step at a time, which is exactly the
    /// bug this replaced.
    ///
    /// So the height is measured rather than inferred: a small probe is dropped in front of
    /// the character from step height, and where *it* comes to rest is the surface. If the
    /// surface is within reach and walkable, the character is lifted exactly onto it.
    fn try_step_up(&mut self, state: &mut CharacterState, horizontal: Vec3, origin: Vec3) -> bool {
        let wanted = horizontal.length();
        if wanted <= EPSILON {
            return false;
        }
        let direction = horizontal / wanted;
        let Some(surface) = self.probe_step_surface(origin, direction) else {
            return false;
        };

        let rise = surface.height - origin.y;
        if rise <= self.config.skin_width
            || rise > self.config.max_step_height + self.config.skin_width
        {
            return false;
        }

        // The character has to fit where it is going. Checked with its real capsule, so a
        // step under a low ceiling is refused rather than pushing the character into it.
        let lifted = origin + Vec3::new(0.0, rise + self.config.skin_width, 0.0);
        self.result.stats.narrow_tests += 1;
        if self.world.capsule_overlaps(
            &self.config.capsule_at(lifted),
            self.config.mask,
            self.buffers,
        ) {
            return false;
        }

        state.position = lifted;
        // Velocity is left alone: a step is a correction to the path, not a change of
        // intent, and clipping it here would slow the character on every stair tread.
        self.sweep(state, horizontal);

        // The probe found walkable ground, so the character is on it even before it has
        // advanced far enough for its own capsule to touch the face proper. Without this a
        // character walking up stairs reports FALLING between treads and cannot jump.
        self.ground = Some(surface.contact);
        self.result.stepped_up = rise;
        true
    }

    /// Drops a small probe in front of the character and reports what it landed on.
    ///
    /// Returns `None` when there is nothing to step onto: no surface within reach, no room
    /// above it, or a surface too steep to stand on.
    fn probe_step_surface(&mut self, origin: Vec3, direction: Vec3) -> Option<StepSurface> {
        // Small, so it measures the surface rather than the character: a probe as wide as
        // the capsule would rest on the same edge the capsule does and measure nothing.
        let radius = (self.config.radius * 0.1).max(0.02);
        // Far enough ahead to be past the obstacle's face, close enough to be the step the
        // character is actually walking into.
        let ahead = origin
            + direction * (self.config.radius + radius + self.config.skin_width * 2.0)
            + Vec3::new(
                0.0,
                self.config.max_step_height + self.config.skin_width,
                0.0,
            );

        let mut probe = CharacterState::at(ahead);
        // Something already occupies the space a step's top would be in — a wall, or a
        // ceiling directly over the step. Either way there is nothing to step onto.
        self.result.stats.narrow_tests += 1;
        if self.world.capsule_overlaps(
            &probe_capsule(ahead, radius),
            self.config.mask,
            self.buffers,
        ) {
            return None;
        }

        if !self.descend_probe(
            &mut probe,
            radius,
            self.config.max_step_height + self.config.skin_width,
        ) {
            return None;
        }

        // What it came to rest on. Asked with a slightly fatter probe about the *same
        // centre*, because resting leaves a gap and a query at the resting position finds
        // nothing at all. Inflating by moving the feet would move the sphere up by as much
        // as it grew and find nothing either — which is what made a step at exactly the
        // limit report no surface.
        let centre = probe.position + Vec3::new(0.0, radius, 0.0);
        let inflated = Capsule::new(
            Segment::new(centre, centre),
            radius + self.config.skin_width * 4.0,
        );
        let stats = self
            .world
            .overlap_capsule(&inflated, self.config.mask, self.buffers);
        self.result.stats.accumulate(stats);

        let mut best: Option<StaticContact> = None;
        for candidate in self.buffers.contacts() {
            let better = best.is_none_or(|current: StaticContact| {
                candidate.contact.levelness() > current.contact.levelness()
            });
            if better {
                best = Some(*candidate);
            }
        }
        let contact = best?;
        if !self.config.is_walkable(contact.contact.normal) {
            return None;
        }
        Some(StepSurface {
            height: probe.position.y,
            contact,
        })
    }

    /// Lowers a probe of `radius` until something stops it. Returns whether anything did.
    fn descend_probe(&mut self, probe: &mut CharacterState, radius: f32, distance: f32) -> bool {
        let substep = (radius * 2.0).max(0.01);
        let mut travelled = 0.0;

        while travelled < distance {
            let drop = substep.min(distance - travelled);
            let candidate = probe.position - Vec3::new(0.0, drop, 0.0);
            self.result.stats.narrow_tests += 1;
            if !self.world.capsule_overlaps(
                &probe_capsule(candidate, radius),
                self.config.mask,
                self.buffers,
            ) {
                probe.position = candidate;
                travelled += drop;
                continue;
            }

            let mut clear = 0.0;
            let mut blocked = drop;
            for _ in 0..BISECTION_STEPS {
                let middle = (clear + blocked) * 0.5;
                let point = probe.position - Vec3::new(0.0, middle, 0.0);
                self.result.stats.narrow_tests += 1;
                if self.world.capsule_overlaps(
                    &probe_capsule(point, radius),
                    self.config.mask,
                    self.buffers,
                ) {
                    blocked = middle;
                } else {
                    clear = middle;
                }
            }
            probe.position -= Vec3::new(0.0, clear, 0.0);
            return true;
        }
        false
    }

    /// Looks for ground just below the feet and moves the character onto it.
    fn snap_to_ground(&mut self, state: &mut CharacterState) {
        let distance = self.config.ground_snap_distance;
        if distance <= 0.0 {
            return;
        }
        let before = *state;
        let ground_before = self.ground;

        self.sweep(state, Vec3::new(0.0, -distance, 0.0));
        if self.ground.is_some() {
            self.result.snapped_to_ground = true;
            return;
        }
        // Nothing under it: it really is falling, and the probe must not have moved it.
        *state = before;
        self.ground = ground_before;
    }

    /// Advances a displacement in substeps short enough that nothing can be crossed.
    fn sweep(&mut self, state: &mut CharacterState, displacement: Vec3) {
        let distance = displacement.length();
        if distance <= EPSILON {
            return;
        }

        let limit = self.config.substep_distance();
        // `ceil` of a ratio, bounded: a NaN or an absurd displacement must not produce a
        // loop count that hangs the tick.
        let substeps = ((distance / limit).ceil() as u32).clamp(1, MAX_SUBSTEPS);
        let step = displacement / substeps as f32;

        for _ in 0..substeps {
            self.result.substeps += 1;
            state.position += step;
            self.resolve_penetration(state);
        }
    }

    /// Pushes the character out of everything it overlaps, clipping velocity as it goes.
    ///
    /// The deepest contact is resolved first and the query is repeated, rather than every
    /// contact being applied at once: pushing out of two surfaces simultaneously
    /// double-counts the corner between them and launches the character out of it.
    fn resolve_penetration(&mut self, state: &mut CharacterState) {
        for _ in 0..self.config.max_resolution_iterations {
            let stats = self.world.overlap_capsule(
                &self.config.capsule_at(state.position),
                self.config.mask,
                self.buffers,
            );
            self.result.stats.accumulate(stats);
            self.result.iterations += 1;

            let Some(deepest) = deepest_contact(self.buffers.contacts()) else {
                return;
            };
            self.note(deepest);

            let push =
                deepest.contact.normal * (deepest.contact.penetration + self.config.skin_width);
            state.position += push;
            clip_velocity(&mut state.velocity, deepest.contact.normal);
        }
    }

    /// Records what a contact means: ground, wall or ceiling.
    fn note(&mut self, contact: StaticContact) {
        let levelness = contact.contact.levelness();
        if self.config.is_walkable(contact.contact.normal) {
            // The most level surface wins, so standing in the join between a ramp and a
            // floor reports the floor rather than whichever was tested first.
            let better = self
                .ground
                .is_none_or(|current| levelness > current.contact.levelness());
            if better {
                self.ground = Some(contact);
            }
        } else if levelness < -1e-3 {
            self.result.hit_ceiling = true;
            self.walls += 1;
        } else {
            self.result.hit_wall = true;
            self.walls += 1;
        }
    }
}

/// What a step probe found.
#[derive(Debug, Clone, Copy)]
struct StepSurface {
    /// World height of the surface just ahead.
    height: f32,
    /// The contact that established it.
    contact: StaticContact,
}

/// The probe shape: a small sphere, as a capsule with no spine.
///
/// Small on purpose. It measures the surface in front of the character, and a probe as wide
/// as the character would rest on the same edge the character does and measure nothing.
fn probe_capsule(feet: Vec3, radius: f32) -> Capsule {
    Capsule::upright(feet, radius, radius * 2.0)
}

/// Hard bound on substeps in one sweep.
///
/// Reached only by a displacement of hundreds of metres in one tick, which is a bug
/// elsewhere. Clamping keeps that bug a wrong position rather than a hung frame.
const MAX_SUBSTEPS: u32 = 256;

/// Halvings used to find where a descent comes to rest.
const BISECTION_STEPS: u32 = 6;

/// The deepest contact in a list, or `None` if it is empty.
///
/// Ties are broken by slot order, which is what keeps the choice deterministic when a
/// character stands exactly in a corner.
fn deepest_contact(contacts: &[StaticContact]) -> Option<StaticContact> {
    let mut best: Option<StaticContact> = None;
    for candidate in contacts {
        let deeper =
            best.is_none_or(|current| candidate.contact.penetration > current.contact.penetration);
        if deeper {
            best = Some(*candidate);
        }
    }
    best
}

/// Removes the component of `velocity` that points into a surface.
///
/// This is sliding, and it is the whole of it: what remains after the normal component is
/// removed is a vector along the surface.
///
/// ```text
///   velocity                  result
///      \                        |
///       \  normal               |
///        v <---#                v
///   ###########################
/// ```
pub fn clip_velocity(velocity: &mut Vec3, normal: Vec3) {
    let into = velocity.dot(normal);
    if into < 0.0 {
        *velocity -= normal * into;
    }
}

/// Everything a contact says about a surface, for a debug readout.
///
/// Not used to decide anything — it exists so a developer HUD can name what the character
/// is standing on without duplicating the rules that decided it.
#[must_use]
pub fn describe(config: &CharacterConfig, contact: &Contact) -> (f32, bool) {
    (contact.slope_angle(), config.is_walkable(contact.normal))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::shape::Obb;
    use crate::world::CollisionWorld;

    /// A flat world with an infinite floor.
    fn flat_world() -> CollisionWorld {
        let mut world = CollisionWorld::new();
        world.add_ground(0.0, "FLOOR");
        world.rebuild_broad_phase();
        world
    }

    fn config() -> CharacterConfig {
        CharacterConfig::default()
    }

    /// One 60 Hz tick of whatever velocity the character currently has.
    ///
    /// Written once here rather than at every call site, because `velocity * step` read
    /// inline is exactly the expression a borrow checker objects to and a reader skips.
    fn tick(
        world: &CollisionWorld,
        config: &CharacterConfig,
        state: &mut CharacterState,
        buffers: &mut QueryBuffers,
    ) -> MoveResult {
        let delta = state.velocity / 60.0;
        move_character(world, config, state, delta, buffers)
    }

    #[test]
    fn a_character_dropped_onto_the_floor_lands_on_it() {
        let world = flat_world();
        let config = config();
        let mut state = CharacterState::at(Vec3::new(0.0, 5.0, 0.0));
        let mut buffers = QueryBuffers::new();

        for _ in 0..200 {
            state.velocity.y -= 25.0 / 60.0;
            tick(&world, &config, &mut state, &mut buffers);
            if state.grounded {
                break;
            }
        }

        assert!(state.grounded, "it never landed");
        assert!(
            state.position.y.abs() < config.skin_width * 3.0,
            "it landed at {}",
            state.position.y
        );
        assert!(state.velocity.y.abs() < 1e-3, "the fall was not absorbed");
        assert!(state.ground_id.is_some());
    }

    #[test]
    fn a_character_never_passes_through_the_floor() {
        let world = flat_world();
        let config = config();
        let mut state = CharacterState::at(Vec3::new(0.0, 200.0, 0.0));
        let mut buffers = QueryBuffers::new();
        let mut lowest = f32::MAX;

        for _ in 0..600 {
            state.velocity.y = (state.velocity.y - 25.0 / 60.0).max(-60.0);
            tick(&world, &config, &mut state, &mut buffers);
            lowest = lowest.min(state.position.y);
        }
        assert!(lowest > -0.05, "it reached {lowest}");
    }

    #[test]
    fn a_character_walking_into_a_wall_stops_at_it() {
        let mut world = flat_world();
        world.add_box(
            Vec3::new(2.0, 0.0, -10.0),
            Vec3::new(3.0, 4.0, 10.0),
            "WALL",
        );
        world.rebuild_broad_phase();
        let config = config();
        let mut state = CharacterState::at(Vec3::ZERO);
        state.grounded = true;
        let mut buffers = QueryBuffers::new();

        for _ in 0..240 {
            state.velocity = Vec3::new(7.0, 0.0, 0.0);
            move_character(
                &world,
                &config,
                &mut state,
                Vec3::new(7.0 / 60.0, 0.0, 0.0),
                &mut buffers,
            );
        }

        assert!(
            state.position.x < 2.0 - config.radius + 0.02,
            "it went into the wall: {}",
            state.position.x
        );
        assert!(state.position.x > 1.0, "it stopped short of the wall");
    }

    #[test]
    fn a_diagonal_run_at_a_wall_slides_along_it() {
        let mut world = flat_world();
        world.add_box(
            Vec3::new(2.0, 0.0, -20.0),
            Vec3::new(3.0, 4.0, 20.0),
            "WALL",
        );
        world.rebuild_broad_phase();
        let config = config();
        let mut state = CharacterState::at(Vec3::ZERO);
        state.grounded = true;
        let mut buffers = QueryBuffers::new();

        for _ in 0..120 {
            state.velocity = Vec3::new(5.0, 0.0, -5.0);
            let delta = state.velocity / 60.0;
            move_character(&world, &config, &mut state, delta, &mut buffers);
        }

        assert!(state.position.x < 2.0, "it passed the wall");
        assert!(
            state.position.z < -3.0,
            "it did not slide along the wall: z = {}",
            state.position.z
        );
    }

    #[test]
    fn a_character_in_a_corner_does_not_get_stuck_or_squeezed_out() {
        let mut world = flat_world();
        world.add_box(
            Vec3::new(2.0, 0.0, -20.0),
            Vec3::new(3.0, 4.0, 2.0),
            "WALL X",
        );
        world.add_box(
            Vec3::new(-20.0, 0.0, 2.0),
            Vec3::new(3.0, 4.0, 3.0),
            "WALL Z",
        );
        world.rebuild_broad_phase();
        let config = config();
        let mut state = CharacterState::at(Vec3::ZERO);
        state.grounded = true;
        let mut buffers = QueryBuffers::new();

        for _ in 0..240 {
            state.velocity = Vec3::new(6.0, 0.0, 6.0);
            tick(&world, &config, &mut state, &mut buffers);
        }

        assert!(
            state.position.x < 2.0 && state.position.z < 2.0,
            "{state:?}"
        );
        assert!(
            state.position.x > 1.0 && state.position.z > 1.0,
            "it was ejected from the corner: {state:?}"
        );

        // And it can walk back out.
        for _ in 0..120 {
            state.velocity = Vec3::new(-6.0, 0.0, -6.0);
            tick(&world, &config, &mut state, &mut buffers);
        }
        assert!(
            state.position.x < 0.0 && state.position.z < 0.0,
            "{state:?}"
        );
    }

    #[test]
    fn a_character_walks_over_a_step_below_the_limit_and_not_over_one_above_it() {
        let config = config();
        // Walks +X into a block that spans x in [2, 6], stopping in the middle of it. Where
        // the character is when it stops is the answer: standing on top, or still in front.
        let walk = |height: f32| {
            let mut world = flat_world();
            world.add_box(
                Vec3::new(2.0, 0.0, -5.0),
                Vec3::new(6.0, height, 5.0),
                "STEP",
            );
            world.rebuild_broad_phase();
            let mut state = CharacterState::at(Vec3::ZERO);
            state.grounded = true;
            let mut buffers = QueryBuffers::new();
            for _ in 0..600 {
                state.velocity = Vec3::new(4.0, state.velocity.y - 25.0 / 60.0, 0.0);
                tick(&world, &config, &mut state, &mut buffers);
                if state.position.x > 4.0 {
                    break;
                }
            }
            state
        };

        for height in [0.15_f32, 0.30, config.max_step_height] {
            let state = walk(height);
            assert!(
                state.position.x > 4.0,
                "a {height} m step blocked it: {state:?}"
            );
            assert!(
                state.grounded,
                "it is airborne on top of the step: {state:?}"
            );
            assert!(
                (state.position.y - height).abs() < 0.05,
                "a {height} m step left it at {}",
                state.position.y
            );
        }

        for height in [config.max_step_height + 0.3, 0.9, 2.0] {
            let state = walk(height);
            assert!(
                state.position.x < 2.0,
                "a {height} m wall was climbed: {state:?}"
            );
            assert!(
                state.position.y < 0.05,
                "it climbed part way up a {height} m wall: {state:?}"
            );
        }
    }

    #[test]
    fn a_zero_height_step_changes_nothing() {
        let config = config();
        let mut world = flat_world();
        world.add_block(Vec3::new(3.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 8.0), "FLAT");
        world.rebuild_broad_phase();
        let mut state = CharacterState::at(Vec3::ZERO);
        state.grounded = true;
        let mut buffers = QueryBuffers::new();
        for _ in 0..180 {
            state.velocity = Vec3::new(4.0, state.velocity.y - 25.0 / 60.0, 0.0);
            tick(&world, &config, &mut state, &mut buffers);
        }
        assert!(state.position.x > 5.0, "{state:?}");
        assert!(state.position.y.abs() < 0.02, "{state:?}");
    }

    /// A ramp rising along +X, its lower edge at the origin.
    ///
    /// Rotating a slab by `+degrees` about +Z carries its own +X axis upwards, so the top
    /// face climbs towards +X and its normal leans back towards −X. The centre is placed
    /// half a slab-length up that axis, which is what puts the bottom edge at the origin.
    fn ramp_world(degrees: f32) -> CollisionWorld {
        let mut world = flat_world();
        let radians = degrees.to_radians();
        let half_length = 6.0;
        let ramp = Obb::ramp(
            Vec3::new(
                half_length * radians.cos(),
                half_length * radians.sin(),
                0.0,
            ),
            Vec3::new(half_length, 0.25, 5.0),
            Vec3::Z,
            radians,
        );
        world.add_ramp(ramp, "RAMP");
        world.rebuild_broad_phase();
        world
    }

    #[test]
    fn a_walkable_slope_is_climbed_and_reported_as_ground() {
        let config = config();
        for degrees in [0.0_f32, 15.0, 30.0, 45.0] {
            let world = ramp_world(degrees);
            let mut state = CharacterState::at(Vec3::new(-1.0, 0.5, 0.0));
            let mut buffers = QueryBuffers::new();
            for _ in 0..600 {
                state.velocity = Vec3::new(4.0, state.velocity.y - 25.0 / 60.0, 0.0);
                tick(&world, &config, &mut state, &mut buffers);
                // Stop well short of the ramp's top edge. Walking off it would land the
                // character back on the floor, and stopping *on* it would measure the
                // normal of the end cap rather than of the surface being climbed.
                if state.position.x > 4.0 {
                    break;
                }
            }
            assert!(state.grounded, "{degrees} degrees: it left the ramp");
            assert!(
                state.position.x > 1.0,
                "{degrees} degrees: it did not climb ({state:?})"
            );
            if degrees > 1.0 {
                assert!(
                    state.position.y > 0.2,
                    "{degrees} degrees: it stayed at the bottom ({state:?})"
                );
                let angle = state.slope_angle().to_degrees();
                assert!(
                    (angle - degrees).abs() < 3.0,
                    "{degrees} degrees: reported {angle}"
                );
            }
        }
    }

    #[test]
    fn a_slope_past_the_limit_is_a_wall() {
        let config = config();
        let world = ramp_world(70.0);
        let mut state = CharacterState::at(Vec3::new(-1.0, 0.5, 0.0));
        let mut buffers = QueryBuffers::new();
        let mut hit_wall = false;
        for _ in 0..240 {
            state.velocity = Vec3::new(4.0, state.velocity.y - 25.0 / 60.0, 0.0);
            hit_wall |= tick(&world, &config, &mut state, &mut buffers).hit_wall;
        }
        assert!(
            state.position.y < 1.0,
            "a 70-degree face was climbed to {}",
            state.position.y
        );
        assert!(hit_wall, "the steep face was never reported as a wall");
    }

    #[test]
    fn walking_off_a_ledge_leaves_the_ground_and_falls() {
        let mut world = CollisionWorld::new();
        world.add_box(
            Vec3::new(-10.0, -1.0, -10.0),
            Vec3::new(0.0, 0.0, 10.0),
            "PLATFORM",
        );
        world.add_ground(-20.0, "FLOOR");
        world.rebuild_broad_phase();
        let config = config();
        let mut state = CharacterState::at(Vec3::new(-3.0, 0.0, 0.0));
        state.grounded = true;
        let mut buffers = QueryBuffers::new();

        let mut left_the_ground = false;
        for _ in 0..120 {
            state.velocity = Vec3::new(5.0, state.velocity.y - 25.0 / 60.0, 0.0);
            tick(&world, &config, &mut state, &mut buffers);
            left_the_ground |= !state.grounded;
        }
        assert!(left_the_ground, "it walked on air past the edge");
        assert!(state.position.y < -1.0, "it did not fall: {state:?}");
    }

    #[test]
    fn walking_down_a_ramp_keeps_the_character_on_it() {
        // Without the ground probe this is a sequence of small falls: the character leaves
        // the surface every tick and lands on it again, and the state flickers.
        let config = config();
        let world = ramp_world(25.0);
        let mut state = CharacterState::at(Vec3::new(6.0, 3.4, 0.0));
        let mut buffers = QueryBuffers::new();
        for _ in 0..120 {
            state.velocity = Vec3::new(0.0, state.velocity.y - 25.0 / 60.0, 0.0);
            tick(&world, &config, &mut state, &mut buffers);
        }
        assert!(state.grounded, "it did not settle onto the ramp: {state:?}");

        let mut airborne_ticks = 0;
        for _ in 0..90 {
            state.velocity = Vec3::new(-4.0, state.velocity.y - 25.0 / 60.0, 0.0);
            tick(&world, &config, &mut state, &mut buffers);
            if !state.grounded {
                airborne_ticks += 1;
            }
        }
        assert!(
            airborne_ticks <= 2,
            "it bounced down the ramp for {airborne_ticks} ticks"
        );
    }

    #[test]
    fn a_very_fast_character_cannot_cross_a_thin_wall() {
        // The tunnelling case: 200 m/s against a 5 cm barrier is 3.3 m of movement in one
        // tick against a wall a fifteenth of the capsule's radius thick.
        let mut world = flat_world();
        world.add_box(
            Vec3::new(5.0, 0.0, -20.0),
            Vec3::new(5.05, 4.0, 20.0),
            "THIN WALL",
        );
        world.rebuild_broad_phase();
        let config = config();
        let mut state = CharacterState::at(Vec3::ZERO);
        state.grounded = true;
        let mut buffers = QueryBuffers::new();

        for _ in 0..60 {
            state.velocity = Vec3::new(200.0, 0.0, 0.0);
            move_character(
                &world,
                &config,
                &mut state,
                Vec3::new(200.0 / 60.0, 0.0, 0.0),
                &mut buffers,
            );
        }
        assert!(
            state.position.x < 5.0,
            "it tunnelled through to {}",
            state.position.x
        );
    }

    #[test]
    fn a_fast_fall_cannot_cross_a_thin_floor() {
        let mut world = CollisionWorld::new();
        world.add_box(
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.02, 10.0),
            "THIN FLOOR",
        );
        world.rebuild_broad_phase();
        let config = config();
        let mut state = CharacterState::at(Vec3::new(0.0, 50.0, 0.0));
        let mut buffers = QueryBuffers::new();

        for _ in 0..300 {
            state.velocity.y = (state.velocity.y - 25.0 / 60.0).max(-120.0);
            tick(&world, &config, &mut state, &mut buffers);
        }
        assert!(state.grounded, "it fell through: {state:?}");
        assert!(state.position.y > 0.0, "{state:?}");
    }

    #[test]
    fn a_ceiling_stops_an_upward_move_and_is_reported() {
        let mut world = flat_world();
        world.add_box(
            Vec3::new(-5.0, 2.5, -5.0),
            Vec3::new(5.0, 3.0, 5.0),
            "CEILING",
        );
        world.rebuild_broad_phase();
        let config = config();
        let mut state = CharacterState::at(Vec3::ZERO);
        let mut buffers = QueryBuffers::new();
        state.velocity = Vec3::new(0.0, 12.0, 0.0);

        let mut hit_ceiling = false;
        for _ in 0..60 {
            let result = tick(&world, &config, &mut state, &mut buffers);
            hit_ceiling |= result.hit_ceiling;
            state.velocity.y -= 25.0 / 60.0;
        }
        assert!(hit_ceiling, "the ceiling was never reported");
        assert!(state.position.y < 2.5, "it went through: {state:?}");
    }

    #[test]
    fn a_character_spawned_inside_a_box_is_pushed_out_of_it() {
        // Overlapping the corner of a small block, not buried in the middle of a large one:
        // a character standing exactly inside a solid pillar that reaches the floor has no
        // free position to be pushed to, and resolution is not a search.
        let mut world = flat_world();
        world.add_block(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0), "BLOCK");
        world.rebuild_broad_phase();
        let config = config();
        let mut state = CharacterState::at(Vec3::new(0.3, 0.0, 0.3));
        let mut buffers = QueryBuffers::new();

        resolve_overlaps(&world, &config, &mut state, &mut buffers);
        assert!(
            !world.capsule_overlaps(
                &config.capsule_at(state.position),
                config.mask,
                &mut buffers
            ),
            "it is still inside: {state:?}"
        );
    }

    #[test]
    fn a_zero_displacement_move_changes_nothing() {
        let world = flat_world();
        let config = config();
        let mut state = CharacterState::at(Vec3::new(2.0, 3.0, -1.0));
        let mut buffers = QueryBuffers::new();
        let before = state;
        let result = move_character(&world, &config, &mut state, Vec3::ZERO, &mut buffers);

        assert_eq!(state.position, before.position);
        assert_eq!(result.moved, Vec3::ZERO);
        assert_eq!(result.substeps, 0);
    }

    #[test]
    fn velocity_is_clipped_into_a_surface_and_left_alone_away_from_one() {
        let mut into = Vec3::new(3.0, -4.0, 0.0);
        clip_velocity(&mut into, Vec3::Y);
        assert_eq!(into, Vec3::new(3.0, 0.0, 0.0));

        let mut away = Vec3::new(3.0, 4.0, 0.0);
        clip_velocity(&mut away, Vec3::Y);
        assert_eq!(away, Vec3::new(3.0, 4.0, 0.0), "leaving is not colliding");
    }

    #[test]
    fn the_walkable_threshold_follows_the_configured_slope_limit() {
        let mut config = CharacterConfig {
            max_slope_angle: 45.0_f32.to_radians(),
            ..CharacterConfig::default()
        };
        assert!(config.is_walkable(Vec3::Y));
        let forty = 40.0_f32.to_radians();
        assert!(config.is_walkable(Vec3::new(0.0, forty.cos(), forty.sin())));
        let fifty = 50.0_f32.to_radians();
        assert!(!config.is_walkable(Vec3::new(0.0, fifty.cos(), fifty.sin())));

        config.max_slope_angle = 60.0_f32.to_radians();
        assert!(config.is_walkable(Vec3::new(0.0, fifty.cos(), fifty.sin())));
    }

    #[test]
    fn a_substep_is_never_longer_than_half_a_radius_unless_asked_for() {
        let config = CharacterConfig::default();
        assert!((config.substep_distance() - config.radius * 0.5).abs() < 1e-6);
        let explicit = CharacterConfig {
            max_substep_distance: 1.5,
            ..CharacterConfig::default()
        };
        assert_eq!(explicit.substep_distance(), 1.5);
    }

    #[test]
    fn moving_costs_no_allocation_once_the_buffers_are_warm() {
        let mut world = flat_world();
        world.add_box(
            Vec3::new(2.0, 0.0, -10.0),
            Vec3::new(3.0, 4.0, 10.0),
            "WALL",
        );
        world.rebuild_broad_phase();
        let config = config();
        let mut state = CharacterState::at(Vec3::ZERO);
        let mut buffers = QueryBuffers::with_capacity(8);

        for _ in 0..60 {
            state.velocity = Vec3::new(6.0, -3.0, 1.0);
            tick(&world, &config, &mut state, &mut buffers);
        }
        let capacity = buffers.capacity();

        for step in 0..2_000 {
            state.velocity = Vec3::new(6.0, -3.0, ((step % 7) as f32 - 3.0) * 2.0);
            tick(&world, &config, &mut state, &mut buffers);
        }
        assert_eq!(buffers.capacity(), capacity, "a move allocated");
    }

    #[test]
    fn the_same_input_sequence_produces_the_same_state_every_time() {
        let run = || {
            let mut world = flat_world();
            world.add_box(
                Vec3::new(2.0, 0.0, -10.0),
                Vec3::new(3.0, 4.0, 10.0),
                "WALL",
            );
            world.add_block(Vec3::new(-2.0, 0.0, 0.0), Vec3::new(1.0, 0.3, 1.0), "STEP");
            world.rebuild_broad_phase();
            let config = config();
            let mut state = CharacterState::at(Vec3::new(0.0, 1.0, 0.0));
            let mut buffers = QueryBuffers::new();
            for step in 0..600 {
                let x = ((step % 13) as f32 - 6.0) * 0.8;
                let z = ((step % 7) as f32 - 3.0) * 0.9;
                state.velocity = Vec3::new(x, state.velocity.y - 25.0 / 60.0, z);
                tick(&world, &config, &mut state, &mut buffers);
            }
            state
        };
        let first = run();
        assert_eq!(first, run());
        assert_eq!(first, run());
    }

    #[test]
    fn a_surface_can_be_described_without_re_deriving_the_rules() {
        let config = config();
        let (angle, walkable) = describe(&config, &Contact::new(Vec3::ZERO, Vec3::Y, 0.0));
        assert!(angle.abs() < 1e-6);
        assert!(walkable);
        let (angle, walkable) = describe(&config, &Contact::new(Vec3::ZERO, Vec3::X, 0.0));
        assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!(!walkable);
    }
}
