//! The boss's chest on screen: it drops from the sky, bounces, waits, and bursts open.
//!
//! ```text
//! Simulation::loot_chest()  --state-->  ChestDirector  --transforms-->  RenderWorld
//!   position, opened, age                clocks, pose()                  body, lid, treasure
//! ```
//!
//! # Events first, state to catch up
//!
//! Online the client simulation never holds a chest - the realm owns it and only its events
//! are injected - so `ChestSpawned` / `ChestOpened` create and open it. Offline the simulation's
//! own chest state is also read every frame: it is the authority there, and it is how a late
//! join or a reload gets the chest that is already landed and already open without anybody
//! having replayed events. A chest the simulation stops reporting disappears the same frame;
//! one that only ever came from events is removed by [`ChestDirector::clear`] when the
//! instance is left.
//!
//! # Real time, not ticks
//!
//! The simulation steps at a fixed rate and its clock is quantised to that step; a fall drawn
//! from it would stutter at any frame rate above the tick rate. The director therefore starts a
//! clock when it first sees a chest - seeded with the simulation's own age of it, so a late
//! sighting resumes mid-fall - and advances that clock by the frame's real delta. The landing
//! still happens at `CHEST_FALL_SECONDS` after the spawn, which is what the simulation gates
//! opening on, to within the one tick of difference between the two clocks.
//!
//! # Three models, one transform
//!
//! The body, the lid and the treasure are separate models (see
//! [`engine_assets::procedural::chest`]). Every frame the director computes one root transform
//! for the whole chest - where it is, how it is tumbling, how squashed - and derives the lid's
//! (hinge offset, rotation about +X) and the treasure's from it. Entities are created once when
//! the chest appears and only moved afterwards.

use engine_assets::procedural::chest::{self as model, LID_HINGE, TREASURE_BASE};
use engine_core::math::{Quat, Sphere, Vec3};
use engine_core::transform::Transform;
use engine_game::Simulation;
use engine_game::event::CombatEvent;
use engine_game::sim::{CHEST_DROP_HEIGHT, CHEST_FALL_SECONDS, CHEST_INTERACT_RANGE};
use engine_render_data::skinning::{ModelBinding, PartGeometry};
use engine_render_data::world::{InstanceStyle, RenderEntity, RenderEntityDesc, RenderWorld};

/// How long the opening takes from the first rattle to the lid settled, in seconds.
pub const OPEN_SECONDS: f32 = 1.0;
/// How long the chest rattles before the lid moves, in seconds.
pub const ANTICIPATION_SECONDS: f32 = 0.28;
/// How far the lid opens, in radians: 105 degrees.
pub const LID_OPEN_ANGLE: f32 = 105.0 * core::f32::consts::PI / 180.0;
/// How long the landing bounce lasts, in seconds.
pub const BOUNCE_SECONDS: f32 = 0.46;
/// Radius of the sphere a right-click is tested against, in metres.
pub const PICK_RADIUS: f32 = 0.95;

/// The uploaded chest models.
#[derive(Debug, Clone)]
pub struct ChestVisuals {
    /// [`model::chest_body`].
    pub body: ModelBinding,
    /// [`model::chest_lid`].
    pub lid: ModelBinding,
    /// [`model::chest_treasure`].
    pub treasure: ModelBinding,
}

/// Where every moving piece of the chest is at one instant. Pure data, so the animation is
/// testable without a renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChestPose {
    /// Metres above its resting place.
    pub height: f32,
    /// The tumble while it falls, about its centre of mass at the base.
    pub tumble: Quat,
    /// Extra yaw while it falls, radians, fading to zero on landing.
    pub spin: f32,
    /// Squash and stretch about the base.
    pub scale: Vec3,
    /// A rattle before opening, radians about the depth axis.
    pub rattle: f32,
    /// How far the lid is open, radians.
    pub lid: f32,
    /// How big the treasure is, `0` hidden to `1` full size.
    pub treasure: f32,
    /// Emissive added to the gold and the treasure.
    pub glow: f32,
}

