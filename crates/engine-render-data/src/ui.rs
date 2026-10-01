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

/// How much bigger than its design size the interface is drawn.
///
/// ## Logical pixels and physical pixels
///
/// Every size in the layout is a **logical pixel**: the size the interface was designed at,
/// which is what a constant like `SLOT_SIZE = 48.0` means. Multiplying by a `UiScale` turns
/// it into a **physical pixel**, which is what a [`UiRect`] holds and what the renderer
/// draws. A layout therefore reads the same at every scale, and "the ability bar is 48 tall"
/// stays true in the only units a designer thinks in.
///
/// The process is per-monitor DPI aware, so physical pixels really are device pixels — a
/// 4K display gets four times as many of them as 1080p across the same panel, which is
/// exactly why this type exists. Nothing here reads the monitor: the scale is a **player
/// setting**, because how big a HUD should be is a preference, not a measurement.
///
/// ## Why a type and not an `f32`
///
/// So that "logical" and "physical" cannot be mixed up silently. A bare multiply is easy to
/// forget or to apply twice, and both mistakes render something that merely looks wrong.
/// [`UiScale::px`] is the only conversion, and it is named after what it produces.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct UiScale(f32);

impl Default for UiScale {
    fn default() -> Self {
        Self(1.0)
    }
}

impl UiScale {
    /// The smallest scale the renderer is built to draw correctly.
    ///
    /// Wider than the range the settings slider offers, deliberately: this is what the atlas
    /// and the layout can *support*, and a bound the player can reach should sit inside it
    /// rather than exactly on it. `engine-presentation` has a test that the offered range
    /// fits within this one.
    pub const MIN: f32 = 0.50;

    /// The largest scale the renderer is built to draw correctly.
    ///
    /// This is what sizes the icon art: an icon is rasterized large enough to be minified at
    /// every scale up to this one, and never magnified. Raising it means re-rasterizing.
    pub const MAX: f32 = 2.00;

    /// The scale the interface was designed at, where a logical pixel is a physical one.
    pub const DEFAULT: f32 = 1.00;

    /// A scale from a raw factor, clamped into `MIN..=MAX`.
    ///
    /// A non-finite factor becomes [`UiScale::DEFAULT`] rather than propagating: a NaN here
    /// would turn every position in the frame into a NaN, and the draw would silently vanish.
    #[must_use]
    pub fn new(factor: f32) -> Self {
        if !factor.is_finite() {
            return Self(Self::DEFAULT);
        }
        Self(factor.clamp(Self::MIN, Self::MAX))
    }

    /// The raw multiplier.
    #[must_use]
    pub fn factor(self) -> f32 {
        self.0
    }

    /// Converts a logical size to physical pixels.
    ///
    /// Rounded to a whole pixel. A HUD is axis-aligned rectangles, and a rectangle on a half
    /// pixel is a rectangle with a grey edge on two sides — the one artefact that makes a
    /// scaled interface look broken rather than merely bigger.
    #[must_use]
    pub fn px(self, logical: f32) -> f32 {
        (logical * self.0).round()
    }

    /// Converts a logical glyph scale to the scale the font is actually drawn at.
    ///
    /// **Snapped to a whole number, and never below one.** The font is a 5x7 bitmap sampled
    /// with a point filter, so it is crisp at integer scales and uneven at every other one —
    /// at 2.5x, two thirds of the rows are two pixels tall and the rest are three. Rounding
    /// means text stays sharp at every UI scale, at the cost of growing in steps rather than
    /// continuously, which is the right trade for a bitmap font.
    #[must_use]
    pub fn text(self, logical: f32) -> f32 {
        (logical * self.0).round().max(1.0)
    }
}

/// The largest physical size, in pixels, that any icon is ever drawn at.
///
/// The Ultimate button is the biggest thing that carries an icon at 66 logical pixels, and
/// [`UiScale::MAX`] is the most it can be multiplied by: `66 * 2 = 132`.
///
/// This is the number the icon art is rasterized to, so that every icon at every UI scale is
/// *minified* and never magnified — a picture scaled down stays sharp, one scaled up does
/// not. `engine-renderer` checks its rasterized size against this at compile time, and
/// `engine-presentation` has a test that no icon it lays out exceeds it.
pub const MAX_ICON_PIXELS: u32 = 132;

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

