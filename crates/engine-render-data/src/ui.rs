//! What the UI wants drawn, as data.
//!
//! ```text
//! UiSnapshot -> layout -> UiDrawList -> renderer -> one vertex buffer -> one draw call
//!               (engine-presentation)              (engine-renderer)
//! ```
//!
//! A [`UiDrawList`] is a frame's worth of UI expressed as commands: rectangles, outlines and
//! runs of text, in the order they should be painted. It contains no vertex, no handle and
//! no GPU concept, which is what lets `engine-presentation` — a crate with no D3D12 — build
//! the entire HUD.
//!
//! ## Why commands and not vertices
//!
//! The obvious alternative is for the layout code to emit vertices directly. It was rejected
//! for one reason: expanding a string into glyph quads needs the font atlas, the atlas lives
//! beside the texture it is uploaded into, and moving it into a data crate would drag the
//! renderer's vertex layout across the seam with it. A command list keeps the shader ABI on
//! the renderer's side of the boundary, where every other piece of it already lives.
//!
//! ## Painter's order, not depth
//!
//! Commands are drawn **in the order they were pushed**, with the depth test off. Phase 3's
//! overlay gave every plate a depth value and let the depth buffer sort them, which works
//! for opaque quads and breaks the moment anything is translucent: a blended fragment that
//! fails the depth test is simply missing, and one that passes writes depth that occludes
//! whatever should have blended with it afterwards. Painter's order is the standard answer
//! for UI, and it makes "what is on top" a property of the code that pushed it rather than
//! of a float somebody has to keep in sync.
//!
//! ## Allocation
//!
//! The command vector and the text arena are cleared, never freed. After the first frame
//! that reaches a given size, building a list writes into storage that already exists.

/// A rectangle in pixels, measured from the top-left of the client area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiRect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width. A non-positive width draws nothing.
    pub width: f32,
    /// Height. A non-positive height draws nothing.
    pub height: f32,
}

impl UiRect {
    /// A rectangle from its top-left corner and size.
    #[must_use]
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether the rectangle has any area to draw.
    #[must_use]
    pub fn is_visible(self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }

    /// The right edge.
    #[must_use]
    pub fn right(self) -> f32 {
        self.x + self.width
    }

    /// The bottom edge.
    #[must_use]
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    /// Whether a point in the same coordinate space is inside this rectangle.
    ///
    /// Half-open: the left and top edges are inside, the right and bottom are not, so two
    /// rectangles that share an edge never both claim the same pixel — which is what stops
    /// two adjacent buttons from lighting up together.
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The same rectangle inset by `amount` on every side.
    ///
    /// Clamped at zero rather than inverting: an inset larger than the rectangle produces
    /// nothing to draw, which is what the caller meant, instead of a rectangle with negative
    /// size that would render inside out.
    #[must_use]
    pub fn inset(self, amount: f32) -> Self {
        let width = (self.width - amount * 2.0).max(0.0);
        let height = (self.height - amount * 2.0).max(0.0);
        Self {
            x: self.x + amount,
            y: self.y + amount,
            width,
            height,
        }
    }

    /// The left `fraction` of this rectangle, for a horizontal bar fill.
    #[must_use]
    pub fn fraction_of_width(self, fraction: f32) -> Self {
        Self {
            width: self.width * fraction.clamp(0.0, 1.0),
            ..self
        }
    }
}

/// Linear RGBA. Alpha is honoured — the UI pass blends.
pub type UiColor = [f32; 4];

/// Where a run of text sits in a [`UiDrawList`]'s arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange {
    /// Byte offset of the first character.
    pub start: u32,
    /// Byte offset one past the last character.
    pub end: u32,
}

/// Horizontal placement of a text run relative to its origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    /// The origin is the left edge.
    #[default]
    Left,
    /// The origin is the horizontal centre.
    Center,
    /// The origin is the right edge.
    Right,
}