/// `1 - (1 - x)^3`, a fast start that settles.
fn ease_out_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

/// Ease-out with an overshoot and settle, `1.70158` being the classic strength.
///
/// Crosses 1 at about three quarters of the way and peaks near 1.1, which is what makes a lid
/// feel heavy: it swings past where it will rest and drops back.
fn ease_out_back(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let c1 = 1.701_58;
    let c3 = c1 + 1.0;
    1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
}

/// How high above the ground the chest is `seconds` after it spawned, at `drop` metres up,
/// landing after `fall` seconds.
///
/// Constant acceleration: the distance fallen goes with the square of the time, so the chest
/// starts slow and arrives fast, and arrives exactly on time.
#[must_use]
pub fn fall_height(drop: f32, fall: f32, seconds: f32) -> f32 {
    let s = (seconds / fall.max(1e-3)).clamp(0.0, 1.0);
    drop * (1.0 - s * s)
}

/// Computes the pose `since_spawn` seconds after the chest appeared and `since_open` seconds
/// after it was opened, with `wall` a free-running clock for ambient pulses and `phase` a
/// per-chest offset so two chests do not tumble identically.
#[must_use]
pub fn pose(since_spawn: f32, since_open: Option<f32>, wall: f32, phase: f32) -> ChestPose {
    let mut pose = ChestPose {
        height: 0.0,
        tumble: Quat::IDENTITY,
        spin: 0.0,
        scale: Vec3::ONE,
        rattle: 0.0,
        lid: 0.0,
        treasure: 0.0,
        glow: 0.0,
    };

    let fall = CHEST_FALL_SECONDS;
    if since_spawn < fall {
        let s = (since_spawn / fall).clamp(0.0, 1.0);
        pose.height = fall_height(CHEST_DROP_HEIGHT, fall, since_spawn);
        // A tumble that dies away as the ground approaches: zero on arrival, so there is no
        // snap on the landing frame.
        let decay = (1.0 - s) * (1.0 - s);
        let pitch = 0.85 * decay * (s * 7.0 + phase).sin();
        let roll = 0.65 * decay * (s * 5.0 + phase * 1.3).cos();
        pose.tumble = Quat::from_axis_angle(Vec3::X, pitch) * Quat::from_axis_angle(Vec3::Z, roll);
        pose.spin = 2.2 * decay;
    } else {
        // Landed: a hop, a smaller hop, then still; and a spring squash that rings out.
        let landed = since_spawn - fall;
        if landed < 0.30 {
            pose.height = 0.16 * (core::f32::consts::PI * landed / 0.30).sin();
        } else if landed < BOUNCE_SECONDS {
            pose.height = 0.04 * (core::f32::consts::PI * (landed - 0.30) / 0.16).sin();
        }
        let k = 0.16 * (-landed * 8.0).exp() * (landed * 26.0).cos();
        pose.scale = Vec3::new(1.0 + 0.5 * k, 1.0 - k, 1.0 + 0.5 * k);
        // Waiting: the gold breathes so a chest in a dark corner still catches the eye.
        pose.glow = 0.16 + 0.12 * (wall * 2.4 + phase).sin();
    }

    if let Some(open) = since_open {
        if open < ANTICIPATION_SECONDS {
            // It shakes with growing violence, then the lid shudders up a hair.
            let build = open / ANTICIPATION_SECONDS;
            pose.rattle = 0.045 * build * (open * 75.0).sin();
            pose.lid = 0.05 * build * (open * 90.0).sin().abs();
            pose.glow += 0.5 * build;
        } else {
            let p = ((open - ANTICIPATION_SECONDS) / (OPEN_SECONDS - ANTICIPATION_SECONDS))
                .clamp(0.0, 1.0);
            pose.lid = LID_OPEN_ANGLE * ease_out_back(p);
            pose.treasure = ease_out_cubic(p * 1.4);
            // A flash as the lid goes, decaying to a shimmer that stays.
            let flash = (-(open - ANTICIPATION_SECONDS) * 3.2).exp();
            pose.glow = 0.35 + 0.15 * (wall * 3.0 + phase).sin() + 1.4 * flash;
        }
    }
    pose
}