/// A picture the renderer can draw, named rather than described.
///
/// The art itself is an SVG under `engine-renderer/assets/icons/<class>`, rasterized into
/// the UI atlas at build time. This crate names the icons and nothing more: a layout crate can
/// ask for [`UiIcon::Meteor`] without knowing there is a texture, which is the same seam that
/// keeps [`UiCommand`] free of vertices.
///
/// Adding one means adding a variant here, a file under that class's folder, and a line in
/// that crate's `build.rs`. Two of those three fail to compile if the third is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiIcon {
    /// The filler bolt.
    Firebolt,
    /// The ground-targeted rock.
    Meteor,
    /// The cone that spreads Ignite.
    FlameCone,
    /// The spear thrown at one target.
    FlameLance,
    /// The big cast a full burn pays for.
    Pyroblast,
    /// The line of fire left burning on the ground.
    Firestorm,
    /// The payoff that spends the banked burn at once.
    Detonate,
    /// The specialisation cooldown.
    FireMastery,
    /// The defensive cooldown.
    FlameShield,
    /// The short teleport.
    Blink,

    // The Warrior's eleven. Cold steel where the mage's are fire, one warm amber the eye
    // lands on, and blood only where an ability actually draws it.
    /// The filler: a fist, and the mend coming off it.
    Recuperate,
    /// The targeted cleave: a boot down, and the ground answering.
    WarStomp,
    /// The maul at the bottom of its swing, and the bleed it leaves.
    ColossusStrike,
    /// The shield coming at you, and the ground going past it.
    ShieldCharge,
    /// The interrupt: an edge driven forward, and a cast coming apart.
    ShieldBash,
    /// The shield square on and braced. The one that does not move.
    ShieldBlock,
    /// A blade through the gauntlet, and the blood becoming a ward.
    PunctureBlood,
    /// A heart caged in steel, and the trace it is banking.
    Adrenaline,
    /// A helm lit from the inside. A state rather than a swing.
    BattleTrance,
    /// A bolt arriving, and the same bolt leaving.
    SpellReflect,
    /// A sword driven in, a shield set against it, and the light behind.
    LastStand,

    // The Druid's thirteen. Green where the mage's are fire and the Warrior's steel, with
    // moonlight and sun on the two that are not healing, and one violet for the resource.
    /// Moonlight left burning on one enemy.
    Moonfire,
    /// The same burn, spread wide: a sun over a pack.
    Sunfire,
    /// The filler, and the marks each cast of it stacks.
    Wrath,
    /// One shoot, put on one target and left to run.
    Rejuvenation,
    /// A bloom open all at once, over the shoot that follows it.
    Regrowth,
    /// A bud in three rings, counting down to the bloom.
    Lifebloom,
    /// Three shoots out of one ring: the party, not one target.
    WildGrowth,
    /// A heal over time coming apart, spent as a burst.
    Swiftmend,
    /// A leaf held open, filling from above.
    Nourish,
    /// The affliction lifted off the leaf, breaking up as it goes.
    NaturesCleanse,
    /// A print leaning into the run, and the ground going past.
    WildRush,
    /// The gauge full and holding. Nothing has been spent.
    AstralPower,
    /// A canopy, and the light coming down through it onto everyone.
    Tranquility,

    // Equipment. The character sheet draws these; nothing casts them.
    /// A helm.
    Helm,
    /// Shoulders.
    Shoulders,
    /// A chestpiece.
    Chestpiece,
    /// A cloak.
    Cloak,
    /// Gloves.
    Gloves,
    /// Leggings.
    Leggings,
    /// Boots.
    Boots,
    /// Bracers.
    Bracers,
    /// An amulet.
    Amulet,
    /// A ring.
    Ring,
    /// A trinket.
    Trinket,

    // The dungeon map: its markers, and the sidebar's portraits and ability schools.
    /// A monster pack on the dungeon map: crossed swords on gold.
    MapPack,
    /// A boss on the dungeon map: a skull on red.
    MapBoss,
    /// The player on the map: an arrow pointing the way they face.
    MapPlayer,
    /// North, on the map's frame.
    MapCompass,
    /// A monster's portrait in the map sidebar, tinted by its look.
    Monster,
    /// A monster ability that deals physical damage.
    AbilityPhysical,
    /// A monster ability that deals fire damage.
    AbilityFire,
    /// A monster ability that deals arcane damage.
    AbilityArcane,
    /// A monster ability that deals nature damage.
    AbilityNature,
    /// A monster ability that heals its allies.
    AbilityHeal,
    /// An elite or empowered pack on the map: a violet crest with a star.
    CrestElite,
    /// The dungeon's emblem in the map's header.
    EmblemCrypt,
    /// Map control: zoom in.
    ControlZoomIn,
    /// Map control: zoom out.
    ControlZoomOut,
    /// Map control: fit the whole level.
    ControlFit,
    /// Map control: centre on the player.
    ControlRecenter,
    /// Close a window.
    ControlClose,
    /// Help glyph: drag with the mouse.
    MouseDrag,
    /// Help glyph: turn the mouse wheel.
    MouseWheel,
    /// Help glyph: click.
    MouseClick,
    /// An empty loot table's chest.
    LootChest,
    /// Locked: the boss is sealed.
    Lock,
    /// Done: an encounter cleared.
    Check,
    /// Ability tag: slows, roots or moves its target.
    TagControl,
    /// Ability tag: can be interrupted.
    TagInterrupt,
    /// Ability tag: hits an area.
    TagArea,
    /// A disclosure chevron: pointing right folded, down open.
    ControlChevron,

    // Buffs and debuffs. An effect that *is* an ability — Rejuvenation, Shield Block — wears
    // that ability's icon; these are the ones that are not.
    /// A burn on the bearer: Ignite, or any fire left running.
    AuraBurning,
    /// The Warrior's bleed: three drops.
    AuraBleeding,
    /// What Adrenaline banked, healing back: a heart in a returning arrow.
    AuraRecovering,
    /// War Stomp's stacks: a breastplate split down the middle.
    AuraSundered,
    /// Astral Sunfire's mark: a sun in a violet halo.
    AuraAstralSunfire,
    /// Wrath's stacks: a leaf behind a broken ward.
    AuraNaturesExposure,
    /// Astral Wrath: a violet star throwing green sparks.
    AuraAstralWrath,
    /// A poison: a green drop and its bubbles.
    AuraPoisoned,
    /// Respawn protection: an hourglass still running.
    AuraRespawnProtection,
    /// A monster's bleed: three claw rakes.
    AuraRended,
    /// A slow: an arrow through the leg and the curse under it.
    AuraCrippled,
    /// A monster's frenzy: eyes and fangs.
    AuraFrenzy,
    /// A stun: stars circling.
    AuraStunned,
    /// A silence: a speech mark struck through.
    AuraSilenced,
    /// The Inferno State: a crown of three flames.
    AuraInferno,
    /// Puncture Blood's absorb: a shield filled with blood.
    AuraBloodWard,
    /// Molten ground's vulnerability: fire underfoot, and more of it taken.
    AuraScorched,
    /// Fortitude: a tower shield with a cross of light.
    AuraFortitude,
    /// Battle Shout: a war horn and the shout leaving it.
    AuraBattleShout,
    /// Arcane Intellect: an arcane star over an open tome.
    AuraArcaneIntellect,
    /// Bloodlust: a blood drop with speed behind it.
    AuraBloodlust,
    /// Regeneration: a leaf with a healing cross.
    AuraRegeneration,
    /// Curse of Weakness: a broken sword inside a hex.
    AuraCurseOfWeakness,
    /// Withering Plague: a spiked spore.
    AuraWitheringPlague,
    /// Magic Vulnerability: a cracked orb of warding.
    AuraMagicVulnerability,
}

