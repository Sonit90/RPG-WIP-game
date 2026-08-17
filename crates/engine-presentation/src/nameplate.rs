//! Nameplates: what each nearby enemy is, how hurt it is, and what is on it.
//!
//! ```text
//!            +------------------+   <- white outline, current target only
//!            | DUMMY        87% |
//!            | BURNING 3.2      |
//!            +------------------+
//!                    |
//!                  (dummy)
//! ```
//!
//! A nameplate is a *projection*, not an object: gameplay knows nothing about it, and the
//! renderer is handed pixels and colours. This module is where the two meet — it takes the
//! simulation and the camera's view-projection and produces a list of laid-out plates in
//! screen space, which the application then pushes into the overlay.
//!
//! Everything here is arithmetic over `engine-core` types, so the projection, the culling
//! rules and the text are all unit tested with no device and no window.
//!
//! ## What decides whether a plate is drawn
//!
//! 1. The entity is hostile to the player and alive. A corpse showing `0%` is noise, and an
//!    ally with an enemy nameplate is a lie.
//! 2. It is within [`NameplateConfig::max_distance`] of the **player**, not of the camera:
//!    the question a player is asking is "what is near me", and a camera pulled back would
//!    otherwise change which enemies have plates.
//! 3. Its anchor point is in front of the camera and inside the viewport. Behind the camera
//!    the projection is a mirrored ghost, which is the classic nameplate bug.

use engine_core::math::{Mat4, Vec3, Vec4};
use engine_game::entity::EntityId;
use engine_game::sim::Simulation;
use engine_game::status::StatusKind;
use engine_game::world::{EntityKind, horizontal_distance};

/// How far an enemy may be from the player and still have a nameplate, in metres.
pub const NAMEPLATE_MAX_DISTANCE: f32 = 40.0;

/// Characters a plate is laid out for.
///
/// Fixed so the health percentage is right-aligned against a stable edge: a plate whose
/// width followed its own text would jitter as the number went from `100%` to `99%`.
pub const NAMEPLATE_COLUMNS: usize = 18;

/// Plates drawn at once, nearest first, before the rest are dropped.
///
/// A bound rather than a preference. Each plate is text, text is one quad per lit font cell,
/// and a hundred plates was measured at 0.44 ms a frame — 1515 FPS against 4654 for the same
/// scene without them. Twenty covers everything a player is fighting and costs a tenth of
/// that. See `docs/phase3.md`.
pub const MAX_NAMEPLATES: usize = 20;

/// Statuses a plate shows before it stops listing them.
///
/// Four is already more than fits the width; the cap exists so the layout cannot be made
/// unbounded by a future ability that stacks a dozen effects.
pub const MAX_NAMEPLATE_STATUSES: usize = 4;

/// One status shown on a plate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NameplateStatus {
    /// How many stacks, for a stacking debuff like Ignite. Zero for anything else.
    pub stacks: u8,
    /// Which status.
    pub kind: StatusKind,
    /// Seconds left on it.
    pub remaining: f32,
}

/// One laid-out nameplate, in screen space.
///
/// `Copy` and free of heap storage — the status list is a fixed array — so the whole set
/// lives in one reusable `Vec` that is cleared every frame without releasing anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nameplate {
    /// Which enemy this describes.
    pub entity: EntityId,
    /// Where the plate's *top-left corner* goes, in pixels from the top-left of the target.
    pub origin: (f32, f32),
    /// Plate size in pixels.
    pub size: (f32, f32),
    /// Distance from the player, in metres.
    pub distance: f32,
    /// Whether this is the player's current target.
    pub is_target: bool,
    /// Uppercase name, for the overlay font.
    pub name: &'static str,
    /// Health remaining, in [0, 1].
    pub health_fraction: f32,
    /// Clip-space depth the plate is drawn at.
    ///
    /// Derived from distance, so a nearer plate sits in front of a further one wherever they
    /// overlap. The overlay shares the scene's `LESS` depth test, which means this sorts the
    /// plates without anything being sorted.
    pub depth: f32,
    statuses: [NameplateStatus; MAX_NAMEPLATE_STATUSES],
    status_count: usize,
}