/// How long a looted chest takes to fade from its shimmer to stillness, in seconds.
pub const LOOTED_FADE_SECONDS: f32 = 0.6;

/// The glow a chest keeps once the local player has looted it: a faint, steady warmth.
pub const LOOTED_GLOW: f32 = 0.06;

/// Stills a chest the local player has already looted.
///
/// The breathing glow and the shimmer exist to say "there is something here for you"; once
/// the player has taken their share that is no longer true, and a chest that kept pulsing
/// would keep calling them back to nothing. The opening plays out in full first - cutting it
/// short would read as a glitch - and then the glow eases down to a constant, so the chest
/// rests rather than snapping dark.
pub fn settle_looted(pose: &mut ChestPose, since_open: Option<f32>) {
    let settled = since_open.map_or(1.0, |open| {
        ((open - OPEN_SECONDS) / LOOTED_FADE_SECONDS).clamp(0.0, 1.0)
    });
    pose.glow += (LOOTED_GLOW - pose.glow) * settled;
}

/// One render entity and the local bounds it was spawned with.
#[derive(Debug, Clone, Copy)]
struct Piece {
    entity: RenderEntity,
    bounds: Sphere,
    part: usize,
}

/// The chest that is on screen.
#[derive(Debug)]
struct Live {
    id: u32,
    position: Vec3,
    yaw: f32,
    phase: f32,
    since_spawn: f32,
    since_open: Option<f32>,
    /// Whether the simulation's own chest state has confirmed this chest.
    from_state: bool,
    /// Whether the local player has taken their share.
    claimed: bool,
    body: Vec<Piece>,
    lid: Vec<Piece>,
    treasure: Vec<Piece>,
    treasure_shown: bool,
}

/// Draws the loot chest from the simulation's state.
#[derive(Debug)]
pub struct ChestDirector {
    visuals: ChestVisuals,
    live: Option<Live>,
    wall: f32,
}

impl ChestDirector {
    /// A director drawing with `visuals`.
    #[must_use]
    pub fn new(visuals: ChestVisuals) -> Self {
        Self {
            visuals,
            live: None,
            wall: 0.0,
        }
    }

    /// Whether a chest is currently on screen.
    #[must_use]
    pub fn is_present(&self) -> bool {
        self.live.is_some()
    }

    /// Removes the chest from `world`.
    pub fn clear(&mut self, world: &mut RenderWorld) {
        if let Some(live) = self.live.take() {
            for piece in live.body.iter().chain(&live.lid).chain(&live.treasure) {
                world.despawn(piece.entity);
            }
        }
    }

    /// Where a prompt belongs: above the closed lid, once the chest has landed.
    #[must_use]
    pub fn prompt_anchor(&self) -> Option<Vec3> {
        let live = self.live.as_ref()?;
        (live.since_spawn >= CHEST_FALL_SECONDS).then(|| live.position + Vec3::new(0.0, 1.25, 0.0))
    }

    /// The distance along a ray at which it meets the chest's pick sphere, if it does.
    ///
    /// A sphere rather than the mesh: the chest is a small, roughly round-ish target the
    /// player is aiming a right-click at, and a generous sphere is easier to hit than the
    /// lid's edge. Only once it has landed; nobody clicks a chest in the air.
    #[must_use]
    pub fn pick(&self, origin: Vec3, direction: Vec3) -> Option<f32> {
        let live = self.live.as_ref()?;
        if live.since_spawn < CHEST_FALL_SECONDS {
            return None;
        }
        let centre = live.position + Vec3::new(0.0, 0.4, 0.0);
        let to_centre = centre - origin;
        let along = to_centre.dot(direction);
        if along < 0.0 {
            return None;
        }
        let miss_squared = to_centre.length_squared() - along * along;
        (miss_squared <= PICK_RADIUS * PICK_RADIUS).then_some(along)
    }