impl UiIcon {
    /// Every icon, in atlas order.
    pub const ALL: [Self; 97] = [
        Self::Firebolt,
        Self::Meteor,
        Self::FlameCone,
        Self::FlameLance,
        Self::Pyroblast,
        Self::Firestorm,
        Self::Detonate,
        Self::FireMastery,
        Self::FlameShield,
        Self::Blink,
        Self::Recuperate,
        Self::WarStomp,
        Self::ColossusStrike,
        Self::ShieldCharge,
        Self::ShieldBash,
        Self::ShieldBlock,
        Self::PunctureBlood,
        Self::Adrenaline,
        Self::BattleTrance,
        Self::SpellReflect,
        Self::LastStand,
        Self::Moonfire,
        Self::Sunfire,
        Self::Wrath,
        Self::Rejuvenation,
        Self::Regrowth,
        Self::Lifebloom,
        Self::WildGrowth,
        Self::Swiftmend,
        Self::Nourish,
        Self::NaturesCleanse,
        Self::WildRush,
        Self::AstralPower,
        Self::Tranquility,
        Self::Helm,
        Self::Shoulders,
        Self::Chestpiece,
        Self::Cloak,
        Self::Gloves,
        Self::Leggings,
        Self::Boots,
        Self::Bracers,
        Self::Amulet,
        Self::Ring,
        Self::Trinket,
        Self::MapPack,
        Self::MapBoss,
        Self::MapPlayer,
        Self::MapCompass,
        Self::Monster,
        Self::AbilityPhysical,
        Self::AbilityFire,
        Self::AbilityArcane,
        Self::AbilityNature,
        Self::AbilityHeal,
        Self::CrestElite,
        Self::EmblemCrypt,
        Self::ControlZoomIn,
        Self::ControlZoomOut,
        Self::ControlFit,
        Self::ControlRecenter,
        Self::ControlClose,
        Self::MouseDrag,
        Self::MouseWheel,
        Self::MouseClick,
        Self::LootChest,
        Self::Lock,
        Self::Check,
        Self::TagControl,
        Self::TagInterrupt,
        Self::TagArea,
        Self::ControlChevron,
        Self::AuraBurning,
        Self::AuraBleeding,
        Self::AuraRecovering,
        Self::AuraSundered,
        Self::AuraAstralSunfire,
        Self::AuraNaturesExposure,
        Self::AuraAstralWrath,
        Self::AuraPoisoned,
        Self::AuraRespawnProtection,
        Self::AuraRended,
        Self::AuraCrippled,
        Self::AuraFrenzy,
        Self::AuraStunned,
        Self::AuraSilenced,
        Self::AuraInferno,
        Self::AuraBloodWard,
        Self::AuraScorched,
        Self::AuraFortitude,
        Self::AuraBattleShout,
        Self::AuraArcaneIntellect,
        Self::AuraBloodlust,
        Self::AuraRegeneration,
        Self::AuraCurseOfWeakness,
        Self::AuraWitheringPlague,
        Self::AuraMagicVulnerability,
    ];