impl Nameplate {
    /// Health remaining as a whole percentage, in 0..=100.
    ///
    /// A living enemy never reads `0%`: rounding a sliver of health down to zero says it is
    /// dead when the next hit still has something to remove.
    #[must_use]
    pub fn health_percent(&self) -> u32 {
        let rounded = (self.health_fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
        if rounded == 0 && self.health_fraction > 0.0 {
            1
        } else {
            rounded
        }
    }

    /// The statuses on this enemy, most recently applied last.
    #[must_use]
    pub fn statuses(&self) -> &[NameplateStatus] {
        &self.statuses[..self.status_count]
    }

    /// Whether anything is on this enemy.
    #[must_use]
    pub fn has_statuses(&self) -> bool {
        self.status_count > 0
    }

    /// Writes the title line — name on the left, health percentage on the right.
    ///
    /// In place, into a buffer the caller reuses: a plate is rebuilt every frame for every
    /// visible enemy, and formatting into a fresh `String` each time would allocate per
    /// enemy per frame.
    pub fn write_title(&self, out: &mut String) {
        use std::fmt::Write as _;
        out.clear();
        let percent = self.health_percent();
        // The name is truncated rather than allowed to push the percentage off its edge:
        // the right-hand column is the number a player reads at a glance.
        let room = NAMEPLATE_COLUMNS.saturating_sub(5);
        // Written character by character rather than through a temporary `String`. A
        // `collect()` here allocated once per plate per frame — twenty plates at 1,400 FPS
        // is 28,000 allocations a second for a name that never changes, which the combat
        // benchmark in Milestone 21 is what caught.
        let mut written = 0;
        for character in self.name.chars().take(room) {
            out.push(character);
            written += 1;
        }
        for _ in written..room {
            out.push(' ');
        }
        let _ = write!(out, " {percent:>3}%");
    }

    /// Writes the status line, or nothing if there is no status.
    ///
    /// Returns whether anything was written, so the caller knows whether the plate is one
    /// line tall or two.
    pub fn write_statuses(&self, out: &mut String) -> bool {
        use std::fmt::Write as _;
        out.clear();
        if self.status_count == 0 {
            return false;
        }
        for status in self.statuses() {
            if !out.is_empty() {
                out.push(' ');
            }
            if status.stacks > 0 {
                let _ = write!(out, "IGNITE {} {:.1}", status.stacks, status.remaining);
            } else {
                let _ = write!(out, "{} {:.1}", status.kind.as_str(), status.remaining);
            }
        }
        // Truncated on a character boundary; the font is ASCII, but the plate width is a
        // layout rule rather than a byte count.
        // Truncated in place: taking the prefix through a temporary `String` allocated once
        // per plate per frame.
        if out.chars().count() > NAMEPLATE_COLUMNS
            && let Some((offset, _)) = out.char_indices().nth(NAMEPLATE_COLUMNS)
        {
            out.truncate(offset);
        }
        true
    }
}

/// How nameplates are placed and sized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NameplateConfig {
    /// How far an enemy may be from the player and still have a plate, in metres.
    pub max_distance: f32,
    /// How many plates are drawn at once. The nearest survive.
    pub max_plates: usize,
    /// Metres above the enemy's head the plate is anchored.
    pub height_offset: f32,
    /// Pixels between the plate's text and its edge.
    pub padding_pixels: f32,
    /// Width of one font cell in pixels, from the renderer's overlay metrics.
    pub cell_pixels: f32,
    /// Left edge of one glyph to the left edge of the next, in pixels.
    pub glyph_advance_pixels: f32,
    /// Top of one text line to the top of the next, in pixels.
    pub line_advance_pixels: f32,
    /// Height of one glyph in pixels.
    pub glyph_height_pixels: f32,
}

impl Default for NameplateConfig {
    fn default() -> Self {
        Self {
            max_distance: NAMEPLATE_MAX_DISTANCE,
            max_plates: MAX_NAMEPLATES,
            height_offset: 0.9,
            padding_pixels: 4.0,
            cell_pixels: 2.0,
            glyph_advance_pixels: 12.0,
            line_advance_pixels: 18.0,
            glyph_height_pixels: 14.0,
        }
    }
}