    /// What the interact key would do for the local player right now, if anything.
    ///
    /// Offline the simulation answers ([`Simulation::can_open_chest`]). Online the client has
    /// no chest of its own - the realm owns it and only its events arrive - so the answer is
    /// worked out here from the same three facts: it has landed, the player is in range, and
    /// they have not taken their share.
    #[must_use]
    pub fn prompt(&self, simulation: &Simulation) -> Option<ChestPrompt> {
        let live = self.live.as_ref()?;
        if live.since_spawn < CHEST_FALL_SECONDS {
            return None;
        }
        let local = simulation.player().entity;
        let may_open = if simulation.loot_chest().is_some() {
            simulation.can_open_chest(local)
        } else {
            let at = simulation.player_position();
            !live.claimed
                && simulation.world().is_alive(local)
                && (at.x - live.position.x).hypot(at.z - live.position.z) <= CHEST_INTERACT_RANGE
        };
        may_open.then_some(ChestPrompt {
            opened: live.since_open.is_some(),
        })
    }

    /// Syncs with the simulation and advances the animation by `delta` real seconds.
    ///
    /// Driven by the frame's events first, so a client that only receives events (online)
    /// still gets its chest; then by the simulation's chest state, which is the offline
    /// authority and the way a late joiner is brought up to date.
    pub fn update(&mut self, simulation: &Simulation, delta: f32, world: &mut RenderWorld) {
        let delta = delta.clamp(0.0, 0.25);
        self.wall = (self.wall + delta) % 1000.0;
        let local = simulation.player().entity;

        for event in simulation.events() {
            match *event {
                CombatEvent::ChestSpawned {
                    chest, position, ..
                } => {
                    self.clear(world);
                    self.create(world, simulation, chest, position, 0.0);
                }
                CombatEvent::ChestOpened { chest, .. } => {
                    if let Some(live) = self.live.as_mut()
                        && live.id == chest
                        && live.since_open.is_none()
                    {
                        live.since_open = Some(0.0);
                    }
                }
                CombatEvent::LootAwarded { player, .. }
                | CombatEvent::LootMailed { player, .. }
                    if player == local =>
                {
                    if let Some(live) = self.live.as_mut() {
                        live.claimed = true;
                    }
                }
                _ => {}
            }
        }

        match simulation.loot_chest() {
            Some(chest) => {
                if self.live.as_ref().is_some_and(|live| live.id != chest.id()) {
                    self.clear(world);
                }
                if self.live.is_none() {
                    self.create(
                        world,
                        simulation,
                        chest.id(),
                        chest.position(),
                        chest.age(simulation.time()),
                    );
                    // Seen already open - a late join - is open: no replayed animation.
                    if let Some(live) = self.live.as_mut() {
                        live.since_open = chest.opened().then_some(OPEN_SECONDS);
                    }
                }
                if let Some(live) = self.live.as_mut() {
                    live.from_state = true;
                    if chest.opened() && live.since_open.is_none() {
                        live.since_open = Some(0.0);
                    }
                    live.claimed = !chest
                        .claims()
                        .iter()
                        .any(|claim| claim.player == local && !claim.claimed);
                }
            }
            // The simulation had a chest and no longer does: a dungeon reset.
            None => {
                if self.live.as_ref().is_some_and(|live| live.from_state) {
                    self.clear(world);
                }
            }
        }

        let Some(live) = self.live.as_mut() else {
            return;
        };
        live.since_spawn += delta;
        if let Some(open) = live.since_open.as_mut() {
            *open += delta;
        }
        let mut pose = pose(live.since_spawn, live.since_open, self.wall, live.phase);
        if live.claimed {
            settle_looted(&mut pose, live.since_open);
        }
        apply(live, &pose, world);
    }