    /// How many icons there are.
    pub const COUNT: usize = Self::ALL.len();

    /// Dense index into the atlas's icon grid.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }
}

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

/// How a [`UiCommand::StyledLabel`] is set: how heavy its strokes are and the dark edge
/// around them.
///
/// Both are the distance field's to give. Emboldening moves the edge outward by a number of
/// pixels, and the outline is the same glyph moved further out, drawn first in a dark
/// colour — so a heavier weight and a readable edge cost no new font file and no second
/// atlas. What text in the world needs is exactly this: numbers and names float over
/// torchlight, bright spell effects and black dungeon floors, and only a dark edge reads
/// against all three.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// The edge's colour, alpha included. Fully transparent draws no edge.
    pub outline: UiColor,
    /// How far the edge reaches past the glyph, in pixels.
    pub outline_width: f32,
    /// How far the edge is moved down, in pixels: a little makes the edge read as a shadow
    /// cast by the letter as well as a line around it.
    pub shadow_offset: f32,
    /// How far every stroke is thickened, in pixels. Zero is the face's own weight.
    pub embolden: f32,
}

impl TextStyle {
    /// No edge and the face's own weight: what a plain [`UiCommand::Label`] draws.
    pub const PLAIN: Self = Self {
        outline: [0.0, 0.0, 0.0, 0.0],
        outline_width: 0.0,
        shadow_offset: 0.0,
        embolden: 0.0,
    };

    /// A dark edge `width` pixels wide, nudged `shadow` pixels down, over strokes thickened by
    /// `embolden` pixels.
    #[must_use]
    pub fn outlined(outline: UiColor, width: f32, shadow: f32, embolden: f32) -> Self {
        let finite = |value: f32| {
            if value.is_finite() {
                value.max(0.0)
            } else {
                0.0
            }
        };
        Self {
            outline,
            outline_width: finite(width),
            shadow_offset: if shadow.is_finite() { shadow } else { 0.0 },
            embolden: finite(embolden),
        }
    }

    /// Whether an edge is drawn at all.
    #[must_use]
    pub fn has_outline(&self) -> bool {
        self.outline[3] > 0.0 && (self.outline_width > 0.0 || self.shadow_offset != 0.0)
    }