impl NameplateConfig {
    /// Width in pixels of a plate holding [`NAMEPLATE_COLUMNS`] characters.
    #[must_use]
    pub fn plate_width(&self) -> f32 {
        NAMEPLATE_COLUMNS as f32 * self.glyph_advance_pixels + self.padding_pixels * 2.0
    }

    /// Height in pixels of a plate of `lines` text lines.
    #[must_use]
    pub fn plate_height(&self, lines: usize) -> f32 {
        let text = match lines {
            0 => 0.0,
            count => (count - 1) as f32 * self.line_advance_pixels + self.glyph_height_pixels,
        };
        text + self.padding_pixels * 2.0
    }
}

/// Builds the frame's nameplates.
///
/// Holds the list across frames and clears it rather than releasing it, so a steady scene
/// allocates nothing.
#[derive(Debug, Default, Clone)]
pub struct NameplateSet {
    plates: Vec<Nameplate>,
}

impl NameplateSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty set with room for `capacity` plates.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            plates: Vec::with_capacity(capacity),
        }
    }

    /// The plates built by the last [`NameplateSet::build`].
    #[must_use]
    pub fn plates(&self) -> &[Nameplate] {
        &self.plates
    }

    /// How many plates fit before the list grows.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.plates.capacity()
    }

    /// The plate under `pointer`, if the pointer is on one.
    ///
    /// A nameplate is a box on the screen with an enemy's name and health in it, so it is a
    /// thing that can be clicked. The *nearest* plate wins where two overlap, which is the
    /// same order the depth test draws them in — so the one the player can read is the one
    /// they select.
    ///
    /// Screen-space, and deliberately not a ray through the body: a plate floats above its
    /// enemy and is still a plate the player pointed at when the body under it is behind a
    /// pillar.
    #[must_use]
    pub fn at(&self, pointer: (f32, f32)) -> Option<&Nameplate> {
        self.plates
            .iter()
            .filter(|plate| {
                pointer.0 >= plate.origin.0
                    && pointer.0 <= plate.origin.0 + plate.size.0
                    && pointer.1 >= plate.origin.1
                    && pointer.1 <= plate.origin.1 + plate.size.1
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }

    /// Rebuilds the set for the current simulation state and camera.
    ///
    /// `viewport` is the render target size in pixels. Returns the plates, ordered furthest
    /// first — which matters only for a caller that ignores [`Nameplate::depth`], since the
    /// depth test is what actually resolves overlap.
    pub fn build(
        &mut self,
        simulation: &Simulation,
        view_projection: &Mat4,
        viewport: (u32, u32),
        config: &NameplateConfig,
    ) -> &[Nameplate] {
        self.plates.clear();
        let (width, height) = viewport;
        if width == 0 || height == 0 {
            return &self.plates;
        }

        let world = simulation.world();
        let player = simulation.player().entity;
        let Some(player_position) = world.position(player) else {
            return &self.plates;
        };
        let target = simulation.player().target;
        let max_distance = config.max_distance.max(0.0);
        let plate_width = config.plate_width();

        for entity in world.entities() {
            if entity == player || !world.is_alive(entity) {
                continue;
            }
            if world.kind(entity) == Some(EntityKind::Player) {
                continue;
            }
            let hostile = world
                .faction(player)
                .zip(world.faction(entity))
                .is_some_and(|(mine, theirs)| mine.is_hostile_to(theirs));
            if !hostile {
                continue;
            }

            let Some(position) = world.position(entity) else {
                continue;
            };
            let distance = horizontal_distance(player_position, position);
            if distance > max_distance {
                continue;
            }

            let anchor =
                position + Vec3::new(0.0, world.height(entity) + config.height_offset, 0.0);
            let Some((screen_x, screen_y)) = project(anchor, view_projection, viewport) else {
                continue;
            };

            let Some(health) = world.health(entity) else {
                continue;
            };
            let mut statuses = [NameplateStatus {
                kind: StatusKind::Burning,
                remaining: 0.0,
                stacks: 0,
            }; MAX_NAMEPLATE_STATUSES];
            let mut status_count = 0;
            // Ignite is the class's whole readable state on an enemy, and it lives in its
            // own tracker rather than in the generic status set: the plate reads it there so
            // "how close is this to a detonation" is visible without selecting the target.
            let burn = simulation.mage().ignite.get(entity);
            if burn.is_active() {
                statuses[status_count] = NameplateStatus {
                    kind: StatusKind::Burning,
                    remaining: burn.remaining,
                    stacks: burn.stacks,
                };
                status_count += 1;
            }
            if let Some(active) = world.statuses(entity) {
                for status in active.iter().take(MAX_NAMEPLATE_STATUSES - status_count) {
                    statuses[status_count] = NameplateStatus {
                        kind: status.kind,
                        remaining: status.remaining,
                        stacks: 0,
                    };
                    status_count += 1;
                }
            }

            let lines = if status_count > 0 { 2 } else { 1 };
            let plate_height = config.plate_height(lines);
            // Anchored by its bottom centre on the projected point, so the plate sits above
            // the head rather than on it and stays centred as the enemy moves.
            let origin = (screen_x - plate_width * 0.5, screen_y - plate_height);

            if !overlaps_viewport(origin, (plate_width, plate_height), viewport) {
                continue;
            }

            self.plates.push(Nameplate {
                entity,
                origin,
                size: (plate_width, plate_height),
                distance,
                is_target: target == Some(entity),
                name: world.kind(entity).map_or("ENEMY", EntityKind::as_str),
                health_fraction: health.fraction(),
                depth: depth_for(distance, max_distance),
                statuses,
                status_count,
            });
        }

        // Nearest first, then bounded: which enemies lose their plate has to be a rule
        // ("the twenty nearest") rather than an accident of iteration order.
        // `sort_unstable_by`, not `sort_by`: the stable sort allocates a merge buffer once
        // the candidate list passes about twenty entries, which is one allocation per frame
        // in any scene with more than twenty enemies in range. The tie-break on entity id
        // already makes the order total, so stability buys nothing here.
        self.plates.sort_unstable_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then_with(|| a.entity.cmp(&b.entity))
        });
        self.plates.truncate(config.max_plates);
        // Then furthest first for drawing. The depth test resolves overlap by itself, but a
        // stable order makes two runs of the same frame produce the same command stream.
        self.plates.reverse();
        &self.plates
    }
}