    /// Puts a new chest on screen, facing the local player.
    fn create(
        &mut self,
        world: &mut RenderWorld,
        simulation: &Simulation,
        id: u32,
        position: Vec3,
        since_spawn: f32,
    ) {
        let toward = simulation.player_position() - position;
        let yaw = if toward.x.hypot(toward.z) > 0.01 {
            (-toward.x).atan2(-toward.z)
        } else {
            0.0
        };
        let mut live = Live {
            id,
            position,
            yaw,
            phase: (id % 16) as f32 * 0.9,
            since_spawn,
            since_open: None,
            from_state: false,
            claimed: false,
            body: Vec::new(),
            lid: Vec::new(),
            treasure: Vec::new(),
            treasure_shown: false,
        };
        spawn_pieces(world, &self.visuals.body, &mut live.body);
        spawn_pieces(world, &self.visuals.lid, &mut live.lid);
        spawn_pieces(world, &self.visuals.treasure, &mut live.treasure);
        for piece in &live.treasure {
            world.set_visible(piece.entity, false);
        }
        self.live = Some(live);
    }
}

/// What pressing the interact key would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChestPrompt {
    /// Whether the chest is already open: the prompt then reads "Loot".
    pub opened: bool,
}

/// Spawns every rigid part of `binding`'s finest level, hidden nowhere: visible.
fn spawn_pieces(world: &mut RenderWorld, binding: &ModelBinding, out: &mut Vec<Piece>) {
    let Some(level) = binding.lod(0) else {
        return;
    };
    for (part, model_part) in level.parts.iter().enumerate() {
        let PartGeometry::Rigid(mesh) = model_part.geometry else {
            continue;
        };
        let entity = world.spawn(RenderEntityDesc {
            transform: Transform::IDENTITY,
            mesh,
            material: model_part.material,
            bounds: model_part.bounds,
            visible: true,
            style: InstanceStyle::NEUTRAL,
            ..RenderEntityDesc::default()
        });
        out.push(Piece {
            entity,
            bounds: model_part.bounds,
            part,
        });
    }
}

