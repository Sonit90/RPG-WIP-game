//! Where nameplates actually end up: minimal-displacement collision resolution.
//!
//! ```text
//! preferred position (projected above the head)
//!         |
//!         v
//! clamp into the allowed area  ------------------+
//!         |                                      |
//!         v                                      |
//! separate overlapping pairs along the cheaper   |  repeated, a few times
//! axis, splitting the push between them          |
//!         |                                      |
//!         v                                      |
//! cap the displacement from the preferred spot --+
//!         |
//!         v
//! final position  --(optionally)-->  smoothed towards, frame by frame
//! ```
//!
//! # A nameplate belongs to its monster
//!
//! The projected point above a monster's head is the plate's *ideal* position, and this
//! module's whole job is to give up as little of it as possible. It is deliberately **not** a
//! vertical list: sorting every plate by screen Y and stacking them produces a tidy column
//! that has stopped describing where anything is, and a player reading it has to match names
//! to bodies by hand. Two plates that overlap are moved apart by the smallest push that
//! separates them, split between the two; a third plate on the other side of the screen is
//! not touched at all.
//!
//! # Margins are multiples of a plate, not pixels
//!
//! `vertical_margin` is a multiple of the plates' own heights and `horizontal_margin` of
//! their widths, and the top safe zone is a fraction of the viewport. Nothing here has a
//! pixel constant in it, so the same settings behave the same way at 720p and at 4K.
//!
//! # Nothing here knows what a monster is
//!
//! The input is rectangles with identities and preferred positions. There is no simulation,
//! no camera and no projection, which is what lets every rule below be a unit test over
//! numbers — including the ones that are impossible to see, like "fifteen plates on one
//! point degrade without anything flying off the screen".

/// One plate the layout has to place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlateBox {
    /// What this plate belongs to. Used to carry smoothing across frames, and never
    /// interpreted — the layout does not know an entity from a widget.
    pub id: u64,
    /// Where it would go if nothing else existed: the top-left corner, in pixels.
    pub preferred: (f32, f32),
    /// Its size in pixels.
    pub size: (f32, f32),
    /// How strongly it holds its preferred position, relative to the others.
    ///
    /// A pair being separated splits the push in inverse proportion to this, so a plate with
    /// twice the weight of its neighbour moves half as far. The current target is heavier
    /// than the rest: the plate the player is looking at is the one that should stay put.
    pub weight: f32,
}

impl PlateBox {
    /// A plate of `size` that would like to be at `preferred`, holding as hard as any other.
    #[must_use]
    pub fn new(id: u64, preferred: (f32, f32), size: (f32, f32)) -> Self {
        Self {
            id,
            preferred,
            size,
            weight: 1.0,
        }
    }

    /// The same plate, holding its position `weight` times as hard.
    #[must_use]
    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = if weight.is_finite() && weight > 0.0 {
            weight
        } else {
            1.0
        };
        self
    }
}

/// The rules the layout has to satisfy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutConstraints {
    /// The area plates are placed in, in pixels.
    pub viewport: (f32, f32),
    /// Minimum vertical gap between two plates, as a multiple of their height.
    pub vertical_margin: f32,
    /// Minimum horizontal gap between two plates, as a multiple of their width.
    pub horizontal_margin: f32,
    /// Fraction of the viewport's height at the top that no plate may enter.
    pub top_safe_zone: f32,
    /// How far a plate may be pushed from its preferred position, as a multiple of its
    /// height.
    ///
    /// The answer to "fifteen monsters standing on one point": a crowd that cannot be
    /// separated stops being separated rather than being flung across the screen. A plate at
    /// its limit still overlaps a neighbour, which is the honest outcome — the alternative is
    /// a plate so far from its monster that it describes nothing.
    pub max_displacement: f32,
    /// How many separation passes to run.
    ///
    /// Fixed rather than "until it converges": a crowd that cannot be resolved would run
    /// forever, and the frame has to end. Each pass is one sweep over the overlapping pairs.
    pub iterations: usize,
}