/// Clip-space depth band nameplates are drawn in.
///
/// Small enough to stay in front of the scene — the near plane is 0.0 — and wide enough that
/// forty metres of distance separates cleanly into distinct depths.
const NEAREST_PLATE_DEPTH: f32 = 0.010;

/// Depth of the furthest plate.
const FURTHEST_PLATE_DEPTH: f32 = 0.100;

/// Depth for a plate at `distance`.
///
/// Nearer is smaller, so a nearer plate wins the `LESS` test wherever two overlap.
fn depth_for(distance: f32, max_distance: f32) -> f32 {
    if max_distance <= 0.0 {
        return NEAREST_PLATE_DEPTH;
    }
    let fraction = (distance / max_distance).clamp(0.0, 1.0);
    NEAREST_PLATE_DEPTH + fraction * (FURTHEST_PLATE_DEPTH - NEAREST_PLATE_DEPTH)
}

/// Whether a plate's rectangle touches the viewport at all.
fn overlaps_viewport(origin: (f32, f32), size: (f32, f32), viewport: (u32, u32)) -> bool {
    let (x, y) = origin;
    let (plate_width, plate_height) = size;
    let (width, height) = (viewport.0 as f32, viewport.1 as f32);
    x + plate_width > 0.0 && y + plate_height > 0.0 && x < width && y < height
}