/// One thing to draw.
///
/// `Copy`, so the renderer walks the list without touching the arena except for text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiCommand {
    /// A filled rectangle.
    Rect {
        /// Where.
        rect: UiRect,
        /// What colour, alpha included.
        color: UiColor,
    },
    /// A rectangular outline drawn inside `rect`.
    Outline {
        /// Where.
        rect: UiRect,
        /// How thick, in pixels.
        thickness: f32,
        /// What colour.
        color: UiColor,
    },
    /// A radial sector, swept clockwise from twelve o'clock.
    ///
    /// What a cooldown looks like in this genre. Expressed as a command rather than as a
    /// pile of triangles the layout code would have to build, so the sweep is one number
    /// the interface can hand over and the renderer decides how finely to tessellate it.
    RadialFill {
        /// The square the sector is inscribed in.
        rect: UiRect,
        /// How much of the circle is covered, in `0.0..=1.0`.
        fraction: f32,
        /// What colour.
        color: UiColor,
    },
    /// A run of text from the list's arena.
    Text {
        /// Where the run is anchored, in pixels.
        origin: (f32, f32),
        /// Which bytes of the arena to draw.
        range: TextRange,
        /// What colour.
        color: UiColor,
        /// Glyph scale. 1.0 is the font's native 5x7 cell size.
        scale: f32,
        /// How the run is placed relative to `origin`.
        align: TextAlign,
    },
}

/// A frame's worth of UI, in paint order.
#[derive(Debug, Default, Clone)]
pub struct UiDrawList {
    commands: Vec<UiCommand>,
    /// Text for every `Text` command, concatenated. Cleared, never freed.
    text: String,
}

impl UiDrawList {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty list with room for `commands` commands and `text` bytes.
    #[must_use]
    pub fn with_capacity(commands: usize, text: usize) -> Self {
        Self {
            commands: Vec::with_capacity(commands),
            text: String::with_capacity(text),
        }
    }

    /// Empties the list, keeping every allocation.
    pub fn clear(&mut self) {
        self.commands.clear();
        self.text.clear();
    }

    /// The commands, in paint order.
    #[must_use]
    pub fn commands(&self) -> &[UiCommand] {
        &self.commands
    }

    /// The text arena the `Text` commands index into.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The characters a `Text` command draws.
    ///
    /// Returns an empty string for a range that does not fall on character boundaries rather
    /// than panicking; a malformed range is a bug in the caller, not a reason to take the
    /// frame down.
    #[must_use]
    pub fn text_of(&self, range: TextRange) -> &str {
        self.text
            .get(range.start as usize..range.end as usize)
            .unwrap_or("")
    }

    /// How many commands the list holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether the list holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Appends a filled rectangle. A rectangle with no area is dropped.
    pub fn rect(&mut self, rect: UiRect, color: UiColor) {
        if !rect.is_visible() {
            return;
        }
        self.commands.push(UiCommand::Rect { rect, color });
    }

    /// Appends an outline drawn inside `rect`.
    ///
    /// An outline thicker than half the rectangle would have its four sides overlap into a
    /// filled block; it is clamped so a degenerate request draws a filled rectangle exactly
    /// once rather than four times over.
    pub fn outline(&mut self, rect: UiRect, thickness: f32, color: UiColor) {
        if !rect.is_visible() || thickness <= 0.0 {
            return;
        }
        let thickness = thickness.min(rect.width * 0.5).min(rect.height * 0.5);
        self.commands.push(UiCommand::Outline {
            rect,
            thickness,
            color,
        });
    }

    /// Appends a radial sector covering `fraction` of the circle inside `rect`.
    ///
    /// A fraction at or below zero draws nothing, which is the ordinary case for an ability
    /// that is ready: a cooldown overlay that is not cooling down should cost nothing.
    pub fn radial(&mut self, rect: UiRect, fraction: f32, color: UiColor) {
        if !rect.is_visible() || fraction <= 0.0 {
            return;
        }
        self.commands.push(UiCommand::RadialFill {
            rect,
            fraction: fraction.min(1.0),
            color,
        });
    }

    /// Appends a run of text anchored at `origin`.
    pub fn text_at(&mut self, origin: (f32, f32), text: &str, color: UiColor, scale: f32) {
        self.text_aligned(origin, text, color, scale, TextAlign::Left);
    }

    /// Appends a run of text with an explicit alignment.
    pub fn text_aligned(
        &mut self,
        origin: (f32, f32),
        text: &str,
        color: UiColor,
        scale: f32,
        align: TextAlign,
    ) {
        if text.is_empty() || scale <= 0.0 {
            return;
        }
        let start = self.text.len() as u32;
        self.text.push_str(text);
        let end = self.text.len() as u32;
        self.commands.push(UiCommand::Text {
            origin,
            range: TextRange { start, end },
            color,
            scale,
            align,
        });
    }