/// Writes `pose` into the chest's entities.
fn apply(live: &mut Live, pose: &ChestPose, world: &mut RenderWorld) {
    let facing = Quat::from_axis_angle(Vec3::Y, live.yaw + pose.spin);
    let rattle = Quat::from_axis_angle(Vec3::Z, pose.rattle);
    let root = Transform::new(
        live.position + Vec3::new(0.0, pose.height, 0.0),
        facing * pose.tumble * rattle,
        pose.scale,
    );
    let lid = root.then(Transform::new(
        LID_HINGE,
        Quat::from_axis_angle(Vec3::X, pose.lid),
        Vec3::ONE,
    ));
    let shown = pose.treasure > 0.001;
    let treasure = root.then(Transform::new(
        TREASURE_BASE,
        Quat::IDENTITY,
        Vec3::splat(pose.treasure.max(0.001)),
    ));
    let gold = InstanceStyle::NEUTRAL.with_emissive(pose.glow);
    let hoard = InstanceStyle::NEUTRAL.with_emissive(0.4 + pose.glow);

    for piece in &live.body {
        world.set_transform(piece.entity, root, piece.bounds);
        if piece.part == model::part::GOLD {
            world.set_style(piece.entity, gold);
        }
    }
    for piece in &live.lid {
        world.set_transform(piece.entity, lid, piece.bounds);
        if piece.part == model::part::GOLD {
            world.set_style(piece.entity, gold);
        }
    }
    for piece in &live.treasure {
        world.set_transform(piece.entity, treasure, piece.bounds);
        world.set_style(piece.entity, hoard);
    }
    if shown != live.treasure_shown {
        for piece in &live.treasure {
            world.set_visible(piece.entity, shown);
        }
        live.treasure_shown = shown;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fall_starts_at_the_drop_height_and_lands_exactly_on_time() {
        assert!((fall_height(7.0, 1.1, 0.0) - 7.0).abs() < 1e-5);
        assert!(fall_height(7.0, 1.1, 1.1).abs() < 1e-5);
        assert!(
            fall_height(7.0, 1.1, 5.0).abs() < 1e-5,
            "it fell through the floor"
        );
    }

    #[test]
    fn the_fall_accelerates() {
        let drop_in = |a: f32, b: f32| fall_height(7.0, 1.1, a) - fall_height(7.0, 1.1, b);
        let first = drop_in(0.0, 0.2);
        let last = drop_in(0.9, 1.1);
        assert!(last > first * 4.0, "{first} then {last}");
    }

    #[test]
    fn the_tumble_has_died_away_by_the_time_it_lands() {
        let before = pose(CHEST_FALL_SECONDS - 0.001, None, 0.0, 1.0);
        let after = pose(CHEST_FALL_SECONDS, None, 0.0, 1.0);
        assert!(before.spin.abs() < 0.01);
        assert!(before.tumble.dot(Quat::IDENTITY).abs() > 0.9999);
        assert!(after.height.abs() < 0.01);
    }

    #[test]
    fn a_landed_chest_bounces_then_rests_and_unsquashes() {
        let hop = pose(CHEST_FALL_SECONDS + 0.15, None, 0.0, 0.0);
        assert!(hop.height > 0.1, "no bounce: {}", hop.height);
        let still = pose(CHEST_FALL_SECONDS + 1.5, None, 0.0, 0.0);
        assert_eq!(still.height, 0.0);
        assert!((still.scale.y - 1.0).abs() < 0.001, "{:?}", still.scale);
        // The impact itself squashes.
        assert!(pose(CHEST_FALL_SECONDS, None, 0.0, 0.0).scale.y < 0.9);
    }

    #[test]
    fn the_lid_rattles_then_swings_past_open_and_settles_at_one_hundred_and_five_degrees() {
        let settled = CHEST_FALL_SECONDS + 5.0;
        let rattling = pose(settled, Some(0.15), 0.0, 0.0);
        assert!(rattling.rattle != 0.0 && rattling.lid < 0.1);
        let peak = (0..100)
            .map(|step| pose(settled, Some(0.28 + step as f32 * 0.0072), 0.0, 0.0).lid)
            .fold(0.0, f32::max);
        assert!(peak > LID_OPEN_ANGLE * 1.03, "no overshoot: {peak}");
        let end = pose(settled, Some(OPEN_SECONDS), 0.0, 0.0);
        assert!((end.lid - LID_OPEN_ANGLE).abs() < 0.01, "{}", end.lid);
        assert!(end.treasure > 0.99);
    }

    #[test]
    fn a_closed_chest_shows_no_treasure_and_an_open_one_keeps_glowing() {
        let closed = pose(10.0, None, 0.0, 0.0);
        assert_eq!(closed.treasure, 0.0);
        assert!(closed.glow > 0.0, "a waiting chest does not draw the eye");
        assert!(pose(10.0, Some(30.0), 0.0, 0.0).glow > 0.15);
    }

    #[test]
    fn a_looted_chest_finishes_opening_then_stops_pulsing() {
        // Mid-opening the looted chest still plays its flash: the animation is not cut short.
        let mut opening = pose(10.0, Some(0.5), 0.0, 0.0);
        let before = opening.glow;
        settle_looted(&mut opening, Some(0.5));
        assert_eq!(opening.glow, before);

        // Once faded, the glow is the same constant whatever the ambient clock says.
        let settled = OPEN_SECONDS + LOOTED_FADE_SECONDS;
        let glows: Vec<f32> = [0.0_f32, 0.7, 1.9, 3.3]
            .iter()
            .map(|wall| {
                let mut still = pose(10.0, Some(settled + 5.0), *wall, 0.0);
                settle_looted(&mut still, Some(settled + 5.0));
                still.glow
            })
            .collect();
        for glow in glows {
            assert!((glow - LOOTED_GLOW).abs() < 1e-6, "still pulsing: {glow}");
        }
    }
}