/// Projects a world-space point to pixels from the top-left of the viewport.
///
/// Returns `None` for a point that is behind the camera or outside the depth range. That
/// check is the whole reason this is a function rather than three lines at the call site: a
/// point behind the camera still divides to a finite position on screen, and drawing it
/// produces a mirrored ghost plate that follows the camera around — which is the classic
/// nameplate bug and is invisible in any test that only checks a point in front.
#[must_use]
pub fn project(point: Vec3, view_projection: &Mat4, viewport: (u32, u32)) -> Option<(f32, f32)> {
    let (width, height) = viewport;
    if width == 0 || height == 0 {
        return None;
    }

    let clip = *view_projection * point.extend(1.0);
    // Positive w is in front of the camera. Zero is the eye plane, which has no projection.
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.perspective_divide()?;
    // Depth in [0, 1] is the convention `Mat4::perspective` produces; outside it the point
    // is nearer than the near plane or beyond the far plane.
    if !(0.0..=1.0).contains(&ndc.z) {
        return None;
    }

    Some((
        (ndc.x * 0.5 + 0.5) * width as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * height as f32,
    ))
}

/// Projects with a `Vec4` already in clip space. Exposed for tests and for a caller that
/// has the clip-space point already.
#[must_use]
pub fn project_clip(clip: Vec4, viewport: (u32, u32)) -> Option<(f32, f32)> {
    if clip.w <= 0.0 || viewport.0 == 0 || viewport.1 == 0 {
        return None;
    }
    let ndc = clip.perspective_divide()?;
    Some((
        (ndc.x * 0.5 + 0.5) * viewport.0 as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport.1 as f32,
    ))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::camera::ThirdPersonCamera;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::command::PlayerCommand;
    use engine_render_data::{Camera, Projection};

    const VIEWPORT: (u32, u32) = (1920, 1080);

    /// The arena, with the camera behind the player looking at the dummies.
    fn scene() -> (Simulation, Camera) {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let controller = ThirdPersonCamera::following(simulation.player_position());
        let mut camera =
            Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
        controller
            .apply(&mut camera, 16.0 / 9.0)
            .expect("a valid aspect");
        (simulation, camera)
    }

    fn config() -> NameplateConfig {
        NameplateConfig::default()
    }

    fn build(simulation: &Simulation, camera: &Camera, set: &mut NameplateSet) -> Vec<Nameplate> {
        set.build(simulation, &camera.view_projection(), VIEWPORT, &config())
            .to_vec()
    }

    #[test]
    fn a_pointer_on_a_plate_finds_the_enemy_it_belongs_to() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        // The nearest one: plates overlap in a group, and the near one is what the player
        // sees and therefore what a click on that patch of screen means.
        let plate = *plates
            .iter()
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
            .expect("the arena has dummies in front");

        // The middle of the plate, and each of its four corners.
        let centre = (
            plate.origin.0 + plate.size.0 * 0.5,
            plate.origin.1 + plate.size.1 * 0.5,
        );
        assert_eq!(set.at(centre).map(|found| found.entity), Some(plate.entity));
        for corner in [
            plate.origin,
            (plate.origin.0 + plate.size.0, plate.origin.1),
            (plate.origin.0, plate.origin.1 + plate.size.1),
            (plate.origin.0 + plate.size.0, plate.origin.1 + plate.size.1),
        ] {
            assert!(set.at(corner).is_some(), "{corner:?} is on the plate");
        }

        // A pixel outside it on every side is not on it.
        assert!(
            set.at((plate.origin.0 - 1.0, centre.1)).is_none()
                || set.at((plate.origin.0 - 1.0, centre.1)).map(|f| f.entity) != Some(plate.entity)
        );
        assert!(set.at((-50.0, -50.0)).is_none(), "nothing is off-screen");
    }

    #[test]
    fn the_nearest_plate_wins_where_two_overlap() {
        // Two plates on top of each other is the ordinary case in a pack, and the one the
        // player can read is the near one — which is the one the depth test draws on top.
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(plates.len() > 1, "the arena has a group in it");

        for plate in &plates {
            let centre = (
                plate.origin.0 + plate.size.0 * 0.5,
                plate.origin.1 + plate.size.1 * 0.5,
            );
            let found = set.at(centre).expect("a plate is under its own centre");
            assert!(
                found.distance <= plate.distance,
                "a further plate was picked over a nearer one"
            );
        }
    }

    #[test]
    fn a_point_in_front_projects_into_the_viewport() {
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        camera.set_position(Vec3::new(0.0, 0.0, 10.0));

        let (x, y) = project(Vec3::ZERO, &camera.view_projection(), VIEWPORT)
            .expect("the origin is in front of the camera");
        assert!((x - 960.0).abs() < 1.0, "{x}");
        assert!((y - 540.0).abs() < 1.0, "{y}");
    }

    #[test]
    fn a_point_behind_the_camera_has_no_projection() {
        // The classic nameplate bug: a point behind the camera still divides to a finite
        // screen position, and drawing it produces a mirrored ghost that follows the camera.
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        camera.set_position(Vec3::new(0.0, 0.0, 10.0));

        assert_eq!(
            project(
                Vec3::new(0.0, 0.0, 20.0),
                &camera.view_projection(),
                VIEWPORT
            ),
            None
        );
        assert_eq!(project_clip(Vec4::new(1.0, 1.0, 1.0, -2.0), VIEWPORT), None);
    }

    #[test]
    fn screen_y_grows_downward() {
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        camera.set_position(Vec3::new(0.0, 0.0, 10.0));
        let view_projection = camera.view_projection();

        let high = project(Vec3::new(0.0, 2.0, 0.0), &view_projection, VIEWPORT).expect("visible");
        let low = project(Vec3::new(0.0, -2.0, 0.0), &view_projection, VIEWPORT).expect("visible");
        assert!(high.1 < low.1, "{high:?} {low:?}");
    }

    #[test]
    fn a_degenerate_viewport_projects_nothing() {
        let camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        assert_eq!(
            project(Vec3::ZERO, &camera.view_projection(), (0, 1080)),
            None
        );
        assert_eq!(
            project(Vec3::ZERO, &camera.view_projection(), (1920, 0)),
            None
        );
    }

    #[test]
    fn every_nearby_enemy_gets_a_plate() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);

        // The arena's dummies are all within 40 m of the player and in front of the camera.
        assert_eq!(plates.len(), arena::dummies(simulation.world()).count());
        assert!(plates.iter().all(|plate| plate.name == "DUMMY"));
        assert!(plates.iter().all(|plate| plate.health_percent() == 100));
    }

    #[test]
    fn the_player_never_has_a_plate() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let player = simulation.player().entity;
        assert!(plates.iter().all(|plate| plate.entity != player));
    }

    #[test]
    fn only_the_current_target_is_outlined() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);

        let target = simulation.player().target.expect("a dummy is targeted");
        let outlined: Vec<EntityId> = plates
            .iter()
            .filter(|plate| plate.is_target)
            .map(|plate| plate.entity)
            .collect();
        assert_eq!(outlined, vec![target], "exactly one plate is the target");
    }

    #[test]
    fn an_enemy_beyond_the_distance_limit_has_no_plate() {
        let (mut simulation, camera) = scene();
        let far = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        if let Some(body) = simulation.world_mut().body_mut(far) {
            body.position = Vec3::new(0.0, 0.0, -80.0);
        }

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(
            plates.iter().all(|plate| plate.entity != far),
            "an enemy 80 m away kept its plate"
        );
        assert!(
            plates
                .iter()
                .all(|plate| plate.distance <= NAMEPLATE_MAX_DISTANCE),
            "a plate survived past the distance limit"
        );
    }

    #[test]
    fn the_limit_is_measured_from_the_player_not_the_camera() {
        // A camera pulled back must not change which enemies have plates.
        let (mut simulation, _) = scene();
        let dummy = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        if let Some(body) = simulation.world_mut().body_mut(dummy) {
            body.position = Vec3::new(0.0, 0.0, -35.0);
        }

        let mut close = ThirdPersonCamera::following(simulation.player_position());
        close.distance = 3.0;
        let mut far = ThirdPersonCamera::following(simulation.player_position());
        far.distance = 25.0;

        let mut set = NameplateSet::new();
        let mut seen = Vec::new();
        for controller in [close, far] {
            let mut camera =
                Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
            controller
                .apply(&mut camera, 16.0 / 9.0)
                .expect("a valid aspect");
            let plates = build(&simulation, &camera, &mut set);
            seen.push(plates.iter().any(|plate| plate.entity == dummy));
        }
        assert_eq!(seen[0], seen[1], "the camera changed the distance rule");
        assert!(seen[0], "a dummy 35 m from the player should have a plate");
    }

    #[test]
    fn the_number_of_plates_is_bounded_and_the_nearest_survive() {
        // Each plate is text, and text is one quad per lit font cell. Without a bound, a
        // large pull would cost more than the frame it is drawn in.
        let mut simulation = arena::training_arena(&ArenaConfig::with_group(200));
        simulation.step_once();
        let controller = ThirdPersonCamera::following(simulation.player_position());
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        controller
            .apply(&mut camera, 16.0 / 9.0)
            .expect("a valid aspect");

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(plates.len() <= MAX_NAMEPLATES, "{} plates", plates.len());

        // What survived is the nearest, not whatever came first out of the world.
        let kept: f32 = plates
            .iter()
            .map(|plate| plate.distance)
            .fold(0.0_f32, f32::max);
        let world = simulation.world();
        let player = simulation.player_position();
        let nearer_dropped = arena::dummies(world)
            .filter(|dummy| world.is_alive(*dummy))
            .filter_map(|dummy| world.position(dummy))
            .map(|position| engine_game::world::horizontal_distance(player, position))
            .filter(|distance| *distance < kept)
            .count();
        assert!(
            nearer_dropped <= plates.len(),
            "a nearer enemy was dropped in favour of a further one"
        );
    }

    #[test]
    fn a_dead_enemy_has_no_plate() {
        let (mut simulation, camera) = scene();
        let dummy = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        simulation.world_mut().damage(dummy, 1_000_000.0);
        simulation.step_once();

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(plates.iter().all(|plate| plate.entity != dummy));
    }

    #[test]
    fn an_enemy_behind_the_camera_has_no_plate() {
        let (simulation, _) = scene();
        let mut controller = ThirdPersonCamera::following(simulation.player_position());
        // Turn a half circle: the dummies are now behind the camera.
        controller.yaw = std::f32::consts::PI;
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        controller
            .apply(&mut camera, 16.0 / 9.0)
            .expect("a valid aspect");

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(plates.is_empty(), "{} ghost plates", plates.len());
    }

    #[test]
    fn health_is_reported_as_a_percentage_that_never_reads_zero_while_alive() {
        let (mut simulation, camera) = scene();
        let dummy = simulation.player().target.expect("a dummy is targeted");
        let max = simulation
            .world()
            .health(dummy)
            .expect("the dummy exists")
            .max();
        simulation.world_mut().damage(dummy, max * 0.75);

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let plate = plates
            .iter()
            .find(|plate| plate.entity == dummy)
            .expect("the target has a plate");
        assert_eq!(plate.health_percent(), 25);

        // One point of health left: a plate that rounds to 0% says it is dead.
        simulation.world_mut().damage(dummy, max * 0.25 - 1.0);
        let plates = build(&simulation, &camera, &mut set);
        let plate = plates
            .iter()
            .find(|plate| plate.entity == dummy)
            .expect("the target has a plate");
        assert_eq!(plate.health_percent(), 1);
    }

    #[test]
    fn the_title_puts_the_name_left_and_the_health_right() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let mut title = String::new();
        plates[0].write_title(&mut title);

        assert!(title.starts_with("DUMMY"), "{title:?}");
        assert!(title.ends_with("100%"), "{title:?}");
        assert_eq!(
            title.chars().count(),
            NAMEPLATE_COLUMNS,
            "the width must not depend on the number: {title:?}"
        );

        // And the width is stable as the number shrinks, or the right edge jitters.
        let mut damaged = plates[0];
        damaged.health_fraction = 0.07;
        let mut second = String::new();
        damaged.write_title(&mut second);
        assert_eq!(second.chars().count(), title.chars().count(), "{second:?}");
    }

    #[test]
    fn statuses_are_listed_with_the_time_left() {
        let (mut simulation, camera) = scene();
        let burning = simulation.player().target.expect("a dummy is targeted");
        // Ignite is the class's burn: it lives in its own tracker, not the status set.
        let maximum = simulation.mage().ignite_max();
        let duration = simulation.mage().ignite_duration();
        simulation
            .mage_mut()
            .ignite
            .apply(burning, 2, maximum, duration);
        simulation.step_once();

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let plate = plates
            .iter()
            .find(|plate| plate.entity == burning)
            .expect("the burning dummy has a plate");

        assert!(plate.has_statuses());
        assert_eq!(plate.statuses().len(), 1);
        assert_eq!(plate.statuses()[0].kind, StatusKind::Burning);
        assert!(plate.statuses()[0].remaining > 0.0);

        let mut line = String::new();
        assert!(plate.write_statuses(&mut line));
        assert!(line.starts_with("IGNITE"), "{line:?}");
        assert!(line.chars().count() <= NAMEPLATE_COLUMNS, "{line:?}");

        // An unaffected dummy has no status line at all, so its plate is one line tall.
        let plain = plates
            .iter()
            .find(|plate| plate.entity != burning)
            .expect("the arena has other dummies");
        assert!(!plain.has_statuses());
        assert!(!plain.write_statuses(&mut line));
        assert!(line.is_empty());
        assert!(plain.size.1 < plate.size.1, "a status adds a line");
    }

    #[test]
    fn every_character_a_plate_can_emit_has_a_glyph() {
        // The renderer's 5x7 font covers exactly this set; anything else renders as a
        // hollow box.
        const ALLOWED: &str = " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ.,:-+/()%=_";
        let (mut simulation, camera) = scene();
        simulation.push_command(PlayerCommand::UseAbility(1));
        simulation.step_once();

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let mut line = String::new();
        for plate in &plates {
            plate.write_title(&mut line);
            for character in line.chars() {
                assert!(ALLOWED.contains(character), "{character:?} in {line:?}");
            }
            if plate.write_statuses(&mut line) {
                for character in line.chars() {
                    assert!(ALLOWED.contains(character), "{character:?} in {line:?}");
                }
            }
        }
    }

    #[test]
    fn a_nearer_plate_is_drawn_in_front_of_a_further_one() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);

        for pair in plates.windows(2) {
            // Ordered furthest first, so depth must decrease along the list.
            assert!(pair[0].distance >= pair[1].distance, "order");
            assert!(
                pair[0].depth >= pair[1].depth,
                "depth does not follow distance"
            );
        }
        assert!(
            plates
                .iter()
                .all(|plate| plate.depth > 0.0 && plate.depth < 0.5)
        );
    }

    #[test]
    fn plates_are_anchored_above_the_enemy_and_centred_on_it() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let config = config();
        let plate = plates
            .iter()
            .find(|plate| plate.is_target)
            .expect("the target has a plate");

        let world = simulation.world();
        let head = world.position(plate.entity).expect("the dummy exists")
            + Vec3::new(0.0, world.height(plate.entity) + config.height_offset, 0.0);
        let (anchor_x, anchor_y) =
            project(head, &camera.view_projection(), VIEWPORT).expect("the head is visible");

        assert!(
            (plate.origin.0 + plate.size.0 * 0.5 - anchor_x).abs() < 0.01,
            "not centred: {plate:?}"
        );
        assert!(
            (plate.origin.1 + plate.size.1 - anchor_y).abs() < 0.01,
            "not sitting above the head: {plate:?}"
        );
    }

    #[test]
    fn rebuilding_reuses_the_allocation() {
        let (mut simulation, camera) = scene();
        let mut set = NameplateSet::new();
        build(&simulation, &camera, &mut set);
        let capacity = set.capacity();

        for _ in 0..600 {
            simulation.push_command(PlayerCommand::Move {
                forward: 1.0,
                right: 0.2,
                yaw: 0.1,
                face_camera: false,
            });
            simulation.step_once();
            set.build(&simulation, &camera.view_projection(), VIEWPORT, &config());
        }
        assert_eq!(set.capacity(), capacity, "the plate list reallocated");
    }
}