impl Default for LayoutConstraints {
    fn default() -> Self {
        Self {
            viewport: (1920.0, 1080.0),
            vertical_margin: engine_settings::nameplate::NAMEPLATE_VERTICAL_MARGIN_DEFAULT,
            horizontal_margin: engine_settings::nameplate::NAMEPLATE_HORIZONTAL_MARGIN_DEFAULT,
            top_safe_zone: engine_settings::nameplate::NAMEPLATE_SAFE_ZONE_DEFAULT,
            max_displacement: DEFAULT_MAX_DISPLACEMENT,
            iterations: DEFAULT_ITERATIONS,
        }
    }
}

/// How far a plate may travel from its monster by default, in plate heights.
///
/// Ten is a quarter of the way down a 1080p screen for a one-line plate: enough for a pack of
/// eight standing on one another to separate completely, and near enough that the plate is
/// still obviously that monster's.
pub const DEFAULT_MAX_DISPLACEMENT: f32 = engine_core::tuning::nameplates::MAX_DISPLACEMENT;

/// Separation passes per frame by default.
///
/// A pass separates every overlapping pair once; a chain of plates pushed into one another
/// takes a few for the push at one end to reach the other. Sixteen resolves a pull that can
/// be resolved at all at the tight margins the redesigned plate uses, and a pass over twenty plates that overlap nothing exits on the first
/// sweep — the cost is paid by crowds, which is where it belongs.
pub const DEFAULT_ITERATIONS: usize = engine_core::tuning::nameplates::LAYOUT_ITERATIONS;

/// How fast a plate slides towards its resolved position, in fraction per second.
///
/// High enough that the plate is on its monster within a couple of frames — a nameplate that
/// visibly lagged behind a moving body would be worse than one that jittered.
pub const DEFAULT_SMOOTHING: f32 = engine_core::tuning::nameplates::SMOOTHING_PER_SECOND;

/// A jump this far, in plate heights, is a cut rather than a movement.
///
/// A plate whose monster teleported, or which has just appeared, must not be animated across
/// the screen from wherever the last plate with its id happened to be.
const SNAP_DISTANCE: f32 = 8.0;

/// One plate's remembered position, for smoothing across frames.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tracked {
    id: u64,
    position: (f32, f32),
    /// The pass this entry was last used in. Entries not used are dropped, so a pack that
    /// died does not leave its plates in the list forever.
    seen: u64,
}

/// Resolves plate positions, and remembers them so they can be smoothed.
///
/// Holds its working buffers across frames and clears rather than releases them, so a steady
/// scene allocates nothing.
#[derive(Debug, Default, Clone)]
pub struct NameplateLayout {
    /// This frame's resolved positions, one per input plate, in the same order.
    resolved: Vec<(f32, f32)>,
    /// Indices into `resolved`, sorted by x. The broad phase.
    order: Vec<usize>,
    /// Where each plate was last drawn, by id.
    tracked: Vec<Tracked>,
    /// Which pass the tracked entries were last touched in.
    pass: u64,
}