    /// This style with its edge faded by `alpha`, for text that is fading out.
    #[must_use]
    pub fn faded(mut self, alpha: f32) -> Self {
        let alpha = if alpha.is_finite() {
            alpha.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.outline[3] *= alpha;
        self
    }
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
    /// A radial sweep, measured clockwise from twelve o'clock, filling `rect`.
    ///
    /// What a cooldown looks like in this genre. Expressed as a command rather than as a
    /// pile of triangles the layout code would have to build, so the sweep is two numbers
    /// the interface hands over and the renderer decides how to tessellate it.
    ///
    /// The sweep runs to the edges of `rect`, corners included — it is not a circle
    /// inscribed in it. A full sweep therefore covers `rect` exactly, and covers nothing
    /// outside it.
    RadialFill {
        /// The rectangle the sweep fills.
        rect: UiRect,
        /// Where the sweep begins, in turns clockwise from twelve o'clock.
        start: f32,
        /// How much of the turn it covers, in `0.0..=1.0`.
        fraction: f32,
        /// What colour.
        color: UiColor,
    },
    /// A picture from the UI atlas, stretched to fill `rect`.
    Icon {
        /// Where.
        rect: UiRect,
        /// Which picture.
        icon: UiIcon,
        /// What to multiply it by. White draws the art untouched; a dimmer colour is how a
        /// slot that cannot be used says so without needing a second copy of the art.
        tint: UiColor,
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
    /// A run of text set in one of the interface's typefaces.
    ///
    /// The proportional counterpart of [`UiCommand::Text`]: Inter or Cinzel from a distance-field
    /// atlas, at any pixel size, measured with [`crate::font::UiFont`]'s own metrics so a layout
    /// can wrap and align it exactly. `Text` remains the 5 × 7 bitmap the rest of the HUD uses.
    Label {
        /// Where the run is anchored: `x` per `align`, `y` the top of the line box.
        origin: (f32, f32),
        /// Which bytes of the arena to draw.
        range: TextRange,
        /// What colour.
        color: UiColor,
        /// Pixels per em.
        size: f32,
        /// Which face.
        font: crate::font::UiFont,
        /// How the run is placed relative to `origin`.
        align: TextAlign,
    },
    /// A run of text in one of the interface's typefaces, thickened and edged: the combat
    /// text and the nameplates.
    ///
    /// The same glyphs as [`UiCommand::Label`], measured by the same metrics, so a layout that
    /// centres one centres the other. The renderer draws the whole run's edge first and the
    /// letters over it, so one letter's edge never cuts into its neighbour.
    StyledLabel {
        /// Where the run is anchored: `x` per `align`, `y` the top of the line box.
        origin: (f32, f32),
        /// Which bytes of the arena to draw.
        range: TextRange,
        /// What colour the letters are.
        color: UiColor,
        /// Pixels per em.
        size: f32,
        /// Which face.
        font: crate::font::UiFont,
        /// How the run is placed relative to `origin`.
        align: TextAlign,
        /// Weight and edge.
        style: TextStyle,
    },
    /// A rounded rectangle: filled with a vertical gradient, softened into a shadow, or
    /// stroked as an outline — the surface every panel, card, button and ring is drawn with.
    ///
    /// The shape is a signed distance evaluated per pixel, so the corners are anti-aliased at
    /// any size and a ring is a circle-radius rectangle stroked. `feather` blurs the edge over
    /// that many pixels (a drop shadow); `stroke`, when positive, draws only a band that wide
    /// inside the edge. A rectangular [`UiCommand::Clip`] applies; a circular one does not.
    Panel {
        /// The shape's bounds, before any feather.
        rect: UiRect,
        /// Corner radius, in pixels, clamped to half the shorter side.
        radius: f32,
        /// Colour at the top edge.
        top: UiColor,
        /// Colour at the bottom edge.
        bottom: UiColor,
        /// Pixels the edge is blurred over. Zero is a crisp edge.
        feather: f32,
        /// Width of an outline, in pixels. Zero fills the shape.
        stroke: f32,
        /// Whether the interior is left undrawn: a drop shadow under an opaque surface, whose
        /// middle nobody can see. Only the band where the shape is not solid is shaded, which
        /// is what keeps a window-sized shadow from costing a window's worth of fill.
        /// Outlines are always hollow.
        hollow: bool,
    },
    /// Confines every command after it to a region, until the next `Clip`.
    ///
    /// State in the stream rather than a field on each command, so the dozens of glyphs,
    /// markers and rows a map or a scrolling list draws do not each carry a copy. The renderer
    /// resolves it: a rectangle is cut on the CPU, exactly, texture coordinates and all; a
    /// circle is cut per pixel in the shader, anti-aliased, because a circle is not a shape a
    /// quad can be trimmed to. [`UiClip::NONE`] ends it.
    Clip(UiClip),
    /// A filled, anti-aliased circle.
    ///
    /// Its own circle is its shape, so it ignores a circular [`UiCommand::Clip`] region; a
    /// rectangular one still applies.
    Disc {
        /// Its centre, in pixels.
        center: (f32, f32),
        /// Its radius, in pixels.
        radius: f32,
        /// What colour.
        color: UiColor,
    },
    /// The map picture, sampled through an affine mapping from the screen to its texture.
    ///
    /// `rect` is where it is drawn. `origin` is the texture coordinate at the rectangle's
    /// top-left corner, and `du` and `dv` are how far the texture coordinate moves for one
    /// pixel right and one pixel down. A rotating minimap is a rotated `du`/`dv` on an
    /// unrotated rectangle; a panned, zoomed map is a shifted `origin` and a scaled pair.
    /// Texture coordinates outside `0..1` are transparent, so the region beyond the level is
    /// whatever was drawn underneath.
    ///
    /// The renderer owns which picture that is (`Renderer::set_ui_map_texture`); a frame with
    /// no map set draws nothing for this command.
    MapImage {
        /// Where, in pixels.
        rect: UiRect,
        /// The texture coordinate at `rect`'s top-left corner.
        origin: (f32, f32),
        /// Texture-coordinate change per pixel to the right.
        du: (f32, f32),
        /// Texture-coordinate change per pixel downward.
        dv: (f32, f32),
        /// What to multiply the picture by.
        tint: UiColor,
    },
    /// An icon turned about its centre: the player's arrow on the map.
    RotatedIcon {
        /// Its centre, in pixels.
        center: (f32, f32),
        /// Its edge, in pixels.
        size: f32,
        /// Which picture.
        icon: UiIcon,
        /// Clockwise turn on screen, in radians. Zero draws the icon as authored.
        rotation: f32,
        /// What to multiply it by.
        tint: UiColor,
    },
}

/// A circle, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiCircle {
    /// Its centre.
    pub center: (f32, f32),
    /// Its radius.
    pub radius: f32,
}

impl UiCircle {
    /// Whether a point is inside it.
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        let (dx, dy) = (x - self.center.0, y - self.center.1);
        dx * dx + dy * dy <= self.radius * self.radius
    }
}