    /// Appends a bar: a background, then a fill covering `fraction` of it.
    ///
    /// One call because every bar in the HUD is the same two rectangles, and a bar whose
    /// fill and background were pushed by different code would eventually disagree about
    /// their inset.
    pub fn bar(&mut self, rect: UiRect, fraction: f32, background: UiColor, fill: UiColor) {
        self.rect(rect, background);
        let inner = rect.inset(1.0);
        self.rect(inner.fraction_of_width(fraction), fill);
    }

    /// Appends every command from `other`, rebasing its text ranges into this arena.
    ///
    /// Used to compose a frame from lists built by independent producers — the world-space
    /// layer and the screen-space layer — without either knowing about the other.
    pub fn append(&mut self, other: &Self) {
        let base = self.text.len() as u32;
        self.text.push_str(&other.text);
        self.commands.extend(other.commands.iter().map(|command| {
            if let UiCommand::Text {
                origin,
                range,
                color,
                scale,
                align,
            } = *command
            {
                UiCommand::Text {
                    origin,
                    range: TextRange {
                        start: range.start + base,
                        end: range.end + base,
                    },
                    color,
                    scale,
                    align,
                }
            } else {
                *command
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: UiColor = [1.0, 1.0, 1.0, 1.0];
    const BLACK: UiColor = [0.0, 0.0, 0.0, 1.0];

    #[test]
    fn commands_are_kept_in_the_order_they_were_pushed() {
        // Paint order *is* the sort. If this stops holding, "on top" stops meaning anything.
        let mut list = UiDrawList::new();
        list.rect(UiRect::new(0.0, 0.0, 10.0, 10.0), BLACK);
        list.text_at((1.0, 1.0), "HP", WHITE, 1.0);
        list.outline(UiRect::new(0.0, 0.0, 10.0, 10.0), 1.0, WHITE);

        assert_eq!(list.len(), 3);
        assert!(matches!(list.commands()[0], UiCommand::Rect { .. }));
        assert!(matches!(list.commands()[1], UiCommand::Text { .. }));
        assert!(matches!(list.commands()[2], UiCommand::Outline { .. }));
    }

    #[test]
    fn text_is_stored_once_and_addressed_by_range() {
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "DUMMY", WHITE, 1.0);
        list.text_at((0.0, 10.0), "100%", WHITE, 1.0);

        assert_eq!(list.text(), "DUMMY100%");
        let UiCommand::Text { range: first, .. } = list.commands()[0] else {
            panic!("the first command is text");
        };
        let UiCommand::Text { range: second, .. } = list.commands()[1] else {
            panic!("the second command is text");
        };
        assert_eq!(list.text_of(first), "DUMMY");
        assert_eq!(list.text_of(second), "100%");
    }

    #[test]
    fn a_rectangle_with_no_area_is_dropped_rather_than_recorded() {
        // A zero-width bar fill is the ordinary case at 0% health, and it must cost nothing.
        let mut list = UiDrawList::new();
        list.rect(UiRect::new(0.0, 0.0, 0.0, 10.0), WHITE);
        list.rect(UiRect::new(0.0, 0.0, 10.0, -1.0), WHITE);
        list.text_at((0.0, 0.0), "", WHITE, 1.0);
        assert!(list.is_empty());
    }

    #[test]
    fn an_outline_thicker_than_its_box_is_clamped_rather_than_inverted() {
        let mut list = UiDrawList::new();
        list.outline(UiRect::new(0.0, 0.0, 10.0, 6.0), 50.0, WHITE);
        let UiCommand::Outline { thickness, .. } = list.commands()[0] else {
            panic!("an outline was pushed");
        };
        assert_eq!(thickness, 3.0, "half the shorter side");
    }

    #[test]
    fn a_bar_is_a_background_and_a_fill_that_tracks_its_fraction() {
        let mut list = UiDrawList::new();
        list.bar(UiRect::new(10.0, 20.0, 100.0, 12.0), 0.25, BLACK, WHITE);

        assert_eq!(list.len(), 2);
        let UiCommand::Rect { rect: fill, .. } = list.commands()[1] else {
            panic!("the fill is a rect");
        };
        // Inset by one on each side, then a quarter of what is left.
        assert_eq!(fill.width, 98.0 * 0.25);
        assert_eq!(fill.x, 11.0);
        assert_eq!(fill.height, 10.0);
    }

    #[test]
    fn a_full_bar_never_overflows_and_an_empty_one_draws_only_its_background() {
        let mut list = UiDrawList::new();
        let rect = UiRect::new(0.0, 0.0, 50.0, 8.0);
        list.bar(rect, 5.0, BLACK, WHITE);
        let UiCommand::Rect { rect: fill, .. } = list.commands()[1] else {
            panic!("the fill is a rect");
        };
        assert_eq!(fill.width, 48.0, "a fraction above one is clamped");

        list.clear();
        list.bar(rect, 0.0, BLACK, WHITE);
        assert_eq!(list.len(), 1, "an empty fill is not recorded");
    }

    #[test]
    fn a_negative_fraction_is_clamped_rather_than_drawing_backwards() {
        let mut list = UiDrawList::new();
        list.bar(UiRect::new(0.0, 0.0, 50.0, 8.0), -1.0, BLACK, WHITE);
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn a_rectangle_claims_its_top_left_edges_and_not_its_bottom_right() {
        let rect = UiRect::new(10.0, 20.0, 100.0, 30.0);
        assert!(rect.contains(10.0, 20.0), "the top-left corner is inside");
        assert!(rect.contains(109.9, 49.9));
        assert!(!rect.contains(110.0, 30.0), "the right edge is outside");
        assert!(!rect.contains(50.0, 50.0), "the bottom edge is outside");
        assert!(!rect.contains(9.9, 30.0));

        // Two adjacent rectangles never both claim a point, which is what makes the rule
        // worth stating: hovering the seam lights exactly one of them.
        let next = UiRect::new(rect.right(), rect.y, 100.0, 30.0);
        for x in [109.0, 110.0, 111.0] {
            assert!(rect.contains(x, 25.0) ^ next.contains(x, 25.0), "{x}");
        }
    }

    #[test]
    fn an_inset_larger_than_the_rectangle_produces_nothing_to_draw() {
        let rect = UiRect::new(0.0, 0.0, 4.0, 4.0).inset(10.0);
        assert_eq!(rect.width, 0.0);
        assert_eq!(rect.height, 0.0);
        assert!(!rect.is_visible());
    }

    #[test]
    fn appending_rebases_text_ranges_into_the_receiving_arena() {
        // Two producers build independently; composing them must not make one read the
        // other's characters.
        let mut world = UiDrawList::new();
        world.text_at((0.0, 0.0), "DUMMY", WHITE, 1.0);

        let mut screen = UiDrawList::new();
        screen.text_at((0.0, 0.0), "PLAYER", WHITE, 1.0);
        screen.append(&world);

        assert_eq!(screen.len(), 2);
        let UiCommand::Text { range, .. } = screen.commands()[1] else {
            panic!("the appended command is text");
        };
        assert_eq!(screen.text_of(range), "DUMMY");
    }

    #[test]
    fn clearing_keeps_every_allocation() {
        // The property that makes a per-frame list free after the first frame.
        let mut list = UiDrawList::with_capacity(64, 512);
        for index in 0..64 {
            list.rect(UiRect::new(index as f32, 0.0, 4.0, 4.0), WHITE);
            list.text_at((0.0, 0.0), "DAMAGE", WHITE, 1.0);
        }
        let commands = list.commands.capacity();
        let text = list.text.capacity();

        for _ in 0..1_000 {
            list.clear();
            for index in 0..64 {
                list.rect(UiRect::new(index as f32, 0.0, 4.0, 4.0), WHITE);
                list.text_at((0.0, 0.0), "DAMAGE", WHITE, 1.0);
            }
        }

        assert_eq!(list.commands.capacity(), commands);
        assert_eq!(list.text.capacity(), text);
    }

    #[test]
    fn a_malformed_text_range_reads_empty_rather_than_panicking() {
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "HP", WHITE, 1.0);
        assert_eq!(list.text_of(TextRange { start: 0, end: 999 }), "");
    }
}