impl NameplateLayout {
    /// An empty layout.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty layout with room for `capacity` plates.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            resolved: Vec::with_capacity(capacity),
            order: Vec::with_capacity(capacity),
            tracked: Vec::with_capacity(capacity),
            pass: 0,
        }
    }

    /// The positions produced by the last call. One per plate, in input order.
    #[must_use]
    pub fn positions(&self) -> &[(f32, f32)] {
        &self.resolved
    }

    /// How many plates fit before the working buffers grow.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.resolved.capacity()
    }

    /// Places every plate as close to its preferred position as the constraints allow.
    ///
    /// Returns one top-left corner per input plate, in input order. Deterministic: the same
    /// input produces the same output, which is what makes the tests below exact.
    pub fn resolve(
        &mut self,
        plates: &[PlateBox],
        constraints: &LayoutConstraints,
    ) -> &[(f32, f32)] {
        self.resolved.clear();
        self.order.clear();
        if plates.is_empty() {
            return &self.resolved;
        }

        let area = AllowedArea::new(constraints);
        // Everything starts exactly where it wants to be. Most frames end here too: a fight
        // where nothing overlaps does one pass, finds nothing and stops.
        for plate in plates {
            self.resolved.push(area.clamp(plate.preferred, plate.size));
            self.order.push(self.order.len());
        }
        if plates.len() < 2 {
            return &self.resolved;
        }

        for _ in 0..constraints.iterations.max(1) {
            if !self.separate(plates, constraints) {
                break;
            }
            self.constrain(plates, &area, constraints);
        }
        &self.resolved
    }

    /// One separation pass. Returns whether anything was moved.
    ///
    /// The broad phase is a sweep along x: the plates are sorted by their left edge and the
    /// inner loop stops as soon as a neighbour is further away than any pair could need. A
    /// pull spread across the screen therefore costs a sort and a walk rather than the
    /// square of the plate count, which is what keeps forty plates affordable.
    fn separate(&mut self, plates: &[PlateBox], constraints: &LayoutConstraints) -> bool {
        let resolved = &mut self.resolved;
        self.order
            .sort_unstable_by(|a, b| resolved[*a].0.total_cmp(&resolved[*b].0).then(a.cmp(b)));

        // The widest any pair could need to be apart horizontally: past this the sweep can
        // stop looking, because x only increases from here.
        let widest = plates
            .iter()
            .map(|plate| plate.size.0)
            .fold(0.0_f32, f32::max);
        let reach = widest * (1.0 + constraints.horizontal_margin.max(0.0));

        let mut moved = false;
        for first in 0..self.order.len() {
            let i = self.order[first];
            for second in (first + 1)..self.order.len() {
                let j = self.order[second];
                if self.resolved[j].0 - self.resolved[i].0 > reach {
                    break;
                }
                moved |= self.separate_pair(plates, constraints, i, j);
            }
        }
        moved
    }

    /// Pushes one overlapping pair apart along whichever axis costs less. Returns whether it
    /// had to.
    fn separate_pair(
        &mut self,
        plates: &[PlateBox],
        constraints: &LayoutConstraints,
        i: usize,
        j: usize,
    ) -> bool {
        let (a, b) = (plates[i], plates[j]);
        let gap_x = (a.size.0 + b.size.0) * 0.5 * constraints.horizontal_margin.max(0.0);
        let gap_y = (a.size.1 + b.size.1) * 0.5 * constraints.vertical_margin.max(0.0);
        let need_x = (a.size.0 + b.size.0) * 0.5 + gap_x;
        let need_y = (a.size.1 + b.size.1) * 0.5 + gap_y;

        let centre_a = centre(self.resolved[i], a.size);
        let centre_b = centre(self.resolved[j], b.size);
        let dx = centre_b.0 - centre_a.0;
        let dy = centre_b.1 - centre_a.1;
        let overlap_x = need_x - dx.abs();
        let overlap_y = need_y - dy.abs();
        if overlap_x <= 0.0 || overlap_y <= 0.0 {
            return false;
        }

        // The cheaper axis, which is the whole of "resolve minimally". For plates — wide and
        // short — that is almost always the vertical one, so a group standing together stacks
        // rather than fanning out across the screen. It is not assumed, though: two plates
        // side by side and barely touching are separated sideways, because that is the
        // smaller move.
        let (share_a, share_b) = split(a.weight, b.weight);
        if overlap_y <= overlap_x {
            // A pair on exactly the same point has no direction to be pushed in. The input
            // order decides, so the same frame always resolves the same way.
            let direction = if dy.abs() <= f32::EPSILON {
                -1.0
            } else {
                dy.signum()
            };
            self.resolved[i].1 -= direction * overlap_y * share_a;
            self.resolved[j].1 += direction * overlap_y * share_b;
        } else {
            let direction = if dx.abs() <= f32::EPSILON {
                -1.0
            } else {
                dx.signum()
            };
            self.resolved[i].0 -= direction * overlap_x * share_a;
            self.resolved[j].0 += direction * overlap_x * share_b;
        }
        true
    }

    /// Caps every displacement and puts every plate back inside the allowed area.
    ///
    /// Run after each separation pass rather than once at the end, so the next pass separates
    /// positions that are actually legal.
    fn constrain(
        &mut self,
        plates: &[PlateBox],
        area: &AllowedArea,
        constraints: &LayoutConstraints,
    ) {
        for (index, plate) in plates.iter().enumerate() {
            let limit = (plate.size.1 * constraints.max_displacement.max(0.0)).max(0.0);
            let position = self.resolved[index];
            let offset = (
                position.0 - plate.preferred.0,
                position.1 - plate.preferred.1,
            );
            let distance = (offset.0 * offset.0 + offset.1 * offset.1).sqrt();
            let capped = if distance > limit && distance > 0.0 {
                let scale = limit / distance;
                (
                    plate.preferred.0 + offset.0 * scale,
                    plate.preferred.1 + offset.1 * scale,
                )
            } else {
                position
            };
            self.resolved[index] = area.clamp(capped, plate.size);
        }
    }

    /// Slides the remembered positions towards this frame's resolved ones.
    ///
    /// Called after [`NameplateLayout::resolve`], with the frame's real time step. The
    /// resolved position is recomputed from the world every frame and never stored back, so
    /// this smooths *how* a plate reaches its monster and can never become where the plate
    /// thinks its monster is.
    ///
    /// A plate that has just appeared, or whose monster moved further than a plate is tall
    /// several times over, is placed rather than animated: sliding it across the screen would
    /// be a plate visibly detached from the body it names.
    pub fn smooth(
        &mut self,
        plates: &[PlateBox],
        delta_seconds: f32,
        per_second: f32,
    ) -> &[(f32, f32)] {
        let delta = if delta_seconds.is_finite() {
            delta_seconds.clamp(0.0, 0.25)
        } else {
            0.0
        };
        let rate = if per_second.is_finite() && per_second > 0.0 {
            per_second
        } else {
            0.0
        };
        // Exponential, so the result does not depend on the frame rate: the same half-second
        // of movement lands in the same place at 60 and at 300 frames a second.
        let blend = if rate > 0.0 && delta > 0.0 {
            1.0 - (-rate * delta).exp()
        } else {
            1.0
        };

        self.pass = self.pass.wrapping_add(1);
        for (index, plate) in plates.iter().enumerate() {
            let target = self.resolved[index];
            let snap = plate.size.1 * SNAP_DISTANCE;
            let position = match self.tracked.iter_mut().find(|entry| entry.id == plate.id) {
                Some(entry) => {
                    entry.seen = self.pass;
                    let offset = (target.0 - entry.position.0, target.1 - entry.position.1);
                    entry.position = if offset.0.abs() > snap || offset.1.abs() > snap {
                        target
                    } else {
                        (
                            entry.position.0 + offset.0 * blend,
                            entry.position.1 + offset.1 * blend,
                        )
                    };
                    entry.position
                }
                None => {
                    self.tracked.push(Tracked {
                        id: plate.id,
                        position: target,
                        seen: self.pass,
                    });
                    target
                }
            };
            self.resolved[index] = position;
        }
        // Whatever was not in this frame's list is gone: a plate that comes back gets a fresh
        // entry and is placed rather than animated, which is what should happen to a monster
        // that walked back into range.
        let pass = self.pass;
        self.tracked.retain(|entry| entry.seen == pass);
        &self.resolved
    }

    /// Forgets every remembered position, so the next frame places rather than slides.
    pub fn clear(&mut self) {
        self.tracked.clear();
        self.resolved.clear();
        self.order.clear();
    }
}