/// Where a [`UiCommand::Clip`] confines what is drawn after it: a rectangle, a circle, both, or
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UiClip {
    /// Nothing outside this rectangle is drawn.
    pub rect: Option<UiRect>,
    /// Nothing outside this circle is drawn.
    pub circle: Option<UiCircle>,
}

impl UiClip {
    /// No clip at all.
    pub const NONE: Self = Self {
        rect: None,
        circle: None,
    };

    /// Clipped to a rectangle.
    #[must_use]
    pub fn rect(rect: UiRect) -> Self {
        Self {
            rect: Some(rect),
            circle: None,
        }
    }

    /// Clipped to a circle.
    #[must_use]
    pub fn circle(center: (f32, f32), radius: f32) -> Self {
        Self {
            rect: None,
            circle: Some(UiCircle { center, radius }),
        }
    }

    /// Whether this clip lets everything through.
    #[must_use]
    pub fn is_none(self) -> bool {
        self.rect.is_none() && self.circle.is_none()
    }

    /// Whether a point survives it.
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        self.rect.is_none_or(|rect| rect.contains(x, y))
            && self.circle.is_none_or(|circle| circle.contains(x, y))
    }
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

    /// Appends an icon filling `rect`, multiplied by `tint`.
    ///
    /// Pass an opaque white tint to draw the art as it was authored.
    pub fn icon(&mut self, rect: UiRect, icon: UiIcon, tint: UiColor) {
        if !rect.is_visible() {
            return;
        }
        self.commands.push(UiCommand::Icon { rect, icon, tint });
    }

    /// Appends a radial sweep covering `fraction` of a turn, filling `rect`.
    ///
    /// A fraction at or below zero draws nothing, which is the ordinary case for an ability
    /// that is ready: a cooldown overlay that is not cooling down should cost nothing.
    pub fn radial(&mut self, rect: UiRect, fraction: f32, color: UiColor) {
        self.radial_from(rect, 0.0, fraction, color);
    }

    /// Appends a radial sweep that begins `start` turns clockwise from twelve o'clock.
    ///
    /// What a cooldown wipe needs: the covered part shrinks by its *leading* edge sweeping
    /// clockwise, so the uncovered part grows from twelve o'clock the way every cooldown in
    /// the genre does. A full sweep covers the whole of `rect`, so an ability that has just
    /// been cast is entirely dimmed rather than dimmed with four bright corners.
    pub fn radial_from(&mut self, rect: UiRect, start: f32, fraction: f32, color: UiColor) {
        if !rect.is_visible() || fraction <= 0.0 {
            return;
        }
        self.commands.push(UiCommand::RadialFill {
            rect,
            start,
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

    /// Appends a run of text in `font` at `size` pixels per em, its line box's top at
    /// `origin.1`.
    pub fn label(
        &mut self,
        origin: (f32, f32),
        text: &str,
        color: UiColor,
        font: crate::font::UiFont,
        size: f32,
        align: TextAlign,
    ) {
        if text.is_empty() || size.is_nan() || size <= 0.0 {
            return;
        }
        let start = self.text.len() as u32;
        self.text.push_str(text);
        let end = self.text.len() as u32;
        self.commands.push(UiCommand::Label {
            origin,
            range: TextRange { start, end },
            color,
            size,
            font,
            align,
        });
    }

    /// Appends a run of text in `font` at `size` pixels per em, drawn in `style`: thickened
    /// and edged so it reads over anything the world puts behind it.
    #[allow(
        clippy::too_many_arguments,
        reason = "a styled run is this many independent facts, as a label is plus its style"
    )]
    pub fn styled_label(
        &mut self,
        origin: (f32, f32),
        text: &str,
        color: UiColor,
        font: crate::font::UiFont,
        size: f32,
        align: TextAlign,
        style: TextStyle,
    ) {
        if text.is_empty() || size.is_nan() || size <= 0.0 {
            return;
        }
        let start = self.text.len() as u32;
        self.text.push_str(text);
        let end = self.text.len() as u32;
        self.commands.push(UiCommand::StyledLabel {
            origin,
            range: TextRange { start, end },
            color,
            size,
            font,
            align,
            style,
        });
    }

    /// Appends a filled rounded rectangle, `top` fading to `bottom`.
    pub fn panel(&mut self, rect: UiRect, radius: f32, top: UiColor, bottom: UiColor) {
        self.panel_shape(rect, radius, top, bottom, 0.0, 0.0);
    }

    /// Appends a rounded outline `width` pixels wide, inside `rect`.
    pub fn panel_stroke(&mut self, rect: UiRect, radius: f32, width: f32, color: UiColor) {
        if width.is_nan() || width <= 0.0 {
            return;
        }
        self.panel_shape(rect, radius, color, color, 0.0, width);
    }

    /// Appends a soft shadow: `rect`'s rounded shape blurred over `feather` pixels.
    ///
    /// Hollow: its middle is not drawn, because something opaque is about to be drawn over
    /// it. For a soft shape that *is* seen whole — a halo, a glow — use [`UiDrawList::glow`].
    pub fn shadow(&mut self, rect: UiRect, radius: f32, feather: f32, color: UiColor) {
        self.panel_shape(rect, radius, color, color, feather.max(0.0), 0.0);
        if let Some(UiCommand::Panel { hollow, .. }) = self.commands.last_mut() {
            *hollow = true;
        }
    }

    /// Appends a soft filled shape: `rect`'s rounded shape blurred over `feather` pixels, drawn
    /// whole — a halo, a glow.
    pub fn glow(&mut self, rect: UiRect, radius: f32, feather: f32, color: UiColor) {
        self.panel_shape(rect, radius, color, color, feather.max(0.0), 0.0);
    }

    /// Appends a rounded rectangle with every option: see [`UiCommand::Panel`].
    pub fn panel_shape(
        &mut self,
        rect: UiRect,
        radius: f32,
        top: UiColor,
        bottom: UiColor,
        feather: f32,
        stroke: f32,
    ) {
        if !rect.is_visible() {
            return;
        }
        let radius = radius.max(0.0).min(rect.width * 0.5).min(rect.height * 0.5);
        self.commands.push(UiCommand::Panel {
            rect,
            radius,
            top,
            bottom,
            feather: if feather.is_finite() {
                feather.max(0.0)
            } else {
                0.0
            },
            stroke: if stroke.is_finite() {
                stroke.max(0.0)
            } else {
                0.0
            },
            hollow: false,
        });
    }

    /// Confines everything appended after this to `clip`, until the next call.
    ///
    /// Pass [`UiClip::NONE`] to end it. Whoever sets a clip ends it: a list handed to
    /// [`UiDrawList::append`] has its clip ended for it, so one producer's clip never reaches
    /// another's commands.
    pub fn clip(&mut self, clip: UiClip) {
        self.commands.push(UiCommand::Clip(clip));
    }

    /// Appends a filled, anti-aliased circle. One with no radius is dropped.
    pub fn disc(&mut self, center: (f32, f32), radius: f32, color: UiColor) {
        if radius.is_nan() || radius <= 0.0 {
            return;
        }
        self.commands.push(UiCommand::Disc {
            center,
            radius,
            color,
        });
    }

    /// Appends the map picture over `rect`, mapped by `origin`, `du` and `dv`.
    ///
    /// See [`UiCommand::MapImage`] for what the three mean.
    pub fn map_image(
        &mut self,
        rect: UiRect,
        origin: (f32, f32),
        du: (f32, f32),
        dv: (f32, f32),
        tint: UiColor,
    ) {
        if !rect.is_visible() {
            return;
        }
        self.commands.push(UiCommand::MapImage {
            rect,
            origin,
            du,
            dv,
            tint,
        });
    }

    /// Appends an icon turned `rotation` radians clockwise about `center`.
    pub fn icon_rotated(
        &mut self,
        center: (f32, f32),
        size: f32,
        icon: UiIcon,
        rotation: f32,
        tint: UiColor,
    ) {
        if size.is_nan() || size <= 0.0 || !rotation.is_finite() {
            return;
        }
        self.commands.push(UiCommand::RotatedIcon {
            center,
            size,
            icon,
            rotation,
            tint,
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
        let clipped = other
            .commands
            .iter()
            .any(|command| matches!(command, UiCommand::Clip(_)));
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
            } else if let UiCommand::Label {
                origin,
                range,
                color,
                size,
                font,
                align,
            } = *command
            {
                UiCommand::Label {
                    origin,
                    range: TextRange {
                        start: range.start + base,
                        end: range.end + base,
                    },
                    color,
                    size,
                    font,
                    align,
                }
            } else if let UiCommand::StyledLabel {
                origin,
                range,
                color,
                size,
                font,
                align,
                style,
            } = *command
            {
                UiCommand::StyledLabel {
                    origin,
                    range: TextRange {
                        start: range.start + base,
                        end: range.end + base,
                    },
                    color,
                    size,
                    font,
                    align,
                    style,
                }
            } else {
                *command
            }
        }));
        if clipped {
            self.commands.push(UiCommand::Clip(UiClip::NONE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_appended_list_s_clip_ends_with_it() {
        let mut inner = UiDrawList::new();
        inner.clip(UiClip::circle((5.0, 5.0), 4.0));
        inner.disc((5.0, 5.0), 2.0, WHITE);
        let mut outer = UiDrawList::new();
        outer.append(&inner);
        outer.rect(UiRect::new(0.0, 0.0, 2.0, 2.0), WHITE);
        let commands = outer.commands();
        assert_eq!(commands[2], UiCommand::Clip(UiClip::NONE));
        assert!(matches!(commands[3], UiCommand::Rect { .. }));
    }

    #[test]
    fn a_clip_contains_what_is_inside_both_of_its_shapes() {
        let clip = UiClip {
            rect: Some(UiRect::new(0.0, 0.0, 10.0, 10.0)),
            circle: Some(UiCircle {
                center: (0.0, 0.0),
                radius: 5.0,
            }),
        };
        assert!(clip.contains(2.0, 2.0));
        assert!(
            !clip.contains(6.0, 6.0),
            "inside the rectangle, outside the circle"
        );
        assert!(
            !clip.contains(-1.0, 0.0),
            "inside the circle, outside the rectangle"
        );
        assert!(UiClip::NONE.contains(1e6, -1e6));
    }

    #[test]
    fn degenerate_discs_icons_and_images_are_dropped() {
        let mut list = UiDrawList::new();
        list.disc((0.0, 0.0), 0.0, WHITE);
        list.disc((0.0, 0.0), f32::NAN, WHITE);
        list.icon_rotated((0.0, 0.0), 0.0, UiIcon::MapPlayer, 0.0, WHITE);
        list.icon_rotated((0.0, 0.0), 8.0, UiIcon::MapPlayer, f32::NAN, WHITE);
        list.map_image(
            UiRect::new(0.0, 0.0, 0.0, 5.0),
            (0.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            WHITE,
        );
        assert!(list.is_empty());
    }

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
    fn an_icon_records_which_picture_it_wants_and_nothing_about_a_texture() {
        // The seam: a layout crate names an icon, and the renderer is the only thing that
        // knows one is a region of an atlas.
        let mut list = UiDrawList::new();
        let rect = UiRect::new(10.0, 20.0, 44.0, 44.0);
        list.icon(rect, UiIcon::Meteor, WHITE);

        assert_eq!(list.len(), 1);
        let UiCommand::Icon {
            rect: drawn,
            icon,
            tint,
        } = list.commands()[0]
        else {
            panic!("an icon was pushed");
        };
        assert_eq!(drawn, rect);
        assert_eq!(icon, UiIcon::Meteor);
        assert_eq!(tint, WHITE);

        // A rectangle with no area is dropped, like every other command.
        list.clear();
        list.icon(UiRect::new(0.0, 0.0, 0.0, 44.0), UiIcon::Meteor, WHITE);
        assert!(list.is_empty());
    }

    #[test]
    fn every_icon_has_a_distinct_index_that_matches_its_place_in_all() {
        // The atlas lays icons out by this index; two icons sharing one would put two
        // pictures in the same cell.
        for (position, icon) in UiIcon::ALL.into_iter().enumerate() {
            assert_eq!(icon.index(), position, "{icon:?}");
        }
        assert_eq!(UiIcon::COUNT, UiIcon::ALL.len());
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