/// The rectangle plates are allowed to occupy.
#[derive(Debug, Clone, Copy, PartialEq)]
struct AllowedArea {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl AllowedArea {
    /// The viewport with the top safe zone taken off it.
    fn new(constraints: &LayoutConstraints) -> Self {
        let (width, height) = constraints.viewport;
        let width = if width.is_finite() {
            width.max(0.0)
        } else {
            0.0
        };
        let height = if height.is_finite() {
            height.max(0.0)
        } else {
            0.0
        };
        Self {
            left: 0.0,
            top: height * constraints.top_safe_zone.clamp(0.0, 1.0),
            right: width,
            bottom: height,
        }
    }

    /// `position` moved the smallest distance that puts a `size` plate inside this area.
    ///
    /// A plate too large for what is left of the screen is pinned to the top-left of the
    /// allowed area rather than being pushed off the other side of it: something the player
    /// can read part of beats something they cannot see.
    fn clamp(&self, position: (f32, f32), size: (f32, f32)) -> (f32, f32) {
        let x = position
            .0
            .clamp(self.left, (self.right - size.0).max(self.left));
        let y = position
            .1
            .clamp(self.top, (self.bottom - size.1).max(self.top));
        (x, y)
    }
}

/// The centre of a plate at `origin`.
fn centre(origin: (f32, f32), size: (f32, f32)) -> (f32, f32) {
    (origin.0 + size.0 * 0.5, origin.1 + size.1 * 0.5)
}

/// How a push is divided between two plates: the heavier one moves less.
fn split(a: f32, b: f32) -> (f32, f32) {
    let a = if a.is_finite() && a > 0.0 { a } else { 1.0 };
    let b = if b.is_finite() && b > 0.0 { b } else { 1.0 };
    let total = a + b;
    (b / total, a / total)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// A plate the size the game draws at 1080p: wide and short.
    const SIZE: (f32, f32) = (240.0, 26.0);

    fn constraints() -> LayoutConstraints {
        LayoutConstraints::default()
    }

    fn plate(id: u64, x: f32, y: f32) -> PlateBox {
        PlateBox::new(id, (x, y), SIZE)
    }

    /// Whether two placed plates' rectangles touch at all.
    fn overlaps(a: (f32, f32), b: (f32, f32)) -> bool {
        a.0 < b.0 + SIZE.0 && b.0 < a.0 + SIZE.0 && a.1 < b.1 + SIZE.1 && b.1 < a.1 + SIZE.1
    }

    /// How far a plate ended up from where it wanted to be.
    fn displacement(plate: &PlateBox, placed: (f32, f32)) -> f32 {
        let (dx, dy) = (placed.0 - plate.preferred.0, placed.1 - plate.preferred.1);
        (dx * dx + dy * dy).sqrt()
    }

    #[test]
    fn one_plate_stays_exactly_where_its_monster_is() {
        let mut layout = NameplateLayout::new();
        let plates = [plate(1, 800.0, 500.0)];
        let placed = layout.resolve(&plates, &constraints());
        assert_eq!(placed, [(800.0, 500.0)]);
    }

    #[test]
    fn two_plates_on_top_of_each_other_are_separated_and_split_the_move() {
        let mut layout = NameplateLayout::new();
        let plates = [plate(1, 800.0, 500.0), plate(2, 800.0, 505.0)];
        let placed = layout.resolve(&plates, &constraints()).to_vec();

        assert!(!overlaps(placed[0], placed[1]), "{placed:?}");
        // Neither is left where it was and neither carried the whole displacement: the push
        // is split, which is what minimises the total.
        let first = displacement(&plates[0], placed[0]);
        let second = displacement(&plates[1], placed[1]);
        assert!(first > 0.0 && second > 0.0, "{first} {second}");
        assert!(
            (first - second).abs() < 1.0,
            "one plate paid for both: {first} vs {second}"
        );
        // And they stayed above and below one another rather than being fanned out sideways,
        // because for a wide short plate the vertical push is the smaller one.
        assert!((placed[0].0 - 800.0).abs() < 0.01, "{placed:?}");
        assert!((placed[1].0 - 800.0).abs() < 0.01, "{placed:?}");
    }

    #[test]
    fn three_plates_on_one_point_all_separate() {
        let mut layout = NameplateLayout::new();
        let plates = [
            plate(1, 900.0, 400.0),
            plate(2, 902.0, 404.0),
            plate(3, 898.0, 408.0),
        ];
        let placed = layout.resolve(&plates, &constraints()).to_vec();
        for i in 0..placed.len() {
            for j in (i + 1)..placed.len() {
                assert!(!overlaps(placed[i], placed[j]), "{i} and {j}: {placed:?}");
            }
        }
    }

    #[test]
    fn plates_that_do_not_touch_are_not_moved_at_all() {
        // The rule the whole design exists for: resolving one collision must not rearrange
        // the screen.
        let mut layout = NameplateLayout::new();
        let plates = [
            plate(1, 100.0, 300.0),
            plate(2, 100.0, 306.0),
            plate(3, 1400.0, 700.0),
        ];
        let placed = layout.resolve(&plates, &constraints()).to_vec();
        assert_eq!(placed[2], (1400.0, 700.0), "an unrelated plate moved");
        assert!(!overlaps(placed[0], placed[1]));
    }

    #[test]
    fn a_plate_at_the_top_of_the_screen_is_pushed_out_of_the_safe_zone() {
        let mut layout = NameplateLayout::new();
        let constraints = constraints();
        let plates = [plate(1, 600.0, 4.0)];
        let placed = layout.resolve(&plates, &constraints).to_vec();

        let safe = constraints.viewport.1 * constraints.top_safe_zone;
        assert!(
            placed[0].1 >= safe - 0.01,
            "{placed:?} entered the top {safe}"
        );
        assert!(placed[0].0 == 600.0, "it moved sideways to obey a top rule");
    }

    #[test]
    fn the_safe_zone_scales_with_the_screen_rather_than_being_pixels() {
        let mut layout = NameplateLayout::new();
        let mut small = constraints();
        small.viewport = (1280.0, 720.0);
        let mut large = constraints();
        large.viewport = (3840.0, 2160.0);

        let plates = [plate(1, 100.0, 0.0)];
        let in_small = layout.resolve(&plates, &small).to_vec()[0].1;
        let in_large = layout.resolve(&plates, &large).to_vec()[0].1;
        assert!((in_small - 720.0 * small.top_safe_zone).abs() < 0.01);
        assert!((in_large - 2160.0 * large.top_safe_zone).abs() < 0.01);
        assert!(
            in_large > in_small,
            "the zone did not scale: {in_small} {in_large}"
        );
    }

    #[test]
    fn a_plate_at_the_edge_of_the_screen_stays_visible() {
        let mut layout = NameplateLayout::new();
        let constraints = constraints();
        let plates = [
            plate(1, -300.0, 500.0),
            plate(2, constraints.viewport.0 + 200.0, 700.0),
        ];
        let placed = layout.resolve(&plates, &constraints).to_vec();
        for (index, position) in placed.iter().enumerate() {
            assert!(
                position.0 >= -0.01,
                "{index} hangs off the left: {position:?}"
            );
            assert!(
                position.0 + SIZE.0 <= constraints.viewport.0 + 0.01,
                "{index} hangs off the right: {position:?}"
            );
        }
    }

    #[test]
    fn plates_keep_the_relative_order_their_monsters_are_in() {
        // Three monsters, left to right, the middle two overlapping. Separating them must not
        // reorder them: a plate that jumped past its neighbour would be pointing at the wrong
        // body.
        let mut layout = NameplateLayout::new();
        let plates = [
            plate(1, 200.0, 500.0),
            plate(2, 220.0, 508.0),
            plate(3, 1200.0, 500.0),
        ];
        let placed = layout.resolve(&plates, &constraints()).to_vec();
        assert!(placed[0].0 <= placed[1].0 + 0.01, "the left two swapped");
        assert!(placed[1].0 < placed[2].0, "the right plate crossed over");
        // The one that was above stays above.
        assert!(placed[0].1 < placed[1].1, "{placed:?}");
    }

    #[test]
    fn a_larger_vertical_margin_separates_plates_further() {
        let mut layout = NameplateLayout::new();
        let plates = [plate(1, 800.0, 500.0), plate(2, 800.0, 502.0)];

        let mut tight = constraints();
        tight.vertical_margin = 0.0;
        let tight_gap = {
            let placed = layout.resolve(&plates, &tight).to_vec();
            (placed[1].1 - placed[0].1).abs()
        };

        let mut wide = constraints();
        wide.vertical_margin = 2.0;
        let wide_gap = {
            let placed = layout.resolve(&plates, &wide).to_vec();
            (placed[1].1 - placed[0].1).abs()
        };

        assert!(
            wide_gap > tight_gap + 1.0,
            "the margin changed nothing: {tight_gap} then {wide_gap}"
        );
        assert!((tight_gap - SIZE.1).abs() < 0.5, "{tight_gap}");
        assert!((wide_gap - SIZE.1 * 3.0).abs() < 0.5, "{wide_gap}");
    }

    #[test]
    fn a_larger_horizontal_margin_separates_plates_further() {
        // Square boxes, side by side and level: here sideways *is* the cheaper axis, which is
        // the case the horizontal margin decides. The layout is not told which axis a plate
        // prefers — it works out which push is smaller — so this is the same code path a wide
        // plate takes, driven into its other branch.
        const BOX: (f32, f32) = (60.0, 60.0);
        let plates = [
            PlateBox::new(1, (700.0, 500.0), BOX),
            PlateBox::new(2, (755.0, 500.0), BOX),
        ];

        let measure = |margin: f32| {
            let mut layout = NameplateLayout::new();
            let mut constraints = constraints();
            constraints.horizontal_margin = margin;
            constraints.vertical_margin = 0.0;
            let placed = layout.resolve(&plates, &constraints).to_vec();
            assert!(
                (placed[0].1 - 500.0).abs() < 0.01 && (placed[1].1 - 500.0).abs() < 0.01,
                "the pair was separated vertically instead: {placed:?}"
            );
            (placed[1].0 - placed[0].0).abs()
        };

        let tight = measure(0.0);
        let wide = measure(0.5);
        assert!((tight - BOX.0).abs() < 0.5, "{tight}");
        assert!((wide - BOX.0 * 1.5).abs() < 0.5, "{wide}");
        assert!(
            wide > tight + 1.0,
            "the margin changed nothing: {tight} then {wide}"
        );
    }

    #[test]
    fn fifteen_monsters_on_one_point_degrade_without_anything_flying_away() {
        let mut layout = NameplateLayout::new();
        let constraints = constraints();
        let plates: Vec<PlateBox> = (0..15)
            .map(|index| plate(index, 900.0, 540.0 + index as f32 * 0.5))
            .collect();
        let placed = layout.resolve(&plates, &constraints).to_vec();

        for (index, position) in placed.iter().enumerate() {
            // Nothing left the screen, nothing entered the safe zone, and nothing travelled
            // further than the displacement cap.
            assert!(position.0 >= -0.01 && position.0 + SIZE.0 <= constraints.viewport.0 + 0.01);
            assert!(position.1 >= constraints.viewport.1 * constraints.top_safe_zone - 0.01);
            assert!(position.1 + SIZE.1 <= constraints.viewport.1 + 0.01);
            let travelled = displacement(&plates[index], *position);
            assert!(
                travelled <= SIZE.1 * constraints.max_displacement + 0.5,
                "plate {index} travelled {travelled}"
            );
        }
    }

    #[test]
    fn a_crowd_still_spreads_out_as_far_as_the_margins_allow() {
        // Degrading gracefully is not the same as giving up: eight plates on one point must
        // still be readable as eight, even if the fifteenth would not be.
        let mut layout = NameplateLayout::new();
        let plates: Vec<PlateBox> = (0..8)
            .map(|index| plate(index, 900.0, 500.0 + index as f32))
            .collect();
        let placed = layout.resolve(&plates, &constraints()).to_vec();

        let mut overlapping = 0;
        for i in 0..placed.len() {
            for j in (i + 1)..placed.len() {
                if overlaps(placed[i], placed[j]) {
                    overlapping += 1;
                }
            }
        }
        assert_eq!(overlapping, 0, "{placed:?}");
    }

    #[test]
    fn a_moving_monster_takes_its_plate_with_it() {
        let mut layout = NameplateLayout::new();
        let mut previous = layout
            .resolve(&[plate(1, 100.0, 500.0)], &constraints())
            .to_vec();
        for step in 1..40 {
            let x = 100.0 + step as f32 * 10.0;
            let placed = layout
                .resolve(&[plate(1, x, 500.0)], &constraints())
                .to_vec();
            assert!(placed[0].0 > previous[0].0, "the plate stopped following");
            assert_eq!(placed[0].0, x, "the plate lagged behind its monster");
            previous = placed;
        }
    }

    #[test]
    fn a_plate_never_drifts_away_from_its_monster_over_time() {
        // The failure a system that stored its own resolved position would have: a plate that
        // starts from where it was last frame accumulates its own displacement until it is
        // describing nothing. Every frame here starts from the world again.
        let mut layout = NameplateLayout::new();
        let constraints = constraints();
        for frame in 0..600 {
            let wobble = (frame as f32 * 0.1).sin() * 3.0;
            let plates = [
                plate(1, 900.0 + wobble, 500.0),
                plate(2, 900.0, 500.0 + wobble),
            ];
            let placed = layout.resolve(&plates, &constraints).to_vec();
            for (index, position) in placed.iter().enumerate() {
                let travelled = displacement(&plates[index], *position);
                assert!(
                    travelled <= SIZE.1 * 2.0,
                    "frame {frame}: plate {index} is {travelled} px from its monster"
                );
            }
        }
    }

    #[test]
    fn smoothing_closes_the_gap_and_never_overshoots() {
        let mut layout = NameplateLayout::new();
        let plates = [plate(1, 100.0, 500.0)];
        layout.resolve(&plates, &constraints());
        layout.smooth(&plates, 1.0 / 60.0, DEFAULT_SMOOTHING);

        // The monster steps sideways by less than a snap. The plate closes the distance over
        // a handful of frames without ever passing the target.
        let moved = [plate(1, 140.0, 500.0)];
        let mut last = 100.0;
        for _ in 0..30 {
            layout.resolve(&moved, &constraints());
            let position = layout.smooth(&moved, 1.0 / 60.0, DEFAULT_SMOOTHING)[0];
            assert!(position.0 >= last - 0.001, "it moved backwards");
            assert!(position.0 <= 140.0 + 0.001, "it overshot: {position:?}");
            last = position.0;
        }
        assert!((last - 140.0).abs() < 1.0, "it never arrived: {last}");
    }

    #[test]
    fn a_new_plate_and_a_teleport_are_placed_rather_than_animated() {
        let mut layout = NameplateLayout::new();
        let plates = [plate(1, 100.0, 500.0)];
        layout.resolve(&plates, &constraints());
        assert_eq!(
            layout.smooth(&plates, 1.0 / 60.0, DEFAULT_SMOOTHING)[0],
            (100.0, 500.0),
            "a plate's first frame was animated from nowhere"
        );

        let jumped = [plate(1, 1500.0, 500.0)];
        layout.resolve(&jumped, &constraints());
        assert_eq!(
            layout.smooth(&jumped, 1.0 / 60.0, DEFAULT_SMOOTHING)[0],
            (1500.0, 500.0),
            "the plate slid across the screen"
        );
    }

    #[test]
    fn a_target_holds_its_position_and_its_neighbour_gives_way() {
        let mut layout = NameplateLayout::new();
        let plates = [
            plate(1, 800.0, 500.0).with_weight(8.0),
            plate(2, 800.0, 504.0),
        ];
        let placed = layout.resolve(&plates, &constraints()).to_vec();
        let target = displacement(&plates[0], placed[0]);
        let other = displacement(&plates[1], placed[1]);
        assert!(
            other > target * 3.0,
            "the target was shoved as hard as the rest: {target} vs {other}"
        );
        assert!(!overlaps(placed[0], placed[1]));
    }

    #[test]
    fn the_same_input_always_produces_the_same_layout() {
        // Determinism, at the smallest scale it can be asserted. Two layouts, one input.
        let plates: Vec<PlateBox> = (0..12)
            .map(|index| {
                plate(
                    index,
                    700.0 + (index % 3) as f32 * 40.0,
                    400.0 + index as f32 * 6.0,
                )
            })
            .collect();
        let mut first = NameplateLayout::new();
        let mut second = NameplateLayout::new();
        assert_eq!(
            first.resolve(&plates, &constraints()),
            second.resolve(&plates, &constraints())
        );
    }

    #[test]
    fn resolving_repeatedly_allocates_nothing() {
        let mut layout = NameplateLayout::with_capacity(40);
        let plates: Vec<PlateBox> = (0..40)
            .map(|index| {
                plate(
                    index,
                    400.0 + index as f32 * 20.0,
                    300.0 + index as f32 * 4.0,
                )
            })
            .collect();
        layout.resolve(&plates, &constraints());
        layout.smooth(&plates, 1.0 / 60.0, DEFAULT_SMOOTHING);
        let capacity = layout.capacity();

        for _ in 0..1_000 {
            layout.resolve(&plates, &constraints());
            layout.smooth(&plates, 1.0 / 60.0, DEFAULT_SMOOTHING);
        }
        assert_eq!(layout.capacity(), capacity, "the layout reallocated");
        assert_eq!(layout.tracked.len(), plates.len(), "the tracker grew");
    }

    #[test]
    fn nonsense_input_produces_a_position_rather_than_a_panic() {
        let mut layout = NameplateLayout::new();
        let mut constraints = constraints();
        constraints.viewport = (0.0, 0.0);
        assert_eq!(layout.resolve(&[], &constraints).len(), 0);

        let plates = [plate(1, 10.0, 10.0), plate(2, 10.0, 10.0)];
        let placed = layout.resolve(&plates, &constraints).to_vec();
        assert!(placed.iter().all(|(x, y)| x.is_finite() && y.is_finite()));

        constraints.viewport = (1920.0, 1080.0);
        constraints.vertical_margin = -1.0;
        constraints.horizontal_margin = f32::NAN;
        constraints.top_safe_zone = 5.0;
        let placed = layout.resolve(&plates, &constraints).to_vec();
        assert!(placed.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
    }
}
